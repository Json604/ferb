"""ORCH-631..633: wait.for waits in code until one of its conditions holds, with one result per wake.

Live 2026-11-12 16:06-16:26 (task f68317e9): a worker polled shell.wait on a long `codex exec` 38 times at 30 s each,
about 40 model calls only to watch.
The waits here run on a stepped clock (TEST-508): each rest moves the clock at once; only the sandboxed script and the
end-to-end run start real commands."""
import asyncio
import json
import shutil
from dataclasses import dataclass, field
from datetime import datetime, timedelta, timezone
from pathlib import Path
from typing import Any

import pytest

from ferb.act.shell.manager import ProcHandle, ShellManager
from ferb.clock.wait import WaitDeps, WaitFor, WaitInput
from ferb.cli.client import Client
from ferb.core.errors import Refused
from ferb.core.types import Identity, ToolContext
from ferb.policy.rules import Kind
from tests.conftest import RunningDaemon, done, install, sup, task, tool, turn_events

T0 = datetime(2026, 11, 12, 16, 0, tzinfo=timezone.utc)


def hm(minutes: float) -> str:
    """The wall time `minutes` after T0 as the owner's Mac shows it."""
    return f"{(T0 + timedelta(minutes=minutes)).astimezone():%H:%M}"


ALL = frozenset({"time", "shell", "fs:read", "identity:browse", "browser:read"})


class Steps:
    """The wait's clock: wall and monotonic time move only when the loop rests, by what it asked for."""
    def __init__(self) -> None:
        self.wall, self.mono, self.rests = T0, 0.0, 0
        self.on_rest: list[Any] = []

    def now(self) -> datetime:
        return self.wall

    def monotonic(self) -> float:
        return self.mono

    async def pause(self, seconds: float, wake: asyncio.Event) -> None:
        self.rests += 1
        self.wall += timedelta(seconds=seconds)
        self.mono += seconds
        for hook in list(self.on_rest):
            hook(self)
        await asyncio.sleep(0)


@dataclass
class Info:
    command: str
    running: bool = True
    exit_code: int | None = None
    stopped: str | None = None


class FakeShells:
    def __init__(self) -> None:
        self.procs: dict[str, Info] = {}
        self.out: dict[str, str] = {}
        self.exits: dict[str, asyncio.Event] = {}

    def add(self, pid: str, command: str) -> None:
        self.procs[pid], self.out[pid], self.exits[pid] = Info(command), "", asyncio.Event()

    def finish(self, pid: str, code: int) -> None:
        self.procs[pid].running, self.procs[pid].exit_code = False, code
        self.exits[pid].set()

    def handle(self, pid: str, task: str) -> ProcHandle:
        return ProcHandle(pid, task)

    def info(self, h: ProcHandle) -> Info:
        if h.id not in self.procs or h.owner_task != "t1":
            raise Refused(f"no process {h.id} for this task")
        return self.procs[h.id]

    async def read(self, h: ProcHandle, since: int = 0, limit: int | None = None) -> tuple[str, int, int]:
        text = self.out[h.id]
        return text[since:], len(text), len(text)

    async def tail(self, h: ProcHandle, limit: int) -> tuple[str, int, int]:
        text = self.out[h.id]
        return text[-limit:], len(text), len(text)

    async def wait(self, h: ProcHandle, timeout: float) -> int | None:
        await self.exits[h.id].wait()
        return self.procs[h.id].exit_code


@dataclass
class Snap:
    url: str
    title: str
    text: str


@dataclass
class FakeBrowser:
    url: str = "https://www.amazon.in/dp/B0X"
    pages: list[str] = field(default_factory=list)
    reads: int = 0
    reloads: int = 0
    downloads: list[list[dict[str, Any]]] = field(default_factory=list)

    def owned_by(self, tab: str, owner: str) -> str:
        if tab != "t1" or owner != "t1":
            raise Refused(f"no tab {tab} open for this task; its open tabs: t1")
        return tab

    def tab_url(self, tab: str) -> str:
        return self.url

    async def snapshot(self, ref: str) -> Snap:
        self.reads += 1
        text = self.pages[min(self.reads, len(self.pages)) - 1]
        return Snap(self.url, "Product", text)

    async def navigate(self, ref: str, url: str) -> None:
        self.reloads += 1

    async def downloads_for(self, ref: str, wait: float = 0.0) -> list[dict[str, Any]]:
        return self.downloads.pop(0) if self.downloads else []


@dataclass
class Rig:
    steps: Steps
    shells: Any
    browser: FakeBrowser
    lines: list[str | None]
    steer: list[int]
    ending: list[str | None]
    ended: list[dict[str, Any]]
    limit: datetime | None
    tool: WaitFor


