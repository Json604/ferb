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
    assert '(deny network-outbound (remote ip "localhost:8765"))' in sb.profile()
    assert '(allow network-outbound (remote tcp "localhost:4321"))' in sb.profile()


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


# ---------- direct connections to pinned local ports (L24, owner Q1: "connections recorded") ----------

PIN = "/usr/bin/python3"
PROCESS = "p-1234abcd"
DIRECT = f'(allow network-outbound (with report) (with message "ferb {PROCESS}") (remote tcp "localhost:8765"))'
DENIED = '(deny network-outbound (remote ip "localhost:8765"))'


def listening(monkeypatch: pytest.MonkeyPatch, executables: dict[int | None, str], port: int = 8765) -> None:
    """netstat's listeners on the port, each process id with the executable proc_pidpath reports for it."""
    rows = "".join(row(f"127.0.0.1.{port}", "*.*", "LISTEN", pid) for pid in executables)
    monkeypatch.setattr(local, "tcp_snapshot", lambda: local.parse_tcp(rows))
    monkeypatch.setattr(local, "executable_of", lambda pid: Path(executables[pid]) if pid in executables else None)


def contained(sb: Sandbox, process: str = PROCESS) -> tuple[list[str], tuple[object, ...]]:
    """The profile lines a shell command FERB tracks as `process` runs under, and its direct local ports."""
    argv, direct = sb.contain("true", process)
    try:
        return Path(argv[2]).read_text().splitlines(), direct
    finally:
        Sandbox.cleanup(Sandbox.profile_path(argv))


def test_a_pinned_port_is_direct_when_its_listener_runs_the_pinned_executable(tmp_path: Path,
                                                                               monkeypatch: pytest.MonkeyPatch) -> None:
    sb = Sandbox(protection(tmp_path, [{"port": 8765, "executable": PIN}]), tmp_path / "profiles", lambda: 4321,
                 listeners=lambda: {8765})
    listening(monkeypatch, {123: PIN})
    lines, direct = contained(sb)
    assert DIRECT in lines
    # The port stays denied for UDP and as a whole first; only TCP to it is allowed back, after the deny (last wins)
    assert DENIED in lines and lines.index(DENIED) < lines.index(DIRECT)
    assert [(pin.port, str(pin.executable)) for pin in direct] == [(8765, PIN)]   # type: ignore[attr-defined]
    assert not any("with report" in line for line in sb.profile(apps=True).splitlines())   # mac.applescript: never


@pytest.mark.parametrize("executables", [{123: "/bin/sh"},                      # another program holds the port
                                         {123: PIN, 456: "/bin/sh"},           # one of two listeners isn't the pin
                                         {None: PIN},                          # netstat can't name the process
                                         {}],                                  # nothing listens yet
                         ids=["other-executable", "one-of-two", "unknown-pid", "no-listener"])
def test_a_pinned_port_whose_listener_is_not_the_pinned_executable_is_excluded(
        tmp_path: Path, monkeypatch: pytest.MonkeyPatch, executables: dict[int | None, str]) -> None:
    sb = Sandbox(protection(tmp_path, [{"port": 8765, "executable": PIN}]), tmp_path / "profiles", lambda: 4321,
                 listeners=lambda: {8765})
    listening(monkeypatch, executables)
    lines, direct = contained(sb)
    assert DENIED in lines and not any("with report" in line or '"localhost:8765"))' in line and "allow" in line
                                       for line in lines)
    assert direct == ()


