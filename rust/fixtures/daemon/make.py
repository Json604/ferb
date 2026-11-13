"""Records the Python daemon as the Rust daemon must reproduce it (ENG-3): every RPC method's frames, and end-to-end
chat scenarios with a scripted model (frames, prompts, tool calls, store rows). README.md beside this file is the map.

Run from the repo root:  uv run python rust/fixtures/daemon/make.py

Each scenario starts a real `ferb.daemon.main.Daemon` (real server, gateway, stores and supervisor) on its own event
loop thread in a fresh temporary home, built as tests/conftest.py's `RunningDaemon` builds it, and talks to it over
its Unix socket with raw newline-JSON lines, recording every line both ways. Nothing outside is reached: FERB_HOME and
HOME point into a temporary folder; time is a FakeClock (2026-01-01T09:00:00Z, moved only by the script), ids are
SequentialIds, run tokens and the untrusted-data wrapper's id are counters; model calls go to a scripted runtime that
records what it was sent; every Mac capability (Keychain, TCC, clipboard, screenshots, browsers, subprocesses that
reach the Mac) is replaced by a stand-in listed in STUBS below. All data is invented.
"""

import asyncio
import contextlib
import hashlib
import json
import os
import re
import shutil
import socket
import sys
import tempfile
import threading
import time
from pathlib import Path
from types import SimpleNamespace
from typing import Any, Callable

REPO = Path(__file__).resolve().parents[3]
HERE = Path(__file__).resolve().parent
# A fixed folder (the same on every run on this machine): FERB hashes real paths into some records (a confirmation's
# key, a ledger payload hash, learning ids), so a random folder would change them run to run. Short: socket paths are
# limited to 104 bytes. Emptied first; removed at the end.
ROOT = Path(tempfile.gettempdir()) / "ferbdx"
shutil.rmtree(ROOT, ignore_errors=True)
ROOT.mkdir()
os.environ["FERB_HOME"] = str(ROOT / "home")
os.environ["HOME"] = str(ROOT / "user")
os.environ["TZ"] = "UTC"
time.tzset()
(ROOT / "user").mkdir()
(ROOT / "tmp").mkdir()
tempfile.tempdir = str(ROOT / "tmp")
for _k in [k for k in os.environ if k.startswith("GIT_")]:
    del os.environ[_k]
sys.path.insert(0, str(REPO))

import jsonschema  # noqa: E402
import pytest  # noqa: E402

MP = pytest.MonkeyPatch()

# ---------------------------------------------------------------------------------------------------------------
# STUBS: what tests/conftest.py's autouse fixtures install, plus what reaches the Mac from an RPC. Each line names
# what it keeps away. Applied before any daemon is built.
# ---------------------------------------------------------------------------------------------------------------

import ferb.act.browser.ext_bridge as ext_bridge  # noqa: E402
import ferb.act.browser.profiles as profiles  # noqa: E402
import ferb.cli.tokenizer as tokenizer  # noqa: E402
import ferb.core.context as core_context  # noqa: E402
import ferb.daemon.auth as auth  # noqa: E402
import ferb.daemon.reports as reports  # noqa: E402
import ferb.runtimes.codex_exec as codex_exec  # noqa: E402
import ferb.sanitize.images as images  # noqa: E402
import ferb.sanitize.untrusted as untrusted  # noqa: E402
import ferb.sense.network as network  # noqa: E402
from ferb.runtimes.claude_sdk import ClaudeRuntime  # noqa: E402
from tests.support.trash import no_real_trash  # noqa: E402

STUBS: list[str] = []


def stub(target: Any, name: str, value: Any, why: str) -> None:
    MP.setattr(target, name, value)
    where = target if isinstance(target, str) else getattr(target, "__name__", type(target).__name__)
    STUBS.append(f"{where}.{name}: {why}")


async def _refuse_runtime(*a: Any, **k: Any) -> None:
    raise AssertionError("a real model runtime was started")


async def _no_codex(ferb_home: Any, binary: Any) -> None:
    return None


class Counter:
    def __init__(self, fmt: str) -> None:
        self.fmt, self.n = fmt, 0

    def __call__(self, *_: Any) -> str:
        self.n += 1
        return self.fmt.format(self.n)


TOKENS = Counter("run-token-{:04d}")


class Hex:
    """Stands for the `secrets` module where FERB names a record with random hex (an approval's id, a timer's token):
    a counter per module, reset for every scenario, so ids are the same on every run."""
    every: list["Hex"] = []

    def __init__(self, prefix: str) -> None:
        self.prefix, self.n = prefix, 0
        Hex.every.append(self)

    def token_hex(self, nbytes: int = 16) -> str:
        self.n += 1
        return f"{self.prefix}{self.n:0{2 * nbytes - len(self.prefix)}x}"

    def token_urlsafe(self, nbytes: int = 32) -> str:
        return self.token_hex(nbytes)
GRANTS = {"accessibility": "granted", "screen_recording": "granted", "input_monitoring": "granted",
          "full_disk_access": "denied"}
RUNNING_APPS = [("com.apple.finder", "Finder", 4101, True), ("com.apple.TextEdit", "TextEdit", 4102, False)]


class FakeDesktop:
    """Stands for ferb.daemon.disturbance.DesktopProbe: the same desktop facts on every sample."""
    space_epoch = 0

    def start(self) -> None:
        return None

    def pump(self) -> None:
        return None

    def close(self) -> None:
        return None

    def __call__(self) -> dict[str, Any]:
        return {"unavailable": {}, "active_space_epoch": 0,
                "frontmost_app": {"pid": 4101, "bundle_id": "com.apple.finder", "name": "Finder"},
                "key_window": {"pid": 4101, "ax_identity": 7}, "fullscreen": False}
FIXED_WRAP_ID = "0123456789ab"


def install_stubs() -> None:
    stub(profiles, "locate", lambda bundle_id: None, "Launch Services knows no browser (conftest no_launch_services)")
    stub(profiles, "_located", {}, "no cached browser lookup")
    stub(profiles, "has_app", lambda bundle_id: True, "every known browser counts as installed")
    stub(profiles, "handlers", lambda: [], "no app opens web links")
    stub(network, "has_route", lambda: True, "the Mac has a network route (conftest network_route)")
    stub(reports, "RELAY_URL", "", "no failure report reaches the relay (conftest no_report_relay)")
    stub(ext_bridge, "peer_refusal", lambda app, browser=None: None, "native-host peer check (conftest)")
    stub(images, "recognize_text", lambda _image: [], "no Vision OCR on invented pictures")
    stub(images, "warm_recognition", lambda: None, "no Vision warm-up at start")
    stub(codex_exec, "bundled_catalog", lambda ferb_home: None, "never runs the codex program")
    stub(codex_exec, "prepare", _no_codex, "nor `codex --version`")
    stub(codex_exec.CodexRuntime, "start", _refuse_runtime, "no real Codex run (conftest no_real_models)")
    stub(ClaudeRuntime, "start", _refuse_runtime, "no real Claude run (conftest no_real_models)")
    stub(tokenizer, "_fetch", lambda url: (_ for _ in ()).throw(OSError("offline")), "no tokenizer download")
    stub(core_context, "tokenizer_sources", lambda: [], "token counts use the byte ceiling (conftest no_machine_tokenizer)")
    core_context._encoding.cache_clear()
    no_real_trash(MP)
    STUBS.append("ferb.act.fs.files._trash_url: refuses (conftest offline_trash)")
    stub(untrusted, "secrets", SimpleNamespace(token_hex=lambda n: FIXED_WRAP_ID),
         "the untrusted wrapper's id is fixed (as rust/fixtures/catalog)")
    stub(auth, "secrets", SimpleNamespace(token_urlsafe=lambda n: TOKENS()), "run tokens: a counter")
    import ferb.act.shell.manager as shell_manager
    import ferb.clock.timers as clock_timers
    import ferb.daemon.questions as questions
    stub(questions, "secrets", Hex("a"), "approval ids: ap- and a counter")
    stub(clock_timers, "secrets", Hex("t"), "timer tokens: a counter")
    stub(shell_manager, "secrets", Hex("p"), "shell process ids: a counter")
    import ferb.orchestrator.continuity as continuity
    stub(continuity, "secrets", Hex("c"), "a new chat's id (chat.attach new): chat- and a counter")
    import subprocess
    import ferb.act.browser.profiles as browser_profiles
    import ferb.act.shell.sandbox as sandbox
    import ferb.device.permissions as permissions
    import ferb.device.selftest as selftest
    import ferb.identity.helper as identity_helper
    import ferb.runtimes.keys as keys
    import ferb.sense.watchers.clipboard as clipboard
    import ferb.sense.watchers.presence as presence
    from ferb.core.errors import Refused

    def no_helper(home: Any = None) -> Any:
        raise Refused("the signed secret helper is unavailable; owner setup is required")

    async def automation(bundle_id: str, ask: bool = False) -> Any:
        return permissions.Status.UNKNOWN

    async def no_version(binary: Any) -> None:
        return None
    stub(identity_helper, "_helper", no_helper, "the secret helper is never found: no Keychain, no codesign")
    stub(keys, "helper", lambda body: no_helper(), "API keys: the helper refuses (never the Keychain)")
    stub(keys, "_security", lambda *a: subprocess.CompletedProcess(["security", *a], 44, b"", b""),
         "API keys: `security` finds nothing (never the login Keychain, never a delete)")
    stub(sandbox.Sandbox, "available", lambda self: (True, "available"),
         "containment counts as available without running sandbox-exec (the shell tools are listed)")
    stub(permissions, "check_all", lambda: dict(GRANTS), "permission reads are invented (doctor.check)")
    stub(permissions, "automation_async", automation, "no Automation prompt (automation.request)")
    stub(permissions, "request", lambda permission: permissions.Status.DENIED, "no TCC prompt")
    stub(clipboard, "request_access", lambda board=None: "default", "the pasteboard is never read")
    stub(selftest, "run_all", lambda: [], "doctor's self-tests (an AX read, an event tap) don't run")
    stub(codex_exec, "check_version", no_version, "never `codex --version`")
    stub(browser_profiles, "_detached", lambda args: None, "no browser is ever launched")
    stub(presence, "session", lambda: {}, "no login-session read (its digest is machine-dependent)")
    import ferb.act.fs.files as fs_files
    import ferb.act.fs.look as fs_look
    real_identity = fs_files.identity

    def file_identity(p: Path) -> dict[str, Any]:
        facts = real_identity(p)
        facts.update({"modified_at": "2026-01-01T09:00:00+00:00", "owner_uid": 501, "owner_gid": 20, "device": 1,
                      "inode": int(sha(fx(str(p)))[:6], 16), "mtime_ns": 1767258000000000000})
        return facts
    stub(fs_files, "identity", file_identity, "a file's reported identity facts (time, owner, device, inode) are "
         "invented and fixed (the real ones differ per run); size and path are real")
    stub(fs_look, "identity", file_identity, "the same for fs.look")
    import platform
    from ferb.act.mac import apps as mac_apps
    import ferb.daemon.disturbance as disturbance
    stub(platform, "mac_ver", lambda *a: ("26.1", ("", "", ""), "arm64"), "an invented macOS version")
    stub(mac_apps, "running", lambda: [mac_apps.RunningApp(*app) for app in RUNNING_APPS],
         "the running apps are an invented list, never this Mac's")
    stub(disturbance, "DesktopProbe", FakeDesktop, "the desktop samples are invented, never this Mac's screen")


install_stubs()

from ferb.clock.clock import FakeClock  # noqa: E402
from ferb.core.ids import SequentialIds  # noqa: E402
from ferb.orchestrator import schemas as S  # noqa: E402
from tests.conftest import STALL, FakeRuntime, RunningDaemon, Script, role_of  # noqa: E402
from tests.support import egress  # noqa: E402

# ---------------------------------------------------------------------------------------------------------------
# Text: temporary paths become /FX, large bodies are stored once by SHA-256
# ---------------------------------------------------------------------------------------------------------------

REALS = sorted({str(ROOT), str(ROOT.resolve()), os.path.realpath(ROOT)}, key=len, reverse=True)


def fx(text: str) -> str:
    """Temporary paths become /FX; this checkout's own path (where doctor finds the pinned codex) becomes /REPO."""
    for real in REALS:
        text = text.replace(real, "/FX")
    return text.replace(str(REPO), "/REPO")


def sha(text: str) -> str:
    return hashlib.sha256(text.encode("utf-8")).hexdigest()


BODIES: dict[str, str] = {}
BODY_MIN = 2048                                       # texts this long or longer are stored once, by reference


def body(text: str) -> Any:
    """A text as recorded; texts of BODY_MIN characters or more become {"sha256": ...} references into bodies.json
    when the scenario is written (`intern`), after its volatile values are masked."""
    return text


def intern(node: Any) -> Any:
    if isinstance(node, dict):
        return {k: intern(v) for k, v in node.items()}
    if isinstance(node, list):
        return [intern(v) for v in node]
    if isinstance(node, str) and len(node) >= BODY_MIN:
        key = sha(node)
        BODIES[key] = node
        return {"sha256": key}
    return node


SCHEMA_NAMES: dict[str, str] = {}


