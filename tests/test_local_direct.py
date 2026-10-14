"""Direct connections to owner-pinned local ports (L24, owner Q1: "executable-pinned port allow list, direct
connections recorded"). The profile allows a pinned port only while its listener runs the pinned executable; the
kernel reports each connection under the command's own unguessable tag, and only those reports are counted."""
import json
import os
import re
import secrets
import shlex
import socket
import subprocess
import sys
import time
from pathlib import Path
from typing import Any, Callable

import pytest

from ferb.act.shell import local
from ferb.act.shell.sandbox import Sandbox
from ferb.config import Config
from ferb.policy.protection import Protection

PIN = "/usr/bin/python3"
PROCESS = "p-1234abcd"
TAG = f"ferb {PROCESS} {'0' * 32}"
DIRECT = f'(allow network-outbound (with report) (with message "{TAG}") (remote tcp "localhost:8765"))'
DENIED = '(deny network-outbound (remote ip "localhost:8765"))'


def protection(tmp_path: Path, entries: object = ()) -> Protection:
    return Protection(tmp_path, tmp_path / ".ferb", [], [], [], local_ports=entries)


def row(ours: str, peer: str, state: str, pid: int | None = 123) -> str:
    owner = f"python:{pid}" if pid is not None else "?:?"
    return f"tcp4 0 0 {ours} {peer} {state} 0 0 {owner} 00100 00000000 0 0\n"


def listening(monkeypatch: pytest.MonkeyPatch, executables: dict[int | None, str], port: int = 8765) -> None:
    """netstat's listeners on the port, each process id with the executable proc_pidpath reports for it."""
    rows = "".join(row(f"127.0.0.1.{port}", "*.*", "LISTEN", pid) for pid in executables)
    monkeypatch.setattr(local, "tcp_snapshot", lambda: local.parse_tcp(rows))
    monkeypatch.setattr(local, "executable_of", lambda pid: Path(executables[pid]) if pid in executables else None)


def contained(sb: Sandbox, tag: str = TAG) -> tuple[list[str], tuple[Any, ...]]:
    """The profile lines a shell command FERB tracks under `tag` runs under, and its direct local ports."""
    argv, direct = sb.contain("true", tag)
    try:
        return Path(argv[2]).read_text().splitlines(), direct
    finally:
        Sandbox.cleanup(Sandbox.profile_path(argv))


def wait_for(check: Callable[[], Any], what: str, seconds: float = 10) -> Any:
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        if found := check():
            return found
        time.sleep(0.05)
    raise AssertionError(what)


# ---------- the profile ----------

def test_each_command_gets_its_own_unguessable_report_tag() -> None:
    first, second = local.report_tag(PROCESS), local.report_tag(PROCESS)
    assert re.fullmatch(rf"ferb {PROCESS} [0-9a-f]{{32}}", first) and first != second


def test_a_pinned_port_is_direct_when_its_listener_runs_the_pinned_executable(tmp_path: Path,
                                                                               monkeypatch: pytest.MonkeyPatch) -> None:
    sb = Sandbox(protection(tmp_path, [{"port": 8765, "executable": PIN}]), tmp_path / "profiles", lambda: 4321,
                 listeners=lambda: {8765})
    listening(monkeypatch, {123: PIN})
    lines, direct = contained(sb)
    assert DIRECT in lines
    # The port stays denied for UDP and as a whole first; only TCP to it is allowed back, after the deny (last wins)
    assert DENIED in lines and lines.index(DENIED) < lines.index(DIRECT)
    assert [(pin.port, str(pin.executable)) for pin in direct] == [(8765, PIN)]
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
    assert DENIED in lines and not any("with report" in line or ('"localhost:8765"))' in line and "allow" in line)
                                       for line in lines)
    assert direct == ()


