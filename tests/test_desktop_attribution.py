"""FERB's records separate desktop changes that happened while one of its on-screen tool calls was in flight or had
just ended from changes with none in progress (owner, 2026-10-09, task fbe968b0: Chrome came forward while FERB only
waited on Codex Astra, a process it had started). Tools declare whether they can change the screen; the gateway records
the flag beside each call; a change with none in progress is stated as that, with when the last one ended and the
processes FERB started that were running (P26, rule 6: facts only; who did it is the reader's conclusion)."""
from pathlib import Path
from typing import Any

from ferb.act.browser.tools import (BrowserAct, BrowserClose, BrowserDialog, BrowserFillOtp, BrowserFillSecret,
                                    BrowserKeepOpen, BrowserLoginCode, BrowserNavigate, BrowserOpen,
                                    BrowserOtpCandidates, BrowserScroll, BrowserSnapshot, BrowserUseTab, BrowserWait)
from ferb.act.fs.files import FsTrash, FsWrite
from ferb.act.fs.recovery import RecoverySnapshot
from ferb.act.mac.tools import (MacAct, MacAppleScript, MacClick, MacMenu, MacOpen, MacPointer, MacScreenshot,
                                MacSnapshot)
from ferb.act.shell.tools import ShellKill, ShellList, ShellRead, ShellRun, ShellSend, ShellStart, ShellWait
from ferb.daemon.gateway import on_screen
from ferb.orchestrator.evidence import TaskEvidence
from tests.conftest import GatewayCap, RunningDaemon, gateway_request, gateway_setup, gateway_tool_call


def test_tools_that_can_change_the_screen_declare_it() -> None:
    for cap in (MacOpen, MacMenu, MacAct, MacClick, MacPointer, MacAppleScript, BrowserOpen, BrowserNavigate,
                BrowserAct, BrowserScroll, BrowserDialog, BrowserUseTab, BrowserLoginCode, BrowserClose,
                BrowserKeepOpen, BrowserFillSecret, BrowserFillOtp, ShellStart, ShellRun, ShellSend, ShellKill):
        assert on_screen(cap), cap.__name__
    for cap in (MacSnapshot, MacScreenshot, BrowserSnapshot, BrowserWait, BrowserOtpCandidates, ShellWait, ShellRead,
                ShellList, FsWrite, FsTrash, TaskEvidence, RecoverySnapshot):
        assert not on_screen(cap), cap.__name__


class _Seen:
    def __init__(self) -> None:
        self.calls: list[tuple[str, bool]] = []

    async def before_action(self, task_id: str, span_id: str, tool: str, args: dict[str, Any],
                            on_screen: bool = False) -> None:
        self.calls.append((tool, on_screen))

    async def after_action(self, span_id: str) -> None:
        return None


def test_the_gateway_records_whether_a_call_can_change_the_screen(daemon: RunningDaemon) -> None:
    screen = GatewayCap("fake.click", "click", {"browser:act"})
    screen.on_screen = True                                       # type: ignore[attr-defined]
    quiet = GatewayCap("fake.read", "read", {"browser:act"})
    grant, _ = gateway_setup(daemon, {"browser:act"}, gateway_request("hi alice@x.com", recipients=("alice@x.com",)),
                             [screen, quiet])
    seen = _Seen()
    daemon.daemon.gateway.disturbance = seen
    gateway_tool_call(daemon, grant, "fake.click")
    gateway_tool_call(daemon, grant, "fake.read")
    assert seen.calls == [("fake.click", True), ("fake.read", False)]


def test_agents_are_told_what_a_change_with_no_ferb_on_screen_action_means() -> None:
    path = Path(__file__).resolve().parents[1] / "ferb" / "orchestrator" / "prompts" / "interaction.md"
    text = " ".join(path.read_text().split())
    assert ("A change with no FERB on-screen action in progress says when the last one ended, or that this task made "
            "none: it may come from the owner, another app, a process FERB started (such as a coding agent, which acts "
            "with its own permissions), or a FERB action whose effect landed late (an app finishing its launch); "
            "weigh the elapsed time and the running processes.") in text
    assert "nearness to a FERB action does not prove FERB caused a change." in text


def test_the_running_processes_a_change_records_are_ferbs_with_commands_redacted(daemon: RunningDaemon,
                                                                                tmp_path: Path) -> None:
    """Rule 14: a command reaches the span, the bus event and the agents' context only redacted."""
    shells = daemon.daemon.shells
    assert shells is not None
    secret = "sk-ant-abcdefghijklmnopqrstuvwx"
    h = daemon.run(shells.start(f"sleep 30 # {secret}", tmp_path, "tree1.worker", long_lived=True))
    done = daemon.run(shells.start("true", tmp_path, "tree1.worker"))
    try:
        daemon.run(shells.wait(done, 10))
        procs = daemon.daemon._running_processes()
    finally:
        daemon.run(shells.kill(h))
    assert [(p["id"], p["task"], p["background"]) for p in procs] == [(h.id, "tree1.worker", True)]
    assert secret not in str(procs) and procs[0]["command"].startswith("sleep 30 #")


def test_ferb_pause_stops_desktop_sensing_for_a_task(daemon: RunningDaemon) -> None:
    """`ferb pause` promises nothing is recorded until `ferb resume`: the daemon's sentinel reads nothing and records
    only that the desktop went unobserved."""
    d = daemon.daemon
    assert d.sense is not None
    reads: list[int] = []

    def read() -> dict[str, Any]:
        reads.append(1)
        return {"frontmost_app": {"pid": 1, "bundle_id": "a.app", "name": "A"}}

    d.disturbance.read = read
    daemon.run(d.sense.pause())
    try:
        daemon.run(d.disturbance.start("paused-task"))
        assert d.disturbance.context("paused-task") == ""
        daemon.run(d.disturbance.finish("paused-task"))
    finally:
        daemon.run(d.sense.resume())
    assert reads == [] and _kinds(daemon, "paused-task") == ["disturbance.paused", "disturbance.summary"]
    daemon.run(d.disturbance.start("watched-task"))
    daemon.run(d.disturbance.finish("watched-task"))
    assert reads and _kinds(daemon, "watched-task") == ["disturbance.baseline", "disturbance.summary"]


def _kinds(daemon: RunningDaemon, task_id: str) -> list[str]:
    async def query() -> list[str]:                  # the database belongs to the daemon's thread
        assert daemon.daemon.db is not None
        return [r[0] for r in daemon.daemon.db.conn.execute(
            "SELECT kind FROM spans WHERE task_id = ? ORDER BY rowid", (task_id,))]
    return daemon.run(query())


def test_ferbs_own_cleanup_is_an_on_screen_action_and_observation_never_blocks_it(daemon: RunningDaemon) -> None:
    sup = daemon.daemon.supervisor
    assert sup is not None
    ended: list[str] = []

    async def end(task_id: str) -> list[dict[str, Any]]:
        ended.append(task_id)
        return [{"resource": "tab", "released": True}]

    class Broken(_Seen):
        async def before_action(self, *args: Any, **kwargs: Any) -> None:
            raise RuntimeError("sentinel store gone")

    sup.on_task_end = end
    seen = _Seen()
    sup.disturbance = seen
    assert daemon.run(sup._end_task("t1.worker", "cleanup-1")) == [{"resource": "tab", "released": True}]
    assert seen.calls == [("task.cleanup", True)]
    sup.disturbance = Broken()
    daemon.run(sup._end_task("t2.worker", "cleanup-2"))
    assert ended == ["t1.worker", "t2.worker"]
