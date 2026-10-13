import asyncio
import json
import plistlib
import re
import shutil
import socket
import subprocess
import tempfile
import threading
import time
from datetime import datetime, timedelta
from pathlib import Path
from typing import Any, AsyncIterator, Callable, Iterator

import anyio
import pytest
from pydantic import BaseModel

from ferb.cli.client import Client
from ferb.clock.clock import FakeClock
from ferb.core.ids import SequentialIds
from ferb.core.types import (ActionSpec, Identity, RiskLevel, ModelRequest, RunHandle, RuntimeEvent, Capabilities, Target,
                             Task, ToolContext, ToolResult)
from ferb.daemon.main import Daemon
from ferb.orchestrator.tree import TaskRecord
from ferb.policy.targets import Request, Targets
from ferb.runtimes.claude_sdk import ClaudeRuntime
from ferb.runtimes.codex_exec import CodexRuntime
from tests.support import egress
from tests.support.confirm import FerbAnswers

pytest_plugins = ["tests.browserkit"]           # browser fixtures: local pages, temporary keychain


class RunningDaemon:
    """A real Daemon on its own event loop in a background thread (the tests talk to it over the socket)."""

    def __init__(self, home: Path, **kwargs: Any) -> None:
        self.loop = asyncio.new_event_loop()
        self.thread = threading.Thread(target=self.loop.run_forever, daemon=True)
        self.thread.start()
        self.daemon: Daemon = self.run(self._make(home, kwargs))
        # Offline: never run `security` against the real Keychain. Tests that need others patch it themselves.
        self.daemon._runtime_available = lambda name: name in ("codex", "claude")   # type: ignore[method-assign]
        self.run(self.daemon.start())

    @staticmethod
    async def _make(home: Path, kwargs: dict[str, Any]) -> Daemon:
        kwargs.setdefault("watchers", False)            # the owner's real activity stays out of test trails
        kwargs.setdefault("distill", False)             # the daily distillation calls a model
        kwargs.setdefault("retention", False)           # tests purge explicitly, never on a timer
        kwargs.setdefault("notifications", False)       # no macOS notifications from tests
        kwargs.setdefault("snapshots", False)           # never a real APFS snapshot (tmutil) from tests
        kwargs.setdefault("persona_scan", None)         # never scan the owner's apps from disposable daemons
        return Daemon(home=home, **kwargs)

    def run(self, coro: Any, timeout: float = 10) -> Any:
        return asyncio.run_coroutine_threadsafe(coro, self.loop).result(timeout=timeout)

    def call_soon(self, fn: Callable[..., Any], *args: Any) -> None:
        """Run fn on the daemon's loop (the bus and the clock belong to it)."""
        done = threading.Event()
        self.loop.call_soon_threadsafe(lambda: (fn(*args), done.set()))
        done.wait(5)

    @property
    def sock(self) -> Path:
        return self.daemon.server.path

    def stop(self) -> None:
        self.run(self.daemon.shutdown())
        self.loop.call_soon_threadsafe(self.loop.stop)
        self.thread.join(5)


@pytest.fixture
def home() -> Iterator[Path]:
    # Short path: AF_UNIX socket paths are limited to 104 bytes on macOS (pytest's tmp_path is too long).
    d = Path(tempfile.mkdtemp(prefix="ferbt-"))
    yield d / "home"
    shutil.rmtree(d, ignore_errors=True)


@pytest.fixture
def daemon(home: Path, monkeypatch: pytest.MonkeyPatch) -> Iterator[RunningDaemon]:
    # Offline tool fixtures supply synthetic image bytes. OCR has its own injected-region regressions; this
    # fixture never invokes platform recognition against fake pictures or silently depends on installed Vision.
    monkeypatch.setattr('ferb.sanitize.images.recognize_text', lambda _image: [])
    d = RunningDaemon(home, clock=FakeClock(), ids=SequentialIds("id"), watch_owner_input=False)
    d.daemon._permissions = lambda: {"accessibility": True, "screen": True}   # type: ignore[method-assign]
    egress.install(getattr(d.daemon, "proxy", None))   # the proxy resolves tests' local origin name (support/egress)
    sup = d.daemon.supervisor                   # offline: fake tools count as usable (test_availability tests the real)
    if sup is not None:
        sup.availability = sup._default_availability
    yield d
    d.stop()


