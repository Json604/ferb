"""Verify a local proxy destination against the owner's executable-pinned ports (S2, D28).

No command/content inspection: the listener and connected TCP peer are kernel socket facts, and proc_pidpath
identifies their executable. Verify before dial and again against the exact established peer before sending bytes;
a different program that replaces a listener in between gets no payload. Unknown identities fail closed.

Residual: these are OS snapshots, not an atomic kernel executable constraint; a process can exec after inspection.
An executable pin trusts that program's scripts/plugins too. An approved service can itself fetch outside FERB's
proxy (owner accepted); FERB records only its own connection to that service, never the service's later traffic.
"""
import ctypes
import ipaddress
import os
import subprocess
from dataclasses import dataclass
from pathlib import Path

from ferb.act.shell.listeners import NETSTAT, _PID, contained
from ferb.policy.protection import Protection


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
