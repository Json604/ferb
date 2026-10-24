"""Own daemon composition, lifecycle and RPC handlers. Must not interpret requests or select targets."""

import asyncio
import json
from datetime import datetime, timedelta
from functools import partial
import os
import signal
import subprocess
import sys
import threading
import time
from pathlib import Path
from typing import Any, AsyncIterator, Awaitable, Callable, NamedTuple

from ferb.bus import DROPPED, Bus, Overflow
from ferb.clock.clock import SystemClock
from ferb.config import LIMITS_DECIDED, Config
from ferb.daemon.gateway import FsRoots, Gateway
from ferb.orchestrator.supervisor import Supervisor
from ferb.orchestrator.verify import Checks
from ferb.orchestrator.worker import couldnt, reason
from ferb.orchestrator.tree import TaskTree
from ferb.act.browser.tools import IdentityDriver
from ferb.core.types import Identity
from ferb.act.browser import downloads, profiles as browser_profiles, uploads
from ferb.act.browser.ext_bridge import FerbIdentity, Hub, Router, WakeServer
from ferb.act.browser.autoinstall import accepted_extension_ids
from ferb.act.evidence.store import EvidenceStore
from ferb.act.fs import recovery
from ferb.act.browser.pairing import Pairings
from ferb.act.browser.resolver import Resolver
from ferb.act.gui_lock import GuiLock
from ferb.act.mac.driver import MacComputerDriver, WebSecrets, profiles_dir, same_title
from ferb.act.mac.profile import Profiles
from ferb.act.mac.input import RealInput
from ferb.act.shell.manager import ProcInfo, ShellManager
from ferb.act.shell.proxy import EgressProxy, local_host
from ferb.act.shell.local import DirectReports, LocalAccess
from ferb.act.shell.sandbox import SANDBOX_EXEC, Sandbox
from ferb.registry import add_context_readers, add_hearing, add_recording, add_evidence, add_recovery, add_identity_browser, add_mac, add_owner_browsers, add_shell, add_time, add_trail
from ferb.sense.watchers.owner_input import OwnerInput
from ferb.sense.exclusions import DEFAULT_EXCLUDE_PATHS, Exclusions
from ferb.sense.manager import Sense
from ferb.sense.trail import Attribution, Trail
from ferb.act.fs.files import normalize_path
from ferb.clock.context import ago as context_ago
from ferb.registry import build as build_registry
from ferb.runtimes.proxy import ToolListProxy
from ferb.core import origin as origin_
from ferb.core.errors import Deferred, FerbError, NoTarget, Refused, Withheld
from ferb.core.ids import RandomIds
from ferb.core.interfaces import Clock, IdSource
from ferb.daemon import resources as resource_use   # not `resources`: the constructor's flag
from ferb.daemon.auth import RunTokens
from ferb.daemon.server import Server, Session, ensure_socket_free
from ferb.memory.journal import Journal
from ferb.memory.learning import (PLANNER_MEMORY_ITEMS, PROMPT_LEAVES_OUT, SEARCH_TEXT_VERSION, SOURCE_REF_INDEX,
                                  WORKER_MEMORY_ITEMS, reindex_search, search_indexed)
from ferb.orchestrator import acks, episodes
from ferb.orchestrator.ledger import Ledger
from ferb.orchestrator.stop import StopControl
from ferb.trace import compose
from ferb.paths import ferb_home, socket_path
from ferb.policy import confirm, protection as protection_lists
from ferb.policy.destinations import Confirmations, KnownDestinations
from ferb.policy.protection import Protection
from ferb.orchestrator.tree import ExecutionState
from ferb.identity.secrets import KeychainSecretStore, specs_from_config
from ferb.sanitize.redact import REDACTED, Redactor
from ferb.store import retention
from ferb.store.db import Database
from ferb.store.spans import SpanStore
from ferb.device.persona import discover_persona

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


def _raise_file_limit() -> None:
    """Set the open-file soft limit to the hard limit clamped to kern.maxfilesperproc, so the egress proxy's
    per-connection descriptors (and everything else FERB holds open) don't hit EMFILE, and children still see a finite
    OPEN_MAX. Best effort; a failure never stops the daemon."""
    try:
        import resource
        soft, hard = resource.getrlimit(resource.RLIMIT_NOFILE)
        target = hard if hard != resource.RLIM_INFINITY else soft
        per_proc = _sysctl_int(b"kern.maxfilesperproc")
        if per_proc:                                          # finite, so children see a finite OPEN_MAX
            target = min(target, per_proc) if hard != resource.RLIM_INFINITY else per_proc
        if target != soft:
            resource.setrlimit(resource.RLIMIT_NOFILE, (target, hard))
    except (OSError, ValueError, ImportError):
        pass


def _sysctl_int(name: bytes) -> int | None:
    import ctypes
    try:
        value, size = ctypes.c_int(0), ctypes.c_size_t(ctypes.sizeof(ctypes.c_int))
        libc = ctypes.CDLL("/usr/lib/libSystem.B.dylib")
        if libc.sysctlbyname(name, ctypes.byref(value), ctypes.byref(size), None, 0) != 0:
            return None
        return int(value.value) or None
    except OSError:
        return None


def mcp_shim_command() -> list[str]:
    """How runtimes start FERB's MCP shim. Isolated (-I): they start it in FERB's workspace, which workers write, and
    `python -m` would otherwise put that folder first on sys.path, ahead of FERB's own package."""
    return [sys.executable, "-I", "-m", "ferb.cli.mcp"]


def _named_node(node: dict[str, Any]) -> str:
    """How FERB names a learned item to the owner in a "Forget for good?" question; a forget of it scrubs this too."""
    return f"learned item “{' '.join(str(node['key']).split())}” ({node['kind']})"


# L36 §2: FERB's questions about one of its guesses (a chat paraphrase; distillation asks with distill.KEEP_GUESS)
BACKLOG_OFFERED = "memory.backlog_offered"     # L36: the older guesses are offered once, as one list
KEEP_WORDING = "Keep this, in FERB's words (you didn't say it this way)? “{text}”"
EDIT_QUESTION = ("How should FERB remember it? Type it in your own words; FERB saves exactly what you type. "
                 "(Its guess: “{text}”)")
# An Edit answer that secret redaction hid whole (rule 14): never saved as the owner's words
EDIT_HIDDEN = ("Those words looked like a secret, so FERB hid them and saved nothing. Type how FERB should remember it, "
               "without the secret.")


def _all_hidden(text: str) -> bool:
    """Words of which redaction left nothing but its marks ("[redacted]", "[redacted] [redacted]")."""
    return bool(text.strip()) and not text.replace(REDACTED, "").strip()


# Automatic pairing (L25, owner 2026-10-24) only where handing a page to one profile is verified: Chrome (spike 19,
# pairing) and Brave (its profiles paired live). Elsewhere `ferb browsers pair` pairs by hand.
AUTOPAIR_BROWSERS = ("chrome", "brave")
# A paired owner profile that isn't connected, where FERB can't start it without a window (no wake service): plain
# words, not a fault (live 2026-10-24: "extension transport disconnected" read as broken)
IDLE_PROFILE = "not connected now — FERB connects once that profile is open in its browser"
# Chromium writes Local State and Preferences up to 10 s after a change (ImportantFileWriter's commit interval), so
# a window switch or FERB being turned on shows on disk that late. A technical wait, not a behaviour limit.
PREFS_SETTLE_S = 12.0


def _front_app() -> str | None:
    """The frontmost app's bundle id (Accessibility, as the app-focus watcher reads it); None when it can't be read."""
    from ferb.sense.watchers.base import focused_app
    try:
        f = focused_app()
    except Exception:
        return None
    return f.bundle_id if f else None


class _Rescrub:
    """A forget's echo scrub to run again as the work running at the forget ends (fix wave 6a): what that work records
    after the scrub (spans, answer, records, chat reply) loses the item too, and nothing else is touched. `work` runs
    the same scrub with the same rules (the forget's Forgotten and line rules, or its NodeEcho) for `running` (the task
    trees, requests and unfinished spans of that moment); both are kept in memory only, never written down, until the
    last job in `waiting` ends or the daemon stops, which can be days for a task waiting on the owner. `queued`: a run
    is waiting to start, and will see what was recorded meanwhile."""

    def __init__(self, work: Callable[[], Awaitable[None]], waiting: set[asyncio.Task[Any]], running: Any) -> None:
        self.work, self.waiting, self.running, self.queued = work, waiting, running, False


class _PurgePlan(NamedTuple):
    """One retention pass: its clock, its windows, the roots it handles and how many wait for the next pass."""
    now: datetime
    trace_days: int
    decision_days: int
    evidence_days: int
    recovery_days: int
    resource_days: int
    roots: list[str]
    left: int