def schema_name(schema: dict[str, Any]) -> str:
    if not SCHEMA_NAMES:
        for name in dir(S):
            value = getattr(S, name)
            if name.endswith("_SCHEMA") and isinstance(value, dict):
                SCHEMA_NAMES.setdefault(sha(json.dumps(value)), name)
        flags = ("inputs", "proposals", "conversation", "watch", "loop", "parts")
        for bits in range(64):
            on = {f: bool(bits >> i & 1) for i, f in enumerate(flags)}
            SCHEMA_NAMES.setdefault(sha(json.dumps(S.worker_schema(**on))),
                                    "worker_schema(" + ",".join(f for f in flags if on[f]) + ")")
    return SCHEMA_NAMES.get(sha(json.dumps(schema)), "sha256:" + sha(json.dumps(schema)))


# ---------------------------------------------------------------------------------------------------------------
# The scripted model: answers from tests/conftest.py's Script; every call it receives is recorded
# ---------------------------------------------------------------------------------------------------------------

PROBLEMS: list[str] = []
TOKEN_OF: dict[str, list[str]] = {}                   # a model call's task id -> the run tokens it was given                              # anything that makes a recording untrustworthy; fatal at the end


def check_answer(text: Any, schema: dict[str, Any], where: str) -> None:
    """Every scripted answer is one a real model could give under the schema it was asked for."""
    if isinstance(text, str) and text != STALL:
        try:
            jsonschema.Draft202012Validator(schema).validate(json.loads(text))
        except Exception as e:                             # noqa: BLE001
            PROBLEMS.append(f"{where}: the scripted answer doesn't fit its schema: {str(e)[:300]}")


class Model:
    def __init__(self, planner: list[Any] | None = None, workers: dict[str, Any] | None = None) -> None:
        self.script = Script(list(planner or []), dict(workers or {}))
        self.calls: list[dict[str, Any]] = []
        self.lock = threading.Lock()
        self.scenario: "Scenario | None" = None
        self.real_build = False             # True: runtimes the daemon builds by name (a check) are its real ones

    def factory(self, role: str, *, mode: str, tool_names: list[str], schema: dict[str, Any],
                effort: str | None = None, **_: Any) -> "RecRuntime":
        return RecRuntime(self, role, mode, list(tool_names), schema, effort)

    def keys(self) -> list[str]:
        return [c["key"] for c in self.calls]

    def started(self, key: str, n: int = 1) -> bool:
        return self.keys().count(key) >= n


class RecRuntime(FakeRuntime):
    def __init__(self, model: Model, role: str, mode: str, tools: list[str], schema: dict[str, Any],
                 effort: str | None) -> None:
        super().__init__(model.script, mode, tools)
        self.model, self.role, self.schema, self.effort = model, role, schema, effort

    async def start(self, task: Any, request: Any, run_token: str) -> Any:
        TOKEN_OF.setdefault(task.id, []).insert(0, run_token)
        handle = await super().start(task, request, run_token)
        key = handle.state["key"]
        with self.model.lock:
            record = {"n": len(self.model.calls) + 1, "key": key, "role_played": role_of(key),
                      "runtime_role": self.role, "mode": self.mode, "task_id": task.id,
                      "schema": schema_name(self.schema), "effort": self.effort, "tools": self.tools,
                      "system": body(request.system), "parts": [body(p) for p in request.parts],
                      "cacheable": request.cacheable, "images": len(request.images), "events": []}
            self.model.calls.append(record)
        handle.state["record"] = record
        return handle

    async def continue_(self, run: Any, message: str) -> None:
        """Same-run input (as Codex's and Claude's adapters accept it): recorded, taken."""
        run.state["record"].setdefault("continued_with", []).append(body(message))

    async def events(self, run: Any) -> Any:
        record = run.state["record"]
        key = run.state["key"]
        if (key == "supervisor" and not self.script.sup) or (key != "supervisor" and key not in self.script.workers
                                                             and role_of(key) == "worker"):
            PROBLEMS.append(f"{self.model.scenario.name if self.model.scenario else '?'}: call {record['n']} "
                            f"({key}) has no scripted answer")
        ended = False
        try:
            async for event in super().events(run):
                data = dict(event.data)
                if event.kind == "final":
                    check_answer(data.get("text"), self.schema, f"call {record['n']} ({record['key']})")
                    ended = True
                record["events"].append({"kind": event.kind, "data": {k: body(v) if isinstance(v, str) else v
                                                                      for k, v in data.items()}})
                yield event
        finally:
            if not ended:                              # FERB cancelled the call or stopped reading it
                record["events"].append({"kind": "(no answer: FERB cancelled or left the call)"})


# ---------------------------------------------------------------------------------------------------------------
# The daemon, as tests/conftest.py's RunningDaemon and `daemon` fixture build it
# ---------------------------------------------------------------------------------------------------------------

PREPARE: list[Callable[[Any], None]] = []         # run on every Daemon before its start (RPC stand-ins)
_NEXT: dict[str, Any] = {}


class Host(RunningDaemon):
    @staticmethod
    async def _make(home: Path, kwargs: dict[str, Any]) -> Any:
        d = await RunningDaemon._make(home, kwargs)
        model: Model = _NEXT["model"]
        # before the start, so the supervisor is built with it (compose.py passes make_runtime=self._make_runtime)
        # and a restart's resume_plans already talks to the scripted model
        d._make_runtime = model.factory
        if not model.real_build:
            d._build_runtime = lambda name, role, **kw: model.factory(
                role, **{k: v for k, v in kw.items() if k in ("mode", "tool_names", "schema", "effort")})
        d._input_monitoring_asked = True              # never reads TCC's Input Monitoring state
        d._permissions = lambda: {"accessibility": True, "screen": True}      # before the start (conftest: after)
        d.permission_check = lambda: dict(GRANTS)
        d.permission_request = _denied
        d.clipboard_request = lambda: "default"
        d._clipboard_access = lambda: "default"
        d.automation_check = _unknown_automation
        d._fresh_grants = _no_fresh_grants
        d.hearing.transcriber.pids = lambda *a: []
        d.claude_sign_in.cli = _no_claude_cli
        d.browser_launch = lambda argv: None
        for fn in PREPARE:
            fn(d)
        return d


def _denied(permission: str) -> Any:
    from ferb.device.permissions import Status
    return Status.DENIED


async def _unknown_automation(bundle: str) -> Any:
    from ferb.device.permissions import Status
    return Status.UNKNOWN


async def _no_fresh_grants() -> dict[str, str]:
    return {}


def _no_claude_cli() -> Any:
    from ferb.core.errors import Unsupported
    raise Unsupported("the claude program isn't available in the fixture")


LIVE: list[Host] = []


def new_host(home: Path, ids: str, model: Model) -> Host:
    _NEXT["model"] = model
    h = Host(home, clock=FakeClock(), ids=SequentialIds(ids), watch_owner_input=False)
    LIVE.append(h)
    d = h.daemon
    egress.install(getattr(d, "proxy", None))
    if d.supervisor is not None:
        d.supervisor.availability = d.supervisor._default_availability
    d.server.is_app = lambda token: True              # FERB.app's APP_ONLY methods are recorded too
    model.script.sock = h.sock
    return h


def on_loop(h: Host, fn: Callable[[], Any]) -> Any:
    async def go() -> Any:
        return fn()
    return h.run(go())


def fingerprint(h: Host) -> tuple[Any, ...]:
    def read() -> tuple[Any, ...]:
        d = h.daemon
        # an episode card is read by a process of its own (memory.py:136): quiet only once none is in flight
        return (d.ids._n, d.db.conn.total_changes, len(asyncio.all_tasks()), len(d._card_readers))
    return on_loop(h, read)


def settle(h: Host, quiet: float = 0.4, limit: float = 30) -> None:
    """Wait until the daemon is quiet: no new id, no database change and the same number of tasks on its loop for
    `quiet` seconds (a finished turn's after-work, e.g. its episode card, has run)."""
    end = time.monotonic() + limit
    last, since = fingerprint(h), time.monotonic()
    while time.monotonic() < end:
        time.sleep(0.03)
        now = fingerprint(h)
        if now != last:
            last, since = now, time.monotonic()
        elif time.monotonic() - since >= quiet and not now[3]:
            return
    raise AssertionError("the daemon never went quiet")


# ---------------------------------------------------------------------------------------------------------------
# Connections: raw newline-JSON lines, recorded both ways
# ---------------------------------------------------------------------------------------------------------------

class Conn:
    """One socket connection to the daemon. `log` holds every line in order: {"send": line} or {"recv": line}
    (lines without their trailing newline; every line the server writes is json.dumps(msg) + "\\n")."""

    def __init__(self, scenario: "Scenario", label: str, role: str = "cli", token: str | None = None,
                 hello: bool = True) -> None:
        self.scenario, self.label = scenario, label
        self.sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        self.sock.settimeout(60)
        self.sock.connect(str(scenario.host.sock))
        self.file = self.sock.makefile("rb")
        self.next_id = 0
        self.closed = False
        self.log: list[dict[str, str]] = []
        scenario.conns.append(self)
        if hello:
            params: dict[str, Any] = {"role": role}
            if token is not None:
                params["token"] = token
            self.call("auth.hello", params)

    def send_line(self, line: str) -> None:
        self.log.append({"send": line})
        self.sock.sendall(line.encode() + b"\n")

    def send(self, method: str, params: Any) -> int:
        self.next_id += 1
        self.send_line(json.dumps({"jsonrpc": "2.0", "id": self.next_id, "method": method, "params": params}))
        return self.next_id

    def read(self) -> dict[str, Any] | None:
        try:
            raw = self.file.readline()
        except (OSError, ValueError):
            return None                                     # this side closed it
        if not raw:
            if not self.closed:
                self.log.append({"closed": "by the daemon"})
            return None
        line = raw.decode()
        assert line.endswith("\n"), line
        self.log.append({"recv": line[:-1]})
        return json.loads(line)              # type: ignore[no-any-return]

    def until(self, rid: Any) -> list[dict[str, Any]]:
        out = []
        while True:
            msg = self.read()
            if msg is None:
                return out
            out.append(msg)
            if msg.get("id") == rid and ("result" in msg or "error" in msg):
                return out

    def call(self, method: str, params: Any = None) -> dict[str, Any]:
        msgs = self.until(self.send(method, {} if params is None else params))
        return msgs[-1] if msgs else {}

    def result(self, method: str, params: Any = None) -> Any:
        msg = self.call(method, params)
        if "error" in msg:
            raise AssertionError(f"{method}: {msg['error']}")
        return msg["result"]

    def close(self) -> None:
        self.closed = True
        self.log.append({"close": "by the client"})
        with contextlib.suppress(OSError):
            self.sock.shutdown(socket.SHUT_RDWR)          # wakes a reader blocked on this connection
        with contextlib.suppress(OSError):
            self.file.close()
            self.sock.close()


class Stream(threading.Thread):
    """A streaming call read in the background (chat.send while the script acts)."""

    def __init__(self, conn: Conn, method: str, params: Any) -> None:
        super().__init__(daemon=True)
        self.conn, self.rid = conn, conn.send(method, params)
        self.msgs: list[dict[str, Any]] = []
        self.start()

    def run(self) -> None:
        self.msgs = self.conn.until(self.rid)

    def wait(self, timeout: float = 60) -> list[dict[str, Any]]:
        self.join(timeout)
        if self.is_alive():
            raise AssertionError(f"{self.conn.label}: the stream never ended")
        return self.msgs

    def events(self) -> list[dict[str, Any]]:
        return [m["params"] for m in self.msgs if "params" in m]


# ---------------------------------------------------------------------------------------------------------------
# Scenarios: one fresh daemon in one fresh home each; an ordered script of what the client did and saw
# ---------------------------------------------------------------------------------------------------------------

SKIP_TABLES = ("schema_migrations", "schema_version", "sqlite_", "learning_search", "memory_search", "trail_search")
# columns written with SQLite's own clock (a migration's strftime('now')): the real time of the run
TABLE_VOLATILE = {"perception_floor": ("at",)}
SCENARIO_N = Counter("{:02d}")


# Values that differ by machine or by run and that no stand-in fixes: a file's identity and times as the file system
# reports them (fs tools), the daemon's own pid and executable. Replaced by "<volatile>" under these keys at any depth of
# any JSON in the fixtures (lines, replies, table cells holding JSON), then the same values wherever else they appear.
VOLATILE_KEYS = ("pid", "max_sample_ms", "executable", "code_hash")
DUMPS = (lambda v: json.dumps(v), lambda v: json.dumps(v, ensure_ascii=False),
         lambda v: json.dumps(v, ensure_ascii=False, separators=(",", ":")), lambda v: json.dumps(v, sort_keys=True))


def scrub(node: Any, keys: tuple[str, ...], seen: set[str]) -> Any:
    """`node` with every value under one of `keys` replaced by "<volatile>" (recorded in `seen` when long enough to
    replace safely as text elsewhere); a string holding JSON is parsed, scrubbed and written back in the same form."""
    if isinstance(node, dict):
        out = {}
        for k, v in node.items():
            if k in keys and v is not None and not isinstance(v, (dict, list, bool)):
                if isinstance(v, str) and re.fullmatch(r"/[^\s\"]{6,}", v):   # a path: safe to replace as text
                    seen.add(str(v))
                out[k] = "<volatile>"
            else:
                out[k] = scrub(v, keys, seen)
        return out
    if isinstance(node, list):
        return [scrub(v, keys, seen) for v in node]
    if isinstance(node, str) and node[:1] in "{[" and any(f'"{k}"' in node for k in keys):
        try:
            parsed = json.loads(node)
        except ValueError:
            return node
        for dump in DUMPS:
            if dump(parsed) == node:
                return dump(scrub(parsed, keys, seen))
    return node


