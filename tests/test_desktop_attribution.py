"""FERB tells desktop changes it may have caused from ones it didn't (owner, 2026-10-09, task fbe968b0: Codex Astra,
which FERB started, opened visible Chrome with its own screen tool). Tools declare whether they can change the screen;
the gateway records the flag beside each call; a change with no such FERB call in flight says so, with when the last
one ended and the processes FERB started that were running (P26, rule 6: facts only; who did it is the reader's
conclusion)."""
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
    h = daemon.run(shells.start(["/bin/sh", "-c", f"sleep 30 # {secret}"], tmp_path, "tree1.worker", long_lived=True))
    done = daemon.run(shells.start(["/bin/sh", "-c", "true"], tmp_path, "tree1.worker"))
    try:
        daemon.run(shells.wait(done, 10))
        procs = daemon.daemon._running_processes()
    finally:
        daemon.run(shells.kill(h))
    assert [(p["id"], p["task"], p["background"]) for p in procs] == [(h.id, "tree1.worker", True)]
    assert secret not in str(procs) and procs[0]["command"].startswith("sleep 30 #")
