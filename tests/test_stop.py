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
from ferb.policy.rules import ActionRequest, Kind, Target
from ferb.policy.targets import Request
from tests.conftest import RunningDaemon



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
    assert daemon.daemon.state.stopped
    assert cli.call("ping")["stopped"] is True                         # the owner's CLI still works
    cli.call("stop.lift")
    assert not daemon.daemon.state.stopped




def test_stop_survives_a_daemon_restart_until_lifted(home: Path) -> None:
    d = RunningDaemon(home)
    Client(d.sock).call("stop.engage")
    d.stop()
    d2 = RunningDaemon(home)                                           # e.g. crash + launchd relaunch
    try:
        assert d2.daemon.state.stopped
        assert Client(d2.sock).call("ping")["stopped"] is True
        Client(d2.sock).call("stop.lift")
        assert not (home / "STOPPED").exists()
    finally:
        d2.stop()










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
