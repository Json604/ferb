"""This Mac's TCP and UDP sockets as kernel facts: endpoints, TCP state, the owning process. Must not judge what a
socket or its program is or does; local.py and listeners.py decide from these facts, and fail closed on a socket whose
process can't be named.

Two readings (regression 2026-10-31, macOS 27.2, measured):
- `inspectable()`: libproc, in FERB's own process, no subprocess. Every process lists its descriptors
  (proc_pidinfo PROC_PIDLISTFDS) and each socket's endpoints, state and accept queue (proc_pidfdinfo
  PROC_PIDFDSOCKETINFO, `struct socket_fdinfo` of the SDK's sys/proc_info.h). It sees every process of the owner's
  user (878 processes, 1.7 ms) and none of another user's or root's (EPERM: launchd's sshd and 8021 listeners). A
  connection the server hasn't accepted yet is in no descriptor table: only its listener's queue counts it.
- `complete()`: the inspectable sockets plus every other socket `nettop` lists, those with no process FERB could
  inspect marked unnamed (pid None). nettop is Apple's, entitled to the kernel's network statistics, and still
  lists every user's sockets when FERB's Python starts it (7 ms). It counts only when it lists every listener the
  inspectable reading found, so a nettop that sees less (the way netstat now does) is no reading at all.
Why not netstat: since macOS 27.2 the kernel's socket lists (net.inet.{tcp,udp}.pcblist[_n]) hold only the caller's
own sockets for FERB's ad-hoc-signed Python and for any program it starts (netstat exits 0 and prints nothing), while
the same netstat started from a terminal, or exec'd in place of that Python, lists everything. Apple's rule isn't
documented; it behaves like Local Network privacy's attribution of a tool to the third-party program that ran it."""

import ctypes
import ipaddress
import re
import socket
import subprocess
from dataclasses import dataclass, replace

NETTOP = "/usr/bin/nettop"
_PROC_ALL_PIDS, _PROC_PIDLISTFDS, _PROC_PIDFDSOCKETINFO, _PROX_FDTYPE_SOCKET = 1, 1, 3, 2
_SOCKINFO_IN, _SOCKINFO_TCP, _INI_IPV4, _INI_IPV6 = 1, 2, 0x1, 0x2
_TCP_STATES = ("CLOSED", "LISTEN", "SYN_SENT", "SYN_RECEIVED", "ESTABLISHED", "CLOSE_WAIT", "FIN_WAIT_1", "CLOSING",
               "LAST_ACK", "FIN_WAIT_2", "TIME_WAIT")                # TSI_S_*, by value
_NETTOP_STATES = {"Listen": "LISTEN", "Established": "ESTABLISHED", "SynSent": "SYN_SENT",
                  "SynReceived": "SYN_RECEIVED", "CloseWait": "CLOSE_WAIT", "FinWait1": "FIN_WAIT_1",
                  "Closing": "CLOSING", "LastAck": "LAST_ACK", "FinWait2": "FIN_WAIT_2", "TimeWait": "TIME_WAIT",
                  "Closed": "CLOSED"}


@dataclass(frozen=True)
class Socket:
    proto: str                          # "tcp" or "udp"
    local: tuple[str, int]              # "*" for any address
    remote: tuple[str, int] | None      # None: not connected
    state: str                          # the TCP state as netstat names it; "" for UDP
    pid: int | None                     # None: a process FERB can't inspect
    queued: int = 0                     # a listener's connections no process has accepted yet


# The SDK's sys/proc_info.h, field for field (natural alignment, as the header is compiled)
class _VinfoStat(ctypes.Structure):
    _fields_ = [("vst_dev", ctypes.c_uint32), ("vst_mode", ctypes.c_uint16), ("vst_nlink", ctypes.c_uint16),
                ("vst_ino", ctypes.c_uint64), ("vst_uid", ctypes.c_uint32), ("vst_gid", ctypes.c_uint32),
                ("vst_times", ctypes.c_int64 * 8), ("vst_size", ctypes.c_int64), ("vst_blocks", ctypes.c_int64),
                ("vst_blksize", ctypes.c_int32), ("vst_flags", ctypes.c_uint32), ("vst_gen", ctypes.c_uint32),
                ("vst_rdev", ctypes.c_uint32), ("vst_qspare", ctypes.c_int64 * 2)]


