"""Daemon startup / shutdown lifecycle (section 2) and its methods.

Build step 1: ping, trace.get, logs.subscribe. Build step 2: the redaction pipeline on every writer and on
`ferb logs`, the policy engine + ledger + undo-window releases, `ferb stop` / `resume-tasks` / `undo` /
`autonomy`. The emergency stop survives a daemon restart (STOPPED marker) until the owner lifts it.
"""

import asyncio
import json
from datetime import datetime, timedelta
import os
import signal
import subprocess
import sys
from dataclasses import asdict
from pathlib import Path
from typing import Any, AsyncIterator, Awaitable, Callable

from ferb.bus import DROPPED, Bus, Overflow
from ferb.clock.clock import SystemClock
from ferb.config import Config
from ferb.daemon.gateway import FsRoots, Gateway
from ferb.orchestrator.supervisor import Supervisor
from ferb.orchestrator.verify import Checks
from ferb.orchestrator.tree import TaskTree
from ferb.act.browser.tools import IdentityDriver
from ferb.core.types import Identity
from ferb.act.browser import profiles as browser_profiles
from ferb.act.browser.ext_bridge import FerbIdentity, Hub, Router, WakeServer
from ferb.act.browser.ext_protocol import FERB_EXTENSION_ID
from ferb.act.browser.pairing import Pairings
from ferb.act.browser.resolver import Resolver, default_browser, last_used_profile
from ferb.act.gui_lock import GuiLock
from ferb.act.mac.driver import MacComputerDriver
from ferb.act.mac.input import RealInput
from ferb.act.shell.manager import ShellManager
from ferb.registry import add_identity_browser, add_mac, add_owner_browsers, add_shell, add_time, add_trail
from ferb.sense.watchers.owner_input import OwnerInput
from ferb.sense.exclusions import DEFAULT_EXCLUDE_PATHS, Exclusions
from ferb.sense.manager import Sense
from ferb.sense.trail import Attribution, Trail
from ferb.act.fs.files import normalize_path
from ferb.clock.context import ago as context_ago
from ferb.registry import build as build_registry
from ferb.runtimes.proxy import ToolListProxy
from ferb.core.errors import FerbError
from ferb.core.ids import RandomIds
from ferb.core.interfaces import Clock, IdSource
from ferb.daemon.auth import RunTokens
from ferb.daemon.server import Server, Session, ensure_socket_free
from ferb.memory.journal import Journal
from ferb.orchestrator.ledger import Ledger
from ferb.orchestrator.stop import StopControl
from ferb.paths import ferb_home, socket_path
from ferb.policy.autonomy import Level
from ferb.policy.engine import PolicyEngine, Verdict
from ferb.identity.secrets import KeychainSecretStore, specs_from_config
from ferb.sanitize.redact import Redactor
from ferb.store.db import Database
from ferb.store.spans import SpanNode, SpanStore

Redact = Callable[[Any], Any]

# `ferb logs` filter flags -> bus topic patterns
LOG_FILTERS = {"tasks": "task.*", "trail": "trail.event", "tools": "tool.called", "browsers": "browser.*",
               "policy": "policy.*", "actions": "action.*", "timers": "timer.*",
               "voice": "voice.*"}
RELEASE_TICK_S = 0.2
MACOS_TICK_S = 0.2


