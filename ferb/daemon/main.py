"""Own daemon composition, lifecycle and RPC handlers. Must not interpret requests or select targets.

The Daemon is composed of one mixin per area (ferb/daemon/areas/): composition, chat, memory, schedules,
banners and notices, browsers, the desktop, shell egress, sensing, the planner's environment, runtimes,
retention, Jev, hearing and the RPC table. This module keeps its state (`__init__`), its lifecycle (start,
shutdown, run) and its error handling."""

import asyncio
import contextlib
import os
import signal
import sys
import threading
from pathlib import Path
from typing import Any, Awaitable, Callable

from ferb.device.autorelease import DrainingExecutor, pool
from ferb.act.browser import downloads, uploads
from ferb.act.browser.ext_bridge import FerbIdentity, Hub, Router, WakeServer
from ferb.act.browser.pairing import Pairings
from ferb.act.browser.tools import IdentityDriver
from ferb.act.evidence.store import EvidenceStore
from ferb.act.fs import recovery
from ferb.act.fs.changes import Changes
from ferb.act.gui_lock import GuiLock
from ferb.act.mac.driver import MacComputerDriver
from ferb.act.shell.local import DirectReports
from ferb.act.shell.manager import ShellManager
from ferb.act.shell.proxy import EgressProxy
from ferb.act.shell.sandbox import Sandbox
from ferb.bus import Bus
from ferb.clock.clock import SystemClock
from ferb.config import UPGRADES, Config
from ferb.core.errors import FerbError
from ferb.core.ids import RandomIds
from ferb.core.interfaces import Clock, IdSource
from ferb.daemon import banners, resources as resource_use
from ferb.daemon.areas.about import About
from ferb.daemon.areas.browsers import Browsers
from ferb.daemon.areas.chat import Chat
from ferb.daemon.areas.compose import Compose
from ferb.daemon.areas.desktop import Desktop
from ferb.daemon.areas.egress import Egress
from ferb.daemon.areas.environment import Environment
from ferb.daemon.areas.intro import Intro
from ferb.daemon.areas.jev import JevGlue
from ferb.daemon.areas.memory import MemoryGlue
from ferb.daemon.areas.memory_import import MemoryImportGlue
from ferb.daemon.areas.notify import Notify
from ferb.daemon.areas.retention import Retention
from ferb.daemon.areas.rpc import LOG_FILTERS, Rpc
from ferb.daemon.areas.runtimes import Runtimes, mcp_shim_command
from ferb.daemon.areas.schedules import Schedules
from ferb.daemon.areas.sensing import Sensing
from ferb.daemon.areas.shared import (FRESH_S, INTERNAL_FRAMES, INTERNAL_MESSAGE_CHARS, MACOS_TICK_S, Redact,
                                      UNHANDLED, _code_path, _Fresh, _no_notify, _raise_file_limit, _Repeats)
from ferb.daemon.areas.voice import VoiceGlue
from ferb.daemon.auth import RunTokens
from ferb.daemon.banners import Banner
from ferb.daemon.gateway import Gateway
from ferb.daemon.server import INTERNAL_ERROR, Server, SLOW_CALL, ensure_socket_free
from ferb.device.persona import discover_persona
from ferb.identity.secrets import KeychainSecretStore, specs_from_config
from ferb.memory.journal import Journal
from ferb.memory.learning import SOURCE_REF_INDEX
from ferb.orchestrator import episodes
from ferb.orchestrator.ledger import Ledger
from ferb.orchestrator.plan_store import PlanStore
from ferb.orchestrator.stop import StopControl
from ferb.orchestrator.supervisor import Supervisor
from ferb.orchestrator.tree import ExecutionState, TaskTree
from ferb.paths import ferb_home, socket_path
from ferb.policy.destinations import Confirmations, KnownDestinations
from ferb.policy.protection import Protection
from ferb.registry import add_recording, add_recovery, build as build_registry
from ferb.runtimes.proxy import ToolListProxy
from ferb.sanitize.redact import Redactor
from ferb.sense.exclusions import Exclusions
from ferb.sense.manager import Sense
from ferb.sense.trail import Attribution, Trail
from ferb.sense.watchers.owner_input import OwnerInput
from ferb.sense.watchers.presence import MacPresence
from ferb.store import salvage
from ferb.store.db import Database, NewerStore
from ferb.store.spans import SpanStore
from ferb.store.task_results import TaskResults

__all__ = ["Daemon", "main", "LOG_FILTERS", "mcp_shim_command", "redacting_hooks", "service_streams"]

LOOP_RESTART_S, LOOP_RESTART_MAX_S = 1.0, 60.0    # a background loop that raised starts again after this (`_supervise`)