def rig(shells: Any = None, limit: datetime | None = T0 + timedelta(hours=4)) -> Rig:
    steps, browser, lines, steer, ending, ended = Steps(), FakeBrowser(), [], [0], [None], []
    r = Rig(steps, shells or FakeShells(), browser, lines, steer, ending, ended, limit, None)    # type: ignore[arg-type]
    r.tool = WaitFor(WaitDeps(now=steps.now, limit=lambda t: r.limit, shells=lambda: r.shells,
                              browsers=lambda identity: browser,
                              status=lambda w, t, until, line: lines.append(line),
                              steered=lambda t: steer[0], ending=lambda: ending[0],
                              ended=lambda t, data: ended.append(data),
                              monotonic=steps.monotonic, pause=steps.pause))
    return r


def ctx(tmp: Path, scopes: frozenset[str] = ALL, folders: tuple[str, ...] = ()) -> ToolContext:
    return ToolContext(task_id="t1", worker_id="w1", request_id="r1", identity=Identity.FERB, scopes=scopes,
                       device_id="d", span_id=None, workspace=str(tmp), owner_roots=(), untrusted_roots=(),
                       folders=folders)


def wait(r: Rig, tmp: Path, *conditions: dict[str, Any], c: ToolContext | None = None,
         **more: Any) -> dict[str, Any]:
    args = WaitInput.model_validate({"any_of": list(conditions), **more})
    out = asyncio.run(r.tool.run(args, c or ctx(tmp)))
    return out.content | ({"_error": True} if out.is_error else {})


# ---------- each condition fires, and times out ----------

# decision: ORCH-631, ORCH-633
def test_a_process_wakes_the_worker_once_when_it_exits_and_the_line_names_it(tmp_path: Path) -> None:
    r = rig()
    r.shells.add("p-1", "codex exec -s danger-full-access -C ~/.ferb/workspace 'fix the tests'")
    r.shells.out["p-1"] = "…\ntests: 214 passed\n"

    def exits(s: Steps) -> None:
        if s.mono >= 1184 and r.shells.procs["p-1"].running:
            r.shells.finish("p-1", 0)
    r.steps.on_rest.append(exits)
    out = wait(r, tmp_path, {"kind": "process", "id": "p-1"}, limit_minutes=40)
    assert out["woke"] == "process" and out["evidence"]["exited"] and out["evidence"]["exit_code"] == 0
    assert "214 passed" in out["evidence"]["output_tail"] and 1184 <= out["after_s"] <= 1186
    assert r.lines == [f"Waiting for `codex exec -s danger-full-access -C …` to finish (up to {hm(40)})", None]


# decision: ORCH-631
def test_a_process_wakes_on_its_output_and_times_out_with_what_it_last_saw(tmp_path: Path) -> None:
    r = rig()
    r.shells.add("p-1", "npm run dev")

    def prints(s: Steps) -> None:
        if s.mono >= 30:
            r.shells.out["p-1"] = "compiling\n  ➜  Local: http://localhost:5173/ ready\n"
    r.steps.on_rest.append(prints)
    out = wait(r, tmp_path, {"kind": "process", "id": "p-1", "match": "READY"})
    assert out["woke"] == "process" and out["evidence"]["matched"] == ["➜ Local: http://localhost:5173/ ready"]
    assert r.lines[0] == f"Waiting for `npm run dev` to print “READY” (up to {hm(30)})"
    r = rig()
    r.shells.add("p-2", "make build")
    out = wait(r, tmp_path, {"kind": "process", "id": "p-2"}, limit_minutes=5)
    assert out["woke"] == "time_limit" and out["evidence"]["limit"] == f"{hm(5)}"
    assert out["evidence"]["last_seen"] == [{"condition": 0, "kind": "process", "running": True}]
    assert "_error" in wait(rig(), tmp_path, {"kind": "process", "id": "p-9"})        # not this task's command


