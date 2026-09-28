"""Spike 12 copy of spike 4's stand-in for ferb-native-host: relays native messages <-> a daemon Unix socket. No logic.

Native messaging framing: 4-byte little-endian length + UTF-8 JSON, on stdin/stdout.
Daemon side: newline-delimited JSON. Socket path comes from argv[1] (baked into the launcher script).
"""

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
        # Daemon gone: exit now so the browser sees the disconnect and the extension reconnects on its next
        # message (otherwise the extension keeps writing to a dead relay and those messages are lost).
        os._exit(0)

    threading.Thread(target=daemon_to_ext, daemon=True).start()
    while (msg := read_native()) is not None:
        sock.sendall(json.dumps(msg).encode() + b"\n")


if __name__ == "__main__":
    # argv from Chrome: [host, <origin>, (--parent-window=…)]; our launcher passes the socket first.
    main(sys.argv[1])