class Daemon(Compose, Chat, MemoryGlue, MemoryImportGlue, Schedules, Notify, Browsers, Desktop, Egress, Sensing, Environment, Runtimes,
             Retention, JevGlue, VoiceGlue, About, Intro, Rpc):
    """FERB's daemon: one process that owns the stores, the agents, the drivers and the socket."""

    def __init__(self, home: Path | None = None, clock: Clock | None = None, ids: IdSource | None = None,
                 redact: Redact | None = None, owner_uid: int | None = None,
                 watch_owner_input: bool = True, watchers: bool = True, distill: bool = True,
                 notifications: bool = True, retention: bool = True, resources: bool = True,
                 snapshots: bool = True,
                 persona_scan: Callable[..., Any] | None = discover_persona, screen_at_send: bool = True,
                 preload_secrets: bool = True, embeddings: bool = True) -> None:
        self.home = home or ferb_home()
        self.clock = clock or SystemClock()
        self.ids = ids or RandomIds()
        self.config = Config(self.home)
        self.persona_scan = persona_scan
        self.persona: Any = None
        self._automation_cache: dict[str, tuple[float, bool]] = {}
        self._automation_checks: dict[str, Any] = {}
        self._redact_override = redact
        self.redact: Redact = redact or Redactor()
        self.preload_secrets = preload_secrets      # the redactor learns FERB's Keychain values at start (step 6a)
        from ferb.sense.daily import DailyActivity
        self.daily_activity = DailyActivity(self.home / 'activity' / 'today.jsonl', self.clock.now)
        self.bus = Bus(self.clock)
        from ferb.daemon.events import Events
        self.events = Events(lambda: self.redact)
        self._activity_device_id: tuple[str, Any] | None = None
        self.tokens = RunTokens(self.clock)
        self.server = Server(socket_path(self.home), self.tokens, owner_uid)
        self.state = ExecutionState()
        self.stop_control = StopControl(self.tokens.revoke_all, self._set_blocked)
        self.db: Database | None = None
        self.journal: Journal | None = None
        self.spans: SpanStore | None = None
        self.ledger: Ledger | None = None
        self.evidence: EvidenceStore | None = None           # captured state the checker re-inspects (V1), 7 days
        self._macos_task: asyncio.Task[None] | None = None
        self.secrets = KeychainSecretStore(lambda: specs_from_config(self.config.section("secrets")))
        self.identity_browser: IdentityDriver | None = None    # the extension in Chrome's FERB profile
        from ferb.act.listen import FOLDER as AUDIO_FOLDER, Hearing
        from ferb.act.mac.audio import AppAudio
        from ferb.act.recording import RecordingManager
        from ferb.sanitize.images import ImageRedactor
        from ferb.sense.transcribe import Transcriber
        # L26: hearing an app's sound during a task (ScreenCaptureKit), transcribed on this Mac by FERB.app (the voice
        # helper nested in FERB.app when the daemon runs from the packaged app, L27)
        self.hearing = Hearing(self.home / "workspace" / AUDIO_FOLDER, AppAudio(),
                               Transcriber(self.voice_bundle, self._hearing_requirement, home=self.home),
                               self._redact_text,
                               excluded_host=lambda host: bool(getattr(self.redact, "excluded_host", lambda h: False)(host)))
        self.recordings = RecordingManager(sanitize=ImageRedactor(self.redact if isinstance(self.redact, Redactor) else Redactor()),
                                           hearing=self.hearing,
                                           staging=self.home / "run" / "recordings")   # frames stay in FERB's own state
        self.registry = build_registry()
        add_recording(self.registry, self.recordings)
        self.tasks = TaskTree()
        self.gateway: Gateway | None = None
        self.confirmations: Confirmations | None = None       # D33, S4: the owner's answers to FERB's questions
        self.destinations: KnownDestinations | None = None    # SQ6: the destinations FERB knows
        self.device_id = "unknown-device"                     # [device] id, read at start (_load_config)
        self.protection: Protection | None = None             # the [protection] lists, read once at start (rule 10)
        self._available_cache: dict[str, tuple[float, bool]] = {}
        # Slow reads the planner's facts and availability need on every turn (TCC checks, browser profile files),
        # kept for FRESH_S and refreshed in a worker thread
        self._fresh = _Fresh(FRESH_S)
        self.supervisor: Supervisor | None = None
        self.task_results: TaskResults | None = None          # what FERB told the owner about each task
        self._proxy: ToolListProxy | None = None              # the Claude runtime's tool-list relay
        self.proxy: EgressProxy | None = None                 # FERB's shell's egress proxy: its only way off the Mac
        self.direct_reports: DirectReports | None = None      # the kernel's reports of direct local connections (L24)
        self._loop: asyncio.AbstractEventLoop | None = None
        # Owner browser profiles through the extension. Tests override the data dirs and the launcher.
        self.browser_data_dirs: dict[str, Path] | None = None
        self.browser_launch: Callable[[list[str]], None] | None = None
        self.pair_load_wait_s = 2.0                          # a profile opened just to pair gets this long to load
        self.pair_timeout_s = 60.0                           # how long a pairing page's key may take (as browsers.pair)
        self._handoffs: dict[str, Any] = {}                  # a human check's handoff id -> its tab (browser_handoff)
        self.handoff_poll_s = 2.0                            # how often a handed-off check's page is looked at
        self.pairings: Pairings | None = None
        self.hub: Hub | None = None
        self.router: Router | None = None
        self.ferb_identity: FerbIdentity | None = None
        self.downloads: downloads.Downloads | None = None
        self.uploads: uploads.Uploads | None = None
        self.wake = WakeServer()
        # The owner's Mac + shell
        self.watch_owner_input = watch_owner_input
        self.owner_input = OwnerInput(wall=lambda: self.clock.now().timestamp())   # the daemon's clock
        # whether the Mac is in use (decision 17, clarified 2026-10-31): wake, unlock, login; never input idleness
        self.presence = MacPresence(wall=lambda: self.clock.now().timestamp())
        from ferb.daemon.disturbance import DesktopProbe, DisturbanceSentinel
        self.disturbance = DisturbanceSentinel(DesktopProbe(), self._record_disturbance,
            owner_input=lambda: {"monitored": self.owner_input.running,
                                 "recent": self.owner_input.active(),
                                 "last_event_wall": self.owner_input.last_owner_wall},
            processes=self._running_processes, now=lambda: self.clock.now(),
            paused=lambda: self.sense is not None and self.sense.paused)     # `ferb pause`: no desktop sensing
        self.gui = GuiLock()
        self.mac: MacComputerDriver | None = None
        self.changes: Changes | None = None             # L39: each step's files as they were before it
        self.shells: ShellManager | None = None
        self.sandbox: Sandbox | None = None                 # the shell's sandbox profile, built at start
        self.containment: tuple[bool, str] = (False, "not checked yet")
        # Watchers + trail. Tests turn the real watchers off (the pipeline still runs).
        self.run_watchers = watchers
        self.trail: Trail | None = None
        self.sense: Sense | None = None
        self.perception: Any = None
        self.attention_notices: Any = None
        self.attribution = Attribution()
        self.watch_problems: list[str] = []
        self._excl: tuple[float, Exclusions] | None = None
        # Memory. Tests turn the daily distillation loop off (it calls a model).
        self.run_distill = distill
        self.memory: Any = None
        self.learning: Any = None
        self.distiller: Any = None
        self._distill_task: asyncio.Task[None] | None = None
        # Decision 8: the local embedding model, fetched once and loaded, then new text embedded in the background.
        # Tests turn it off (never a download) and put a fake embedder in place themselves.
        self.run_embeddings = embeddings
        self._embed_task: asyncio.Task[None] | None = None
        # L36 §3: the start's re-index of the search text, in a worker thread; asked to stop at shutdown
        self._reindex_task: asyncio.Task[None] | None = None
        self._reindex_stop = threading.Event()
        self.run_retention = retention
        self._retention_task: asyncio.Task[None] | None = None
        # L30: FERB's resource log, a row of numbers every 5 minutes. Tests turn the timer off (never a real reading).
        self.run_resources = resources
        self.resource_log: Any = None
        self._resources_task: asyncio.Task[None] | None = None
        self.take_snapshots = snapshots                  # S1 local APFS snapshots; tests never take a real one
        # L12: the owner's front window, captured as each message arrives (request id -> FrontWindow | None). Tests
        # never take a real screenshot: they leave it off or put a fake here.
        self.front_window: Callable[[str, int | None], Awaitable[Any]] | None = (
            self._front_window if screen_at_send else None)
        self._tab_granted: set[str] = set()   # browsers whose front tab FERB may read: GRANTED, kept until a read fails
        self._last_runs: set[str] = set()     # one-time schedules marked done whose last run is under way (M9)
        self._schedule_runs: set[str] = set()  # schedules with a run under way now (its next one never overlaps it)
        self.snapshots: recovery.Snapshots | None = None
        # Deletes FERB's own old snapshots in the async retention pass (None: macOS thinning only; tests)
        self.snapshot_deleter: Callable[[str], Awaitable[bool]] | None = (
            recovery.delete_local_snapshot if snapshots else None)
        # Jev in shadow mode: None until a key is in the Keychain
        self.jev: Any = None
        self.jev_post: Any = None                        # tests: a fake Jev endpoint
        # Chat: turns outlive a closed chat; which task trees have a chat attached
        self._shutting_down = False
        self._stop = asyncio.Event()   # run() waits on it: SIGTERM, SIGINT or update.prepare (L34 Phase G)
        self._turns: set[asyncio.Task[None]] = set()
        self._attached_trees: set[str] = set()
        self._asked_kinds: dict[str, str] = {}              # an open question's kind, until it is answered
        self._answered_ids: set[str] = set()                # questions whose answer is recorded (question.answered)
        self._log_subscriptions: set[Any] = set()             # open `logs.subscribe` streams' bus subscriptions (L9)
        # L10: app -> Jev yes batches waiting for that app's next consideration; an app is a key while one runs
        self._attention_waiting: dict[str, list[dict[str, Any]]] = {}
        self._held: list[Banner] = []                        # banners waiting for the owner to be back (13)
        self.posters = banners.Posters()                     # FERB.app's notifications.subscribe streams (L34 D)
        self.schedules: Any = None                            # durable schedules
        self._runs: set[asyncio.Task[None]] = set()
        # Episode cards are read off the loop (L28) by exactly this entry point: the packaged app's stub accepts only
        # exact entry points, so it takes no arguments (the request goes on stdin; the home comes as FERB_HOME).
        self.card_reader = [sys.executable, "-I", "-m", "ferb.memory.card_reader"]
        self._owner_followups: dict[str, tuple[str, str]] = {}  # FERB's Yes/No questions -> what a Yes does (I2, I1)
        # a request that failed inside FERB, or answered late, journaled at most once per window (review Q-I2)
        self._repeats = _Repeats(lambda kind, data: self._journal().append(kind, data), self.clock)
        self._forgetting: set[asyncio.Task[None]] = set()        # work a forget left behind (login names), running
        self._login_names = asyncio.Lock()      # L36 §5 review m-3: one username write at a time, in request order
        self._card_readers: set[asyncio.subprocess.Process] = set()      # in flight; killed at shutdown
        self._card_lock = asyncio.Lock()                                  # one card capture at a time
        self._held_task: asyncio.Task[None] | None = None
        self._background: set["asyncio.Future[Any]"] = set()       # fire-and-forget work (`_spawn`)
        # Timers
        self.timers: Any = None
        self._timer_task: asyncio.Task[None] | None = None
        self._sleepers: dict[str, asyncio.Event] = {}
        self.notify: Callable[[str, str], Awaitable[None]] = self._osascript_notify if notifications else _no_notify
        # L34 C: the setup window's permission checks and requests; tests replace them (nothing there may prompt)
        from ferb.device import permissions as _permissions
        self.permission_check: Callable[[], dict[str, str]] = _permissions.check_all
        self.permission_request: Callable[[str], Any] = _permissions.request
        self._input_monitoring_asked = False                  # asked with the owner's first request
        self.automation_check: Callable[[str], Awaitable[Any]] = (
            lambda bundle: _permissions.automation_async(bundle, ask=False))
        # notes/voice.md decisions 2-3: a voice turn's replies, spoken by the configured provider through one shared
        # path that redacts first; a new turn or a stop interrupts it. Tests replace `build_speaker` (nothing speaks).
        from ferb.voice import speak
        self.build_speaker: Callable[[], Any] = lambda: speak.build(self.config)
        self.speech = speak.Speech(lambda: self.build_speaker(), self._redact_text,
                                   said=lambda words: self._publish("voice.said", {"text": words}),
                                   failed=self._speech_failed)
        self.stop_control.register("speech", self.speech.interrupt)
        self._voiced: set[str] = set()      # request ids whose turn also reads a spoken message merged into it

        def _clipboard_request() -> Any:          # one read, so macOS shows its paste alert (I3); nothing kept
            from ferb.sense.watchers.clipboard import request_access
            return request_access()
        self.clipboard_request: Callable[[], Any] = _clipboard_request
        # L34 C (owner 2026-10-25): FERB's own Claude sign-in for FERB.app; `cli_binary` is looked up when it starts
        from ferb.runtimes import claude_login as _claude_login
        self.claude_sign_in = _claude_login.ClaudeSignIn(
            self.home, lambda: _claude_login.cli_binary(), self._add_secret_value, self._claude_signed_in)
        self._wire_events()

    # ---------- lifecycle: start, shutdown, run ----------

    @property
    def stop_marker(self) -> Path:
        return self.home / "STOPPED"

    async def start(self) -> None:
        """Build everything in dependency order, then serve. A failure part way runs `shutdown` for what was built
        (journaled when the journal exists), then raises; another daemon already serving stops it before anything is
        built or touched."""
        self._loop = asyncio.get_running_loop()
        self._loop.set_exception_handler(self._loop_exception)
        # every asyncio.to_thread job drains its own autorelease pool (PyObjC on long-lived worker threads)
        self._loop.set_default_executor(DrainingExecutor(thread_name_prefix="ferb-worker"))
        self.home.mkdir(parents=True, exist_ok=True)
        os.chmod(self.home, 0o700)
        _raise_file_limit()                     # the egress proxy holds a descriptor per live connection (F5a Task 3)
        ensure_socket_free(self.server.path)   # refuse before touching the database if another daemon runs
        try:
            await self._start()
        except BaseException as e:
            if self.journal is not None:
                with contextlib.suppress(Exception):
                    self.journal.append("daemon.start_failed", {
                        "error": self._redact_text(f"{type(e).__name__}: {e}")[:INTERNAL_MESSAGE_CHARS]})
            with contextlib.suppress(Exception):
                await self.shutdown()
            raise

    def _new_protection_defaults(self) -> None:
        """A never-read default config.toml was never offered (a browser added to the one browser list) is added once
        and journaled with every default offered so far; one the owner removed after that stays removed. The lists
        are read again when it added any."""
        from ferb.policy import protection as protection_lists
        offered = set(protection_lists.NEVER_READ_OFFERED) | {
            e for row in self.journal.of_kind(protection_lists.NEW_DEFAULTS, "key", "protection.never_read")
            for e in row.data.get("offered", [])}
        new = [e for e in protection_lists.DEFAULT_NEVER_READ if e not in offered]
        if not new:
            return
        added = self.config.add_defaults("protection", "never_read", new)
        self.journal.append(protection_lists.NEW_DEFAULTS, {"key": "protection.never_read", "added": added,
                                                            "offered": list(protection_lists.DEFAULT_NEVER_READ)})
        if added:
            self.protection = protection_lists.load(self.config.get, Path.home(), self.home)

    async def _start(self) -> None:
        # What an older FERB left that can't be read is set aside before anything opens it; a database a newer FERB
        # migrated stops the start here, unchanged (owner, 2026-11-01; ferb/store/salvage.py)
        await asyncio.to_thread(salvage.check_home, self.home, salvage.this_build())
        self.config.ensure_privacy_defaults()
        self._load_config()
        self.db = Database(self.home / "ferb.db")
        self.db.migrate()
        self.db.conn.execute(SOURCE_REF_INDEX)  # one an interrupted store clean-out lost (a no-op when it is there)
        self.journal = Journal(self.db, self.clock, self.ids, self.redact)
        if self.preload_secrets:
            self._spawn(self._preload_secrets(), "secret preload")
        self._reindex_task = asyncio.get_running_loop().create_task(self._reindex_search())   # searches wait for it
        self.db.on_wal_kept = lambda after, details: self._journal().append(
            "store.wal_not_truncated", {"after": after, **details})
        self.server.on_internal = self._record_internal
        if self.server.is_app is Server.is_app_default:      # tests may have put their own check in place
            from ferb.daemon.app_peer import AppPeers
            self.server.is_app = AppPeers()                   # auth.APP_ONLY: FERB.app's own process only
        self.server.on_slow = lambda method, seconds: self._repeats.record(
            SLOW_CALL, (SLOW_CALL, method), {"method": method, "seconds": round(seconds, 1)})
        if self.run_resources:                  # its first window starts here, so start-up's own work is in it
            try:
                self.resource_log = resource_use.ResourceLog(
                    self.db, self.clock, resource_use.MacReadings(), self._redact_text, self._perception_counts,
                    preview=lambda: self.gateway.hooks.previews.snapshot() if self.gateway is not None else {})
            except Exception as e:              # FERB runs without it; the next start tries again
                self.journal.append("resources.failed", {"error": f"{type(e).__name__}: {e}"})
        for decided in UPGRADES:                        # each once, in order, and journaled: never silent
            if not self.journal.of_kind("config.upgraded", "upgrade", decided):
                self.journal.append("config.upgraded", {"upgrade": decided, "changes": self.config.upgrade(decided)})
        self._new_protection_defaults()
        from ferb.orchestrator.continuity import LEGACY_UPGRADE, upgrade_legacy
        if not self.journal.of_kind("store.upgraded", "upgrade", LEGACY_UPGRADE):   # once, and journaled (step 8c)
            self.journal.append("store.upgraded", {"upgrade": LEGACY_UPGRADE,
                                                   "changes": upgrade_legacy(self.db, self.redact)})
        self.spans = SpanStore(self.db, self.clock, self.ids, self.redact)
        self.evidence = EvidenceStore(self.home / "evidence", self.clock, self.ids, self.redact, self.db)
        self.snapshots = recovery.Snapshots(recovery.local_snapshot if self.take_snapshots else None, self.db,
                                            self.clock, self.ids, self.redact)
        add_recovery(self.registry, self.snapshots)
        self.ledger = Ledger(self.db, self.clock, self.ids)
        self.tasks.attach(self.db, self.clock.now)
        self.task_results = TaskResults(self.db)
        interrupted = self.tasks.interrupt_running()          # a restart: what was running didn't finish
        if interrupted:
            self.journal.append("tasks.interrupted_after_restart", {"task_ids": interrupted})
            # a tree whose plan didn't end resumes by itself (resume_plans): no "pick it up again?" and no episode yet
            resuming = set(PlanStore(self.db, self.redact).pending())
            for tid in {t.split(".", 1)[0] for t in interrupted} - resuming:   # chat tells the owner next time
                goal = next((r["goal"] for r in self.tasks.rows(tid) if r["id"] == tid), "")
                self.task_results.interrupted(tid, self.clock.now().isoformat(), goal,
                                              f"This was interrupted when FERB restarted: {goal}. Want me to pick it "
                                              "up again?")
                # Its permanent record (L1), from what it recorded before the restart. A failure never stops
                # start-up: episodes.record notes it, or leaves it to retention when even the note fails.
                episodes.record(self.spans, self.journal, tid, self.clock.now().isoformat())
        try:
            await asyncio.to_thread(self.recordings.sweep)  # frames a restart cut off belong to no recording now
        except OSError as e:
            self.journal.append("recordings.startup_failed", {"error": f"{type(e).__name__}: {e}"})
        if self.run_retention:                             # tests leave the real temporary folder alone
            await self._sweep_stale_runs()
        await self._build_browsers()
        await self._build_mac_and_shell()
        self._build_stores()
        self._build_agents()
        await self._build_sense()
        self._build_memory()
        await self._build_jev()
        self._build_timers()
        self._register_rpc()
        await self._prepare_codex()
        await self.server.start()          # refuses to start if another daemon answers on the socket
        session = self._login_session()
        previous = self.journal.recent_values("daemon.started", "session", 1)
        self.journal.append("daemon.started", {"pid": os.getpid()} | ({"session": session} if session else {}))
        await self._tell_set_aside()
        self._mac_started(session is not None and previous != [session])
        # L28: an unknown window is said, never guessed. The catalog shipped with the pinned binary was dumped before
        # serving (`_prepare_codex`), so a fresh install knows each model's window before Codex has ever fetched
        # a catalog for FERB's login.
        for role, problem in self._runtime_limits().items():
            if "problem" in problem:
                self.journal.append("runtime.limits_unknown", {"role": role, **problem})
                print(f"FERB: the {role} runtime {problem['runtime']} has no documented context window: "
                      f"{problem['problem']}", file=sys.stderr)
        unknown = self.ledger.mark_unknown_after_restart()
        if unknown:
            self.journal.append("ledger.unknown_after_restart", {"intent_ids": unknown})
        if self.stop_marker.exists():      # a stop outlives a crash/restart until the owner lifts it
            self.state.stopped = True
            self.stop_control.engaged = True
            self.journal.append("stop.still_engaged", {})
        self._macos_task = asyncio.create_task(self._supervise("macos_events", self._macos_events))
        self._spawn(self._fresh.prime(self._permissions), "permission check")   # the planner's first turn won't wait
        if self.supervisor and not self.state.stopped:
            resumed = asyncio.create_task(self.supervisor.resume_plans())
            self._turns.add(resumed)
            resumed.add_done_callback(self._turns.discard)

    async def shutdown(self) -> None:
        self._shutting_down = True
        await self._settle_forgets()                         # what a forget left behind finishes first
        self._reindex_stop.set()                             # the search re-index ends after its batch (next start resumes)
        if self._reindex_task is not None:
            await asyncio.wait({self._reindex_task})
        if self.persona is not None:
            await self.persona.close()
        await self.claude_sign_in.close()
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
                                self._held_task, self._resources_task, self._embed_task)
                     if job is not None}, "producers")
        if self.supervisor:
            self.supervisor.suspend_plans()
            self.supervisor.cancel_all()
        await drain(self._turns | self._runs | {job for job in self._background if isinstance(job, asyncio.Task)},
                    "task turns")
        if self.supervisor:                                  # learning captures: cancelled, journaled as deferred
            await self.supervisor.stop_episodes()
        self._stop_card_readers()                            # e.g. one a verdict is waiting on
        if self.gateway is not None:                         # shadow advice still running
            await drain(set(self.gateway._background), "gateway background work")
        if self._proxy:
            self._proxy.close()
        await self.speech.close()
        await self.hearing.shutdown()
        await self.recordings.shutdown()
        if self.identity_browser is not None:
            await self.identity_browser.shutdown()
        if self.router is not None:
            await self.router.shutdown()
        if self.shells is not None:
            await self.shells.kill_all()                     # reports each process's network record first
        if self.proxy is not None:
            self.proxy.close()
        if self.direct_reports is not None:                  # after the proxy's last records went out
            self.direct_reports.close()
        if self.jev is not None:
            self.jev.close()
        if self.sense is not None:
            await self.sense.stop()
        if self.owner_input.running:
            self.owner_input.stop()
        await self.disturbance.shutdown()
        await self.wake.close()
        await self.server.close()
        if self.resource_log is not None:                    # the minutes since the last sample (L30)
            self._sample_resources()
        if self.journal:
            with contextlib.suppress(Exception):             # repeats counted in an open window (review Q-I2)
                self._repeats.flush()
            self.journal.append("daemon.stopped", {"pid": os.getpid()})
        if self.db:
            self.db.close()

    async def run(self) -> None:
        """Run until SIGTERM / SIGINT, or until an update stops FERB (`update.prepare`), then shut down cleanly. The exit
        is clean, so launchd doesn't start the daemon again (KeepAlive SuccessfulExit false). The signal handlers come
        first: a stop asked while FERB is still starting shuts it down as soon as the start is done."""
        loop = asyncio.get_running_loop()
        for sig in (signal.SIGTERM, signal.SIGINT):
            loop.add_signal_handler(sig, self._stop.set)
        await self.start()
        await self._stop.wait()
        await self.shutdown()

    # ---------- errors and background work ----------

    def _loop_exception(self, loop: asyncio.AbstractEventLoop, context: dict[str, Any]) -> None:
        """The event loop's handler for an exception nobody retrieved (a task that failed with no one awaiting it, a
        callback that raised): journaled redacted, once per window for the same place (`_Repeats`), never printed
        unredacted to stderr as asyncio's default handler does."""
        import traceback
        error = context.get("exception")
        job = context.get("task") or context.get("future")
        coro = getattr(job, "get_coro", lambda: None)() if job is not None else None
        frames = ([f"{_code_path(f.filename)}:{f.lineno}:{f.name}" for f in traceback.extract_tb(error.__traceback__)]
                  if isinstance(error, BaseException) else [])
        record = {"message": self._redact_text(str(context.get("message", "")))[:INTERNAL_MESSAGE_CHARS],
                  "error": type(error).__name__ if error is not None else None,
                  "detail": self._redact_text(str(error))[:INTERNAL_MESSAGE_CHARS] if error is not None else None,
                  "task": getattr(coro, "__qualname__", None), "frames": frames[-INTERNAL_FRAMES:]}
        try:
            self._repeats.record(UNHANDLED, (UNHANDLED, str(record["error"]), *frames[-1:]), record)
        except Exception as journal_error:
            print(f"FERB: an unhandled {record['error'] or 'error'} ({record['message']}) couldn't be journaled "
                  f"({type(journal_error).__name__})", file=sys.stderr)

    def _spawn(self, work: Awaitable[Any], what: str) -> "asyncio.Future[Any]":
        """Fire-and-forget work: kept referenced until it ends (a dropped task can be collected mid-run), drained at
        shutdown, and a failure journaled (`daemon.background_failed`, redacted) instead of lost."""
        job = asyncio.ensure_future(work)
        self._background.add(job)

        def ended(done: "asyncio.Future[Any]") -> None:
            self._background.discard(done)
            if done.cancelled() or done.exception() is None:
                return
            exc = done.exception()
            try:
                self._journal().append("daemon.background_failed", {
                    "what": what, "error": self._redact_text(f"{type(exc).__name__}: {exc}")[:INTERNAL_MESSAGE_CHARS]})
            except Exception:
                pass
        job.add_done_callback(ended)
        return job

    async def _supervise(self, name: str, loop: Callable[[], Awaitable[None]]) -> None:
        """Run a background loop that should never end; if it raises, journal it (`daemon.loop_failed`, redacted) and
        start it again after a pause that grows with each failure in a row (LOOP_RESTART_S up to LOOP_RESTART_MAX_S)."""
        failures = 0
        while True:
            try:
                await loop()
                return                                          # a loop that ends by itself (e.g. no CoreFoundation)
            except asyncio.CancelledError:
                raise
            except Exception as e:
                failures += 1
                with contextlib.suppress(Exception):
                    self._journal().append("daemon.loop_failed", {
                        "loop": name, "failures": failures,
                        "error": self._redact_text(f"{type(e).__name__}: {e}")[:INTERNAL_MESSAGE_CHARS]})
                await asyncio.sleep(min(LOOP_RESTART_MAX_S, LOOP_RESTART_S * 2 ** (failures - 1)))

    # ---------- small helpers every area uses ----------

    def _set_blocked(self, blocked: bool) -> None:
        self.state.stopped = blocked
        if blocked:
            self.stop_marker.touch(mode=0o600)
        else:
            self.stop_marker.unlink(missing_ok=True)

    async def _macos_events(self) -> None:
        """Give macOS's event system a turn on the daemon's thread, a few times a second. Without it, its running-app
        list never refreshes in the daemon: an app launched after the daemon started stayed "not running" for FERB
        (owner's WhatsApp report, 2026-10-01). One zero-wait turn takes under a millisecond."""
        try:
            import CoreFoundation as CF
        except ImportError:
            return
        while True:
            with pool():                                     # the event loop's thread lives as long as the daemon
                CF.CFRunLoopRunInMode(CF.kCFRunLoopDefaultMode, 0, True)
            await asyncio.sleep(MACOS_TICK_S)

    def _redact_text(self, text: str) -> str:
        f = getattr(self.redact, "text", None)
        return str(f(text)) if f else text

    def _record_internal(self, method: str, error: BaseException) -> None:
        """A request that failed inside FERB (server.py's catch-all, whose reply names only the exception's type): its
        method, the exception's type, its message redacted (rule 14) and cut short, and where it was raised as
        file:line:function, innermost last, journaled for `ferb stats` (L36: a notch answer that failed left no
        trace), once per window for the same method, type and innermost frame (`_Repeats`). An exception recorded
        already (while a hidden answer was still redacted by its value: `_resolve_approval`) isn't recorded again."""
        import traceback
        if getattr(error, "_ferb_recorded", False):
            return
        with contextlib.suppress(AttributeError):
            setattr(error, "_ferb_recorded", True)
        frames = [f"{_code_path(f.filename)}:{f.lineno}:{f.name}" for f in traceback.extract_tb(error.__traceback__)]
        self._repeats.record(INTERNAL_ERROR, (INTERNAL_ERROR, method, type(error).__name__, *frames[-1:]),
                             {"method": method, "error": type(error).__name__,
                              "message": self._redact_text(str(error))[:INTERNAL_MESSAGE_CHARS],
                              "frames": frames[-INTERNAL_FRAMES:]})

    def _redact_id(self, text: str) -> str:
        """Text that is one of FERB's own ids (a card's task-id key): redacted by structure (`Redactor.ident`)."""
        f = getattr(self.redact, "ident", None)
        return str(f(text)) if f else self._redact_text(text)

    def _journal(self) -> Journal:
        if self.journal is None:
            raise FerbError("daemon not started")
        return self.journal


