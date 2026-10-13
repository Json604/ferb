"""Verify local destinations against the owner's executable-pinned ports (S2, D28, owner Q1, L24).

No command/content inspection: the listener and connected TCP peer are kernel socket facts, and proc_pidpath
identifies their executable. Two routes reach a pinned port:
- Through FERB's proxy (HTTP clients, which use the proxy environment): verified before dial and again against the
  exact established peer before sending bytes; a different program that replaces a listener in between gets no
  payload. Each connection is recorded with its address, executable and byte counts (proxy.py).
- Directly (psql, redis-cli, anything that ignores the proxy): `direct_ports` checks, when a shell command's profile
  is built, that every listener on the port runs its pinned executable; only then does the profile allow TCP to it
  (sandbox.py). The allow rule asks the kernel to report each connection with the command's own tag (`report_tag`:
  its FERB process id and 128 random bits, written only into its profile, which the command can't read), and
  `DirectReports` counts only kernel reports in the exact allow shape that carry a tag FERB issued and still expects,
  so the command's `shell.network` record names the port, the executable and the programs that connected
  (`local_direct`, each `local: true`). The kernel's deny reports carry text a program chooses (a file name), so
  they can never parse as an allow report or name another command's tag (review 2026-10-14, measured).
Unknown identities fail closed: no direct port, and the proxy refuses.

Residuals: these are OS snapshots, not an atomic kernel executable constraint; a process can exec after inspection.
A direct allowance lasts the command's life, so a program that takes the port over later (the pinned server stopped)
is reachable by that command directly; the proxy route re-checks every connection. Likewise the tree's No to one of
this Mac's addresses is checked when the profile is built: a No given while a command already runs doesn't take its
direct ports away (the proxy route checks the No per connection). The kernel reports one
connection per program at once and batches repeats, delivering the batch's count with a later report, so
`reported_connections` is what was reported by the time of the record (at least one per program that connected),
not an exact count, and bytes aren't counted on the direct route. An executable pin trusts that program's
scripts/plugins too. An approved service can itself fetch outside FERB's proxy (owner accepted); FERB records only
its own connections to that service, never the service's later traffic.
"""
import ctypes
import ipaddress
import json
import os
import re
import secrets
import subprocess
import sys
import threading
import time
from collections.abc import Callable
from dataclasses import dataclass
from pathlib import Path
from typing import Any

from ferb.act.shell.listeners import NETSTAT, _PID, contained
from ferb.policy.protection import LocalPort, Protection

LOG = "/usr/bin/log"
# The kernel's own reports (process 0) that carry a FERB tag; a program can't log as the kernel
STREAM = [LOG, "stream", "--style", "ndjson", "--predicate", 'processID == 0 AND eventMessage CONTAINS "ferb p-"']
# An allow report and nothing else: the program's name can't hold parentheses or a line break, the address no
# parentheses, and the message is a whole tag. A deny report ("…(pid) deny(1) file-write-create <path>") can't match
_REPORT = re.compile(r"(?:(\d+) duplicate reports? for )?Sandbox: ([^()\n]+)\(\d+\) allow network-outbound "
                     r"[^\s()]*:(\d+)\n(ferb p-[0-9a-f]+ [0-9a-f]{32})")
# Ends the stream with FERB: its stdin is a pipe only the daemon holds, so a daemon that stops or crashes closes it
# and this stops `log stream` (which never reads stdin itself), then exits with the stream's own exit code
WATCH = ("import subprocess, sys, threading\n"
         "stream = subprocess.Popen(sys.argv[1:], stdin=subprocess.DEVNULL)\n"
         "threading.Thread(target=lambda: (sys.stdin.read(), stream.kill()), daemon=True).start()\n"
         "sys.exit(stream.wait())\n")


def report_tag(process: str) -> str:
    """The message a command's direct allow rule gives the kernel's reports: its process id and 128 random bits."""
    return f"ferb {process} {secrets.token_hex(16)}"


class LocalRefused(Exception):
    """A local connection cannot be tied to an approved executable or a contained process."""


@dataclass(frozen=True)
class TcpSocket:
    local: tuple[str, int]
    remote: tuple[str, int] | None
    state: str
    pid: int | None


@dataclass(frozen=True)
class LocalPeer:
    address: str
    executable: str