def replace_text(node: Any, seen: set[str], keys: tuple[str, ...]) -> Any:
    """Then in plain text (a prompt holding a tool result's JSON): `"key": value` for each volatile key, and every value
    masked above wherever it appears."""
    if isinstance(node, dict):
        return {k: replace_text(v, seen, keys) for k, v in node.items()}
    if isinstance(node, list):
        return [replace_text(v, seen, keys) for v in node]
    if isinstance(node, str):
        if any(k in node for k in keys):
            names = "|".join(map(re.escape, keys))
            node = re.sub(r'"(' + names + r')": ("(?:[^"\\]|\\.)*"|-?[0-9][0-9.eE+-]*)', r'"\1": "<volatile>"', node)
            # the same inside JSON written as a string within JSON text (\"key\": value)
            node = re.sub(r'\\"(' + names + r')\\": (\\"(?:[^"\\]|\\[^"])*\\"|-?[0-9][0-9.eE+-]*)',
                          r'\\"\1\\": \\"<volatile>\\"', node)
        for value in sorted(seen, key=len, reverse=True):
            node = node.replace(value, "<volatile>")
    return node


class Scenario:
    def __init__(self, name: str, what: str, entry: list[str], model: Model | None = None,
                 volatile: list[str] | None = None) -> None:
        self.name, self.what, self.entry = name, what, entry
        self.root = ROOT / SCENARIO_N()
        TOKENS.n = 0                                       # run tokens and hex ids count from 1 in every scenario
        for h in Hex.every:
            h.n = 0
        self.home = self.root / "home"
        (self.root / "user").mkdir(parents=True)
        os.environ["HOME"] = str(self.root / "user")       # each scenario its own invented home folder
        self.model = model or Model()
        self.model.scenario = self
        self.volatile = list(volatile or [])
        self.conns: list[Conn] = []
        self.steps: list[dict[str, Any]] = []
        self.gateway_calls: list[dict[str, Any]] = []
        self.restarts = 0
        self.labels: dict[str, int] = {}
        self.open_streams: list[Stream] = []
        self.host = self._boot("id")

    def _boot(self, ids: str) -> Host:
        h = new_host(self.home, ids, self.model)
        g = h.daemon.gateway
        if g is not None:
            plain = g.call

            async def recorded(grant: Any, name: str, args: dict[str, Any], *a: Any, **k: Any) -> Any:
                row = {"task_id": grant.task_id, "worker_id": grant.worker_id, "tool": name,
                       "arguments": json.loads(json.dumps(args, default=str))}
                self.gateway_calls.append(row)
                reply = await plain(grant, name, args, *a, **k)
                row["reply"] = json.loads(json.dumps(reply, default=str))
                return reply
            g.call = recorded
        settle(h)
        return h

    # ---- the client's actions, each a step ----

    def conn(self, label: str, role: str = "cli", token: str | None = None) -> Conn:
        n = self.labels[label] = self.labels.get(label, 0) + 1
        label = label if n == 1 else f"{label}#{n}"        # every connection of a scenario has its own label
        c = Conn(self, label, role, token, hello=False)
        self.steps.append({"do": "connect", "conn": label})
        if role:
            start = len(c.log)
            params: dict[str, Any] = {"role": role} | ({"token": token} if token is not None else {})
            c.call("auth.hello", params)
            self.steps.append({"do": "call", "conn": label, "lines": c.log[start:]})
        return c

    def call(self, c: Conn, method: str, params: Any = None, *, raw: str | None = None) -> dict[str, Any]:
        start = len(c.log)
        if raw is not None:
            c.send_line(raw)
            msg = c.read() or {}
        else:
            msg = c.call(method, params)
        self.steps.append({"do": "call", "conn": c.label, "lines": c.log[start:]})
        return msg

    def stream(self, c: Conn, method: str, params: Any) -> Stream:
        start = len(c.log)
        s = Stream(c, method, params)
        s.start_at = start  # type: ignore[attr-defined]
        self.open_streams.append(s)
        self.steps.append({"do": "stream", "conn": c.label, "lines": c.log[start:start + 1]})
        return s

    def end(self, s: Stream, timeout: float = 60) -> list[dict[str, Any]]:
        s.wait(timeout)
        self.open_streams.remove(s)
        self.steps.append({"do": "stream_end", "conn": s.conn.label,
                           "lines": s.conn.log[s.start_at + 1:]})  # type: ignore[attr-defined]
        return s.events()

    def close(self, c: Conn) -> None:
        c.close()
        self.steps.append({"do": "close", "conn": c.label})

    def expect_end(self, c: Conn) -> None:
        """Read once more: the daemon closed the connection (a refused hello ends it)."""
        start = len(c.log)
        c.read()
        self.steps.append({"do": "read_until_closed", "conn": c.label, "lines": c.log[start:]})
        c.close()

    def wait(self, what: str, check: Callable[[], bool], timeout: float = 30) -> None:
        end = time.monotonic() + timeout
        while time.monotonic() < end:
            if check():
                self.steps.append({"do": "wait", "until": what})
                return
            time.sleep(0.02)
        raise AssertionError(f"{self.name}: never: {what}")

    def wait_model(self, key: str, n: int = 1) -> None:
        self.wait(f"the model has received call {key!r} ({n})", lambda: self.model.started(key, n))
        settle(self.host)

    def advance(self, seconds: float) -> None:
        """The clock moves; then one pass of the timer loop, as tests/support/schedules.py `fire` does (the loop's own
        wait is real time: `Timers.run` waits next_due_in() seconds of asyncio time)."""
        self.host.call_soon(self.host.daemon.clock.advance, seconds)
        self.steps.append({"do": "clock", "advance_s": seconds,
                           "now": on_loop(self.host, lambda: self.host.daemon.clock.now().isoformat())})
        fired = self.host.run(self.host.daemon.timers.tick())
        self.steps.append({"do": "timers_tick", "fired": fired})

    def settle(self) -> None:
        settle(self.host)

    def restart(self) -> None:
        """FERB stops (its clean shutdown) and starts again on the same home, ids under a new prefix (as
        tests/support/schedules.py own_daemon does: FERB's real ids are random)."""
        settle(self.host)
        self.host.stop()
        LIVE.remove(self.host)
        self.restarts += 1
        self.steps.append({"do": "restart", "ids_prefix": f"r{self.restarts}"})
        self.host = self._boot(f"r{self.restarts}")

    def note(self, text: str) -> None:
        self.steps.append({"do": "note", "text": text})

    # ---- the end ----

    def tables(self) -> dict[str, Any]:
        def read() -> dict[str, Any]:
            conn = self.host.daemon.db.conn
            names = [r[0] for r in conn.execute("SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name")]
            out: dict[str, Any] = {}
            for name in names:
                if name.startswith(SKIP_TABLES) or "_fts" in name or name.endswith(("_data", "_idx", "_docsize",
                                                                                    "_config", "_content")):
                    continue
                cur = conn.execute(f'SELECT * FROM "{name}"')
                rows = cur.fetchall()
                if not rows:
                    continue
                cols = [c[0] for c in cur.description]
                hide = TABLE_VOLATILE.get(name, ())
                out[name] = {"columns": cols,
                             "rows": [["<volatile>" if col in hide else cell(v) for col, v in zip(cols, tuple(r))]
                                      for r in rows]}
            return out
        return on_loop(self.host, read)

    def finish(self, tables: bool = True) -> dict[str, Any]:
        settle(self.host)
        dump = self.tables() if tables else None
        # the instructions as each call got them, and as the trace stores them (redacted): table cells refer to them
        sent = {c["system"] for c in self.model.calls if isinstance(c["system"], str)}
        stored = {on_loop(self.host, lambda t=t: self.host.daemon._redact_text(t)) for t in sent}
        self.prompt_forms = {fx(t) for t in sent | stored if len(t) >= BODY_MIN}
        self.host.stop()
        LIVE.remove(self.host)
        for st in list(self.open_streams):               # streams the daemon's shutdown ended
            st.join(10)
            self.steps.append({"do": "stream_end_at_shutdown", "conn": st.conn.label,
                               "lines": st.conn.log[st.start_at + 1:]})  # type: ignore[attr-defined]
        steps = []
        for st in self.steps:
            st = dict(st)
            steps.append(st)
        out: dict[str, Any] = {"name": self.name, "what": self.what, "python": self.entry,
                               "volatile": self.volatile, "steps": steps}
        if self.model.calls:
            out["model_calls"] = json.loads(fx(json.dumps(self.model.calls)))
        if self.gateway_calls:
            out["gateway_calls"] = json.loads(fx(json.dumps(self.gateway_calls)))
        if dump is not None:
            out["tables"] = json.loads(fx(json.dumps(dump)))
        seen: set[str] = set()
        keys = VOLATILE_KEYS + tuple(self.volatile)
        out = scrub(json.loads(fx(json.dumps(out))), keys, seen)
        out = replace_text(out, seen, keys)
        if "tables" in out:                               # a model's instructions copied into a span's input
            out["tables"] = embed_refs(out["tables"], self.prompt_forms)
        return intern(out)  # type: ignore[no-any-return]


def embed_refs(node: Any, texts: set[str]) -> Any:
    """In table cells, each of `texts` written inside JSON (json.dumps(text), with or without ensure_ascii) becomes
    "@@body:<sha256>:ascii@@" or "@@body:<sha256>:utf8@@" (quotes included in what it stands for): a reader puts back
    json.dumps(bodies[sha]) or json.dumps(bodies[sha], ensure_ascii=False). It keeps one copy of each prompt."""
    if isinstance(node, dict):
        return {k: embed_refs(v, texts) for k, v in node.items()}
    if isinstance(node, list):
        return [embed_refs(v, texts) for v in node]
    if isinstance(node, str) and len(node) >= BODY_MIN:
        for text in texts:
            for form, enc in (("ascii", json.dumps(text)), ("utf8", json.dumps(text, ensure_ascii=False))):
                if enc in node:
                    key = sha(text)
                    BODIES[key] = text
                    node = node.replace(enc, f'"@@body:{key}:{form}@@"')
    return node


def cell(v: Any) -> Any:
    if isinstance(v, bytes):
        return {"hex": v.hex()}
    return v


# ---------------------------------------------------------------------------------------------------------------
# Model answers in the full shape of their schemas (checked against the schema each call was given)
# ---------------------------------------------------------------------------------------------------------------

def side(**k: Any) -> dict[str, Any]:
    base: dict[str, Any] = {"answer_question": None, "remember": [], "forget": [], "timers": [], "schedules": [],
                            "schedule_changes": [], "show_schedules": False, "reminder_cancels": [],
                            "conversation_changes": []}
    return base | k


def condition(text: str, check: dict[str, Any] | None = None, layer: str = "outcome") -> dict[str, Any]:
    return {"text": text, "owner_text": "", "layer": layer, "check": check}


def plan_task(goal: str, *, name: str = "", identity: str = "owner", conds: list[dict[str, Any]] | None = None,
              plan: dict[str, Any] | None = None, paths: list[str] | None = None, sites: list[str] | None = None,
              recipients: list[str] | None = None, parts: list[dict[str, Any]] | None = None) -> dict[str, Any]:
    return {"goal": goal, "name": name, "identity": identity, "worker_type": "default",
            "targets": {"apps": [], "sites": sites or [], "paths": paths or [], "recipients": recipients or []},
            "success_conditions": conds or [condition(goal)], "plan": plan, "folders": [], "keep_tabs_open": False,
            "parts": parts or [], "earlier_results": []}


def step(id: str, goal: str, *, kind: str = "do", label: str = "", depends_on: list[str] | None = None,
         conditions: list[str] | None = None, question: str = "", options: list[str] | None = None,
         until: str | None = None, repeat: dict[str, Any] | None = None) -> dict[str, Any]:
    return {"id": id, "kind": kind, "label": label or goal, "goal": goal, "depends_on": depends_on or [],
            "inputs": [], "worker_type": "default", "conditions": conditions or [], "until": until, "event": None,
            "event_match": [], "question": question, "options": options or [], "conversation": None,
            "repeat": repeat, "earlier_results": []}


def plan(*steps: dict[str, Any]) -> dict[str, Any]:
    return {"steps": list(steps), "input_paths": [], "big_job": False, "time_estimate": "", "cost_estimate": "",
            "standing_sends": []}


def planner(kind: str, text: str, task: dict[str, Any] | None = None, *, control: dict[str, Any] | None = None,
            side_entries: dict[str, Any] | None = None, answers_question: bool = False) -> str:
    return json.dumps({"kind": kind, "answers_question": answers_question, "task": task, "control": control,
                       "side_entries": side_entries, "text": text})


def control(task_id: str, action: str, new_goal: bool = False) -> dict[str, Any]:
    return {"task_id": task_id, "action": action, "new_goal": new_goal, "standing_sends": "unchanged",
            "supersedes": []}