def redacting_hooks(redact: Callable[[str], str], err: Any = None) -> None:
    """Exceptions that reach the interpreter (the main thread, another thread, an unraisable one in a finalizer) are
    printed to stderr (launchd's `daemon.err.log`) redacted by the daemon's redactor, never raw: a traceback's message
    can hold a value FERB read (rule 14). One that the redactor itself fails on prints only its type."""
    import threading
    import traceback

    def write(kind: type[BaseException] | None, error: BaseException | None, tb: Any, where: str = "") -> None:
        out = err or sys.stderr
        try:
            text = "".join(traceback.format_exception(kind, error, tb)) if kind is not None else ""
            out.write((f"FERB: {where}\n" if where else "") + str(redact(text)))
        except Exception:
            out.write(f"FERB: an unhandled {getattr(kind, '__name__', 'error')} (its text withheld)\n")
        out.flush()
    sys.excepthook = lambda kind, error, tb: write(kind, error, tb)
    threading.excepthook = lambda a: write(a.exc_type, a.exc_value, a.exc_traceback,
                                           f"in thread {getattr(a.thread, 'name', '?')}")
    sys.unraisablehook = lambda u: write(type(u.exc_value) if u.exc_value is not None else None, u.exc_value,
                                         u.exc_traceback, str(redact(str(u.err_msg or "unraisable exception"))))


