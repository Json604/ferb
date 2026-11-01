"""Socket server (section 2), talking to a real daemon over its Unix socket.
Ported from spike 8: socket/folder permissions, stale socket replaced, second daemon refused, EOF handling."""

import asyncio
import json
import os
import socket
import stat
from pathlib import Path
from typing import Any

import pytest

from ferb.cli.client import Client, RpcError
from ferb.clock.clock import FakeClock
from ferb.core.errors import AlreadyRunning
from ferb.core.ids import SequentialIds
from ferb.core.types import Identity
from ferb.daemon.auth import RunTokens
from ferb.daemon.server import INTERNAL, INTERNAL_ERROR, MAX_MESSAGE, TOO_LARGE, UNAUTHORIZED, Server
from ferb.memory.dashboard import Dashboard
from ferb.runtimes.proxy import ToolListProxy
from tests.conftest import RunningDaemon, raw, send


def test_socket_and_folder_are_owner_only(daemon: RunningDaemon) -> None:        # spike 8
    assert stat.S_IMODE(daemon.sock.parent.stat().st_mode) == 0o700
    assert stat.S_IMODE(daemon.sock.stat().st_mode) == 0o600
    assert stat.S_IMODE(daemon.daemon.home.stat().st_mode) == 0o700


def test_starting_the_socket_never_changes_the_process_umask(home: Path) -> None:
    """Past bug (first CI run, 2026-11-01): the socket was made 0600 through the process-wide umask, held across the
    server start's yield. A start stalled there left every folder the process created afterwards 0600 (no search
    bit), so the whole suite failed with EACCES. Other work on the loop and other threads keep the umask."""
    async def main() -> list[int]:
        seen: list[int] = []

        async def probe() -> None:
            for _ in range(5):
                mask = os.umask(0o022)
                os.umask(mask)
                seen.append(mask)
                await asyncio.sleep(0)

        server = Server(home / "run" / "ferb.sock", RunTokens(FakeClock()))
        await asyncio.gather(server.start(), probe())
        try:
            assert stat.S_IMODE((home / "run" / "ferb.sock").stat().st_mode) == 0o600
        finally:
            await server.close()
        return seen

    mask = os.umask(0o022)
    os.umask(mask)
    assert set(asyncio.run(main())) == {mask}


def test_the_daemon_and_its_local_servers_start_without_asking_the_resolver(home: Path,
                                                                            monkeypatch: pytest.MonkeyPatch) -> None:
    """Past bug (first CI run, 2026-11-01): http.server's bind looked 127.0.0.1 up for a host name (socket.getfqdn),
    on the daemon's loop while it built the shell's egress proxy; on a Mac with a slow resolver the start stalled for
    seconds. No FERB listener asks the resolver anything to start."""
    def lookup(*_a: Any) -> str:
        raise AssertionError("a reverse DNS lookup while starting")
    monkeypatch.setattr(socket, "getfqdn", lookup)
    RunningDaemon(home, clock=FakeClock(), ids=SequentialIds("id"), watch_owner_input=False).stop()
    ToolListProxy(upstream="http://127.0.0.1:9").close()
    Dashboard(lambda params: {}).server.server_close()       # never served: shutdown() would wait for it


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


def test_native_host_role_refuses_an_unknown_extension_origin(daemon: RunningDaemon) -> None:
    with pytest.raises(RpcError) as e:
        Client(daemon.sock, role="native_host")                  # no origin: not a FERB extension
    assert e.value.code == UNAUTHORIZED


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


def _journaled(d: RunningDaemon, kind: str) -> list[dict[str, Any]]:
    async def read() -> list[dict[str, Any]]:
        return [e.data for e in d.daemon._journal().of_kind(kind)]
    return list(d.run(read()))


def test_a_failure_while_answering_with_a_hidden_secret_journals_it_redacted(daemon: RunningDaemon) -> None:
    """Review Q-M3 (rule 14): a hidden-secret answer is redacted by its exact value only while it is held; the
    failure was recorded after it was released, so an error naming the value would be journaled verbatim."""
    from tests.test_choice_answers_not_secret import ask
    secret = ask(daemon, "Enter the secret for https://x.test (hidden; Enter cancels).", [], kind="owner_secret")

    def refuse(ap: Any) -> str:
        raise RuntimeError(f"the vault refused {ap.answer}")
    daemon.daemon._question_answered = refuse          # type: ignore[method-assign]
    c = Client(daemon.sock)
    try:
        with pytest.raises(RpcError) as e:
            c.call("chat.answer", {"approval_id": secret.id, "text": "zq7wx"})
    finally:
        c.close()
    assert e.value.code == INTERNAL
    [row] = _journaled(daemon, INTERNAL_ERROR)
    assert (row["method"], row["error"]) == ("chat.answer", "RuntimeError")
    assert "the vault refused" in row["message"] and "zq7wx" not in json.dumps(row)
