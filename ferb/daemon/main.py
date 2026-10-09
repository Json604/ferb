"""Own daemon composition, lifecycle and RPC handlers. Must not interpret requests or select targets."""

import asyncio
import json
from datetime import datetime, timedelta
import os
import signal
import subprocess
import sys
import time
from pathlib import Path
from typing import Any, AsyncIterator, Awaitable, Callable, NamedTuple

from ferb.bus import DROPPED, Bus, Overflow
from ferb.clock.clock import SystemClock
from ferb.config import Config
from ferb.daemon.gateway import FsRoots, Gateway
from ferb.orchestrator.supervisor import Supervisor
from ferb.orchestrator.verify import Checks
from ferb.orchestrator.tree import TaskTree
from ferb.act.browser.tools import IdentityDriver
from ferb.core.types import Identity
from ferb.act.browser import downloads, profiles as browser_profiles
from ferb.act.browser.ext_bridge import FerbIdentity, Hub, Router, WakeServer
from ferb.act.browser.ext_protocol import FERB_EXTENSION_ID
from ferb.act.evidence.store import EvidenceStore
from ferb.act.fs import recovery
from ferb.act.browser.pairing import Pairings
from ferb.act.browser.resolver import Resolver
from ferb.act.gui_lock import GuiLock
from ferb.act.mac.driver import MacComputerDriver, profiles_dir
from ferb.act.mac.profile import Profiles
from ferb.act.mac.input import RealInput
from ferb.act.shell.manager import ShellManager
from ferb.registry import add_context_readers, add_recording, add_evidence, add_recovery, add_identity_browser, add_mac, add_owner_browsers, add_shell, add_time, add_trail
from ferb.sense.watchers.owner_input import OwnerInput
from ferb.sense.exclusions import DEFAULT_EXCLUDE_PATHS, Exclusions
from ferb.sense.manager import Sense
from ferb.sense.trail import Attribution, Trail
from ferb.act.fs.files import normalize_path
from ferb.clock.context import ago as context_ago
from ferb.registry import build as build_registry
from ferb.runtimes.proxy import ToolListProxy
from ferb.core import origin as origin_
from ferb.core.errors import FerbError
from ferb.core.ids import RandomIds
from ferb.core.interfaces import Clock, IdSource
from ferb.daemon.auth import RunTokens
from ferb.daemon.server import Server, Session, ensure_socket_free
from ferb.memory.journal import Journal
from ferb.orchestrator import episodes
from ferb.orchestrator.ledger import Ledger
from ferb.orchestrator.stop import StopControl
from ferb.trace import compose
from ferb.paths import ferb_home, socket_path
from ferb.orchestrator.tree import ExecutionState
from ferb.identity.secrets import KeychainSecretStore, specs_from_config
from ferb.sanitize.redact import Redactor
from ferb.store import retention
from ferb.store.db import Database
from ferb.store.spans import SpanStore

Redact = Callable[[Any], Any]

# `ferb logs` filter flags -> bus topic patterns
LOG_FILTERS = {"tasks": "task.*", "trail": "trail.event", "tools": "tool.*", "browsers": "browser.*",
               "actions": "action.*", "timers": "timer.*",
               "voice": "voice.*"}
MACOS_TICK_S = 0.2


def _provider(name: str, model: str) -> Any:
    if name == "anthropic":
        from ferb.runtimes.anthropic_api import AnthropicProvider
        return AnthropicProvider(model)
    if name == "openai":
        from ferb.runtimes.openai_api import OpenAIProvider
        return OpenAIProvider(model)
    from ferb.runtimes.google_api import GoogleProvider
    return GoogleProvider(model)


ROOTS_PER_PURGE = 200
RESTORE_LIST = 100                                # `ferb restore` lists the newest this many records


class _PurgePlan(NamedTuple):
    """One retention pass: its clock, its windows, the roots it handles and how many wait for the next pass."""
    now: datetime
    trace_days: int
    decision_days: int
    evidence_days: int
    recovery_days: int
    roots: list[str]
    left: int