def service_streams(home: Path) -> None:
    """The daemon's working folder is FERB's home, however it was started (FERB.app's service definition is fixed
    inside the signed app and can't name the owner's home; a checkout's LaunchAgent names it too), and its output goes
    to FERB's logs when launchd gave it none (logs/daemon.out.log, daemon.err.log, as the checkout's LaunchAgent
    sets). The working folder is never on sys.path (`-P`, or FERB.app's isolated agent)."""
    import stat
    logs = home / "logs"
    logs.mkdir(parents=True, exist_ok=True)
    os.chdir(home)
    null = os.stat(os.devnull).st_rdev
    for fd, name in ((1, "daemon.out.log"), (2, "daemon.err.log")):
        try:
            st = os.fstat(fd)
        except OSError:
            st = None
        if st is None or (stat.S_ISCHR(st.st_mode) and st.st_rdev == null):
            target = os.open(logs / name, os.O_WRONLY | os.O_CREAT | os.O_APPEND, 0o600)
            os.dup2(target, fd)
            os.close(target)


def main() -> None:
    service_streams(ferb_home())
    daemon = Daemon()
    redacting_hooks(daemon._redact_text)
    try:
        asyncio.run(daemon.run())
    except NewerStore as e:
        # Not a crash: nothing was changed, and a clean exit keeps launchd from starting it again and again (KeepAlive
        # SuccessfulExit false); FERB.app's setup says the same when it next runs
        print(f"FERB: {e}", file=sys.stderr)
