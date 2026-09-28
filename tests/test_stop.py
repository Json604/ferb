"""Daemon-level: `ferb stop` / `resume-tasks` / `undo` / `autonomy` through the real socket and CLI, the stop
surviving a restart, and redaction on `ferb logs` + the journal. Safety tests 21.2 (partial) and 21.5 (partial)."""

import io
import threading
import time
from pathlib import Path

import pytest

from ferb.cli import main as cli_main
from ferb.cli.client import Client, RpcError
from ferb.cli.logs import run as logs_run
from ferb.clock.clock import FakeClock
from ferb.core.ids import SequentialIds
from ferb.core.types import Identity
from ferb.policy.intent import RequestIntent
from ferb.policy.rules import ActionRequest, Kind, Target
from tests.conftest import RunningDaemon

SEND = ActionRequest("gmail.send", Kind.SEND, Identity.OWNER, frozenset({"browser:act"}), "t1", "w1", "r1",
                     frozenset({"browser:act"}), Target(recipients=("alice@x.com",)))


def test_stop_revokes_shim_tokens_within_a_second_and_blocks_actions(daemon: RunningDaemon) -> None:
    token = daemon.daemon.tokens.issue("t1", "w1", {"browser:act"}, Identity.OWNER, ttl_s=600)
    shim = Client(daemon.sock, role="mcp_shim", token=token)
    assert shim.call("ping")
    cli = Client(daemon.sock)
    t0 = time.monotonic()
    summary = cli.call("stop.engage")
    with pytest.raises(RpcError, match="revoked"):
        shim.call("ping")
    assert time.monotonic() - t0 < 1.0                                   # 21.2: within one second
    assert summary["tokens_revoked"] == 1
    assert daemon.daemon.engine.evaluate(SEND).step == "stop"
    assert cli.call("ping")["stopped"] is True                         # the owner's CLI still works
    cli.call("stop.lift")
    daemon.run(_record_intent(daemon))
    assert daemon.daemon.engine.evaluate(SEND).decision == "hold"


async def _record_intent(d: RunningDaemon) -> None:
    d.daemon.engine.intents.record(RequestIntent.from_plan("r1", {"email"}, recipients=frozenset({"alice@x.com"})))


def test_stop_survives_a_daemon_restart_until_lifted(home: Path) -> None:
    d = RunningDaemon(home)
    Client(d.sock).call("stop.engage")
    d.stop()
    d2 = RunningDaemon(home)                                           # e.g. crash + launchd relaunch
    try:
        assert d2.daemon.engine.stopped
        assert Client(d2.sock).call("ping")["stopped"] is True
        Client(d2.sock).call("stop.lift")
        assert not (home / "STOPPED").exists()
    finally:
        d2.stop()


def test_stop_cancels_held_sends(daemon: RunningDaemon) -> None:
    daemon.run(_record_intent(daemon))
    v = daemon.daemon.engine.evaluate(SEND)
    assert v.decision == "hold"
    assert Client(daemon.sock).call("stop.engage")["holds_undone"] == 1
    assert daemon.daemon.engine.holds.get(v.hold_id).state == "undone"


def test_ferb_undo_via_cli(daemon: RunningDaemon, monkeypatch: pytest.MonkeyPatch,
                           capsys: pytest.CaptureFixture[str]) -> None:
    monkeypatch.setenv("FERB_HOME", str(daemon.daemon.home))
    daemon.run(_record_intent(daemon))
    v = daemon.daemon.engine.evaluate(SEND)
    assert cli_main(["undo"]) == 0                                     # the only pending send
    assert f"undone: {v.hold_id}" in capsys.readouterr().out
    assert cli_main(["undo"]) == 1                                     # nothing left
    assert "nothing pending" in capsys.readouterr().out


def test_undo_window_release_is_published_on_the_bus(home: Path) -> None:
    clock = FakeClock()
    d = RunningDaemon(home, clock=clock, ids=SequentialIds("id"), undo_window_s=10)
    try:
        out = io.StringIO()
        c = Client(d.sock)
        t = threading.Thread(target=logs_run, args=(c, ["actions"], out, 2), daemon=True)
        t.start()
        time.sleep(0.3)
        d.run(_record_intent(d))
        d.call_soon(lambda: d.daemon.publish_verdict(d.daemon.engine.evaluate(SEND)))
        d.call_soon(clock.advance, 11)
        t.join(5)
        lines = out.getvalue().splitlines()
        assert "action.held" in lines[0] and '"seconds":10' in lines[0]
        assert "action.released" in lines[1]
    finally:
        d.stop()


def test_autonomy_persists_in_config(daemon: RunningDaemon, monkeypatch: pytest.MonkeyPatch,
                                     capsys: pytest.CaptureFixture[str]) -> None:
    monkeypatch.setenv("FERB_HOME", str(daemon.daemon.home))
    assert cli_main(["autonomy"]) == 0 and "autonomy: balanced" in capsys.readouterr().out
    assert cli_main(["autonomy", "careful"]) == 0
    assert 'autonomy = "careful"' in (daemon.daemon.home / "config.toml").read_text()
    assert (daemon.daemon.home / "config.toml").stat().st_mode & 0o077 == 0


def test_secrets_never_reach_logs_or_journal(daemon: RunningDaemon) -> None:
    """21.5 (partial, build step 2): through the real daemon with the real redactor."""
    secret = "sk-ant-api03-SEEDEDSECRETVALUE1234"
    out = io.StringIO()
    c = Client(daemon.sock)
    t = threading.Thread(target=logs_run, args=(c, ["tasks"], out, 1), daemon=True)
    t.start()
    time.sleep(0.3)
    daemon.call_soon(daemon.daemon.bus.publish, "task.created", {"goal": f"use key {secret}", "password": "hunter2"})
    t.join(5)
    assert secret not in out.getvalue() and "hunter2" not in out.getvalue()
    assert "use key [redacted]" in out.getvalue()

    async def write() -> None:
        daemon.daemon.journal.append("chat.message", {"text": f"my key is {secret}", "token": "abc123xyz"})
        sp = daemon.daemon.spans.start("tool.call", task_id="t1", input={"api_key": "zzz-key", "q": secret})
        daemon.daemon.spans.finish(sp, error=f"auth failed with {secret}")
    daemon.run(write())
    raw = b"".join(p.read_bytes() for p in daemon.daemon.home.glob("ferb.db*"))
    for leaked in (secret.encode(), b"abc123xyz", b"zzz-key", b"hunter2"):
        assert leaked not in raw