# decision: ORCH-631, ORCH-633
def test_a_file_wakes_when_it_appears_settles_or_goes(tmp_path: Path) -> None:
    downloads = tmp_path / "Downloads"
    downloads.mkdir()
    report = downloads / "report.pdf"
    r = rig()
    r.steps.on_rest.append(lambda s: report.write_bytes(b"%PDF" * 10) if s.mono >= 60 and not report.exists() else 0)
    out = wait(r, tmp_path, {"kind": "file", "path": str(report), "state": "exists"})
    assert out["woke"] == "file" and out["evidence"]["bytes"] == 40 and r.lines[0].startswith(
        "Waiting for report.pdf to appear")
    grows = {"n": 0}

    def write(s: Steps) -> None:
        if grows["n"] < 5:
            grows["n"] += 1
            with report.open("ab") as f:
                f.write(b"x" * 1000)
    r = rig()
    r.steps.on_rest.append(write)
    out = wait(r, tmp_path, {"kind": "file", "path": str(report), "state": "settled", "settle_s": 10})
    assert out["woke"] == "file" and out["evidence"]["bytes"] == 5040 and out["evidence"]["unchanged_s"] >= 10
    assert r.lines[0] == f"Waiting for report.pdf to finish downloading (up to {hm(30)})"
    r = rig()
    r.steps.on_rest.append(lambda s: report.unlink() if s.mono >= 20 and report.exists() else 0)
    assert wait(r, tmp_path, {"kind": "file", "path": str(report), "state": "gone"})["evidence"]["exists"] is False
    out = wait(rig(), tmp_path, {"kind": "file", "path": "export.csv", "state": "exists"}, limit_minutes=1)
    assert out["woke"] == "time_limit" and out["evidence"]["last_seen"][0]["exists"] is False


# decision: ORCH-631, ORCH-632
def test_a_page_is_read_no_more_often_than_its_floor_and_wakes_on_its_words(tmp_path: Path) -> None:
    r = rig()
    r.browser.pages = ["Currently unavailable."] * 4 + ["In stock.\nAdd to Cart"]
    out = wait(r, tmp_path, {"kind": "page", "tab": "t1", "match": "in stock", "reload": True, "every_s": 60})
    assert out["woke"] == "page" and out["evidence"]["matched"] == ["In stock."]
    assert r.browser.reads == 5 and r.browser.reloads == 5 and 240 <= out["after_s"] <= 241
    assert r.lines[0] == f"Watching www.amazon.in for “in stock” every 1 min (up to {hm(30)})"
    r = rig()
    r.browser.pages = ["Currently unavailable."]
    out = wait(r, tmp_path, {"kind": "page", "tab": "t1", "match": "In stock"}, limit_minutes=10)
    assert out["woke"] == "time_limit" and r.browser.reads <= 11                      # 60 s default: never more often
    assert "_error" in wait(rig(), tmp_path, {"kind": "page", "tab": "t1", "match": "x", "every_s": 10})
    assert "_error" in wait(rig(), tmp_path, {"kind": "page", "tab": "t2", "match": "x"})   # not this task's tab
    r = rig()
    r.browser.pages = ["Order #1: preparing", "Order #1: preparing", "Order #1: out for delivery"]
    out = wait(r, tmp_path, {"kind": "page", "tab": "t1", "every_s": 30})
    assert out["evidence"]["changed"] and out["evidence"]["new_lines"] == ["Order #1: out for delivery"]


# decision: ORCH-631
def test_an_address_a_new_message_and_a_download_each_wake_the_worker(tmp_path: Path) -> None:
    r = rig()
    r.steps.on_rest.append(lambda s: setattr(r.browser, "url", "https://www.amazon.in/thankyou") if s.mono > 8 else 0)
    out = wait(r, tmp_path, {"kind": "url", "tab": "t1", "match": "/thankyou"})
    assert out["woke"] == "url" and out["evidence"]["url"].endswith("/thankyou")
    r = rig()
    r.browser.url = "https://mail.google.com/mail/u/0/#inbox"
    r.browser.pages = ["Inbox\nAlice Rao — Lunch? — 9:02", "Inbox\nAlice Rao — Lunch? — 9:02",
                       "Inbox\nAlice Rao — Invoice 2231 — 16:01\nAlice Rao — Lunch? — 9:02"]
    out = wait(r, tmp_path, {"kind": "mail", "tab": "t1", "sender": "alice rao"})
    assert out["woke"] == "mail" and out["evidence"]["new_lines"] == ["Alice Rao — Invoice 2231 — 16:01"]
    assert r.lines[0] == f"Watching mail.google.com for a new message from “alice rao” every 1 min (up to {hm(30)})"
    r = rig()
    r.browser.downloads = [[], [], [{"path": "/x/statement.pdf", "bytes": 912}]]
    out = wait(r, tmp_path, {"kind": "download", "tab": "t1"})
    assert out["woke"] == "download" and out["evidence"]["downloads"][0]["path"] == "/x/statement.pdf"