def worker(status: str = "done", answer: str = "", *, reason: str = "", question: str | None = None,
           artifacts: list[str] | None = None, how: str = "", **role: Any) -> str:
    return json.dumps({"status": status, "reason": reason, "question": question, "answer": answer,
                       "artifacts": artifacts or [], "how": how, **role})


def checker(*met: bool | None, next: str = "accept", findings: list[tuple[str, str, str]] | None = None,
            missing: str = "", instructions: str = "", question: str = "", reason: str = "",
            evidence: str = "Seen in the worker's report and the tool results.") -> str:
    return json.dumps({"results": [{"index": i, "met": m, "applies": True, "evidence": evidence,
                                    "how": None if m is not None else "Not visible from here."}
                                   for i, m in enumerate(met)],
                       "next": next, "findings": [{"quote": q, "kind": k, "evidence": e} for q, k, e in findings or []],
                       "missing": missing, "instructions": instructions, "worker_type": None, "question": question,
                       "reason": reason})


def acting(scn: "Scenario", key: str, calls: list[Any], final: str) -> Callable[[str], str]:
    """A scripted model turn that calls tools through the tool bridge's socket role (mcp_shim, with the run token the
    daemon gave this call), then answers `final`. Its lines are recorded under that model call."""
    def run(token: str) -> str:
        record = [c for c in scn.model.calls if c["key"] == key][-1]
        c = Conn(scn, f"mcp:{key}", "mcp_shim", token, hello=False)
        start = len(c.log)
        c.call("auth.hello", {"role": "mcp_shim", "token": token})
        for call in calls:
            if callable(call):
                call(c)
            else:
                c.call("tools.call", {"name": call[0], "arguments": call[1]})
        c.close()
        record["tool_bridge_lines"] = c.log[start:]
        return final
    return run


class Gate:
    """A scripted model turn that runs until the scenario lets it end (its answer, or its tool calls then answer)."""

    def __init__(self, scn: "Scenario", key: str, final: str, calls: list[Any] | None = None) -> None:
        self.event = threading.Event()
        inner = acting(scn, key, calls or [], final)

        def run(token: str) -> str:
            if not self.event.wait(60):
                PROBLEMS.append(f"{scn.name}: gate {key} was never opened")
            return inner(token)
        self.answer = run

    def open(self, scn: "Scenario") -> None:
        self.event.set()
        scn.steps.append({"do": "release_model", "note": "the held model turn may answer now"})


def read_change_records(c: Conn) -> None:
    """A checker's look at each change record of its task (L39: a pass on a step that changed files needs one), as
    tests/conftest.py `looked` does."""
    page = c.call("tools.call", {"name": "task.evidence", "arguments": {"kind": "context.source",
                                                                       "query": "Change record", "limit": 100}})
    for record in page["result"]["content"]["records"]:
        c.call("tools.call", {"name": "task.evidence", "arguments": {"span_id": record["id"]}})


# ---------------------------------------------------------------------------------------------------------------
# chat_scenarios.json
# ---------------------------------------------------------------------------------------------------------------

CHAT = ["ferb/daemon/areas/chat.py:322 Chat._chat_send", "ferb/orchestrator/supervisor.py:2388 Supervisor.send"]


def chat_turn(scn: Scenario, text: str, label: str = "app", session: str = "s1",
              **extra: Any) -> list[dict[str, Any]]:
    c = scn.conn(label)
    events = scn.end(scn.stream(c, "chat.send", {"session_id": session, "text": text} | extra))
    scn.close(c)
    scn.settle()
    return events


def gw_pending(scn: Scenario, tool: str) -> bool:
    return any(c["tool"] == tool and "reply" not in c for c in scn.gateway_calls)


def pending_approvals(scn: Scenario) -> list[Any]:
    return on_loop(scn.host, lambda: list(scn.host.daemon.state.approvals.pending()))


def tree_of(scn: Scenario, key: str = "worker") -> str:
    return next(c["task_id"] for c in scn.model.calls if c["key"] == key).split(".", 1)[0]


def sc_plain_reply() -> dict[str, Any]:
    m = Model([planner("reply", "Hello! What can I do for you today?")])
    scn = Scenario("plain_reply", "A greeting answered by the planner alone: no task.", CHAT, m)
    chat_turn(scn, "hi there")
    return scn.finish()


def sc_clarify() -> dict[str, Any]:
    m = Model([planner("clarify", "Which restaurant, and for what time?"),
               planner("task", "Booking the noodle bar for 7pm.",
                       plan_task("Find a free table at the noodle bar at 7pm tonight", name="Noodle bar table"),
                       answers_question=True)],
              {"worker": worker("done", "The noodle bar has a free table at 7pm."), "worker.verify": checker(True)})
    scn = Scenario("clarify_then_answer", "The planner asks a clarifying question; the owner's next message answers "
                   "it and becomes a task.", CHAT, m)
    chat_turn(scn, "find me a table tonight")
    chat_turn(scn, "the noodle bar, 7pm")
    return scn.finish()


def sc_one_step() -> dict[str, Any]:
    m = Model([planner("task", "On it.", plan_task("Say hello in the notes", name="Hello note"))],
              {"worker": worker("done", "Said hello."), "worker.verify": checker(True)})
    scn = Scenario("one_step_task", "A one-step task: planner, worker, checker, outcome.", CHAT, m)
    chat_turn(scn, "say hello")
    return scn.finish()


def sc_one_step_tools() -> dict[str, Any]:
    m = Model([planner("task", "Writing it.", plan_task("Write a shopping list to list.md with milk and eggs",
                                                        name="Shopping list", paths=["list.md"]))])
    scn = Scenario("one_step_with_tools", "A one-step task whose worker writes a file through the tool bridge and "
                   "whose checker reads it back.", CHAT, m)
    m.script.workers["worker"] = acting(scn, "worker", [("fs.write", {"path": "list.md", "content": "- milk\n- eggs\n",
                                                                     "effect": "edit", "why": "the list"})],
                                        worker("done", "Wrote list.md with milk and eggs.", artifacts=["list.md"]))
    m.script.workers["worker.verify"] = acting(scn, "worker.verify", [("fs.read", {"path": "list.md"}),
                                                                      read_change_records], checker(True))
    chat_turn(scn, "write a shopping list with milk and eggs")
    return scn.finish()


def sc_unread_changes() -> dict[str, Any]:
    m = Model([planner("task", "Writing it.", plan_task("Write a packing list to pack.md", name="Packing list",
                                                        paths=["pack.md"]))])
    scn = Scenario("checker_skips_change_records", "The worker writes a file; the checker accepts twice without "
                   "reading the change record (L39): the step can't be verified and ends done_unverified.", CHAT, m)
    m.script.workers["worker"] = acting(scn, "worker", [("fs.write", {"path": "pack.md", "content": "- socks\n",
                                                                     "effect": "edit", "why": "the list"})],
                                        worker("done", "Wrote pack.md.", artifacts=["pack.md"]))
    m.script.workers["worker.verify"] = [checker(True), checker(True)]
    chat_turn(scn, "write a packing list with socks")
    return scn.finish()


def sc_multi_step() -> dict[str, Any]:
    spec = plan_task("Collect the three invented city temperatures and write them to temps.md", name="City temps",
                     plan=plan(step("collect", "Collect today's temperature for Avalon, Brindle and Corvo",
                                    label="Collect temperatures"),
                               step("write", "Write the temperatures to temps.md", label="Write temps.md",
                                    depends_on=["collect"])))
    m = Model([planner("task", "Two steps: collect, then write.", spec)],
              {"step.collect": worker("done", "Avalon 21C, Brindle 18C, Corvo 25C."),
               "step.collect.verify": checker(True),
               "step.write": worker("done", "Wrote temps.md.", artifacts=["temps.md"]),
               "step.write.verify": checker(True)})
    scn = Scenario("multi_step_plan", "A two-step plan with a dependency: each step's worker and checker, the "
                   "step events and the checklist.", CHAT, m)
    chat_turn(scn, "get the temperatures for Avalon, Brindle and Corvo and write them down")
    return scn.finish()


def confirm_scenario(name: str, what: str, answer: str) -> dict[str, Any]:
    m = Model([planner("task", "Adding the alias.", plan_task("Add the alias ll to ~/.zshrc", name="Shell alias",
                                                               paths=["~/.zshrc"]))])
    scn = Scenario(name, what, CHAT + ["ferb/daemon/gateway.py:735 Gateway.call (a protected write asks)",
                                       "ferb/daemon/areas/chat.py:191 _chat_approve"], m)
    final_ok = worker("done", "Added the alias to ~/.zshrc.", artifacts=["~/.zshrc"])
    final_no = worker("not_done", "", reason="The owner said No to changing ~/.zshrc.")
    m.script.workers["worker"] = acting(scn, "worker", [("fs.write", {"path": "~/.zshrc",
                                                                     "content": "alias ll='ls -l'\n",
                                                                     "overwrite": True, "effect": "edit",
                                                                     "why": "add the alias"})],
                                        final_ok if answer == "yes" else final_no)
    m.script.workers["worker.verify"] = acting(scn, "worker.verify", [read_change_records], checker(True)) \
        if answer == "yes" else checker(False, next="fail", reason="The owner declined the change.")
    app = scn.conn("app")
    turn = scn.stream(scn.conn("chat"), "chat.send", {"session_id": "s1", "text": "add an alias ll for ls -l to my zshrc"})
    scn.wait("an approval is pending", lambda: bool(pending_approvals(scn)))
    scn.settle()
    status = scn.call(app, "chat.status", {})
    ap = status["result"]["approvals"][0]["approval_id"]
    if answer == "yes":
        scn.call(app, "chat.approve", {"approval_id": ap, "granted": True, "choice": 1})
    elif answer == "no":
        scn.call(app, "chat.approve", {"approval_id": ap, "granted": True, "choice": 2})
    else:
        tree = tree_of(scn)
        scn.call(app, "task.stop", {"task_ids": [tree]})
    scn.end(turn)
    scn.settle()
    scn.call(app, "chat.status", {})
    scn.close(app)
    return scn.finish()


def sc_confirm_yes() -> dict[str, Any]:
    return confirm_scenario("confirm_yes", "A write to a protected file (~/.zshrc, in the invented home) asks; the "
                            "owner's Yes (chat.approve choice 1) lets it run.", "yes")


def sc_confirm_no() -> dict[str, Any]:
    return confirm_scenario("confirm_no", "The same question answered No (choice 2): nothing is written; the worker "
                            "reports not done and the checker fails the task.", "no")


def sc_confirm_stop() -> dict[str, Any]:
    return confirm_scenario("confirm_stop_pending", "The same question left open; the owner stops the task "
                            "(task.stop) while it is pending.", "stop")


def stalled_task(scn: Scenario, text: str, key: str = "worker") -> tuple[Stream, str]:
    """Send `text` on its own connection and wait until the model holds call `key`; the tree's id."""
    turn = scn.stream(scn.conn("chat"), "chat.send", {"session_id": "s1", "text": text})
    scn.wait_model(key)
    return turn, tree_of(scn, key)


def sc_pause_resume() -> dict[str, Any]:
    m = Model([planner("task", "On it.", plan_task("Tidy the notes in notes.md", name="Tidy notes", paths=["notes.md"]))])
    scn = Scenario("pause_steer_resume", "A running task paused by the owner's words (planner control pause), "
                   "then resumed with a correction (control continue); the resumed attempt gets the correction.",
                   CHAT, m)
    m.script.workers["worker"] = [STALL, acting(scn, "worker", [("fs.write", {"path": "notes.md",
                                                                               "content": "# Notes\n\n## Today\n",
                                                                               "effect": "edit", "why": "tidy"})],
                                                worker("done", "Tidied notes.md and kept the headings.",
                                                       artifacts=["notes.md"]))]
    m.script.workers["worker.verify"] = acting(scn, "worker.verify", [read_change_records], checker(True))
    turn, tree = stalled_task(scn, "tidy my notes")
    m.script.sup.append(planner("reply", "Paused; tell me how you'd like it.", control=control(tree, "pause")))
    chat_turn(scn, "wait, hold on")
    scn.wait("the task is paused", lambda: row_status(scn, tree) == "paused")
    m.script.sup.append(planner("reply", "Carrying on, keeping the headings.", control=control(tree, "continue")))
    chat_turn(scn, "ok, carry on but keep the headings")
    scn.end(turn)
    return scn.finish()


def sc_steer_live() -> dict[str, Any]:
    m = Model([planner("task", "On it.", plan_task("Draft a short note to the team about Friday", name="Team note"))])
    scn = Scenario("steer_running_task", "A correction while the worker runs (control continue): FERB delivers the "
                   "owner's words into the running model turn (continue_), which then finishes with them.", CHAT, m)
    gate = Gate(scn, "worker", worker("done", "Drafted the note and mentioned the 3pm start."))
    m.script.workers["worker"] = gate.answer
    m.script.workers["worker.verify"] = checker(True)
    turn, tree = stalled_task(scn, "draft a note to the team about friday")
    m.script.sup.append(planner("reply", "Adding the 3pm start.", control=control(tree, "continue")))
    chat_turn(scn, "mention it starts at 3pm")
    gate.open(scn)
    scn.end(turn)
    return scn.finish()