class Daemon:
    def __init__(self, home: Path | None = None, clock: Clock | None = None, ids: IdSource | None = None,
                 redact: Redact | None = None, owner_uid: int | None = None,
                 watch_owner_input: bool = True, watchers: bool = True, distill: bool = True,
                 notifications: bool = True, retention: bool = True, resources: bool = True, autopair: bool = True,
                 snapshots: bool = True,
                 persona_scan: Callable[..., Any] | None = discover_persona, screen_at_send: bool = True) -> None:
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
        from ferb.sense.daily import DailyActivity
        self.daily_activity = DailyActivity(self.home / 'activity' / 'today.jsonl', self.clock.now)
        self.bus = Bus(self.clock)
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
        self.secrets = KeychainSecretStore(lambda: specs_from_config(self.config.load().get("secrets")))
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
        self.protection: Protection | None = None             # the [protection] lists, read once at start (rule 10)
        self._available_cache: dict[str, tuple[float, bool]] = {}
        self.supervisor: Supervisor | None = None
        self._proxy: ToolListProxy | None = None              # the Claude runtime's tool-list relay
        self.proxy: EgressProxy | None = None                 # FERB's shell's egress proxy: its only way off the Mac
        self.direct_reports: DirectReports | None = None      # the kernel's reports of direct local connections (L24)
        self._loop: asyncio.AbstractEventLoop | None = None
        # Owner browser profiles through the extension. Tests override the data dirs and the launcher.
        self.browser_data_dirs: dict[str, Path] | None = None
        self.browser_launch: Callable[[list[str]], None] | None = None
        self.pair_load_wait_s = 2.0                          # a profile opened just to pair gets this long to load
        self.pair_timeout_s = 60.0                           # how long a pairing page's key may take (as browsers.pair)
        self.front_app: Callable[[], str | None] = _front_app   # the frontmost app's bundle id, for automatic pairing
        self.run_autopair = autopair
        self._autopair_task: asyncio.Task[None] | None = None
        self._autopair_again = False
        self._autopair_tried: set[tuple[str, str]] = set()  # one automatic try per profile while FERB runs
        self.pairings: Pairings | None = None
        self.hub: Hub | None = None
        self.router: Router | None = None
        self.ferb_identity: FerbIdentity | None = None
        self.downloads: downloads.Downloads | None = None
        self.uploads: uploads.Uploads | None = None
        self.wake = WakeServer()
        # The owner's Mac + shell
        self.watch_owner_input = watch_owner_input
        self.owner_input = OwnerInput()
        from ferb.daemon.disturbance import DesktopProbe, DisturbanceSentinel
        self.disturbance = DisturbanceSentinel(DesktopProbe(), self._record_disturbance,
            owner_input=lambda: {"monitored": self.owner_input.running,
                                 "recent": self.owner_input.active(),
                                 "last_event_wall": self.owner_input.last_owner_wall},
            processes=self._running_processes, now=lambda: self.clock.now(),
            paused=lambda: self.sense is not None and self.sense.paused)     # `ferb pause`: no desktop sensing
        self.voice: Any = None                              # push-to-talk, built at start
        self.voice_speaker: Any = None                      # tests replace FERB's voice
        self._ptt = False
        self.gui = GuiLock()
        self.mac: MacComputerDriver | None = None
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
        self.consolidator: Any = None
        self.distiller: Any = None
        self._distill_task: asyncio.Task[None] | None = None
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
        self._log_subscriptions: set[Any] = set()             # open `logs.subscribe` streams' bus subscriptions (L9)
        # L10: app -> Jev yes batches waiting for that app's next consideration; an app is a key while one runs
        self._attention_waiting: dict[str, list[dict[str, Any]]] = {}
        self._held: list[tuple[str, str]] = []               # notifications waiting for the owner to be back (13)
        self.schedules: Any = None                            # durable schedules
        self._runs: set[asyncio.Task[None]] = set()
        # Episode cards are read off the loop (L28) by exactly this entry point: the packaged app's stub accepts only
        # exact entry points, so it takes no arguments (the request goes on stdin; the home comes as FERB_HOME).
        self.card_reader = [sys.executable, "-I", "-m", "ferb.memory.card_reader"]
        self._owner_followups: dict[str, tuple[str, str]] = {}  # FERB's Yes/No questions -> what a Yes does (I2, I1)
        self._forgetting: set[asyncio.Task[None]] = set()        # forgets whose echo scrub is still running
        self._rescrubs: set[_Rescrub] = set()           # forgets waiting for jobs running at them to end (fix wave 6a)
        self._turn_requests: dict[asyncio.Task[Any], str] = {}  # a running chat turn -> its request
        self._card_readers: set[asyncio.subprocess.Process] = set()      # in flight; killed at shutdown
        self._card_lock = asyncio.Lock()                                  # one card capture at a time
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
        _raise_file_limit()                     # the egress proxy holds a descriptor per live connection (F5a Task 3)
        ensure_socket_free(self.server.path)   # refuse before touching the database if another daemon runs
        self.config.ensure_privacy_defaults()
        self._load_config()
        self.db = Database(self.home / "ferb.db")
        self.db.migrate()
        self.db.conn.execute(SOURCE_REF_INDEX)  # one an interrupted store clean-out lost (a no-op when it is there)
        self.journal = Journal(self.db, self.clock, self.ids, self.redact)
        self._reindex_task = asyncio.get_running_loop().create_task(self._reindex_search())   # searches wait for it
        self.db.on_wal_kept = lambda after, details: self._journal().append(
            "store.wal_not_truncated", {"after": after, **details})
        if self.run_resources:                  # its first window starts here, so start-up's own work is in it
            try:
                self.resource_log = resource_use.ResourceLog(self.db, self.clock, resource_use.MacReadings(),
                                                          self._redact_text, self._perception_counts)
            except Exception as e:              # FERB runs without it; the next start tries again
                self.journal.append("resources.failed", {"error": f"{type(e).__name__}: {e}"})
        if not self.journal.of_kind("config.upgraded", "upgrade", LIMITS_DECIDED):   # once, and journaled: never silent
            self.journal.append("config.upgraded", {"upgrade": LIMITS_DECIDED,
                                                    "changes": self.config.upgrade_ferb_written()})
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
        try:
            await asyncio.to_thread(self.recordings.sweep)  # frames a restart cut off belong to no recording now
        except OSError as e:
            self.journal.append("recordings.startup_failed", {"error": f"{type(e).__name__}: {e}"})
        await self._build_browsers()
        self._build_mac_and_shell()
        self._build_agents()
        await self._build_sense()
        self._build_memory()
        await self._build_jev()
        self._build_timers()
        for name, fn in (("ping", self._ping), ("sense.pause", self._sense_pause),
                         ("schedules.list", self._schedules_list), ("schedules.change", self._schedules_change),
                         ("chat.status", self._chat_status), ("results.ack", self._results_ack),
                         ("chat.attach", self._chat_attach),
                         ("setup.persona", self._setup_persona),
                         ("task.stop", self._task_stop),
                         ("reminders.list", self._reminders_list), ("reminders.cancel", self._reminders_cancel),
                         ("reminders.unseen", self._reminders_unseen),
                         ("memory.learning", self._memory_learning), ("memory.list", self._memory_list), ("memory.show", self._memory_show),
                         ("memory.add", self._memory_add), ("memory.confirm", self._memory_confirm),
                         ("memory.edit", self._memory_edit), ("memory.retract", self._memory_retract),
                         ("memory.forget", self._memory_forget), ("memory.distill", self._memory_distill),
                         ("jev.stats", self._jev_stats), ("jev.enable", self._jev_enable), ("jev.reload", self._jev_reload), ("runtimes.check", self._runtimes_check),
                         ("timeline.get", self._timeline),
                         ("recovery.list", self._recovery_list), ("recovery.restore", self._recovery_restore),
                         ("sense.resume", self._sense_resume), ("sense.status", self._sense_status), ("trace.get", self._trace), ("verdict.set", self._verdict_set), ("stop.engage", self._stop_engage),
                         ("stop.lift", self._stop_lift),
                         ("doctor.check", self._doctor_check), ("clipboard.request", self._clipboard_request),
                         ("automation.request", self._automation_request), ("tools.list", self._tools_list),
                         ("tools.call", self._tools_call), ("chat.approve", self._chat_approve),
                         ("chat.answer", self._chat_answer),
                         ("test.mode", self._test_mode),
                         ("identity.check", self._identity_check), ("browsers.list", self._browsers_list),
                         ("browsers.pair", self._browsers_pair), ("browsers.installed", self._browsers_installed), ("browsers.unpair", self._browsers_unpair),
                         ("identity.pair", self._identity_pair), ("ps.list", self._ps_list),
                         ("ps.kill", self._ps_kill)):
            self.server.unary[name] = fn
        self.server.unary['conversations.list'] = self._conversations_list
        self.server.unary['conversations.change'] = self._conversations_change
        self.server.unary['notifications.rules'] = self._notification_rules
        self.server.unary['notifications.label'] = self._notification_label
        self.server.unary['attention.ack'] = self._attention_ack
        self.server.streams["logs.subscribe"] = self._logs
        self.server.streams["chat.send"] = self._chat_send
        self.server.streams['ps.attach'] = self._ps_attach
        self.server.unary['ps.send'] = self._ps_send
        self._build_voice()
        await self._dump_codex_catalog()
        await self.server.start()          # refuses to start if another daemon answers on the socket
        self.journal.append("daemon.started", {"pid": os.getpid()})
        # L28: an unknown window is said, never guessed. The catalog shipped with the pinned binary was dumped before
        # serving (`_dump_codex_catalog`), so a fresh install knows each model's window before Codex has ever fetched
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
        self._macos_task = asyncio.create_task(self._macos_events())
        if self.supervisor and not self.state.stopped:
            resumed = asyncio.create_task(self.supervisor.resume_plans())
            self._turns.add(resumed)
            resumed.add_done_callback(self._turns.discard)

    async def shutdown(self) -> None:
        self._shutting_down = True
        await self._settle_forgets()                         # a forget's scrub runs in a thread: it finishes first
        self._reindex_stop.set()                             # the search re-index ends after its batch (next start resumes)
        if self._reindex_task is not None:
            await asyncio.wait({self._reindex_task})
        if self.persona is not None:
            await self.persona.close()
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
                                self._held_task, self._resources_task, self._autopair_task)
                     if job is not None}, "producers")
        if self.supervisor:
            self.supervisor.suspend_plans()
            self.supervisor.cancel_all()
        await drain(self._turns | self._runs, "task turns")
        await self._settle_forgets()                         # a scrub run again for a task that just stopped
        if self.supervisor:                                  # learning captures: cancelled, journaled as deferred
            await self.supervisor.stop_episodes()
        self._stop_card_readers()                            # e.g. one a verdict is waiting on
        if self.gateway is not None:                         # first looks still being taken (D26)
            await drain(set(self.gateway.looks), "first looks")
        if self._proxy:
            self._proxy.close()
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
        self.voice = Voice(self._voice_turn, self.state.approvals.pending, self.voice_speaker or SaySpeaker(),
                           self._journal(), self._launch_voice_app,
                           mode=lambda: str(self.config.get("voice", "mic", "auto")), render=render, question=question,
                           publish=self._publish, hide=lambda text: self._hide_code_reply(text, "voice"),
                           redact=lambda text: self.redact(text),
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
        """~/.ferb/FERB.app from a checkout install; the voice helper nested in FERB.app when the daemon runs from it."""
        from ferb.device import install
        return install.voice_app(self.home, install.app_bundle())

    def _hearing_requirement(self) -> str | None:
        """FERB.app's signature as `ferb setup` recorded it (hearing), or as `ferb setup --voice` did (same app)."""
        req = self.config.get("hearing", "requirement") or self.config.get("voice", "requirement")
        return str(req) if req else None

    async def _hearing_status(self, probe: bool) -> dict[str, Any]:
        """L26 for doctor and setup: FERB.app installed and signed as recorded, and (probe) its Speech Recognition state
        and on-device support, asked of FERB.app itself in the background. Never a prompt."""
        out: dict[str, Any] = {"installed": self.voice_bundle.exists()}
        if not out["installed"] or not probe:
            return out
        from ferb.sense.transcribe import signed
        req = self._hearing_requirement()
        out["signed"] = bool(req) and await asyncio.to_thread(signed, self.voice_bundle, str(req))
        if out["signed"]:
            try:
                out.update(await self.hearing.transcriber.status(self.hearing.folder))
            except Exception as e:
                out["error"] = self._redact_text(f"{type(e).__name__}: {e}")
        return out

    def _voice_app_exe(self) -> str | None:
        exe = self.voice_bundle / "Contents" / "MacOS" / "FERB"
        return str(exe) if self._ptt and exe.exists() else None

    def _voice_signed(self, exe: str) -> bool:
        """FERB.app's signature meets the requirement recorded by `ferb setup --voice` (bundle id + certificate)."""
        req = self.config.get("voice", "requirement")
        if not req:
            return False
        r = subprocess.run(["/usr/bin/codesign", "--verify", "--strict", f"-R={req}", str(self.voice_bundle)],
                           capture_output=True)
        return r.returncode == 0

    def _launch_voice_app(self) -> None:
        if self.voice_bundle.exists():
            subprocess.Popen(["/usr/bin/open", "-g", str(self.voice_bundle)], stdout=subprocess.DEVNULL,
                             stderr=subprocess.DEVNULL)

    def _hide_code_reply(self, text: str, session_id: str | None = None) -> str:
        """Rule 14 for a chat message (typed or spoken) while a question is open, before any model sees it, as
        chat.answer hides an answer (`Redactor.answer`, re-review 2 R1): its codes are masked when the question asks
        for a code, and a one-word reply to a password or PIN question is hidden whole. The planner routes answers
        now (L3), so the secret must not travel with the message; a longer message may be a steer and keeps its
        words. The questions: workers' open ones, and the planner's own clarifying question in this conversation
        (re-review 3, R1-b), also when FERB's own "which is it?" has since replaced it as the pending question
        (`Supervisor.displaced_question`). FERB's memory questions never ask for a secret, so neither they nor FERB's own
        "which is it?" about them hide anything, and a reply that is exactly an option of a waiting question is a
        choice (L36)."""
        if not isinstance(self.redact, Redactor):
            return text
        from ferb.orchestrator.steering import MEMORY_ASKS
        open_now = self.state.approvals.pending()
        choice = text.strip().rstrip(".!").strip()      # as an option pick reads it (supervisor `_bare`): "Keep." is Keep
        for ap in open_now:
            if ap.capability in MEMORY_ASKS:            # FERB's memory questions never ask for a secret (L36)
                continue
            # a login-code prompt asks for a code by its kind, whatever its words say
            reason = "login code" if ap.kind == "owner_login_code" else ap.reason
            if ap.kind in ("owner_question", "owner_login_code") and ap.option_for(choice) is None:
                text = self.redact.answer(text, question=reason, reply=True)
        if session_id is not None and self.supervisor is not None:
            asked = self.supervisor.continuity.pending(session_id)[1]
            # never a choice (exactly an option of a waiting question), and never after FERB's own "which is it?"
            # about memory questions only, whose words quote a guess (L36: "passwords" there asks for nothing); the
            # planner's question FERB's asking displaced still counts (rule 14)
            if not any(ap.option_for(choice) is not None for ap in open_now):
                if asked and not self.supervisor.asked_only_about(session_id, MEMORY_ASKS):
                    text = self.redact.answer(text, question=asked, reply=True)
                displaced = self.supervisor.displaced_question(session_id)
                if displaced:
                    text = self.redact.answer(text, question=displaced, reply=True)
        return text

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
        if params.get("on_device") is False:                     # no on-device recognition: never the network
            self._journal().append("voice.failed", {"why": str(params.get("error") or "on-device recognition unavailable")})
            self.voice._say("On-device speech recognition isn't available on this Mac, so I didn't listen. Your voice "
                            "never leaves the Mac.")
            return {"handled": "on_device_unavailable"}
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
        if self.protection is None:     # [protection] and the workspace, once at start: before migrations and browsers
            self.protection = protection_lists.load(self.config.get, Path.home(), self.home)
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
        # The protection lists (S2, SQ3, SQ5, D27), read in _load_config: FERB's own state is always added by code
        # (rule 10); the shell and FERB's tools that name files share this one object.
        assert self.protection is not None
        # FERB's downloads folder and its upload copies of outside content are always outside content (D35, MP2):
        # code adds them, config can't remove them.
        roots = FsRoots(str(workspace), tuple(self.config.get("fs", "owner_roots", [])),
                        downloads.untrusted_roots(self.config.get("fs", "untrusted_roots", ["~/Downloads"]), workspace),
                        protection=self.protection)
        assert self.ledger and self.spans and self.journal and self.db
        from ferb.orchestrator.skills import Skills, SkillsRead
        skills = Skills(self.home / 'skills')
        self.registry.add(SkillsRead(skills))
        self.gateway = Gateway(self.registry.capabilities, self.state, self.ledger, self.spans, self.tasks,
                               self.clock, self.redact, self._publish, roots, device)
        test_mode = self.config.get('test_mode', 'state', {})
        if isinstance(test_mode, dict) and test_mode.get('enabled') is True:
            allowed = test_mode.get('allow_send', [])
            self.gateway.test_send_allowlist = (set(allowed) if isinstance(allowed, list) and
                all(isinstance(item, str) and item.strip() for item in allowed) else set())
        self.gateway.disturbance = self.disturbance
        self.gateway.memory_read = lambda task_id, refs: self.db.conn.executemany(   # re-review a3
            "INSERT OR IGNORE INTO task_memory_refs VALUES (?,?)", [(task_id, ref) for ref in refs])
        self.gateway.journal_problem = lambda kind, data: self._journal().append(kind, data)   # re-review R4
        self.gateway.evidence = self.evidence
        self.gateway.recovery = self._recovery()
        self.gateway.snapshots = self.snapshots
        self.gateway.confirmations = Confirmations(self.db, self.clock, self.ids, self.redact)     # D33, S4 (F5a)
        self.gateway.destinations = KnownDestinations(self.db, self.clock, self.ids, self.redact)  # SQ6
        if self.shells is not None:                  # a running shell process's task: its destinations are its own
            self.gateway.proc_task = self.shells.live_owner
        self.gateway.app_window = self._app_window
        self.supervisor = Supervisor(make_runtime=self._make_runtime, gateway=self.gateway, state=self.state,
                                     tokens=self.tokens, tasks=self.tasks, spans=self.spans, journal=self.journal,
                                     publish=self._publish, clock=self.clock, ids=self.ids, device_id=device,
                                     workspace=str(workspace), on_violation=self._runtime_violation,
                                     on_task_end=self._task_end, facts=self._facts,
                                     checks=Checks(self._urls_of, self.spans), roots=roots)
        self.supervisor.browser_context = self._browser_context
        self.supervisor.task_tabs = self._task_tabs
        self.supervisor.skills_catalog = skills.catalog
        self.supervisor.on_resumed_outcome = lambda event: self._task_result(event, shown=False)
        self.supervisor.on_task_start = self.disturbance.start
        self.supervisor.on_task_finish = self._tree_finished
        self.supervisor.disturbance_context = self.disturbance.context
        self.supervisor.disturbance = self.disturbance
        self.supervisor.relevant_context = self._relevant_context      # its fourth element: the search's source refs
        add_context_readers(self.registry, self.supervisor.continuity,
                            lambda task_id: (rec.tree_id if (rec := self.tasks.get(task_id)) else None),
                            lambda: self.memory, lambda ref: self.learning._source_origins(ref) if self.learning else {origin_.external('unlabeled'), origin_.PRIVATE},
                            search=self._memory_item_search)
        from ferb.identity.secret_tools import SecretsList
        self.registry.add(SecretsList(lambda: self.home,               # L36 §5: names only, never values
                                      lambda ctx, place: self.gateway._login_sites(ctx, place) if self.gateway else []))
        self.supervisor.worker_types = lambda: ['default', *[n for n in self.registry.runtimes if self._runtime_available(n)]]
        self.gateway.route_question = self.supervisor.route_question
        from ferb.orchestrator.conversation_tools import ConversationRead
        from ferb.orchestrator.attention import AttentionNotices
        self.registry.add(ConversationRead(self.supervisor.conversations, self.tasks.tree_of))
        self.attention_notices = AttentionNotices(self.db, self.clock, self.redact)
        self.supervisor.on_loop_notice = self._loop_notice
        self.supervisor.notification_rule = self._notification_rule_from_chat
        self.supervisor.on_conversation_resume = self._resume_conversation_plans
        self.supervisor.typed_behind_memory = self._typed_behind_memory   # attended scopes too (final review I1)
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
        if hasattr(driver, "native_secrets"):          # B6: an unread tab's fields from Accessibility, as the router's
            driver.native_secrets = self._native_page_secrets
        if hasattr(driver, "native_capture"):          # I1: a tab the debugger can't attach to, as the router's
            driver.native_capture = self._native_window_capture
        if hasattr(driver, "native_place"):            # L33: FERB's cursor, as the router's
            driver.native_place = self._native_place
        from ferb.identity.helper import seal as helper_seal
        driver.seal_value = lambda key, value, command, origin: helper_seal(self.home, key, value, command, origin)
        add_identity_browser(self.registry, driver, self.recordings)

    async def _task_end(self, task_id: str) -> list[dict[str, Any]]:
        resources = await self.hearing.end_task(task_id)          # L26: listening ends with the task
        resources.extend(await self.recordings.end_task(task_id))
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

    async def _tree_finished(self, tree_id: str) -> None:
        """A task tree ended: the desktop observer's summary, then FERB's upload copies for it go (rule 17), except
        those a tab that outlives the tree still shows. What couldn't be removed is journaled; the next start-up
        sweeps it."""
        try:
            await self.disturbance.finish(tree_id)
        finally:
            if self.uploads is not None:
                await self._clean_uploads(self.uploads.forget_tree, tree_id, about={"task_id": tree_id})

    def _running_processes(self) -> list[dict[str, Any]]:
        """FERB-started processes running now (the `ferb ps` data), commands redacted (rule 14): a front-app or Space
        change's record names them (P26). Facts only; nothing is inferred about what they did."""
        if self.shells is None:
            return []
        return [{"id": p.id, "task": p.owner_task, "command": self._redact_text(p.command), "background": p.long_lived}
                for p in self.shells.list() if p.running]

    def _tab_closed(self, br: Any, tab_id: int) -> Awaitable[None] | None:
        """A tab closed in a paired browser: the upload copies it showed need not stay (rule 17). Those of a tree that
        already ended go now, in the background."""
        if self.uploads is None or not self.uploads.closed(uploads.tab_key(br.name, tab_id)):
            return None
        return self._clean_uploads(self.uploads.collect)

    async def _clean_uploads(self, step: Callable[..., int], *args: Any, about: dict[str, Any] | None = None) -> None:
        """One upload-copy cleanup in a worker thread; what it couldn't remove is journaled (paths, never contents)."""
        try:
            await asyncio.to_thread(step, *args)
        except OSError as e:
            self._journal().append("browser.uploads_cleanup_failed",
                                   {**(about or {}), "error": f"{type(e).__name__}: {e}",
                                    "left": list(getattr(e, "left", []))})

    def _stop_identity_browser(self) -> None:
        asyncio.get_event_loop().create_task(self.hearing.shutdown())
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

    async def _web_secrets(self, browser: str, bounds: tuple[float, float, float, float],
                           title: str = "") -> WebSecrets | None:
        """For a native capture of a browser window (L13): ask every connected profile of this browser, together, for
        the mask state of the active tab of its (not minimized) window at these bounds (with the captured window's
        `title`, when several of its windows are there, protocol 15), and that tab. Exactly one profile must own
        such a window, and its answer is the WebSecrets. Otherwise nobody could tell, and the WebSecrets has no page
        and says why (no paired profile connected, none or two of a profile's windows there, more than one profile,
        no answer), for the capture's note (fix wave 4: the live case's "neither read" couldn't be explained)."""
        async def ask(br: Any) -> Any:
            try:
                return await br.call("window_secrets", {"bounds": list(bounds), "title": title}, timeout=4)
            except FerbError:
                return None
        bridges = [br for br in (self.hub.bridges.values() if self.hub else []) if br.conn is not None
                   and br.binding.browser == browser]
        if not bridges:
            return WebSecrets(None, f"no paired profile of {browser} is connected")
        answers = await asyncio.gather(*(ask(br) for br in bridges))
        owners = [r for r in answers if isinstance(r, dict) and r.get("matched")]
        if len(owners) > 1 and title:          # each profile's own window there: the one showing the captured page
            owners = [r for r in owners if isinstance(r.get("tab"), dict) and isinstance(r["tab"].get("title"), str)
                      and same_title(r["tab"]["title"], title)] or owners      # (two maximized profiles, m4)
        if len(owners) == 1:
            return WebSecrets.from_reply(owners[0])
        if owners:
            return WebSecrets(None, "more than one paired profile has a window at this window's place")
        if any(isinstance(r, dict) and r.get("reason") == "ambiguous_window" for r in answers):
            return WebSecrets(None, "a paired profile has more than one window at this window's place")
        if any(not isinstance(r, dict) for r in answers):      # timed out or failed: the driver won't ask again
            return WebSecrets(None, "a paired profile didn't answer", late=True)
        return WebSecrets(None, "no paired profile has a window at this window's place (another profile, or a "
                                "private window)")

    async def _native_page_secrets(self, pid: int, bounds: tuple[float, float, float, float], title: str) -> Any:
        """B6: for a task tab's capture whose page the extension couldn't read, the secret fields Accessibility finds in
        the page area of that browser process's window at `bounds` showing the tab titled `title`
        (`MacComputerDriver.page_secrets`). None: no Mac driver, or it can't tell (a failure is journaled by type; the
        capture then goes to text recognition)."""
        if self.mac is None:
            return None
        try:
            return await self.mac.page_secrets(pid, bounds, title)
        except Exception as e:
            from ferb.daemon.gateway import error_name
            self._journal().append("screen.page_secrets_failed", {"error": error_name(e)})
            return None

    async def _native_place(self, rect: tuple[float, float, float, float], viewport: tuple[float, float, float, float],
                            pid: int, bounds: tuple[float, float, float, float],
                            title: str) -> tuple[float, float, float, float] | None:
        """L33: a task tab's element in screen points for FERB's cursor, placed on the page area of the on-screen
        browser window showing the tab (`MacComputerDriver.place_on_web_area`); None without a Mac driver."""
        if self.mac is None:
            return None
        return await self.mac.place_on_web_area(rect, viewport, pid, bounds, title)

    async def _native_window_capture(self, pid: int, bounds: tuple[float, float, float, float],
                                     title: str) -> tuple[bytes | None, str]:
        """Round 3 (I1): for a task tab the debugger can't attach to, which is the one its window shows: that on-screen
        browser window (at `bounds`, carrying the tab's title `title`) captured natively, as mac.screenshot
        captures (masked from AX and the extension where they can read it, `Shot.masking`): (JPEG, note). Otherwise
        (None, why), in FERB's words (round 4, item 3): no Mac driver, screenshots switched off ([mac] snapshot_image),
        no such window on screen or more than one, the app excluded, or the capture failed (journaled by type)."""
        mac = self.mac
        if mac is None:
            return None, "FERB has no Mac driver to capture windows with"
        if not mac.snapshot_images():
            return None, "screenshots are switched off ([mac] snapshot_image)"
        from ferb.act.mac import screenshot
        try:
            near = [w for w in await asyncio.to_thread(screenshot.normal_windows)
                    if w.pid == pid and all(abs(a - b) <= 2 for a, b in zip(w.bounds, bounds))]
            if title:                          # the window showing the tab carries its title, even when it is the only
                near = [w for w in near if same_title(w.title, title)]   # one there (round 4: rule 5, the right window)
            if not near:
                return None, "no window of the browser showing that tab is on screen (it may be on another Space)"
            if len(near) > 1:
                return None, "more than one window of the browser at that place shows that tab's title"
            _, shot, jpeg = await mac.window_image(f"pid:{pid}", window=str(near[0].id))
            now = await asyncio.to_thread(screenshot.window, near[0].id)
            if title and (now is None or not same_title(now.title, title)):     # checked again (round 5): it may
                return None, "the window's page changed while it was captured (its title isn't the tab's now)"
            return jpeg, shot.masking
        except NoTarget:                       # gone between the listing and the capture
            return None, "the window went away before it could be captured"
        except Withheld as e:                  # its masks couldn't be painted: said once, by the caller
            return None, str(e).replace("; pixels withheld", "").removeprefix("pixels withheld: ")
        except Refused as e:                   # FERB's own words (an excluded app)
            return None, str(e)
        except Exception as e:
            from ferb.daemon.gateway import error_name
            self._journal().append("screen.native_capture_failed", {"error": error_name(e)})
            return None, f"the window capture failed ({error_name(e)})"

    async def _front_window(self, request_id: str, client_pid: int | None = None) -> Any:
        """L12: the owner's front window as this message arrived (ferb/sense/front_window.py): the topmost window that
        isn't FERB's (nor, while in front, that of the app hosting the CLI that sent it, `client_pid`), its redacted
        screenshot kept as evidence under the request; an app on the exclusion list is reported unnamed and unlooked
        at, a site on it under the excluded label. None: no Mac driver or evidence store, sensing paused (`ferb
        pause`), or nothing but FERB in front. A failure is journaled by type and never blocks the message."""
        from ferb.act.mac import accessibility, screenshot
        from ferb.daemon.gateway import error_name
        from ferb.sanitize.images import ImageRedactor
        from ferb.sense import front_window as fw
        mac, evidence = self.mac, self.evidence
        if mac is None or evidence is None or (self.sense is not None and self.sense.paused):
            return None
        exclusions, daemon_pid = self._exclusions(), os.getpid()

        async def shoot(w: Any) -> tuple[bytes, str, tuple[str, str] | None]:
            if not mac.snapshot_images():                    # the owner's `[mac] snapshot_image` switch
                raise Refused("screenshots are switched off ([mac] snapshot_image)")
            _, shot, jpeg = await mac.window_image(f"pid:{w.pid}", window=str(w.id))
            return jpeg, shot.masking, shot.tab

        async def keep(image: bytes, facts: dict[str, Any], origin: str) -> str:
            return await evidence.put_async(request_id, None, "front_window", image=image, data=facts, origin=origin)
        try:
            sender = await asyncio.to_thread(fw.sending_app, client_pid, fw.app_of, fw.parent_pid) \
                if client_pid else None
            sources = fw.Sources(
                windows=screenshot.normal_windows, app_of=fw.app_of,
                own=lambda pid, bundle: fw.ferb_process(pid, bundle, daemon_pid, fw.parent_pid),
                excluded=lambda bundle, name, url: exclusions.app(bundle, name) or exclusions.url(url),
                tab=self._front_tab, shoot=shoot, keep=keep, redact=self.redact, sender=sender,
                main_frame=accessibility.main_window_frame,
                redact_image=ImageRedactor(self.redact if isinstance(self.redact, Redactor) else Redactor()))
            return await fw.capture(sources)
        except Exception as e:
            self._journal().append("screen.capture_failed", {"request_id": request_id, "error": error_name(e)})
            return None

    def _front_tab(self, bundle_id: str, window: Any, after_shot: bool = False) -> tuple[str, str] | None:
        """The active tab (URL, title) of the browser window the front-window capture took (L12, `window`): every
        window of the browser read through AppleScript and the captured one picked by its place and title
        (`browser_tabs.window_tab`; LookupError when no single window is it: no URL rather than another window's).
        Only when the owner already granted Automation for that browser (the check never asks; while it is still
        answering the read is None, and the capture tries again after the shot). A kept GRANTED answer serves only that
        read after the shot, and only while a fresh check is still pending; it is dropped as soon as a check answers
        anything else (a permission reset mid-run must not lead to a prompt), and when a read fails."""
        from ferb.sense.watchers.browser_tabs import WINDOW_SCRIPTS, applescript_windows, window_tab
        if bundle_id not in WINDOW_SCRIPTS:
            return None
        if not self._automation_granted(bundle_id):
            probe = self._automation_checks.get(bundle_id)
            if probe is None or probe.done():          # a check answered something other than GRANTED
                self._tab_granted.discard(bundle_id)
                return None
            if not (after_shot and bundle_id in self._tab_granted):
                return None
        self._tab_granted.add(bundle_id)
        try:
            listed = applescript_windows(bundle_id)
        except (OSError, subprocess.SubprocessError):
            listed = None
        if listed is None:
            self._tab_granted.discard(bundle_id)
            return None
        found = window_tab(listed, window.bounds, window.title)
        return (found.url, found.title) if found.url else None

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
                                     excluded=self.config.exclude_apps, web_secrets=self._web_secrets,
                                     profiles=Profiles(profiles_dir(self.home)))   # what landed per app, learned
        self.mac.snapshot_images = lambda: bool(self.config.get("mac", "snapshot_image", True))   # image + tree
        # FERB's shell and AppleScript run only inside its sandbox, built from the protection lists (S2, D27, rule
        # 10): when sandbox-exec can't run, their tools are left out and `ferb doctor` says why, never uncontained (Q7)
        assert self.protection is not None
        # Its egress proxy is the shell's only way off the Mac (S2, D28): each process gets its own credentials, and
        # every destination is checked by the gateway (L2: any remote host for a tracked process) and recorded with its
        # byte counts (F5a Task 3). Every TLS hello's server name goes through the same check (S4 fronting: a tree's
        # No to a host can't be passed through a differing SNI).
        self._loop = asyncio.get_running_loop()
        self.proxy = EgressProxy(self._allow_destination, redactor=self.redact if isinstance(self.redact, Redactor)
                                 else None, excluded=getattr(self.redact, "excluded_host", None),
                                 inspect_sni=lambda proc, host, port: True,
                                 local_access=LocalAccess(self.protection) if self.protection.local_ports else None,
                                 allow_local=self._allow_local_destination)
        self.sandbox = Sandbox(self.protection, self.home / "run" / "sandbox",
                               lambda: self.proxy.port if self.proxy is not None else None, socket=self.server.path)
        self.sandbox.sweep()
        self.containment = self.sandbox.available()
        contained = self.sandbox if self.containment[0] else None
        if contained is None:                                   # no shell, so no proxy for it
            self.proxy.close()
            self.proxy = None
        self.mac.ask_secret = lambda ctx, target, **kw: self.gateway.ask_secret(ctx, target, **kw)
        self.mac.release_secret = lambda value: self.gateway.release_secret(value)
        add_mac(self.registry, self.mac, self._device_profile, self.recordings, contained, self.protection)
        self.hearing.apps = self.mac.app                          # the Mac tools' exact app lookup (exclusions too)
        self.hearing.sweep()                                      # sound and answers a restart left behind (L26)
        add_hearing(self.registry, self.hearing,
                    lambda identity: self.identity_browser if identity is Identity.FERB else self.router)
        if contained is not None and self.protection.local_ports:   # pinned local ports: read the kernel's reports
            self.direct_reports = DirectReports(on_end=self._local_reports_stopped)
            self.direct_reports.start()
        self.shells = ShellManager(self.home / "logs" / "shell", self._redact_text,
                                   stream_factory=getattr(self.redact, "stream", None), sandbox=self.sandbox,
                                   proxy=self.proxy, on_network=self._shell_network,
                                   redactor=self.redact if isinstance(self.redact, Redactor) else None,
                                   on_escape=lambda task, grant: self._journal().append("escape.used", {
                                       "task_id": task, "rule": grant.rule, "command": grant.command,
                                       "cwd": grant.cwd, "confirmation": grant.confirmation_id}),
                                   reports=self.direct_reports, local_refused=self._local_refused)
        if contained is not None:
            add_shell(self.registry, self.shells, self.protection)
        else:
            self._journal().append("shell.unavailable", {"why": self.containment[1]})
        add_trail(self.registry, lambda: self.trail)
        add_evidence(self.registry, self.spans, self._inspection_parent,
                     lambda task_id: (rec.tree_id if (rec := self.tasks.get(task_id)) else None), self.evidence,
                     lambda: getattr(self.redact, "secret_values", set)())   # a page never splits one (rule 14)
        add_time(self.registry, lambda: self.timers, self.clock.now, lambda: self._sleep_until,
                 lambda task_id: (rec.task.deadline if (rec := self.tasks.get(task_id)) else None))

    def _allow_destination(self, proc: str, host: str, port: int) -> tuple[bool, str]:
        """On a proxy thread: the gateway's answer, from the daemon's loop. Anything that stops it answering refuses."""
        try:
            assert self.gateway is not None and self._loop is not None
            fut = asyncio.run_coroutine_threadsafe(self.gateway.destination_allowed(proc, host, port), self._loop)
            return fut.result(10)
        except Exception:
            return False, "FERB could not check this destination"

    def _allow_local_destination(self, proc: str, host: str, port: int) -> tuple[bool, str]:
        """Only the proxy calls this, after verifying an explicit local connected peer against its configured pin.
        The pin is the standing local permission; task ownership and an explicit No still apply."""
        try:
            assert self.gateway is not None and self._loop is not None
            fut = asyncio.run_coroutine_threadsafe(
                self.gateway.destination_allowed(proc, host, port, local_approved=True), self._loop)
            return fut.result(10)
        except Exception:
            return False, "FERB could not check this local destination"

    def _local_refused(self, task_id: str) -> bool:
        """Whether this task's tree has the owner's No to one of this Mac's addresses (the shared host key, SQ6): its
        shell commands then reach pinned local ports only through the proxy, which keeps that No final for each
        connection (L24). A task FERB can't place counts as refused."""
        root = self.tasks.tree_of(task_id)
        if root is None:
            return True
        confirmations = self.gateway.confirmations if self.gateway is not None else None
        for key in confirmations.refused(root) if confirmations is not None else []:
            try:
                rule, host = json.loads(key)
            except (ValueError, TypeError):
                continue
            if rule == confirm.SINK_DESTINATION and isinstance(host, str) and (
                    local_host(host) or host.rstrip(".").endswith(".localhost")):
                return True
        return False

    def _local_reports_stopped(self, why: str) -> None:
        """The kernel's reports of direct local connections stopped by themselves (the stream ended or couldn't
        start; told once, from the stream's thread): from now on those records say unknown. `ferb doctor` shows it."""
        self._journal_from_any_thread("shell.local_reports_stopped", {"why": why})

    def _local_reports_status(self) -> dict[str, Any] | None:
        """For `ferb doctor`: whether the kernel's reports of direct local connections are being read (None: no
        pinned local ports, nothing to read)."""
        reports = self.direct_reports
        return None if reports is None else {"running": reports.running, "stopped": reports.ended}

    def _shell_network(self, info: ProcInfo, facts: dict[str, Any]) -> None:
        """A finished shell process's final network record (ferb trace / journal). The proxy may report it from a
        tunnel's own thread when a setsid child closes it, so the write is scheduled on the daemon loop."""
        self._journal_from_any_thread("shell.network", {"task_id": info.owner_task, "process": info.id, **facts})

    def _journal_from_any_thread(self, kind: str, record: dict[str, Any]) -> None:
        """A journal row from the daemon loop or another thread (written on the loop)."""
        def write() -> None:
            self._journal().append(kind, record)
        try:
            on_loop = asyncio.get_running_loop() is self._loop
        except RuntimeError:                                  # a proxy thread
            on_loop = False
        if on_loop or self._loop is None or not self._loop.is_running():
            write()                                           # at shutdown too: before the database closes
        else:
            self._loop.call_soon_threadsafe(write)

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
            self.mac.acting = a.acting
        if self.gateway is not None:
            self.gateway.on_path = a.ferb_touched
        if self.hub is not None:
            self.hub.on_tab = self._on_tab
            self.hub.paused = lambda: self.sense is not None and self.sense.paused
            self.hub.owned = lambda br, tab_id: self._tab_task(br, tab_id) is not None
        if self.supervisor is not None:
            self.supervisor.signals = self.signals
        from ferb.sense.perception import Perception
        self.perception = Perception(self)
        self.perception.bind_clipboard()
        self.trail.tag = self.perception.notify
        self.perception.purge_text()
        watchers: list[Any] = [self.perception]
        if self.run_watchers:
            watchers += [AppFocusWatcher(on_change=self._on_app_focus), WindowTitleWatcher(lambda: bool(self.config.get("sense", "window_titles",
                                                                                           True))),
                        IdleWatcher(lambda: self.owner_input.last_owner_wall, lambda: self.owner_input.running,
                                    float(self.config.get("sense", "idle_after_s", 120))),
                        FilesWatcher(self._watch_folders), AppInstallsWatcher(self._app_changed),
                        ScriptedTabsWatcher(self._automation_granted, self._any_extension_connected)]
            watchers += self.perception.native_watchers()
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

    def _automation_granted(self, bundle_id: str) -> bool:
        """Already granted by the owner; never asks (the prompt would appear out of nowhere)."""
        import time as _t
        from ferb.device import permissions
        hit = self._automation_cache.get(bundle_id)
        if hit and _t.monotonic() - hit[0] < 60:
            return hit[1]
        probe = self._automation_checks.get(bundle_id)
        if probe is None:
            probe = self._automation_checks[bundle_id] = permissions.automation_check(bundle_id, ask=False)
        if not probe.done():
            return False  # watcher skips this sample; a stuck native call must not hold its executor thread
        self._automation_checks.pop(bundle_id, None)
        ok = probe.result() is permissions.Status.GRANTED
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
                "owner_input": self.owner_input.running, "folder_problems": self.watch_problems,
                "perception": self.perception.status() if self.perception is not None else {}}

    # ---------- memory ----------

    def _build_memory(self) -> None:
        from ferb.memory import summary
        from ferb.memory.distill import Distiller
        from ferb.memory.items import Memory
        from ferb.memory.recall import context_block
        assert self.db is not None and self.journal is not None
        self.memory = Memory(self.db, self.clock, self.ids, self._publish, on_change=self._export_memory,
                             redact=self._redact_text)
        from ferb.memory.learning import Learning
        from ferb.memory.consolidate import Consolidator
        from ferb.memory.tools import LearningRead
        self.learning = Learning(self.db, self.clock, self._publish, self._redact_text, redact_id=self._redact_id)
        self.learning.note = lambda kind, data: self._journal().append(kind, data)
        self.learning.secret_values = lambda: getattr(self.redact, "secret_values", set)()
        self.consolidator = Consolidator(self.learning, self.memory, self._make_runtime,
            idle=lambda: self.supervisor is None or not self.supervisor.running_trees(),
            usage=self._record_learning_usage("consolidate"),
            capture=lambda task_id: self._capture_card(task_id, None),
            cards_refreshed=self._replay_deferred_receipts, spans=self.spans,
            forget=lambda ids: self._forget_nodes(ids, named=False))
        self.registry.add(LearningRead(lambda: self.learning, self._search))
        self.memory.on_forget = self._forget_learning_sources
        self.memory.after_forget = self._forget_stating_cards
        self.memory.on_settled = lambda ids, deleted: self._close_memory_questions(ids, None if deleted else
                                                                                  ("confirm", "edit"))
        journal = self.journal
        self.memory.rebuild_summary = lambda day: summary.rebuild(self.db, journal, day)
        self.distiller = Distiller(self.db, self.journal, self.memory, self._make_runtime, self.clock,
                                   purge=self._purge_trail, device_id=str(self.config.get("device", "id", "dev")),
                                   record_usage=self._record_learning_usage("distill"), spans=self.spans)
        self.distiller.ask_keep = self._ask_keep
        if self.supervisor is not None:
            self.supervisor.apps = self._installed_apps
            self.supervisor.running_apps = self._running_apps
            self.supervisor.pasted_is_data = lambda: bool(self.config.get("policy", "pasted_text_is_data", True))
            self.supervisor.environment = self._environment
            self.supervisor.availability = self._availability
            # L36 §4: every worker and plan step gets the one search at the owner's worker budget, its task's targets
            # also ranked alone (no procedures: a worker's procedure is chosen by the planner)
            self.supervisor.memory_push = lambda words, focus: self._learning_candidates(
                words, WORKER_MEMORY_ITEMS, procedures=False, focus=focus)
            self.supervisor.remember = self._remember
            self.supervisor.forget_request = self._chat_forget
            self.supervisor.question_answered = self._question_answered
            self.supervisor.settle = self._settle_forgets
            self.supervisor.procedure_context = self._learning_context
            self.supervisor.memory_candidates = lambda words, focus: self._learning_candidates(words, focus=focus)
            self.supervisor.procedure_details = lambda ident, task, slots: self.learning.procedure_parts(
                ident, self._app_versions(), slots)
            self.supervisor.procedure_problem = self._procedure_problem
            self.supervisor.identity_context = self._learning_identity
            self.supervisor.on_episode = self._capture_learning_episode
            self.supervisor.procedure_outcome = self._procedure_outcome
            self.supervisor.slot_values = self.learning.slot_values
            self.supervisor.slot_problem = self.learning.slot_problem
        self._replay_deferred_receipts()
        self._restore_memory_questions()
        if self.run_distill:
            self._distill_task = asyncio.get_running_loop().create_task(self._distill_loop())
        if self.run_retention:
            self._retention_task = asyncio.get_running_loop().create_task(self._retention_loop())
        if self.resource_log is not None:
            self._resources_task = asyncio.get_running_loop().create_task(self._resources_loop())

    def _app_versions(self) -> dict[str, str]:
        profile = self._device_profile()
        return {a.bundle_id: a.version for a in profile.apps} if profile else {}

    async def _capture_learning_episode(self, task_id: str) -> None:
        """A finished task's episode card, after its outcome (`Supervisor.on_episode`). Failures are journaled."""
        try:
            if self.learning is not None:
                await self._capture_card(task_id, self._app_versions(), report=True)
        except Deferred:
            raise                                     # the supervisor journals it with its procedure, for the replay
        except Exception as e:
            self._journal().append("memory.episode_failed", {"task_id": task_id, "error": self._redact_text(str(e))})

    async def _capture_card(self, task_id: str, apps: dict[str, str] | None, *, report: bool = False) -> Any:
        """Capture (or refresh, `apps` None) a task's episode card (L28). The after-outcome capture, an owner verdict
        and the consolidator's re-capture all come here, one at a time. The reads run in the card reader
        (`_read_card`); only the write is on the loop. When a card input changed while the reader read (a forget, a
        verdict, a write of a card: `Learning.generation`), it reads again. `report`: journal the refs skipped as
        damaged by redaction or expired by retention, and a card skipped as built on forgotten content (C1). Raises
        LearningRefused when the card can't be built."""
        assert self.learning is not None
        async with self._card_lock:
            while True:
                generation = self.learning.generation
                prepared = await self._read_card(task_id, apps)
                if generation == self.learning.generation:
                    break
            if report and prepared:
                for kind, key in (("memory.episode_refs_damaged", "damaged"), ("memory.episode_refs_expired", "expired")):
                    if prepared.get(key):
                        self._journal().append(kind, {"task_id": task_id, "refs": prepared[key]})
                if prepared.get("forgotten"):        # ids only: what they held is gone
                    self._journal().append("memory.episode_skipped", {"task_id": task_id,
                                                                     "why": "built on forgotten content"})
            return self.learning.write_episode(prepared)

    async def _read_card(self, task_id: str, apps: dict[str, str] | None) -> dict[str, Any] | None:
        """Every read of a card, in `ferb.memory.card_reader`: a process of its own on a read-only connection, because
        the lineage resolution is CPU-bound (1.3-1.6 s on the owner's data) and in a thread it would still hold the
        interpreter lock against the event loop. A reader whose wait is cancelled or fails is killed and reaped. Once
        the daemon is stopping (its readers already killed), no reader starts: Deferred. The home also goes on stdin,
        only as a check the reader makes against the home it finds itself."""
        from ferb.memory.learning import LearningRefused
        if self._shutting_down:
            raise Deferred("FERB is stopping; this card is captured again after the next start")
        proc = await asyncio.create_subprocess_exec(
            *self.card_reader, stdin=asyncio.subprocess.PIPE, stdout=asyncio.subprocess.PIPE,
            stderr=asyncio.subprocess.DEVNULL, env={**os.environ, "FERB_HOME": str(self.home)})
        self._card_readers.add(proc)
        try:
            out, _ = await proc.communicate(json.dumps({"task_id": task_id, "apps": apps,
                                                        "home": str(self.home)}).encode())
        finally:
            self._card_readers.discard(proc)
            if proc.returncode is None:
                proc.kill()
                await proc.wait()
        try:
            answer = json.loads(out)
        except ValueError:
            if self._shutting_down:                  # killed by shutdown
                raise Deferred("FERB is stopping; this card is captured again after the next start") from None
            answer = {"error": f"the card reader exited {proc.returncode} without an answer"}
        if "error" in answer:
            raise LearningRefused(str(answer["error"]))
        prepared: dict[str, Any] | None = answer["prepared"]
        return prepared

    def _replay_deferred_receipts(self) -> None:
        """Procedure receipts whose capture shutdown deferred (`memory.episode_deferred` with a procedure_id, and the
        planner's typed slot values, L29): each is recorded once its task's card exists (at start, and after the
        consolidator captures cards), then journaled as `memory.receipt_replayed` so it is replayed once. Receipts drive
        a procedure's state (learned, suspect)."""
        if self.learning is None:
            return
        journal = self._journal()
        done = {(e.data.get("task_id"), e.data.get("procedure_id")) for e in journal.of_kind("memory.receipt_replayed")}
        cards: set[str] | None = None
        for row in journal.of_kind("memory.episode_deferred"):
            task_id, procedure_id = row.data.get("task_id"), row.data.get("procedure_id")
            if not procedure_id or (task_id, procedure_id) in done:
                continue
            if cards is None:
                cards = {str(n["key"]) for n in self.learning.items("episode")}
            if task_id not in cards:
                continue                              # its card comes with the consolidator's next pass
            slots = row.data.get("slots")                # absent in an entry from before L29
            self._procedure_outcome(str(procedure_id), str(task_id), slots if isinstance(slots, dict) else None)
            journal.append("memory.receipt_replayed", {"task_id": task_id, "procedure_id": procedure_id})
            done.add((task_id, procedure_id))

    def _stop_card_readers(self) -> None:
        """Shutdown: kill every reader still in flight rather than wait for it; the hourly consolidator captures those
        cards again."""
        for proc in list(self._card_readers):
            if proc.returncode is None:
                proc.kill()

    def _procedure_problem(self, ident: str) -> str | None:
        """Why the planner's procedure_id can't be used, or None (I5): it must name a current procedure the owner
        hasn't turned down, rolled back or left quarantined."""
        node = self.learning.get(ident) if self.learning is not None else None
        if node is None:
            return "no stored item has this id"
        if node["kind"] != "procedure":
            return f"a {node['kind']}, not a procedure"
        if node["valid_to"] or node["state"] in ("retired", "rolled_back", "quarantined"):
            return f"not a current procedure ({'superseded' if node['valid_to'] else node['state']})"
        return None

    def _procedure_outcome(self, procedure_id: str, task_id: str, slots: dict[str, Any] | None = None) -> None:
        """A procedure run's receipt, with the planner's typed values for its slots (L29) when it gave them."""
        if self.learning is None:
            return
        node = next((n for n in self.learning.items("episode") if n["key"] == task_id), None)
        if node:
            from ferb.memory.learning import LearningRefused
            try:
                self.learning.receipt(procedure_id, node["id"], slots)
            except LearningRefused:
                self._journal().append("memory.receipt_unavailable", {"procedure_id": procedure_id, "task_id": task_id})

    def _refresh_learning_verdict(self, task_id: str) -> None:
        if not self.learning:
            return
        for node in self.learning.items("episode", history=True):
            if node["key"] != task_id:
                continue
            row = self.db.conn.execute("SELECT verdict FROM owner_verdicts WHERE task_id=? ORDER BY seq DESC LIMIT 1",
                                       (task_id,)).fetchone()
            if row and row[0] == "fail":
                self.db.conn.execute("UPDATE learning_receipts SET successful=0 WHERE episode_id=?", (node["id"],))
        for procedure in self.learning.items("procedure"):
            self.learning.procedure(procedure["id"], self._app_versions())

    def _learning_identity(self) -> str:
        if not self.learning:
            return ""
        self.learning.sync_facts(self.memory)
        return self.learning.identity()

    async def _reindex_search(self) -> None:
        """L36 §3: the search index by content (`search_text`), once per SEARCH_TEXT_VERSION, asked at every start (a
        no-op once done). It runs in a worker thread on a connection of its own, in short batches each its own
        transaction (`reindex_search`): the event loop never waits for it, a forget waits one batch at most, and a stop
        or a crash keeps what was done (the next start finishes). Searches wait for it (`_search_ready`), never ranking
        a half re-indexed store. Journaled only when nodes were re-indexed (m3); a failure is journaled and leaves
        search as it stands until the next start."""
        assert self.db is not None
        path, stop = self.db.path, self._reindex_stop

        def work() -> tuple[bool, bool]:
            db = Database(path)
            try:
                db.conn.execute("PRAGMA busy_timeout = 60000")   # a long write elsewhere delays it, never fails it
                return reindex_search(db, stop=stop.is_set), search_indexed(db)
            finally:
                db.close()
        try:
            reindexed, finished = await asyncio.to_thread(work)
        except Exception as e:
            self._journal().append("memory.search_reindex_failed",
                                   {"error": self._redact_text(f"{type(e).__name__}: {e}")})
            return
        if reindexed:
            self._journal().append("memory.search_reindexed", {"version": SEARCH_TEXT_VERSION, "finished": finished})

    async def _search_ready(self) -> None:
        """Wait for the start's search re-index, if it still runs (`_reindex_search`)."""
        if self._reindex_task is not None and not self._reindex_task.done():
            await asyncio.wait({self._reindex_task})

    async def _query_vector(self, words: str) -> tuple[list[int] | None, bool]:
        """The query's embedding when the owner adopted a model, computed off the event loop (L28); (None, False)
        after a failed encode, reported once (keyword ranking only)."""
        embedder = self.learning.embedder if self.learning else None
        if embedder is None or not words:
            return None, True
        try:
            return await asyncio.to_thread(embedder.encode, words, query=True), True
        except Exception as e:                       # reported once; keyword ranking only, no second encode
            self._journal().append("memory.embedding_unavailable", {"error": self._redact_text(str(e))})
            return None, False

    async def _search(self, query: str, *, limit: int | None = None, leave_out: frozenset[str] = frozenset(),
                      sources: bool = True, focus: str = '', **filters: Any) -> list[dict[str, Any]]:
        """The one ranked search (L36 §3) for the planner, attention, workers, memory.read and memory.learned: memory
        items synced to fact nodes first, the query's embedding off the loop, SQLite on it. `sources` False: without
        each node's source refs (a prompt's lines). `focus`: the target words of the tasks the caller works for, also
        ranked alone (`Learning.search`; the planner's and workers' searches, L36 §4 fix round)."""
        if not self.learning:
            return []
        await self._search_ready()
        self.learning.sync_facts(self.memory)
        vector, embed = await self._query_vector(query)
        return list(self.learning.search(query, query_vector=vector, embed=embed, limit=limit, leave_out=leave_out,
                                         sources=sources, focus=focus, **filters))

    async def _memory_item_search(self, query: str) -> list[str]:
        """memory.read's query, ranked by the one search (L36 §3): the memory items behind the matching facts."""
        return [str(n["body"]["id"]) for n in await self._search(query, kind="fact") if n["body"].get("id")]

    async def _learning_candidates(self, words: str, budget: int = PLANNER_MEMORY_ITEMS,
                                   procedures: bool = True, *, focus: str = '') -> list[dict[str, Any]]:
        """Relevant learned memory, ranked, at most `budget` items (the owner's: planner 60, worker 20; L36 §3), each
        line saying what it is (`shown_text`); with `procedures`, every procedure the planner may choose first, outside
        the budget (L29). `focus`: the target words of the tasks the turn or worker serves, also ranked alone (L36 §4
        fix round). Each item cites itself: its node carries its own sources, so forgetting any of them reaches
        what cites the node (the memory.learned tool cites items the same way)."""
        if not self.learning:
            return []
        found = await self._search(words, limit=budget, leave_out=PROMPT_LEAVES_OUT, sources=False, focus=focus)
        from ferb.memory.learning import line_safe, shown_state, shown_text
        from ferb.orchestrator.context_layers import PROCEDURES_ROUTE
        # L29: every procedure the planner may choose, whatever the words, by its intent's name, description and slots
        # (the planner matches by meaning), the most recently used intent first; its origins with those of every
        # current label of its intent (the newest names it, its text copied from the run that wrote it), which it cites
        # too. Their fold route makes them optional blocks fitted to the window.
        learned = []
        for item in self.learning.learned_procedures() if procedures else []:
            node = item['node']
            made = set(node['origins']) | set(item['label_origins'])
            learned.append({'text': f"{node['id']} [procedure, {shown_state(node)}, {origin_.least_trusted(made)}; "
                                    f"data, not authorization] {item['name']}: {item['description']} — slots: "
                                    + line_safe(json.dumps(item['slots'], ensure_ascii=False, sort_keys=True)),
                            'reference': 'memory.learned item_id=' + node['id'], 'origin': origin_.join(made),
                            'source_refs': [node['id'], *item['labels']], 'fold': PROCEDURES_ROUTE})
        return learned + [
            {'text': f"{p['id']} [{p['kind']}, {shown_state(p)}, {p['origin']}; data, not authorization] "
                     + shown_text(p),
             'reference': 'memory.learned item_id=' + p['id'],
             'origin': origin_.join(p.get('origins', origin_.split(p['origin']))),
             'source_refs': [p['id']]}
            for p in found]

    def _learning_context(self, words: str) -> str:
        if not self.learning:
            return ""
        self.learning.sync_facts(self.memory)
        return self.learning.candidates(words)

    def _forget_learning_sources(self, plan: Any) -> None:
        self.daily_activity.forget_sources(set(plan.trail))
        refs = (["memory:" + s for s in plan.items] + ["journal:" + s for s in plan.journal] +
                ["trail:" + s for s in plan.trail])
        if self.learning:
            self._forget_nodes(refs, named=False)        # what is built on it; a node a question was about went too
        if self.supervisor:
            self.supervisor.conversations.forget_sources(set(refs))
            # a running summary that folded a forgotten entry; one that folded FERB's rows stating it goes in
            # echoes.finish
            self.supervisor.forget_summaries(set(plan.journal))
        items = list(plan.items)                         # L36 §5: a username goes with the memory it came from,
        self._after_forgets(lambda: self._clear_login_names(items))   # in a worker thread, awaited like the scrub

    async def _clear_login_names(self, item_ids: list[str]) -> None:
        """Clear every login record's username that came from one of these memory items (forgotten or retracted), in a
        worker thread: the handle file's lock may be held by a hidden input's Keychain save (coordinator ruling)."""
        from ferb.identity.owner_secrets import OwnerSecrets
        try:
            cleared = await asyncio.to_thread(OwnerSecrets(self.home).clear_usernames,
                                              {"memory:" + i for i in item_ids})
        except Refused as e:                             # an unreadable handle file: left as it is, said in the journal
            self._journal().append("memory.login_unreadable", {"error": str(e)})
            return
        if cleared:
            self._journal().append("memory.login_cleared", {"places": cleared})

    def _forget_memory(self, item_id: str, keep_sources: bool = False) -> Any:
        """A forget's deletions, now, on the event loop; its echo scrub then runs in a worker thread on a connection of
        its own (`_finish_forget`, review minor), and a chat turn waits for it first (`_settle_forgets`)."""
        plan = self.memory.forget(item_id, keep_sources, finish=False)
        self._forget_scrub(lambda running: self._finish_forget(plan, running))
        return plan

    def _forget_scrub(self, work: Callable[[Any], Awaitable[None]]) -> None:
        """A forget's echo scrub, now (behind those still running), and again for the work running at the forget as it
        ends (`_Rescrub`): a tree as it finishes (`task.finished`, before its job writes the "while you were away"
        result, so the next chat turn already waits) and each job (chat turns, resumed plans, scheduled runs, attention
        turns) after its last record. A run again touches only what that work wrote (`echoes.Running`), keyed by the
        requests running now (the chat turns', the running trees' and planner turns') and the spans unfinished now:
        never anything started after the forget, a tree resumed in place under a new request included."""
        from ferb.memory.echoes import Running
        self._after_forgets(lambda: work(None))
        jobs = {job for job in self._turns | self._runs if not job.done()}
        if not jobs:
            return
        assert self.db is not None
        unfinished = self.db.conn.execute(
            "SELECT id, kind, CASE WHEN kind = 'model.turn' THEN json_extract(input_json, '$.request_id') END "
            "FROM spans WHERE ended_at IS NULL").fetchall()
        trees = {key for key, _ in self.supervisor.running_trees()} if self.supervisor else set()
        marks = ','.join('?' * len(trees))
        runs = {str(r[0]) for r in self.db.conn.execute(f"SELECT request_id FROM tasks WHERE id IN ({marks})",
                                                       tuple(trees))} if trees else set()
        running = Running({self._turn_requests[job] for job in jobs if job in self._turn_requests} | runs
                          | {str(r[2]) for r in unfinished if r[2] is not None},       # a planner turn under way
                          trees, {str(r[0]) for r in unfinished})
        again = _Rescrub(lambda: work(running), jobs, running)
        self._rescrubs.add(again)
        for job in jobs:
            job.add_done_callback(partial(self._ran_at_a_forget, again))

    def _ran_at_a_forget(self, again: _Rescrub, job: asyncio.Task[Any]) -> None:
        """A job that was running at a forget ended: its scrub runs again; once none is left, the forget's rules are let
        go."""
        again.waiting.discard(job)
        if not again.waiting:
            self._rescrubs.discard(again)
        self._scrub_again(again)

    def _tree_finished_at_a_forget(self, tree_id: str) -> None:
        """A tree of the work running at a forget finished (its answer and outcome are journaled): its scrub runs again
        now, so a chat turn that starts before the tree's job ends (still writing its result) waits for it. Only a
        tree run for a request recorded at the forget counts: not a worker's own finish, not a tree resumed in place
        under a new request since."""
        assert self.db is not None
        row = self.db.conn.execute("SELECT request_id, parent_id FROM tasks WHERE id = ?", (tree_id,)).fetchone()
        if row is None or row["parent_id"] is not None:
            return
        for again in list(self._rescrubs):
            if row["request_id"] in again.running.requests:
                self._scrub_again(again)

    def _scrub_again(self, again: _Rescrub) -> None:
        """Queue a forget's scrub again; one run waiting to start covers what is recorded before it starts."""
        if again.queued:
            return
        again.queued = True

        async def run() -> None:
            again.queued = False
            await again.work()
        self._after_forgets(run)

    def _after_forgets(self, work: Callable[[], Awaitable[None]]) -> None:
        """A forget's echo scrub, queued behind those still running: one at a time, so two never rewrite the same row
        from the same old copy. A chat turn and the forget RPCs wait for it (`_settle_forgets`)."""
        prior = list(self._forgetting)

        async def run() -> None:
            await asyncio.gather(*prior, return_exceptions=True)
            await work()
        job = asyncio.get_running_loop().create_task(run())
        self._forgetting.add(job)
        job.add_done_callback(self._forgetting.discard)

    def _forget_nodes(self, refs: list[str], named: bool = True) -> list[str]:
        """Learned items go (`Learning.forget`: every version, what is built on them, tombstones), and FERB's questions
        about them close. A learned item the owner named has an echo pass (re-review R3, round 5, closing round): the
        one they asked to forget (`named`) and any a waiting "Forget for good?" names. FERB's naming of it in that
        question (`_named_node`) goes from FERB's replies and every span; what each of its versions states (its body's
        text, a procedure's intent, a lesson's trigger; never an episode card's goal, which other tasks share, and
        never its key alone, which can be a label such as "owner") goes where that version was shown or cited; its
        statement (a lesson's or identity's text, two words or more) also from FERB's chat replies and notices word for
        word (`echoes.finish_node`, in a worker thread)."""
        from ferb.memory import echoes
        from ferb.memory.learning import ID_KEYED
        asked = {target for kind, target in self._owner_followups.values() if kind == "forget:learning"}
        # each named item: its naming; each version's texts; an identity version's time; a version's statement
        found: dict[str, tuple[str, dict[str, list[str]], dict[str, str], dict[str, str]]] = {}
        for ident in (set(refs) if named else set()) | asked:
            node = self.learning.get(ident)
            if node is None:
                continue
            versions: dict[str, list[str]] = {}
            since: dict[str, str] = {}
            statements: dict[str, str] = {}
            for row in self.db.conn.execute("SELECT id, body_json, recorded_at FROM learning_nodes WHERE kind = ? AND "
                                            "key = ?", (node["kind"], node["key"])).fetchall():
                body = json.loads(row["body_json"])
                said = [] if node["kind"] in ID_KEYED else [body.get(k) for k in ("text", "intent", "trigger")]
                versions[row["id"]] = [t for t in said if isinstance(t, str) and t.strip()]
                text = body.get("text")
                if node["kind"] not in ID_KEYED and isinstance(text, str) and len(text.split()) >= 2:
                    statements[row["id"]] = text
                if node["kind"] == "identity":
                    since[row["id"]] = str(row["recorded_at"])
            found[ident] = (_named_node(node), versions, since, statements)
        deleted = self.learning.forget(refs)
        self._close_memory_questions(deleted)
        gone = [found[ident] for ident in deleted if ident in found]
        echo = echoes.NodeEcho(sorted({g[0] for g in gone}),
                               {v: said for g in gone for v, said in g[1].items() if v in deleted},
                               {v: at for g in gone for v, at in g[2].items() if v in deleted},
                               {v: text for g in gone for v, text in g[3].items() if v in deleted},
                               until=self.clock.now().isoformat())           # an identity's scope ends at the forget
        if echo.naming:
            self._forget_scrub(lambda running: self._finish_node_forget(echo, running))
        return deleted

    async def _finish_node_forget(self, echo: Any, running: Any = None) -> None:
        from ferb.memory import echoes
        try:
            scrubbed = await asyncio.to_thread(echoes.finish_node_at, self.db.path, echo, running)
            if scrubbed.cards and self.learning is not None:      # cards that state it go too (no partial card)
                self._forget_nodes(scrubbed.cards, named=False)
            self.db.truncate_wal("memory.forget")
        except Exception as e:                       # the deletions stand; what's left is said, never raised
            self._journal().append("memory.forget_unfinished", {"error": f"{type(e).__name__}: {e}"})

    async def _finish_forget(self, plan: Any, running: Any = None) -> None:
        from ferb.memory import echoes
        try:
            if plan.echoes is not None:
                plan.scrubbed = await asyncio.to_thread(echoes.finish_at, self.db.path, plan.echoes, running)
            self._forget_stating_cards(plan)
            self.db.truncate_wal("memory.forget")
        except Exception as e:                       # the deletions stand; what's left is said, never raised
            self._journal().append("memory.forget_unfinished", {"error": f"{type(e).__name__}: {e}"})

    async def _settle_forgets(self) -> None:
        """Wait until every forget's echo scrub has finished."""
        while self._forgetting:
            await asyncio.gather(*list(self._forgetting), return_exceptions=True)

    def _forget_stating_cards(self, plan: Any) -> None:
        """After a forget's echo scrub: the cards of tasks given the item whose body states it are forgotten too, with
        their keys tombstoned so no capture rebuilds them (fix round, review Q1: no partial card)."""
        cards = plan.scrubbed.cards if plan.scrubbed is not None else []
        if cards and self.learning is not None:
            self._forget_nodes(cards, named=False)

    def _chat_forget(self, item_id: str, request_id: str) -> str:
        """A forget the owner asked for in chat (memory review I2), by the id the planner was shown (a memory item, the
        fact node it was shown as, or another learned item). Nothing is deleted now: FERB asks a plain Yes/No question
        naming everything that goes, as `ferb memory forget` does, and `_question_answered` forgets on the Yes."""
        node = self.learning.get(item_id) if self.learning is not None else None
        ident = str(node["body"].get("id", "")) if node and node["kind"] == "fact" else item_id
        if self.memory is not None and self.memory.get(ident) is not None:
            target, lines = ("memory", ident), self.memory.plan_forget(ident).describe(self.db)
        elif node is not None and node["kind"] not in ("fact", "identity") and self.learning is not None:
            doomed = sorted(self.learning.descendants([item_id]))
            target = ("learning", item_id)
            lines = [f"{_named_node(node)} and {len(doomed) - 1} record(s) built on it"]
        else:
            return "I couldn't find that memory, so nothing was deleted."
        reason = "Forget for good? This deletes:\n" + "\n".join("- " + line for line in lines)
        self._ask_owner("forget:" + target[0], target[1], reason, "memory-forget-" + request_id)
        return reason + "\nAnswer Yes or No."

    def _ask_owner(self, kind: str, ident: str, reason: str, request_id: str, approval_id: str | None = None) -> None:
        """One of FERB's own questions about memory, its capability and options by kind (`MEMORY_QUESTIONS`): a forget's
        Yes / No and a guess's Keep / Edit / Drop are FERB checks (one of their options answers it, in chat or with the
        notch's buttons); a guess's Edit takes the owner's own words (L36). `_question_answered` does what the answer
        asks. It is kept in `memory_questions` until it is answered, so a restart asks it again
        (`_restore_memory_questions`; rule 9)."""
        from ferb.core.types import Identity
        from ferb.orchestrator.steering import MEMORY_QUESTIONS
        from ferb.policy.rules import ActionRequest
        capability, options = MEMORY_QUESTIONS[kind.split(":", 1)[0]]
        action = ActionRequest(capability, "owner_question", Identity.OWNER, frozenset(), "", "", request_id,
                               frozenset())
        ap = self.state.approvals.question(action, reason, list(options), approval_id)
        self.db.conn.execute("INSERT OR IGNORE INTO memory_questions VALUES (?,?,?,?,?,?)",
                             (ap.id, kind, ident, reason, request_id, self.clock.now().isoformat()))
        self._owner_followups[ap.id] = (kind, ident)
        self._publish("approval.requested", {"approval_id": ap.id, "capability": ap.capability, "kind": ap.kind,
                                             "reason": ap.reason, "task_id": None, "options": list(ap.options)})

    def _ask_keep(self, item: Any, reason: str) -> None:
        """L36 §2 ("Ask right away"): one of FERB's guesses about the owner, asked at once with Keep / Edit / Drop;
        only while it waits (an inferred item still proposed), and never twice for one item (an open Keep or Edit
        question about it is enough)."""
        if item.origin != "inferred" or item.status != "proposed" or self.db.conn.execute(
                "SELECT 1 FROM memory_questions WHERE target = ? AND kind IN ('confirm', 'edit')", (item.id,)).fetchone():
            return
        self._ask_owner("confirm", item.id, reason, "memory-confirm-" + item.id)

    def _settle_guess(self, item_id: str, action: str, text: str = "") -> str:
        """The owner's answer to one of FERB's guesses (L36 §2). Keep makes it active, owner-confirmed. Edit saves the
        owner's text as their own words (a new owner-stated version of the same fact) and retracts the guess. Drop
        deletes the guess alone (`Memory.drop`; Ruling: never `forget`, which purges every version and the owner's
        messages). While a waiting "Forget for good?" would delete the guess (about it, or about another version of the
        same fact: `_forget_would_take`), Drop retracts it instead (scan ruling I1, re-review R1, fix round 1): a
        deletion would close a question about it unanswered, or cut it from the messages the question names, and that
        Yes still purges every copy. A replaced or dropped guess keeps its fingerprint either way. A line for the
        owner; FerbError for an unknown action, an empty edit, or an edit secret redaction hid whole (`EDIT_HIDDEN`)."""
        item = self.memory.get(item_id)
        if item is None or item.status != "proposed" or item.origin != "inferred":
            return "That guess isn't waiting any more."
        if action == "keep":
            self.memory.confirm(item_id)
            return "Kept."
        if action == "drop":
            if self._forget_would_take(item_id):
                self.memory.retract(item_id, "the owner said Drop", never_again=True)
                return "Dropped. FERB won't suggest it again; your question about forgetting it still waits."
            self.memory.drop(item_id)
            self._forget_nodes(["memory:" + item_id], named=False)      # its index node goes with it
            return "Dropped. FERB won't suggest it again."
        words = text.strip()
        if action == "edit" and words:
            if _all_hidden(self._redact_text(words)):
                raise FerbError(EDIT_HIDDEN)
            entry = self._journal().append("memory.edited", {"id": item_id, "content": words})
            mine = self.memory.edit(item_id, words, ("journal", entry.id))
            self.memory.retract(item_id, "the owner wrote it in their own words", never_again=True)
            return "Saved in your words: " + " ".join(mine.content.split())
        raise FerbError("Pick Keep, Edit or Drop; Edit needs the words to save.")

    def _guess_backlog(self) -> dict[str, Any] | None:
        """L36 ruling ("The backlog"): FERB's guesses that were waiting before it asked about each at once, offered
        once as one "Review N older guesses" list (each Keep / Edit / Drop, through op "guess"); a guess already asked
        about is not in it. Marked offered whenever it is read, so it never comes back; None when there is nothing."""
        if self._journal().of_kind(BACKLOG_OFFERED):
            return None
        asked = {r["target"] for r in self.db.conn.execute("SELECT target FROM memory_questions")}
        waiting = [i for i in self.memory.items("proposed") if i.origin == "inferred" and i.id not in asked]
        entry = self._journal().append(BACKLOG_OFFERED, {"count": len(waiting)})
        if not waiting:
            return None
        return {"id": entry.id, "text": f"Review {len(waiting)} older guess{'' if len(waiting) == 1 else 'es'}",
                "proposals": [{"id": i.id, "text": " ".join(i.content.split())} for i in waiting]}

    def _forget_would_take(self, item_id: str) -> bool:
        """Whether a waiting "Forget for good?" about a memory would delete this item: its target, another version of the
        same fact, or an item built on the same sources (`plan_forget`, worked out as its Yes would)."""
        from ferb.memory.items import MemoryRefused
        for row in self.db.conn.execute("SELECT target FROM memory_questions WHERE kind = 'forget:memory'").fetchall():
            try:
                if item_id in self.memory.plan_forget(row["target"]).items:
                    return True
            except MemoryRefused:                        # its memory went another way
                continue
        return False

    def _close_memory_questions(self, targets: Any, kinds: tuple[str, ...] | None = None) -> None:
        """FERB's questions about memory that stopped waiting (re-review NEW-1): of `kinds` (None: every kind) about
        `targets`. A memory confirmed or retracted another way closes its "Keep this?" (Keep / Edit / Drop) and Edit
        questions only; a memory or learned item deleted closes every question about it, the owner's "Forget for
        good?" too, which nothing else closes (re-review R1). Each is closed (superseded: never a No or a Drop),
        dropped from what its answer would do, and deleted from `memory_questions`, so no later turn or restart shows
        its words again."""
        gone = set(targets)
        if not gone:
            return
        for approval_id, (kind, target) in list(self._owner_followups.items()):
            if target in gone and (kinds is None or kind in kinds):
                self._owner_followups.pop(approval_id, None)
                if self.state.approvals.supersede(approval_id) is not None:
                    self._publish("approval.resolved", {"approval_id": approval_id, "state": "superseded"})
        marks = ",".join("?" * len(gone))
        which = f" AND kind IN ({','.join('?' * len(kinds))})" if kinds is not None else ""
        self.db.conn.execute(f"DELETE FROM memory_questions WHERE target IN ({marks}){which}",
                             [*sorted(gone), *(kinds or ())])

    def _restore_memory_questions(self) -> None:
        """At start: FERB's questions about memory that were still waiting (a forget's Yes / No, a guess's Keep / Edit /
        Drop and its Edit) are asked again, with their ids, words and options; one whose memory has gone or stopped
        waiting meanwhile is dropped."""
        for row in self.db.conn.execute("SELECT * FROM memory_questions ORDER BY at").fetchall():
            if row["kind"] == "forget:learning":
                gone = self.learning is None or self.learning.get(row["target"]) is None
            else:
                item = self.memory.get(row["target"]) if self.memory is not None else None
                gone = item is None or (row["kind"] in ("confirm", "edit") and item.status != "proposed")
            if gone:
                self.db.conn.execute("DELETE FROM memory_questions WHERE approval_id = ?", (row["approval_id"],))
            elif self.state.approvals.get(row["approval_id"]) is None:
                self._ask_owner(row["kind"], row["target"], row["reason"], row["request_id"], row["approval_id"])

    def _question_answered(self, ap: Any) -> str:
        """FERB's own follow-up to an answered question: a chat forget on its Yes (I2); a guess's Keep, Edit (which asks
        for the owner's words) or Drop (`_settle_guess`, L36). A line for the owner, or ''."""
        followup = self._owner_followups.pop(ap.id, None)
        if followup is None:
            return ""
        self.db.conn.execute("DELETE FROM memory_questions WHERE approval_id = ?", (ap.id,))
        what, ident = followup
        from ferb.memory.items import MemoryRefused
        yes = ap.state == "granted" and ap.choice == "Yes"
        if what == "confirm":
            # Ruling: a declined question is Drop (skipping FERB's own check is a No, L3)
            choice = ap.choice if ap.state == "granted" else "Drop"
            if choice == "Edit":
                item = self.memory.get(ident)
                if item is None or item.status != "proposed":
                    return "That guess isn't waiting any more."
                self._ask_owner("edit", ident, EDIT_QUESTION.format(text=" ".join(item.content.split())),
                                "memory-edit-" + ident)
                return ""
            return self._settle_guess(ident, "keep" if choice == "Keep" else "drop")
        if what == "edit":
            if ap.state != "granted" or not ap.answer:
                return self._settle_guess(ident, "drop")
            try:
                return self._settle_guess(ident, "edit", ap.answer)
            except FerbError as e:      # e.g. words routed from chat already hidden (rule 14): asked again, never saved
                self._ask_owner("edit", ident, ap.reason, "memory-edit-" + ident)
                return str(e)
        if not yes:
            return "Kept: nothing was forgotten."
        kind = what.removeprefix("forget:")
        try:
            if kind == "memory":
                gone = len(self._forget_memory(ident).items)
            elif self.learning.get(ident) is None:
                raise MemoryRefused(ident)
            else:
                gone = len(self._forget_nodes([ident]))
        except MemoryRefused:                            # forgotten another way while the question waited
            return "It was already gone: nothing else was deleted."
        self._journal().append("memory.forgot", {"via": "chat", "items": gone})
        return f"Forgot it: {gone} item(s) deleted, with the records built on them."

    async def _memory_learning(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        from ferb.memory.learning import LearningRefused
        l = self.learning
        if l is None:
            raise LearningRefused("learning store unavailable")
        op, ident = params.get("op", "inventory"), str(params.get("id", ""))
        if op == "backlog":
            return {"backlog": self._guess_backlog()}
        if op == "guess":
            return {"line": self._settle_guess(ident, str(params.get("action", "")), str(params.get("text") or ""))}
        if op == "inventory":
            l.sync_facts(self.memory)
            return {"items": [n | {"confirmable": not n["confirmed"] and (
                n["kind"] in ("identity_proposal", "procedure") or (n["kind"] == "fact" and n["state"] == "proposed"))}
                for n in l.items(history=bool(params.get("history")))]}
        if op == "search":
            return {"items": l.search(str(params.get("query", "")), **{
                k: str(params[k]) for k in ("kind", "app", "person", "since", "until") if params.get(k)})}
        if op == "get":
            node = l.get(ident)
            return {"item": node, "history": [n for n in l.items(history=True)
                    if node and n["kind"] == node["kind"] and n["key"] == node["key"]]}
        if op == "confirm":
            node = l.get(ident)
            if node and node["kind"] == "fact":           # a memory item waiting for the owner (I1): confirm it there
                self.memory.confirm(str(node["body"]["id"]))
                l.sync_facts(self.memory)
                return {"item": self._item_dict(self.memory.get(str(node["body"]["id"])))}
            return {"item": l.confirm(ident)}
        if op == "edit":
            node = l.get(ident)
            if node and node["kind"] == "fact":
                item_id = node["body"]["id"]
                await self._memory_edit(session, {"id": item_id, "content": str(params.get("text", ""))})
                l.sync_facts(self.memory)
                return {"items": l.search(node["key"])}
            return {"item": l.edit(ident, str(params.get("text", "")))}
        if op == "lesson_feedback":
            if params.get("feedback") not in ("helpful", "harmful"):
                raise LearningRefused("feedback must be helpful or harmful")
            return {"item": l.lesson_feedback(ident, params["feedback"] == "helpful")}
        if op == "retire":
            l.db.conn.execute("UPDATE learning_nodes SET state='retired' WHERE id=?", (ident,))
            return {"item": l.get(ident)}
        if op == "forget":
            node = l.get(ident)
            if node and node["kind"] == "fact":
                item_id = node["body"]["id"]
                if params.get("confirm"):
                    self._forget_memory(item_id)
                    await self._settle_forgets()
                    return {"deleted": [ident]}
                plan = self.memory.plan_forget(item_id)
                return {"descendants": sorted(l.descendants(["memory:" + s for s in plan.items])),
                        "will_delete": plan.describe(self.db)}
            doomed = sorted(l.descendants([ident]))
            if params.get("confirm"):
                deleted = self._forget_nodes([ident])
                await self._settle_forgets()             # the RPC answers once its echo pass is done (R3)
                return {"deleted": deleted}
            return {"descendants": doomed}
        if op == "rollback":
            return {"invalidated": l.rollback(str(params.get("batch", "")))}
        if op == "digest":
            return {"digest": l.digest()}
        if op == "identity":
            return {"item": l.put("identity", str(params.get("key", "owner")), {"text": str(params["text"])}, [],
                    origin_label="owner", made_by="owner:identity/1", owner=True, state="active")}
        if op == "consolidate":
            return await self.consolidator.catch_up(force=True)       # the owner asked: failed jobs too (I3)
        if op == "eval_export":
            from ferb.eval.memory import history_queries
            return history_queries(l)
        if op == "embedding":
            from ferb.memory import embeddings
            action = str(params.get("action", "status"))
            missing = embeddings.extra_missing()
            if action == "status":
                return {"active": getattr(l.embedder, "name", None), "available": missing is None,
                        "evaluations": [dict(r) for r in
                                        self.db.conn.execute("SELECT * FROM learning_embedding_evals ORDER BY at")]}
            if missing is not None:                      # found before any download (M8)
                raise LearningRefused(f"local embeddings need FERB's optional memory extra ({missing}), which this "
                                      "install doesn't have; nothing was downloaded")
            manifest_path = Path(str(params["manifest"]))
            data = embeddings.manifest(manifest_path)
            root = self.home / "models" / "embeddings" / embeddings.digest(data)
            await asyncio.to_thread(embeddings.download, root, data)
            if action == "prepare":
                return {"ready": True, "path": str(root), "active": False}
            model = await asyncio.to_thread(embeddings.LocalMLX, root, data)
            if action == "evaluate":
                cases = json.loads(Path(str(params["dataset"])).read_text())
                return await embeddings.evaluate_async(l, model, cases.get("cases", []) if isinstance(cases, dict) else cases)
            if action == "adopt":
                embeddings.adopt(l, model, str(params["evaluation"]))
                config = self.home / "memory-embedding.json"
                config.write_text(json.dumps({"manifest": data, "root": str(root), "model": model.name}))
                config.chmod(0o600)
                return {"active": model.name}
            raise LearningRefused("unknown embedding action")
        raise LearningRefused("unknown learning operation")

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
        resource_days = retention.days(self.config.get("retention", "resource_days", resource_use.DEFAULT_DAYS))
        roots = retention.finished_roots_before(self.db, (now - timedelta(days=trace_days)).isoformat())
        return _PurgePlan(now, trace_days, decision_days, evidence_days, recovery_days, resource_days,
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
        `retention.purged` is journaled only when the pass purged something or left roots for the next one. Old
        resource samples (L30, numbers only) go too, counted in the result, not journaled."""
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
        if self.perception is not None:
            self.perception.purge_text()
        return {"spans": spans, "decisions": decisions, "evidence": evidence, "recovery": copies, "downloads": trashed,
                "resources": self._purge_resources(plan)}

    def _purge_resources(self, plan: _PurgePlan) -> int:
        """L30: resource samples older than resource_days, by local day. Isolated: a failure is journaled and the pass
        goes on."""
        try:
            return resource_use.purge(self.db, plan.now.astimezone().date(), plan.resource_days)
        except Exception as e:
            self._journal().append("retention.failed", {"resources": True, "error": f"{type(e).__name__}: {e}"})
            return 0

    def _purge_downloads(self) -> int:
        """D35: when FERB's downloads folder passes browser.download_total_mb, its oldest working copies go to the
        Trash (no time-based clearing, owner 2026-10-09); their records stay. Journaled when any moved."""
        if self.downloads is None:
            return 0
        try:
            trashed = self.downloads.purge()
        except Exception as e:
            self._journal().append("retention.failed", {"downloads": True, "error": f"{type(e).__name__}: {e}"})
            return 0
        if trashed:
            self._journal().append("retention.downloads", {
                "trashed": trashed, "total_mb": self.downloads.limits().total_bytes // (1024 * 1024)})
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
        """Hourly, starting a minute after start-up so start-up stays quick. A failed pass is journaled, and the loop
        goes on even when that can't be (then it goes to stderr, redacted), as the resource loop does."""
        await asyncio.sleep(60)
        while True:
            try:
                await self._purge_pass()
            except Exception as e:
                error = f"{type(e).__name__}: {e}"
                try:
                    self._journal().append("retention.failed", {"error": error})
                except Exception as journal_error:
                    print(f"FERB: a retention pass failed ({self._redact_text(error)}) and couldn't be journaled "
                          f"({type(journal_error).__name__})", file=sys.stderr)
            await asyncio.sleep(3600)

    async def _resources_loop(self) -> None:
        """L30: a resource sample every SAMPLE_SECONDS (the owner's "every 5 minutes"); shutdown writes the last."""
        while True:
            await self.clock.sleep_until(self.clock.now() + timedelta(seconds=resource_use.SAMPLE_SECONDS))
            self._sample_resources()

    def _sample_resources(self) -> None:
        """One sample; a failure is journaled and the next sample covers this window too. Never raises: the loop goes
        on and shutdown finishes even when the failure can't be journaled (then it goes to stderr)."""
        try:
            self.resource_log.sample()
        except Exception as e:
            error = f"{type(e).__name__}: {e}"
            try:
                self._journal().append("resources.failed", {"error": error})
            except Exception as journal_error:
                print(f"FERB: a resource sample failed ({self._redact_text(error)}) and couldn't be journaled "
                      f"({type(journal_error).__name__})", file=sys.stderr)

    def _perception_counts(self) -> tuple[int, dict[str, int]]:
        """The perception batcher's wakes and its batches by flush reason since start, for the resource log; none
        before perception is built (start-up's baseline)."""
        if self.perception is None:
            return 0, {}
        batcher = self.perception.batches
        return batcher.wakes, dict(batcher.batches_by_reason)

    def _record_learning_usage(self, role: str) -> Callable[[str, dict[str, Any]], None]:
        """A learner's usage recorder: each usage event is a `model.usage` span naming that learner's role
        (consolidate or distill)."""
        def record(runtime: str, data: dict[str, Any]) -> None:
            span = self.spans.start("model.usage", task_id=None, parent_id=None,
                                    input={"runtime": runtime, "role": role})
            self.spans.finish(span, output=data, cost_usd=data.get("cost_usd"))
        return record

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
            await self._memory_pass()
            await asyncio.sleep(3600)

    async def _memory_pass(self) -> None:
        """One hourly pass: distill, consolidate, restore the memory embedding. Each job has its own error scope, so
        one failing never stops the others (L16); a failure never takes the daemon down and is tried again next hour."""
        assert self.distiller is not None
        jobs: list[tuple[str, Callable[[], Awaitable[Any]]]] = [("memory.distill_failed", self.distiller.catch_up)]
        if self.consolidator is not None:
            jobs.append(("memory.consolidate_failed", self.consolidator.catch_up))
        if self.learning is not None:
            jobs.append(("memory.embedding_restore_failed", self._restore_memory_embedding))
        for failed, job in jobs:
            try:
                await job()
            except asyncio.CancelledError:
                raise
            except Exception as e:
                self._journal().append(failed, {"error": f"{type(e).__name__}: {e}"})

    async def _restore_memory_embedding(self) -> None:
        if self.learning.embedder is not None:
            return
        config = self.home / "memory-embedding.json"
        from ferb.memory.embeddings import LocalMLX, extra_missing
        if not config.is_file() or extra_missing() is not None:     # keyword ranking only (status says why)
            return
        value = json.loads(config.read_text())
        adopted = self.db.conn.execute("SELECT 1 FROM learning_embedding_evals WHERE model=? AND adopted=1",
                                       (value["model"],)).fetchone()
        if adopted:
            self.learning.embedder = await asyncio.to_thread(LocalMLX, Path(value["root"]), value["manifest"])

    def _export_memory(self) -> None:
        from ferb.memory.export import export
        if self.memory is not None:
            export(self.memory, self.home / "memory")

    async def _remember(self, items: list[Any], own_words: str, source_id: str,
                        sites: frozenset[str] = frozenset()) -> list[dict[str, str]]:
        """Chat capture (memory review I1, MP2; the check is `items.capture`, shared with distillation's quotes, L36).
        `own_words`: what the owner typed (pasted parts left out). The quote must be whole words of it. Saved as
        owner_stated is only the owner's own text: the whole sentence(s) of their words that hold the planner's content
        (`owner_sentences`), so a slice never drops a negation or a condition. Any other content is FERB's wording of
        what they said: an inferred proposal, out of every prompt until they answer Keep to the question that offers
        it (`_ask_keep`); one whose quote shares no word with it rests on nothing the owner said and is refused. A
        preference in the owner's typed words joins their identity whatever was on screen (owner decision 2026-10-20:
        "No hold for my typed words"). Per saved item: its receipt and the line code adds to the reply; per refused
        item: why, in plain words for the owner's line (owner 2026-10-24: the reason, never "a problem on my side").
        An entry with `login_origin` also gives that site's login record its username (`_save_login_name`, in a worker
        thread; `sites`: the sites the turn's task names, L36 §5)."""
        from ferb.memory.items import CAPTURE_TOLD, capture
        saved: list[dict[str, str]] = []
        for it in items:
            got = capture(own_words, str(it.content), str(it.quote))
            if got.refused:
                self._journal().append("memory.capture_refused", {"key": it.key, "why": got.refused})
                saved.append({"refused": CAPTURE_TOLD[got.refused]})
                continue
            try:
                m = self.memory.propose(it.kind, it.subject, it.key, got.content, got.origin,
                                        [("journal", source_id)], 1.0 if got.origin == "owner_stated" else 0.7)
            except FerbError as e:
                self._journal().append("memory.capture_refused", {"key": it.key, "why": str(e)})
                saved.append({"refused": reason(e, self.redact)})
                continue
            text = " ".join(m.content.split())
            told = (await self._save_login_name(it, m.id, own_words, sites, m.origin)
                    if getattr(it, "login_origin", None) else "")
            if m.origin == "owner_stated" or m.status == "active":      # active: FERB's wording the owner kept before
                saved.append({"receipt": f"remembered ({m.id}): {m.content}",
                              "line": f"Remembered: {text}" + (f"\n{told}" if told else "")})
            else:
                self._ask_keep(m, KEEP_WORDING.format(text=text))
                saved.append({"receipt": f"proposed ({m.id}): {m.content} (FERB's wording; waits for the owner's Keep)",
                              "line": "Saved in my words, not yours, so I won't use it until you answer Keep: "
                                      f"“{text}”."})
        return saved

    async def _save_login_name(self, entry: Any, item_id: str, own_words: str, sites: frozenset[str],
                               origin: str = "owner_stated") -> str:
        """L36 §5: an owner-stated entry's username for its site's login record. Only when the username is the owner's
        own words (kept in their spelling, `owner_span`; never FERB's wording, which is an inferred item) and the site
        is an https:// origin whose host the owner typed in this message or one the turn's task names
        (`Supervisor._turn_sites`), exactly, a leading "www." aside (`same_site`, the rule a record's sites follow:
        never a subdomain, which on shared hosting is another tenant's). The write runs in a worker thread: the handle
        file's lock may be held by a hidden input's Keychain save (coordinator ruling). A line for the owner, or ""
        (the refusal is journaled, `memory.login_refused`)."""
        from ferb.identity.owner_secrets import OwnerSecrets, login_place, same_site, site_host, typed_site
        from ferb.memory.items import owner_span
        from ferb.policy import provenance
        from ferb.sanitize.redact import REDACTED
        place = login_place(str(entry.login_origin))
        host = site_host(place) if place.startswith("https://") else ""
        named = bool(host) and (typed_site(host, provenance.prepare(own_words))
                                or any(same_site(site_host(site), host) for site in sites))
        username = owner_span(own_words, str(entry.content)) if origin == "owner_stated" else None
        why = ("the site isn't an https:// address" if not host else "the site isn't one you named" if not named
               else "the username isn't in your words" if not username
               else "the username was hidden as a secret" if REDACTED in str(entry.content) else "")
        if not why and username:
            try:
                await asyncio.to_thread(OwnerSecrets(self.home).set_username, place, username, "memory:" + item_id)
                return f"Saved as your login name for {host}."
            except Refused as e:
                why = str(e)
        self._journal().append("memory.login_refused", {"item": item_id, "place": place, "why": why})
        return ""

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
        await self._clear_login_names([str(params["id"])])   # L36 §5: a username goes with the memory it came from
        return {"retracted": params["id"]}

    async def _memory_forget(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        keep = bool(params.get("keep_sources"))
        if not params.get("confirm"):
            plan = self.memory.plan_forget(str(params["id"]), keep)
            return {"confirmed": False, "will_delete": plan.describe(self.db)}
        described = self.memory.plan_forget(str(params["id"]), keep).describe(self.db)
        self._forget_memory(str(params["id"]), keep)
        await self._settle_forgets()                     # the RPC answers once the forget is complete
        return {"confirmed": True, "deleted": described}

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
            self.supervisor.schedule_live = self._schedule_live
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

    async def _osascript_notify(self, title: str, text: str) -> None:            # posts as OSASCRIPT_POSTER
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

    async def _task_result(self, ev: dict[str, Any], shown: bool, alert: bool = False, publish: bool = True) -> None:
        """What FERB told (or will tell) the owner about a task; a macOS notification only when chat was closed and no
        connected FERB client receives `task.result` to show it (L9). An `alert` (a watch's condition met) is told the
        way a reminder is: a notification now, whether or not a client shows it, even while the owner is away. Without
        `publish` (a watch check with nothing to tell) it is only recorded: no client hears of it (fix wave 6c, I3)."""
        assert self.db is not None
        ev = self.redact(ev)
        answer = str(ev.get("answer") or ev.get("summary") or "")
        shown = shown or ev.get("status") == "cancelled"          # stopped by the owner: they know
        at = str(ev.get("at") or self.clock.now().isoformat())    # the outcome's own time when a chat turn sent it
        self.db.conn.execute("INSERT OR REPLACE INTO task_results (task_id, at, status, goal, answer, shown) VALUES "
                             "(?, ?, ?, ?, ?, ?)", (ev["task_id"], at, ev.get("status", ""),
                                                    str(ev.get("goal", "")), answer, int(shown)))
        if not publish:
            return
        if alert:
            await self._notify("FERB alert", answer, urgent=True)
        elif not shown and ev.get("status") != "cancelled" and not self._client_shows("task.result"):
            await self._notify("FERB finished" if ev.get("status") == "done" else "FERB", answer)
        if "request_id" not in ev:                  # a resumed plan or scheduled run: the request its task ran for
            row = self.db.conn.execute("SELECT request_id FROM tasks WHERE id = ?", (ev["task_id"],)).fetchone()
            ev = {**ev, "request_id": row["request_id"] if row is not None else None}
        self._publish('task.result', {**ev, 'at': at, 'shown': shown})

    def _approval_notice(self, data: dict[str, Any]) -> None:
        """An approval is waiting and no chat is open for that task: tell the owner (notification), only when no
        connected FERB client receives `approval.requested` to show it (L9, spec 8.4; final review M2). The banner
        names the question, never a CLI command (spec 4.4). The setup profile question is never a banner (L3): an open
        client (`chat.attach`, `ferb setup`) started it and shows it inline."""
        tid = str(data.get("task_id") or "")
        if (tid.split(".", 1)[0] in self._attached_trees or data.get("capability") == "setup.persona"
                or self._client_shows("approval.requested")):
            return
        text = f"FERB needs your OK: {str(data.get('summary') or data.get('reason', ''))}"   # the short line first
        asyncio.get_running_loop().create_task(self._notify("FERB needs you", text, urgent=True))

    async def _notify_safely(self, title: str, text: str) -> None:
        banner = None
        if self.perception is not None and self.notify is not _no_notify:
            # FERB knows its own banners (L10): the Notification Center row they become is FERB's, not an arrival.
            shown = {"title": self._redact_text(title), "body": self._redact_text(text)}     # as osascript shows it
            banner = self.perception.banner_posted(self.redact(shown), OSASCRIPT_POSTER)
        try:
            await self.notify(title, text)
        except Exception:
            if banner is not None:                    # never posted: no row of it will come back
                self.perception.banner_failed(banner)

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
        rows = self.db.conn.execute("SELECT r.*, t.outcome_json, t.request_id FROM task_results r LEFT JOIN tasks t "
                                    "ON t.id = r.task_id WHERE r.shown = 0 ORDER BY r.at").fetchall()
        if not params.get('peek'):
            for r in rows:
                self.db.conn.execute("UPDATE task_results SET shown = 1 WHERE task_id = ?", (r["task_id"],))
        persona = self.persona.status() if self.persona is not None else {'state': 'not_started'}
        approvals = self.state.approvals.pending()
        request_trees: dict[str, set[str]] = {}
        for request_id in {ap.request_id for ap in approvals}:
            request_trees[request_id] = {row[0] for row in self.db.conn.execute(
                "SELECT DISTINCT tree_id FROM tasks WHERE request_id=?", (request_id,))}
        pending = [{"approval_id": ap.id, "capability": ap.capability, "kind": ap.kind, "reason": ap.reason,
                    "task_id": next(iter(request_trees[ap.request_id]))
                        if len(request_trees.get(ap.request_id, ())) == 1 else None,
                    "options": list(ap.options), **({"display_text": ap.display_text} if ap.display_text else {}),
                    **({"summary": ap.summary} if ap.summary else {})} for ap in approvals]
        if persona.get('approval'):
            pending = [persona['approval'] if p['approval_id'] == persona['approval']['approval_id'] else p
                       for p in pending]
        running = [{"task_id": t, "goal": g}                  # a watch check isn't listed: no client adopts it (I3)
                   for t, g in (self.supervisor.running_trees(watches=False) if self.supervisor else [])]
        results = []
        for row in rows:
            recorded = json.loads(row['outcome_json'] or '{}')
            # request_id and at: a client whose chat.send stream ended early still places the answer (L6, L7)
            results.append({key: row[key] for key in ('task_id', 'request_id', 'at', 'status', 'goal', 'answer')} |
                           {'artifacts': recorded.get('artifacts', []), 'verified': recorded.get('verified'),
                            'verification': recorded.get('verification', [])})
        return {"results": results, "running": running, "approvals": self.redact(pending),
                "persona": {'state': persona['state'], **({'error': persona['error']} if persona.get('error') else {})},
                "memory_digest": self.learning.digest() if self.learning and not params.get("peek") else None,
                "notices": self.attention_notices.unseen() if self.attention_notices else [],
                "delegated_conversations": self.supervisor.conversations.list() if self.supervisor else [],
                "loops": self.supervisor.loop_statuses() if self.supervisor else []}

    async def _results_ack(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        """A client that reads `chat.status` with `peek` showed these results in its chat: mark only them shown, so
        `ferb chat` doesn't replay them as arrived while it was closed."""
        assert self.db is not None
        task_ids = params.get('task_ids')
        if not isinstance(task_ids, list) or not all(isinstance(t, str) and t for t in task_ids):
            raise ValueError('task_ids must be a list of task ids')
        for task_id in task_ids:
            self.db.conn.execute("UPDATE task_results SET shown = 1 WHERE task_id = ?", (task_id,))
        return {"ok": True}

    def _resume_conversation_plans(self) -> None:
        if self.supervisor is not None:
            job = asyncio.create_task(self.supervisor.resume_plans())
            self._runs.add(job)
            job.add_done_callback(self._runs.discard)

    async def _conversations_list(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        assert self.supervisor is not None
        ident = params.get('id')
        return self.supervisor.conversations.digest(ident) if ident else {'conversations': self.supervisor.conversations.list()}

    async def _conversations_change(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        assert self.supervisor is not None
        self.supervisor.change_conversation(str(params['id']), str(params['action']), str(params['owner_words']))
        self._journal().append('conversation.owner_change', self.redact(params), origin='owner')
        return {'conversation': self.supervisor.conversations.get(str(params['id']))}

    def _notification_rule_from_chat(self, rule: Any, owner_words: str) -> str:
        if self.perception is None:
            return 'Notification perception is unavailable.'
        try:
            ident = self.perception.policy.add(rule.app, rule.conversation, rule.topic, rule.quote, owner_words,
                                               rule.until_at, rule.exceptions)
            self._journal().append('notification.owner_rule', {'id': ident, **rule.model_dump()}, origin='owner')
            return 'Saved your notification rule for ' + (rule.conversation or rule.app) + '.'
        except ValueError as exc:
            return str(exc)

    async def _notification_rules(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        if self.perception is None or self.db is None:
            raise ValueError('Notification perception is unavailable')
        if params.get('remove'):
            self.db.conn.execute('DELETE FROM notification_rules WHERE id=?', (str(params['remove']),))
        return {'rules': [dict(r) for r in self.db.conn.execute('SELECT * FROM notification_rules ORDER BY created_at')],
                'measurements': self.perception.policy.measurements()}

    async def _notification_label(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        if self.perception is None:
            raise ValueError('Notification perception is unavailable')
        self.perception.policy.label(str(params['event_id']), str(params['expected']), str(params['owner_quote']))
        return self.perception.policy.measurements()

    async def _attention_ack(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        """`attention.ack {ids}`: a client showed these notices (they leave `chat.status`). They were told when posted.
        `{id}` is the single-notice form older CLIs send."""
        if self.attention_notices is None:
            raise ValueError('Owner notices are unavailable')
        ids = params['ids'] if 'ids' in params else [params['id']]
        if not isinstance(ids, list) or not ids or not all(isinstance(i, str) and i for i in ids):
            raise ValueError('ids must be a nonempty list of notice ids')
        self.attention_notices.acknowledge(ids)
        return {'ok': True}

    def _client_shows(self, topic: str) -> bool:
        """A connected FERB client receives this topic (L9): an authenticated `logs.subscribe` stream with no filter
        (the notch UI, `ferb chat`) or a filter that includes it. Then the client shows it and the macOS banner stays
        off; a subscription filtered to other topics (`ferb logs --filter timers`, `ferb voice`) shows nothing."""
        return any(sub.wants(topic) for sub in self._log_subscriptions)

    async def _loop_notice(self, tree_id: str, step_id: str, text: str, notice_origin: str = origin_.FERB) -> None:
        """L31: a loop's back-off or pause notice, posted to the owner's chat like a notice (listed in `chat.status`
        until a client shows it; a banner when no client is connected). Its origin follows its text: a back-off is
        FERB's own; a pause quotes the round's words, so it also carries INFERRED."""
        if self.attention_notices is None:
            return
        import uuid
        notice = self.attention_notices.add(f'loop-{step_id}-{uuid.uuid4().hex[:12]}', [f'task:{tree_id}'], text,
                                            notice_origin=notice_origin)
        if notice is not None:
            await self._post_notice(notice)

    async def _post_notice(self, notice: dict[str, Any]) -> None:
        """L9: a notice is a FERB message in the owner's chat: journaled `chat.notice`, published `attention.notice`, and
        listed in `chat.status` until a client acknowledges it. Posting it is telling: a notification it cites gets its
        told receipt now. A macOS banner (its text, never its outside quotes) only when no connected FERB client
        receives `attention.notice`."""
        assert self.attention_notices is not None and self.db is not None
        view = self.attention_notices.view(notice)
        self._journal().append('chat.notice', {k: view[k] for k in ('id', 'text', 'quotes')}, origin=notice['origin'])
        if self.perception is not None:
            for ref in json.loads(notice['source_refs_json']):
                row = self.db.conn.execute('SELECT source,data_json FROM trail WHERE id=?', (ref.removeprefix('trail:'),)).fetchone()
                if row and row['source'] == 'notification':
                    data = json.loads(row['data_json'])
                    self.perception.policy.told(data.get('bundle_id', ''), data.get('conversation', ''),
                                               ref.removeprefix('trail:'), view['text'])
        self._publish('attention.notice', view)
        if not self._client_shows('attention.notice'):
            await self._notify('FERB noticed', view['text'])

    def _attention_effect(self, decision: str, effect: str) -> None:
        """Q4/C3: what Jev's live trail.attention yes led to, on its decision row (`ferb decide stats`)."""
        if self.jev is not None:
            self.jev.mark_effect('trail.attention', decision, effect)

    def _attention_app(self, event: dict[str, Any]) -> str:
        """The app a Jev yes batch is about: that of its first observation row (a batch never spans apps)."""
        from ferb.sense.batches import app_of
        for ref in event.get('source_refs', []):
            if self.db is not None and isinstance(ref, str) and ref.startswith('trail:'):
                row = self.db.conn.execute('SELECT source,data_json FROM trail WHERE id=?', (ref[6:],)).fetchone()
                if row is not None and row['source'] != 'notification':
                    return app_of(json.loads(row['data_json']))
        return ''

    def _attention_arrived(self, event: dict[str, Any]) -> None:
        """L10, one moment one message: at most one consideration runs per app. A yes batch for an app whose
        consideration is running waits and merges into that app's next consideration; there is no timer."""
        app = self._attention_app(event)
        waiting = self._attention_waiting.get(app)
        if waiting is not None:
            waiting.append(event)
            return
        self._attention_waiting[app] = [event]
        job = asyncio.create_task(self._attention_turns(app))
        self._runs.add(job)
        job.add_done_callback(self._runs.discard)

    async def _attention_turns(self, app: str) -> None:
        """Consider an app's waiting batches together, then whatever arrived meanwhile, until none waits."""
        try:
            while self._attention_waiting[app]:
                events, self._attention_waiting[app] = self._attention_waiting[app], []
                await self._consider_attention(*events)
        finally:
            del self._attention_waiting[app]

    async def _consider_attention(self, *events: dict[str, Any]) -> None:
        """One consideration of one or more yes batches about one app (merged, rows in the order they happened). Every
        batch's decision records the outcome. Already told is scoped to the rows' app (L10)."""
        from ferb.sense.batches import app_of
        decisions = list(dict.fromkeys('batch:' + str(event.get('batch_id')) for event in events))

        def effect(text: str) -> None:
            together = f' (one consideration for {len(decisions)} batches)' if len(decisions) > 1 else ''
            for decision in decisions:
                self._attention_effect(decision, text + together)
        if self.supervisor is None or self.db is None or self.attention_notices is None:
            effect('not considered: the orchestrator is not running')
            return
        try:
            found = []
            for ref in dict.fromkeys(ref for event in events for ref in event.get('source_refs', [])):
                if not isinstance(ref, str) or not ref.startswith('trail:'):
                    continue
                row = self.db.conn.execute('SELECT * FROM trail WHERE id=?', (ref[6:],)).fetchone()
                if row is None or row['source'] == 'notification':
                    continue                 # notification decisions remain hard shadow, including mixed batches
                found.append((row['seq'], ref, {'id': row['id'], 'at': row['at'], 'source': row['source'],
                                                'actor': row['actor'], 'task_id': row['task_id'],
                                                'data': json.loads(row['data_json']), 'origin': 'external:observation'}))
            found.sort(key=lambda item: item[0])                  # merged batches read in the order they happened
            refs, rows = [ref for _, ref, _ in found], [row for _, _, row in found]
            if not rows:
                effect('not considered: no observation rows left (notifications stay shadow)')
                return
            told = self.attention_notices.told({app_of(row['data']) for row in rows})
            result = await self.supervisor.attention(refs, rows, told)
            if result.get('tell') is not True or not str(result.get('text', '')).strip():
                effect('considered; nothing told')
                return
            notice = self.attention_notices.add(decisions[0], result['source_refs'], str(result['text']),
                                                result.get('quotes'), refs,
                                                origin_.least_trusted(row['origin'] for row in rows))
            if notice is not None:                    # posted once: a replayed batch never posts again
                await self._post_notice(notice)
                effect('told in chat: notice ' + notice['id'])
        except Exception as exc:
            effect('consideration failed: ' + type(exc).__name__)
            self._journal().append('attention.failed', {'batch_ids': [str(e.get('batch_id')) for e in events],
                                                        'error': type(exc).__name__})

    async def _chat_attach(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        """Chat is open again: approvals for running tasks are asked there, not by notification."""
        for t, _ in (self.supervisor.running_trees() if self.supervisor else []):
            self._attached_trees.add(t)
        assert self.supervisor is not None
        conversation = self.supervisor.continuity.attach(bool(params.get("new")), params.get("resume"))
        self._prepare_persona()
        return {"ok": True, "session_id": conversation, "capabilities": ["input_paths"]}

    def _remember_persona(self, content: str) -> None:
        content = self._redact_text(content)
        entry = self._journal().append('memory.added', {'content': content})
        from ferb.device.persona import PERSONA_KEY
        self.memory.propose('preference', 'owner', PERSONA_KEY, content, 'owner_stated',
                            [('journal', entry.id)], 1.0)

    def _prepare_persona(self, refresh: bool = False) -> dict[str, Any]:
        if self._shutting_down:
            return {'state': 'closed'}
        from ferb.daemon.persona import PersonaReview
        if self.persona is None:
            self.persona = PersonaReview(self.config, self.state.approvals, None,
                remember=self._remember_persona, publish=self._publish,
                record=lambda kind, data: self._journal().append(kind, data), redact=self.redact)
        if self.persona_scan is not None:
            # Snapshot only owner-attributed app names/counts on the database thread; no window titles/content.
            activity = [dict(r) for r in self.db.conn.execute(
                "SELECT json_extract(data_json,'$.bundle_id') AS bundle_id, "
                "json_extract(data_json,'$.app') AS app, COUNT(*) AS focus_events, MAX(at) AS last_used "
                "FROM trail WHERE source='app_focus' AND actor='owner' GROUP BY bundle_id")]
            device = self.config.get('device', 'id')
            if not device:
                device = self.ids.new_id()
                self.config.set('device', 'id', device)
            excluded = self.config.exclude_apps()
            self.persona.collect = lambda: self.persona_scan(self.home, device, activity, excluded)
        return self.persona.start(refresh=refresh)

    async def _setup_persona(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        if params.get('status') and self.persona is not None:
            return self.persona.status()
        return self._prepare_persona(refresh=params.get('refresh') is True)

    async def _task_stop(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        """Ctrl-C in ferb chat: stop the request's task, recorded as stopped by the owner."""
        assert self.supervisor is not None
        ids = [str(t) for t in params.get("task_ids", [])] or [t for t, _ in self.supervisor.running_trees()]
        stopped = [t for t in ids if self.supervisor.stop_tree(t)]
        for t in stopped:
            self._journal().append("task.stopped", {"task_id": t, "by": "owner"})
        return {"stopped": stopped}

    # ---------- schedules ----------

    def _schedule_create(self, spec: Any, own_words: str, request_id: str, origins: set[str],
                         request_origins: set[str] | None = None) -> str:
        """From chat: the owner's words (quote-checked), the targets recorded now, the first run in the future.
        `origins`: what the turn that wrote the scheduled request was given; its runs inherit them (S3, MP2).
        `request_origins`: what that request itself carried, its runs' request origins for the sink rule (S4, L4)."""
        from ferb.orchestrator.schedules import WATCH_SAID, Rule, ScheduleError
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
            # D30: the standing scope stated now, with the owner here, is the only one the unattended runs get; only a
            # scope whose destinations the owner typed (now, or when stating a remembered item) is recorded (S3, MP2)
            scopes, unnamed = self._schedule_scopes(getattr(spec, "standing_sends", []), own_words, request_id)
            s = self.schedules.add(spec.request, own_words,
                                   recorded.to_json() | ({"standing_sends": scopes} if scopes else {}), rule, first,
                                   spec.watch)
        except (ScheduleError, ValueError) as e:
            return f"I couldn't schedule that: {e}."
        t = self.timers.add("schedule", first, s.id)
        self.schedules.set_timer(s.id, t.id, first)
        # the statement the owner is shown: each run's grant (`schedule:<id>`) links back to these recorded scopes
        self._journal().append("schedule.created", {"schedule_id": s.id, "request": s.request, "rule": rule.describe(),
                                                    "request_id": request_id, "standing_sends": scopes,
                                                    "not_recorded": unnamed, "watch": s.watch}
                               | ({"request_origins": sorted(request_origins)} if request_origins else {}),
                               origin=origin_.join(origins))
        # The owner's line in plain words (L11); the schedule's id stays in the journal row above
        at = f"{first.astimezone():%a %d %b %H:%M}"
        when = f"on {at}" if rule.repeat == "none" else f"{rule.describe()}, first on {at}"
        said = ", ".join(part for part in (f"🗓 Scheduled: {s.request} — {when}", WATCH_SAID[s.watch]) if part)
        from ferb.orchestrator.plans import SendScope
        from ferb.policy import confirm
        if scopes:
            said += "\n" + confirm.from_task_lines("Each run will send these without asking each time:\n" + "\n".join(
                "- " + SendScope.model_validate(scope).line() for scope in scopes))
        if unnamed:
            said += "\n" + confirm.from_task_lines(
                "These weren't recorded, because their destination isn't in words you typed (now, or when you told me "
                "to remember it). In each run they get FERB's usual checks, and a send that asks can't be answered "
                "with nobody there, so it's reported not done:\n" + "\n".join("- " + line for line in unnamed))
        return said

    def _schedule_scopes(self, stated: list[Any], own_words: str,
                         request_id: str) -> tuple[list[dict[str, Any]], list[str]]:
        """The stated scopes a schedule records (redacted), and the lines of those it doesn't: a scope is recorded only
        when its site, account and recipients appear in what the owner typed, in this turn or behind a remembered item
        (`_typed_behind_memory`); never in a remembered item's wording (D30, S3, MP2)."""
        from ferb.policy.standing import owner_named
        owner_text = "\n".join([own_words, *self._typed_behind_memory(request_id)])
        recorded, unnamed = [], []
        for scope in stated:
            if owner_named(scope, owner_text):
                recorded.append(self.redact(scope.model_dump()))
            else:
                unnamed.append(self.redact(scope.line()))
        return recorded, unnamed

    def _typed_behind_memory(self, request_id: str) -> list[str]:
        """What the owner typed behind each active owner-stated memory item: the text of the `ferb memory add` / `edit`
        entry it rests on, or the owner's own words (pasted parts left out) of the chat turn it was captured in. Never
        the item's content (memory review I1 keeps chat captures to text the owner typed; this stays the narrower rule).
        The current request's turn is left out: an item captured in it adds nothing beyond its typed words (MP2). The setup
        persona is left out too: FERB built it from app names and the owner only approved it, so it never names a
        send destination (controller ruling, Task 7 x Task 18)."""
        from ferb.device.persona import PERSONA_KEY
        from ferb.memory.items import slug
        from ferb.sanitize.untrusted import recorded_own_words
        if self.memory is None:
            return []
        persona = slug(PERSONA_KEY)                      # as memory stores the key
        ids = sorted({src.removeprefix("journal:") for item in self.memory.items()
                      if item.origin == "owner_stated" and item.key != persona
                      for src in item.source_ids if src.startswith("journal:")})
        if not ids:
            return []
        assert self.db is not None
        typed = []
        for kind, data_json in self.db.conn.execute(
                f"SELECT kind, data_json FROM journal WHERE id IN ({','.join('?' * len(ids))}) ORDER BY seq", ids):
            data = json.loads(data_json)
            if kind in ("memory.added", "memory.edited"):
                typed.append(str(data.get("content", "")))
            elif kind == "chat.message" and data.get("request_id") != request_id:
                parts = data.get("untrusted_parts")      # the pasted parts it held, plus one for attached paths
                own = recorded_own_words(str(data.get("text", "")),
                                         None if parts is None else int(parts) - bool(data.get("input_paths")))
                typed += [own] if own else []
        return typed

    def _reminder_cancel_from_chat(self, rc: Any, own_words: str) -> str:
        norm = " ".join(own_words.lower().split())
        if len(rc.quote.strip()) < 3 or " ".join(rc.quote.lower().split()) not in norm:
            return "I didn't change any reminder — could you say which one in your own words?"
        t = self.timers.get(rc.id) if self.timers is not None else None
        if t is None or t.kind != "reminder" or not self.timers.cancel(rc.id):
            return "I couldn't find that reminder among the pending ones."
        self._journal().append("timer.cancelled", {"timer_id": rc.id, "by": "owner"})     # the id stays here (L11)
        return f"⏰ Cancelled the reminder “{t.message}” for {t.fire_at.astimezone():%a %d %b %H:%M}."

    def _schedule_change_from_chat(self, ch: Any, own_words: str) -> str:
        norm = " ".join(own_words.lower().split())
        if len(ch.quote.strip()) < 3 or " ".join(ch.quote.lower().split()) not in norm:
            return "I didn't change any schedule — could you say which one in your own words?"
        if not self._changeable(ch.id, ch.action):          # the owner's line never shows the id (L11)
            return "There's no active schedule like that to change."
        return self._schedule_change(ch.id, ch.action)

    def _last_run_holds_scope(self, sid: str) -> bool:
        """Whether a one-time schedule's last run is still under way: from `_run_schedule` marking it done (its
        planner turn included, before the run's tree is granted the scope) until the run ends, or while a run holds
        the schedule's recorded scope (`schedule:<id>`, D30; a plan resumed after a restart) (final review M9)."""
        from ferb.policy.standing import SCHEDULE_GRANT, StandingPermissions
        assert self.db is not None
        if sid in self._last_runs:
            return not self._last_run_withdrawn(sid)
        standing = self.supervisor.standing if self.supervisor is not None else StandingPermissions(self.db)
        return standing.held(SCHEDULE_GRANT + sid)

    def _changeable(self, sid: str, action: str) -> bool:
        """An active or paused schedule; or a done one whose last run is under way, for a cancel or pause, which
        withdraws that run's scope (M9)."""
        s = self.schedules.get(sid)
        if s is None or s.state == "cancelled":
            return False
        return s.state != "done" or (action in ("cancel", "pause") and self._last_run_holds_scope(sid))

    def _listed_schedules(self) -> list[Any]:
        """The schedules the owner (`ferb schedules`) and the planner (its facts) can change: active and paused ones,
        and a done one whose last run is under way (M9)."""
        return self.schedules.all() + [s for s in self.schedules.all(("done",)) if self._last_run_holds_scope(s.id)]

    def _schedule_change(self, sid: str, action: str) -> str:
        s = self.schedules.get(sid)
        if s is None or not self._changeable(sid, action):
            return f"There's no active schedule {sid}."
        if s.state == "done":            # its last run is under way: the cancel or pause withdraws that run's scope
            if action == "cancel":
                self.schedules.set_state(sid, "cancelled")
            self._journal().append(f"schedule.{action}", {"schedule_id": sid})   # before the withdrawal: a grant
            self._withdraw_schedule_scope(sid, action)                           # checks it from now on
            covered = ("Its run already under way keeps going, but nothing it sends is covered any more: each send "
                       "gets FERB's usual checks.")
            if action == "pause":            # no future run to pause: it stays done, never "paused" for good
                return f"{s.request} has no future runs, so there's nothing left to pause. {covered}"
            return f"Cancelled: {s.request}. {covered}"
        if action == "cancel":
            self.schedules.set_state(sid, "cancelled")
            if s.timer_id:
                self.timers.cancel(s.timer_id)
            self._withdraw_schedule_scope(sid, action)
        elif action == "pause":
            self.schedules.set_state(sid, "paused")
            if s.timer_id:
                self.timers.cancel(s.timer_id)
            self._withdraw_schedule_scope(sid, action)
        elif action == "resume":
            self.schedules.set_state(sid, "active")
            nxt = s.rule.next_after(max(self.clock.now(), s.first_run - timedelta(seconds=1)), s.first_run) \
                if s.rule.repeat != "none" else (s.next_run if s.next_run and s.next_run > self.clock.now() else None)
            if nxt is None:
                self.schedules.set_state(sid, "done")
                return f"{s.request} has no future runs left."
            t = self.timers.add("schedule", nxt, sid)
            self.schedules.set_timer(sid, t.id, nxt)
        self._journal().append(f"schedule.{action}", {"schedule_id": sid})
        return {"cancel": "Cancelled", "pause": "Paused", "resume": "Resumed"}[action] + f": {s.request}"

    def _withdraw_schedule_scope(self, sid: str, action: str) -> None:
        """D30: a cancelled or paused schedule's recorded scope stops covering its runs at once, a run in flight or one
        suspended at shutdown included (each holds it as `schedule:<id>`)."""
        from ferb.policy.standing import SCHEDULE_GRANT, StandingPermissions
        assert self.db is not None
        standing = self.supervisor.standing if self.supervisor is not None else StandingPermissions(self.db)
        tasks = standing.revoke_approval(SCHEDULE_GRANT + sid)
        if tasks:
            self._journal().append("standing.revoked", {"approval_id": SCHEDULE_GRANT + sid, "task_ids": tasks,
                                                        "reason": f"schedule {action}"})

    def _schedule_live(self, sid: str) -> bool:
        """Whether a schedule's runs may still hold its recorded scope: it exists and wasn't cancelled or paused. A
        done schedule counts (`_run_schedule` marks it done just before running its last occurrence, D30) unless the
        owner paused it during that run (a pause of a schedule with no future runs leaves it done, journaled) or it is
        a one-shot watch that ended (`schedule.met`): that withdraws its scope as a cancel does (fix wave 6c, M1)."""
        s = self.schedules.get(sid)
        return s is not None and (s.state == "active" or s.state == "done" and not self._last_run_withdrawn(sid))

    def _last_run_withdrawn(self, sid: str) -> bool:
        """A `schedule.pause` or `schedule.met` journaled after the schedule's latest run started."""
        assert self.db is not None
        row = self.db.conn.execute(
            "SELECT kind FROM journal WHERE kind IN ('schedule.run', 'schedule.pause', 'schedule.met') "
            "AND json_extract(data_json, '$.schedule_id') = ? ORDER BY seq DESC LIMIT 1", (sid,)).fetchone()
        return row is not None and row[0] in ("schedule.pause", "schedule.met")

    async def _run_schedule(self, sid: str, overdue_s: float) -> None:
        """Schedule the next occurrence and let the orchestrator assess this run from current timing/failure facts."""
        from ferb.orchestrator.schedules import FINISHED, watch_met
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
            self._last_runs.add(sid)                 # changeable (cancel, pause) from now until its run ends (M9)
        if self.supervisor is None:
            self._last_runs.discard(sid)
            return
        try:
            final = await self.supervisor.run_scheduled(s, late=overdue_s > 60, overdue_s=overdue_s)
        except Exception as e:                              # a run's failure is reported with its reason, never lost
            self._journal().append("schedule.failed", {"schedule_id": sid, "why": f"{type(e).__name__}: {e}"})
            final = {"task_id": f"{sid}:{now:%Y%m%d%H%M}", "status": "failed",
                     "answer": f"“{s.request}” didn't work this time; I'll try again at the next run. Why: "
                               f"{reason(e, self.redact)}"}
        finally:
            self._last_runs.discard(sid)
        # Only a verified run counts as success. A done-but-unverified run (V4) doesn't clear earlier failures, and
        # isn't counted as one either: the next run's consecutive_failures stays a count of real failures.
        status = final.get("status")
        self.schedules.ran(sid, True if status == "done" else None if status == "done_unverified" else False)
        answer = str(final.get("answer") or final.get("summary") or "")
        # A watch (fix wave 6c): a met condition (its check judged the reported condition) is told like a reminder,
        # once per crossing: not when the latest earlier finished check that reported it found it met, an unconfirmed
        # one included (the owner was told then; stored with the schedule, re-review N1). A confirmed met run of a
        # one-shot watch ends it, as the owner's stored words for it say, and says so. A finished check with nothing
        # new to tell is recorded but reaches no client (review I3), and so does a check of a one-shot watch that
        # another check ended meanwhile (M1).
        told_before = s.last_reported_met is True            # as this run's planner was shown it
        reported = final.get("watch_met")                    # recorded only by a watch's check, which judged it
        if s.watch != "none" and status in FINISHED and reported is not None:
            self.schedules.reported(sid, bool(reported))
        met = s.watch != "none" and watch_met(status, reported)
        superseded = s.watch == "until_met" and self._watch_ended(sid)
        alert = met and not told_before and not superseded
        result = {**final, "goal": s.request}
        ended = (met and not superseded and s.watch == "until_met" and status == "done"
                 and self._end_watch(sid, str(final.get("task_id"))))
        if ended:                        # decided before `quiet`: the stop line always reaches the owner
            line = f"🗓 Stopped checking: {s.request}"
            answer = f"{answer}\n\n{line}"
            self._record_stop_line(str(final.get("task_id")), line)
        quiet = s.watch != "none" and not ended and (
            superseded or status in FINISHED and reported is not None and not alert)
        if overdue_s > 60:
            answer = f"(ran late) {answer}"
        await self._task_result(result | {"answer": answer}, shown=quiet, alert=alert, publish=not quiet)

    def _watch_ended(self, sid: str) -> bool:
        """A one-shot watch already ended by one of its checks (`schedule.met`)."""
        return bool(self._journal().of_kind("schedule.met", "schedule_id", sid))

    def _record_stop_line(self, task_id: str, line: str) -> None:
        """The stop line in the ending task's recorded outcome as well, so a client restoring the answer from the
        task's record shows it too (review M3)."""
        assert self.db is not None
        row = self.db.conn.execute("SELECT status, outcome_json FROM tasks WHERE id = ?", (task_id,)).fetchone()
        if row is None:
            return
        outcome, line = json.loads(row["outcome_json"] or "{}"), self._redact_text(line)
        outcome["answer"] = f"{outcome.get('answer', '')}\n\n{line}"
        self.tasks.set_status(task_id, row["status"], outcome)

    def _end_watch(self, sid: str, task_id: str) -> bool:
        """A one-shot watch's confirmed run found its condition met: it ends (done, its next check cleared, journaled
        `schedule.met`, its recorded scope withdrawn as a cancel withdraws it). The stop is the owner's own words stored
        for that schedule; nothing a run reads can create or widen one. A watch the owner paused or cancelled meanwhile
        is left as they set it."""
        s = self.schedules.get(sid)
        if s is None or s.state != "active":
            return False
        if s.timer_id:
            self.timers.cancel(s.timer_id)
        self.schedules.set_timer(sid, None, None)
        self.schedules.set_state(sid, "done")
        self._journal().append("schedule.met", {"schedule_id": sid, "task_id": task_id})   # before the withdrawal:
        self._withdraw_schedule_scope(sid, "met")             # a grant checks it from now on, as for a cancel (M1)
        return True

    # ---------- progressive setup ----------

    async def _schedules_list(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        return {"schedules": [{"id": s.id, "request": s.request, "rule": s.rule.describe(), "state": s.state,
                               "next_run": s.next_run.isoformat() if s.next_run else None, "failures": s.failures,
                               "watch": s.watch} for s in self._listed_schedules()]}

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
        if self.gateway is not None:
            self.gateway.shadow_action = self._destructive_shadow if jev is not None else None
        if self.distiller is not None:
            self.distiller.jev = jev
        if self.trail is not None and self.perception is not None:
            self.trail.tag = self.perception.notify
            self.perception.batches.wake.set()

    async def _destructive_shadow(self, facts: dict[str, Any], ref: str) -> None:
        from ferb.decide.decider import Ask
        if self.jev is None:
            return
        baseline = "destructive" if facts.get("kind") in {"pay", "delete_permanent", "account_security", "send", "post"} else "uncertain"
        try:
            await self.jev.ask(self.redact(facts), [Ask("action.destructive", baseline, ref)])
            metadata = {key: facts.get(key) for key in ('tool', 'kind', 'target', 'owner_words')}
            got = await self.jev.advice(self.redact(metadata), [Ask('action.alignment', 'uncertain', ref)])
            if got.get('action.alignment') == 'mismatch':
                self._journal().append('security.alignment', {'task_id': facts.get('task_id'), 'ref': ref,
                                                              'metadata': metadata, 'advice': 'mismatch'})
                self._publish('security.alignment', {'task_id': facts.get('task_id'), 'ref': ref,
                                                     'advice': 'mismatch; orchestrator should inspect the request and action'})
        except Exception:
            pass  # shadow advice never changes the concrete action path


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
        if QUESTIONS[q].live:
            raise FerbError(f"{q} is live by the plan of record (Q4); it has no switch to turn on or off")
        if QUESTIONS[q].shadow_only and params.get("on", True):
            raise FerbError(f"{q} is measured in shadow; it cannot act before owner-reviewed measurement")
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

    async def _environment(self, own_words: str) -> dict[str, Any]:
        """Installed apps and browser connection facts; the model interprets the owner's requested route. The
        database is read here, on the event loop; the Mac's and the browsers' files and the running apps are read in a
        thread (`_environment_files`), so a slow read never holds up another message's ack or a task's live line (L8)."""
        from ferb.policy import services as svc
        ferb = self._identity_profile()
        bindings = self.pairings.all() if self.pairings else []
        alignment = self.db.conn.execute("SELECT data_json FROM journal WHERE kind='security.alignment' "
            "AND json_extract(data_json, '$.task_id') IN (SELECT id FROM tasks WHERE status='running') ORDER BY seq"
            ).fetchall() if self.db is not None else []
        found, apps, running, macos, extension = await asyncio.to_thread(
            self._environment_files, {(b.browser, b.folder) for b in bindings})
        names = {(p.browser, p.folder): p.display_name for p in found} if self.pairings else {}
        paired = [(b.browser, b.folder, names.get((b.browser, b.folder), b.folder)) for b in bindings
                  if b.identity is Identity.OWNER and (b.browser, b.folder) != ferb]
        stable, live = svc.summary(svc.Environment(apps, paired), macos, running)
        profiles = [{'browser': p.browser, 'profile': p.folder, 'name': p.display_name, 'account_hint': p.email,
                     'connection': self._profile_state(p, extension.get((p.browser, p.folder))),
                     'identity': 'ferb' if (p.browser, p.folder) == ferb else 'owner'} for p in found]
        if alignment:
            live += "\nNon-enforcing alignment advice for active tasks: " + json.dumps([json.loads(r[0]) for r in alignment])
        stable += '\nBrowser/profile inventory (facts, not a selected route): ' + json.dumps(profiles)
        return {"stable": stable, "live": live}

    def _environment_files(self, paired: set[tuple[str, str]]) -> tuple[list[Any], list[tuple[str, str]], set[str], str,
                                                                        dict[tuple[str, str], bool]]:
        """The environment's reads that touch no database, run in a thread: browser profiles (Local State), installed
        apps (the device profile), running apps, the macOS version, and whether FERB's extension is in each profile not
        yet paired (its Preferences files)."""
        import platform
        from ferb.act.browser.profiles import has_extension
        from ferb.act.mac import apps as appsm
        found = self._profiles()
        try:
            running = {a.bundle_id for a in appsm.running()}
        except Exception:
            running = set()
        ids = frozenset(accepted_extension_ids(self.config))   # the pinned unpacked id and the store id (L25)
        extension = {(p.browser, p.folder): has_extension(p, ids, (self.browser_data_dirs or {}).get(p.browser))
                     for p in found if (p.browser, p.folder) not in paired}
        return found, self._installed_apps(), running, platform.mac_ver()[0] or "unknown", extension

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

    def _task_tabs(self, task_id: str) -> list[dict[str, Any]]:
        """The browser tabs a task holds now, by tab id, with the page each held when the task took it over (a checker's
        browser context; a loop round's step context, L31). A model reads them: redacted as every tool result is
        (re-review 2, I4)."""
        tabs = [{"tool_prefix": prefix, **tab}
                for prefix, drv in (("browser", self.router), ("ferb_browser", self.ferb_identity))
                if drv is not None for tab in drv.task_tabs(task_id)]
        return list(self.redact(tabs)) if tabs else []

    def _browser_context(self, task_id: str) -> str:
        parent = self._inspection_parent(task_id)
        if parent is None:
            return ""
        tabs = self._task_tabs(parent)
        return ("\n\nExisting task tabs available for inspection (reuse these aliases; do not reopen "
                "the page or guess tab ids). A tab is known by its id: one you navigated elsewhere is still the same "
                "tab, and taken_over_from is the page it held when the task took it over: "
                + json.dumps(tabs)) if tabs else ""

    def _relevant_context(self, words: str) -> tuple[str, str, str, list[str]]:
        """Installed actions and worker types (FERB's own) and today's matching activity, with its origin (S3):
        recorded trail rows carry the trail labels; FERB's own observed actions are FERB's."""
        from ferb.act.catalog import search_actions
        from ferb.core import origin
        from ferb.sanitize.untrusted import wrap_untrusted
        from ferb.sense.tools import trail_sources
        actions = search_actions(dict(self.registry.capabilities), words)   # a copy: this may run off the event loop
        activity = self.daily_activity.search(words)        # the turn's one search (L28): text and refs share it
        trail = {str(row['source']) for row in activity if row.get('source')}
        labels = {origin.from_read_source(k, i) for k, i in trail_sources(trail)} if trail else set()
        return ('\nRelevant installed actions: ' + json.dumps(actions) +
                '\nRelevant activity today (data): ' + wrap_untrusted(json.dumps(activity)) +
                '\nWorker types: ' + json.dumps(['default', *[n for n in self.registry.runtimes if self._runtime_available(n)]]) + '\n',
                origin.join({origin.FERB} | labels),
                json.dumps([row["data"] if "data" in row else row for row in activity], ensure_ascii=False),
                ["trail:" + str(row["source_id"]) for row in activity if row.get("source") and row.get("source_id")])

    def _profile_state(self, p: Any, installed: bool | None = None) -> str:
        """connected / paired (FERB opens it) / installed, not paired / no extension. `installed`: whether the
        extension is in the profile, when its files were already read (the environment reads them in a thread)."""
        from ferb.act.browser.profiles import has_extension
        b = self.pairings.for_profile(p.browser, p.folder) if self.pairings else None
        if b is not None:
            br = self.hub.bridges.get((p.browser, p.folder)) if self.hub else None
            return "connected" if br is not None and br.conn else "paired"
        if installed is None:
            data = (self.browser_data_dirs or {}).get(p.browser)
            installed = has_extension(p, frozenset(accepted_extension_ids(self.config)), data)
        return "installed" if installed else "missing"

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
                problems["owner_browsers"] = f"{shown} paired; {IDLE_PROFILE}"
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
        if "audio" in registered and not perms.get("screen", True):   # app sound is captured under Screen Recording
            problems["hearing"] = "FERB doesn't have the Screen Recording permission, which covers app sound — run " \
                                  "`ferb doctor`"
        return build(registered, problems, profiles)

    def _installed_apps(self) -> list[tuple[str, str]]:
        """(bundle id, name) of the Mac's apps, for matching the names the owner uses."""
        p = self._device_profile()
        return [(a.bundle_id, a.name) for a in p.apps] if p is not None else []

    def _running_apps(self) -> list[tuple[str, str, int]]:
        """(bundle id, name, pid) of the apps running now; none when they can't be read."""
        from ferb.act.mac import apps as appsm
        try:
            return [(a.bundle_id, a.name, a.pid) for a in appsm.running()]
        except Exception:
            return []

    def _device_profile(self) -> Any:
        from ferb.device.profile import ProfileStore
        did = self.config.get("device", "id")
        return ProfileStore(self.home).load(str(did)) if did else None

    def _redact_text(self, text: str) -> str:
        f = getattr(self.redact, "text", None)
        return str(f(text)) if f else text

    def _redact_id(self, text: str) -> str:
        """Text that is one of FERB's own ids (a card's task-id key): redacted by structure (`Redactor.ident`)."""
        f = getattr(self.redact, "ident", None)
        return str(f(text)) if f else self._redact_text(text)

    async def _ps_list(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        assert self.shells
        return {"processes": [p.__dict__ for p in self.shells.list()]}

    async def _ps_kill(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        assert self.shells
        return {"killed": await self.shells.kill_id(str(params.get("id", "")))}

    async def _ps_attach(self, session: Session, params: dict[str, Any]) -> AsyncIterator[dict[str, Any]]:
        """Owner-only stream. Connection lifetime owns input; disconnect releases it even after a CLI crash."""
        assert self.shells
        proc_id = str(params.get('id', ''))
        task = self.shells.owner_of(proc_id)
        if task is None:
            raise FerbError('That process is not available.')
        handle = self.shells.handle(proc_id, task)
        lease = self.shells.take_input(proc_id) if params.get('take') else ''
        offset = max(0, int(params.get('since', 0)))
        try:
            yield {'lease': lease, 'id': proc_id}
            while True:
                text, offset, _ = await self.shells.read(handle, offset)
                info = self.shells.info(handle)
                yield {'text': text, 'offset': offset, 'running': info.running, 'exit_code': info.exit_code}
                if not info.running:
                    return
                await asyncio.sleep(0.1)
        finally:
            self.shells.release_input(proc_id, lease)

    async def _ps_send(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        assert self.shells
        self.shells.owner_send(str(params.get('id', '')), str(params.get('lease', '')), str(params.get('text', '')))
        return {'sent': True}

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
        self.hub.on_unpaired = self._autopair_soon
        resolver = Resolver(self._profiles, self.pairings.all, self._identity_profile)
        await self.wake.start()
        self.router = Router(self.hub, resolver, self._profiles, self.wake, self.browser_launch)
        self.router.native_secrets = self._native_page_secrets      # B6: an unread tab's fields from Accessibility
        self.router.native_capture = self._native_window_capture    # I1: a tab the debugger can't attach to
        self.router.native_place = self._native_place                # L33: where an element is, for FERB's cursor
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
        self.hub.on_tab_closed = self._tab_closed
        # Rule 17: a file given to a browser to upload is FERB's own copy, never one in a folder macOS protects.
        self.uploads = uploads.Uploads(self.home / "workspace" / uploads.FOLDER,
                                       lambda: self._gateway().roots.untrusted_roots)
        try:
            await asyncio.to_thread(self.uploads.sweep)  # copies a restart left behind belong to no task now
        except OSError as e:
            self._journal().append("browser.uploads_startup_failed", {"error": f"{type(e).__name__}: {e}",
                                                                      "left": list(getattr(e, "left", []))})
        from ferb.identity.helper import seal as helper_seal
        self.router.seal_value = lambda key, value, command, origin: helper_seal(self.home, key, value, command, origin)
        self.router.ask_code = lambda ctx, origin: self.gateway.ask_login_code(ctx, origin)
        self.router.ask_secret = lambda ctx, target, **kw: self.gateway.ask_secret(ctx, target, **kw)
        self.router.release_secret = lambda value: self.gateway.release_secret(value)
        if self.mac is not None:
            self.mac.ask_secret = self.router.ask_secret
            self.mac.release_secret = lambda value: self.gateway.release_secret(value)
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
            drv.uploads = self.uploads
            drv.task_running = lambda task_id: self.tasks.get(task_id) is not None   # removed when its tree ends
            drv.task_root = self._task_root
            drv.max_tabs = lambda: int(self.config.get("browser", "task_max_tabs", 50))
            drv.redact_text = lambda text: str(self.redact({"u": text})["u"])
        self.downloads.task_tab = self.hub.task_tab      # only a live task's tab holds downloads (D35)
        # Browser/profile selection is an agent decision, not a phrase-derived permission gate.
        self._attach_identity_if_paired()
        self.server.native = self.hub.serve
        # the pinned unpacked id and, once configured, the Chrome Web Store id (L25)
        self.server.extension_ids = lambda: frozenset(accepted_extension_ids(self.config))

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
        self.pairings.revoke(token)                          # its page went unanswered: the key can't pair later
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
        ids = accepted_extension_ids(self.config)          # the copy installed in that profile: unpacked or store
        p = next((p for p in known if (p.browser, p.folder) == (browser, folder)), None)
        found = browser_profiles.extension_entry(p, ids, (self.browser_data_dirs or {}).get(browser)) if p else None
        ext = found[0] if found else ids[0]
        browser_profiles.open_known_profile(browser, folder, f"chrome-extension://{ext}/pair.html#token={token}",
                                            known, background=False, **kwargs)   # the owner must see this page
        return wake

    def _on_app_focus(self, focused: Any) -> None:
        """The owner switched apps: to a browser while an unpaired FERB copy is connected, FERB can pair it now."""
        if self.hub is not None and self.hub.unpaired and any(
                browser_profiles.BROWSERS[b].bundle_id == focused.bundle_id for b in AUTOPAIR_BROWSERS):
            self._autopair_soon()

    def _autopair_soon(self) -> None:
        """L25 automatic pairing (owner 2026-10-24): one pass at a time; a trigger during a pass runs one more."""
        if not self.run_autopair or self._shutting_down:
            return
        if self._autopair_task is not None and not self._autopair_task.done():
            self._autopair_again = True
            return
        self._autopair_again = False
        self._autopair_task = asyncio.get_running_loop().create_task(self._autopair())

    async def _autopair(self) -> None:
        while True:
            try:
                await self._autopair_pass()
            except Exception as e:                         # FERB goes on; `ferb browsers pair` still works
                self._journal().append("browsers.autopair_failed", {
                    "why": "the automatic pairing pass failed",
                    "error": self._redact_text(f"{type(e).__name__}: {e}")})
            if not self._autopair_again:
                return
            self._autopair_again = False

    def _autopair_wanted(self) -> bool:
        """An unpaired FERB copy is connected and FERB isn't paused (`ferb pause` reads nothing of the desktop)."""
        return (self.hub is not None and self.pairings is not None and bool(self.hub.unpaired)
                and not (self.sense is not None and self.sense.paused))

    def _autopair_front(self) -> tuple[str, str] | None:
        """The browser in front and the profile it last recorded as active (Local State), for the browsers whose
        profile delivery is verified (AUTOPAIR_BROWSERS). Files and Accessibility only (run in a thread)."""
        front = self.front_app()
        browser = next((b for b in AUTOPAIR_BROWSERS if browser_profiles.BROWSERS[b].bundle_id == front), None)
        if browser is None:
            return None
        folder = browser_profiles.last_used(browser, (self.browser_data_dirs or {}).get(browser))
        return (browser, folder) if folder else None

    def _autopair_open(self, browser: str, folder: str) -> bool:
        """Database and memory facts (on the loop): not FERB's identity profile, not tried in this run, not paired,
        not unpaired by the owner (`ferb browsers unpair` sticks), and no pairing of it already in progress."""
        assert self.pairings is not None
        return ((browser, folder) != self._identity_profile() and (browser, folder) not in self._autopair_tried
                and self.pairings.for_profile(browser, folder) is None
                and not self.pairings.autopair_off(browser, folder) and not self.pairings.pending(browser, folder))

    def _autopair_target(self, browser: str, folder: str) -> tuple[str, list[browser_profiles.BrowserProfile]] | None:
        """The FERB copy turned on in that profile, unless the profile holds FERB's own account (its mailbox address:
        setup pairs it as FERB, even before `ferb setup --identity` records it). Files only (run in a thread)."""
        known = self._profiles()
        p = next((p for p in known if (p.browser, p.folder) == (browser, folder)), None)
        if p is None:
            return None
        address = str(self.config.get("mailbox", "address", "") or "").strip().lower()
        if address and p.email == address:
            return None
        data = (self.browser_data_dirs or {}).get(browser)
        found = browser_profiles.extension_entry(p, accepted_extension_ids(self.config), data)
        if found is None or not browser_profiles.extension_enabled(found[1]):
            return None
        return found[0], known

    async def _autopair_pass(self) -> None:
        """An unpaired FERB copy is connected: pair the profile the owner is in by its self-closing pairing page
        (pair.js), with no command. Chrome records a window switch, and FERB being turned on, in Local State and
        Preferences up to PREFS_SETTLE_S late, so the profile counts only once two readings that far apart agree with
        the browser in front each time; the front app is read again just before the page is handed over, so it goes
        to the window in front. One try per profile while FERB runs, journaled either way; its key is revoked when the
        wait ends."""
        if not self._autopair_wanted():
            return
        assert self.pairings is not None
        end = self.clock.monotonic() + self.pair_timeout_s
        seen = await asyncio.to_thread(self._autopair_front)
        while True:
            if seen is None or not self._autopair_open(*seen):
                return
            await self.clock.sleep_until(self.clock.now() + timedelta(seconds=PREFS_SETTLE_S))
            if not self._autopair_wanted():
                return
            now = await asyncio.to_thread(self._autopair_front)
            if now == seen:
                break
            if self.clock.monotonic() >= end:               # the owner kept switching: next trigger
                return
            seen = now
        browser, folder = seen
        if not self._autopair_open(browser, folder):
            return
        target = await asyncio.to_thread(self._autopair_target, browser, folder)
        if target is None or await asyncio.to_thread(self.front_app) != browser_profiles.BROWSERS[browser].bundle_id:
            return
        ext, known = target
        self._autopair_tried.add((browser, folder))
        token = self.pairings.issue_token(browser, folder)
        kwargs: dict[str, Any] = {"launch": self.browser_launch} if self.browser_launch else {}
        browser_profiles.open_known_profile(browser, folder, f"chrome-extension://{ext}/pair.html#token={token}",
                                            known, background=False, **kwargs)   # its window is the one in front
        end = self.clock.monotonic() + self.pair_timeout_s
        try:
            while self.clock.monotonic() < end and self.pairings.for_profile(browser, folder) is None:
                await asyncio.sleep(0.2)
        finally:
            self.pairings.revoke(token)
        if token in self.pairings.paired:
            self._journal().append("browsers.autopaired", {"browser": browser, "folder": folder})
        elif self.pairings.for_profile(browser, folder) is None:
            self._journal().append("browsers.autopair_failed", {
                "browser": browser, "folder": folder,
                "why": "the pairing page's key wasn't used; `ferb browsers pair` pairs it by hand"})

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
        self.pairings.revoke(token)                          # its page went unanswered: the key can't pair later
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
        for sc in (self._listed_schedules() if self.schedules is not None else []):
            state = "done; its last run is under way" if sc.state == "done" else sc.state
            lines.append(f"- Schedule {sc.id} ({state}): {sc.request} — {sc.describe()}")
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
                lines.append(f"- The owner's {b.browser} profile “{name}”: paired{self._paired_state(b, br)}")
            for browser in sorted(set(self.hub.unpaired.values())):
                note = self._unpaired_note(browser)
                if note:
                    lines.append(f"- {note}")
        return "\n".join(lines)

    def _paired_state(self, binding: Any, bridge: Any) -> str:
        """How the planner may use one paired owner profile now, in plain words: a profile that isn't connected is
        usually just not running, and the browser tools start it without a window when the wake service is there
        (`_TabDriver._connected`, spike 19); its first page needs FERB's minimized window, whose creation may switch
        Space (background.js `getFerbWindow`). Live 2026-10-24: "extension transport disconnected" read as broken."""
        if bridge is not None and bridge.conn:
            return ", connected"
        refused = self.hub.refused.get(binding.install_id) if self.hub is not None else None
        if refused:
            return f"; its extension was refused: {refused}"
        if self.router is not None and self.router.wake is not None:
            return ("; not connected now (usually the profile just isn't running) — FERB's browser tools start it in "
                    "the background, with no window; its first page needs FERB's minimized window, whose creation "
                    "may switch Space")
        return f"; {IDLE_PROFILE}"

    def _publish(self, topic: str, data: dict[str, Any]) -> None:
        if topic == "approval.requested" and data.get("task_id") and "request_id" not in data:
            asked = self.state.approvals.get(str(data.get("approval_id", "")))
            if asked is not None and asked.request_id:      # a task's question names its request (event contract)
                data = {**data, "request_id": asked.request_id}
        if self.supervisor:
            self.supervisor.notify_plan_event(topic, self.redact(data))
        if topic in ('trail.event', 'action.observed'):
            self.daily_activity.record(self.redact(dict(data, what=data.get('what', data.get('source', topic)),
                                                        where=data.get('where', data.get('data', {})),
                                                        device_id=self.config.get('device', 'id', 'unknown-device'))))
        if topic == 'trail.event' and self.supervisor is not None:
            self.supervisor.conversations.observe(self.redact(data))
        if topic == 'perception.attention' and not self._shutting_down:
            self._attention_arrived(self.redact(data))
        if topic == "task.finished" and self._rescrubs:
            self._tree_finished_at_a_forget(str(data.get("task_id", "")))
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
        shim = mcp_shim_command()
        if name == "codex":
            from ferb.device.install import repo_root
            from ferb.core.errors import Unsupported
            from ferb.runtimes.codex_exec import MODEL, CodexRuntime, codex_binary, codex_home
            if codex_home(self.home) is None:
                raise Unsupported("Codex isn't signed in for FERB: choose Sign in to Codex… in FERB's menu bar, "
                                  "or run `ferb setup --codex-login`")
            workdir = self.home / "workspace" if mode == "worker" else self.home / "run" / "supervisor"
            return CodexRuntime(codex_binary(self.home, repo_root()), mode, workdir, self.home,  # type: ignore[arg-type]
                                shim_cmd=shim, tool_names=tool_names, output_schema=schema, model=model or MODEL,
                                timeout_s=None if mode == 'worker' else 900, on_abandoned=self.server.abandon,
                                redact=self.redact)
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
        text = str(params.get("text") or "")
        return self._resolve_approval(str(params["approval_id"]), bool(text), None, text or None)

    async def _test_mode(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        if self.gateway is None:
            raise FerbError("the tool gateway is not ready")
        if params.get("status") is True:
            return {"enabled": self.gateway.test_send_allowlist is not None,
                    "allow_send": sorted(self.gateway.test_send_allowlist or ())}
        allowed = params.get("allow_send", [])
        if not isinstance(allowed, list) or not all(isinstance(x, str) and x.strip() for x in allowed):
            raise FerbError('allow_send must be a list of exact recipients or "SITE @ACCOUNT" entries for posts')
        from ferb.daemon.gateway import allowlist_problem
        problems = [p for p in (allowlist_problem(x) for x in allowed) if p]
        if problems:
            raise FerbError("; ".join(problems))
        next_allowlist = None if params.get("off") is True else set(allowed)
        record = {"enabled": next_allowlist is not None, "allow_send": sorted(next_allowlist or ())}
        self.config.set('test_mode', 'state', record)  # one atomic config write before the running state changes
        self.gateway.test_send_allowlist = next_allowlist
        self._journal().append("test.mode", record)
        return record

    def _resolve_approval(self, approval_id: str, granted: bool, choice: int | None = None,
                          text: str | None = None) -> dict[str, Any]:
        """The owner's answer to a question, typed (`ferb chat`) or spoken (push-to-talk)."""
        ap = self.state.approvals.get(approval_id)
        retained = None
        if text is not None and granted:
            if ap is not None and ap.kind in {"owner_admin_password", "owner_secret"}:
                if any(c in text for c in "\r\n\x00"):
                    raise FerbError("the hidden password must be one line")
                if not isinstance(self.redact, Redactor):
                    raise FerbError("hidden input redaction is unavailable")
                self.redact.retain_secret(text)
                retained = text
            else:
                text = text.strip()
                granted = bool(text)
                from ferb.orchestrator.steering import MEMORY_ASKS, OPTION_ONLY, choices
                if ap is not None and ap.capability in OPTION_ONLY and text and ap.option_for(text) is None:
                    raise FerbError(f"Pick {choices(ap.options)}.")
                # Only the sealed secret inputs (a secret, the admin password, a login code) make hidden secrets
                # (L18, D28). A picked option is never one; other typed words to a question are hidden for this
                # answer alone when the question asks for a one-time code or a sign-in password or PIN
                # (`Redactor.answer`), and never remembered.
                if (ap is not None and ap.kind == "owner_question" and isinstance(self.redact, Redactor)
                        and ap.option_for(text) is None):
                    # FERB's memory questions never ask for a secret: only secret classes are hidden (L36)
                    text = (self.redact.text(text) if ap.capability in MEMORY_ASKS
                            else self.redact.answer(text, question=ap.reason))
                if ap is not None and ap.capability == "memory.edit" and _all_hidden(text):
                    raise FerbError(EDIT_HIDDEN)
                if ap is not None and ap.kind == "owner_login_code" and text:
                    self._add_secret_value(text)
        try:
            ap = self.state.approvals.resolve(approval_id, granted, choice, text)
            followed = self._question_answered(ap)          # a chat forget answered with the notch's buttons (I2)
            if followed and ap.capability == "memory.forget":
                self._publish("memory.forgot", {"approval_id": ap.id, "text": followed})
            if ap.capability == 'setup.persona' and self.persona is not None:
                try:
                    self.persona.resolve(ap)
                except Exception:
                    ap.state, ap.choice, ap.answer = 'pending', None, None
                    raise
        except BaseException:
            if retained is not None and isinstance(self.redact, Redactor):
                self.redact.release_secret(retained)
            raise
        self._journal().append("approval.resolved", {"approval_id": ap.id, "state": ap.state,
                                                     "capability": ap.capability})
        self._publish("approval.resolved", {"approval_id": ap.id, "state": ap.state})
        return {"approval_id": ap.id, "state": ap.state}

    def _chat_ack(self, session_id: str, request_id: str, message: str = "",
                  received: datetime | None = None) -> dict[str, Any]:
        """TQ3/P25: a neutral phrase said on receipt, never one of the last eight said; journaled into chat history,
        with when the message arrived (`ferb baseline` times the ack, plan line and answer from it, L8). `message`:
        the owner's message as the daemon redacted it, as `chat.message` records it, for clients to keep (L7)."""
        said = [str(row[0]) for row in self._journal().db.conn.execute(
            "SELECT json_extract(data_json, '$.text') FROM journal WHERE kind = 'chat.ack' ORDER BY seq DESC LIMIT ?",
            (acks.RECENT,))]
        text = acks.pick(said[::-1])
        entry = self._journal().append("chat.ack", {"session_id": session_id, "request_id": request_id, "text": text,
                                                    "received_at": (received or self.clock.now()).isoformat()})
        return {"type": "ack", "request_id": request_id, "text": text, "message": message.strip(),
                "at": entry.at.isoformat()}

    async def _chat_send(self, session: Session, params: dict[str, Any]) -> AsyncIterator[dict[str, Any]]:
        """One chat turn: an acknowledgement on receipt, then the supervisor's events, merged with bus events about
        the tasks this turn started (tool calls, owner questions and action observations) so `ferb chat` can show
        them inline. Every event names this message's request_id and when it was sent (L7)."""
        received = self.clock.now()
        if self._shutting_down:
            raise FerbError("daemon is shutting down; no new chat task was started")
        if self.supervisor is None:
            raise FerbError("daemon not started")
        paths = params.get('input_paths', [])
        if not isinstance(paths, list) or not all(isinstance(p, str) and p.strip() and '\x00' not in p for p in paths):
            raise FerbError('input_paths must be a list of nonempty filesystem paths')
        input_paths = list(dict.fromkeys(normalize_path(p, str(self.home / 'workspace')) for p in paths))
        if self.redact(input_paths) != input_paths:
            raise FerbError('input_paths contains protected secret text; rename the attachment before sharing its path')
        session_id = str(params.get("session_id", "default"))
        text = self._hide_code_reply(self.redact(str(params.get("text", ""))), session_id)
        request_id = self.supervisor.ids.new_id()
        queue: asyncio.Queue[dict[str, Any] | None] = asyncio.Queue()
        # Before the per-session routing lock: a message is acknowledged while an earlier one is still planning (P29)
        queue.put_nowait(self._chat_ack(session_id, request_id, text, received))
        # L12: what the owner has in front of them as they send, captured beside routing; the planner waits for it
        screen = (asyncio.ensure_future(self.front_window(request_id, session.pid if session is not None else None))
                  if self.front_window is not None else None)
        my_tasks: set[str] = set()
        attached = {"on": True}                     # False once the owner closes chat: the task keeps running
        sub = self.bus.subscribe({"tool.called", "approval.requested", "action.*"})

        def stamped(ev: dict[str, Any]) -> dict[str, Any]:
            return {**ev, "request_id": request_id, "at": self.clock.now().isoformat()}

        async def turn() -> None:
            try:
                async for ev in self.supervisor.send(session_id, text,  # type: ignore[union-attr]
                                                     input_paths=input_paths, request_id=request_id, screen=screen):
                    ev = stamped(ev)
                    if ev.get("task_id"):
                        my_tasks.add(ev["task_id"])
                        if attached["on"]:
                            self._attached_trees.add(ev["task_id"])
                    if ev.get("type") == "outcome":
                        await self._task_result(ev, shown=attached["on"])
                        self._attached_trees.discard(ev["task_id"])
                    await queue.put(ev)
            except FerbError as e:
                await queue.put(stamped({"type": "error", "text": str(e)}))
            except Exception as e:      # logged whole; the owner reads its reason (owner 2026-10-24), never "Sorry"
                self._journal().append("chat.failed", {"request_id": request_id,
                                                       "why": f"{type(e).__name__}: {self.redact(str(e))}"})
                # no task of this turn had started (none named in its events: a task's plan line comes before its
                # work), or one may have been under way
                said = couldnt("finish that" if my_tasks else "start that", e, self.redact)
                await queue.put(stamped({"type": "reply", "text": self.redact(said)}))
            finally:
                if screen is not None and not screen.done():   # the turn ended before routing used it
                    screen.cancel()
                await queue.put(None)

        async def bus_events() -> None:
            async for ev in sub:
                tid = ev.data.get("task_id")
                ours = tid in my_tasks or self.tasks.tree_of(str(tid)) in my_tasks
                if ours or (ev.topic.startswith("action.") and not tid):
                    wrapper = stamped({"type": "event", "topic": ev.topic, "data": self.redact(ev.data)})
                    # an observation outside any task goes to every open chat: it is about no message
                    await queue.put(wrapper if ours else {**wrapper, "request_id": None})

        # The turn takes the routing lock in its first step, ahead of any message that arrives after this ack is sent
        t1, t2 = asyncio.create_task(turn()), asyncio.create_task(bus_events())
        self._turns.add(t1)
        t1.add_done_callback(self._turns.discard)
        self._turn_requests[t1] = request_id                 # what a forget meanwhile finds running (fix wave 6a)
        t1.add_done_callback(lambda done: self._turn_requests.pop(done, None))
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
        if not outcome.get('result'):
            row = self.db.conn.execute('SELECT outcome_json FROM tasks WHERE id=?', (task_id,)).fetchone()
            if row is not None and row[0] != '{}':
                outcome['result'] = json.loads(row[0])
        episode = episodes.get(self.db, task_id)
        # running, or written before F3: compose it live; a learning model call is no task and has no story
        if episode is None and spans and not all(s["kind"] == "model.learning" for s in spans):
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
                back = await recovery.restore_async(row["id"], kept.db, kept.root,
                                                    self._gateway()._path_closed("recovery.restore"))
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
        if self.learning is not None:
            self.learning.inputs_changed()            # a card read before this verdict is read again (L28)
            try:
                await self._capture_card(task_id, None, report=True)
            except Exception as e:                    # the verdict stands; the next capture refreshes the card
                kind = "memory.episode_deferred" if isinstance(e, Deferred) else "memory.episode_failed"
                self._journal().append(kind, {"task_id": task_id, "error": self._redact_text(str(e))})
            self._refresh_learning_verdict(task_id)
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
        ok, reason = await asyncio.to_thread(self.sandbox.available) if self.sandbox else (False, "not built")
        # The proxy's liveness check (expect 407): it also restarts a dead serving thread, and says so; a down proxy
        # means the shell has no network and fails closed (F5a Task 3 fix rounds 1-2)
        probe = await asyncio.to_thread(self.proxy.probe) if self.proxy is not None else {}
        out["containment"] = {"sandbox_exec": os.path.exists(SANDBOX_EXEC), "profile_ok": ok, "reason": reason,
                              "shell": "shell.run" in self.registry.capabilities,
                              "shell_reason": self.containment[1],
                              "proxy": self.proxy is not None, "proxy_alive": bool(probe.get("alive")),
                              "proxy_restarted": bool(probe.get("restarted")),
                              "proxy_last_error": probe.get("last_error"),
                              "local_reports": self._local_reports_status(),
                              "snapshots": bool(self.snapshots is not None and self.snapshots.take is not None),
                              # OPEN QUESTION 3: shell commands run without a snapshot when one can't be taken
                              "snapshot_failure": self.snapshots.last_failure() if self.snapshots else None}
        out["hearing"] = await self._hearing_status(bool(params.get("hearing", params.get("selftest", True))))
        out["clipboard"] = await asyncio.to_thread(self._clipboard_access)     # I3: never reads the contents
        out["voice"] = {"push_to_talk": self._ptt, "installed": self.voice_bundle.exists(),
                        "connected": self.voice is not None and self.voice.app is not None,
                        "permissions": self.voice.permissions if self.voice is not None else {}}
        from ferb.core.context import token_method
        out["tokenizer"] = token_method()                 # FERB's pinned o200k copy, or the byte ceiling (8a)
        out.update({"pid": os.getpid(), "executable": os.path.realpath(sys.executable),
                    "version": ferb.__version__, "code_hash": code_hash(Path(ferb.__file__).parent),
                    "home": str(self.home), "stopped": self.state.stopped})
        return out

    @staticmethod
    def _clipboard_access() -> str | None:
        """How macOS treats this daemon reading the clipboard (I3): accessBehavior, read without the contents."""
        try:
            from ferb.sense.watchers.clipboard import access, general_board
            return access(general_board())
        except Exception:
            return None

    async def _clipboard_request(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        """`ferb setup` only, with the owner's yes (I3): one read by this daemon, so macOS shows its paste alert now
        and lists FERB in Paste from Other Apps. Nothing read is kept; journaled."""
        from ferb.sense.watchers.clipboard import request_access
        state = await asyncio.to_thread(request_access)
        self._journal().append("clipboard.access", {"access": state, "requested": True})
        return {"access": state}

    async def _dump_codex_catalog(self) -> None:
        """L28, final review M8: when a role runs on Codex, the catalog shipped with the pinned binary is dumped (once
        per Codex version; `codex debug models --bundled` is a process) in a worker thread before the daemon serves,
        so no limits read on the event loop ever runs it. A failure leaves the window unknown, which is said."""
        from ferb.runtimes.catalog import ROLES
        from ferb.runtimes.codex_exec import bundled_catalog
        if any(self._runtime_for(role) == "codex" for role in ROLES):
            await asyncio.to_thread(bundled_catalog, self.home)

    def _runtime_limits(self) -> dict[str, dict[str, Any]]:
        """Each role's runtime and model with its documented context window, reply limit and per-input character cap
        (L28), or the problem."""
        from ferb.core.errors import ModelLimitsUnknown
        from ferb.runtimes.catalog import CATALOG, ROLES
        out: dict[str, dict[str, Any]] = {}
        for role in ROLES:
            name = self._runtime_for(role)
            model = self._model_for(name)
            entry: dict[str, Any] = {"runtime": name, "model": model or (CATALOG[name].default_model if name in CATALOG else "")}
            try:
                if name not in CATALOG:
                    raise ModelLimitsUnknown(f"unknown runtime {name!r}")
                limits = CATALOG[name].limits(model, self.home)
                entry.update(model=limits.model, context_window=limits.context_window, max_output=limits.max_output,
                             context_budget=limits.context_budget, max_input_chars=limits.max_input_chars,
                             source=limits.source)
            except ModelLimitsUnknown as exc:
                entry["problem"] = str(exc)
            out[role] = entry
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
        status["limits"] = self._runtime_limits()
        return status

    async def _automation_request(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        """Ask macOS for Apple Events permission to each app (shows the system prompt; must run in the daemon so
        the grant attaches to FERB's interpreter). Apps that aren't running are asked on first use instead."""
        from ferb.device import permissions
        bundle_ids = [str(b) for b in params.get("bundle_ids", [])]
        results = {b: (await permissions.automation_async(b, ask=True)).value for b in bundle_ids}
        self._journal().append("automation.requested", results)
        return {"results": results}

    async def _logs(self, session: Session, params: dict[str, Any]) -> AsyncIterator[dict[str, Any]]:
        wanted = {LOG_FILTERS[f] for f in params.get("filters", []) if f in LOG_FILTERS} or None
        sub = self.bus.subscribe(wanted, maxsize=int(params.get("maxsize", 1000)))
        self._log_subscriptions.add(sub)
        try:
            async for ev in sub:
                yield {"topic": ev.topic, "at": ev.at.isoformat(),
                       "class": "notice" if ev.topic == DROPPED else ev.cls.value, "data": self.redact(ev.data)}
        except Overflow:
            yield {"topic": "bus.overflow", "at": self.clock.now().isoformat(), "class": "notice",
                   "data": {"message": "fell behind on critical events; restart `ferb logs`"}}
        finally:
            self._log_subscriptions.discard(sub)
            sub.close()


# `osascript -e 'display notification …'` posts under Script Editor's bundle id (Notification Center's app identifier).
OSASCRIPT_POSTER = "com.apple.ScriptEditor2"


async def _no_notify(title: str, text: str) -> None:
    return None


def main() -> None:
    asyncio.run(Daemon().run())