def test_no_direct_port_when_the_listener_list_cant_be_read(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    sb = Sandbox(protection(tmp_path, [{"port": 8765, "executable": PIN}]), tmp_path / "profiles", lambda: 4321,
                 listeners=lambda: None)
    listening(monkeypatch, {123: PIN})
    lines, direct = contained(sb)
    assert '(deny network-outbound (remote ip "localhost:*"))' in lines
    assert DIRECT not in lines and direct == ()


# ---------- the kernel's reports ----------

def kernel_line(message: str, process_id: int = 0) -> str:
    """One `log stream --style ndjson` event as macOS prints it (fields FERB reads, measured 2026-10-14)."""
    return json.dumps({"processID": process_id, "processImagePath": "/kernel", "eventMessage": message,
                       "senderImagePath": "/System/Library/Extensions/Sandbox.kext/Contents/MacOS/Sandbox"})


def fake_stream(*lines: str, pid_file: Path | None = None, then: str = "time.sleep(60)") -> list[str]:
    """A stand-in for `log stream`: its filter banner, the given events, then it keeps listening (or `then`)."""
    script = ("import os, sys, time\n"
              "if sys.argv[1]: open(sys.argv[1], 'w').write(str(os.getpid()))\n"
              "print('Filtering the log data using \"...\"', flush=True)\n"
              "for line in sys.argv[2:]:\n    print(line, flush=True)\n" + then + "\n")
    return [sys.executable, "-c", script, str(pid_file or ""), *lines]


def allow(program: str, port: int, tag: str, repeats: int = 0) -> str:
    head = f"{repeats} duplicate reports for " if repeats else ""
    return kernel_line(f"{head}Sandbox: {program}(4242) allow network-outbound remote:*:{port}\n{tag}")


TAG_A = f"ferb p-aaaa1111 {'a' * 32}"
TAG_B = f"ferb p-bbbb2222 {'b' * 32}"


def test_kernel_reports_count_each_programs_direct_connections_for_its_command_only() -> None:
    started = time.time() + 1                                 # a command that started after the stream listened
    reports = local.DirectReports(fake_stream(
        allow("psql", 8765, TAG_A), allow("psql", 8765, TAG_A, repeats=3), allow("redis-cli", 8765, TAG_A),
        allow("psql", 8765, TAG_B),                                                     # another command
        kernel_line(f"Sandbox: curl(4245) deny(1) network-outbound remote:*:8765\n{TAG_A}"),   # not a connection
        kernel_line(f"Sandbox: psql(4246) allow network-outbound remote:*:8765\n{TAG_A}", 4246),  # not the kernel
        "not json"))
    reports.expect("p-aaaa1111", TAG_A)
    reports.expect("p-bbbb2222", TAG_B)
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
    assert reports.connections("p-bbbb2222", started) is None              # a stopped stream: unknown from now on


def test_a_contained_program_cant_forge_another_commands_reports() -> None:
    """Review fix 1: sandbox deny reports also come from the kernel and carry a path the program chose. A file named
    like an allow report, or a report with the wrong tag or an unknown command, is never counted or kept."""
    started = time.time() + 1
    old_shape = "x(1) allow network-outbound remote:*:5432\nferb p-aaaa1111"
    forged = [
        kernel_line(f"Sandbox: python3(123) deny(1) file-write-create /Users/o/.ferb/{old_shape}"),   # as measured
        kernel_line(f"Sandbox: python3(123) deny(1) file-write-create /Users/o/.ferb/x(1) allow network-outbound "
                    f"remote:*:5432\n{TAG_A}"),                                 # even with the right tag
        kernel_line(f"7 duplicate reports for Sandbox: python3(123) deny(1) file-write-create /x(1) allow "
                    f"network-outbound remote:*:5432\n{TAG_A}"),
        allow("psql", 5432, f"ferb p-aaaa1111 {'c' * 32}"),                    # the wrong tag
        allow("psql", 5432, f"ferb p-cccc3333 {'c' * 32}"),                    # a command FERB isn't expecting
        allow("psql", 5432, "ferb p-aaaa1111"),                                # no tag at all
        kernel_line(f"Sandbox: ps(ql)(4242) allow network-outbound remote:*:5432\n{TAG_A}"),
        allow("psql", 5432, TAG_A)]                                             # the one real report, last
    reports = local.DirectReports(fake_stream(*forged))
    reports.expect("p-aaaa1111", TAG_A)
    assert reports.start()
    try:
        wait_for(lambda: reports.connections("p-aaaa1111", started), "the real report was never counted")
        assert reports.connections("p-aaaa1111", started) == {5432: {"reported_connections": 1, "clients": ["psql"]}}
        assert set(reports._seen) == {"p-aaaa1111"}                            # nothing kept for unknown ids
    finally:
        reports.close()


def test_reports_after_a_command_is_forgotten_are_not_kept() -> None:
    started = time.time() + 1
    reports = local.DirectReports(fake_stream(then="time.sleep(0.5)\n"
                                              f"print({allow('psql', 5432, TAG_A)!r}, flush=True)\ntime.sleep(60)"))
    reports.expect("p-aaaa1111", TAG_A)
    reports.forget("p-aaaa1111")
    assert reports.start()
    try:
        time.sleep(1.5)
        assert reports._seen == {} and reports.connections("p-aaaa1111", started) == {}
    finally:
        reports.close()


def test_a_stream_that_ends_is_reported_and_closing_is_not() -> None:
    ended: list[str] = []
    dying = local.DirectReports(fake_stream(then="sys.exit(3)"), on_end=ended.append)
    assert dying.start()
    wait_for(lambda: ended, "the stream's end was never reported")
    assert "3" in ended[0] and dying.ended == ended[0] and not dying.running
    closing: list[str] = []
    healthy = local.DirectReports(fake_stream(), on_end=closing.append)
    assert healthy.start()
    wait_for(lambda: healthy.running, "the stream never started listening")
    healthy.close()
    time.sleep(0.3)
    assert closing == [] and healthy.ended is None


def test_kernel_reports_unknown_when_the_system_log_cant_be_read() -> None:
    ended: list[str] = []
    reports = local.DirectReports(["/nonexistent/log", "stream"], on_end=ended.append)
    reports.start()
    wait_for(lambda: ended, "a stream that can't run was never reported")
    assert reports.connections("p-aaaa1111", 0.0) is None and not reports.running


def test_the_stream_stops_when_the_daemon_goes_away(tmp_path: Path) -> None:
    """Review minor 5: the stream's stdin is a pipe only FERB holds; a daemon that crashes closes it, and the stream
    stops instead of lingering."""
    pid_file = tmp_path / "pid"
    reports = local.DirectReports(fake_stream(pid_file=pid_file))
    assert reports.start()
    pid = int(wait_for(lambda: pid_file.exists() and pid_file.read_text(), "the stand-in stream never started"))
    assert reports._proc is not None and reports._proc.stdin is not None
    reports._proc.stdin.close()                                # what the daemon's death does to the pipe

    def gone() -> bool:
        try:
            os.kill(pid, 0)
        except ProcessLookupError:
            return True
        return False
    try:
        wait_for(gone, "the stream outlived the daemon")
    finally:
        if not gone():                                         # this test's own stand-in, if the check failed
            os.kill(pid, 9)
        reports.close()


# ---------- the shell's record ----------

class FakeReports:
    def __init__(self, seen: dict[int, dict[str, object]] | None) -> None:
        self.seen, self.forgotten, self.expected = seen, [], {}

    def expect(self, process: str, tag: str) -> None:
        self.expected[process] = tag

    def connections(self, process: str, started: float) -> dict[int, dict[str, object]] | None:
        return self.seen

    def forget(self, process: str) -> None:
        self.forgotten.append(process)


def shell_with_direct_port(tmp_path: Path, monkeypatch: pytest.MonkeyPatch, reports: object,
                           refused: bool = False) -> tuple[Any, list[tuple[Any, dict[str, Any]]]]:
    from ferb.act.shell.manager import ShellManager
    from ferb.act.shell.proxy import EgressProxy
    ferb = tmp_path / "h" / ".ferb"
    (ferb / "workspace").mkdir(parents=True)
    p = Protection(tmp_path / "h", ferb, [], [], [], local_ports=[{"port": 8765, "executable": PIN}])
    proxy = EgressProxy(lambda *args: (True, ""))
    sb = Sandbox(p, tmp_path / "profiles", lambda: proxy.port, listeners=lambda: {8765})
    listening(monkeypatch, {123: PIN})
    records: list[tuple[Any, dict[str, Any]]] = []
    shells = ShellManager(tmp_path / "logs", lambda text: text, sandbox=sb, proxy=proxy,
                          on_network=lambda info, facts: records.append((info, facts)), reports=reports,  # type: ignore[arg-type]
                          local_refused=lambda task: refused)
    return shells, records


def run_true(shells: Any, tmp_path: Path, check: Callable[[Any], None] = lambda h: None) -> Any:
    import asyncio

    async def scenario() -> Any:
        h = await shells.start("true", tmp_path, "task-1")
        await shells.wait(h, 20)
        check(h)
        info = shells.info(h)
        await shells.end_task("task-1")
        return info
    try:
        return asyncio.run(scenario())
    finally:
        shells.proxy.close()


def test_a_direct_port_is_in_the_commands_network_record_with_local_true(tmp_path: Path,
                                                                         monkeypatch: pytest.MonkeyPatch) -> None:
    reports = FakeReports({8765: {"reported_connections": 2, "clients": ["psql"]}})
    shells, records = shell_with_direct_port(tmp_path, monkeypatch, reports)
    expected = [{"host": "localhost", "port": 8765, "executable": PIN, "local": True, "direct": True,
                 "reported_connections": 2, "clients": ["psql"]}]

    def live(h: Any) -> None:
        assert shells.network(h)["local_direct"] == expected
        assert shells.info(h).direct == ({"port": 8765, "executable": PIN},)
    info = run_true(shells, tmp_path, live)
    [(_, facts)] = records
    assert facts["local_direct"] == expected and facts["destinations"] == [] and facts["refused"] == []
    assert reports.forgotten == [info.id]
    # the command's own tag, the one its profile gives the kernel
    assert re.fullmatch(rf"ferb {info.id} [0-9a-f]{{32}}", reports.expected[info.id])


@pytest.mark.parametrize("seen, journaled", [
    (None, {"reported_connections": None, "clients": None}),          # the kernel's reports can't be read: unknown
    ({}, None)])                                                       # nothing connected: nothing to journal
def test_a_direct_port_is_journaled_only_when_used_or_unknown(tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
                                                              seen: object, journaled: object) -> None:
    shells, records = shell_with_direct_port(tmp_path, monkeypatch, FakeReports(seen))   # type: ignore[arg-type]
    run_true(shells, tmp_path)
    if journaled is None:
        assert records == []
    else:
        [(_, facts)] = records
        assert facts["local_direct"] == [{"host": "localhost", "port": 8765, "executable": PIN, "local": True,
                                          "direct": True, **journaled}]   # type: ignore[dict-item]


def test_a_tree_no_to_this_mac_keeps_the_command_on_the_proxy(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    reports = FakeReports({})
    shells, records = shell_with_direct_port(tmp_path, monkeypatch, reports, refused=True)
    info = run_true(shells, tmp_path)
    assert info.direct == () and records == [] and reports.expected == {}


def test_a_tree_no_to_this_macs_address_keeps_its_local_ports_proxied(daemon: Any) -> None:
    """The owner's No to one of this Mac's addresses in a task tree stays final (the proxy checks it per connection),
    so that tree's commands get no direct local port; only the latest answer counts."""
    from tests.conftest import gateway_request, gateway_setup
    d = daemon.daemon
    grant, _ = gateway_setup(daemon, set(), gateway_request("check the dev server"), [])

    async def after(host: str | None = None, granted: bool = False) -> bool:
        if host is not None:
            d.gateway.confirmations.record("task-1", "sink:destination", json.dumps(["sink:destination", host]),
                                           granted, None)
        return bool(d._local_refused(grant.task_id))
    run = daemon.run
    assert run(after()) is False
    assert run(after("example.com")) is False                                # a No to another site
    assert run(after("127.0.0.1")) is True
    assert run(after("127.0.0.1", granted=True)) is False                    # a later Yes is the latest answer
    assert run(after("localhost")) is True

    async def unknown() -> bool:
        return bool(d._local_refused("unknown-task"))
    assert run(unknown()) is True                                            # a task FERB can't place: no direct port


# ---------- for real: sandbox-exec and the system log ----------

def live_reports() -> local.DirectReports:
    reports = local.DirectReports()
    reports.start()
    wait_for(lambda: reports.running or reports.ended, "the system log stream neither started nor stopped")
    if not reports.running:
        reports.close()
        pytest.skip(f"the unified log can't be read here ({reports.ended})")
    return reports


def test_a_contained_client_reaches_a_matching_pinned_listener_directly_and_the_kernel_reports_it(
        tmp_path: Path) -> None:
    """Real sandbox-exec and the real system log: TCP to the pinned port works only while the listener runs the
    pinned executable, the kernel reports the connection under the command's own tag, and a file the command names
    like an allow report (a deny report the kernel writes too) is never counted."""
    listener = socket.socket()
    listener.bind(("127.0.0.1", 0))
    listener.listen()
    port = listener.getsockname()[1]
    me = local.executable_of(os.getpid())
    assert me is not None
    home = tmp_path / "h"
    ferb = home / ".ferb"
    (ferb / "workspace").mkdir(parents=True)
    reports = live_reports()
    try:
        for pin, reached in ((str(me), True), ("/bin/sh", False)):
            sb = Sandbox(Protection(home, ferb, [], [], [], local_ports=[{"port": port, "executable": pin}]),
                         tmp_path / "profiles", listeners=lambda: {port})
            process, started = "p-" + secrets.token_hex(4), time.time()
            victim = "p-" + secrets.token_hex(4)
            tag = local.report_tag(process)
            reports.expect(process, tag)
            reports.expect(victim, local.report_tag(victim))
            forge = str(Path(os.path.realpath(ferb)) / f"x(1) allow network-outbound remote:*:{port}\nferb {victim}")
            script = (f"import socket\ntry:\n    open({forge!r}, 'w')\nexcept OSError:\n    pass\n"
                      f"socket.create_connection(('127.0.0.1', {port}), 5)\n")
            argv, direct = sb.contain(f"/usr/bin/python3 -c {shlex.quote(script)}", tag)
            try:
                r = subprocess.run(argv, capture_output=True, text=True, timeout=30)
            finally:
                Sandbox.cleanup(Sandbox.profile_path(argv))
            assert (r.returncode == 0) is reached, r.stderr
            assert bool(direct) is reached
            if reached:
                seen = wait_for(lambda: reports.connections(process, started), "the kernel's report")
                assert seen[port]["reported_connections"] >= 1
            else:
                assert "Operation not permitted" in r.stderr
            time.sleep(1)                                        # the forged deny report has been logged by now
            assert reports.connections(victim, started) == {}
    finally:
        reports.close()
        listener.close()


# ---------- the daemon ----------

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
        assert d.daemon._local_reports_status() == {"running": reports.running, "stopped": None}
    finally:
        d.stop()
    assert stream.poll() is not None                          # closed with the daemon


def test_a_stopped_report_stream_is_journaled_and_shown_by_doctor(home: Path) -> None:
    from ferb.cli.setup import local_reports_check
    from ferb.clock.clock import FakeClock
    from ferb.core.ids import SequentialIds
    from tests.conftest import RunningDaemon
    cfg = Config(home)
    cfg.ensure_privacy_defaults()
    cfg.set("protection", "local_ports", [{"port": 8765, "executable": PIN}])
    d = RunningDaemon(home, clock=FakeClock(), ids=SequentialIds("id"), watch_owner_input=False)
    try:
        reports = d.daemon.direct_reports
        assert reports is not None and reports._proc is not None
        reports._proc.stdin.close()                            # the stream stops (as if it died)

        async def journaled() -> list[dict[str, Any]]:
            return [e.data for e in d.daemon.journal.of_kind("shell.local_reports_stopped")]
        rows = wait_for(lambda: d.run(journaled()), "the stopped stream was never journaled")
        assert rows[0]["why"] == reports.ended

        async def status() -> Any:
            return d.daemon._local_reports_status()
        shown = d.run(status())
        assert shown == {"running": False, "stopped": reports.ended}
        check = local_reports_check({"local_reports": shown})
        assert check is not None and check.status == "warn" and "unrecorded" in check.detail
    finally:
        d.stop()
    assert local_reports_check({"local_reports": None}) is None                  # no pinned ports: nothing to say
    running = local_reports_check({"local_reports": {"running": True, "stopped": None}})
    assert running is not None and running.status == "ok"


def test_no_kernel_report_stream_without_pinned_local_ports(daemon: Any) -> None:
    assert daemon.daemon.direct_reports is None


def test_a_command_that_fails_to_start_leaves_no_expected_tag(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    import asyncio
    from ferb.act.shell import pty_proc
    reports = FakeReports({})
    shells, _ = shell_with_direct_port(tmp_path, monkeypatch, reports)

    async def broken(self: Any) -> None:
        raise OSError("no pty")
    monkeypatch.setattr(pty_proc.PtyProc, "start", broken)

    async def scenario() -> None:
        with pytest.raises(OSError):
            await shells.start("true", tmp_path, "task-1")
    try:
        asyncio.run(scenario())
    finally:
        shells.proxy.close()
    assert list(reports.expected) and reports.forgotten == list(reports.expected)
