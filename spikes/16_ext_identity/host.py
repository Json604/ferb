"""Spike 16 relay (from spike 4, plus spike 12's fix): native messages <-> Unix socket; exits when either side
closes. Installed to ~/.ferb/spike16 (Chrome can't execute from ~/Desktop)."""

import json
import os
import socket
import struct
import sys
import threading


def read_native() -> dict | None:
    head = sys.stdin.buffer.read(4)
    if len(head) < 4:
        return None
    (n,) = struct.unpack("<I", head)
    return json.loads(sys.stdin.buffer.read(n))


def write_native(msg: dict) -> None:
    data = json.dumps(msg).encode()
    sys.stdout.buffer.write(struct.pack("<I", len(data)) + data)
    sys.stdout.buffer.flush()


def main(sock_path: str) -> None:
    sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    sock.connect(sock_path)
    rfile = sock.makefile("r")

    def daemon_to_ext() -> None:
        for line in rfile:
            write_native(json.loads(line))
        os._exit(0)                               # daemon gone: exit so the extension reconnects cleanly

    threading.Thread(target=daemon_to_ext, daemon=True).start()
    while (msg := read_native()) is not None:
        sock.sendall(json.dumps(msg).encode() + b"\n")


if __name__ == "__main__":
    main(sys.argv[1])