# decision: ORCH-631, ORCH-632
def test_a_time_and_the_limits(tmp_path: Path) -> None:
    out = wait(rig(), tmp_path, {"kind": "time", "in_minutes": 12})
    assert out["woke"] == "time" and out["evidence"]["at"] == f"{hm(12)}" and 720 <= out["after_s"] <= 721
    # the worker sets the limit shorter or longer than the 30-minute default, up to the task's own limit
    r = rig()
    out = wait(r, tmp_path, {"kind": "file", "path": "never.txt", "state": "exists"}, limit_minutes=90)
    assert out["woke"] == "time_limit" and out["evidence"]["limit"] == f"{hm(90)}"
    assert wait(rig(), tmp_path, {"kind": "file", "path": "never.txt", "state": "exists"},
                limit_minutes=2)["evidence"]["limit"] == f"{hm(2)}"
    refused = wait(rig(), tmp_path, {"kind": "time", "in_minutes": 5}, limit_minutes=300)
    assert refused["_error"] and "past this task's time limit (" in refused["error"]
    assert "_error" in wait(rig(), tmp_path, {"kind": "time", "in_minutes": 45})     # after the 30-minute default
    r = rig(limit=T0 + timedelta(minutes=10))                                      # the task ends sooner: its limit
    out = wait(r, tmp_path, {"kind": "file", "path": "never.txt", "state": "exists"})
    assert out["evidence"]["limit"] == f"{hm(10)}"


# ---------- any of, the owner's correction, stop and restart ----------

# decision: ORCH-631, ORCH-633
def test_any_one_condition_wakes_and_a_correction_wakes_too(tmp_path: Path) -> None:
    r = rig()
    r.shells.add("p-1", "codex exec 'fix the tests'")
    r.steps.on_rest.append(lambda s: (tmp_path / "out.json").write_text("{}") if s.mono >= 50 else 0)
    out = wait(r, tmp_path, {"kind": "process", "id": "p-1"}, {"kind": "file", "path": "out.json", "state": "exists"})
    assert out["woke"] == "file" and out["evidence"]["condition"] == 1
    assert r.lines[0] == f"Waiting for `codex exec 'fix the tests'` to finish, or 1 other thing (up to {hm(30)})"
    r = rig()
    r.shells.add("p-1", "codex exec 'fix the tests'")
    r.steps.on_rest.append(lambda s: r.steer.__setitem__(0, 1) if s.mono >= 100 else 0)
    out = wait(r, tmp_path, {"kind": "process", "id": "p-1"})
    assert out["woke"] == "correction" and r.lines[-1] is None


# decision: ORCH-633
def test_stop_cancels_the_wait_and_a_restart_leaves_a_fact(tmp_path: Path) -> None:
    async def go(r: Rig, ending: str | None) -> None:
        r.shells.add("p-1", "codex exec 'fix the tests'")
        r.ending[0] = ending
        args = WaitInput.model_validate({"any_of": [{"kind": "process", "id": "p-1"}]})
        job = asyncio.ensure_future(r.tool.run(args, ctx(tmp_path)))
        for _ in range(20):
            await asyncio.sleep(0)
        job.cancel()
        with pytest.raises(asyncio.CancelledError):
            await job
    r = rig()
    asyncio.run(go(r, None))
    assert r.lines[-1] is None and r.ended == []
    r = rig()
    asyncio.run(go(r, "restart"))
    assert r.ended == [{"line": f"Waiting for `codex exec 'fix the tests'` to finish (up to {hm(30)})",
                        "began": T0.isoformat(), "why": "restart"}]


# ---------- scripts run in FERB's sandbox, as shell.run does ----------

# decision: ORCH-631
def test_a_wait_with_a_script_is_a_shell_command_for_the_gateway() -> None:
    args = WaitInput.model_validate({"any_of": [{"kind": "script", "command": "test -s out.json", "every_s": 5}]})
    spec = rig().tool.describe(args)
    assert spec.kind == Kind.RUN_SHELL and spec.content == "test -s out.json"
    plain = WaitInput.model_validate({"any_of": [{"kind": "time", "in_minutes": 1}]})
    assert rig().tool.describe(plain).kind == Kind.READ
    with pytest.raises(ValueError):
        WaitInput.model_validate({"any_of": [{"kind": "script", "command": "a"}, {"kind": "script", "command": "b"}]})
    assert "_error" in wait(rig(), Path("/tmp"), {"kind": "script", "command": "true", "every_s": 1})