class _SockbufInfo(ctypes.Structure):
    _fields_ = [("sbi_cc", ctypes.c_uint32), ("sbi_hiwat", ctypes.c_uint32), ("sbi_mbcnt", ctypes.c_uint32),
                ("sbi_mbmax", ctypes.c_uint32), ("sbi_lowat", ctypes.c_uint32), ("sbi_flags", ctypes.c_short),
                ("sbi_timeo", ctypes.c_short)]


class _InV6(ctypes.Structure):
    _fields_ = [("in6_hlim", ctypes.c_uint8), ("in6_cksum", ctypes.c_int), ("in6_ifindex", ctypes.c_ushort),
                ("in6_hops", ctypes.c_short)]


class _InSockinfo(ctypes.Structure):
    _fields_ = [("insi_fport", ctypes.c_int), ("insi_lport", ctypes.c_int), ("insi_gencnt", ctypes.c_uint64),
                ("insi_flags", ctypes.c_uint32), ("insi_flow", ctypes.c_uint32), ("insi_vflag", ctypes.c_uint8),
                ("insi_ip_ttl", ctypes.c_uint8), ("rfu_1", ctypes.c_uint32),
                ("insi_faddr", ctypes.c_uint32 * 4), ("insi_laddr", ctypes.c_uint32 * 4),   # in4in6_addr | in6_addr
                ("insi_v4", ctypes.c_ubyte), ("insi_v6", _InV6)]


class _TcpSockinfo(ctypes.Structure):
    _fields_ = [("tcpsi_ini", _InSockinfo), ("tcpsi_state", ctypes.c_int), ("tcpsi_timer", ctypes.c_int * 4),
                ("tcpsi_mss", ctypes.c_int), ("tcpsi_flags", ctypes.c_uint32), ("rfu_1", ctypes.c_uint32),
                ("tcpsi_tp", ctypes.c_uint64)]


class _UnSockinfo(ctypes.Structure):                                # the union's largest member
    _fields_ = [("unsi_conn_so", ctypes.c_uint64), ("unsi_conn_pcb", ctypes.c_uint64),
                ("unsi_addr", ctypes.c_char * 255), ("unsi_caddr", ctypes.c_char * 255)]   # SOCK_MAXADDRLEN


class _Proto(ctypes.Union):
    _fields_ = [("pri_in", _InSockinfo), ("pri_tcp", _TcpSockinfo), ("pri_un", _UnSockinfo)]


class _SocketInfo(ctypes.Structure):
    _fields_ = [("soi_stat", _VinfoStat), ("soi_so", ctypes.c_uint64), ("soi_pcb", ctypes.c_uint64),
                ("soi_type", ctypes.c_int), ("soi_protocol", ctypes.c_int), ("soi_family", ctypes.c_int),
                ("soi_options", ctypes.c_short), ("soi_linger", ctypes.c_short), ("soi_state", ctypes.c_short),
                ("soi_qlen", ctypes.c_short), ("soi_incqlen", ctypes.c_short), ("soi_qlimit", ctypes.c_short),
                ("soi_timeo", ctypes.c_short), ("soi_error", ctypes.c_ushort), ("soi_oobmark", ctypes.c_uint32),
                ("soi_rcv", _SockbufInfo), ("soi_snd", _SockbufInfo), ("soi_kind", ctypes.c_int),
                ("rfu_1", ctypes.c_uint32), ("soi_proto", _Proto)]


class _ProcFileinfo(ctypes.Structure):
    _fields_ = [("fi_openflags", ctypes.c_uint32), ("fi_status", ctypes.c_uint32), ("fi_offset", ctypes.c_int64),
                ("fi_type", ctypes.c_int32), ("fi_guardflags", ctypes.c_uint32)]


class _SocketFdinfo(ctypes.Structure):
    _fields_ = [("pfi", _ProcFileinfo), ("psi", _SocketInfo)]


SOCKET_FDINFO_SIZE = ctypes.sizeof(_SocketFdinfo)                     # 792 on macOS 26 and 27 (arm64, x86_64)


def _address(words: ctypes.Array[ctypes.c_uint32], v4: bool) -> str:
    raw = bytes(words)
    if v4:
        parsed: ipaddress.IPv4Address | ipaddress.IPv6Address = ipaddress.IPv4Address(raw[12:])
    else:
        if raw[0] == 0xfe and raw[1] & 0xc0 == 0x80:                  # link-local: drop a scope the kernel embeds
            raw = raw[:2] + b"\0\0" + raw[4:]
        parsed = ipaddress.IPv6Address(raw)
        if parsed.ipv4_mapped is not None:
            parsed = parsed.ipv4_mapped
    return "*" if parsed.is_unspecified else str(parsed)


