"""Owner-approved local services: executable-pinned, authenticated and recorded (S2, D28, D31).

Offline regressions for the owner's dev-server/Ollama decision; all sockets use disposable local listeners.
"""
import base64
import http.client
import os
import socket
import sys
import threading
from pathlib import Path

import pytest

from ferb.act.shell import local
from ferb.act.shell.proxy import EgressProxy
from ferb.act.shell.sandbox import Sandbox, TRUST_SERVICES
from ferb.config import Config
from ferb.policy.protection import BadProtection, Protection, TRUSTD, load


def protection(tmp_path: Path, entries: object = ()) -> Protection:
    return Protection(tmp_path, tmp_path / ".ferb", [], [], [], local_ports=entries)


@pytest.mark.parametrize("entries", ["8765", [8765], [{"port": True, "executable": "/bin/x"}],
                                    [{"port": 0, "executable": "/bin/x"}],
                                    [{"port": 65536, "executable": "/bin/x"}],
                                    [{"port": 8765, "executable": "python"}],
                                    [{"port": 8765, "executable": ""}],
                                    [{"port": 8765, "executable": "/bin/x", "anything": True}],
                                    [{"port": 8765, "executable": "/bin/x"},
                                     {"port": 8765, "executable": "/bin/y"}]])
def test_local_port_entries_fail_closed_when_ambiguous(tmp_path: Path, entries: object) -> None:
    with pytest.raises(BadProtection, match="local_ports"):
        protection(tmp_path, entries)


def test_local_ports_are_loaded_and_defaults_preserve_owner_choice(tmp_path: Path) -> None:
    cfg = Config(tmp_path)
    cfg.ensure_privacy_defaults()
    assert cfg.get("protection", "local_ports") == []
    chosen = [{"port": 8765, "executable": str(Path(sys.executable).resolve())}]
    cfg.set("protection", "local_ports", chosen)
    cfg.ensure_privacy_defaults()
    p = load(cfg.get, tmp_path, tmp_path / ".ferb")
    assert p.local_ports[0].port == 8765
    assert str(p.local_ports[0].executable) == chosen[0]["executable"]
    assert cfg.get("protection", "local_ports") == chosen


def test_configured_port_is_denied_directly_even_before_a_listener_exists(tmp_path: Path) -> None:
    p = protection(tmp_path, [{"port": 8765, "executable": "/usr/bin/python3"}])
    sb = Sandbox(p, tmp_path / "profiles", lambda: 4321, listeners=lambda: set())
    profile = sb.profile()
    assert '(deny network-outbound (remote ip "*:*"))' in profile and "localhost:8765" not in profile
    assert '(allow network-outbound (remote tcp "localhost:4321"))' in profile


def test_trustd_is_one_liftable_rule_and_leaves_every_other_deny_intact(tmp_path: Path) -> None:
    p = protection(tmp_path)
    sb = Sandbox(p, tmp_path / "profiles", listeners=lambda: set())
    assert p.liftable(TRUSTD) and p.entry(TRUSTD) is None
    assert next(r for r in p.rules() if r["id"] == TRUSTD)["liftable"]
    denied = {f'(deny mach-lookup (global-name "{name}"))' for name in TRUST_SERVICES}
    for apps in (False, True):
        base = set(sb.profile(apps=apps).splitlines())
        lifted = set(sb.profile(TRUSTD, apps=apps).splitlines())
        assert base - lifted == denied
        assert not lifted - base
        assert '(deny mach-lookup (global-name "com.apple.SecurityServer"))' in lifted


def row(ours: str, peer: str, state: str, pid: int | None = 123) -> str:
    owner = f"python:{pid}" if pid is not None else "?:?"
    return f"tcp4 0 0 {ours} {peer} {state} 0 0 {owner} 00100 00000000 0 0\n"


