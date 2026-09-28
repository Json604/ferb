"""Socket server (section 2), talking to a real daemon over its Unix socket.
Ported from spike 8: socket/folder permissions, stale socket replaced, second daemon refused, EOF handling."""

import json
import socket
import stat
from pathlib import Path

import pytest

from ferb.cli.client import Client, RpcError
from ferb.clock.clock import FakeClock
from ferb.core.errors import AlreadyRunning, NotRunning
from ferb.core.ids import SequentialIds
from ferb.core.types import Identity
from ferb.daemon.server import MAX_MESSAGE, TOO_LARGE, UNAUTHORIZED, UNSUPPORTED
from tests.conftest import RunningDaemon


def raw(sock_path: Path) -> tuple[socket.socket, "socket.SocketIO"]:
    s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    s.settimeout(5)
    s.connect(str(sock_path))
    return s, s.makefile("r")


def send(s: socket.socket, obj: object) -> None:
    s.sendall(json.dumps(obj).encode() + b"\n")


def test_socket_and_folder_are_owner_only(daemon: RunningDaemon) -> None:        # spike 8
    assert stat.S_IMODE(daemon.sock.parent.stat().st_mode) == 0o700
    assert stat.S_IMODE(daemon.sock.stat().st_mode) == 0o600
    assert stat.S_IMODE(daemon.daemon.home.stat().st_mode) == 0o700


def test_cli_ping(daemon: RunningDaemon) -> None:
    c = Client(daemon.sock)
    assert c.call("ping")["role"] == "cli"
    c.close()


def test_method_before_hello_is_refused(daemon: RunningDaemon) -> None:
    s, f = raw(daemon.sock)
    send(s, {"jsonrpc": "2.0", "id": 1, "method": "ping", "params": {}})
    assert json.loads(f.readline())["error"]["code"] == UNAUTHORIZED
    assert f.readline() == ""        # connection closed


def test_malformed_json_and_unknown_method_get_errors_not_a_crash(daemon: RunningDaemon) -> None:
    s, f = raw(daemon.sock)
    send(s, {"jsonrpc": "2.0", "id": 1, "method": "auth.hello", "params": {"role": "cli"}})
    f.readline()
    s.sendall(b"{not json\n")
    assert json.loads(f.readline())["error"]["code"] == -32700
    send(s, {"jsonrpc": "1.0", "id": 2, "method": "ping"})
    assert json.loads(f.readline())["error"]["code"] == -32600
    send(s, {"jsonrpc": "2.0", "id": 3, "method": "trace.get", "params": {"task_id": "x"}})
    assert json.loads(f.readline())["result"]["spans"] == []      # still serving


def test_oversized_message_is_rejected_and_connection_closed(daemon: RunningDaemon) -> None:
    s, f = raw(daemon.sock)
    s.sendall(b"x" * (MAX_MESSAGE + 10) + b"\n")
    assert json.loads(f.readline())["error"]["code"] == TOO_LARGE
    assert f.readline() == ""


def test_cli_role_from_another_uid_is_refused(home: Path) -> None:
    d = RunningDaemon(home, owner_uid=12345)          # pretend the owner is someone else
    try:
        with pytest.raises(RpcError) as e:
            Client(d.sock)
        assert e.value.code == UNAUTHORIZED
    finally:
        d.stop()


def test_mcp_shim_needs_a_valid_token_and_loses_access_when_revoked(home: Path) -> None:
    clock = FakeClock()
    d = RunningDaemon(home, clock=clock, ids=SequentialIds("id"))
    try:
        with pytest.raises(RpcError):
            Client(d.sock, role="mcp_shim", token="made-up")
        token = d.daemon.tokens.issue("task", "w1", set(), Identity.OWNER, ttl_s=60)
        shim = Client(d.sock, role="mcp_shim", token=token)
        assert shim.call("ping")["role"] == "mcp_shim"
        with pytest.raises(RpcError) as e:
            shim.call("trace.get", {"task_id": "t"})        # not in the shim's method list
        assert e.value.code == UNAUTHORIZED
        d.call_soon(d.daemon.tokens.revoke_all)              # e.g. `ferb stop`
        with pytest.raises(RpcError, match="revoked"):
            shim.call("ping")
    finally:
        d.stop()


def test_native_host_role_fails_explicitly(daemon: RunningDaemon) -> None:
    with pytest.raises(RpcError) as e:
        Client(daemon.sock, role="native_host")
    assert e.value.code == UNSUPPORTED


def test_stale_socket_file_is_replaced(home: Path) -> None:                     # spike 8
    (home / "run").mkdir(parents=True)
    stale = socket.socket(socket.AF_UNIX)
    stale.bind(str(home / "run" / "ferb.sock"))
    stale.close()
    d = RunningDaemon(home)
    try:
        assert Client(d.sock).call("ping")
    finally:
        d.stop()


def test_second_daemon_refuses_to_start_before_touching_the_database(daemon: RunningDaemon) -> None:  # spike 8
    from ferb.memory.journal import Journal
    before = daemon.run(_count_journal(daemon))
    with pytest.raises(AlreadyRunning):
        RunningDaemon(daemon.daemon.home)
    assert Client(daemon.sock).call("ping")                 # the first one is unaffected
    assert daemon.run(_count_journal(daemon)) == before     # the second wrote nothing


async def _count_journal(d: RunningDaemon) -> int:
    assert d.daemon.journal is not None
    return len(d.daemon.journal.entries())


def test_client_reports_daemon_not_running_and_eof(home: Path) -> None:        # spike 8
    with pytest.raises(NotRunning, match="not running"):
        Client(home / "run" / "ferb.sock")
    # a server that accepts and immediately closes (like a dying daemon): EOF -> NotRunning, not a JSON error
    (home / "run").mkdir(parents=True)
    srv = socket.socket(socket.AF_UNIX)
    srv.bind(str(home / "run" / "ferb.sock"))
    srv.listen(1)
    import threading
    threading.Thread(target=lambda: srv.accept()[0].close(), daemon=True).start()
    with pytest.raises(NotRunning):
        Client(home / "run" / "ferb.sock")
    srv.close()
