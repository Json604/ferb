"""Spike 8 stand-in daemon: one Unix socket, newline-delimited JSON-RPC, run by launchd as a user agent.

Methods: ping (pid, uptime, peer uid), tcc (permission status as seen by *this* process).
Socket: owner-only file in an owner-only directory; stale socket files are replaced at start.
"""

import json
import os
import socket
import stat
import struct
import sys
import threading
import time
from pathlib import Path

RUN_DIR = Path.home() / ".ferb" / "run"
SOCK = RUN_DIR / "spike8.sock"
STARTED = time.monotonic()

SOL_LOCAL, LOCAL_PEERCRED = 0, 1  # <sys/un.h>: struct xucred via getsockopt


def peer_uid(conn: socket.socket) -> int:
    raw = conn.getsockopt(SOL_LOCAL, LOCAL_PEERCRED, 76)  # xucred: u_int version, uid_t uid, short, gid_t[16]
    return struct.unpack_from("Ii", raw)[1]


def tcc() -> dict:
    sys.path.insert(0, str(Path(__file__).parent))
    try:
        import probes as P  # spike 6 probes, copied next to this file by the installer
    except ImportError as e:
        return {"error": f"probes unavailable: {e}"}
    return {"accessibility": P.accessibility().value, "screen_recording": P.screen_recording().value,
            "full_disk_access": P.full_disk_access().value,
            "automation_chrome": P.automation("com.google.Chrome").value}


def handle(req: dict, uid: int) -> dict:
    method = req.get("method")
    if method == "ping":
        return {"pid": os.getpid(), "uptime": round(time.monotonic() - STARTED, 3), "peer_uid": uid,
                "executable": sys.executable}
    if method == "tcc":
        return tcc()
    raise ValueError(f"unknown method {method!r}")


def serve(conn: socket.socket) -> None:
    with conn:
        uid = peer_uid(conn)
        if uid != os.getuid():  # CLI role is proven by socket ownership: same user only
            conn.sendall(json.dumps({"error": "forbidden"}).encode() + b"\n")
            return
        for line in conn.makefile("r"):
            req = json.loads(line)
            try:
                resp = {"id": req.get("id"), "result": handle(req, uid)}
            except Exception as e:  # failures are values across the boundary
                resp = {"id": req.get("id"), "error": str(e)}
            conn.sendall(json.dumps(resp).encode() + b"\n")


def main() -> None:
    RUN_DIR.mkdir(parents=True, exist_ok=True)
    os.chmod(RUN_DIR, 0o700)
    if SOCK.exists():  # stale from a crash: only remove if nobody answers
        probe = socket.socket(socket.AF_UNIX)
        try:
            probe.connect(str(SOCK))
            sys.exit("another daemon is already listening")
        except (ConnectionRefusedError, FileNotFoundError):
            SOCK.unlink(missing_ok=True)
        finally:
            probe.close()
    old = os.umask(0o177)  # socket created 0600
    srv = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    srv.bind(str(SOCK))
    os.umask(old)
    assert stat.S_IMODE(SOCK.stat().st_mode) == 0o600
    srv.listen(8)
    while True:
        conn, _ = srv.accept()
        threading.Thread(target=serve, args=(conn,), daemon=True).start()


if __name__ == "__main__":
    main()