def test_socket_identity_uses_exact_connected_peer_not_the_new_listener(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    p = protection(tmp_path, [{"port": 8765, "executable": "/usr/bin/python3"}])
    access = local.LocalAccess(p)
    monkeypatch.setattr(local, "executable_of", lambda pid: Path("/usr/bin/python3") if pid == 123 else Path("/bin/sh"))
    monkeypatch.setattr(local, "tcp_snapshot", lambda: local.parse_tcp(
        row("127.0.0.1.8765", "*.*", "LISTEN") +
        row("127.0.0.1.8765", "127.0.0.1.54321", "ESTABLISHED", 456)))
    access.before_connect(("127.0.0.1", 8765))
    with pytest.raises(local.LocalRefused, match="executable"):
        access.connected(("127.0.0.1", 8765), ("127.0.0.1", 54321))


@pytest.mark.parametrize("rows", [None, [], local.parse_tcp(row("127.0.0.1.8765", "*.*", "LISTEN", None))])
def test_unknown_listener_identity_is_refused(tmp_path: Path, monkeypatch: pytest.MonkeyPatch, rows: object) -> None:
    access = local.LocalAccess(protection(tmp_path, [{"port": 8765, "executable": "/bin/x"}]))
    monkeypatch.setattr(local, "tcp_snapshot", lambda: rows)
    with pytest.raises(local.LocalRefused):
        access.before_connect(("127.0.0.1", 8765))


def test_unconfigured_contained_server_is_preserved(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    access = local.LocalAccess(protection(tmp_path, [{"port": 8765, "executable": "/bin/x"}]))
    monkeypatch.setattr(local, "tcp_snapshot", lambda: local.parse_tcp(
        row("127.0.0.1.9876", "*.*", "LISTEN") + row("127.0.0.1.9876", "127.0.0.1.54321", "ESTABLISHED")))
    monkeypatch.setattr(local, "contained", lambda *args: True)
    monkeypatch.setattr(local, "executable_of", lambda pid: Path("/usr/bin/python3"))
    access.before_connect(("127.0.0.1", 9876))
    evidence = access.connected(("127.0.0.1", 9876), ("127.0.0.1", 54321))
    assert evidence.executable == "/usr/bin/python3" and evidence.address == "127.0.0.1"


def test_local_standing_permission_never_bypasses_task_ownership_or_a_final_no(tmp_path: Path) -> None:
    import asyncio
    from types import SimpleNamespace
    from ferb.daemon.gateway import Gateway
    from ferb.act.shell.proxy import DEST_REFUSED_NO

    gateway = object.__new__(Gateway)
    gateway.proc_task = lambda proc: None
    gateway.tasks = SimpleNamespace(tree_of=lambda task: "tree")
    assert not asyncio.run(gateway.destination_allowed("unknown", "127.0.0.1", 8765, local_approved=True))[0]
    gateway.proc_task = lambda proc: "task"
    gateway.confirmations = SimpleNamespace(find=lambda *args: {"granted": False})
    assert asyncio.run(gateway.destination_allowed("p", "127.0.0.1", 8765, local_approved=True)) == (False, DEST_REFUSED_NO)
    gateway.confirmations = SimpleNamespace(find=lambda *args: None)
    assert asyncio.run(gateway.destination_allowed("p", "127.0.0.1", 8765, local_approved=True)) == (True, "")


def auth(proxy: EgressProxy) -> dict[str, str]:
    token = proxy.credentials("p-local")
    return {"Proxy-Authorization": "Basic " + base64.b64encode(f"p-local:{token}".encode()).decode()}


@pytest.mark.parametrize("tunnel", [False, True])
def test_local_connection_records_actual_executable_address_and_bytes(tmp_path: Path, tunnel: bool) -> None:
    import http.server
    class Handler(http.server.BaseHTTPRequestHandler):
        def do_GET(self) -> None:
            self.send_response(200)
            self.send_header("Content-Length", "2")
            self.end_headers()
            self.wfile.write(b"ok")

        def log_message(self, *args: object) -> None:
            pass

    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    path = local.executable_of(os.getpid())
    assert path is not None
    access = local.LocalAccess(protection(tmp_path, [{"port": server.server_port, "executable": str(path)}]))
    proxy = EgressProxy(lambda *args: (True, ""), local_access=access)
    try:
        assert proxy.env("p-local")["NO_PROXY"] == ""
        client = http.client.HTTPConnection("127.0.0.1", proxy.port, timeout=5)
        if tunnel:
            client.set_tunnel("127.0.0.1", server.server_port, headers=auth(proxy))
            client.request("GET", "/")
        else:
            client.request("GET", f"http://127.0.0.1:{server.server_port}/", headers=auth(proxy))
        assert client.getresponse().read() == b"ok"
        facts = proxy.facts("p-local")["destinations"][0]
        assert facts["bytes_out"] > 0 and facts["bytes_in"] > 0 and facts["connections"] == 1
        assert facts["local_connections"] == [{"address": "127.0.0.1", "executable": str(path), "connections": 1}]
        client.close()
    finally:
        proxy.close()
        server.shutdown()
        server.server_close()


def test_wrong_connected_peer_receives_no_http_payload(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    listener = socket.socket()
    listener.bind(("127.0.0.1", 0))
    listener.listen()
    port = listener.getsockname()[1]
    access = local.LocalAccess(protection(tmp_path, [{"port": port, "executable": "/bin/x"}]))
    monkeypatch.setattr(access, "before_connect", lambda address: None)
    def changed(*args: object) -> None:
        raise local.LocalRefused("the connected executable changed")
    monkeypatch.setattr(access, "connected", changed)
    proxy = EgressProxy(lambda *args: (True, ""), local_access=access)
    try:
        client = http.client.HTTPConnection("127.0.0.1", proxy.port, timeout=5)
        client.request("POST", f"http://127.0.0.1:{port}/private", body="private-value", headers=auth(proxy))
        assert client.getresponse().status == 403
        peer, _ = listener.accept()
        peer.settimeout(2)
        assert peer.recv(1) == b""
        peer.close()
        assert proxy.facts("p-local")["destinations"] == []
        assert "executable" in proxy.facts("p-local")["refused"][0]["why"]
        client.close()
    finally:
        proxy.close()
        listener.close()