def decode(raw: bytes, pid: int) -> Socket | None:
    """One PROC_PIDFDSOCKETINFO answer as a Socket; None for anything but an IPv4/IPv6 TCP or UDP socket."""
    if len(raw) != SOCKET_FDINFO_SIZE:
        raise ValueError("socket_fdinfo has an unexpected size")
    info = _SocketFdinfo.from_buffer_copy(raw).psi
    if info.soi_family not in (socket.AF_INET, socket.AF_INET6):
        return None
    if info.soi_kind == _SOCKINFO_TCP:
        proto, ini = "tcp", info.soi_proto.pri_tcp.tcpsi_ini
        state_value = info.soi_proto.pri_tcp.tcpsi_state
        state = _TCP_STATES[state_value] if 0 <= state_value < len(_TCP_STATES) else f"STATE_{state_value}"
    elif info.soi_kind == _SOCKINFO_IN and info.soi_protocol == socket.IPPROTO_UDP:
        proto, ini, state = "udp", info.soi_proto.pri_in, ""
    else:
        return None
    v4 = bool(ini.insi_vflag & _INI_IPV4) or not (ini.insi_vflag & _INI_IPV6) and info.soi_family == socket.AF_INET
    lport, fport = socket.ntohs(ini.insi_lport & 0xFFFF), socket.ntohs(ini.insi_fport & 0xFFFF)
    remote = (_address(ini.insi_faddr, v4), fport) if fport else None
    return Socket(proto, (_address(ini.insi_laddr, v4), lport), remote, state, pid, max(0, info.soi_qlen))


def _libproc() -> ctypes.CDLL | None:
    try:
        lib = ctypes.CDLL("/usr/lib/libproc.dylib", use_errno=True)
        lib.proc_listpids.argtypes = [ctypes.c_uint32, ctypes.c_uint32, ctypes.c_void_p, ctypes.c_int]
        lib.proc_pidinfo.argtypes = [ctypes.c_int, ctypes.c_int, ctypes.c_uint64, ctypes.c_void_p, ctypes.c_int]
        lib.proc_pidfdinfo.argtypes = [ctypes.c_int, ctypes.c_int, ctypes.c_int, ctypes.c_void_p, ctypes.c_int]
        for f in (lib.proc_listpids, lib.proc_pidinfo, lib.proc_pidfdinfo):
            f.restype = ctypes.c_int
        return lib
    except (OSError, AttributeError):
        return None


_LIB = _libproc()


class _FdInfo(ctypes.Structure):
    _fields_ = [("proc_fd", ctypes.c_int32), ("proc_fdtype", ctypes.c_uint32)]