@pytest.fixture
def ferb_yes(daemon: RunningDaemon) -> Iterator[FerbAnswers]:
    """FERB's own questions (the code floor, the sink rule) answered Yes as they come, for tests about something the
    call passes through on the way (provenance, P19, recovery). Strict: the test declares each question it expects
    (`ferb_yes.expect(rule, n)`), and teardown fails on any other or any missing one."""
    with FerbAnswers(daemon) as answers:
        yield answers
    answers.verify()


def real_codex_login() -> Path | None:
    """FERB's own Codex login (`ferb setup --codex-login`), when the owner has one; never the owner's personal ~/.codex."""
    login = Path.home() / ".ferb" / "codex-home"
    return login if (login / "auth.json").exists() else None


@pytest.fixture(autouse=True)
def no_real_models(request: pytest.FixtureRequest, monkeypatch: pytest.MonkeyPatch) -> None:
    """Offline tests must never reach a real model (cost, flakiness): only `-m real` tests may. Live tests run daemons
    in temporary homes, so they borrow FERB's own Codex login (Codex refuses to run without one, F2-R11)."""
    if request.node.get_closest_marker("real"):
        import ferb.runtimes.codex_exec as cx
        login = real_codex_login()
        if login is not None:
            monkeypatch.setattr(cx, "codex_home", lambda ferb_home: login)
        return

    async def refuse(*a: Any, **k: Any) -> None:
        raise AssertionError("an offline test tried to start a real model runtime; mark it `real` or use a fake")
    monkeypatch.setattr(CodexRuntime, "start", refuse)
    monkeypatch.setattr(ClaudeRuntime, "start", refuse)


def pytest_collection_modifyitems(config: Any, items: list[Any]) -> None:
    """Claude-runtime real tests run only when the owner asks for them by name (-m real_claude); -m real is Codex only."""
    # Unmarked real-surface files are still live-only; keep explicit Claude/API selections separate.
    for item in items:
        if '_real' in item.path.stem and not any(item.get_closest_marker(name) for name in ('real', 'real_claude', 'real_api')):
            item.add_marker(pytest.mark.real)
    if "real_claude" in (config.getoption("-m") or ""):
        return
    keep = [i for i in items if i.get_closest_marker("real_claude") is None]
    dropped = [i for i in items if i.get_closest_marker("real_claude") is not None]
    if dropped:
        config.hook.pytest_deselected(items=dropped)
        items[:] = keep


def only_tools(d: "RunningDaemon", *keep: str) -> None:
    """Real tests are locked down (CLAUDE.md): the test daemon keeps only the tools the test needs — names or name
    prefixes ("fs.", "mac.apps"). Mac, shell and AppleScript tools only when the test is about them."""
    caps = d.daemon.registry.capabilities
    for name in [n for n in caps if not any(n == k or (k.endswith(".") and n.startswith(k)) for k in keep)]:
        del caps[name]


def without_owner_mac(d: "RunningDaemon") -> None:
    """Browser tests: everything but the owner's Mac, shell and AppleScript."""
    only_tools(d, *{n.split(".")[0] + "." for n in d.daemon.registry.capabilities
                    if n.split(".")[0] not in ("mac", "shell", "applescript")})


class Answers:
    """Answers FERB's approval questions as they appear (the owner at `ferb chat`), recording each question."""

    def __init__(self, d: RunningDaemon, granted: bool = True, choice: int | None = None) -> None:
        self.d, self.granted, self.choice, self.asked = d, granted, choice, []
        self._task = asyncio.run_coroutine_threadsafe(self._loop(), d.loop)

    async def _loop(self) -> None:
        approvals = self.d.daemon.state.approvals
        while True:
            for ap in approvals.pending():
                self.asked.append(ap)
                if ap.options:                                   # "Which Rahul?": the owner's pick (or cancel)
                    approvals.resolve(ap.id, self.choice is not None, self.choice)
                else:
                    approvals.resolve(ap.id, self.granted)
            await asyncio.sleep(0.02)

    def stop(self) -> None:
        self._task.cancel()


def raw(sock_path: Path) -> tuple[socket.socket, "socket.SocketIO"]:
    s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    s.settimeout(5)
    s.connect(str(sock_path))
    return s, s.makefile("r")