def _ip(address: str) -> str:
    """Normalize mapped IPv4 and the numeric part of a scoped IPv6 address for endpoint comparison."""
    parsed = ipaddress.ip_address(address.split("%", 1)[0])
    if isinstance(parsed, ipaddress.IPv6Address) and parsed.ipv4_mapped is not None:
        parsed = parsed.ipv4_mapped
    return str(parsed)


def _endpoint(text: str) -> tuple[str, int] | None:
    address, _, port = text.rpartition(".")
    if not port.isdigit():
        return None
    try:
        return ("*" if address == "*" else _ip(address)), int(port)
    except ValueError:
        return None


def parse_tcp(text: str) -> list[TcpSocket]:
    out = []
    for line in text.splitlines():
        fields = line.split()
        if len(fields) < 6 or not fields[0].startswith("tcp"):
            continue
        endpoint = _endpoint(fields[3])
        if endpoint is None:
            continue
        match = _PID.search(line)
        out.append(TcpSocket(endpoint, _endpoint(fields[4]), fields[5], int(match[1]) if match else None))
    return out


def tcp_snapshot() -> list[TcpSocket] | None:
    try:
        result = subprocess.run([NETSTAT, "-anv", "-p", "tcp"], capture_output=True, text=True, timeout=5)
    except (OSError, subprocess.SubprocessError):
        return None
    if result.returncode or "process:pid" not in result.stdout:
        return None
    return parse_tcp(result.stdout)


def executable_of(pid: int) -> Path | None:
    """The kernel-reported executable path; PROC_PIDPATHINFO_MAXSIZE is the platform's buffer size."""
    try:
        lib = ctypes.CDLL("/usr/lib/libproc.dylib")
        call = lib.proc_pidpath
        call.argtypes = [ctypes.c_int, ctypes.c_void_p, ctypes.c_uint32]
        call.restype = ctypes.c_int
        buf = ctypes.create_string_buffer(4096)
        if call(pid, buf, len(buf)) <= 0:
            return None
        return Path(os.path.realpath(os.fsdecode(buf.value)))
    except (OSError, AttributeError, ValueError):
        return None


class LocalAccess:
    def __init__(self, protection: Protection) -> None:
        self.protection = protection
        self.pins = {entry.port: entry.executable for entry in protection.local_ports}

    @staticmethod
    def _sockets() -> list[TcpSocket]:
        sockets = tcp_snapshot()
        if sockets is None:
            raise LocalRefused("FERB couldn't inspect the local TCP sockets")
        return sockets

    def _verified(self, rows: list[TcpSocket], port: int) -> Path:
        if not rows or any(row.pid is None for row in rows):
            raise LocalRefused("FERB couldn't identify the local listener or connected peer")
        expected = self.pins.get(port)
        executable = None
        for row in rows:
            assert row.pid is not None
            executable = executable_of(row.pid)
            if executable is None:
                raise LocalRefused("FERB couldn't identify the local server's executable")
            if expected is not None:
                if executable != expected:
                    raise LocalRefused("the local server's executable does not match protection.local_ports")
            elif not contained(row.pid, self.protection.ferb_home, self.protection.workspace):
                raise LocalRefused("this local port is not in protection.local_ports and its server is outside FERB")
        assert executable is not None
        return executable

    def before_connect(self, target: tuple[str, int]) -> None:
        address, port = _ip(target[0]), target[1]
        rows = [row for row in self._sockets() if row.state == "LISTEN" and row.local[1] == port
                and row.local[0] in (address, "*", "0.0.0.0", "::")]
        self._verified(rows, port)

    def connected(self, target: tuple[str, int], client: tuple[str, int]) -> LocalPeer:
        """Verify the server end of this exact connected socket, not whichever listener now holds that port."""
        server = (_ip(target[0]), target[1])
        source = (_ip(client[0]), client[1])
        rows = [row for row in self._sockets() if row.state == "ESTABLISHED"
                and row.local == server and row.remote == source]
        executable = self._verified(rows, target[1])
        return LocalPeer(server[0], str(executable))


def direct_ports(protection: Protection) -> tuple[LocalPort, ...]:
    """The owner-pinned ports a shell command starting now may reach directly: each port whose every listening TCP
    socket belongs to a process running its pinned executable at this moment. None of them when the sockets can't be
    read; a port with no listener, a listener netstat can't name, or any other executable is left out."""
    if not protection.local_ports:
        return ()
    sockets = tcp_snapshot()
    if sockets is None:
        return ()
    allowed = []
    for pin in protection.local_ports:
        pids = [row.pid for row in sockets if row.state == "LISTEN" and row.local[1] == pin.port]
        if pids and all(pid is not None and executable_of(pid) == pin.executable for pid in pids):
            allowed.append(pin)
    return tuple(allowed)