class Daemon:
    def __init__(self, home: Path | None = None, clock: Clock | None = None, ids: IdSource | None = None,
                 redact: Redact | None = None, owner_uid: int | None = None,
                 watch_owner_input: bool = True, watchers: bool = True, distill: bool = True,
                 notifications: bool = True, retention: bool = True, snapshots: bool = True) -> None:
        self.home = home or ferb_home()
        self.clock = clock or SystemClock()
        self.ids = ids or RandomIds()
        self.config = Config(self.home)
        self._redact_override = redact
        self.redact: Redact = redact or Redactor()
        from ferb.sense.daily import DailyActivity
        self.daily_activity = DailyActivity(self.home / 'activity' / 'today.jsonl', self.clock.now)
        self.bus = Bus(self.clock)
        self.tokens = RunTokens(self.clock)
        self.server = Server(socket_path(self.home), self.tokens, owner_uid)
        self.state = ExecutionState(self.clock)
        self.stop_control = StopControl(self.tokens.revoke_all, self._set_blocked)
        self.db: Database | None = None
        self.journal: Journal | None = None
        self.spans: SpanStore | None = None
        self.ledger: Ledger | None = None
        self.evidence: EvidenceStore | None = None           # captured state the checker re-inspects (V1), 7 days
        self._macos_task: asyncio.Task[None] | None = None
        self.secrets = KeychainSecretStore(lambda: specs_from_config(self.config.load().get("secrets")))
        self.identity_browser: IdentityDriver | None = None    # the extension in Chrome's FERB profile
        from ferb.act.recording import RecordingManager
        self.recordings = RecordingManager()
        self.registry = build_registry()
        add_recording(self.registry, self.recordings)
        self.tasks = TaskTree()
        self.gateway: Gateway | None = None
        self._available_cache: dict[str, tuple[float, bool]] = {}
        self.supervisor: Supervisor | None = None
        self._proxy: ToolListProxy | None = None
        # Owner browser profiles through the extension. Tests override the data dirs and the launcher.
        self.browser_data_dirs: dict[str, Path] | None = None
        self.browser_launch: Callable[[list[str]], None] | None = None
        self.pair_load_wait_s = 2.0                          # a profile opened just to pair gets this long to load
        self.pairings: Pairings | None = None
        self.hub: Hub | None = None
        self.router: Router | None = None
        self.ferb_identity: FerbIdentity | None = None
        self.downloads: downloads.Downloads | None = None
        self.wake = WakeServer()
        # The owner's Mac + shell
        self.watch_owner_input = watch_owner_input
        self.owner_input = OwnerInput()
        from ferb.daemon.disturbance import DesktopProbe, DisturbanceSentinel
        self.disturbance = DisturbanceSentinel(DesktopProbe(), self._record_disturbance,
            owner_input=lambda: {"monitored": self.owner_input.running,
                                 "recent": self.owner_input.active(),
                                 "last_event_wall": self.owner_input.last_owner_wall})
        self.voice: Any = None                              # push-to-talk, built at start
        self.voice_speaker: Any = None                      # tests replace FERB's voice
        self._ptt = False
        self.gui = GuiLock()
        self.mac: MacComputerDriver | None = None
        self.shells: ShellManager | None = None
        # Watchers + trail. Tests turn the real watchers off (the pipeline still runs).
        self.run_watchers = watchers
        self.trail: Trail | None = None
        self.sense: Sense | None = None
        self.attribution = Attribution()
        self.watch_problems: list[str] = []
        self._excl: tuple[float, Exclusions] | None = None
        # Memory. Tests turn the daily distillation loop off (it calls a model).
        self.run_distill = distill
        self.memory: Any = None
        self.distiller: Any = None
        self._distill_task: asyncio.Task[None] | None = None
        self.run_retention = retention
        self._retention_task: asyncio.Task[None] | None = None
        self.take_snapshots = snapshots                  # S1 local APFS snapshots; tests never take a real one
        self.snapshots: recovery.Snapshots | None = None
        # Deletes FERB's own old snapshots in the async retention pass (None: macOS thinning only; tests)
        self.snapshot_deleter: Callable[[str], Awaitable[bool]] | None = (
            recovery.delete_local_snapshot if snapshots else None)
        # Jev in shadow mode: None until a key is in the Keychain
        self.jev: Any = None
        self.jev_post: Any = None                        # tests: a fake Jev endpoint
        # Chat: turns outlive a closed chat; which task trees have a chat attached
        self._shutting_down = False
        self._turns: set[asyncio.Task[None]] = set()
        self._attached_trees: set[str] = set()
        self._held: list[tuple[str, str]] = []               # notifications waiting for the owner to be back (13)
        self.schedules: Any = None                            # durable schedules
        self._runs: set[asyncio.Task[None]] = set()
        self._held_task: asyncio.Task[None] | None = None
        # Timers
        self.timers: Any = None
        self._timer_task: asyncio.Task[None] | None = None
        self._sleepers: dict[str, asyncio.Event] = {}
        self.notify: Callable[[str, str], Awaitable[None]] = self._osascript_notify if notifications else _no_notify

    @property
    def stop_marker(self) -> Path:
        return self.home / "STOPPED"

    async def start(self) -> None:
        self.home.mkdir(parents=True, exist_ok=True)
        os.chmod(self.home, 0o700)
        ensure_socket_free(self.server.path)   # refuse before touching the database if another daemon runs
        self.config.ensure_privacy_defaults()
        self._load_config()
        self.db = Database(self.home / "ferb.db")
        self.db.migrate()
        self.journal = Journal(self.db, self.clock, self.ids, self.redact)
        self.spans = SpanStore(self.db, self.clock, self.ids, self.redact)
        self.evidence = EvidenceStore(self.home / "evidence", self.clock, self.ids, self.redact, self.db)
        self.snapshots = recovery.Snapshots(recovery.local_snapshot if self.take_snapshots else None, self.db,
                                            self.clock, self.ids, self.redact)
        add_recovery(self.registry, self.snapshots)
        self.ledger = Ledger(self.db, self.clock, self.ids)
        self.tasks.attach(self.db)
        interrupted = self.tasks.interrupt_running(self.clock.now())          # a restart: what was running didn't finish
        if interrupted:
            self.journal.append("tasks.interrupted_after_restart", {"task_ids": interrupted})
            for tid in {t.split(".", 1)[0] for t in interrupted}:          # chat tells the owner next time
                goal = next((r["goal"] for r in self.tasks.rows(tid) if r["id"] == tid), "")
                self.db.conn.execute("INSERT OR IGNORE INTO task_results (task_id, at, status, goal, answer, shown) "
                                     "VALUES (?, ?, 'interrupted', ?, ?, 0)",
                                     (tid, self.clock.now().isoformat(), goal,
                                      f"This was interrupted when FERB restarted: {goal}. Want me to pick it "
                                      "up again?"))
                # Its permanent record (L1), from what it recorded before the restart. A failure never stops
                # start-up: episodes.record notes it, or leaves it to retention when even the note fails.
                episodes.record(self.spans, self.journal, tid, self.clock.now().isoformat())
        await self._build_browsers()
        self._build_mac_and_shell()
        self._build_agents()
        await self._build_sense()
        self._build_memory()
        await self._build_jev()
        self._build_timers()
        for name, fn in (("ping", self._ping), ("sense.pause", self._sense_pause),
                         ("schedules.list", self._schedules_list), ("schedules.change", self._schedules_change),
                         ("chat.status", self._chat_status), ("chat.attach", self._chat_attach),
                         ("task.stop", self._task_stop),
                         ("reminders.list", self._reminders_list), ("reminders.cancel", self._reminders_cancel),
                         ("reminders.unseen", self._reminders_unseen),
                         ("memory.list", self._memory_list), ("memory.show", self._memory_show),
                         ("memory.add", self._memory_add), ("memory.confirm", self._memory_confirm),
                         ("memory.edit", self._memory_edit), ("memory.retract", self._memory_retract),
                         ("memory.forget", self._memory_forget), ("memory.distill", self._memory_distill),
                         ("jev.stats", self._jev_stats), ("jev.enable", self._jev_enable), ("jev.reload", self._jev_reload), ("runtimes.check", self._runtimes_check),
                         ("timeline.get", self._timeline),
                         ("recovery.list", self._recovery_list), ("recovery.restore", self._recovery_restore),
                         ("sense.resume", self._sense_resume), ("sense.status", self._sense_status), ("trace.get", self._trace), ("verdict.set", self._verdict_set), ("stop.engage", self._stop_engage),
                         ("stop.lift", self._stop_lift),
                         ("doctor.check", self._doctor_check),
                         ("automation.request", self._automation_request), ("tools.list", self._tools_list),
                         ("tools.call", self._tools_call), ("chat.approve", self._chat_approve),
                         ("chat.answer", self._chat_answer),
                         ("identity.check", self._identity_check), ("browsers.list", self._browsers_list),
                         ("browsers.pair", self._browsers_pair), ("browsers.installed", self._browsers_installed), ("browsers.unpair", self._browsers_unpair),
                         ("identity.pair", self._identity_pair), ("ps.list", self._ps_list),
                         ("ps.kill", self._ps_kill)):
            self.server.unary[name] = fn
        self.server.streams["logs.subscribe"] = self._logs
        self.server.streams["chat.send"] = self._chat_send
        self._build_voice()
        await self.server.start()          # refuses to start if another daemon answers on the socket
        self.journal.append("daemon.started", {"pid": os.getpid()})
        unknown = self.ledger.mark_unknown_after_restart()
        if unknown:
            self.journal.append("ledger.unknown_after_restart", {"intent_ids": unknown})
        if self.stop_marker.exists():      # a stop outlives a crash/restart until the owner lifts it
            self.state.stopped = True
            self.stop_control.engaged = True
            self.journal.append("stop.still_engaged", {})
        self._macos_task = asyncio.create_task(self._macos_events())

    async def shutdown(self) -> None:
        self._shutting_down = True
        current = asyncio.current_task()

        async def drain(jobs: set[asyncio.Task[Any]], family: str) -> None:
            pending = [job for job in jobs if job is not current and not job.done()]
            for job in pending:
                job.cancel()
            results = await asyncio.gather(*pending, return_exceptions=True)
            for result in results:
                if isinstance(result, BaseException) and not isinstance(result, asyncio.CancelledError) and self.journal:
                    self.journal.append("daemon.shutdown_failed", {"family": family,
                        "error": f"{type(result).__name__}: {result}"})

        # Stop task producers before taking the task snapshot. Task finalizers still need the
        # gateway resources, disturbance observer and database until cancellation has completed.
        await drain({job for job in (self._macos_task, self._distill_task, self._retention_task, self._timer_task,
                                self._held_task)
                     if job is not None}, "producers")
        if self.supervisor:
            self.supervisor.cancel_all()
        await drain(self._turns | self._runs, "task turns")
        if self.gateway is not None:                         # first looks still being taken (D26)
            await drain(set(self.gateway.looks), "first looks")
        if self._proxy:
            self._proxy.close()
        await self.recordings.shutdown()
        if self.identity_browser is not None:
            await self.identity_browser.shutdown()
        if self.router is not None:
            await self.router.shutdown()
        if self.shells is not None:
            await self.shells.kill_all()
        if self.jev is not None:
            self.jev.close()
        if self.sense is not None:
            await self.sense.stop()
        if self.owner_input.running:
            self.owner_input.stop()
        await self.disturbance.shutdown()
        await self.wake.close()
        await self.server.close()
        if self.journal:
            self.journal.append("daemon.stopped", {"pid": os.getpid()})
        if self.db:
            self.db.close()

    async def run(self) -> None:
        """Run until SIGTERM / SIGINT, then shut down cleanly."""
        await self.start()
        loop = asyncio.get_running_loop()
        done = asyncio.Event()
        for sig in (signal.SIGTERM, signal.SIGINT):
            loop.add_signal_handler(sig, done.set)
        await done.wait()
        await self.shutdown()

    # ---------- policy plumbing ----------

    def _set_blocked(self, blocked: bool) -> None:
        self.state.stopped = blocked
        if blocked:
            self.stop_marker.touch(mode=0o600)
        else:
            self.stop_marker.unlink(missing_ok=True)

    def _build_voice(self) -> None:
        from ferb.cli.chat import question, render
        from ferb.voice.controller import Voice
        from ferb.voice.speak import SaySpeaker
        self.voice = Voice(self._voice_turn, self.state.approvals.pending, self._resolve_approval,
                           self.voice_speaker or SaySpeaker(),
                           self._journal(), self._launch_voice_app,
                           mode=lambda: str(self.config.get("voice", "mic", "auto")), render=render, question=question,
                           publish=self._publish,
                           enabled=lambda: self._ptt, sounds=lambda: bool(self.config.get("voice", "sounds", True)))
        loop = asyncio.get_running_loop()

        def on_key(code: int, down: bool) -> None:               # the tap's thread: hand off to the loop
            if self._ptt:
                loop.call_soon_threadsafe(self.voice.on_key, code, down)
        self.owner_input.on_key = on_key
        self.server.unary["voice.transcript"] = self._voice_transcript
        self.server.unary["voice.reload"] = self._voice_reload
        self.server.unary["voice.switch"] = self._voice_switch
        self.server.streams["voice.attach"] = self._voice_attach
        self.server.voice_app = self._voice_app_exe
        self.server.voice_signed = self._voice_signed
        if self._ptt:
            self._launch_voice_app()

    @property
    def voice_bundle(self) -> Path:
        return self.home / "FERB.app"

    def _voice_app_exe(self) -> str | None:
        exe = self.voice_bundle / "Contents" / "MacOS" / "FERB"
        return str(exe) if self._ptt and exe.exists() else None

    def _voice_signed(self, exe: str) -> bool:
        """FERB.app's signature meets the requirement recorded by `ferb setup --voice` (bundle id + certificate)."""
        req = self.config.get("voice", "requirement")
        if not req:
            return False
        r = subprocess.run(["codesign", "--verify", f"-R={req}", str(self.voice_bundle)], capture_output=True)
        return r.returncode == 0

    def _launch_voice_app(self) -> None:
        if self.voice_bundle.exists():
            subprocess.Popen(["open", "-g", str(self.voice_bundle)], stdout=subprocess.DEVNULL,
                             stderr=subprocess.DEVNULL)

    async def _voice_turn(self, text: str) -> AsyncIterator[dict[str, Any]]:
        async for ev in self._chat_send(None, {"session_id": "voice", "text": text}):     # type: ignore[arg-type]
            yield ev

    async def _voice_attach(self, session: Session, params: dict[str, Any]) -> AsyncIterator[dict[str, Any]]:
        """FERB.app connects and waits for commands: start (with the chosen mic), stop, discard."""
        q = self.voice.attach(list(params.get("devices", [])), dict(params.get("permissions", {})))
        try:
            while True:
                yield await q.get()
        finally:
            self.voice.detach(q)

    async def _voice_transcript(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        if params.get("error"):                                  # FERB.app couldn't listen
            self._journal().append("voice.failed", {"why": str(params["error"])})
            self.voice._say("I couldn't use the microphone.")
            return {"handled": "error"}
        result: dict[str, Any] = await self.voice.transcript(str(params.get("text", "")))
        return result

    async def _voice_switch(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        """`ferb voice on|off`. Off: the setting, FERB stops speaking, FERB.app quits. On: needs `ferb setup --voice`
        to have recorded FERB.app's signature."""
        on = bool(params.get("on"))
        if on and not self.config.get("voice", "requirement"):
            return {"needs_setup": True, "push_to_talk": False}
        self.config.set("voice", "push_to_talk", on)
        out = await self._voice_reload(session, {})
        if not on:
            self._quit_voice_app()
        self._journal().append("voice.switched", {"on": on})
        return out

    def _quit_voice_app(self) -> None:
        from AppKit import NSRunningApplication
        for app in NSRunningApplication.runningApplicationsWithBundleIdentifier_("com.ferb.app"):
            app.terminate()

    async def _voice_reload(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        """`ferb setup --voice` finished: read the voice settings again and open FERB.app (no daemon restart)."""
        self._load_config()
        if self._ptt:
            self._launch_voice_app()
        elif self.voice is not None:
            self.voice.speaker.stop()                     # off: stop speaking replies to voice requests now
        return {"push_to_talk": self._ptt}

    def _load_config(self) -> None:
        self._ptt = bool(self.config.get("voice", "push_to_talk", False))
        if self._redact_override is None:
            domains, apps = set(self.config.get("privacy", "exclude_domains", [])), set(self.config.exclude_apps())
            if isinstance(self.redact, Redactor):      # the journal, spans and gateway hold this instance: keep it
                self.redact.configure(domains, apps)
            else:
                self.redact = Redactor(domains, apps)

    async def _macos_events(self) -> None:
        """Give macOS's event system a turn on the daemon's thread, a few times a second. Without it, its running-app
        list never refreshes in the daemon: an app launched after the daemon started stayed "not running" for FERB
        (owner's WhatsApp report, 2026-10-01). One zero-wait turn takes under a millisecond."""
        try:
            import CoreFoundation as CF
        except ImportError:
            return
        while True:
            CF.CFRunLoopRunInMode(CF.kCFRunLoopDefaultMode, 0, True)
            await asyncio.sleep(MACOS_TICK_S)

    def _build_agents(self) -> None:
        device = str(self.config.get("device", "id", "unknown-device"))
        workspace = self.home / "workspace"
        workspace.mkdir(parents=True, exist_ok=True)
        # FERB's downloads folder is always outside content (D35, MP2): code adds it, config can't remove it.
        roots = FsRoots(str(workspace), tuple(self.config.get("fs", "owner_roots", [])),
                        downloads.untrusted_roots(self.config.get("fs", "untrusted_roots", ["~/Downloads"]), workspace))
        assert self.ledger and self.spans and self.journal
        self.gateway = Gateway(self.registry.capabilities, self.state, self.ledger, self.spans, self.tasks,
                               self.clock, self.redact, self._publish, roots, device)
        self.gateway.disturbance = self.disturbance
        self.gateway.evidence = self.evidence
        self.gateway.recovery = self._recovery()
        self.gateway.snapshots = self.snapshots
        self.gateway.app_window = self._app_window
        self.supervisor = Supervisor(make_runtime=self._make_runtime, gateway=self.gateway, state=self.state,
                                     tokens=self.tokens, tasks=self.tasks, spans=self.spans, journal=self.journal,
                                     publish=self._publish, clock=self.clock, ids=self.ids, device_id=device,
                                     workspace=str(workspace), on_violation=self._runtime_violation,
                                     on_task_end=self._task_end, facts=self._facts,
                                     checks=Checks(self._urls_of, self.spans), roots=roots)
        self.supervisor.browser_context = self._browser_context
        self.supervisor.on_task_start = self.disturbance.start
        self.supervisor.on_task_finish = self.disturbance.finish
        self.supervisor.disturbance_context = self.disturbance.context
        self.supervisor.relevant_context = self._relevant_context
        add_context_readers(self.registry, self.supervisor.continuity,
                            lambda task_id: (rec.tree_id if (rec := self.tasks.get(task_id)) else None),
                            lambda: self.memory)
        self.supervisor.worker_types = lambda: ['default', *[n for n in self.registry.runtimes if self._runtime_available(n)]]
        self.gateway.route_question = self.supervisor.route_question
        self.stop_control.register("runtimes", self.supervisor.cancel_all)
        self.stop_control.register("identity_browser", self._stop_identity_browser)

    def _record_disturbance(self, task_id: str, kind: str, data: dict[str, Any]) -> None:
        """Persist observations before broadcast; tool arguments stay in their referenced action span."""
        if self.spans is None:
            return
        task = self.tasks.get(task_id)
        span = self.spans.start(kind, task_id=task_id, parent_id=task.span_id if task else None,
                                input={"operation": "observe desktop"})
        self.spans.finish(span, output=data)
        if kind in ("disturbance.change", "disturbance.error"):
            self._publish("disturbance.observed", {"task_id": task_id, "span_id": span.id, **data})

    def use_identity_browser(self, driver: IdentityDriver) -> None:
        """Attach FERB's own browser: registers the browser.* tools; its tabs close with their task and on stop."""
        self.identity_browser = driver
        add_identity_browser(self.registry, driver, self.recordings)

    async def _task_end(self, task_id: str) -> list[dict[str, Any]]:
        resources = await self.recordings.end_task(task_id)
        for name, drv in (("ferb_browser", self.identity_browser), ("browser", self.router)):
            if drv is not None:
                try:
                    resources.extend(await drv.release(task_id) or [])
                except Exception as exc:
                    resources.append({'resource': name, 'released': False, 'error': f"{type(exc).__name__}: {exc}"})
        if (kept := getattr(self, "downloads", None)) is not None:
            kept.forget_task(task_id)                    # after its tabs are let go of: their locks and unreported facts
        if self.shells is not None:
            try:
                await self.shells.end_task(task_id)
            except Exception as exc:
                resources.append({'resource': 'shell', 'released': False, 'error': f"{type(exc).__name__}: {exc}"})
        return resources

    def _stop_identity_browser(self) -> None:
        asyncio.get_event_loop().create_task(self.recordings.shutdown())
        for drv in (self.identity_browser, self.router):
            if drv is not None:
                asyncio.get_event_loop().create_task(drv.shutdown())
        if self.shells is not None:
            asyncio.get_event_loop().create_task(self.shells.kill_all())   # kills task-owned processes

    def _unpaired_note(self, browser: str) -> str | None:
        """The FERB extension is installed and connected in this browser but not paired."""
        if self.hub is None or browser not in self.hub.unpaired.values() or self._owner_extension_connected(browser):
            return None
        name = {"brave": "Brave", "chrome": "Chrome"}.get(browser, browser)
        return (f"The FERB extension is installed in {name} but not paired. Tell the owner to pair it: "
                f"`ferb browsers pair {browser} \"<profile name>\"` (or say yes when ferb chat offers it).")

    def _owner_extension_connected(self, browser: str) -> bool:
        """A paired owner profile of this browser is connected: browser work goes through the extension."""
        return self.hub is not None and any(br.conn is not None and br.binding.browser == browser
                                            and br.binding.identity is Identity.OWNER for br in self.hub.bridges.values())

    async def _web_password(self, browser: str, bounds: tuple[float, float, float, float]) -> bool | None:
        """Ask each connected profile of this browser whether its window at these bounds shows a password field in
        its active tab; the profile that owns the window answers. None: no profile could tell."""
        for br in list(self.hub.bridges.values()) if self.hub else []:
            if br.conn is None or br.binding.browser != browser:
                continue
            try:
                r = await br.call("window_password", {"bounds": list(bounds)}, timeout=4)
            except FerbError:
                continue
            if isinstance(r, dict) and r.get("matched"):
                p = r.get("password")
                return p if isinstance(p, bool) else None
        return None

    def _build_mac_and_shell(self) -> None:
        device = str(self.config.get("device", "id", "unknown-device"))
        def owner_wait(holder: str, on: bool) -> None:          # the owner's time never counts as a stall
            if self.supervisor is not None:
                self.supervisor.activity.waiting(holder, on)
        def available() -> bool:                                  # paused: no owner-input tap, so no real input
            return not (self.sense is not None and self.sense.paused)
        def owner_active() -> bool:                    # foreground input always yields when the owner provides input
            return self.owner_input.active()
        self.mac = MacComputerDriver(device, RealInput(owner_active, self.gui, on_wait=owner_wait,
                                                       available=available),
                                     excluded=self.config.exclude_apps, web_password=self._web_password,
                                     profiles=Profiles(profiles_dir(self.home)))   # what landed per app, learned
        self.mac.snapshot_images = lambda: bool(self.config.get("mac", "snapshot_image", True))   # image + tree
        add_mac(self.registry, self.mac, self._device_profile, self.recordings)
        self.shells = ShellManager(self.home / "logs" / "shell", self._redact_text,
                                   stream_factory=getattr(self.redact, "stream", None))
        add_shell(self.registry, self.shells)
        add_trail(self.registry, lambda: self.trail)
        add_evidence(self.registry, self.spans, self._inspection_parent,
                     lambda task_id: (rec.tree_id if (rec := self.tasks.get(task_id)) else None), self.evidence)
        add_time(self.registry, lambda: self.timers, self.clock.now, lambda: self._sleep_until,
                 lambda task_id: (rec.task.deadline if (rec := self.tasks.get(task_id)) else None))

    # ---------- watchers + trail ----------

    def _exclusions(self) -> Exclusions:
        """[privacy] lists, re-read when config.toml changes (the owner may edit it while FERB runs)."""
        try:
            mtime = self.config.path.stat().st_mtime
        except OSError:
            mtime = 0.0
        if self._excl is None or self._excl[0] != mtime:
            self._excl = (mtime, Exclusions.build(self.config.exclude_apps(),
                                                  self.config.get("privacy", "exclude_domains", []),
                                                  self.config.get("privacy", "exclude_paths", DEFAULT_EXCLUDE_PATHS)))
        return self._excl[1]

    def _watch_folders(self) -> list[Path]:
        from ferb.sense.watchers.files import valid_folders
        ok, self.watch_problems = valid_folders(list(self.config.get("sense", "watch_folders", [])), self._exclusions())
        for problem in self.watch_problems:
            self._journal().append("sense.folder_skipped", {"why": problem})
        return ok

    async def _build_sense(self) -> None:
        from ferb.sense.watchers.app_focus import AppFocusWatcher
        from ferb.sense.watchers.app_installs import AppInstallsWatcher
        from ferb.sense.watchers.browser_tabs import ScriptedTabsWatcher
        from ferb.sense.watchers.files import FilesWatcher
        from ferb.sense.watchers.idle import IdleWatcher
        from ferb.sense.watchers.windows import WindowTitleWatcher
        assert self.db is not None
        a = self.attribution
        a.shell_cwds = lambda: [(p.cwd, p.owner_task) for p in (self.shells.list() if self.shells else [])
                                if p.running]
        self.trail = Trail(self.db, self.clock, self.ids, self.redact, self._exclusions, self._publish,
                           lambda: str(self.config.get("device", "id", "unknown-device")),
                           lambda: self.sense is not None and self.sense.paused, a)
        if self.mac is not None:
            self.mac.on_activate = a.ferb_activated
        if self.gateway is not None:
            self.gateway.on_path = a.ferb_touched
        if self.hub is not None:
            self.hub.on_tab = self._on_tab
            self.hub.paused = lambda: self.sense is not None and self.sense.paused
            self.hub.owned = lambda br, tab_id: self._tab_task(br, tab_id) is not None
        if self.supervisor is not None:
            self.supervisor.signals = self.signals
        watchers: list[Any] = []
        if self.run_watchers:
            watchers = [AppFocusWatcher(), WindowTitleWatcher(lambda: bool(self.config.get("sense", "window_titles",
                                                                                           True))),
                        IdleWatcher(lambda: self.owner_input.last_owner_wall, lambda: self.owner_input.running,
                                    float(self.config.get("sense", "idle_after_s", 120))),
                        FilesWatcher(self._watch_folders), AppInstallsWatcher(self._app_changed),
                        ScriptedTabsWatcher(self._automation_granted, self._any_extension_connected)]
        self.sense = Sense(self.home, self.trail, watchers, self.owner_input if self.watch_owner_input else None,
                           on_pause=self._on_pause, on_fail=self._watcher_failed)
        await self.sense.start()
        if self.sense.paused:
            self._journal().append("sense.still_paused", {})
        elif self.watch_owner_input and not self.owner_input.running:
            self._journal().append("owner_input.unavailable", {"why": "no event tap (Accessibility / Input Monitoring)"})

    def _watcher_failed(self, source: str, why: str) -> None:
        self._journal().append("sense.watcher_failed", {"source": source, "error": why})

    def _tab_task(self, br: Any, tab_id: int) -> str | None:
        for driver in (self.router, self.ferb_identity):
            owner = driver.owner_of(br, tab_id) if driver is not None else None
            if owner is not None:
                return owner
        return None

    def _on_tab(self, br: Any, e: Any) -> None:
        from ferb.sense.watchers.browser_tabs import tab_observation
        if self.trail is None:
            return
        o = tab_observation(br.binding.browser, br.binding.folder, br.binding.identity is Identity.FERB, e.event,
                            e.url, e.title, e.incognito, self._tab_task(br, e.tab_id))
        if o is not None:
            self.trail.record(o)

    def _on_pause(self, paused: bool) -> None:
        if paused and self.hub is not None:
            self.hub.forget_owner_tabs()
        self._journal().append("sense.paused" if paused else "sense.resumed", {})
        self._publish("sense.paused" if paused else "sense.resumed", {})

    def _any_extension_connected(self, browser: str) -> bool:
        return self.hub is not None and any(br.conn is not None and br.binding.browser == browser
                                            for br in self.hub.bridges.values())

    _automation_cache: dict[str, tuple[float, bool]] = {}

    def _automation_granted(self, bundle_id: str) -> bool:
        """Already granted by the owner; never asks (the prompt would appear out of nowhere)."""
        import time as _t
        from ferb.device import permissions
        hit = self._automation_cache.get(bundle_id)
        if hit and _t.monotonic() - hit[0] < 60:
            return hit[1]
        ok = permissions.automation(bundle_id, ask=False) is permissions.Status.GRANTED
        self._automation_cache[bundle_id] = (_t.monotonic(), ok)
        return ok

    def _app_changed(self, change: str, info: Any, path: str) -> None:
        """An app appeared or disappeared: rescan that app only (Info.plist) into the device profile."""
        from ferb.device.profile import ProfileStore
        did = str(self.config.get("device", "id", "") or "")
        store = ProfileStore(self.home)
        profile = store.load(did) if did else None
        if profile is not None:
            apps = [x for x in profile.apps if x.path != path and (info is None or x.bundle_id != info.bundle_id)]
            if info is not None:
                apps.append(info)
            profile.apps = sorted(apps, key=lambda x: x.bundle_id)
            store.save(profile, f"app {change}: {info.bundle_id if info else Path(path).name}")
        self._publish("device.changed", {"change": change, "path": path,
                                         "bundle_id": info.bundle_id if info else None})

    def signals(self) -> tuple[datetime | None, datetime | None]:
        """For the time header: owner last active (owner-input tap), last trail event."""
        last = self.owner_input.last_owner_wall
        owner = datetime.fromtimestamp(last).astimezone() if last else None
        return owner, self.trail.last_event_at if self.trail else None

    async def _sense_pause(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        assert self.sense is not None
        await self.sense.pause()
        return {"paused": True}

    async def _sense_resume(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        assert self.sense is not None
        await self.sense.resume()
        return {"paused": False, "watchers": self.sense.running}

    async def _sense_status(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        s = self.sense
        return {"paused": bool(s and s.paused), "watchers": s.running if s else [],
                "failed": dict(s.failed) if s else {},
                "owner_input": self.owner_input.running, "folder_problems": self.watch_problems}

    # ---------- memory ----------

    def _build_memory(self) -> None:
        from ferb.memory import summary
        from ferb.memory.distill import Distiller
        from ferb.memory.items import Memory
        from ferb.memory.recall import context_block
        assert self.db is not None and self.journal is not None
        self.memory = Memory(self.db, self.clock, self.ids, self._publish, on_change=self._export_memory,
                             redact=self._redact_text)
        self.memory.on_forget = lambda plan: self.daily_activity.forget_sources(set(plan.trail))
        journal = self.journal
        self.memory.rebuild_summary = lambda day: summary.rebuild(self.db, journal, day)
        self.distiller = Distiller(self.db, self.journal, self.memory, self._make_runtime, self.clock,
                                   purge=self._purge_trail, device_id=str(self.config.get("device", "id", "dev")),
                                   record_usage=self._record_distill_usage)
        if self.supervisor is not None:
            self.supervisor.apps = self._installed_apps
            self.supervisor.pasted_is_data = lambda: bool(self.config.get("policy", "pasted_text_is_data", True))
            self.supervisor.environment = self._environment
            self.supervisor.availability = self._availability
            self.supervisor.recall = lambda words: context_block(self.memory, words)
            self.supervisor.remember = self._remember
        if self.run_distill:
            self._distill_task = asyncio.get_running_loop().create_task(self._distill_loop())
        if self.run_retention:
            self._retention_task = asyncio.get_running_loop().create_task(self._retention_loop())

    def _purge_records(self) -> dict[str, int]:
        """D21: spans of tasks that ended more than trace_days ago (once their episode exists) and loose spans;
        Jev decisions older than decision_days; evidence (screenshots, snapshots, recordings) older than
        evidence_days (V1, D20: 7); recovery copies and records older than recovery_days (S1: 7). Never the task list,
        episodes, journal or memory. One synchronous pass (tests, call sites); the hourly loop runs the same steps in
        `_purge_pass`."""
        plan = self._purge_plan()
        return self._purge_rest(plan, sum(self._purge_root(root, plan.now) for root in plan.roots))

    async def _purge_pass(self) -> dict[str, int]:
        """The retention pass on the daemon loop: the same steps as `_purge_records`, yielding between roots. If
        shutdown cancels it between roots, what it already purged is still journaled (`stopped`)."""
        plan = self._purge_plan()
        spans = done = 0
        finished = False
        try:
            for root in plan.roots:
                spans += self._purge_root(root, plan.now)
                done += 1
                await asyncio.sleep(0)
            finished = True
        finally:
            if not finished and spans:
                self._journal_purged(plan, spans, 0, 0, 0, plan.left + len(plan.roots) - done, stopped=True)
        result = self._purge_rest(plan, spans)               # journaled before the slow snapshot step
        result["snapshots"] = await self._purge_snapshots(plan)
        return result

    async def _purge_snapshots(self, plan: _PurgePlan) -> int:
        """FERB's own snapshots older than recovery_days (S1): each deletion takes seconds (measured 15.5 s) and is
        journaled as it happens, so shutdown can cancel the pass at any point without losing the record; a tmutil
        run already started is left to finish on its own."""
        kept = self._recovery()

        def gone(name: str, deleted: bool) -> None:
            self._journal().append("retention.snapshot_purged", {"snapshot": name, "deleted": deleted,
                                                                 "recovery_days": plan.recovery_days})
        return await recovery.purge_snapshots((plan.now - timedelta(days=plan.recovery_days)).isoformat(), kept.db,
                                              self.snapshot_deleter, gone)

    def _purge_plan(self) -> _PurgePlan:
        now = self.clock.now()
        trace_days = retention.days(self.config.get("retention", "trace_days", 30))          # each at least 1 day
        decision_days = retention.days(self.config.get("retention", "decision_days", 30))
        evidence_days = retention.days(self.config.get("retention", "evidence_days", 7))
        recovery_days = retention.days(self.config.get("retention", "recovery_days", 7))
        roots = retention.finished_roots_before(self.db, (now - timedelta(days=trace_days)).isoformat())
        return _PurgePlan(now, trace_days, decision_days, evidence_days, recovery_days,
                          roots[:ROOTS_PER_PURGE], max(0, len(roots) - ROOTS_PER_PURGE))   # oldest first; the rest wait

    def _purge_root(self, root: str, now: datetime) -> int:
        """One root: its spans go only once its episode exists (built now if missing). Isolated: a failure is
        journaled and the pass goes on."""
        try:
            if episodes.get(self.db, root) is None:          # build it while the spans still exist
                episodes.record(self.spans, self._journal(), root, now.isoformat())
            if episodes.get(self.db, root) is None:
                return 0                                     # no episode: keep the spans
            return retention.purge_task_spans(self.db, root)
        except Exception as e:
            self._journal().append("retention.failed", {"task_id": root, "error": f"{type(e).__name__}: {e}"})
            return 0

    def _purge_rest(self, plan: _PurgePlan, spans: int) -> dict[str, int]:
        """Loose spans, old Jev decisions, old evidence and old recovery copies and records (a trash record stays while
        its item is in the Trash, TRASH_RECORD_MAX_DAYS at most); snapshots are the async pass's last step.
        `retention.purged` is journaled only when the pass purged something or left roots for the next one."""
        spans += retention.purge_loose_spans(self.db, (plan.now - timedelta(days=plan.trace_days)).isoformat())
        decisions = retention.purge_decisions(self.db, (plan.now - timedelta(days=plan.decision_days)).isoformat())
        evidence = self.evidence.purge_before((plan.now - timedelta(days=plan.evidence_days)).isoformat()) \
            if self.evidence is not None else 0
        kept = self._recovery()
        copies = recovery.purge_before(
            (plan.now - timedelta(days=plan.recovery_days)).isoformat(), kept.db, kept.root,
            trash_backstop_iso=(plan.now - timedelta(days=recovery.TRASH_RECORD_MAX_DAYS)).isoformat())
        if spans or decisions or evidence or copies or plan.left:
            self._journal_purged(plan, spans, decisions, evidence, copies, plan.left)
        trashed = self._purge_downloads()
        return {"spans": spans, "decisions": decisions, "evidence": evidence, "recovery": copies, "downloads": trashed}

    def _purge_downloads(self) -> int:
        """D35: FERB's downloaded working copies older than retention.download_days, or past browser.download_total_mb
        (oldest first), go to the Trash; their records stay. Journaled when any moved."""
        if self.downloads is None:
            return 0
        try:
            trashed = self.downloads.purge()
        except Exception as e:
            self._journal().append("retention.failed", {"downloads": True, "error": f"{type(e).__name__}: {e}"})
            return 0
        if trashed:
            self._journal().append("retention.downloads", {"trashed": trashed,
                                                           "download_days": self.downloads.limits().keep_days})
        return trashed

    def _journal_purged(self, plan: _PurgePlan, spans: int, decisions: int, evidence: int, copies: int, left: int,
                        stopped: bool = False) -> None:
        self._journal().append("retention.purged", {"trace_days": plan.trace_days, "spans": spans,
                                                    "decision_days": plan.decision_days, "decisions": decisions,
                                                    "evidence_days": plan.evidence_days, "evidence": evidence,
                                                    "recovery_days": plan.recovery_days, "recovery": copies,
                                                    "roots_left": left} | ({"stopped": True} if stopped else {}))

    def _gateway(self) -> Gateway:
        if self.gateway is None:
            raise FerbError("daemon not started")
        return self.gateway

    def _recovery(self) -> recovery.Recovery:
        """S1: where FERB keeps copies of files before it changes them (~/.ferb/recovery) and their records."""
        if self.db is None:
            raise FerbError("daemon not started")
        return recovery.Recovery(self.home / "recovery", self.db, self.clock, self.ids)

    async def _retention_loop(self) -> None:
        """Hourly, starting a minute after start-up so start-up stays quick."""
        await asyncio.sleep(60)
        while True:
            try:
                await self._purge_pass()
            except Exception as e:
                self._journal().append("retention.failed", {"error": f"{type(e).__name__}: {e}"})
            await asyncio.sleep(3600)

    def _record_distill_usage(self, runtime: str, data: dict[str, Any]) -> None:
        span = self.spans.start("model.usage", task_id=None, parent_id=None, input={"runtime": runtime, "role": "distill"})
        self.spans.finish(span, output=data, cost_usd=data.get("cost_usd"))

    def _purge_trail(self) -> Any:
        if self.trail is None:
            return None
        purged, refused = self.trail.purge(int(self.config.get("sense", "trail_retention_days", 30)))
        if purged:
            self._journal().append("trail.purged", {"days": purged})
        return purged, refused

    async def _distill_loop(self) -> None:
        """Hourly: summarize and distill every past day not done yet, then purge (summarized days only)."""
        while True:
            try:
                assert self.distiller is not None
                await self.distiller.catch_up()
            except asyncio.CancelledError:
                raise
            except Exception as e:                             # never takes the daemon down; tried again next hour
                self._journal().append("memory.distill_failed", {"error": f"{type(e).__name__}: {e}"})
            await asyncio.sleep(3600)

    def _export_memory(self) -> None:
        from ferb.memory.export import export
        if self.memory is not None:
            export(self.memory, self.home / "memory")

    def _remember(self, items: list[Any], own_words: str, source_id: str) -> list[str]:
        """Chat capture (owner_stated): only when the quote is really the owner's own words (not pasted text)."""
        norm = " ".join(own_words.lower().split())
        saved = []
        for it in items:
            quote = " ".join(str(it.quote).lower().split())
            if len(quote) < 3 or quote not in norm:
                self._journal().append("memory.capture_refused", {"key": it.key, "why": "quote not in the owner's words"})
                continue
            try:
                m = self.memory.propose(it.kind, it.subject, it.key, it.content, "owner_stated",
                                        [("journal", source_id)], 1.0)
            except FerbError as e:
                self._journal().append("memory.capture_refused", {"key": it.key, "why": str(e)})
                continue
            saved.append(f"remembered ({m.id}): {m.content}")
        return saved

    def _item_dict(self, it: Any, conflict: bool = False) -> dict[str, Any]:
        return {"id": it.id, "kind": it.kind, "subject": it.subject, "key": it.key, "content": it.content,
                "origin": it.origin, "status": it.status, "valid_from": it.valid_from.isoformat(),
                "valid_to": it.valid_to.isoformat() if it.valid_to else None, "supersedes": it.supersedes,
                "sources": it.source_ids, "evidence_days": self.memory.evidence_days(it.id), "conflict": conflict}

    async def _memory_list(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        status = params.get("status", "active")
        items = self.memory.items(None if status == "all" else status, params.get("subject"),
                                  valid_now=not params.get("history"))
        return {"items": [self._item_dict(i) for i in items]}

    async def _memory_show(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        subject = str(params["subject"])
        items = self.memory.items(None, subject, valid_now=False)
        return {"subject": subject, "items": [self._item_dict(i) for i in items]}

    async def _memory_add(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        from ferb.memory.items import slug
        content = str(params["content"]).strip()
        entry = self._journal().append("memory.added", {"content": content})
        it = self.memory.propose(str(params.get("kind") or "fact"), str(params.get("subject") or "owner"),
                                 str(params.get("key") or slug(content, 30)), content, "owner_stated",
                                 [("journal", entry.id)], 1.0)
        return self._item_dict(it)

    async def _memory_confirm(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        return self._item_dict(self.memory.confirm(str(params["id"])))

    async def _memory_edit(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        entry = self._journal().append("memory.edited", {"id": params["id"], "content": params["content"]})
        return self._item_dict(self.memory.edit(str(params["id"]), str(params["content"]), ("journal", entry.id)))

    async def _memory_retract(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        self.memory.retract(str(params["id"]), str(params.get("reason", "the owner")))
        return {"retracted": params["id"]}

    async def _memory_forget(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        keep = bool(params.get("keep_sources"))
        if not params.get("confirm"):
            plan = self.memory.plan_forget(str(params["id"]), keep)
            return {"confirmed": False, "will_delete": plan.describe(self.db)}
        plan = self.memory.forget(str(params["id"]), keep)
        return {"confirmed": True, "deleted": plan.describe(self.db)}

    async def _memory_distill(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        """Distill one day now (owner / tests); normally the hourly loop does it."""
        assert self.distiller is not None
        return dict(await self.distiller.run_day(str(params["day"])))

    async def _timeline(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        since = datetime.fromisoformat(params["since"]).astimezone() if params.get("since") else None
        rows = self._journal().entries(since)
        return {"entries": [{"at": e.at.isoformat(), "kind": e.kind, "data": e.data} for e in rows]}

    # ---------- timers ----------

    def _build_timers(self) -> None:
        from ferb.clock.timers import Timers
        assert self.db is not None
        from ferb.orchestrator.schedules import Schedules
        self.timers = Timers(self.db, self.clock, self.ids, self._timer_fired)
        self.timers.cancel_all_sleeps()
        self.schedules = Schedules(self.db, self.clock, self.ids)
        if self.supervisor is not None:
            self.supervisor.on_schedule = self._schedule_create
            self.supervisor.on_schedule_change = self._schedule_change_from_chat
            self.supervisor.on_reminder_cancel = self._reminder_cancel_from_chat
        if self.supervisor is not None:
            self.supervisor.timers = self.timers
        self._timer_task = asyncio.get_running_loop().create_task(self.timers.run())
        self._held_task = asyncio.get_running_loop().create_task(self._flush_held())
    async def _timer_fired(self, t: Any) -> None:
        """The consumer. Sleeps wake their worker; reminders reach the owner once (notification, chat, logs,
        journal), even if a crash made the timer fire again."""
        from ferb.clock.timers import mark_handled
        if self._shutting_down:
            return
        if t.kind == "sleep":
            ev = self._sleepers.get(t.id)
            if ev is not None:
                ev.set()
            return
        if t.kind == "schedule":
            from ferb.clock.timers import mark_handled as handled
            assert self.db is not None
            if handled(self.db, t.id, self.clock.now()):                 # exactly once, even after a crash
                run = asyncio.create_task(self._run_schedule(t.message, t.overdue_s or 0))
                self._runs.add(run)
                run.add_done_callback(self._runs.discard)
            return
        assert self.db is not None
        c = self.db.conn
        c.execute("BEGIN")
        try:
            if not mark_handled(self.db, t.id, self.clock.now()):
                c.execute("COMMIT")
                return                                               # already delivered before a crash
            self._journal().append("timer.delivered", {"timer_id": t.id, "kind": t.kind, "message": t.message,
                                                       "overdue_s": round(t.overdue_s or 0)})
            c.execute("COMMIT")
        except Exception:
            c.execute("ROLLBACK")
            raise
        late = f" (due {round((t.overdue_s or 0) / 60)} min ago)" if (t.overdue_s or 0) >= 60 else ""
        self._publish("timer.fired", {"timer_id": t.id, "kind": t.kind, "message": t.message,
                                      "overdue_by": round(t.overdue_s or 0)})
        try:
            await self._notify("FERB reminder" if t.kind == "reminder" else "FERB timer", f"{t.message}{late}",
                               urgent=True)                     # a reminder is for now
        except Exception:
            pass                                                     # chat, logs and the journal still have it

    async def _osascript_notify(self, title: str, text: str) -> None:
        """A macOS notification (the OS's own, not FERB UI). Text is redacted and quoted for AppleScript."""
        def q(v: str) -> str:
            return '"' + str(self._redact_text(v)).replace("\\", "\\\\").replace('"', '\\"') + '"'
        proc = await asyncio.create_subprocess_exec("osascript", "-e", f"display notification {q(text)} with title "
                                                    f"{q(title)}", stdout=asyncio.subprocess.DEVNULL,
                                                    stderr=asyncio.subprocess.DEVNULL)
        await asyncio.wait_for(proc.wait(), 10)

    async def _sleep_until(self, worker_id: str, task_id: str, at: datetime) -> str:
        """time.sleep_until: a durable sleep timer; waiting on it is the owner's kind of wait (not a stall)."""
        assert self.timers is not None
        t = self.timers.add("sleep", at, "", task_id=task_id)
        ev = self._sleepers[t.id] = asyncio.Event()
        if self.supervisor is not None:
            self.supervisor.activity.waiting(worker_id, True)
        try:
            await ev.wait()
            return str(t.id)
        finally:
            self._sleepers.pop(t.id, None)
            if self.supervisor is not None:
                self.supervisor.activity.waiting(worker_id, False)
            self.timers.cancel(t.id)

    # ---------- chat continuity ----------

    async def _task_result(self, ev: dict[str, Any], shown: bool) -> None:
        """What FERB told (or will tell) the owner about a task; a macOS notification if chat was closed."""
        assert self.db is not None
        answer = str(ev.get("answer") or ev.get("summary") or "")
        shown = shown or ev.get("status") == "cancelled"          # stopped by the owner: they know
        self.db.conn.execute("INSERT OR REPLACE INTO task_results (task_id, at, status, goal, answer, shown) VALUES "
                             "(?, ?, ?, ?, ?, ?)", (ev["task_id"], self.clock.now().isoformat(), ev.get("status", ""),
                                                    str(ev.get("goal", "")), answer, int(shown)))
        if not shown and ev.get("status") != "cancelled":
            await self._notify("FERB finished" if ev.get("status") == "done" else "FERB", answer)

    def _approval_notice(self, data: dict[str, Any]) -> None:
        """An approval is waiting and no chat is open for that task: tell the owner (notification)."""
        tid = str(data.get("task_id") or "")
        if tid.split(".", 1)[0] in self._attached_trees:
            return
        text = f"FERB needs your OK: {str(data.get('reason', ''))} — open ferb chat"
        asyncio.get_running_loop().create_task(self._notify("FERB needs you", text, urgent=True))

    async def _notify_safely(self, title: str, text: str) -> None:
        try:
            await self.notify(title, text)
        except Exception:
            pass

    def owner_idle(self) -> bool:
        """Away from the Mac longer than [sense] idle_after_s (the owner-input tap); unknown counts as present."""
        last = self.owner_input.last_owner_wall
        if not self.owner_input.running or last is None:
            return False
        import time as _t
        return _t.time() - last > float(self.config.get("sense", "idle_after_s", 120))

    async def _notify(self, title: str, text: str, urgent: bool = False) -> None:
        """Time awareness in code (13): while the owner is idle, non-urgent notifications wait until they're back."""
        if not urgent and self.owner_idle():
            self._held.append((title, text))
            return
        await self._notify_safely(title, text)

    async def _flush_held(self) -> None:
        while True:
            await asyncio.sleep(10)
            if self._held and not self.owner_idle():
                held, self._held = self._held, []
                for title, text in held:
                    await self._notify_safely(title, text)

    async def _chat_status(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        """For `ferb chat` when it opens: answers that arrived while it was closed (then marked shown), tasks still
        running, and questions waiting for the owner."""
        assert self.db is not None
        rows = self.db.conn.execute("SELECT * FROM task_results WHERE shown = 0 ORDER BY at").fetchall()
        for r in rows:
            self.db.conn.execute("UPDATE task_results SET shown = 1 WHERE task_id = ?", (r["task_id"],))
        pending = [{"approval_id": ap.id, "capability": ap.capability, "kind": ap.kind, "reason": ap.reason,
                    "options": list(ap.options)} for ap in self.state.approvals.pending()]
        running = [{"task_id": t, "goal": g} for t, g in (self.supervisor.running_trees() if self.supervisor else [])]
        return {"results": [{"task_id": r["task_id"], "status": r["status"], "goal": r["goal"], "answer": r["answer"]}
                            for r in rows], "running": running, "approvals": pending}

    async def _chat_attach(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        """Chat is open again: approvals for running tasks are asked there, not by notification."""
        for t, _ in (self.supervisor.running_trees() if self.supervisor else []):
            self._attached_trees.add(t)
        assert self.supervisor is not None
        conversation = self.supervisor.continuity.attach(bool(params.get("new")), params.get("resume"))
        return {"ok": True, "session_id": conversation}

    async def _task_stop(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        """Ctrl-C in ferb chat: stop the request's task, recorded as stopped by the owner."""
        assert self.supervisor is not None
        ids = [str(t) for t in params.get("task_ids", [])] or [t for t, _ in self.supervisor.running_trees()]
        stopped = [t for t in ids if self.supervisor.stop_tree(t)]
        for t in stopped:
            self._journal().append("task.stopped", {"task_id": t, "by": "owner"})
        return {"stopped": stopped}

    # ---------- schedules ----------

    def _schedule_create(self, spec: Any, own_words: str, request_id: str, origins: set[str]) -> str:
        """From chat: the owner's words (quote-checked), the targets recorded now, the first run in the future.
        `origins`: what the turn that wrote the scheduled request was given; its runs inherit them (S3, MP2)."""
        from ferb.orchestrator.schedules import Rule, ScheduleError
        from ferb.policy.targets import Targets
        norm = " ".join(own_words.lower().split())
        if len(spec.quote.strip()) < 3 or " ".join(spec.quote.lower().split()) not in norm:
            self._journal().append("schedule.refused", {"why": "quote not in the owner's words"})
            return "I couldn't set that up — could you ask me again in your own words?"
        try:
            from ferb.clock.when import resolve
            first = resolve(spec.when, self.clock.now())                        # code computes the date, not the model
            rule = Rule.build(spec.repeat, first, spec.weekday, spec.every_minutes)
            t = spec.targets
            recorded = Targets.of(
                t.apps, t.sites, [normalize_path(p, str(self.home / "workspace")) for p in t.paths],
                t.recipients)
            s = self.schedules.add(spec.request, own_words, recorded.to_json(), rule, first)
        except (ScheduleError, ValueError) as e:
            return f"I couldn't schedule that: {e}."
        t = self.timers.add("schedule", first, s.id)
        self.schedules.set_timer(s.id, t.id, first)
        self._journal().append("schedule.created", {"schedule_id": s.id, "request": s.request, "rule": rule.describe(),
                                                    "request_id": request_id}, origin=origin_.join(origins))
        return f"🗓 scheduled ({s.id}): {s.request} — {rule.describe()}, first on {first.astimezone():%a %d %b %H:%M}"

    def _reminder_cancel_from_chat(self, rc: Any, own_words: str) -> str:
        norm = " ".join(own_words.lower().split())
        if len(rc.quote.strip()) < 3 or " ".join(rc.quote.lower().split()) not in norm:
            return "I didn't change any reminder — could you say which one in your own words?"
        t = self.timers.get(rc.id) if self.timers is not None else None
        if t is None or t.kind != "reminder" or not self.timers.cancel(rc.id):
            return f"I couldn't find a pending reminder {rc.id}."
        self._journal().append("timer.cancelled", {"timer_id": rc.id, "by": "owner"})
        return f"⏰ cancelled reminder {rc.id}: {t.message}"

    def _schedule_change_from_chat(self, ch: Any, own_words: str) -> str:
        norm = " ".join(own_words.lower().split())
        if len(ch.quote.strip()) < 3 or " ".join(ch.quote.lower().split()) not in norm:
            return "I didn't change any schedule — could you say which one in your own words?"
        return self._schedule_change(ch.id, ch.action)

    def _schedule_change(self, sid: str, action: str) -> str:
        s = self.schedules.get(sid)
        if s is None or s.state in ("cancelled", "done"):
            return f"There's no active schedule {sid}."
        if action == "cancel":
            self.schedules.set_state(sid, "cancelled")
            if s.timer_id:
                self.timers.cancel(s.timer_id)
        elif action == "pause":
            self.schedules.set_state(sid, "paused")
            if s.timer_id:
                self.timers.cancel(s.timer_id)
        elif action == "resume":
            self.schedules.set_state(sid, "active")
            nxt = s.rule.next_after(max(self.clock.now(), s.first_run - timedelta(seconds=1)), s.first_run) \
                if s.rule.repeat != "none" else (s.next_run if s.next_run and s.next_run > self.clock.now() else None)
            if nxt is None:
                self.schedules.set_state(sid, "done")
                return f"{sid} has no future runs left."
            t = self.timers.add("schedule", nxt, sid)
            self.schedules.set_timer(sid, t.id, nxt)
        self._journal().append(f"schedule.{action}", {"schedule_id": sid})
        return {"cancel": "Cancelled", "pause": "Paused", "resume": "Resumed"}[action] + f": {s.request}"

    async def _run_schedule(self, sid: str, overdue_s: float) -> None:
        """Schedule the next occurrence and let the orchestrator assess this run from current timing/failure facts."""
        s = self.schedules.get(sid)
        if s is None or s.state != "active":
            return
        now = self.clock.now()
        nxt = s.rule.next_after(now, s.first_run)
        if nxt is not None:
            t = self.timers.add("schedule", nxt, sid)
            self.schedules.set_timer(sid, t.id, nxt)
        else:
            self.schedules.set_timer(sid, None, None)
            self.schedules.set_state(sid, "done")
        if self.supervisor is None:
            return
        try:
            final = await self.supervisor.run_scheduled(s, late=overdue_s > 60, overdue_s=overdue_s)
        except Exception as e:                              # a run's failure is reported, never lost
            self._journal().append("schedule.failed", {"schedule_id": sid, "why": f"{type(e).__name__}: {e}"})
            final = {"task_id": f"{sid}:{now:%Y%m%d%H%M}", "status": "failed",
                     "answer": f"“{s.request}” didn't work this time; I'll try again at the next run."}
        # Only a verified run counts as success. A done-but-unverified run (V4) doesn't clear earlier failures, and
        # isn't counted as one either: the next run's consecutive_failures stays a count of real failures.
        status = final.get("status")
        self.schedules.ran(sid, True if status == "done" else None if status == "done_unverified" else False)
        answer = str(final.get("answer") or final.get("summary") or "")
        if overdue_s > 60:
            answer = f"(ran late) {answer}"
        await self._task_result({**final, "goal": s.request, "answer": answer}, shown=False)

    # ---------- progressive setup ----------

    async def _schedules_list(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        return {"schedules": [{"id": s.id, "request": s.request, "rule": s.rule.describe(), "state": s.state,
                               "next_run": s.next_run.isoformat() if s.next_run else None, "failures": s.failures}
                              for s in self.schedules.all()]}

    async def _schedules_change(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        return {"text": self._schedule_change(str(params["id"]), str(params["action"]))}

    async def _reminders_list(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        assert self.timers is not None
        return {"pending": [{"id": t.id, "kind": t.kind, "at": t.fire_at.isoformat(), "message": t.message}
                            for t in self.timers.pending()]}

    async def _reminders_cancel(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        assert self.timers is not None
        t = self.timers.get(str(params["id"]))
        if t is None or t.kind == "sleep" or not self.timers.cancel(t.id):
            raise FerbError(f"no pending reminder {params['id']}")
        return {"cancelled": t.id}

    async def _reminders_unseen(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        """For `ferb chat`: reminders delivered but not shown in chat yet (then marked shown)."""
        assert self.db is not None
        rows = self.db.conn.execute("SELECT id, message, overdue_s, delivered_at FROM timers WHERE state = 'delivered' "
                                    "AND kind != 'sleep' AND chat_shown = 0 ORDER BY fire_at").fetchall()
        for r in rows:
            self.db.conn.execute("UPDATE timers SET chat_shown = 1 WHERE id = ?", (r["id"],))
        return {"reminders": [{"id": r["id"], "message": self._redact_text(r["message"]),
                               "overdue_s": r["overdue_s"]} for r in rows]}

    # ---------- Jev ----------

    async def _jev_key(self) -> str | None:
        """From the Keychain (service "ferb", account jev/api_key); never from config, never logged."""
        from ferb.decide.decider import KEY_ACCOUNT
        from ferb.identity.secrets import SERVICE
        proc = await asyncio.create_subprocess_exec("security", "find-generic-password", "-s", SERVICE, "-a",
                                                    KEY_ACCOUNT, "-w", stdout=asyncio.subprocess.PIPE,
                                                    stderr=asyncio.subprocess.DEVNULL)
        out, _ = await proc.communicate()
        key = out.decode().strip() if proc.returncode == 0 else ""
        return key or None

    async def _build_jev(self) -> None:
        """Jev joins only once its key is in the Keychain; without it every hook is skipped (no rows, no calls)."""
        from ferb.decide.decider import JevClient, ShadowDecider
        if self.jev is not None:
            self.jev.close()
            self.jev = None
        key = await self._jev_key() if self.jev_post is None else "test-key"
        if key is None:
            self._wire_jev(None)
            return
        cached = key

        async def get_key() -> str:
            return cached
        assert self.db is not None
        self.jev = ShadowDecider(self.db, self.clock, self.ids, JevClient(get_key, post=self.jev_post), self.redact,
                                 enabled=lambda: set(self.config.get("jev", "enabled", [])))
        self._wire_jev(self.jev)

    def _wire_jev(self, jev: Any) -> None:
        if self.distiller is not None:
            self.distiller.jev = jev
        if self.trail is not None:
            self.trail.tag = self._jev_tag if jev is not None else (lambda e: None)

    def _jev_tag(self, e: Any) -> None:
        """Trail step 4: the row is already past exclusions and redaction, and stored."""
        asyncio.get_running_loop().create_task(self._jev_tag_async(e))

    async def _jev_tag_async(self, e: Any) -> None:
        from ferb.decide.decider import Ask
        if self.jev is None:
            return
        ref = f"trail:{e.id}"
        try:
            got = await self.jev.ask({"source": e.source, "actor": e.actor, "data": e.data},
                                     [Ask("trail.relevance", "note", ref), Ask("trail.sensitive", "no", ref)])
        except Exception:
            return                                                      # tagging never affects the trail
        assert self.db is not None
        if "trail.relevance" in got:
            self.db.conn.execute("UPDATE trail SET relevance = ? WHERE id = ?", (got["trail.relevance"], e.id))
        if got.get("trail.sensitive") == "yes":
            keep = {"source", "browser", "profile", "host", "app", "bundle_id", "change", "state", "is_dir"}
            data = {k: (v if k in keep else "[sensitive]") for k, v in e.data.items()}
            self.db.conn.execute("UPDATE trail SET data_json = ?, sensitive = 1 WHERE id = ?",
                                 (json.dumps(data), e.id))
            self.daily_activity.redact_source(e.id, data)
            self.jev.mark_effect("trail.sensitive", ref, "extra redaction")

    async def _jev_stats(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        if self.jev is None:
            return {"configured": False, "stats": []}
        stats = self.jev.stats()
        for st in stats:
            if st["answered"]:
                st["examples"] = self._jev_examples(st["question"])
        assert self.db is not None
        recent = self.db.conn.execute("SELECT jev, error FROM decisions WHERE error IS NULL OR error NOT IN "
                                      "('not sampled') ORDER BY seq DESC LIMIT 20").fetchall()
        answered = sum(1 for r in recent if r["jev"] is not None)
        last_error = next((r["error"] for r in recent if r["error"]), None)
        from ferb.decide.questions import QUESTIONS
        return {"configured": True, "dropped": self.jev.dropped, "stats": stats, "recent_answered": answered,
                "recent_total": len(recent), "last_error": last_error,
                "retired_enabled": sorted(set(self.config.get("jev", "enabled", [])) - QUESTIONS.keys())}

    def _jev_examples(self, question: str) -> list[str]:
        """A few items Jev flagged, read from their source now (redacted), so nothing extra is kept."""
        assert self.db is not None
        out = []
        for ref in self.jev.flagged_refs(question):
            kind, _, rid = ref.partition(":")
            row = None
            if kind == "trail":
                row = self.db.conn.execute("SELECT source, data_json FROM trail WHERE id = ?", (rid,)).fetchone()
                text = f"{row['source']}: {row['data_json']}" if row else ""
            elif kind == "span":
                row = self.db.conn.execute("SELECT input_json FROM spans WHERE id = ?", (rid,)).fetchone()
                text = row["input_json"] if row else ""
            else:
                text = ref
            if text:
                out.append(str(self._redact_text(text)))
        return out

    async def _jev_enable(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        from ferb.decide.questions import QUESTIONS
        q = str(params["question"])
        if q not in QUESTIONS:
            raise FerbError(f"unknown question {q}")
        enabled = set(self.config.get("jev", "enabled", [])) & QUESTIONS.keys()
        if params.get("on", True):
            if self.jev is None:
                raise FerbError("Jev isn't set up: `ferb setup --jev`")
            enabled.add(q)
        else:
            enabled.discard(q)
        self.config.set("jev", "enabled", sorted(enabled))
        self._journal().append("jev.enabled" if params.get("on", True) else "jev.disabled", {"question": q})
        return {"enabled": sorted(enabled)}

    async def _jev_reload(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        await self._build_jev()
        out: dict[str, Any] = {"configured": self.jev is not None}
        if self.jev is not None and params.get("ping"):
            from ferb.decide.decider import JevUnavailable
            try:
                await self.jev.client.ask("FERB setup check", {"q0": {"type": "noul",
                                                                      "instructions": "Is this a setup check?"}}, 20)
                out["ping"] = "ok"
            except JevUnavailable as e:
                out["ping"] = str(e)
        return out

    def _environment(self, own_words: str) -> dict[str, Any]:
        """Installed apps and browser connection facts; the model interprets the owner's requested route."""
        import platform
        from ferb.act.mac import apps as appsm
        from ferb.policy import services as svc
        ferb = self._identity_profile()
        names = {(p.browser, p.folder): p.display_name for p in self._profiles()} if self.pairings else {}
        paired = [(b.browser, b.folder, names.get((b.browser, b.folder), b.folder))
                  for b in (self.pairings.all() if self.pairings else [])
                  if b.identity is Identity.OWNER and (b.browser, b.folder) != ferb]
        env = svc.Environment(self._installed_apps(), paired)
        try:
            running = {a.bundle_id for a in appsm.running()}
        except Exception:
            running = set()
        stable, live = svc.summary(env, platform.mac_ver()[0] or "unknown", running)
        profiles = [{'browser': p.browser, 'profile': p.folder, 'name': p.display_name, 'account_hint': p.email,
                     'connection': self._profile_state(p),
                     'identity': 'ferb' if (p.browser, p.folder) == ferb else 'owner'} for p in self._profiles()]
        stable += '\nBrowser/profile inventory (facts, not a selected route): ' + json.dumps(profiles)
        return {"stable": stable, "live": live}

    def _task_root(self, task_id: str) -> str:
        """The request's root task a worker step belongs to: download budgets and the tab cap are per task tree."""
        rec = self.tasks.get(task_id)
        return (rec.tree_id if rec is not None and rec.tree_id else None) or task_id.split(".", 1)[0]

    def _inspection_parent(self, task_id: str) -> str | None:
        """A registered checker borrows its worker's tabs/evidence, without changing their ownership."""
        rec = self.tasks.get(task_id)
        if rec is None or rec.task.role != "verifier":
            return None
        child = rec.task
        parent = self.tasks.get(child.parent_id) if child.parent_id else None
        if parent is None or parent.task.role != "worker" or not child.scopes <= parent.task.scopes:
            return None
        if (child.request_id, child.identity, child.device_id, rec.tree_id) != (
                parent.task.request_id, parent.task.identity, parent.task.device_id, parent.tree_id):
            return None
        return parent.task.id

    async def _app_window(self, app: str, scopes: frozenset[str]) -> Any:
        """The gateway's first look at a Mac app a worker started on (D26, D29): one background picture of its
        window, or None (no Mac driver, or nothing to take under mac.snapshot's image rules)."""
        if self.mac is None:
            return None
        from ferb.act.mac.tools import app_window
        return await app_window(self.mac, app, scopes)

    def _browser_context(self, task_id: str) -> str:
        parent = self._inspection_parent(task_id)
        if parent is None:
            return ""
        tabs = [{"tool_prefix": prefix, "tab": tab}
                for prefix, drv in (("browser", self.router), ("ferb_browser", self.ferb_identity))
                if drv is not None for tab in drv.tabs_of(parent)]
        return ("\n\nExisting task tabs available for inspection (reuse these aliases; do not reopen "
                "the page or guess tab ids): " + json.dumps(tabs)) if tabs else ""

    def _relevant_context(self, words: str) -> tuple[str, str]:
        """Installed actions and worker types (FERB's own) and today's matching activity, with its origin (S3):
        recorded trail rows carry the trail labels; FERB's own observed actions are FERB's."""
        from ferb.act.catalog import search_actions
        from ferb.core import origin
        from ferb.sanitize.untrusted import wrap_untrusted
        from ferb.sense.tools import trail_sources
        actions = search_actions(self.registry.capabilities, words)
        activity = self.daily_activity.search(words)
        trail = {str(row['source']) for row in activity if row.get('source')}
        labels = {origin.from_read_source(k, i) for k, i in trail_sources(trail)} if trail else set()
        return ('\nRelevant installed actions: ' + json.dumps(actions) +
                '\nRelevant activity today (data): ' + wrap_untrusted(json.dumps(activity)) +
                '\nWorker types: ' + json.dumps(['default', *[n for n in self.registry.runtimes if self._runtime_available(n)]]) + '\n',
                origin.join({origin.FERB} | labels))

    def _profile_state(self, p: Any) -> str:
        """connected / paired (FERB opens it) / installed, not paired / no extension."""
        from ferb.act.browser.profiles import has_extension
        b = self.pairings.for_profile(p.browser, p.folder) if self.pairings else None
        if b is not None:
            br = self.hub.bridges.get((p.browser, p.folder)) if self.hub else None
            return "connected" if br is not None and br.conn else "paired"
        ids = frozenset(self.config.get("browser", "extension_ids", [FERB_EXTENSION_ID]))
        data = (self.browser_data_dirs or {}).get(p.browser)
        return "installed" if has_extension(p, ids, data) else "missing"

    def _time_meanings(self) -> list[tuple[str, str]]:
        """The owner's own meanings of fuzzy time words ("next friday", "evening"), stored when they corrected FERB."""
        if self.memory is None:
            return []
        return [(it.key.removeprefix("time_").replace("_", " "), it.content)
                for it in self.memory.items("active", "owner") if it.kind == "preference" and it.key.startswith("time_")]

    def _permissions(self) -> dict[str, bool]:
        """The daemon's own macOS permissions (the process that acts). Replaceable in tests."""
        from ferb.device import permissions as perm
        try:
            return {"accessibility": perm.accessibility() is perm.Status.GRANTED,
                    "screen": perm.screen_recording() is perm.Status.GRANTED}
        except Exception:
            return {"accessibility": True, "screen": True}              # can't tell: plan validation still guards

    def _availability(self) -> Any:
        """What the planner may use right now, and what's missing with how to fix it (owner decision, 2026-09-30)."""
        from ferb.orchestrator.availability import build
        registered = set().union(*(c.scopes for c in self.registry.capabilities.values()))
        problems: dict[str, str] = {}
        profiles = ""
        if "browser:act" in registered:
            ferb = self._identity_profile()
            owner = [b for b in (self.pairings.all() if self.pairings else [])
                     if b.identity is Identity.OWNER and (b.browser, b.folder) != ferb]
            names = {(p.browser, p.folder): p.display_name for p in self._profiles()} if owner else {}
            live = [b for b in owner if self.hub is not None and (br := self.hub.bridges.get((b.browser, b.folder)))
                    is not None and br.conn is not None]
            if not owner:
                problems["owner_browsers"] = "none of the owner's browser profiles is paired — `ferb browsers pair " \
                                             "<browser> \"<profile>\"`"
            elif not live and not (self.router and self.router.wake):
                shown = ", ".join(f"{b.browser.capitalize()} “{names.get((b.browser, b.folder), b.folder)}”"
                                  for b in owner)
                problems["owner_browsers"] = f"{shown} paired; extension transport disconnected"
            profiles = ", ".join(f"{b.browser.capitalize()} “{names.get((b.browser, b.folder), b.folder)}”"
                                 for b in live)
            sleeping = [b for b in owner if b not in live]
            if sleeping and self.router and self.router.wake:
                wakeable = ", ".join(f"{b.browser.capitalize()} “{names.get((b.browser, b.folder), b.folder)}”"
                                     for b in sleeping)
                profiles = (f"{profiles}; " if profiles else "none; ") + f"can wake paired profiles: {wakeable}"
        prof = self._identity_profile()
        if prof is not None and self.identity_browser is None:   # FERB's own browser is part of this FERB
            problems["ferb_browser"] = "not paired — run `ferb setup --identity`"
        # Not open right now is fine: FERB opens its own profile when a task needs it (the owner's report, 2026-09-30)
        perms = self._permissions()
        if registered & {"mac:read", "mac:act"} and not perms.get("accessibility", True):
            problems["mac"] = "FERB doesn't have the Accessibility permission — run `ferb doctor`"
        if "screen" in registered and not perms.get("screen", True):
            problems["screen"] = "FERB doesn't have the Screen Recording permission — run `ferb doctor`"
        return build(registered, problems, profiles)

    def _installed_apps(self) -> list[tuple[str, str]]:
        """(bundle id, name) of the Mac's apps, for matching the names the owner uses."""
        p = self._device_profile()
        return [(a.bundle_id, a.name) for a in p.apps] if p is not None else []

    def _device_profile(self) -> Any:
        from ferb.device.profile import ProfileStore
        did = self.config.get("device", "id")
        return ProfileStore(self.home).load(str(did)) if did else None

    def _redact_text(self, text: str) -> str:
        f = getattr(self.redact, "text", None)
        return str(f(text)) if f else text

    async def _ps_list(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        assert self.shells
        return {"processes": [p.__dict__ for p in self.shells.list()]}

    async def _ps_kill(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        assert self.shells
        return {"killed": await self.shells.kill_id(str(params.get("id", "")))}

    # ---------- owner browsers ----------

    def _profiles(self) -> list[browser_profiles.BrowserProfile]:
        return browser_profiles.discover_all(self.browser_data_dirs)

    def _identity_profile(self) -> tuple[str, str] | None:
        folder = self.config.get("identity", "profile")
        return (str(self.config.get("identity", "browser", "chrome")), str(folder)) if folder else None

    async def _build_browsers(self) -> None:
        assert self.db and self.ledger
        self.pairings = Pairings(self.db, self.clock)
        self.hub = Hub(self.pairings, self.ledger, self._publish,
                       lambda: str(self.config.get("device", "id", "unknown-device")))
        resolver = Resolver(self._profiles, self.pairings.all, self._identity_profile)
        await self.wake.start()
        self.router = Router(self.hub, resolver, self._profiles, self.wake, self.browser_launch)
        # D35: a download in a task tab becomes a file in the workspace plus facts, never a dialog.
        self.downloads = downloads.Downloads(self.home / "workspace" / downloads.FOLDER, self.db, self.clock.now,
                                             self.config.get, lambda text: str(self.redact({"u": text})["u"]),
                                             self._publish)
        for name, step in (("sweep", self.downloads.sweep), ("finalize_stale", self.downloads.finalize_stale)):
            try:                                         # what a restart cut off, each step on its own
                step()
            except Exception as e:
                self._journal().append("browser.downloads_startup_failed",
                                       {"step": name, "error": f"{type(e).__name__}: {e}"})
        self.downloads.task_root = self._task_root
        self.hub.on_download = self.downloads.receive
        self.router.ask_code = lambda ctx, origin: self.gateway.ask_login_code(ctx, origin)
        self.router.add_secret_value = self._add_secret_value
        add_owner_browsers(self.registry, self.router, self.recordings)
        pairings = self.pairings
        self.ferb_identity = FerbIdentity(self.hub, self._identity_profile, self.secrets, self._add_secret_value,
                                          lambda: str(self.config.get("mailbox", "address", "") or ""),
                                          lambda install_id: pairings.key(install_id), self._profiles,
                                          self.wake, self.browser_launch)
        for drv in (self.router, self.ferb_identity):
            drv.inspection_parent = self._inspection_parent
            drv.downloads = self.downloads
            drv.task_running = lambda task_id: self.tasks.get(task_id) is not None   # removed when its tree ends
            drv.task_root = self._task_root
            drv.max_tabs = lambda: int(self.config.get("browser", "task_max_tabs", 20))
        self.downloads.task_tab = self.hub.task_tab      # only a live task's tab holds downloads (D35)
        # Browser/profile selection is an agent decision, not a phrase-derived permission gate.
        self._attach_identity_if_paired()
        self.server.native = self.hub.serve
        self.server.extension_ids = lambda: frozenset(self.config.get("browser", "extension_ids",
                                                                      [FERB_EXTENSION_ID]))

    def _attach_identity_if_paired(self) -> bool:
        """FERB's tools exist only once its identity profile is paired as FERB."""
        prof = self._identity_profile()
        b = self.pairings.for_profile(*prof) if (prof and self.pairings) else None
        if b is None or b.identity is not Identity.FERB or self.ferb_identity is None:
            return False
        if self.identity_browser is None:
            self.use_identity_browser(self.ferb_identity)
        return True

    async def _identity_pair(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        """Pair FERB's identity profile as identity FERB: its Chrome accounts must be exactly FERB's;
        re-pairing needs the owner's confirmation."""
        from ferb.identity import seed
        assert self.pairings
        prof, address = self._identity_profile(), str(self.config.get("mailbox", "address", "") or "")
        if not prof or not address:
            raise FerbError("choose FERB's identity profile first: `ferb setup --identity`")
        data = (self.browser_data_dirs or {}).get(prof[0])
        try:
            await asyncio.to_thread(seed.verify, address, prof[1], *([data] if data else []))
        except seed.SeedError as e:
            raise FerbError(f"refusing to pair: {e}") from None
        current = self.pairings.for_profile(*prof)
        if current is not None and not params.get("confirm"):
            raise FerbError("FERB's identity profile is already paired; re-pairing needs your confirmation")
        before = self.hub.bridges.get(prof) if self.hub else None
        hellos0 = before.hellos if before is not None else 0
        token = self.pairings.issue_token(prof[0], prof[1], Identity.FERB)
        wake = await self._open_pairing(prof[0], prof[1], token)
        end = self.clock.monotonic() + float(params.get("timeout_s", 60))
        while self.clock.monotonic() < end:
            b = self.pairings.for_profile(*prof)
            br = self.hub.bridges.get(prof) if self.hub else None
            # its own token and a connection attached after it (not a changed binding: re-pairing the same install
            # gives an identical one, and its bridge may still hold the connection from before)
            if token in self.pairings.paired and b is not None and b.identity is Identity.FERB and br is not None \
                    and br.conn and (br is not before or br.hellos > hellos0):
                await self._close_wake(br, wake)
                self._attach_identity_if_paired()
                return {"paired": True, "profile": prof[1]}
            await asyncio.sleep(0.2)
        return {"paired": False, "profile": prof[1], "hint": "is the FERB extension loaded in FERB's profile?"}

    async def _open_pairing(self, browser: str, folder: str, token: str) -> str | None:
        """Open the extension's pairing page in that profile. A profile with no window isn't loaded, and a pairing
        page handed to it can go unanswered: load it first with a blank local page (closed after pairing)."""
        known = self._profiles()
        kwargs: dict[str, Any] = {"launch": self.browser_launch} if self.browser_launch else {}
        wake = None
        br = self.hub.bridges.get((browser, folder)) if self.hub else None
        if br is None or br.conn is None:
            wake = self.wake.url()
            browser_profiles.open_known_profile(browser, folder, wake, known, background=False, **kwargs)
            await asyncio.sleep(self.pair_load_wait_s)
        ext = sorted(self.server.extension_ids())[0]
        browser_profiles.open_known_profile(browser, folder, f"chrome-extension://{ext}/pair.html#token={token}",
                                            known, background=False, **kwargs)   # the owner must see this page
        return wake

    @staticmethod
    async def _close_wake(br: Any, wake: str | None) -> None:
        if wake:
            try:
                await br.call("wake_close", {"url": wake}, timeout=5)
            except FerbError:
                pass

    async def _browsers_list(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        assert self.pairings and self.hub
        ferb = self._identity_profile()
        paired = {(b.browser, b.folder): b for b in self.pairings.all()}
        out = []
        for p in self._profiles():
            b = paired.get((p.browser, p.folder))
            br = self.hub.bridges.get((p.browser, p.folder))
            out.append({"browser": p.browser, "folder": p.folder, "name": p.display_name, "email": p.email,
                        "ferb_identity": (p.browser, p.folder) == ferb,
                        "paired": b is not None, "identity": b.identity.value if b else None,
                        "connected": bool(br and br.conn), "version": br.version if br and br.conn else "",
                        "last_version": br.version if br else "",
                        "refused": self.hub.refused.get(b.install_id) if b else None})
        unpaired = list(self.hub.unpaired.values())
        return {"profiles": out, "extension_ids": sorted(self.server.extension_ids()),
                "unpaired_connections": {browser: unpaired.count(browser) for browser in sorted(set(unpaired))}}

    async def _browsers_pair(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        """Open the extension's own pairing page in that profile with a one-time token; wait for the pairing."""
        assert self.pairings
        known = self._profiles()
        name, browser = str(params.get("profile", "")), params.get("browser")
        r = browser_profiles.resolve(name, known, browser=browser)
        if r.kind != "one":
            raise FerbError(r.message(name, known))
        p = r.candidates[0]
        if (p.browser, p.folder) == self._identity_profile():
            raise FerbError(f"{p.display_name} is FERB's own profile; it's paired as FERB (`ferb setup --identity`)")
        if self._profile_state(p) == "missing":                 # the extension first (owner's report 2026-10-01)
            from ferb.device.extensions import extension_path
            browser_profiles.open_known_profile(p.browser, p.folder, "chrome://extensions", known,
                                                background=False)   # the owner does the clicks: they must see it
            return {"paired": False, "needs_install": True, "browser": p.browser, "folder": p.folder,
                    "name": p.display_name, "extension_dir": str(extension_path(self.home))}
        token = self.pairings.issue_token(p.browser, p.folder)
        wake = await self._open_pairing(p.browser, p.folder, token)
        timeout = float(params.get("timeout_s", 60))
        end = self.clock.monotonic() + timeout
        while self.clock.monotonic() < end:
            b = self.pairings.for_profile(p.browser, p.folder)
            if b is not None and self.hub and (br := self.hub.bridges.get((p.browser, p.folder))) and br.conn:
                await self._close_wake(br, wake)
                return {"paired": True, "browser": p.browser, "folder": p.folder, "name": p.display_name}
            await asyncio.sleep(0.2)
        return {"paired": False, "browser": p.browser, "folder": p.folder, "name": p.display_name,
                "hint": "is the FERB extension loaded and enabled in that profile?"}

    async def _browsers_installed(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        """Is FERB's extension installed in that profile yet (read from its settings on disk)?"""
        r = browser_profiles.resolve(str(params.get("profile", "")), self._profiles(), browser=params.get("browser"))
        if r.kind != "one":
            raise FerbError(r.message(str(params.get("profile", "")), self._profiles()))
        return {"installed": self._profile_state(r.candidates[0]) != "missing"}

    async def _browsers_unpair(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        assert self.pairings
        known = self._profiles()
        r = browser_profiles.resolve(str(params.get("profile", "")), known, browser=params.get("browser"))
        if r.kind != "one":
            raise FerbError(r.message(str(params.get("profile", "")), known))
        p = r.candidates[0]
        return {"unpaired": self.pairings.unpair(p.browser, p.folder)}

    def _urls_of(self, task_id: str) -> list[str]:
        return [u for drv in (self.router, self.ferb_identity) if drv is not None for u in drv.urls_of(task_id)]

    def _add_secret_value(self, value: str) -> None:
        add = getattr(self.redact, "add_secret_value", None)
        if add is not None:
            add(value)

    def _facts(self) -> str:
        address = self.config.get("mailbox", "address", "")
        lines = [f"- FERB's own email (its identity, never the owner's): {address}" if address else
                 "- FERB has no email account set up."]
        lines += [f"- FERB secret {s.ref.key}: {s.account_hint}, usable on {', '.join(s.allowed_origins)}"
                  for s in self.secrets.specs()]
        for sc in (self.schedules.all() if self.schedules is not None else []):
            lines.append(f"- Schedule {sc.id} ({sc.state}): {sc.request} — {sc.rule.describe()}")
        for t in (self.timers.pending(("reminder",)) if self.timers is not None else []):
            lines.append(f"- Reminder {t.id}: {t.message} — {t.fire_at.astimezone():%a %d %b %Y %H:%M}")
        for phrase, meaning in self._time_meanings():
            lines.append(f"- When the owner says “{phrase}”, they mean: {meaning}")
        if self.pairings is not None and self.hub is not None:           # the owner's browsers, as FERB can use them
            ferb = self._identity_profile()
            for b in self.pairings.all():
                if (b.browser, b.folder) == ferb or b.identity is not Identity.OWNER:
                    continue
                br = self.hub.bridges.get((b.browser, b.folder))
                name = next((p.display_name for p in self._profiles() if (p.browser, p.folder) == (b.browser, b.folder)),
                            b.folder)
                lines.append(f"- The owner's {b.browser} profile “{name}”: paired"
                             f"{', connected' if br is not None and br.conn else ', extension transport disconnected'}")
            for browser in sorted(set(self.hub.unpaired.values())):
                note = self._unpaired_note(browser)
                if note:
                    lines.append(f"- {note}")
        return "\n".join(lines)

    def _publish(self, topic: str, data: dict[str, Any]) -> None:
        if topic in ('trail.event', 'action.observed'):
            self.daily_activity.record(self.redact(dict(data, what=data.get('what', data.get('source', topic)),
                                                        where=data.get('where', data.get('data', {})),
                                                        device_id=self.config.get('device', 'id', 'unknown-device'))))
        self.bus.publish(topic, self.redact(data))
        if topic == "approval.requested":
            self._approval_notice(data)

    def _runtime_violation(self, runtime: str, detail: str) -> None:
        """Record this run’s tool-source violation; the orchestrator can select another attempt."""
        self._journal().append("runtime.failed", {"runtime": runtime, "detail": detail})

    def _runtime_for(self, role: str) -> str:
        """The runtime a role uses: a planner-chosen worker type, else [runtimes] orchestrator / worker / checker."""
        if role.startswith("worker:") and role != "worker:default":
            return role.partition(":")[2]
        if role == "orchestrator":
            return str(self.config.get("runtimes", "orchestrator", "codex"))
        worker = str(self.config.get("runtimes", "worker", "codex"))
        return str(self.config.get("runtimes", "checker", worker)) if role == "checker" else worker

    def _model_for(self, name: str) -> str | None:
        model = self.config.get("models", name) or (self.config.get("runtimes", "claude_model") if name == "claude" else None)
        return str(model) if model else None

    def _make_runtime(self, role: str, *, mode: str, tool_names: list[str], schema: dict[str, Any]) -> Any:
        return self._build_runtime(self._runtime_for(role), role, mode=mode, tool_names=tool_names, schema=schema)

    def _build_runtime(self, name: str, role: str, *, mode: str, tool_names: list[str], schema: dict[str, Any]) -> Any:
        if name not in self.registry.runtimes:
            raise FerbError(f"unknown runtime {name!r} in config.toml [runtimes]")
        model = self._model_for(name)
        shim = [sys.executable, "-m", "ferb.cli.mcp"]
        if name == "codex":
            from ferb.device.install import repo_root
            from ferb.core.errors import Unsupported
            from ferb.runtimes.codex_exec import MODEL, CodexRuntime, codex_binary, codex_home
            if codex_home(self.home) is None:
                raise Unsupported("Codex isn't signed in for FERB: run `ferb setup --codex-login`")
            workdir = self.home / "workspace" if mode == "worker" else self.home / "run" / "supervisor"
            return CodexRuntime(codex_binary(self.home, repo_root()), mode, workdir, self.home,  # type: ignore[arg-type]
                                shim_cmd=shim, tool_names=tool_names, output_schema=schema, model=model or MODEL,
                                timeout_s=None if mode == 'worker' else 900)
        if name in ("anthropic", "openai", "google"):
            from ferb.runtimes.api import ApiRuntime, GatewayBridge
            from ferb.runtimes.catalog import CATALOG
            provider = _provider(name, model or CATALOG[name].default_model)
            return ApiRuntime(provider, mode=mode, role=role, tool_names=tool_names,  # type: ignore[arg-type]
                              output_schema=schema, bridge=GatewayBridge(self.gateway, self.tokens), key=self._api_key)
        from ferb.runtimes.claude_sdk import ClaudeRuntime
        if self._proxy is None:
            self._proxy = ToolListProxy()
        return ClaudeRuntime(mode, self._proxy, str(self.home), shim_cmd=shim, tool_names=tool_names,  # type: ignore[arg-type]
                             output_schema=schema, model=model)

    async def _api_key(self, provider: str) -> str | None:
        """From the Keychain at call time; added to the redactor so an echo never reaches a trace or the journal."""
        from ferb.runtimes import keys
        key = await keys.api_key(provider)
        if key:
            self._add_secret_value(key)
        return key

    def _runtime_available(self, name: str) -> bool:
        from ferb.runtimes.catalog import CATALOG
        now = time.monotonic()
        hit = self._available_cache.get(name)
        if hit is None or now - hit[0] > 60:
            hit = (now, name in CATALOG and CATALOG[name].available(self.home)[0])
            self._available_cache[name] = hit
        return hit[1]

    async def _runtimes_check(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        """One tiny typed call through a runtime: proves the key, model and network work (costs a fraction of a cent)."""
        from ferb.core.types import ModelRequest, Task
        name = str(params.get("runtime"))
        self._available_cache.pop(name, None)
        schema = {"type": "object", "properties": {"ok": {"type": "boolean"}}, "required": ["ok"],
                  "additionalProperties": False}
        try:
            rt = self._build_runtime(name, "orchestrator", mode="supervisor", tool_names=[], schema=schema)
            device = str(self.config.get("device", "id", "unknown-device"))
            task = Task("runtime-check", None, "chat", device, Identity.OWNER, set(), "mixed", "runtime-check", [], None)
            run = await rt.start(task, ModelRequest('Answer {"ok": true}.', ("Check.",), 0), "")
        except Exception as e:
            return {"ok": False, "detail": f"{type(e).__name__}: {e}"}
        detail = ""
        try:
            async for ev in rt.events(run):
                if ev.kind == "final":
                    try:
                        answer = json.loads(ev.data["text"])
                    except (TypeError, ValueError):
                        answer = None
                    if not isinstance(answer, dict):
                        return {"ok": False, "detail": 'the answer was not {"ok": ...}'}
                    return {"ok": answer.get("ok") is True, "detail": "answered"}
                if ev.kind in ("error", "violation"):
                    detail = str(ev.data)
        finally:
            await rt.cancel(run)
        return {"ok": False, "detail": detail or "no answer"}

    async def _tools_list(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        if session.grant is None or self.gateway is None:
            raise FerbError("no run token")
        return {"tools": self.gateway.tools_for(session.grant)}

    async def _tools_call(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        if session.grant is None or self.gateway is None:
            raise FerbError("no run token")
        return await self.gateway.call(session.grant, str(params.get("name")), dict(params.get("arguments") or {}))

    async def _chat_approve(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        choice = params.get("choice")
        return self._resolve_approval(str(params["approval_id"]), bool(params["granted"]),
                                      int(choice) if choice is not None else None)

    async def _chat_answer(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        """The owner's free-text answer to a worker's question (owner.ask); empty text declines."""
        text = str(params.get("text") or "").strip()
        ap = self.state.approvals.get(str(params["approval_id"]))
        if ap is not None and ap.kind == "owner_login_code" and text:
            self._add_secret_value(text)
        return self._resolve_approval(str(params["approval_id"]), bool(text), None, text or None)

    def _resolve_approval(self, approval_id: str, granted: bool, choice: int | None = None,
                          text: str | None = None) -> dict[str, Any]:
        """The owner's answer to a question, typed (`ferb chat`) or spoken (push-to-talk)."""
        ap = self.state.approvals.resolve(approval_id, granted, choice, text)
        self._journal().append("approval.resolved", {"approval_id": ap.id, "state": ap.state,
                                                     "capability": ap.capability})
        self._publish("approval.resolved", {"approval_id": ap.id, "state": ap.state})
        return {"approval_id": ap.id, "state": ap.state}

    async def _chat_send(self, session: Session, params: dict[str, Any]) -> AsyncIterator[dict[str, Any]]:
        """One chat turn: the supervisor's events, merged with bus events about the tasks this turn started
        (tool calls, owner questions and action observations) so `ferb chat` can show them inline."""
        if self._shutting_down:
            raise FerbError("daemon is shutting down; no new chat task was started")
        if self.supervisor is None:
            raise FerbError("daemon not started")
        queue: asyncio.Queue[dict[str, Any] | None] = asyncio.Queue()
        my_tasks: set[str] = set()
        attached = {"on": True}                     # False once the owner closes chat: the task keeps running
        sub = self.bus.subscribe({"tool.called", "approval.requested", "action.*"})

        async def turn() -> None:
            try:
                async for ev in self.supervisor.send(str(params.get("session_id", "default")),  # type: ignore[union-attr]
                                                     str(params.get("text", ""))):
                    if ev.get("task_id"):
                        my_tasks.add(ev["task_id"])
                        if attached["on"]:
                            self._attached_trees.add(ev["task_id"])
                    if ev.get("type") == "outcome":
                        await self._task_result(ev, shown=attached["on"])
                        self._attached_trees.discard(ev["task_id"])
                    await queue.put(ev)
            except FerbError as e:
                await queue.put({"type": "error", "text": str(e)})
            except Exception as e:                       # logged; the owner gets a plain sentence, not internals
                self._journal().append("chat.failed", {"why": f"{type(e).__name__}: {self.redact(str(e))}"})
                await queue.put({"type": "reply", "text": "Sorry — something went wrong on my side. Could you try "
                                                          "that again?"})
            finally:
                await queue.put(None)

        async def bus_events() -> None:
            async for ev in sub:
                tid = ev.data.get("task_id")
                if tid in my_tasks or (ev.topic.startswith("action.") and not tid):
                    await queue.put({"type": "event", "topic": ev.topic, "data": self.redact(ev.data)})

        t1, t2 = asyncio.create_task(turn()), asyncio.create_task(bus_events())
        self._turns.add(t1)
        t1.add_done_callback(self._turns.discard)
        try:
            while (item := await queue.get()) is not None:
                yield item
        finally:
            t2.cancel()
            sub.close()
            if not t1.done():                           # chat closed mid-task: never stops it (Ctrl-C does)
                attached["on"] = False
                for tid in my_tasks:
                    self._attached_trees.discard(tid)

    def _journal(self) -> Journal:
        if self.journal is None:
            raise FerbError("daemon not started")
        return self.journal

    # ---------- methods ----------

    async def _ping(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        return {"pid": os.getpid(), "role": session.role.value, "now": self.clock.now().isoformat(),
                "stopped": self.state.stopped}

    def _resolve_root_task(self, task_id: str, allow_last: bool = False) -> str:
        """Owner-typed task id: exact id, or a unique 6+ character root-task prefix; `last` only when allowed."""
        if self.db is None:
            raise FerbError("daemon not started")
        if task_id == "last" and allow_last:  # Explicit CLI request for the latest task, never an action target.
            row = self.db.conn.execute("SELECT id FROM tasks WHERE parent_id IS NULL "
                                       "ORDER BY created_at DESC, rowid DESC LIMIT 1").fetchone()
            return str(row['id']) if row else task_id
        if len(task_id) >= 6 and not self.db.conn.execute("SELECT 1 FROM tasks WHERE id = ?", (task_id,)).fetchone():
            matches = self.db.conn.execute("SELECT id FROM tasks WHERE substr(id, 1, ?) = ? "
                                           "AND parent_id IS NULL ORDER BY id", (len(task_id), task_id)).fetchall()
            if len(matches) > 1:
                raise FerbError('ambiguous task prefix: ' + ', '.join(r['id'] for r in matches))
            if matches:
                return str(matches[0]['id'])
        return task_id

    async def _trace(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        task_id = str(params.get("task_id") or "last")
        if self.spans is None or self.db is None:
            raise FerbError("daemon not started")
        task_id = self._resolve_root_task(task_id, allow_last=True)
        spans = self.spans.tree_json(task_id)
        outcome = self.spans.outcome(task_id)
        episode = episodes.get(self.db, task_id)
        if episode is None and spans:                    # running, or written before F3: compose it live
            episode = compose.compose({"task_id": task_id, "spans": spans, "outcome": outcome})
        return {"task_id": task_id, "spans": spans, "outcome": outcome, "episode": episode}

    async def _recovery_list(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        """`ferb restore`: what FERB can put back, newest first (S1), at most RESTORE_LIST of `total`."""
        kept = self._recovery()
        rows, total = recovery.recent(kept.db, kept.root, RESTORE_LIST)
        return {"days": retention.days(self.config.get("retention", "recovery_days", 7)), "total": total,
                "trash_days": recovery.TRASH_RECORD_MAX_DAYS,
                "records": [r | {"original": recovery.short(r["original"])} for r in rows]}

    async def _recovery_restore(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        """`ferb restore <id>`: put a file back (recovery.restore never overwrites a change made since); journaled. It
        holds the gateway's lock on that file (the key FERB's own fs calls take), so it never races a FERB write; the
        file work runs in a worker thread."""
        kept = self._recovery()
        try:
            row = recovery.find(kept.db, str(params.get("id") or ""))
            locks = ["file:" + str(Path(row["original"]).resolve())] if row["kind"] in ("clone", "trash") else []
            async with self._gateway().locks.hold(locks, "owner:ferb restore"):
                back = await recovery.restore_async(row["id"], kept.db, kept.root)
        except recovery.NotRestorable as e:
            raise FerbError(str(e)) from None
        original = Path(row["original"])
        in_place = back == original.resolve()
        self._journal().append("recovery.restored", {"id": row["id"], "kind": row["kind"], "original": str(original),
                                                     "restored_to": str(back), "in_place": in_place})
        return {"id": row["id"], "kind": row["kind"], "original": recovery.short(original),
                "restored_to": recovery.short(back), "in_place": in_place}

    async def _verdict_set(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        """`ferb verdict`: the owner's pass/fail on a task. Acceptance is their word; code never infers it."""
        if self.db is None:
            raise FerbError("daemon not started")
        verdict = str(params.get("verdict") or "")
        if verdict not in ("pass", "fail"):
            raise FerbError("verdict must be 'pass' or 'fail'")
        given = str(params.get("task_id") or "")
        task_id = self._resolve_root_task(given)
        row = self.db.conn.execute("SELECT parent_id, tree_id FROM tasks WHERE id = ?", (task_id,)).fetchone()
        if row is None:
            raise FerbError(f"unknown task {given!r}")
        if row["parent_id"] is not None:
            raise FerbError(f"task {task_id} is part of {row['tree_id']}; give the verdict for {row['tree_id']}")
        self.db.conn.execute(
            "INSERT INTO owner_verdicts (id, task_id, verdict, note, at) VALUES (?,?,?,?,?)",
            (self.ids.new_id(), task_id, verdict, self.redact({"note": str(params.get("note") or "")})["note"],
             self.clock.now().isoformat()))
        self.db.conn.commit()
        self._journal().append("owner.verdict", {"task_id": task_id, "verdict": verdict})
        return {"task_id": task_id, "verdict": verdict}

    async def _stop_engage(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        summary = self.stop_control.engage()
        self._journal().append("stop.engaged", summary)
        self.bus.publish("stop.engaged", summary)
        return summary

    async def _stop_lift(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        self.stop_control.lift()
        self._journal().append("stop.lifted", {})
        self.bus.publish("stop.lifted", {})
        return {"stopped": False}

    async def _identity_check(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        """Setup / doctor: FERB's Chrome profile holds exactly FERB's account, and each
        secret is in the Keychain. The session itself is checked through the extension when a session check is requested."""
        from ferb.identity import seed
        address = str(self.config.get("mailbox", "address", "") or "")
        folder = str(self.config.get("identity", "profile", "") or "")
        out: dict[str, Any] = {"address": address, "profile": folder, "profile_ok": False, "error": None,
                               "session": None,
                               "secrets": [{"key": s.ref.key, "in_keychain": await self.secrets.present(s.ref)}
                                           for s in self.secrets.specs()]}
        if not address:
            out["error"] = "no [mailbox] address in config.toml"
        elif not folder:
            out["error"] = "no [identity] profile in config.toml"
        else:
            try:
                p = await asyncio.to_thread(seed.verify, address, folder)
                out["profile_ok"], out["profile_name"] = True, p.name
            except seed.SeedError as e:
                out["error"] = str(e)
        out["paired"] = self._attach_identity_if_paired()
        if out["profile_ok"] and out["paired"] and self.ferb_identity is not None and params.get("session", True):
            try:
                out["session"] = await self.ferb_identity.session_ok()     # through the extension (wakes it)
            except (FerbError, Exception) as e:
                out["session"], out["error"] = False, f"{type(e).__name__}: {e}"
        return out

    async def _doctor_check(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        """Permissions and self-test as seen by *this* process — the one that will really act."""
        import ferb
        from ferb.device import permissions, selftest
        from ferb.device.install import code_hash

        def run() -> dict[str, Any]:
            results = selftest.run_all() if params.get("selftest", True) else []
            return {"permissions": permissions.check_all(),
                    "selftest": [{"name": r.name, "ok": r.ok, "detail": r.detail} for r in results]}
        out = await asyncio.to_thread(run)
        out["runtimes"] = await self._runtime_status()
        out["voice"] = {"push_to_talk": self._ptt, "installed": self.voice_bundle.exists(),
                        "connected": self.voice is not None and self.voice.app is not None,
                        "permissions": self.voice.permissions if self.voice is not None else {}}
        out.update({"pid": os.getpid(), "executable": os.path.realpath(sys.executable),
                    "version": ferb.__version__, "code_hash": code_hash(Path(ferb.__file__).parent),
                    "home": str(self.home), "stopped": self.state.stopped})
        return out

    async def _runtime_status(self) -> dict[str, Any]:
        import importlib.metadata
        from ferb.device.install import repo_root
        from ferb.runtimes.claude_sdk import SDK_VERSION
        from ferb.runtimes.codex_exec import CODEX_VERSION, check_version, codex_binary
        status: dict[str, Any] = {
            "orchestrator": self.config.get("runtimes", "orchestrator", "codex"),
            "worker": self.config.get("runtimes", "worker", "codex"),
            "unregistered": dict(self.registry.unregistered)}
        try:
            binary = codex_binary(self.home, repo_root())
            await check_version(binary)
            status["codex"] = f"{CODEX_VERSION} ({binary})"
        except FerbError as e:
            status["codex"] = f"problem: {e}"
        installed = importlib.metadata.version("claude-agent-sdk")
        status["claude"] = installed if installed == SDK_VERSION else f"problem: {installed} != pinned {SDK_VERSION}"
        return status

    async def _automation_request(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        """Ask macOS for Apple Events permission to each app (shows the system prompt; must run in the daemon so
        the grant attaches to FERB's interpreter). Apps that aren't running are asked on first use instead."""
        from ferb.device import permissions
        bundle_ids = [str(b) for b in params.get("bundle_ids", [])]
        results = await asyncio.to_thread(lambda: {b: permissions.automation(b, ask=True).value for b in bundle_ids})
        self._journal().append("automation.requested", results)
        return {"results": results}

    async def _logs(self, session: Session, params: dict[str, Any]) -> AsyncIterator[dict[str, Any]]:
        wanted = {LOG_FILTERS[f] for f in params.get("filters", []) if f in LOG_FILTERS} or None
        sub = self.bus.subscribe(wanted, maxsize=int(params.get("maxsize", 1000)))
        try:
            async for ev in sub:
                yield {"topic": ev.topic, "at": ev.at.isoformat(),
                       "class": "notice" if ev.topic == DROPPED else ev.cls.value, "data": self.redact(ev.data)}
        except Overflow:
            yield {"topic": "bus.overflow", "at": self.clock.now().isoformat(), "class": "notice",
                   "data": {"message": "fell behind on critical events; restart `ferb logs`"}}
        finally:
            sub.close()


async def _no_notify(title: str, text: str) -> None:
    return None


def main() -> None:
    asyncio.run(Daemon().run())