def stop_scenario(name: str, what: str, how: str) -> dict[str, Any]:
    m = Model([planner("task", "Watching the build.", plan_task("Wait for the invented build to finish",
                                                                name="Build watch"))],
              {"worker": STALL})
    scn = Scenario(name, what, CHAT, m)
    turn, tree = stalled_task(scn, "tell me when the build finishes")
    if how == "rpc":
        app = scn.conn("app")
        scn.call(app, "task.stop", {"task_ids": [tree]})
        scn.call(app, "task.stop", {})                         # invalid: no ids
        scn.close(app)
    elif how == "chat":
        m.script.sup.append(planner("reply", "", control=control(tree, "stop")))
        chat_turn(scn, "stop that")
    elif how == "chat+new":
        m.script.sup.append(planner("task", "Checking the weather for Avalon.",
                                    plan_task("Check the invented weather for Avalon", name="Avalon weather"),
                                    control=control(tree, "stop")))
        m.script.workers["worker"] = [worker("done", "Avalon: sunny, 21C.")]
        m.script.workers["worker.verify"] = checker(True)
        chat_turn(scn, "forget the build, check the weather in Avalon instead")
    elif how == "global":
        app = scn.conn("app")
        scn.call(app, "stop.engage", {})
        scn.end(turn)
        scn.settle()
        scn.call(app, "chat.status", {})
        m.script.sup.append(planner("reply", "Everything is stopped. Say when I can carry on."))
        c = scn.conn("chat2")
        scn.end(scn.stream(c, "chat.send", {"session_id": "s1", "text": "hello?"}))
        scn.close(c)
        scn.call(app, "stop.lift", {})
        scn.call(app, "chat.status", {})
        scn.close(app)
        return scn.finish()
    scn.end(turn)
    return scn.finish()


def sc_stop_rpc() -> dict[str, Any]:
    return stop_scenario("stop_by_task_stop", "The app's Stop on a running task (task.stop with its id), and the "
                         "refusal of task.stop without ids.", "rpc")


def sc_stop_chat() -> dict[str, Any]:
    return stop_scenario("stop_by_chat", "The owner says stop in chat: the planner's control stop; the reply is the "
                         "task's end as code recorded it (ORCH-579).", "chat")


def sc_stop_and_new() -> dict[str, Any]:
    return stop_scenario("stop_plus_new_task", "One message stops the running task and starts a new one (ORCH-638): "
                         "code's stop line, then the new task's plan.", "chat+new")


def sc_global_stop() -> dict[str, Any]:
    return stop_scenario("global_stop", "stop.engage while a task runs: everything stops; a chat message while "
                         "stopped; stop.lift.", "global")


def sc_merged() -> dict[str, Any]:
    m = Model([STALL, planner("task", "Checking both cities.",
                              plan_task("Check the invented weather for Avalon and Brindle", name="Two cities weather"))],
              {"worker": worker("done", "Avalon: sunny. Brindle: rain."), "worker.verify": checker(True)})
    scn = Scenario("second_message_merged", "A second message sent before FERB's first reply: the turn restarts once "
                   "and reads both (ORCH-529); the second message's stream says it was merged.", CHAT, m)
    first = scn.stream(scn.conn("chat"), "chat.send", {"session_id": "s1", "text": "what's the weather in Avalon"})
    scn.wait_model("supervisor")
    second = scn.stream(scn.conn("chat2"), "chat.send", {"session_id": "s1", "text": "and Brindle too"})
    scn.end(second)
    scn.end(first)
    return scn.finish()


def sc_schedule() -> dict[str, Any]:
    sched = {"request": "Check the invented permit page for open slots", "name": "Permit check",
             "when": {"in_minutes": 5, "in_days": None, "day": None, "date": None, "time": None}, "repeat": "none",
             "weekday": None, "every_minutes": None, "times": [], "random": [], "quote": "in 5 minutes",
             "targets": {"apps": [], "sites": [], "paths": [], "recipients": []}, "standing_sends": [],
             "watch": "none", "until": None, "when_active": False, "folders": [], "identity": "owner",
             "worker_type": "default", "profile": "", "account": ""}
    m = Model([planner("reply", "I'll check the permit page in 5 minutes.", side_entries=side(schedules=[sched])),
               planner("task", "Checking the permit page.", plan_task("Check the invented permit page for open slots",
                                                                       name="Permit check"))],
              {"worker": worker("done", "No open slots yet."), "worker.verify": checker(True)})
    scn = Scenario("schedule_created_and_fired", "A schedule made from chat (side_entries.schedules), listed, then "
                   "fired by the clock: the scheduled run's planner turn, worker and checker; what the app's "
                   "notifications stream and chat.status show.", CHAT + [
                       "ferb/daemon/areas/schedules.py Schedules (timers, scheduled runs)",
                       "ferb/orchestrator/supervisor.py:1629 Supervisor.run_scheduled"], m)
    app = scn.conn("app")
    chat_turn(scn, "check the permit page in 5 minutes")
    scn.call(app, "schedules.list", {})
    notes = scn.conn("notifications")
    feed = scn.stream(notes, "notifications.subscribe", {"authorized": True})
    scn.advance(300)
    scn.wait_model("worker.verify")
    scn.settle()
    scn.call(app, "chat.status", {})
    scn.call(app, "schedules.list", {})
    scn.close(notes)
    feed.join(5)
    scn.open_streams.remove(feed)
    scn.steps.append({"do": "stream_cut", "conn": "notifications", "lines": notes.log[feed.start_at + 1:]})
    scn.close(app)
    return scn.finish()


def sc_timer() -> dict[str, Any]:
    timer = {"when": {"in_minutes": 2, "in_days": None, "day": None, "date": None, "time": None},
             "message": "Take the bread out", "quote": "a timer for 2 minutes", "when_active": False, "name": "Bread",
             "kind": "timer"}
    m = Model([planner("reply", "Timer set for 2 minutes.", side_entries=side(timers=[timer]))])
    scn = Scenario("timer_set_and_fired", "A timer from chat (side_entries.timers), listed, fired by the clock; its "
                   "notification and its state; then timers.repeat.", CHAT + [
                       "ferb/daemon/areas/schedules.py reminders.list, timers.states, timers.repeat"], m)
    app = scn.conn("app")
    notes = scn.conn("notifications")
    feed = scn.stream(notes, "notifications.subscribe", {"authorized": True})
    chat_turn(scn, "set a timer for 2 minutes to take the bread out")
    listed = scn.call(app, "reminders.list", {})
    ids = [r["id"] for r in listed["result"].get("pending", [])]
    scn.advance(120)
    scn.settle()
    scn.call(app, "timers.states", {"ids": ids})
    if ids:
        scn.call(app, "timers.repeat", {"id": ids[0]})
    scn.call(app, "reminders.list", {})
    scn.close(notes)
    feed.join(5)
    scn.open_streams.remove(feed)
    scn.steps.append({"do": "stream_cut", "conn": "notifications", "lines": notes.log[feed.start_at + 1:]})
    scn.close(app)
    return scn.finish()


def sc_wait_for() -> dict[str, Any]:
    m = Model([planner("task", "Waiting two minutes, then checking.",
                       plan_task("Wait two minutes, then report that the wait is over", name="Short wait"))])
    scn = Scenario("wait_for_time", "A worker waits in code (wait.for, a time condition) with no model calls; the "
                   "clock moves; the one result wakes it (ORCH-631..633).", CHAT + [
                       "ferb/clock/wait.py WaitFor"], m)
    m.script.workers["worker"] = acting(scn, "worker", [("wait.for", {"any_of": [{"kind": "time", "in_minutes": 2}],
                                                                      "limit_minutes": 5, "why": "the wait"})],
                                        worker("done", "Waited two minutes."))
    m.script.workers["worker.verify"] = checker(True)
    turn = scn.stream(scn.conn("chat"), "chat.send", {"session_id": "s1", "text": "wait two minutes then tell me"})
    scn.wait("the worker's wait.for is running", lambda: gw_pending(scn, "wait.for"))
    scn.settle()
    scn.advance(120)
    scn.end(turn)
    return scn.finish()


def sc_tool_failure_retry() -> dict[str, Any]:
    m = Model([planner("task", "Reading the report.", plan_task("Summarise report.txt", name="Report summary",
                                                                 paths=["report.txt"]))])
    scn = Scenario("tool_failure_then_retry", "The worker's tool call fails (a missing file); it reports not done; the "
                   "checker chooses retry with instructions; the second attempt reads the file the owner meanwhile "
                   "made and is accepted.", CHAT, m)
    m.script.workers["worker"] = [
        acting(scn, "worker", [("fs.read", {"path": "report.txt"})],
               worker("not_done", "", reason="report.txt doesn't exist in the workspace.")),
        acting(scn, "worker", [("fs.read", {"path": "report.txt"})],
               worker("done", "The report says sales rose 4% in the invented quarter."))]
    m.script.workers["worker.verify"] = [
        checker(None, next="retry", missing="The report wasn't read.", instructions="Read report.txt again; it may "
                "exist now."),
        checker(True)]
    turn = scn.stream(scn.conn("chat"), "chat.send", {"session_id": "s1", "text": "summarise report.txt"})
    scn.wait("the first checker has answered", lambda: m.started("worker.verify"))
    (scn.home / "workspace" / "report.txt").write_text("Sales rose 4% in the invented quarter.\n")
    scn.note("the owner writes workspace/report.txt (\"Sales rose 4% in the invented quarter.\\n\") between the "
             "attempts")
    scn.end(turn)
    return scn.finish()


def sc_plan_repair() -> dict[str, Any]:
    bad = plan_task("Collect and write the invented city temperatures", name="City temps",
                    plan=plan(step("collect", "Collect the temperatures"),
                              step("write", "Write temps.md", depends_on=["gather"])))
    good = plan_task("Collect and write the invented city temperatures", name="City temps",
                     plan=plan(step("collect", "Collect the temperatures"),
                               step("write", "Write temps.md", depends_on=["collect"])))
    m = Model([planner("task", "Two steps.", bad), planner("task", "Two steps.", good)],
              {"step.collect": worker("done", "Avalon 21C."), "step.collect.verify": checker(True),
               "step.write": worker("done", "Wrote temps.md."), "step.write.verify": checker(True)})
    scn = Scenario("plan_repair", "The planner's plan names a dependency that doesn't exist; code sends it back "
                   "with the problem (the repair path) and the second plan runs.", CHAT, m)
    chat_turn(scn, "collect the temperatures and write them down")
    return scn.finish()


def sc_checker_fail() -> dict[str, Any]:
    m = Model([planner("task", "Looking.", plan_task("Find the invented museum's opening hours", name="Museum hours"))],
              {"worker": worker("done", "The museum opens at 10am."),
               "worker.verify": checker(False, next="fail", reason="Nothing the worker read shows the hours.",
                                        evidence="No page or file with the hours was read.")})
    scn = Scenario("checker_rejects", "The checker finds the outcome not met and chooses fail: the task ends failed "
                   "with the checker's reason.", CHAT, m)
    chat_turn(scn, "when does the museum open")
    return scn.finish()


def sc_checker_findings() -> dict[str, Any]:
    said = "The library opens at 10am. It has a rooftop garden."
    m = Model([planner("task", "Looking.", plan_task("Find the invented library's opening hours",
                                                       name="Library hours"))],
              {"worker": worker("done", said),
               "worker.verify": checker(True, findings=[("It has a rooftop garden.", "unsupported",
                                                         "Nothing about a garden was seen.")]),
               "worker.fix": json.dumps({"answer": "The library opens at 10am."}),
               "worker.recheck": json.dumps({"findings": []})})
    scn = Scenario("checker_findings_fix", "The checker accepts the outcome but flags an unsupported sentence "
                   "(ORCH-545): one fix turn, a re-check, the fixed answer.", CHAT, m)
    chat_turn(scn, "library hours?")
    return scn.finish()


def sc_worker_question() -> dict[str, Any]:
    spec = plan_task("Sort the invented downloads then report", name="Sort downloads",
                     plan=plan(step("sort", "Sort the downloads"),
                               step("report", "Report what was sorted", depends_on=["sort"])))
    m = Model([planner("task", "On it.", spec)],
              {"step.sort": [worker("not_done", "", reason="Which folder should I sort?",
                                    question="Which folder should I sort?"),
                             worker("done", "Sorted the Downloads folder.")],
               "step.sort.verify": [checker(None, next="ask", question="Which folder should I sort?"),
                                    checker(True)],
               "step.report": worker("done", "Reported."), "step.report.verify": checker(True)})
    scn = Scenario("worker_question_answered", "A plan step needs the owner: it pauses in place and asks once "
                   "(approval.requested); chat.answer carries it on with the answer.", CHAT, m)
    app = scn.conn("app")
    turn = scn.stream(scn.conn("chat"), "chat.send", {"session_id": "s1", "text": "sort my downloads and tell me"})
    scn.wait("an approval is pending", lambda: bool(pending_approvals(scn)))
    scn.settle()
    status = scn.call(app, "chat.status", {})
    ap = status["result"]["approvals"][0]["approval_id"]
    scn.call(app, "chat.answer", {"approval_id": ap, "text": "the Downloads folder"})
    scn.end(turn)
    scn.close(app)
    return scn.finish()


