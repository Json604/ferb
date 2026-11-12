"""ORCH-628 through the real daemon, gateway, shell and scripted model: while a worker waits on (or reads) one of its
own commands, its live line names that command and how long it has run. Live 2026-11-12 16:06-16:26 (task f68317e9):
the line said "shell.wait", then "waiting for a command", for 20 minutes, and the planner, asked twice what it was
waiting for, said "The task record does not show the command itself"."""
from typing import Any

from ferb.cli.client import Client
from tests.conftest import RunningDaemon, done, install, sup, task, tool, turn_events


# decision: ORCH-628
def test_waiting_on_a_command_says_which_command(daemon: RunningDaemon) -> None:
    def act(token: str) -> str:
        started = tool(daemon.sock, token, "shell.start", command="sleep 0.3; echo built  # the deck build")
        proc = started["content"]["id"]
        tool(daemon.sock, token, "shell.wait", id=proc, timeout_s=5)
        tool(daemon.sock, token, "shell.read", id=proc)
        return done("Built.")
    install(daemon, [sup("task", "Building.", task("Build the deck"))], {"worker": act})
    c = Client(daemon.sock, timeout=30)
    try:
        raw: list[dict[str, Any]] = list(c.stream("chat.send", {"session_id": "s1", "text": "build the deck"}))
    finally:
        c.close()
    turn_events(raw)
    lines = [e["text"] for e in raw if e["type"] == "progress" and e.get("task_id")]
    assert "starting a command" in lines
    assert "waiting for `sleep 0.3; echo built # the deck build`" in lines, lines
    assert "reading the output of `sleep 0.3; echo built # the deck build`" in lines, lines