@pytest.mark.skipif(not shutil.which("sandbox-exec"), reason="sandbox-exec is not available here")
# decision: ORCH-631, HAND-203
def test_a_script_runs_in_the_sandbox_and_cant_escape_it(tmp_path: Path) -> None:
    from ferb.act.shell.sandbox import Sandbox
    from ferb.policy.protection import Protection
    home, ferb = tmp_path / "home", tmp_path / "home" / ".ferb"
    for d in ("Documents", ".ferb/workspace"):
        (home / d).mkdir(parents=True)
    work = ferb / "workspace"
    sb = Sandbox(Protection(home, ferb, [], [], []), tmp_path / "profiles", listeners=lambda: set())
    shells = ShellManager(tmp_path / "logs", lambda s: s, sandbox=sb)
    r = rig(shells)
    (work / "out.json").write_text("{\"ok\": true}")
    out = wait(r, work, {"kind": "script", "command": "test -s out.json && cat out.json", "every_s": 5},
               c=ctx(work))
    assert out["woke"] == "script" and out["evidence"]["exit_code"] == 0 and "ok" in out["evidence"]["output_tail"]
    assert r.lines[0] == f"Checking `test -s out.json && cat out.json` every 5 s (up to {hm(30)})"
    escape = home / "Documents" / "planted.txt"
    r = rig(shells)
    out = wait(r, work, {"kind": "script", "command": f"echo x > '{escape}'", "every_s": 60}, c=ctx(work),
               limit_minutes=3)
    assert out["woke"] == "time_limit" and not escape.exists()
    assert "Operation not permitted" in out["evidence"]["last_seen"][0]["output_tail"]
    assert not shells.list()                       # each finished run's record went (a long wait leaves no pile)
    gone = wait(rig(shells), work, {"kind": "script", "command": "no-such-tool --check"}, c=ctx(work))
    assert gone["_error"] and "couldn't run" in gone["evidence"]["error"]


# ---------- end to end: one wake instead of polling ----------

# decision: ORCH-631, ORCH-633
def test_a_worker_waits_on_its_command_with_one_call_and_one_wake(daemon: RunningDaemon) -> None:
    """The f68317e9 shape through the real daemon, gateway, shell and scripted model: the worker starts a long command
    and waits once. Before: 38 shell.wait calls at 30 s for 19 minutes, each one a model call. After: shell.start,
    one wait.for, the report: three model turns whatever the command's length."""
    calls: list[str] = []

    def act(token: str) -> str:
        started_ = tool(daemon.sock, token, "shell.start", command="sleep 1; echo built  # the long build")
        calls.append("shell.start")
        woke = tool(daemon.sock, token, "wait.for", any_of=[{"kind": "process", "id": started_["content"]["id"]}],
                    why="the build")
        calls.append("wait.for")
        assert not woke["is_error"], woke
        assert woke["content"]["woke"] == "process" and "built" in woke["content"]["evidence"]["output_tail"]
        return done("Built.")
    script = install(daemon, [sup("task", "Building.", task("Build the deck"))], {"worker": act})
    c = Client(daemon.sock, timeout=60)
    try:
        raw: list[dict[str, Any]] = list(c.stream("chat.send", {"session_id": "s1", "text": "build the deck"}))
    finally:
        c.close()
    turn_events(raw)
    lines = [e["text"] for e in raw if e["type"] == "progress" and e.get("task_id")]
    assert any(line.startswith("Waiting for `sleep 1; echo built # the long build` to finish (up to ")
               for line in lines), lines
    async def read() -> list[Any]:
        return daemon.daemon.db.conn.execute(
            "SELECT json_extract(input_json, '$.tool') FROM spans WHERE kind = 'tool.call'").fetchall()
    spans = daemon.run(read())
    tools = [str(s[0]) for s in spans]
    assert tools.count("wait.for") == 1 and "shell.wait" not in tools and "shell.read" not in tools
    assert len(script.calls("worker")) == 1 and calls == ["shell.start", "wait.for"]
    model_turns = len(calls) + 1                   # each tool call is one model turn in a real runtime, then the report
    assert model_turns == 3, json.dumps(tools)


# decision: ORCH-633
def test_the_next_run_of_a_task_is_told_that_a_restart_ended_its_wait(daemon: RunningDaemon) -> None:
    line = "Waiting for `codex exec 'fix the tests'` to finish (up to 16:30)"

    async def go(tree: str) -> str | None:
        if tree == "tree1":
            daemon.daemon._wait_ended("tree1.worker", {"line": line, "began": T0.isoformat(), "why": "restart"})
        return daemon.daemon.supervisor._wait_cut(tree)        # type: ignore[no-any-return]
    told = daemon.run(go("tree1"))
    assert told is not None and told.startswith("FERB restarted at ") and line in told
    assert "That wait ended without waking" in told
    assert daemon.run(go("another-tree")) is None