def send(s: socket.socket, obj: object) -> None:
    s.sendall(json.dumps(obj).encode() + b"\n")


def trail_row(d: RunningDaemon, day: str, actor: str = "owner", source: str = "browser_tab",
              data: dict[str, Any] | None = None, hour: int = 10) -> str:
    rid = f"t-{day}-{actor}-{source}-{hour}-{len(data or {})}-{id(data)}"
    at = datetime.fromisoformat(f"{day}T{hour:02d}:00:00").astimezone()

    async def go() -> str:
        d.daemon.db.conn.execute(                                   # type: ignore[union-attr]
            "INSERT INTO trail (id, at, day, device_id, actor, source, data_json) VALUES (?, ?, ?, 'dev', ?, ?, ?)",
            (rid, at.isoformat(), day, actor, source, json.dumps(data or {"host": "github.com", "browser": "chrome",
                                                                           "profile": "Default", "url": "https://github.com/"})))
        return rid
    return d.run(go())


class FakeDistillRuntime:
    def __init__(self, answer: Any) -> None:
        self.answer, self.contexts = answer, []

    async def start(self, task: Any, request: ModelRequest, token: str) -> RunHandle:
        self.contexts.append(request.context)
        return RunHandle("r", "fake", task.id, {})

    async def events(self, run: RunHandle) -> Any:
        if isinstance(self.answer, Exception):
            raise self.answer
        yield RuntimeEvent("final", {"text": json.dumps(self.answer)})

    async def cancel(self, run: RunHandle) -> None:
        return None


def make_app(root: Path, name: str, bundle_id: str | None, version: str = "1.0", binary: bool = False,
             sdef: bool = False, **extra: object) -> Path:
    b = root / f"{name}.app"
    (b / "Contents" / "Resources").mkdir(parents=True)
    info: dict[str, object] = {"CFBundleName": name, "CFBundleShortVersionString": version, **extra}
    if bundle_id:
        info["CFBundleIdentifier"] = bundle_id
    (b / "Contents" / "Info.plist").write_bytes(plistlib.dumps(info, fmt=plistlib.FMT_BINARY if binary
                                                               else plistlib.FMT_XML))
    if sdef:
        (b / "Contents" / "Resources" / f"{name}.sdef").write_text("<dictionary/>")
    return b


class FakeRunner:
    """Records commands; simulates uv installing Python/venv and launchctl load state."""

    def __init__(self, home: Path) -> None:
        self.home, self.cmds, self.loaded = home, [], False

    def __call__(self, cmd: list[str]) -> subprocess.CompletedProcess[str]:
        self.cmds.append(cmd)
        out = ""
        if cmd[:3] == ["uv", "python", "install"]:
            py = self.home / "python" / "cpython-3.12.99-macos-aarch64-none" / "bin" / "python3.12"
            py.parent.mkdir(parents=True, exist_ok=True)
            py.write_text("#!/bin/sh\n")
        elif cmd[:2] == ["uv", "venv"]:
            vpy = Path(cmd[-1]) / "bin" / "python"
            vpy.parent.mkdir(parents=True, exist_ok=True)
            vpy.unlink(missing_ok=True)
            vpy.symlink_to(cmd[cmd.index("--python") + 1])
        elif cmd[:2] == ['xcrun', 'swiftc']:
            Path(cmd[cmd.index('-o') + 1]).write_bytes(b'fake helper binary')
        elif cmd[:3] == ['/usr/bin/codesign', '-d', '-r-']:
            out = 'designated => identifier "com.ferb.secrets" and certificate leaf = H"fake-test-certificate"\n'
        elif cmd[:2] == ["launchctl", "bootstrap"]:
            self.loaded = True
        elif cmd[:2] == ["launchctl", "bootout"]:
            self.loaded = False
        elif cmd[:2] == ["launchctl", "print"]:
            return subprocess.CompletedProcess(cmd, 0 if self.loaded else 113, "pid = 4242\n" if self.loaded else "")
        return subprocess.CompletedProcess(cmd, 0, out, "")

    def ran(self, *prefix: str) -> int:
        return sum(1 for c in self.cmds if c[:len(prefix)] == list(prefix))


# Scripted model/task support shared by orchestration tests.

STALL = "__stall__"