def _pids(lib: ctypes.CDLL) -> list[int] | None:
    for _ in range(3):
        need = lib.proc_listpids(_PROC_ALL_PIDS, 0, None, 0)
        if need <= 0:
            return None
        buf = (ctypes.c_int * (need // ctypes.sizeof(ctypes.c_int) + 256))()
        got = lib.proc_listpids(_PROC_ALL_PIDS, 0, buf, ctypes.sizeof(buf))
        if got <= 0:
            return None
        if got < ctypes.sizeof(buf):                                  # not cut short by a full buffer
            return [pid for pid in buf[:got // ctypes.sizeof(ctypes.c_int)] if pid > 0]
    return None


def inspectable() -> tuple[list[Socket], set[int]] | None:
    """The TCP and UDP sockets of every process FERB may inspect (the owner's user), and those processes' ids. None
    when libproc can't be read, or answers in a shape this code doesn't know."""
    lib = _LIB
    if lib is None:
        return None
    pids = _pids(lib)
    if pids is None:
        return None
    fds = (_FdInfo * 4096)()
    one = ctypes.create_string_buffer(SOCKET_FDINFO_SIZE)
    sockets: list[Socket] = []
    inspected: set[int] = set()
    for pid in pids:
        got = lib.proc_pidinfo(pid, _PROC_PIDLISTFDS, 0, fds, ctypes.sizeof(fds))
        while got >= ctypes.sizeof(fds):                              # maybe cut short: size it and read again
            need = lib.proc_pidinfo(pid, _PROC_PIDLISTFDS, 0, None, 0)
            fds = (_FdInfo * (max(need, got) // ctypes.sizeof(_FdInfo) + 1024))()
            got = lib.proc_pidinfo(pid, _PROC_PIDLISTFDS, 0, fds, ctypes.sizeof(fds))
        if got <= 0:                                                  # another user's (EPERM) or gone (ESRCH)
            continue
        inspected.add(pid)
        for entry in fds[:got // ctypes.sizeof(_FdInfo)]:
            if entry.proc_fdtype != _PROX_FDTYPE_SOCKET:
                continue
            n = lib.proc_pidfdinfo(pid, entry.proc_fd, _PROC_PIDFDSOCKETINFO, one, SOCKET_FDINFO_SIZE)
            if n <= 0:                                                # closed since the list
                continue
            if n != SOCKET_FDINFO_SIZE:
                return None
            found = decode(one.raw, pid)
            if found is not None:
                sockets.append(found)
    return sockets, inspected


_HEADER = ",state,"
_PROCESS = re.compile(r"(.*)\.(\d+),,")                               # "<name>.<pid>,,"
_SOCKET = re.compile(r"([a-z]+)([46]) (\S+)<->(\S+),([A-Za-z0-9]*),")  # "tcp4 1.2.3.4:80<->*:*,Listen,"


def _nettop_endpoint(text: str, v4: bool) -> tuple[str, int | None] | None:
    address, sep, port = text.rpartition(":" if v4 else ".")
    if not sep or not (port.isdigit() or port == "*"):
        return None
    if address != "*":
        try:
            parsed = ipaddress.ip_address(address.split("%", 1)[0])
        except ValueError:
            return None
        if isinstance(parsed, ipaddress.IPv6Address) and parsed.ipv4_mapped is not None:
            parsed = parsed.ipv4_mapped
        address = "*" if parsed.is_unspecified else str(parsed)
    return address, (int(port) if port.isdigit() else None)


def parse_nettop(text: str) -> list[tuple[int, Socket]] | None:
    """(pid, socket) for each TCP and UDP socket line of `nettop -L 1 -n -x -J state`. Any line in another shape (a
    program name holding a line break, a format change) makes the whole listing unreadable: None."""
    lines = text.splitlines()
    if not lines or lines[0] != _HEADER:
        return None
    out: list[tuple[int, Socket]] = []
    pid = None
    for line in lines[1:]:
        socket_line = _SOCKET.fullmatch(line)
        if socket_line is None:
            process = _PROCESS.fullmatch(line)
            if process is None:
                return None
            pid = int(process[2])
            continue
        if pid is None:
            return None
        proto, family, local_text, remote_text, state = socket_line.groups()
        if proto not in ("tcp", "udp"):                               # a flow over a socket (quic4)
            continue
        local, remote = _nettop_endpoint(local_text, family == "4"), _nettop_endpoint(remote_text, family == "4")
        if local is None or remote is None:
            return None
        if local[1] is None:                                          # not bound to a port: no endpoint to reach
            continue
        far = None if remote[1] is None else (remote[0], remote[1])
        out.append((pid, Socket(proto, (local[0], local[1]), far,
                                _NETTOP_STATES.get(state, state.upper()) if proto == "tcp" else "", pid)))
    return out


def _nettop() -> list[tuple[int, Socket]] | None:
    try:
        r = subprocess.run([NETTOP, "-L", "1", "-n", "-x", "-J", "state"], capture_output=True, text=True,
                           timeout=5, errors="replace")
    except (OSError, subprocess.SubprocessError):
        return None
    return parse_nettop(r.stdout) if r.returncode == 0 else None


def _endpoint_key(s: Socket) -> tuple[str, str, int] | None:
    """A socket anyone can reach, as both readings name it: a TCP listener or an unconnected bound UDP socket."""
    if (s.proto == "tcp" and s.state == "LISTEN") or (s.proto == "udp" and s.remote is None and s.local[1]):
        return s.proto, s.local[0], s.local[1]
    return None


def complete() -> list[Socket] | None:
    """Every TCP and UDP socket on this Mac: the inspectable ones with their process, the rest (another user's,
    root's) with pid None. None when either reading fails, or when nettop doesn't list every listener the inspectable
    reading found (it would be missing other programs' sockets too)."""
    for _ in range(2):                                                # a listener that closed between the readings
        got = inspectable()
        if got is None:
            return None
        sockets, inspected = got
        listed = _nettop()
        if listed is None:
            return None
        known = {key for key in map(_endpoint_key, sockets) if key is not None}
        seen = {key for key in (_endpoint_key(s) for _, s in listed) if key is not None}
        if known and known <= seen:
            return sockets + [replace(s, pid=None) for pid, s in listed if pid not in inspected]
    return None