def test_no_direct_port_when_the_listener_list_cant_be_read(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    sb = Sandbox(protection(tmp_path, [{"port": 8765, "executable": PIN}]), tmp_path / "profiles", lambda: 4321,
                 listeners=lambda: None)
    listening(monkeypatch, {123: PIN})
    lines, direct = contained(sb)
    assert '(deny network-outbound (remote ip "localhost:*"))' in lines
    assert DIRECT not in lines and direct == ()


def kernel_line(message: str, process_id: int = 0) -> str:
    """One `log stream --style ndjson` event as macOS prints it (fields FERB reads, measured 2026-10-14)."""
    import json
    return json.dumps({"processID": process_id, "processImagePath": "/kernel", "eventMessage": message,
                       "senderImagePath": "/System/Library/Extensions/Sandbox.kext/Contents/MacOS/Sandbox"})


def fake_stream(*lines: str) -> list[str]:
    """A stand-in for `log stream`: its filter banner, the given events, then it keeps listening."""
    script = ("import sys, time\nprint('Filtering the log data using \"...\"', flush=True)\n"
              "for line in sys.argv[1:]:\n    print(line, flush=True)\ntime.sleep(60)\n")
    return [sys.executable, "-c", script, *lines]


def wait_for(check: object, what: str) -> object:
    import time
    deadline = time.monotonic() + 10
    while time.monotonic() < deadline:
        if found := check():                                  # type: ignore[operator]
            return found
        time.sleep(0.05)
    raise AssertionError(what)


def test_kernel_reports_count_each_programs_direct_connections_for_its_command_only() -> None:
    import time
    started = time.time() + 1                                 # a command that started after the stream listened
    reports = local.DirectReports(fake_stream(
        kernel_line("Sandbox: psql(4242) allow network-outbound remote:*:8765\nferb p-aaaa1111"),
        kernel_line("3 duplicate reports for Sandbox: psql(4242) allow network-outbound remote:*:8765\nferb p-aaaa1111"),
        kernel_line("Sandbox: redis-cli(4243) allow network-outbound remote:*:8765\nferb p-aaaa1111"),
        kernel_line("Sandbox: psql(4244) allow network-outbound remote:*:8765\nferb p-bbbb2222"),        # another command
        kernel_line("Sandbox: curl(4245) deny(1) network-outbound remote:*:8765\nferb p-aaaa1111"),     # not a connection
        kernel_line("Sandbox: psql(4246) allow network-outbound remote:*:8765\nferb p-aaaa1111", 4246),  # not the kernel
        "not json"))
    assert reports.start()
    try:
        seen = wait_for(lambda: (reports.connections("p-aaaa1111", started) or {}).get(8765, {})
                        .get("reported_connections") == 5 and reports.connections("p-aaaa1111", started),
                        "the kernel's reports were never counted")
        assert seen == {8765: {"reported_connections": 5, "clients": ["psql", "redis-cli"]}}
        assert reports.connections("p-bbbb2222", started) == {8765: {"reported_connections": 1, "clients": ["psql"]}}
        assert reports.connections("p-aaaa1111", 0.0) is None              # it started before the stream listened
        reports.forget("p-aaaa1111")
        assert reports.connections("p-aaaa1111", started) == {}
    finally:
        reports.close()
    wait_for(lambda: reports.connections("p-bbbb2222", started) is None, "a stopped stream still counts as complete")


def test_kernel_reports_unknown_when_the_system_log_cant_be_read() -> None:
    reports = local.DirectReports(["/nonexistent/log", "stream"])
    assert not reports.start()
    assert reports.connections("p-aaaa1111", 0.0) is None


class FakeReports:
    def __init__(self, seen: dict[int, dict[str, object]] | None) -> None:
        self.seen, self.forgotten = seen, []

    def connections(self, process: str, started: float) -> dict[int, dict[str, object]] | None:
        return self.seen

    def forget(self, process: str) -> None:
        self.forgotten.append(process)


def shell_with_direct_port(tmp_path: Path, monkeypatch: pytest.MonkeyPatch, reports: object,
                           refused: bool = False) -> tuple[object, list[tuple[object, dict[str, object]]]]:
    from ferb.act.shell.manager import ShellManager
    from ferb.act.shell.proxy import EgressProxy as Proxy
    ferb = tmp_path / "h" / ".ferb"
    (ferb / "workspace").mkdir(parents=True)
    p = Protection(tmp_path / "h", ferb, [], [], [], local_ports=[{"port": 8765, "executable": PIN}])
    proxy = Proxy(lambda *args: (True, ""))
    sb = Sandbox(p, tmp_path / "profiles", lambda: proxy.port, listeners=lambda: {8765})
    listening(monkeypatch, {123: PIN})
    records: list[tuple[object, dict[str, object]]] = []
    shells = ShellManager(tmp_path / "logs", lambda text: text, sandbox=sb, proxy=proxy,
                          on_network=lambda info, facts: records.append((info, facts)), reports=reports,  # type: ignore[arg-type]
                          local_refused=lambda task: refused)
    return shells, records


def test_a_direct_port_is_in_the_commands_network_record_with_local_true(tmp_path: Path,
                                                                         monkeypatch: pytest.MonkeyPatch) -> None:
    import asyncio
    reports = FakeReports({8765: {"reported_connections": 2, "clients": ["psql"]}})
    shells, records = shell_with_direct_port(tmp_path, monkeypatch, reports)
    expected = [{"host": "localhost", "port": 8765, "executable": PIN, "local": True, "direct": True,
                 "reported_connections": 2, "clients": ["psql"]}]

    async def scenario() -> None:
        h = await shells.start("true", tmp_path, "task-1")                    # type: ignore[attr-defined]
        await shells.wait(h, 20)                                             # type: ignore[attr-defined]
        assert shells.network(h)["local_direct"] == expected                 # type: ignore[attr-defined]
        assert shells.info(h).direct == ({"port": 8765, "executable": PIN},)  # type: ignore[attr-defined]
        await shells.end_task("task-1")                                      # type: ignore[attr-defined]
    try:
        asyncio.run(scenario())
    finally:
        shells.proxy.close()                                                 # type: ignore[attr-defined]
    [(info, facts)] = records
    assert facts["local_direct"] == expected and facts["destinations"] == [] and facts["refused"] == []
    assert reports.forgotten == [info.id]                                   # type: ignore[attr-defined]


@pytest.mark.parametrize("refused, reports, shown", [
    (False, None, {"reported_connections": None, "clients": None}),           # the kernel's reports aren't readable
    (True, FakeReports({}), None)])                                           # the tree said No to this Mac
def test_a_direct_port_record_says_unknown_without_reports_and_a_local_no_keeps_it_proxied(
        tmp_path: Path, monkeypatch: pytest.MonkeyPatch, refused: bool, reports: object, shown: object) -> None:
    import asyncio
    shells, records = shell_with_direct_port(tmp_path, monkeypatch, reports, refused)

    async def scenario() -> str:
        h = await shells.start("true", tmp_path, "task-1")                    # type: ignore[attr-defined]
        await shells.wait(h, 20)                                             # type: ignore[attr-defined]
        text = shells._procs[h.id][1].direct                                 # type: ignore[attr-defined]
        await shells.end_task("task-1")                                      # type: ignore[attr-defined]
        return text
    try:
        direct = asyncio.run(scenario())
    finally:
        shells.proxy.close()                                                 # type: ignore[attr-defined]
    if shown is None:
        assert direct == () and records == []                                # nothing direct, nothing reached
    else:
        [(_, facts)] = records
        assert facts["local_direct"] == [{"host": "localhost", "port": 8765, "executable": PIN, "local": True,
                                          "direct": True, **shown}]          # type: ignore[dict-item]


def test_a_tree_no_to_this_macs_address_keeps_its_local_ports_proxied(daemon: object) -> None:
    """The owner's No to one of this Mac's addresses in a task tree stays final (the proxy checks it per connection),
    so that tree's commands get no direct local port; only the latest answer counts."""
    import json
    from tests.conftest import gateway_request, gateway_setup
    d = daemon.daemon                                                        # type: ignore[attr-defined]
    grant, _ = gateway_setup(daemon, set(), gateway_request("check the dev server"), [])  # type: ignore[arg-type]

    async def after(host: str | None = None, granted: bool = False) -> bool:
        if host is not None:
            d.gateway.confirmations.record("task-1", "sink:destination", json.dumps(["sink:destination", host]),
                                           granted, None)
        return bool(d._local_refused(grant.task_id))
    run = daemon.run                                                         # type: ignore[attr-defined]
    assert run(after()) is False
    assert run(after("example.com")) is False                                # a No to another site
    assert run(after("127.0.0.1")) is True
    assert run(after("127.0.0.1", granted=True)) is False                    # a later Yes is the latest answer
    assert run(after("localhost")) is True

    async def unknown() -> bool:
        return bool(d._local_refused("unknown-task"))
    assert run(unknown()) is True                                            # a task FERB can't place: no direct port


def test_a_contained_client_reaches_a_matching_pinned_listener_directly_and_the_kernel_reports_it(
        tmp_path: Path) -> None:
    """Real sandbox-exec and the real system log: TCP to the pinned port works only while the listener runs the
    pinned executable, and the kernel reports the connection with the command's FERB process id."""
    import secrets
    import subprocess
    import time
    listener = socket.socket()
    listener.bind(("127.0.0.1", 0))
    listener.listen()
    port = listener.getsockname()[1]
    me = local.executable_of(os.getpid())
    assert me is not None
    ferb = tmp_path / "h" / ".ferb"
    (ferb / "workspace").mkdir(parents=True)
    reports = local.DirectReports()
    assert reports.start()
    connect = f"/usr/bin/python3 -c \"import socket; socket.create_connection(('127.0.0.1', {port}), 5)\""
    try:
        wait_for(lambda: reports.connections("p-00000000", time.time()) is not None, "the system log stream")
        for pin, reached in ((str(me), True), ("/bin/sh", False)):
            sb = Sandbox(Protection(tmp_path / "h", ferb, [], [], [], local_ports=[{"port": port, "executable": pin}]),
                         tmp_path / "profiles", listeners=lambda: {port})
            process, started = "p-" + secrets.token_hex(4), time.time()
            argv, direct = sb.contain(connect, process)
            try:
                r = subprocess.run(argv, capture_output=True, text=True, timeout=30)
            finally:
                Sandbox.cleanup(Sandbox.profile_path(argv))
            assert (r.returncode == 0) is reached, r.stderr
            assert bool(direct) is reached
            if reached:
                seen = wait_for(lambda: reports.connections(process, started), "the kernel's report")
                assert seen[port]["reported_connections"] >= 1                 # type: ignore[index]
            else:
                assert "Operation not permitted" in r.stderr
    finally:
        reports.close()
        listener.close()


def test_the_daemon_reads_the_kernels_reports_only_when_local_ports_are_pinned(home: Path) -> None:
    from ferb.clock.clock import FakeClock
    from ferb.core.ids import SequentialIds
    from tests.conftest import RunningDaemon
    cfg = Config(home)
    cfg.ensure_privacy_defaults()
    cfg.set("protection", "local_ports", [{"port": 8765, "executable": PIN}])
    d = RunningDaemon(home, clock=FakeClock(), ids=SequentialIds("id"), watch_owner_input=False)
    try:
        reports = d.daemon.direct_reports
        assert reports is not None and d.daemon.shells is not None and d.daemon.shells.reports is reports
        stream = reports._proc
        assert stream is not None and stream.poll() is None
    finally:
        d.stop()
    assert stream.poll() is not None                          # closed with the daemon


def test_no_kernel_report_stream_without_pinned_local_ports(daemon: object) -> None:
    assert daemon.daemon.direct_reports is None                # type: ignore[attr-defined]
