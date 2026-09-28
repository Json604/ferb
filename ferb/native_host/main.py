"""ferb-native-host (section 2): the native-messaging host Chrome/Brave launch for the FERB extension. It only
relays: native messages (4-byte little-endian length + JSON, stdin/stdout) <-> newline-delimited JSON on the daemon
socket. It checks nothing but size; the daemon authenticates the caller origin and every message.

Exits as soon as either side closes (spike 12): a host that outlives its daemon connection would swallow the
extension's next message. Never logs message contents.
"""

import json
import os
import socket
import struct
import sys
import threading
from typing import Any, BinaryIO

from ferb.paths import socket_path

MAX_MESSAGE = 1024 * 1024


def read_native(stream: BinaryIO) -> dict[str, Any] | None:
    head = stream.read(4)
    if len(head) < 4:
        return None
    (n,) = struct.unpack("<I", head)
    if n > MAX_MESSAGE:
        return None                                   # oversized: end the connection rather than buffer it
    body = stream.read(n)
    if len(body) < n:
        return None
    msg = json.loads(body)
    return msg if isinstance(msg, dict) else None


def write_native(stream: BinaryIO, msg: dict[str, Any]) -> None:
    data = json.dumps(msg).encode()
    stream.write(struct.pack("<I", len(data)) + data)
    stream.flush()


def relay(origin: str, sock_path: str, stdin: BinaryIO, stdout: BinaryIO) -> int:
    sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    try:
        sock.connect(sock_path)
    except OSError:
        return 1                                      # daemon not running: the extension retries with backoff
    rfile = sock.makefile("rb")
    sock.sendall(json.dumps({"jsonrpc": "2.0", "id": 1, "method": "auth.hello",
                             "params": {"role": "native_host", "origin": origin}}).encode() + b"\n")
    reply = json.loads(rfile.readline() or b"{}")
    if "result" not in reply:
        return 2                                      # refused (e.g. an extension id FERB doesn't know)

    def daemon_to_ext() -> None:
        for line in rfile:
            if len(line) > MAX_MESSAGE:
                break
            write_native(stdout, json.loads(line))
        os._exit(0)                                   # daemon gone: exit so the extension reconnects cleanly

    threading.Thread(target=daemon_to_ext, daemon=True).start()
    while (msg := read_native(stdin)) is not None:
        sock.sendall(json.dumps(msg).encode() + b"\n")
    return 0


def main() -> None:
    # Chrome passes the caller's origin (chrome-extension://<id>/) as the first argument.
    origin = sys.argv[1] if len(sys.argv) > 1 else ""
    sys.exit(relay(origin, str(socket_path()), sys.stdin.buffer, sys.stdout.buffer))


if __name__ == "__main__":
    main()