def cond(label: str, kind: str = "none", **k: Any) -> dict[str, Any]:
    return {"text": label, "check": {"kind": kind, "path": k.get("path"), "text": k.get("text"),
                                    "url_contains": k.get("url_contains"), "site": k.get("site"),
                                    "expected": k.get("expected")}}


def task(goal: str = "the goal", identity: str = "ferb", apps: tuple[str, ...] = (), sites: tuple[str, ...] = (),
         paths: tuple[str, ...] = (), recipients: tuple[str, ...] = (),
         conds: tuple[dict[str, Any], ...] | None = None) -> dict[str, Any]:
    """The supervisor's task, as the model returns it."""
    return {"goal": goal, "identity": identity,
            "targets": {"apps": list(apps), "sites": list(sites), "paths": list(paths), "recipients": list(recipients)},
            "success_conditions": list(conds) if conds is not None else [cond(goal)]}


def sup(kind: str, text: str, t: dict[str, Any] | None = None) -> str:
    return json.dumps({"kind": kind, "text": text, "task": t})


def done(summary: str = "ok") -> str:
    return json.dumps({"status": "done", "summary": summary, "artifacts": [], "evidence": []})


class Script:
    def __init__(self, supervisor: list[Any], workers: dict[str, Any]) -> None:
        self.sup, self.workers = supervisor, workers
        self.contexts: list[tuple[str, str, str, list[str]]] = []      # (mode, key, context, tool names)
        self.requests: list[tuple[str, str, Any]] = []                  # (mode, key, ModelRequest)
        self.times: dict[str, tuple[float, float]] = {}
        self.fill_report_verdict = True       # complete older scripted verdicts (complete_report_verdict)

    def step(self, key: str) -> Any:
        # Most lifecycle tests are not about recovery policy. Their scripted orchestrator escalates the worker's
        # report; recovery/authorization tests supply their own explicit decisions to exercise other choices.
        if key not in self.workers and key.endswith('.question'):
            return json.dumps({'ask_owner': True, 'answer': '', 'question': ''})
        if key not in self.workers and key.endswith('.dispatch'):
            ctx = next(c for _, k, c, _ in reversed(self.contexts) if k == key)
            raw = ctx.split('Worker report (data): ', 1)[1]
            report, _ = json.JSONDecoder().raw_decode(raw[raw.index('{'):])
            return json.dumps({'action': 'ask' if report['status'] == 'needs_user' else 'finish',
                               'question': report['summary'], 'instructions': report['summary'],
                               'worker_type': 'default'})
        if key not in self.workers and key.endswith('.verify'):
            ctx = next(c for _, k, c, _ in reversed(self.contexts) if k == key)
            indices = re.findall(r'^([0-9]+)\. ', ctx, re.M)
            return json.dumps({'results': [{'index': int(i), 'met': True,
                                           'evidence': 'Scripted inspection for lifecycle test'} for i in indices]})
        w = self.workers[key]
        return w.pop(0) if isinstance(w, list) else w


# WARNING: every scripted .verify step gets met=True for FERB's report condition unless the test sets
# script.fill_report_verdict = False; tests about the report condition must opt out.
def complete_report_verdict(script: "Script", context: str, final: str) -> str:
    """Scripted checker verdicts written before FERB added its report condition (F4 live fixes) answer only the
    planner's conditions. FERB numbers its report condition last in the checker's Conditions list; a verdict that
    doesn't answer it gets met=True for it, so those tests keep testing what they did. A test about the report
    condition answers it itself, or sets script.fill_report_verdict = False to leave it unanswered."""
    from ferb.orchestrator.supervisor import REPORT_CONDITION
    found = re.findall(r"^([0-9]+)\. " + re.escape(REPORT_CONDITION.text) + r"$", context, re.M)
    if not script.fill_report_verdict or not found:
        return final
    try:
        verdict = json.loads(final)
    except ValueError:
        return final
    results = verdict.get("results") if isinstance(verdict, dict) else None
    index = int(found[-1])                    # numbered last: a planner condition in the same words comes first
    if not isinstance(results, list) or any(isinstance(r, dict) and r.get("index") == index for r in results):
        return final
    results.append({"index": index, "met": True, "evidence": "Scripted: the report matches the records.", "how": None})
    return json.dumps(verdict)