def sc_check_in() -> dict[str, Any]:
    m = Model([planner("task", "On it.", plan_task("Book the invented table", name="Table booking"))],
              {"worker": worker("not_done", "", reason="The site errored."),
               "worker.verify": [checker(None, next="retry", missing="The table isn't booked.",
                                         instructions="Try another way.") for _ in range(5)]})
    scn = Scenario("check_in_after_five", "Five failed attempts, then FERB's check-in (Keep going? Yes/No, options "
                   "only: typed words refused); No stops with what it found (ORCH-307, ORCH-536).", CHAT, m)
    app = scn.conn("app")
    turn = scn.stream(scn.conn("chat"), "chat.send", {"session_id": "s1", "text": "book the table"})
    scn.wait("an approval is pending", lambda: bool(pending_approvals(scn)))
    scn.settle()
    ap = scn.call(app, "chat.status", {})["result"]["approvals"][0]["approval_id"]
    scn.call(app, "chat.answer", {"approval_id": ap, "text": "no, stop it"})
    scn.call(app, "chat.approve", {"approval_id": ap, "granted": True, "choice": 2})
    scn.end(turn)
    scn.close(app)
    return scn.finish()


def sc_restart_mid_task() -> dict[str, Any]:
    m = Model([planner("task", "On it.", plan_task("Compare the two invented price lists", name="Price compare"))],
              {"worker": [STALL, worker("done", "List B is cheaper by 12%.")], "worker.verify": checker(True)})
    scn = Scenario("restart_mid_task", "FERB stops (its clean shutdown) while a worker runs and starts again on the "
                   "same home: the task resumes by itself (resume_plans), its step runs again, the result is waiting "
                   "in chat.status for the app.", CHAT + ["ferb/daemon/main.py:418 interrupt_running",
                                                          "ferb/orchestrator/supervisor.py resume_plans"], m)
    turn, _ = stalled_task(scn, "compare the two price lists")
    scn.restart()
    turn.join(10)
    scn.open_streams.remove(turn)
    scn.steps.append({"do": "stream_end_at_shutdown", "conn": "chat", "lines": turn.conn.log[turn.start_at + 1:]})
    scn.wait_model("worker.verify")
    scn.settle()
    app = scn.conn("app")
    status = scn.call(app, "chat.status", {})
    done_ids = [r["task_id"] for r in status["result"].get("results", [])]
    if done_ids:
        scn.call(app, "results.ack", {"task_ids": done_ids})
    scn.call(app, "tasks.list", {})
    scn.close(app)
    return scn.finish()


def sc_restart_pending_question() -> dict[str, Any]:
    m = Model([planner("task", "On it.", plan_task("Book the invented table", name="Table booking"))],
              {"worker": worker("not_done", "", reason="The site errored."),
               "worker.verify": [checker(None, next="retry", missing="Not booked.", instructions="Try another way.")
                                 for _ in range(5)]})
    scn = Scenario("restart_with_open_question", "A check-in is open when FERB restarts: after the start it is asked "
                   "again (a new approval) and No ends the task; no attempt runs again.", CHAT, m)
    turn = scn.stream(scn.conn("chat"), "chat.send", {"session_id": "s1", "text": "book the table"})
    scn.wait("an approval is pending", lambda: bool(pending_approvals(scn)))
    scn.restart()
    turn.join(10)
    scn.open_streams.remove(turn)
    scn.steps.append({"do": "stream_end_at_shutdown", "conn": "chat", "lines": turn.conn.log[turn.start_at + 1:]})
    scn.wait("an approval is pending again", lambda: bool(pending_approvals(scn)))
    scn.settle()
    app = scn.conn("app")
    ap = scn.call(app, "chat.status", {})["result"]["approvals"][0]["approval_id"]
    scn.call(app, "chat.approve", {"approval_id": ap, "granted": True, "choice": 2})
    scn.wait("the task ended", lambda: not on_loop(scn.host, scn.host.daemon.supervisor.running_trees))
    scn.settle()
    scn.call(app, "chat.status", {})
    scn.close(app)
    return scn.finish()


def sc_input_paths() -> dict[str, Any]:
    m = Model([planner("task", "Reading the attached file.", plan_task("Say what the attached notes.txt is about",
                                                                       name="Attached notes"))],
              {"worker": worker("done", "It is a list of three invented errands."), "worker.verify": checker(True)})
    scn = Scenario("attachment_and_bad_params", "A message with an attached file (input_paths), then chat.send's "
                   "refusals: input_paths not a list, a reserved session id.", CHAT, m)
    (scn.home / "workspace").mkdir(parents=True, exist_ok=True)
    (scn.home / "workspace" / "notes.txt").write_text("- post office\n- bakery\n- library\n")
    scn.note("workspace/notes.txt holds three invented errands")
    chat_turn(scn, "what is this about?", input_paths=["notes.txt"])
    c = scn.conn("bad")
    scn.end(scn.stream(c, "chat.send", {"session_id": "s1", "text": "x", "input_paths": "notes.txt"}))
    scn.end(scn.stream(c, "chat.send", {"session_id": "preview:x", "text": "x"}))
    scn.close(c)
    return scn.finish()


def row_status(scn: Scenario, task_id: str) -> str | None:
    def read() -> Any:
        r = scn.host.daemon.db.conn.execute("SELECT status FROM tasks WHERE id = ?", (task_id,)).fetchone()
        return r[0] if r else None
    return on_loop(scn.host, read)  # type: ignore[no-any-return]


CHAT_SCENARIOS: list[Callable[[], dict[str, Any]]] = [
    sc_plain_reply, sc_clarify, sc_one_step, sc_one_step_tools, sc_unread_changes, sc_multi_step, sc_confirm_yes, sc_confirm_no,
    sc_confirm_stop, sc_pause_resume, sc_steer_live, sc_stop_rpc, sc_stop_chat, sc_stop_and_new, sc_global_stop,
    sc_merged, sc_schedule, sc_timer, sc_wait_for, sc_tool_failure_retry, sc_plan_repair, sc_checker_fail,
    sc_checker_findings, sc_worker_question, sc_check_in, sc_restart_mid_task, sc_restart_pending_question,
    sc_input_paths]


# ---------------------------------------------------------------------------------------------------------------
# rpc_transcripts.json: every method of auth.METHODS, each called with valid and with invalid parameters
# ---------------------------------------------------------------------------------------------------------------

RPC = ["ferb/daemon/server.py:222 Server._connection", "ferb/daemon/server.py:259 Server._hello",
       "ferb/daemon/server.py:290 Server._dispatch", "ferb/daemon/areas/rpc.py:130 Rpc._register_rpc"]


def calls(scn: Scenario, c: Conn, *pairs: tuple[str, Any]) -> list[dict[str, Any]]:
    return [scn.call(c, method, params) for method, params in pairs]


def drain_stream(scn: Scenario, label: str, method: str, params: Any, act: Callable[[], None] | None = None,
                 wait: float = 0.3) -> None:
    """A stream that never ends by itself (logs, notifications): opened, `act` run, then the client leaves."""
    c = scn.conn(label)
    feed = scn.stream(c, method, params)
    if act is not None:
        act()
    scn.settle()
    time.sleep(wait)
    scn.close(c)
    feed.join(5)
    scn.open_streams.remove(feed)
    scn.steps.append({"do": "stream_cut", "conn": c.label, "lines": c.log[feed.start_at + 1:]})


def rpc_protocol() -> dict[str, Any]:
    scn = Scenario("protocol", "The connection itself: auth.hello for each role (good and refused), a call before "
                   "hello, invalid JSON, a request that isn't one, an unknown method, a method the role may not call, "
                   "an APP_ONLY method from a process that isn't FERB.app, ping.", RPC + [
                       "ferb/daemon/auth.py:28 METHODS, :61 APP_ONLY, :155 check_native_host"])
    c = scn.conn("before-hello", role="")
    scn.call(c, "ping", {})                                  # no hello: refused, and the connection ends
    scn.expect_end(c)
    c = scn.conn("garbage", role="")
    scn.call(c, "", raw="this is not json")
    scn.call(c, "", raw='{"jsonrpc": "2.0", "id": 7}')
    scn.call(c, "auth.hello", {"role": "no-such-role"})
    scn.expect_end(c)
    c = scn.conn("cli")
    calls(scn, c, ("ping", {}), ("ping", {"extra": [1, 2]}), ("no.such_method", {}), ("tools.call", {}),
          ("tools.list", {}), ("auth.hello", {"role": "cli"}))
    scn.call(c, "", raw='{"jsonrpc": "2.0", "id": "string-id", "method": "ping", "params": {}}')
    scn.call(c, "", raw='{"jsonrpc": "2.0", "id": null, "method": "ping"}')
    scn.close(c)
    scn.host.daemon.server.is_app = lambda token: False
    scn.note("from here FERB.app's signature check says no (server.is_app returns False)")
    c = scn.conn("not-the-app")
    calls(scn, c, ("seen.list", {}), ("tasks.preview", {"task_id": "x"}), ("chat.history.days", {}))
    scn.close(c)
    scn.host.daemon.server.is_app = lambda token: True
    scn.note("server.is_app returns True again")
    c = scn.conn("mcp-bad-token", role="")
    scn.call(c, "auth.hello", {"role": "mcp_shim", "token": "not-a-token"})
    scn.expect_end(c)
    c = scn.conn("mcp-no-token", role="")
    scn.call(c, "auth.hello", {"role": "mcp_shim"})
    scn.expect_end(c)
    c = scn.conn("native-bad-origin", role="")
    scn.call(c, "auth.hello", {"role": "native_host", "origin": "chrome-extension://aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa/"})
    scn.expect_end(c)
    ext = sorted(scn.host.daemon.server.extension_ids())[0]
    c = scn.conn("native-host", role="")
    scn.call(c, "auth.hello", {"role": "native_host", "origin": f"chrome-extension://{ext}/"})
    scn.note("after a native host's hello the connection carries extension messages (rust/fixtures/native-host)")
    scn.close(c)
    return scn.finish(tables=False)


def rpc_setup_and_about() -> dict[str, Any]:
    scn = Scenario("settings_setup_about", "Settings, setup, permissions, About, health, sensing, the global stop, "
                   "secrets, keys, Jev, notices and other calls that need no task. Mac reads are stand-ins (STUBS).",
                   RPC + ["ferb/daemon/areas/rpc.py, about.py, sensing.py, notify.py, jev.py, runtimes.py, intro.py, "
                          "memory_import.py, records.py, voice.py"],
                   )
    project = scn.root / "user" / ".claude" / "projects" / "invented-project"
    project.mkdir(parents=True)
    (project / "session-1.jsonl").write_text("".join(json.dumps(r) + "\n" for r in [
        {"type": "user", "timestamp": "2025-12-30T10:00:00Z", "message": {"role": "user", "content":
                                                                          "I keep my invented notes in ~/notes"}},
        {"type": "assistant", "timestamp": "2025-12-30T10:00:05Z", "message": {"role": "assistant", "content": [
            {"type": "text", "text": "Noted."}]}}]))
    scn.note("~/.claude/projects/invented-project/session-1.jsonl holds one invented exchange (for "
             "memory.import.estimate)")
    c = scn.conn("app")
    calls(scn, c,
          ("memory.import.estimate", {"source": "claude_code"}),
          ("settings.get", {}), ("settings.set", {"key": "sense.window_titles", "value": False}),
          ("settings.set", {"key": "no.such_setting", "value": 1}), ("settings.get", {}),
          ("about.get", {}), ("about.guide", {"app_settings": [{"key": "appearance", "name": "Appearance",
                                                                 "where": "Settings → General", "what": "Light or dark",
                                                                 "value": "System"}]}),
          ("about.guide", {"app_settings": "not a list"}),
          ("doctor.check", {"selftest": False, "hearing": False}), ("doctor.check", {"selftest": "yes"}),
          ("setup.permissions", {}), ("setup.permissions", {"fresh": "screen_recording"}),
          ("setup.permission_request", {"permission": "accessibility"}),
          ("setup.permission_request", {"permission": "full_disk_access"}),
          ("setup.finish", {"app": {"notifications": "granted", "microphone": "granted"}}),
          ("setup.finish", {"app": "nope"}),
          ("clipboard.request", {}), ("clipboard.request", {"extra": True}),
          ("automation.request", {"bundle_ids": ["com.apple.TextEdit"]}), ("automation.request", {"bundle_ids": 7}),
          ("sense.status", {}), ("sense.pause", {}), ("sense.status", {}), ("sense.resume", {}),
          ("sense.pause", {"minutes": "x"}), ("sense.resume", {}),
          ("stop.engage", {}), ("ping", {}), ("stop.lift", {}), ("stop.lift", {"x": 1}),
          ("ui.click_through", {"windows": [1, 2]}), ("ui.click_through", {"windows": "x"}),
          ("identity.check", {"session": False}), ("identity.check", {"session": "maybe"}),
          ("secrets.saved", {}), ("secrets.delete", {"place": "example.test", "name": "Example"}),
          ("secrets.delete", {}),
          ("seen.list", {}), ("seen.image", {"id": "no-such-image"}), ("seen.image", {}),
          ("keys.store", {"provider": "anthropic", "key": "fixture-invented-value"}),
          ("keys.store", {"provider": "no-such-provider", "key": "x"}),
          ("keys.delete", {"provider": "anthropic"}), ("keys.delete", {"provider": "no-such-provider"}),
          ("jev.stats", {}), ("jev.enable", {"question": "trail.relevance", "on": False}),
          ("jev.enable", {"question": "no.such_question", "on": False}), ("jev.reload", {}), ("jev.stats", {"x": 1}),
          ("notifications.categories", {}), ("notifications.categories", {"categories": 1}),
          ("notices.list", {}), ("notices.list", {"limit": "x"}),
          ("attention.ack", {"ids": []}), ("attention.ack", {}),
          ("conversations.list", {}), ("conversations.list", {"id": "no-such-conversation"}),
          ("conversations.change", {"id": "no-such-conversation", "action": "pause", "owner_words": "pause it"}),
          ("conversations.change", {}),
          ("setup.persona", {}), ("setup.persona", {"force": "x"}),
          ("intro.suggest", {}), ("intro.suggest", {"count": "x"}),
          ("tasks.preview", {"task_id": "no-such-task"}), ("tasks.preview", {}),
          ("questions.picture", {"approval_id": "no-such-question"}), ("questions.picture", {}),
          ("memory.import.estimate", {"source": "codex"}), ("memory.import.estimate", {"source": "no-such-source"}),
          ("memory.import.status", {}), ("memory.import.cancel", {}),
          ("memory.import.start", {"source": "chatgpt"}), ("memory.import.start", {"source": "no-such-source"}),
          ("timeline.get", {"limit": 5}), ("timeline.get", {"since": "not a time"}),
          ("reports.diagnostic", {"app_version": "9.9.9", "send": False}))
    scn.close(c)
    return scn.finish(tables=False)


