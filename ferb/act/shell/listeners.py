"""Own finding which servers on this Mac run outside FERB's containment, so the shell's sandbox profile keeps them out
of reach (F5a Task 3: the loopback width). Must not judge what a server is or does: a listening port belongs to a
process inside FERB's sandbox or it doesn't, by fact.

Why: a contained command could otherwise use another program's local server to leave the Mac unrecorded (a local
proxy or VPN helper, an `ssh -L` tunnel, a DNS forwarder, a browser's debugging port) or to run code outside the
sandbox (a notebook server, an agent gateway). Servers a FERB command started itself (a dev server, a test suite's
server) stay reachable, so they must be told apart. Measured on macOS 26.6 (F5a Task 3 report) and 27.2:
- `sockets.complete()` lists every listening TCP socket and every unconnected UDP socket of every user, with the
  process id of each the owner's user runs and none for another user's or root's (about 9 ms). Until macOS 27.2
  this was `netstat -anv`, which now prints nothing when FERB starts it (regression 2026-10-31, sockets.py).
- `sandbox_check` (libsystem_sandbox) answers, for another process, whether its sandbox allows an operation.
  FERB's profile is the one sandbox that denies reading FERB's run folder and allows FERB's workspace: unsandboxed
  programs are allowed both, other sandboxes (Apple's daemons, App Store apps) deny both. A server keeps its
  sandbox when it detaches from the command that started it (pg_ctl, a daemonizing server).
- Seatbelt profiles are fixed when a command starts, so the list is the one at that moment. A server that starts
  listening later (another program's, or a FERB command's) is therefore reached only through FERB's proxy, which
  checks each connection's listener (local.py); the shell profile allows directly only the ports whose listeners
  all ran inside FERB's containment when the command started (sandbox.py)."""

import ctypes
import os
from pathlib import Path
from typing import Any

from ferb.act.shell import sockets

_PATH, _NO_REPORT = 1, 0x40000000                                    # SANDBOX_FILTER_PATH, SANDBOX_CHECK_NO_REPORT


def _sandbox_check() -> Any:
    try:
        f = ctypes.CDLL("/usr/lib/libSystem.B.dylib").sandbox_check
    except (OSError, AttributeError):
        return None
    f.restype = ctypes.c_int
    f.argtypes = [ctypes.c_int, ctypes.c_char_p, ctypes.c_int]       # variadic: the path follows (arm64 needs this)
    return f


_CHECK = _sandbox_check()


def contained(pid: int, ferb_home: Path, workspace: Path) -> bool:
    """Whether the process runs under FERB's shell profile (or mac.applescript's): sandboxed, denied FERB's run folder,
    allowed its workspace. Anything the kernel can't answer for counts as outside (fail closed)."""
    if _CHECK is None:
        return False
    run, ws = (os.path.realpath(p).encode() for p in (ferb_home / "run", workspace))
    try:
        return bool(_CHECK(pid, None, 0) == 1
                and _CHECK(pid, b"file-read-data", _PATH | _NO_REPORT, ctypes.c_char_p(run)) == 1
                and _CHECK(pid, b"file-read-data", _PATH | _NO_REPORT, ctypes.c_char_p(ws)) == 0)
    except (OSError, ctypes.ArgumentError):
        return False


def local_listeners(ferb_home: Path, workspace: Path) -> tuple[set[int], set[int]] | None:
    """(outside, inside): ports of this Mac's servers that run outside FERB's containment, and ports whose every
    listener runs inside it (a FERB command's own server), as listed now. None when the listing can't be read, so the
    caller keeps every local server out."""
    every = sockets.complete()
    if every is None:
        return None
    found = [(s.local[1], s.pid) for s in every if s.local[1]
             and ((s.proto == "tcp" and s.state == "LISTEN") or (s.proto == "udp" and s.remote is None))]
    inside: dict[int, bool] = {}
    for _, pid in found:
        if pid is not None and pid not in inside:
            inside[pid] = contained(pid, ferb_home, workspace)
    outside = {port for port, pid in found if pid is None or not inside[pid]}
    return outside, {port for port, _ in found} - outside


def outside_ports(ferb_home: Path, workspace: Path) -> set[int] | None:
    """Ports of this Mac's servers that run outside FERB's containment (any address: every listener is on one of the
    Mac's own). None when the listing can't be read, so the caller keeps every local server out."""
    both = local_listeners(ferb_home, workspace)
    return None if both is None else both[0]