class DirectReports:
    """The kernel's reports of direct connections to pinned local ports, by FERB process id, read live from the
    system log for as long as FERB runs (started only when the owner has pinned local ports). A report is a fact the
    kernel states when the connection is made: which program connected, to which port, under which command's profile.
    Only a command FERB `expect`s, by its exact tag, is counted, until it is forgotten. When the stream isn't running,
    or wasn't yet when a command started, that command's count is unknown (None); when it stops by itself, `ended`
    says why and `on_end` is told once (`close` stops it without that)."""

    def __init__(self, argv: list[str] | None = None, on_end: Callable[[str], None] | None = None) -> None:
        self.argv, self.on_end = argv or STREAM, on_end
        self._lock = threading.Lock()
        self._tags: dict[str, str] = {}           # tag -> process, for the commands FERB expects reports of
        self._seen: dict[str, dict[int, tuple[int, set[str]]]] = {}   # process -> port -> (connections, programs)
        self._since: float | None = None          # when the stream started listening (wall clock)
        self._closing = False
        self.ended: str | None = None             # why the stream stopped by itself
        self._proc: subprocess.Popen[str] | None = None

    @property
    def running(self) -> bool:
        with self._lock:
            return self._since is not None and self.ended is None and not self._closing

    def start(self) -> bool:
        try:
            self._proc = subprocess.Popen([sys.executable, "-I", "-c", WATCH, *self.argv], stdin=subprocess.PIPE,
                                          stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True,
                                          errors="replace")
        except OSError as e:
            self._stopped(f"the system log stream couldn't start ({type(e).__name__}: {e})")
            return False
        threading.Thread(target=self._read, name="ferb-direct-reports", daemon=True).start()
        return True

    def _read(self) -> None:
        assert self._proc is not None and self._proc.stdout is not None
        try:
            for line in self._proc.stdout:
                if self._since is None:            # `log stream` names its filter once it listens
                    with self._lock:
                        self._since = time.time()
                try:
                    event = json.loads(line)
                except ValueError:
                    continue
                if not isinstance(event, dict) or event.get("processID") != 0:
                    continue
                match = _REPORT.fullmatch(str(event.get("eventMessage", "")).strip())
                if match is None:
                    continue
                repeats, program, port, tag = match.groups()
                with self._lock:
                    process = self._tags.get(tag)
                    if process is None:            # a tag FERB didn't issue, or a command already forgotten
                        continue
                    ports = self._seen.setdefault(process, {})
                    count, programs = ports.get(int(port), (0, set()))
                    ports[int(port)] = (count + int(repeats or 1), programs | {program})
        finally:                                   # from here on, what a command connected to is unknown
            code = self._proc.wait()
            self._stopped(f"the system log stream ended (exit {code})")

    def _stopped(self, why: str) -> None:
        with self._lock:
            if self._closing or self.ended is not None:
                return
            self.ended = why
        if self.on_end is not None:
            self.on_end(why)

    def expect(self, process: str, tag: str) -> None:
        """Count the reports carrying `tag` (the command's `report_tag`) for `process`."""
        with self._lock:
            self._tags[tag] = process

    def connections(self, process: str, started: float) -> dict[int, dict[str, Any]] | None:
        """{port: {"reported_connections": n, "clients": [program, ...]}} reported so far for a command started at
        `started` (wall clock); None when the stream wasn't listening for all of its life."""
        with self._lock:
            if self.ended is not None or self._closing or self._since is None or self._since > started:
                return None
            return {port: {"reported_connections": count, "clients": sorted(programs)}
                    for port, (count, programs) in self._seen.get(process, {}).items()}

    def forget(self, process: str) -> None:
        """The command's record is written: its reports are neither kept nor counted any more."""
        with self._lock:
            self._seen.pop(process, None)
            for tag in [tag for tag, p in self._tags.items() if p == process]:
                del self._tags[tag]

    def close(self) -> None:
        """Stop the stream (closing its stdin stops it, as a daemon that goes away does)."""
        with self._lock:
            self._closing = True
        if self._proc is not None:
            if self._proc.stdin is not None:
                self._proc.stdin.close()
            self._proc.wait()