class Daemon:
    def __init__(self, home: Path | None = None, clock: Clock | None = None, ids: IdSource | None = None,
                 redact: Redact | None = None, owner_uid: int | None = None, undo_window_s: float = 10.0,
                 watch_owner_input: bool = True, watchers: bool = True, distill: bool = True,
                 notifications: bool = True) -> None:
        self.home = home or ferb_home()
        self.clock = clock or SystemClock()
        self.ids = ids or RandomIds()
        self.config = Config(self.home)
        self._redact_override = redact
        self.redact: Redact = redact or Redactor()
        self.bus = Bus(self.clock)
        self.tokens = RunTokens(self.clock)
        self.server = Server(socket_path(self.home), self.tokens, owner_uid)
        self.engine = PolicyEngine(self.clock, str(self.home / "workspace"), level=self._level,
                                   trusted_kinds=self._trusted_kinds, undo_window_s=undo_window_s,
                                   trusted_accounts=self._trusted_accounts, remember_account=self._remember_account)
        self.stop_control = StopControl(self.tokens.revoke_all, self._set_blocked, self.engine.holds.undo_all)
        self.db: Database | None = None
        self.journal: Journal | None = None
        self.spans: SpanStore | None = None
        self.ledger: Ledger | None = None
        self._release_task: asyncio.Task[None] | None = None
        self._macos_task: asyncio.Task[None] | None = None
        self._autonomy = Level.BALANCED
        self.secrets = KeychainSecretStore(lambda: specs_from_config(self.config.load().get("secrets")))
        self.identity_browser: IdentityDriver | None = None    # the extension in Chrome's FERB profile (step 6)
        self.registry = build_registry()
        self.tasks = TaskTree()
        self.gateway: Gateway | None = None
        self.supervisor: Supervisor | None = None
        self._proxy: ToolListProxy | None = None
        # Owner browser profiles through the extension (build step 6a). Tests override the data dirs and the launcher.
        self.browser_data_dirs: dict[str, Path] | None = None
        self.browser_launch: Callable[[list[str]], None] | None = None
        self.browser_default: Callable[[], str | None] = default_browser
        self.pair_load_wait_s = 2.0                          # a profile opened just to pair gets this long to load
        self.pairings: Pairings | None = None
        self.hub: Hub | None = None
        self.router: Router | None = None
        self.ferb_identity: FerbIdentity | None = None
        self.identity_allow_local = False                # tests only: FERB's tools may open local pages
        self.wake = WakeServer()
        # The owner's Mac + shell (build step 7)
        self.watch_owner_input = watch_owner_input
        self.owner_input = OwnerInput()
        self.voice: Any = None                              # push-to-talk (20.1), built at start
        self.voice_speaker: Any = None                      # tests replace FERB's voice
        self._ptt = False
        self.gui = GuiLock()
        self.mac: MacComputerDriver | None = None
        self.shells: ShellManager | None = None
        # Watchers + trail (build step 10). Tests turn the real watchers off (the pipeline still runs).
        self.run_watchers = watchers
        self.trail: Trail | None = None
        self.sense: Sense | None = None
        self.attribution = Attribution()
        self.watch_problems: list[str] = []
        self._excl: tuple[float, Exclusions] | None = None
        # Memory (build step 11). Tests turn the daily distillation loop off (it calls a model).
        self.run_distill = distill
        self.memory: Any = None
        self.distiller: Any = None
        self._distill_task: asyncio.Task[None] | None = None
        # Jev in shadow mode (build step 13): None until a key is in the Keychain
        self.jev: Any = None
        self.jev_post: Any = None                        # tests: a fake Jev endpoint
        # Chat (build step 14a): turns outlive a closed chat; which task trees have a chat attached
        self._turns: set[asyncio.Task[None]] = set()
        self._attached_trees: set[str] = set()
        self._held: list[tuple[str, str]] = []               # notifications waiting for the owner to be back (13)
        self.schedules: Any = None                            # build step 15
        self._runs: set[asyncio.Task[None]] = set()
        self._held_task: asyncio.Task[None] | None = None
        # Timers (build step 14)
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
        self.ledger = Ledger(self.db, self.clock, self.ids)
        self.tasks.attach(self.db)
        interrupted = self.tasks.interrupt_running()          # a restart: what was running didn't finish
        if interrupted:
            self.journal.append("tasks.interrupted_after_restart", {"task_ids": interrupted})
            for tid in {t.split(".", 1)[0] for t in interrupted}:          # chat tells the owner next time (14a)
                goal = next((r["goal"] for r in self.tasks.rows(tid) if r["id"] == tid), "")
                self.db.conn.execute("INSERT OR IGNORE INTO task_results (task_id, at, status, goal, answer, shown) "
                                     "VALUES (?, ?, 'interrupted', ?, ?, 0)",
                                     (tid, self.clock.now().isoformat(), goal,
                                      f"This was interrupted when FERB restarted: {goal[:200]}. Want me to pick it "
                                      "up again?"))
        await self._build_browsers()
        self._build_mac_and_shell()
        self._build_agents()
        await self._build_sense()
        self._build_memory()
        await self._build_jev()
        self._build_timers()
        for name, fn in (("ping", self._ping), ("sense.pause", self._sense_pause),
                         ("schedules.list", self._schedules_list), ("schedules.change", self._schedules_change),
                         ("questions.answer", self._questions_answer),
                         ("chat.status", self._chat_status), ("chat.attach", self._chat_attach),
                         ("task.stop", self._task_stop),
                         ("reminders.list", self._reminders_list), ("reminders.cancel", self._reminders_cancel),
                         ("reminders.unseen", self._reminders_unseen),
                         ("memory.list", self._memory_list), ("memory.show", self._memory_show),
                         ("memory.add", self._memory_add), ("memory.confirm", self._memory_confirm),
                         ("memory.edit", self._memory_edit), ("memory.retract", self._memory_retract),
                         ("memory.forget", self._memory_forget), ("memory.distill", self._memory_distill),
                         ("jev.stats", self._jev_stats), ("jev.enable", self._jev_enable), ("jev.reload", self._jev_reload),
                         ("timeline.get", self._timeline),
                         ("sense.resume", self._sense_resume), ("sense.status", self._sense_status), ("trace.get", self._trace), ("stop.engage", self._stop_engage),
                         ("stop.lift", self._stop_lift), ("undo", self._undo), ("autonomy.get", self._autonomy_get),
                         ("autonomy.set", self._autonomy_set), ("doctor.check", self._doctor_check),
                         ("automation.request", self._automation_request), ("tools.list", self._tools_list),
                         ("tools.call", self._tools_call), ("chat.approve", self._chat_approve),
                         ("identity.check", self._identity_check), ("browsers.list", self._browsers_list),
                         ("browsers.pair", self._browsers_pair), ("browsers.unpair", self._browsers_unpair),
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
            self.engine.stopped = True
            self.stop_control.engaged = True
            self.journal.append("stop.still_engaged", {})
        self._release_task = asyncio.create_task(self._release_loop())
        self._macos_task = asyncio.create_task(self._macos_events())

    async def shutdown(self) -> None:
        if self._release_task:
            self._release_task.cancel()
        if self._macos_task:
            self._macos_task.cancel()
        if self.supervisor:
            self.supervisor.cancel_all()
        if self._proxy:
            self._proxy.close()
        if self.identity_browser is not None:
            await self.identity_browser.shutdown()
        if self.router is not None:
            await self.router.shutdown()
        if self.shells is not None:
            await self.shells.kill_all()
        if self._distill_task is not None:
            self._distill_task.cancel()
        if self.jev is not None:
            self.jev.close()
        if self._timer_task is not None:
            self._timer_task.cancel()
        if self._held_task is not None:
            self._held_task.cancel()
        if self.sense is not None:
            await self.sense.stop()
        if self.owner_input.running:
            self.owner_input.stop()
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

    def publish_verdict(self, v: Verdict) -> None:
        for topic, data in v.events:
            self.bus.publish(topic, self.redact(data))

    def _set_blocked(self, blocked: bool) -> None:
        self.engine.stopped = blocked
        if blocked:
            self.stop_marker.touch(mode=0o600)
        else:
            self.stop_marker.unlink(missing_ok=True)

    def _level(self) -> Level:
        return self._autonomy

    def _trusted_kinds(self) -> frozenset[str]:
        return frozenset(self.config.get("policy", "trusted_kinds", []))

    # ---------- push-to-talk (section 20.1) ----------

    def _build_voice(self) -> None:
        from ferb.cli.chat import question, render
        from ferb.voice.controller import Voice
        from ferb.voice.speak import SaySpeaker
        self.voice = Voice(self._voice_turn, self.engine.approvals.pending, self._resolve_approval,
                           self.voice_speaker or SaySpeaker(),
                           self._journal(), self._launch_voice_app,
                           mode=lambda: str(self.config.get("voice", "mic", "auto")), render=render, question=question,
                           cancel_countdown=self._cancel_countdown, publish=self._publish,
                           enabled=lambda: self._ptt, sounds=lambda: bool(self.config.get("voice", "sounds", True)))
        loop = asyncio.get_running_loop()

        def on_key(code: int, down: bool) -> None:               # the tap's thread: hand off to the loop
            if self._ptt:
                loop.call_soon_threadsafe(self.voice.on_key, code, down)
        self.owner_input.on_key = on_key
        self.server.unary["voice.transcript"] = self._voice_transcript
        self.server.unary["voice.reload"] = self._voice_reload
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

    def _cancel_countdown(self) -> bool:
        """Spoken "stop" during a send / call countdown: the newest pending one is undone."""
        pending = self.engine.holds.pending()
        if not pending:
            return False
        hold = pending[-1]
        if not self.engine.undo(hold.id):
            return False
        self._journal().append("action.undone", {"hold_id": hold.id, "by": "voice"})
        self.bus.publish("action.undone", {"hold_id": hold.id})
        return True

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
            self._journal().append("voice.failed", {"why": str(params["error"])[:300]})
            self.voice._say("I couldn't use the microphone.")
            return {"handled": "error"}
        result: dict[str, Any] = await self.voice.transcript(str(params.get("text", "")))
        return result

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
        self._autonomy = Level(self.config.get("policy", "autonomy", Level.BALANCED.value))
        if self._redact_override is None:
            self.redact = Redactor(set(self.config.get("privacy", "exclude_domains", [])), set(self.config.exclude_apps()))

    def _front_bundle(self) -> str | None:
        if not self.watch_owner_input:                    # tests: the owner's real Mac stays out
            return None
        from ferb.act.mac import apps
        f = apps.frontmost()
        return f.bundle_id if f else None

    def _put_owner_back(self, owner: str) -> None:
        """After waking a browser profile: if a browser took the front from the owner, give it back at once."""
        from ferb.act.browser.profiles import BUNDLES
        from ferb.act.mac import apps
        now = apps.frontmost()
        if now is not None and now.bundle_id != owner and now.bundle_id in BUNDLES.values():
            self._journal().append("browser.focus_returned", {"from": now.bundle_id, "to": owner})
            apps.activate(owner)

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

    async def _release_loop(self) -> None:
        """Undo windows: publish releases as holds come due (the gateway executes them from build step 4)."""
        while True:
            for _hold, verdict in self.engine.release_due():
                self.publish_verdict(verdict)
            await asyncio.sleep(RELEASE_TICK_S)

    # ---------- agents: registry, gateway, runtimes, supervisor (build step 4) ----------

    def _build_agents(self) -> None:
        device = str(self.config.get("device", "id", "unknown-device"))
        workspace = self.home / "workspace"
        workspace.mkdir(parents=True, exist_ok=True)
        roots = FsRoots(str(workspace), tuple(self.config.get("fs", "owner_roots", [])),
                        tuple(self.config.get("fs", "untrusted_roots", ["~/Downloads"])))
        assert self.ledger and self.spans and self.journal
        self.gateway = Gateway(self.registry.capabilities, self.engine, self.ledger, self.spans, self.tasks,
                               self.clock, self.redact, self._publish, roots, device)
        self.gateway.grant_folder, self.gateway.queue_question = self._grant_folder, self._queue_question
        self.gateway.owner_words = self._owner_words
        self.gateway.name_of = lambda t: next((n for b, n in self._installed_apps() if b.lower() == t.lower()), t)
        self.supervisor = Supervisor(make_runtime=self._make_runtime, gateway=self.gateway, engine=self.engine,
                                     tokens=self.tokens, tasks=self.tasks, spans=self.spans, journal=self.journal,
                                     publish=self._publish, clock=self.clock, ids=self.ids, device_id=device,
                                     workspace=str(workspace), on_violation=self._runtime_violation,
                                     on_task_end=self._task_end, facts=self._facts,
                                     checks=Checks(self.ledger, self._urls_of, self._account_of), roots=roots)
        self.stop_control.register("runtimes", self.supervisor.cancel_all)
        self.stop_control.register("identity_browser", self._stop_identity_browser)

    def use_identity_browser(self, driver: IdentityDriver, allow_local: bool = False) -> None:
        """Attach FERB's own browser: registers the browser.* tools; its tabs close with their task and on stop."""
        self.identity_browser = driver
        add_identity_browser(self.registry, driver, allow_local)

    async def _task_end(self, task_id: str) -> None:
        for drv in (self.identity_browser, self.router):
            if drv is not None:
                await drv.release(task_id)
        if self.shells is not None:
            await self.shells.end_task(task_id)              # its processes end with it (unless long-lived)

    def _stop_identity_browser(self) -> None:
        for drv in (self.identity_browser, self.router):
            if drv is not None:
                asyncio.get_event_loop().create_task(drv.shutdown())
        if self.shells is not None:
            asyncio.get_event_loop().create_task(self.shells.kill_all())   # 10.5: kills task-owned processes

    def _unpaired_note(self, browser: str) -> str | None:
        """The FERB extension is installed and connected in this browser but not paired (build step 14a)."""
        if self.hub is None or browser not in self.hub.unpaired.values() or self._owner_extension_connected(browser):
            return None
        name = {"brave": "Brave", "chrome": "Chrome"}.get(browser, browser)
        if not self.config.get("gates", "owner_profiles", False):
            return (f"The FERB extension is installed in {name} but not paired yet. Pairing your real profiles waits "
                    "until FERB's safety tests all pass; tell the owner that, and don't fall back to screenshots of "
                    f"{name}.")
        return (f"The FERB extension is installed in {name} but not paired. Tell the owner to pair it: "
                f"`ferb browsers pair {browser} \"<profile name>\"` (or say yes when ferb chat offers it).")

    def _owner_extension_connected(self, browser: str) -> bool:
        """A paired owner profile of this browser is connected: browser work goes through the extension (9.2)."""
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
        def owner_active() -> bool:                    # foreground input always yields (10.5; yield_to_owner retired)
            return self.owner_input.active()
        self.mac = MacComputerDriver(device, RealInput(owner_active, self.gui, on_wait=owner_wait,
                                                       available=available),
                                     excluded=self.config.exclude_apps, web_password=self._web_password)
        self.mac.snapshot_images = lambda: bool(self.config.get("mac", "snapshot_image", True))   # image + tree
        from ferb.policy.services import communication_bundles
        self.mac.communication = lambda bundle: bundle.lower() in communication_bundles()
        if self.watch_owner_input:                     # the guard: the owner's input never lands in FERB's app (10.5)
            from ferb.sense.watchers.owner_input import InputGuard
            loop = asyncio.get_running_loop()
            mac = self.mac

            def stepped_in() -> None:                  # on the guard's thread: note it, hand the rest to the loop
                self.owner_input.owner_event()
                loop.call_soon_threadsafe(mac.restore_owner, None)
            self.mac.guard = InputGuard(stepped_in)
        add_mac(self.registry, self.mac, self._device_profile, self._owner_extension_connected, self._unpaired_note)
        self.shells = ShellManager(self.home / "logs" / "shell", self._redact_text)
        add_shell(self.registry, self.shells)
        add_trail(self.registry, lambda: self.trail)
        add_time(self.registry, lambda: self.timers, self.clock.now, lambda: self._sleep_until,
                 lambda task_id: (rec.task.deadline if (rec := self.tasks.get(task_id)) else None))

    # ---------- watchers + trail (build step 10) ----------

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
        if self.router is not None:
            self.router.resolver.history = self.trail.owner_site_profile
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

    # ---------- memory (build step 11) ----------

    def _build_memory(self) -> None:
        from ferb.memory import summary
        from ferb.memory.distill import Distiller
        from ferb.memory.items import Memory
        from ferb.memory.recall import context_block
        assert self.db is not None and self.journal is not None
        self.memory = Memory(self.db, self.clock, self.ids, self._publish, on_change=self._export_memory,
                             redact=self._redact_text)
        journal = self.journal
        self.memory.rebuild_summary = lambda day: summary.rebuild(self.db, journal, day)
        self.distiller = Distiller(self.db, self.journal, self.memory, self._make_runtime, self.clock,
                                   purge=self._purge_trail, device_id=str(self.config.get("device", "id", "dev")))
        if self.supervisor is not None:
            self.supervisor.apps = self._installed_apps
            self.supervisor.pasted_is_data = lambda: bool(self.config.get("policy", "pasted_text_is_data", True))
            self.supervisor.environment = self._environment
            self.supervisor.availability = self._availability
            self.supervisor.owner_browser = self._owner_browser
            self.supervisor.recall = lambda words: context_block(self.memory, words)
            self.supervisor.remember = self._remember
        if self.run_distill:
            self._distill_task = asyncio.get_running_loop().create_task(self._distill_loop())

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
                self._journal().append("memory.distill_failed", {"error": f"{type(e).__name__}: {e}"[:300]})
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
        return {"entries": [{"at": e.at.isoformat(), "kind": e.kind, "data": e.data} for e in rows][-500:]}

    # ---------- timers (build step 14) ----------

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
            self.supervisor.owner_email = self._owner_email
        if self.supervisor is not None:
            self.supervisor.timers = self.timers
            self.supervisor.sleep_until = self._sleep_until
        self._timer_task = asyncio.get_running_loop().create_task(self.timers.run())
        self._held_task = asyncio.get_running_loop().create_task(self._flush_held())
        if self.supervisor is not None:
            self.supervisor.replies = self._inbox_replies

    async def _timer_fired(self, t: Any) -> None:
        """The consumer. Sleeps wake their worker; reminders reach the owner once (notification, chat, logs,
        journal), even if a crash made the timer fire again."""
        from ferb.clock.timers import mark_handled
        if t.kind == "sleep":
            ev = self._sleepers.get(t.id)
            if ev is not None:
                ev.set()
            return
        if t.kind == "schedule":
            from ferb.clock.timers import mark_handled as handled
            assert self.db is not None
            if handled(self.db, t.id, self.clock.now()):                 # exactly once, even after a crash
                self._runs.add(asyncio.get_running_loop().create_task(self._run_schedule(t.message, t.overdue_s or 0)))
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
            return '"' + str(self._redact_text(v))[:240].replace("\\", "\\\\").replace('"', '\\"') + '"'
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

    # ---------- chat continuity (build step 14a) ----------

    async def _task_result(self, ev: dict[str, Any], shown: bool) -> None:
        """What FERB told (or will tell) the owner about a task; a macOS notification if chat was closed."""
        assert self.db is not None
        answer = str(ev.get("answer") or ev.get("summary") or "")
        shown = shown or ev.get("status") == "cancelled"          # stopped by the owner: they know
        self.db.conn.execute("INSERT OR REPLACE INTO task_results (task_id, at, status, goal, answer, shown) VALUES "
                             "(?, ?, ?, ?, ?, ?)", (ev["task_id"], self.clock.now().isoformat(), ev.get("status", ""),
                                                    str(ev.get("goal", ""))[:300], answer, int(shown)))
        if not shown and ev.get("status") != "cancelled":
            await self._notify("FERB finished" if ev.get("status") == "done" else "FERB", answer[:200])

    def _approval_notice(self, data: dict[str, Any]) -> None:
        """An approval is waiting and no chat is open for that task: tell the owner (notification)."""
        tid = str(data.get("task_id") or "")
        if tid.split(".", 1)[0] in self._attached_trees:
            return
        text = f"FERB needs your OK: {str(data.get('reason', ''))[:150]} — open ferb chat"
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
                for title, text in held[-5:]:
                    await self._notify_safely(title, text)

    async def _inbox_replies(self, since: datetime) -> list[str]:
        """Replies in FERB's inbox to email FERB sent since then (the "since we last talked" summary)."""
        from ferb.identity.mailbox import FEED_URL, parse_feed
        assert self.db is not None
        sent = [json.loads(r["input_json"]).get("args", {}) for r in self.db.conn.execute(
            "SELECT input_json FROM spans WHERE kind = 'tool.call' AND started_at >= ? AND input_json LIKE ?",
            (since.isoformat(), '%"mailbox.send"%'))]
        subjects = {str(a.get("subject", "")).strip().lower() for a in sent if a.get("subject")}
        if not subjects or self.ferb_identity is None:
            return []
        _, msgs = parse_feed(await self.ferb_identity._read_feed(FEED_URL))
        out = []
        for m in msgs:
            subj = m.subject.lower().removeprefix("re:").strip()
            if subj in subjects and m.issued >= since:
                out.append(f"{m.sender} replied to \"{m.subject}\" {context_ago(m.issued, self.clock.now())}")
        return out

    async def _chat_status(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        """For `ferb chat` when it opens: answers that arrived while it was closed (then marked shown), tasks still
        running, and questions waiting for the owner."""
        assert self.db is not None
        rows = self.db.conn.execute("SELECT * FROM task_results WHERE shown = 0 ORDER BY at").fetchall()
        for r in rows:
            self.db.conn.execute("UPDATE task_results SET shown = 1 WHERE task_id = ?", (r["task_id"],))
        pending = [{"approval_id": ap.id, "capability": ap.capability, "kind": ap.kind, "reason": ap.reason,
                    "options": list(ap.options)} for ap in self.engine.approvals.pending()]
        questions = [{"id": q["id"], "question": q["question"]} for q in self.db.conn.execute(
            "SELECT id, question FROM owner_questions WHERE state = 'open' ORDER BY seq").fetchall()]
        running = [{"task_id": t, "goal": g} for t, g in (self.supervisor.running_trees() if self.supervisor else [])]
        return {"results": [{"task_id": r["task_id"], "status": r["status"], "goal": r["goal"], "answer": r["answer"]}
                            for r in rows], "running": running, "approvals": pending, "questions": questions}

    async def _chat_attach(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        """Chat is open again: approvals for running tasks are asked there, not by notification."""
        for t, _ in (self.supervisor.running_trees() if self.supervisor else []):
            self._attached_trees.add(t)
        return {"ok": True}

    async def _task_stop(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        """Ctrl-C in ferb chat: stop the request's task, recorded as stopped by the owner."""
        assert self.supervisor is not None
        ids = [str(t) for t in params.get("task_ids", [])] or [t for t, _ in self.supervisor.running_trees()]
        stopped = [t for t in ids if self.supervisor.stop_tree(t)]
        for h in self.engine.holds.pending():                     # a send held for undo in that task goes too
            if any(h.action.task_id == t or h.action.task_id.startswith(t + ".") for t in stopped):
                self.engine.holds.undo(h.id)
        for t in stopped:
            self._journal().append("task.stopped", {"task_id": t, "by": "owner"})
        return {"stopped": stopped}

    # ---------- schedules (build step 15) ----------

    def _owner_email(self) -> str | None:
        """The owner's email, once they've told FERB (owner-stated memory, key "email"): progressive setup (8)."""
        import re as _re
        if self.memory is None:
            return None
        for it in self.memory.items("active", "owner"):
            if it.key in ("email", "email_address", "my_email") or "email" in it.key:
                m = _re.search(r"[\w.+-]+@[\w-]+\.[\w.-]+", it.content)
                if m and it.origin == "owner_stated":
                    return m.group(0)
        return None

    def _schedule_create(self, spec: Any, own_words: str, request_id: str) -> str:
        """From chat: the owner's words (quote-checked), the intent recorded now, the first run in the future."""
        from ferb.orchestrator import dag
        from ferb.orchestrator.schedules import Rule, ScheduleError
        from ferb.orchestrator.supervisor import _wants_me
        from ferb.policy.intent import to_json
        from ferb.policy.names import resolve_apps
        norm = " ".join(own_words.lower().split())
        if len(spec.quote.strip()) < 3 or " ".join(spec.quote.lower().split()) not in norm:
            self._journal().append("schedule.refused", {"why": "quote not in the owner's words"})
            return "I couldn't set that up — could you ask me again in your own words?"
        problems = dag.validate_intent(spec.intent, own_words, self._installed_apps())
        if problems:
            return "To schedule that, please name the sites, apps or people involved yourself."
        if _wants_me(spec.intent.recipients) and self._owner_email() is None:
            return "What email address should I use for you? Tell me, then ask me to schedule it again."
        try:
            from ferb.clock.when import resolve
            first = resolve(spec.when, self.clock.now())                        # code computes the date, not the model
            rule = Rule.build(spec.repeat, first, spec.weekday, spec.every_minutes)
            me = self._owner_email()
            bundles = set(resolve_apps(spec.intent.apps, self._installed_apps(), own_words)[0].values())
            from ferb.policy.intent import RequestIntent
            i = spec.intent
            recorded = RequestIntent.from_plan(
                request_id, set(i.verbs), set(i.extra_kinds),
                recipients=frozenset(me if (me and r.strip().lower() in {"me", "myself", "owner"}) else r
                                     for r in i.recipients),
                sites=frozenset(i.sites) | frozenset(b.lower() for b in bundles), accounts=frozenset(i.accounts),
                paths=frozenset(normalize_path(p, str(self.home / "workspace")) for p in i.paths),
                max_amount=i.max_amount, irreversible=frozenset(i.irreversible_kinds))
            s = self.schedules.add(spec.request, own_words, to_json(recorded), rule, first)
        except (ScheduleError, ValueError) as e:
            return f"I couldn't schedule that: {e}."
        t = self.timers.add("schedule", first, s.id)
        self.schedules.set_timer(s.id, t.id, first)
        self._journal().append("schedule.created", {"schedule_id": s.id, "request": s.request, "rule": rule.describe()})
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
        """A due run: the next one is set up first; a run missed by too much is skipped; the result reaches the owner
        (notification, next chat); three failures in a row pause the schedule."""
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
        if overdue_s > s.rule.late_limit_s():
            self._journal().append("schedule.skipped", {"schedule_id": sid, "late_s": round(overdue_s)})
            await self._task_result({"task_id": f"{sid}:{now:%Y%m%d%H%M}", "status": "skipped", "goal": s.request,
                                     "answer": f"I skipped “{s.request}”: it was due "
                                               f"{context_ago(now - timedelta(seconds=overdue_s), now)}, while the "
                                               "Mac was asleep or FERB wasn't running."}, shown=False)
            return
        if self.supervisor is None:
            return
        try:
            final = await self.supervisor.run_scheduled(s, late=overdue_s > 60)
        except Exception as e:                              # a run's failure is reported, never lost
            self._journal().append("schedule.failed", {"schedule_id": sid, "why": f"{type(e).__name__}: {e}"[:300]})
            final = {"task_id": f"{sid}:{now:%Y%m%d%H%M}", "status": "failed",
                     "answer": f"“{s.request}” didn't work this time; I'll try again at the next run."}
        ok = final.get("status") == "done"
        fails = self.schedules.ran(sid, ok)
        answer = str(final.get("answer") or final.get("summary") or "")
        if overdue_s > 60:
            answer = f"(ran late) {answer}"
        await self._task_result({**final, "goal": s.request, "answer": answer}, shown=False)
        if fails >= 3:
            self._schedule_change(sid, "pause")
            await self._notify("FERB paused a schedule", f"“{s.request}” failed 3 times in a row, so I paused it "
                               f"(ferb schedules resume {sid}).", urgent=True)

    # ---------- progressive setup (section 8) ----------

    def _grant_folder(self, folder: str) -> None:
        """The owner said yes: the folder joins [fs] owner_roots (config) and takes effect at once."""
        from ferb.daemon.gateway import FsRoots
        roots = list(self.config.get("fs", "owner_roots", []))
        if folder not in roots:
            roots.append(folder)
            self.config.set("fs", "owner_roots", roots)
        if self.gateway is not None:
            r = self.gateway.roots
            self.gateway.roots = FsRoots(r.workspace, tuple(dict.fromkeys((*r.owner_roots, folder))),
                                         r.untrusted_roots)
        self._journal().append("setup.folder_granted", {"folder": folder})

    def _queue_question(self, kind: str, target: str, text: str) -> None:
        assert self.db is not None
        if self.db.conn.execute("SELECT 1 FROM owner_questions WHERE kind = ? AND target = ? AND state = 'open'",
                                (kind, target)).fetchone():
            return
        self.db.conn.execute("INSERT INTO owner_questions (id, at, kind, target, question, state) VALUES "
                             "(?, ?, ?, ?, ?, 'open')", ("q-" + self.ids.new_id()[:10], self.clock.now().isoformat(),
                                                         kind, target, text))

    async def _questions_answer(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        """The owner's answer, at chat, to a question an unattended run couldn't ask."""
        assert self.db is not None
        r = self.db.conn.execute("SELECT * FROM owner_questions WHERE id = ? AND state = 'open'",
                                 (str(params["id"]),)).fetchone()
        if r is None:
            raise FerbError("no such open question")
        yes = bool(params.get("yes"))
        self.db.conn.execute("UPDATE owner_questions SET state = ? WHERE id = ?", ("yes" if yes else "no", r["id"]))
        if yes and r["kind"] == "folder_access":
            self._grant_folder(r["target"])
        return {"ok": True}

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

    # ---------- Jev (build step 13) ----------

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
        if self.gateway is not None:
            self.gateway.jev = jev
        if self.distiller is not None:
            self.distiller.jev = jev
        if self.supervisor is not None:
            self.supervisor.jev = jev
        if self.router is not None:
            self.router.jev = jev
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
            self.jev.mark_effect("trail.sensitive", ref, "extra redaction")

    async def _jev_stats(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        if self.jev is None:
            return {"configured": False, "stats": []}
        stats = self.jev.stats()
        for st in stats:
            if not st["real_baseline"] and st["answered"]:
                st["examples"] = self._jev_examples(st["question"])
        assert self.db is not None
        recent = self.db.conn.execute("SELECT jev, error FROM decisions WHERE error IS NULL OR error NOT IN "
                                      "('not sampled') ORDER BY seq DESC LIMIT 20").fetchall()
        answered = sum(1 for r in recent if r["jev"] is not None)
        last_error = next((r["error"] for r in recent if r["error"]), None)
        return {"configured": True, "dropped": self.jev.dropped, "stats": stats, "recent_answered": answered,
                "recent_total": len(recent), "last_error": last_error}

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
                out.append(str(self._redact_text(text))[:300])
        return out

    async def _jev_enable(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        from ferb.decide.decider import may_enable
        from ferb.decide.questions import QUESTIONS
        q = str(params["question"])
        if q not in QUESTIONS:
            raise FerbError(f"unknown question {q}")
        enabled = set(self.config.get("jev", "enabled", []))
        if params.get("on", True):
            if self.jev is None:
                raise FerbError("Jev isn't set up: `ferb setup --jev`")
            st = next(x for x in self.jev.stats() if x["question"] == q)
            ok, why = may_enable(st)
            if not ok:
                raise FerbError(f"not yet: {why}")
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
        """Routes for what the owner named, and the environment summary (9.3, build step 16)."""
        import platform
        from ferb.act.mac import apps as appsm
        from ferb.policy import services as svc
        assert self.db is not None
        since = (self.clock.now() - timedelta(days=14)).isoformat()
        rows = self.db.conn.execute(
            "SELECT at, source, data_json FROM trail WHERE actor = 'owner' AND source IN ('app_focus', "
            "'browser_tab') AND at >= ?", (since,)).fetchall()                    # the owner's own activity only
        usage_apps, usage_web = svc.usage_from_trail(rows)
        ferb = self._identity_profile()
        names = {(p.browser, p.folder): p.display_name for p in self._profiles()} if self.pairings else {}
        paired = [(b.browser, b.folder, names.get((b.browser, b.folder), b.folder))
                  for b in (self.pairings.all() if self.pairings else [])
                  if b.identity is Identity.OWNER and (b.browser, b.folder) != ferb]
        prefs = {}
        if self.memory is not None:
            for it in self.memory.items("active", "owner"):
                if it.key.startswith("route_") or it.key.startswith("route:"):
                    prefs["route:" + it.key.split(":", 1)[-1].removeprefix("route_").replace("_", " ")] = it.content
        env = svc.Environment(self._installed_apps(), paired, usage_apps, usage_web, prefs,
                              owner_browser=self._owner_browser(own_words))
        named = svc.services_named(own_words, env)
        routes = [r for s in named if (r := svc.route(s, env)) is not None]
        unknown = [s.name for s in named if svc.route(s, env) is None]
        try:
            running = {a.bundle_id for a in appsm.running()}
        except Exception:
            running = set()
        stable, live = svc.summary(env, platform.mac_ver()[0] or "unknown", named, running)
        return {"routes": routes, "stable": stable, "live": live, "unknown": unknown}

    def _owner_browser(self, own_words: str) -> bool:
        """The owner's words point at their own browser (a paired profile's name counts: "in my Work profile")."""
        from ferb.policy.services import owner_browser_named
        names = [p.display_name for p in self._profiles()] if self.pairings else []
        return owner_browser_named(own_words, names)

    def _owner_words(self, request_id: str, name: str) -> str:
        """For the contact check (16a): this request's own words, the owner's messages over the last day, and what the
        owner has explicitly told FERB to remember about that name — never pasted or read content."""
        import re as _re
        strip = _re.compile(r"<untrusted-data[^>]*>.*?</untrusted-data>", _re.S)
        since = self.clock.now() - timedelta(days=1)
        words = [strip.sub(" ", str(e.data.get("text", ""))) for e in self._journal().entries(since)
                 if e.kind == "chat.message"]
        if self.memory is not None:
            low = name.strip().lower()
            words += [f"{it.subject} {it.content}" for it in self.memory.items("active")
                      if it.origin == "owner_stated" and low and (low in it.content.lower() or low in it.subject.lower())]
        return "\n".join(words)

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
            elif not live:
                shown = ", ".join(f"{b.browser.capitalize()} “{names.get((b.browser, b.folder), b.folder)}”"
                                  for b in owner)
                problems["owner_browsers"] = f"{shown} paired but not open — open it and FERB reconnects within 30 s"
            profiles = ", ".join(f"{b.browser.capitalize()} “{names.get((b.browser, b.folder), b.folder)}”"
                                 for b in live)
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
        """(bundle id, name) of the Mac's apps, for matching the names the owner uses (14a)."""
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

    # ---------- owner browsers (build step 6a) ----------

    def _profiles(self) -> list[browser_profiles.BrowserProfile]:
        return browser_profiles.discover_all(self.browser_data_dirs)

    def _identity_profile(self) -> tuple[str, str] | None:
        folder = self.config.get("identity", "profile")
        return (str(self.config.get("identity", "browser", "chrome")), str(folder)) if folder else None

    def _open_hosts(self) -> list[tuple[str, str, str]]:
        if self.hub is None:
            return []
        return [(b.binding.browser, b.binding.folder, host) for b in self.hub.bridges.values() if b.conn
                for host, _active in b.tabs.values()]

    async def _build_browsers(self) -> None:
        assert self.db and self.ledger
        self.pairings = Pairings(self.db, self.clock)
        self.hub = Hub(self.pairings, self.ledger, self._publish,
                       lambda: str(self.config.get("device", "id", "unknown-device")))
        def data(b: str) -> Path | None:
            return (self.browser_data_dirs or {}).get(b)
        resolver = Resolver(self._profiles, self.pairings.all, self._open_hosts, self._identity_profile,
                            prefer=lambda: list(self.config.get("browser", "prefer_for_actions", ["brave", "chrome"])),
                            default_browser=lambda: self.browser_default(),
                            last_used=lambda b: last_used_profile(b, data(b)))
        await self.wake.start()
        self.router = Router(self.hub, resolver, self._profiles, self.wake, self.browser_launch)
        add_owner_browsers(self.registry, self.router)
        pairings = self.pairings
        self.ferb_identity = FerbIdentity(self.hub, self._identity_profile, self.secrets, self._add_secret_value,
                                          lambda: str(self.config.get("mailbox", "address", "") or ""),
                                          lambda install_id: pairings.key(install_id), self._profiles,
                                          self.wake, self.browser_launch)
        for drv in (self.router, self.ferb_identity):          # a wake never leaves the owner in the browser (9.3)
            drv.front, drv.put_back = self._front_bundle, self._put_owner_back
        self._attach_identity_if_paired()
        self.server.native = self.hub.serve
        self.server.extension_ids = lambda: frozenset(self.config.get("browser", "extension_ids",
                                                                      [FERB_EXTENSION_ID]))

    def _attach_identity_if_paired(self) -> bool:
        """FERB's tools exist only once its identity profile is paired as FERB (10.1)."""
        prof = self._identity_profile()
        b = self.pairings.for_profile(*prof) if (prof and self.pairings) else None
        if b is None or b.identity is not Identity.FERB or self.ferb_identity is None:
            return False
        if self.identity_browser is None:
            self.use_identity_browser(self.ferb_identity, self.identity_allow_local)
        return True

    async def _identity_pair(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        """Pair FERB's identity profile as identity FERB: its Chrome accounts must be exactly FERB's (test 20);
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
        gen0 = before.gen if before is not None else 0
        token = self.pairings.issue_token(prof[0], prof[1], Identity.FERB)
        wake = await self._open_pairing(prof[0], prof[1], token)
        end = self.clock.monotonic() + float(params.get("timeout_s", 60))
        while self.clock.monotonic() < end:
            b = self.pairings.for_profile(*prof)
            br = self.hub.bridges.get(prof) if self.hub else None
            # its own token and a connection attached after it (not a changed binding: re-pairing the same install
            # gives an identical one, and its bridge may still hold the connection from before)
            if token in self.pairings.paired and b is not None and b.identity is Identity.FERB and br is not None \
                    and br.conn and (br is not before or br.gen > gen0):
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
            browser_profiles.open_known_profile(browser, folder, wake, known, **kwargs)
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

    def _trusted_accounts(self) -> frozenset[str]:
        return frozenset(self.config.get("policy", "trusted_accounts", []))

    def _remember_account(self, key: str) -> None:
        keys = sorted(self._trusted_accounts() | {key})
        self.config.set("policy", "trusted_accounts", keys)
        self._journal().append("policy.account_trusted", {"site_profile": key})

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
                        "connected": bool(br and br.conn), "version": br.version if br else "",
                        "refused": self.hub.refused.get(b.install_id) if b else None})
        return {"profiles": out, "extension_ids": sorted(self.server.extension_ids())}

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

    async def _account_of(self, task_id: str, site: str) -> str | None:
        for drv in (self.router, self.ferb_identity):
            if drv is not None:
                acct = await drv.account_of(task_id, site)
                if acct:
                    return acct
        return None

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
                             f"{', connected' if br is not None and br.conn else ', not open right now'}")
            for browser in sorted(set(self.hub.unpaired.values())):
                note = self._unpaired_note(browser)
                if note:
                    lines.append(f"- {note}")
        return "\n".join(lines)

    def _publish(self, topic: str, data: dict[str, Any]) -> None:
        self.bus.publish(topic, self.redact(data))
        if topic == "approval.requested":
            self._approval_notice(data)
        if topic in ("contact.selected", "plan.extended"):                # who a call went to; a route the owner OK'd
            self._journal().append(topic, self.redact(data))

    def _runtime_violation(self, runtime: str, detail: str) -> None:
        """Section 7: a run that shows a tool outside the allowlist unregisters its runtime."""
        self.registry.unregistered[runtime] = detail
        self._journal().append("runtime.unregistered", {"runtime": runtime, "detail": detail})
        self._publish("policy.blocked", {"step": "runtime", "reason": f"{runtime} unregistered: {detail}"})

    def _make_runtime(self, role: str, *, mode: str, tool_names: list[str], schema: dict[str, Any]) -> Any:
        name = str(self.config.get("runtimes", "orchestrator" if role == "orchestrator" else "worker", "codex"))
        if name not in self.registry.runtimes:
            raise FerbError(f"unknown runtime {name!r} in config.toml [runtimes]")
        if name in self.registry.unregistered:
            raise FerbError(f"runtime {name} is unregistered: {self.registry.unregistered[name]}")
        shim = [sys.executable, "-m", "ferb.cli.mcp"]
        if name == "codex":
            from ferb.device.install import repo_root
            from ferb.runtimes.codex_exec import CodexRuntime, codex_binary
            workdir = self.home / "workspace" if mode == "worker" else self.home / "run" / "supervisor"
            return CodexRuntime(codex_binary(self.home, repo_root()), mode, workdir, self.home,  # type: ignore[arg-type]
                                shim_cmd=shim, tool_names=tool_names, output_schema=schema)
        from ferb.runtimes.claude_sdk import ClaudeRuntime
        if self._proxy is None:
            self._proxy = ToolListProxy()
        return ClaudeRuntime(mode, self._proxy, str(self.home), shim_cmd=shim, tool_names=tool_names,  # type: ignore[arg-type]
                             output_schema=schema, model=self.config.get("runtimes", "claude_model"))

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

    def _resolve_approval(self, approval_id: str, granted: bool, choice: int | None = None) -> dict[str, Any]:
        """The owner's answer to a question, typed (`ferb chat`) or spoken (push-to-talk)."""
        ap = self.engine.approvals.resolve(approval_id, granted, choice)
        self._journal().append("approval.resolved", {"approval_id": ap.id, "state": ap.state,
                                                     "capability": ap.capability})
        self._publish("approval.resolved", {"approval_id": ap.id, "state": ap.state})
        return {"approval_id": ap.id, "state": ap.state}

    async def _chat_send(self, session: Session, params: dict[str, Any]) -> AsyncIterator[dict[str, Any]]:
        """One chat turn: the supervisor's events, merged with bus events about the tasks this turn started
        (tool calls, approval requests, undo countdowns, blocks) so `ferb chat` can show them inline."""
        if self.supervisor is None:
            raise FerbError("daemon not started")
        queue: asyncio.Queue[dict[str, Any] | None] = asyncio.Queue()
        my_tasks: set[str] = set()
        attached = {"on": True}                     # False once the owner closes chat: the task keeps running (14a)
        sub = self.bus.subscribe({"tool.called", "approval.requested", "action.*", "policy.blocked",
                                  "worker.exposed"})

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
                self._journal().append("chat.failed", {"why": f"{type(e).__name__}: {self.redact(str(e))}"[:300]})
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
                "stopped": self.engine.stopped}

    async def _trace(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        task_id = str(params.get("task_id", ""))
        if self.spans is None:
            raise FerbError("daemon not started")
        return {"task_id": task_id, "spans": [_node_json(n) for n in self.spans.tree(task_id)]}

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

    async def _undo(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        """`ferb undo [id]`: with no id, undo the only pending send (or list them if several)."""
        hold_id = params.get("hold_id")
        pending = self.engine.holds.pending()
        if hold_id is None:
            if len(pending) != 1:
                return {"undone": None, "pending": [{"hold_id": h.id, "capability": h.action.capability,
                                                     "kind": h.action.kind} for h in pending]}
            hold_id = pending[0].id
        if not self.engine.undo(str(hold_id)):
            raise FerbError(f"nothing to undo for {hold_id} (already sent, undone, or unknown)")
        self._journal().append("action.undone", {"hold_id": hold_id})
        self.bus.publish("action.undone", {"hold_id": hold_id})
        return {"undone": hold_id}

    async def _autonomy_get(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        return {"level": self._autonomy.value}

    async def _autonomy_set(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        level = Level(str(params.get("level")))
        self.config.set("policy", "autonomy", level.value)       # atomic write, stays 0600
        self._autonomy = level
        self._journal().append("autonomy.set", {"level": level.value})
        return {"level": level.value}

    async def _identity_check(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        """Setup / doctor (section 8): FERB's Chrome profile holds exactly FERB's account (safety test 20), and each
        secret is in the Keychain. The session itself is checked through the extension from build step 6."""
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
        """Permissions and self-test as seen by *this* process — the one that will really act (section 8)."""
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
                    "home": str(self.home), "stopped": self.engine.stopped})
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
        bundle_ids = [str(b) for b in params.get("bundle_ids", []) if b != "com.brave.Browser"]  # never Brave
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


def _node_json(node: SpanNode) -> dict[str, Any]:
    s = asdict(node.span)
    for k in ("started_at", "ended_at"):
        s[k] = s[k].isoformat() if s[k] else None
    s["children"] = [_node_json(c) for c in node.children]
    return s


def main() -> None:
    asyncio.run(Daemon().run())