def rpc_browsers() -> dict[str, Any]:
    scn = Scenario("browsers", "The browser and identity-profile calls without Full Disk Access (data_access is "
                   "False, as conftest sets it): nothing is discovered, so each answers from config alone or refuses.",
                   RPC + ["ferb/daemon/areas/browsers.py"])
    c = scn.conn("app")
    calls(scn, c,
          ("browsers.list", {}), ("browsers.list", {"x": 1}),
          ("browsers.installed", {"profile": "chrome/Default"}), ("browsers.installed", {}),
          ("browsers.unpair", {"profile": "chrome/Default"}), ("browsers.unpair", {}),
          ("browsers.pair", {"browser": "chrome", "folder": "Default"}), ("browsers.pair", {}),
          ("setup.browsers", {}), ("setup.browsers", {"x": 1}),
          ("setup.browser_open", {"browser": "chrome", "folder": "Default"}), ("setup.browser_open", {}),
          ("setup.browser_pair", {"browser": "chrome", "folder": "Default"}), ("setup.browser_pair", {}),
          ("setup.browsers_pair_all", {}), ("setup.browsers_pair_all", {"x": 1}),
          ("browser.show_handoff", {"handoff_id": "no-such-handoff"}), ("browser.show_handoff", {}),
          ("identity.find", {"address": "ferb@example.test"}), ("identity.find", {}),
          ("identity.choose", {"address": "ferb@example.test", "browser": "chrome", "folder": "Default"}),
          ("identity.choose", {"clear": True}), ("identity.choose", {}),
          ("identity.pair", {}), ("identity.pair", {"browser": 7}))
    s = scn.stream(c, "setup.browsers_pairing", {})
    scn.end(s)
    s = scn.stream(c, "setup.browsers_pairing", {"profiles": "x"})
    scn.end(s)
    scn.close(c)
    return scn.finish(tables=False)


def rpc_runtimes() -> dict[str, Any]:
    m = Model()
    m.real_build = True
    scn = Scenario("runtimes", "The AI settings: status, models, choosing a runtime and model, checks and sign-in. "
                   "Codex and Claude count as available (conftest) but neither has a login in the temporary home, so "
                   "the real runtime builders refuse (SignedOut) before any program runs; API keys: none (the "
                   "Keychain stand-in finds nothing).", RPC + ["ferb/daemon/areas/runtimes.py"], m)
    c = scn.conn("app")
    calls(scn, c,
          ("runtimes.status", {}), ("runtimes.status", {"runtime": 7}),
          ("runtimes.models", {"runtime": "codex"}), ("runtimes.models", {"runtime": "claude"}),
          ("runtimes.models", {"runtime": "no-such-runtime"}),
          ("runtimes.use", {"runtime": "claude", "roles": ["worker"]}),
          ("runtimes.use", {"runtime": "no-such-runtime"}), ("runtimes.use", {"runtime": "codex", "roles": "x"}),
          ("runtimes.model", {"runtime": "claude", "model": "invented-model-1"}),
          ("runtimes.model", {"runtime": "codex"}), ("runtimes.model", {"runtime": "no-such-runtime"}),
          ("runtimes.check", {"runtime": "codex"}), ("runtimes.check", {"runtime": "no-such-runtime"}),
          ("runtimes.check_calls", {"runtime": "codex"}), ("runtimes.check_calls", {"runtime": "no-such-runtime"}),
          ("runtimes.switch_task", {"task_id": "no-such-task", "runtime": "claude"}),
          ("runtimes.switch_task", {"task_id": "x", "runtime": "claude", "scope": "nope"}),
          ("runtimes.sign_in", {"runtime": "claude"}), ("runtimes.sign_in", {"runtime": "claude", "cancel": True}),
          ("runtimes.sign_in", {"runtime": "codex"}), ("runtimes.sign_in", {"runtime": "claude", "code": "x"}),
          ("runtimes.status", {}))
    scn.close(c)
    return scn.finish(tables=False)


def rpc_memory() -> dict[str, Any]:
    scn = Scenario("memory", "The owner's memory as Settings shows it: add, list, show, edit, retract, restore, "
                   "forget (asked, then confirmed), repeats, the core profile and learning.",
                   RPC + ["ferb/daemon/areas/memory.py"])
    c = scn.conn("app")
    added = scn.call(c, "memory.add", {"content": "The owner's invented cat is called Biscuit.", "kind": "fact",
                                       "subject": "owner"})
    item = (added.get("result") or {}).get("id", "no-id")
    other = scn.call(c, "memory.add", {"content": "The owner prefers tea to coffee.", "kind": "preference"})
    second = (other.get("result") or {}).get("id", "no-id")
    calls(scn, c,
          ("memory.add", {}), ("memory.list", {}), ("memory.list", {"status": "all", "history": True}),
          ("memory.show", {"subject": "owner"}), ("memory.show", {}),
          ("memory.edit", {"id": item, "content": "The owner's invented cat is called Biscuit and is grey."}),
          ("memory.edit", {"id": "no-such-item", "content": "x"}), ("memory.edit", {}),
          ("memory.retract", {"id": second, "reason": "the owner"}), ("memory.retract", {}),
          ("memory.restore", {"id": second}), ("memory.restore", {"id": "no-such-item"}),
          ("memory.forget", {"id": item}), ("memory.forget", {"id": item, "confirm": True}), ("memory.forget", {}),
          ("memory.repeats", {}), ("memory.repeats", {"x": 1}),
          ("memory.profile", {"op": "get"}), ("memory.profile", {"op": "no-such-op"}),
          ("memory.learning", {"op": "inventory"}), ("memory.learning", {"op": "no-such-op"}),
          ("memory.list", {}))
    scn.close(c)
    return scn.finish()


def history_entry(i: int, kind: str = "turn", day: str = "2026-01-01") -> dict[str, Any]:
    return {"id": f"00000000-0000-4000-8000-{i:012d}", "kind": kind, "at": f"{day}T09:0{i}:00.000000+00:00",
            "day": day, "open": kind == "turn", "body": {"text": f"invented entry {i}"}}


def rpc_chat_history() -> dict[str, Any]:
    scn = Scenario("chat_history", "FERB.app's own chat history (APP-805): put, import, page by day, days; each "
                   "refusal of a bad entry.", RPC + ["ferb/daemon/areas/chat_history.py", "ferb/store/chat_history.py"])
    c = scn.conn("app")
    calls(scn, c,
          ("chat.history.put", {"entries": [history_entry(1), history_entry(2, "reply")]}),
          ("chat.history.import", {"entries": [history_entry(2, "reply"), history_entry(3, "notice")]}),
          ("chat.history.put", {"entries": [{"id": "not-a-uuid", "kind": "turn"}]}),
          ("chat.history.put", {"entries": [1]}), ("chat.history.import", {"entries": "x"}),
          ("chat.history.page", {"day": "2026-01-01"}), ("chat.history.page", {"day": "2026-01-01", "limit": 1}),
          ("chat.history.page", {"day": "2026-01-01", "open": True}), ("chat.history.page", {"day": "yesterday"}),
          ("chat.history.days", {}), ("chat.history.days", {"x": 1}))
    scn.close(c)
    return scn.finish()


def rpc_tasks() -> dict[str, Any]:
    m = Model([planner("task", "Updating the list.", plan_task("Add bread to list.md", name="List update",
                                                                paths=["list.md"])),
               planner("task", "Looking.", plan_task("Find the invented museum's opening hours", name="Museum hours"))])
    scn = Scenario("tasks_and_records", "A finished task (it overwrote a file, so there is a recovery copy) and a "
                   "failed one; then the task list, trace, verdict, a failure report and its Send (no relay set up), "
                   "chat.status / attach / results.ack, recovery.list / restore.",
                   RPC + ["ferb/daemon/areas/rpc.py", "ferb/daemon/areas/chat.py", "ferb/daemon/reports.py",
                          "ferb/act/fs/recovery.py"], m)
    (scn.home / "workspace").mkdir(parents=True, exist_ok=True)
    (scn.home / "workspace" / "list.md").write_text("- milk\n")
    scn.note("workspace/list.md holds \"- milk\\n\"")
    m.script.workers["worker"] = [
        acting(scn, "worker", [("fs.write", {"path": "list.md", "content": "- milk\n- bread\n", "overwrite": True,
                                             "effect": "edit", "why": "add bread"})],
               worker("done", "Added bread to list.md.", artifacts=["list.md"])),
        worker("done", "The museum opens at 10am.")]
    m.script.workers["worker.verify"] = [acting(scn, "worker.verify", [read_change_records], checker(True)),
                                         checker(False, next="fail", reason="Nothing read shows the hours.")]
    chat_turn(scn, "add bread to my list")
    chat_turn(scn, "when does the museum open")
    c = scn.conn("app")
    listed = scn.call(c, "tasks.list", {})
    ids = [t["task_id"] for t in listed["result"]["tasks"]]
    failed = next(t["task_id"] for t in listed["result"]["tasks"] if t["status"] == "failed")
    done_ = next(t["task_id"] for t in listed["result"]["tasks"] if t["status"] != "failed")
    calls(scn, c, ("tasks.list", {"limit": 1}), ("tasks.list", {"limit": 0}),
          ("trace.get", {"task_id": done_}), ("trace.get", {"task_id": "no-such-task"}),
          ("verdict.set", {"task_id": done_, "verdict": "pass", "note": "looks right"}),
          ("verdict.set", {"task_id": done_, "verdict": "maybe"}),
          ("verdict.set", {"task_id": "no-such-task", "verdict": "fail"}),
          ("chat.status", {}), ("chat.attach", {}), ("chat.attach", {"new": True}),
          ("results.ack", {"task_ids": ids}), ("results.ack", {"task_ids": "x"}), ("chat.status", {}),
          ("task.stop", {"task_ids": [done_]}))
    built = scn.call(c, "reports.build", {"task_id": failed, "app_version": "9.9.9"})
    path = (built.get("result") or {}).get("path", "")
    calls(scn, c, ("reports.build", {"task_id": "no-such-task"}),
          ("reports.send", {"path": path, "title": "Invented failure", "app_version": "9.9.9"}),
          ("reports.send", {"path": "/etc/hosts"}))
    rec = scn.call(c, "recovery.list", {})
    records = (rec.get("result") or {}).get("records", [])
    if records:
        scn.call(c, "recovery.restore", {"id": records[0]["id"]})
    calls(scn, c, ("recovery.restore", {"id": "no-such-record"}), ("recovery.list", {"x": 1}))
    scn.close(c)
    return scn.finish()