class FakeRuntime:
    name = "fake"
    capabilities = Capabilities("direct", "prefix", False, "native", False, False, False, False, "api")

    def __init__(self, script: Script, mode: str, tools: list[str]) -> None:
        self.script, self.mode, self.tools = script, mode, tools

    async def start(self, task: Task, request: ModelRequest, run_token: str) -> RunHandle:
        key = task.id.split(".", 1)[1] if "." in task.id else "supervisor"   # worker / worker.verify / …
        self.script.contexts.append((self.mode, key, request.context, self.tools))
        self.script.requests.append((self.mode, key, request))
        return RunHandle(f"run-{task.id}", self.name, task.id, {"token": run_token, "key": key, "cancel": False,
                                                                 "context": request.context})

    async def events(self, run: RunHandle) -> AsyncIterator[RuntimeEvent]:
        key = run.state["key"]
        step = self.script.sup.pop(0) if key == "supervisor" else self.script.step(key)
        t0 = time.monotonic()
        if isinstance(step, RuntimeEvent):
            yield step
            return
        if step == STALL:
            while not run.state["cancel"]:
                await asyncio.sleep(0.02)
            return
        if callable(step):
            final = await anyio.to_thread.run_sync(step, run.state["token"])
        else:
            final = step
        if key.endswith(".verify") and isinstance(final, str):
            final = complete_report_verdict(self.script, run.state["context"], final)
        yield RuntimeEvent("usage", {"input_tokens": 10, "output_tokens": 5, "cost_usd": 0.001})
        self.script.times[key] = (t0, time.monotonic())
        yield RuntimeEvent("final", {"text": final})

    async def continue_(self, run: RunHandle, message: str) -> None:
        raise NotImplementedError

    async def cancel(self, run: RunHandle) -> None:
        run.state["cancel"] = True


# WARNING: scripts made here fill met=True for FERB's report condition in every scripted .verify step unless the test
# sets script.fill_report_verdict = False (see complete_report_verdict); tests about the report condition must opt out.
def install(d: RunningDaemon, supervisor: list[Any], workers: dict[str, Any] | None = None) -> Script:
    script = Script(supervisor, workers or {})
    d.daemon.supervisor.make_runtime = lambda role, *, mode, tool_names, schema: FakeRuntime(script, mode, tool_names)
    return script


def chat(d: RunningDaemon, text: str, session: str = "s1", timeout: float = 30) -> list[dict[str, Any]]:
    """One chat turn's events after the daemon's acknowledgement, which must come first (L7)."""
    c = Client(d.sock, timeout=timeout)
    try:
        return turn_events(list(c.stream("chat.send", {"session_id": session, "text": text})))
    finally:
        c.close()


def turn_events(events: list[dict[str, Any]]) -> list[dict[str, Any]]:
    """A chat.send stream as the turn's own events: checks the event contract (L7: the acknowledgement first, every
    event naming the acknowledged request_id and when it was sent; an observation outside any task names none. L8:
    each live line names its task and when its tool call started), then drops the acknowledgement, the live lines and
    those two fields, so tests about what a turn did compare its events directly. tests/test_chat_events.py and
    tests/test_progress_line.py cover the rest."""
    assert events and events[0]["type"] == "ack", events

    def outside(e: dict[str, Any]) -> bool:
        return e["type"] == "event" and e["topic"].startswith("action.") and not e["data"].get("task_id")
    assert all(e.get("request_id") == (None if outside(e) else events[0]["request_id"]) and e.get("at")
               for e in events), events
    assert all(e.get("task_id") and e.get("text") and e.get("started_at") for e in events if e["type"] == "progress")
    return [{k: v for k, v in e.items() if k not in ("request_id", "at")} for e in events[1:] if e["type"] != "progress"]


def tool(sock: Any, token: str, name: str, **args: Any) -> dict[str, Any]:
    c = Client(sock, role="mcp_shim", token=token)
    try:
        return c.call("tools.call", {"name": name, "arguments": args})       # type: ignore[no-any-return]
    finally:
        c.close()


def outcome(evs: list[dict[str, Any]]) -> dict[str, Any]:
    return next(e for e in evs if e["type"] == "outcome")


def writes(sock: Any, path: str, text: str) -> Callable[[str], str]:
    def act(token: str) -> str:
        r = tool(sock, token, "fs.write", path=path, content=text, overwrite=True, effect="edit")
        return json.dumps({"status": "failed" if r["is_error"] else "done", "summary": f"wrote {path}",
                           "artifacts": [path], "evidence": []})
    return act