def rpc_questions() -> dict[str, Any]:
    m = Model([planner("task", "Adding the alias.", plan_task("Add the alias ll to ~/.zshrc", name="Shell alias",
                                                               paths=["~/.zshrc"]))])
    scn = Scenario("questions", "The answers to FERB's questions: a confirmation's box takes a correction "
                   "(chat.correct, GATE-510), chat.approve and chat.answer on questions that aren't waiting, and "
                   "each refusal.", RPC + ["ferb/daemon/areas/chat.py:191-289"], m)
    write = ("fs.write", {"path": "~/.zshrc", "content": "alias ll='ls -l'\n", "overwrite": True, "effect": "edit",
                          "why": "add the alias"})
    m.script.workers["worker"] = [acting(scn, "worker", [write], worker("not_done", "", reason="The owner changed "
                                                                       "the request; the write didn't run.")),
                                  worker("done", "Added the alias to ~/.bashrc instead.")]
    m.script.workers["worker.verify"] = [checker(None, next="retry", missing="The alias isn't added.",
                                                 instructions="Do what the owner said instead."), checker(True)]
    turn = scn.stream(scn.conn("chat"), "chat.send", {"session_id": "s1", "text": "add an alias ll to my zshrc"})
    scn.wait("an approval is pending", lambda: bool(pending_approvals(scn)))
    scn.settle()
    c = scn.conn("app")
    ap = scn.call(c, "chat.status", {})["result"]["approvals"][0]["approval_id"]
    calls(scn, c, ("chat.correct", {"approval_id": ap, "text": ""}),
          ("chat.answer", {"approval_id": ap, "text": "sure thing"}),
          ("chat.correct", {"approval_id": ap, "text": "put it in ~/.bashrc instead"}),
          ("chat.correct", {"approval_id": ap, "text": "again"}),
          ("chat.approve", {"approval_id": ap, "granted": True, "choice": 1}),
          ("chat.approve", {"approval_id": "no-such-question", "granted": True}),
          ("chat.answer", {"approval_id": "no-such-question", "text": "x"}), ("chat.approve", {}),
          ("chat.correct", {}))
    scn.end(turn)
    scn.settle()
    scn.call(c, "chat.status", {})
    scn.close(c)
    return scn.finish()


def rpc_time() -> dict[str, Any]:
    sched = {"request": "Water the invented plants", "name": "Plant watering",
             "when": {"in_minutes": None, "in_days": None, "day": "tomorrow", "date": None, "time": "08:00"},
             "repeat": "daily", "weekday": None, "every_minutes": None, "times": [], "random": [],
             "quote": "every morning at 8", "targets": {"apps": [], "sites": [], "paths": [], "recipients": []},
             "standing_sends": [], "watch": "none", "until": None, "when_active": False, "folders": [],
             "identity": "owner", "worker_type": "default", "profile": "", "account": ""}
    reminder = {"when": {"in_minutes": 30, "in_days": None, "day": None, "date": None, "time": None},
                "message": "Call the invented plumber", "quote": "in 30 minutes", "when_active": False,
                "name": "Plumber", "kind": "reminder"}
    timer = {"when": {"in_minutes": 10, "in_days": None, "day": None, "date": None, "time": None},
             "message": "Tea is ready", "quote": "a 10 minute timer", "when_active": False, "name": "Tea",
             "kind": "timer"}
    m = Model([planner("reply", "Done: a daily schedule, a reminder and a timer.",
                       side_entries=side(schedules=[sched], timers=[reminder, timer]))])
    scn = Scenario("schedules_reminders_timers", "A schedule, a reminder and a timer made in one chat turn; then "
                   "every call the app makes about them, each with a refusal.",
                   RPC + ["ferb/daemon/areas/schedules.py:822-990"], m)
    chat_turn(scn, "water the plants every morning at 8, remind me in 30 minutes to call the plumber, and set a "
                   "10 minute timer for tea")
    c = scn.conn("app")
    listed = scn.call(c, "schedules.list", {})
    sid = (listed.get("result") or {}).get("schedules", [{}])[0].get("id", "no-such-schedule")
    pending = scn.call(c, "reminders.list", {})["result"]["pending"]
    rid = next((r["id"] for r in pending if r["kind"] == "reminder"), "no-such-reminder")
    tid = next((r["id"] for r in pending if r["kind"] == "timer"), "no-such-timer")
    calls(scn, c, ("schedules.list", {"x": 1}),
          ("schedules.change", {"id": sid, "action": "pause"}), ("schedules.change", {"id": sid, "action": "resume"}),
          ("schedules.change", {"id": "no-such-schedule", "action": "cancel"}), ("schedules.change", {}),
          ("timers.states", {"ids": [rid, tid]}), ("timers.states", {"ids": "x"}),
          ("timers.repeat", {"id": tid}), ("timers.repeat", {"id": "no-such-timer"}),
          ("reminders.cancel", {"id": rid}), ("reminders.cancel", {"id": "no-such-reminder"}),
          ("reminders.cancel", {}), ("schedules.change", {"id": sid, "action": "cancel"}),
          ("schedules.list", {}), ("reminders.list", {}), ("reminders.list", {"x": 1}))
    scn.advance(600)
    scn.settle()
    calls(scn, c, ("timers.states", {"ids": [tid]}), ("timers.repeat", {"id": tid}), ("timers.repeat", {"id": tid}))
    scn.close(c)
    return scn.finish()


def rpc_streams() -> dict[str, Any]:
    m = Model([planner("task", "Here's how I'd do it.", plan_task("Find the three largest invented files in the "
                                                                  "workspace", name="Largest files"))])
    scn = Scenario("streams_and_processes", "The streaming calls and FERB's processes: logs.subscribe (a filter and "
                   "a refused one), notifications.subscribe, intro.preview (the real planner turn alone, ORCH-500), "
                   "ps.* with no process running.", RPC + ["ferb/daemon/areas/rpc.py:587 _logs",
                                                           "ferb/daemon/areas/notify.py:135",
                                                           "ferb/daemon/areas/intro.py:131",
                                                           "ferb/daemon/areas/egress.py:106-141"], m)
    c = scn.conn("app")
    drain_stream(scn, "logs", "logs.subscribe", {"filters": ["policy", "timers"]},
                 lambda: (scn.call(c, "stop.engage", {}), scn.call(c, "stop.lift", {})))
    scn.end(scn.stream(c, "logs.subscribe", {"filters": ["no-such-filter"]}))
    drain_stream(scn, "notifications", "notifications.subscribe", {"authorized": True})
    scn.end(scn.stream(c, "intro.preview", {"request": "find my three largest files"}))
    scn.end(scn.stream(c, "intro.preview", {}))
    scn.end(scn.stream(c, "ps.attach", {"id": "no-such-process"}))
    calls(scn, c, ("ps.list", {}), ("ps.kill", {"id": "no-such-process"}), ("ps.kill", {}),
          ("ps.send", {"id": "no-such-process", "text": "y"}), ("ps.send", {}))
    scn.close(c)
    return scn.finish(tables=False)


def rpc_mcp() -> dict[str, Any]:
    m = Model([planner("task", "Looking at the workspace.", plan_task("List the invented workspace", name="List"))],
              {"worker.verify": checker(True)})
    scn = Scenario("mcp_shim", "The tool bridge's role (mcp_shim) with the run token a worker's call was given: ping, "
                   "tools.list, tools.call (a tool that runs, an unknown tool, invalid arguments), a cli method "
                   "refused; the same token after the task ended.", RPC + ["ferb/daemon/areas/chat.py:181-189",
                                                                          "ferb/daemon/gateway.py:735 Gateway.call"],
                   m)
    gate = Gate(scn, "worker", worker("done", "The workspace is empty."))
    m.script.workers["worker"] = gate.answer
    turn = scn.stream(scn.conn("chat"), "chat.send", {"session_id": "s1", "text": "what's in the workspace"})
    scn.wait_model("worker")
    record = [r for r in m.calls if r["key"] == "worker"][-1]
    token = on_loop(scn.host, lambda: next(iter(TOKEN_OF.get(record["task_id"], [""]))))
    c = scn.conn("mcp", role="mcp_shim", token=token)
    calls(scn, c, ("ping", {}), ("tools.list", {}),
          ("tools.call", {"name": "fs.list", "arguments": {"path": "."}}),
          ("tools.call", {"name": "no.such_tool", "arguments": {}}),
          ("tools.call", {"name": "fs.read", "arguments": {"path": 7}}),
          ("tools.call", {"name": "fs.read"}), ("chat.status", {}))
    scn.close(c)
    gate.open(scn)
    scn.end(turn)
    scn.settle()
    c = scn.conn("mcp-after", role="mcp_shim", token=token)
    scn.close(c)
    return scn.finish(tables=False)


def rpc_update() -> dict[str, Any]:
    m = Model([planner("task", "Watching.", plan_task("Wait for the invented build", name="Build watch"))],
              {"worker": STALL})
    scn = Scenario("update_prepare", "update.prepare while a task runs (not ready: it names the task), then with "
                   "even_if_busy (ready; the daemon stops taking work and shuts its service down); a chat message "
                   "after it is refused.", RPC + ["ferb/daemon/areas/rpc.py:112 _update_prepare"], m)
    scn.stream(scn.conn("chat"), "chat.send", {"session_id": "s1", "text": "tell me when the build is done"})
    scn.wait_model("worker")
    c = scn.conn("app")
    calls(scn, c, ("update.prepare", {"version": "9.9.9"}), ("update.prepare", {"version": "9.9.9",
                                                                                 "even_if_busy": True}))
    scn.end(scn.stream(c, "chat.send", {"session_id": "s1", "text": "hello"}))
    scn.close(c)
    scn.note("the daemon then shuts down (as update.prepare's stop does in the service); the task's stream ends")
    return scn.finish(tables=False)


RPC_SCENARIOS: list[Callable[[], dict[str, Any]]] = [
    rpc_protocol, rpc_setup_and_about, rpc_browsers, rpc_runtimes, rpc_memory, rpc_chat_history, rpc_tasks,
    rpc_questions, rpc_time, rpc_streams, rpc_mcp, rpc_update]


def coverage(scenarios: list[dict[str, Any]]) -> dict[str, Any]:
    """Which methods of auth.METHODS were called, per role, with a result and with an error."""
    from ferb.daemon.auth import METHODS, Role
    seen: dict[str, dict[str, set[str]]] = {}
    for scn in scenarios:
        roles: dict[str, str] = {}
        sent: dict[tuple[str, Any], str] = {}
        streaming: dict[str, str] = {}
        for st in scn["steps"]:
            for ln in st.get("lines", []):
                ln = {k: BODIES[v["sha256"]] if isinstance(v, dict) else v for k, v in ln.items()}
                if "send" in ln:
                    try:
                        msg = json.loads(ln["send"])
                    except ValueError:
                        continue
                    if not isinstance(msg, dict) or "method" not in msg:
                        continue
                    if msg["method"] == "auth.hello":
                        roles[st["conn"]] = str((msg.get("params") or {}).get("role"))
                    sent[(st["conn"], msg.get("id"))] = msg["method"]
                    streaming[st["conn"]] = msg["method"]
                elif "recv" in ln:
                    msg = json.loads(ln["recv"])
                    if str(msg.get("method", "")).endswith(".event"):
                        seen.setdefault(roles.get(st["conn"], "?"), {}).setdefault(
                            msg["method"][:-len(".event")], set()).add("result")
                        continue
                    method = sent.get((st["conn"], msg.get("id")))
                    if method is None or "method" in msg:
                        continue
                    outcome = "error" if "error" in msg else "result"
                    seen.setdefault(roles.get(st["conn"], "?"), {}).setdefault(method, set()).add(outcome)
    out: dict[str, Any] = {}
    for role in Role:
        table = sorted(METHODS[role])
        got = seen.get(role.value, {})
        out[role.value] = {"methods": len(table), "called": sum(1 for m in table if m in got),
                           "with_result": sorted(m for m in table if "result" in got.get(m, ())),
                           "with_error": sorted(m for m in table if "error" in got.get(m, ())),
                           "not_called": sorted(m for m in table if m not in got)}
    return out


def write(name: str, value: Any) -> int:
    text = fx(json.dumps(value, ensure_ascii=False, indent=1)) + "\n"
    (HERE / name).write_text(text, encoding="utf-8")
    return len(text.encode("utf-8"))


def main() -> dict[str, int]:
    only = set(sys.argv[1:])
    chats = []
    for fn in CHAT_SCENARIOS:
        if only and fn.__name__ not in only:
            continue
        t0 = time.monotonic()
        try:
            chats.append(fn())
        except Exception as e:                             # noqa: BLE001
            import traceback
            traceback.print_exc()
            PROBLEMS.append(f"{fn.__name__}: {type(e).__name__}: {e}")
            for h in list(LIVE):
                with contextlib.suppress(Exception):
                    h.stop()
                LIVE.remove(h)
        print(f"{fn.__name__}: {time.monotonic() - t0:.1f}s", file=sys.stderr)
    rpcs = []
    for fn in RPC_SCENARIOS:
        if only and fn.__name__ not in only:
            continue
        t0 = time.monotonic()
        try:
            rpcs.append(fn())
        except Exception as e:                             # noqa: BLE001
            import traceback
            traceback.print_exc()
            PROBLEMS.append(f"{fn.__name__}: {type(e).__name__}: {e}")
            for h in list(LIVE):
                with contextlib.suppress(Exception):
                    h.stop()
                LIVE.remove(h)
        print(f"{fn.__name__}: {time.monotonic() - t0:.1f}s", file=sys.stderr)
    sizes = {}
    if rpcs:
        sizes["rpc_transcripts.json"] = write("rpc_transcripts.json", {"coverage": coverage(rpcs + chats),
                                                                       "scenarios": rpcs})
    if chats:
        sizes["chat_scenarios.json"] = write("chat_scenarios.json", {"scenarios": chats})
    sizes["bodies.json"] = write("bodies.json", dict(sorted(BODIES.items())))
    if PROBLEMS:
        raise SystemExit("problems:\n" + "\n".join(PROBLEMS))
    return sizes


if __name__ == "__main__":
    try:
        done = main()
    finally:
        MP.undo()
        shutil.rmtree(ROOT, ignore_errors=True)
    for name, size in done.items():
        print(f"{name}: {size} bytes")