class FakeCap:
    identities = {Identity.OWNER, Identity.FERB}
    risk, reversible = RiskLevel.MEDIUM, False

    class Input(BaseModel):
        to: str = ""
        site: str = ""
        text: str = ""

    def __init__(self, name: str, kind: str, scopes: set[str], boom: bool = False,
                 reads: tuple[tuple[str, str], ...] = (), content: dict[str, Any] | None = None) -> None:
        self.name, self.description, self.kind, self.scopes = name, name, kind, scopes
        self.boom, self.reads, self.content, self.calls = boom, reads, content or {"ok": True}, 0

    def describe(self, args: Any) -> ActionSpec:
        return ActionSpec(self.kind, Target(recipients=(args.to,) if args.to else (), site=args.site or None),
                          content=args.text or None)

    async def run(self, args: Any, ctx: ToolContext) -> ToolResult:
        self.calls += 1
        if self.boom:
            raise ConnectionError("lost the acknowledgement")
        return ToolResult(self.content, read_sources=self.reads)


class GatewayCap:
    identities = {Identity.OWNER}
    risk, reversible = RiskLevel.MEDIUM, False

    class Input(BaseModel):
        to: str = "alice@x.com"
        text: str = "hi"

    def __init__(self, name: str, kind: str, scopes: set[str], boom: bool = False,
                 reads: tuple[tuple[str, str], ...] = (), site: str | None = None) -> None:
        self.name, self.description, self.kind, self.scopes = name, f"fake {kind}", kind, scopes
        self.boom, self.reads, self.site, self.calls = boom, reads, site, 0

    def describe(self, args: BaseModel) -> ActionSpec:
        to = getattr(args, "to")
        return ActionSpec(self.kind, Target(recipients=(to,) if to else (), site=self.site), content=getattr(args, "text"))

    async def run(self, args: BaseModel, ctx: ToolContext) -> ToolResult:
        self.calls += 1
        if self.boom:
            raise ConnectionError("lost ack after Send")
        return ToolResult({"ok": True, "n": self.calls}, read_sources=self.reads)


def gateway_request(words: str = "", **targets: object) -> Request:
    """The facts the gateway keeps about a request: the owner's own words and the targets the model understood."""
    return Request("req-1", words, Targets.of(**targets))   # type: ignore[arg-type]


def gateway_setup(d: RunningDaemon, scopes: set[str], req: Request | None, caps: list[GatewayCap],
          identity: Identity = Identity.OWNER):
    for c in caps:
        d.daemon.registry.capabilities[c.name] = c
    task = Task("task-1", None, "g", "dev", identity, scopes, "worker", "req-1", [],
                d.daemon.clock.now() + timedelta(minutes=10))

    async def go():
        root = d.daemon.spans.start("task", task_id=task.id, input={})
        d.daemon.tasks.add(TaskRecord(task, d.daemon.clock.now(), root.id))
        if req is not None:
            d.daemon.state.requests.record(req)
        token = d.daemon.tokens.issue(task.id, "w1", scopes, identity, ttl_s=600)
        return d.daemon.tokens.validate(token), token
    return d.run(go())


def gateway_tool_call(d: RunningDaemon, grant, name: str, **args):
    return d.run(d.daemon.gateway.call(grant, name, args), timeout=20)


async def run_direct(d: RunningDaemon, mode: str, prompt: str, tool_names: list[str], schema: dict,
                     token: str = "", task: Task | None = None) -> dict:
    rt = d.daemon._make_runtime("worker" if mode == "worker" else "orchestrator", mode=mode,
                                tool_names=tool_names, schema=schema)
    task = task or Task("t-sup", None, "chat", "dev", Identity.OWNER, set(), "mixed", "r-sup", [], None)
    run = await rt.start(task, ModelRequest("", (prompt,), 0), token)
    out: dict = {"events": []}
    try:
        async for ev in rt.events(run):
            out["events"].append(ev)
            if ev.kind == "final":
                out["final"], out["seen"] = ev.data["text"], set(ev.data.get("tools_seen", []))
            if ev.kind == "violation":
                out["violation"] = ev.data
    finally:
        await rt.cancel(run)
    return out
