"""Writes cases.txt: what the PYTHON native-messaging relay (ferb/native_host/main.py) does with invented messages
(ENG-3). Run from the repo root:

    uv run python rust/fixtures/native-host/make.py

Each case starts the real Python relay (`python -m ferb.native_host.main <origin>`) with FERB_HOME set to a temporary
folder that holds a fake daemon socket, plays a scripted browser on its stdin/stdout, and records the exact bytes
that reached the daemon (`py_hello`, `py_up`), the exact bytes that reached the browser (`py_down`) and the exit
code. Nothing touches ~/.ferb or a real daemon.

The Rust relay forwards the sender's bytes instead of parsing and rewriting them (rust/crates/ferb-native-host/
src/main.rs says why). So each case also records `raw_up` and `raw_down`: the messages Python forwarded, in the
sender's own bytes (whitespace around them dropped, a raw newline between tokens turned into a space). This script
REFUSES to write the fixture unless, for every forwarded message, Python's own parser reads the raw bytes and the
Python relay's bytes as the same value: `json.dumps(json.loads(raw)) == what the Python relay wrote`. That covers
every string exactly, so every signed body too. The Rust test (tests/fixtures.rs) then asserts its bytes equal
`raw_*`, its hello equals `py_hello` and its exit code equals `exit`.

`py_lingered 1` marks the cases where the Python relay did not exit after a daemon line it could not parse (its
reading thread died; main.py:53-57); the recorded exit code is the one after the browser closed stdin. The Rust
relay exits with 1 there instead.

File format: `case <name>` starts a case; then `key value` lines. Byte values are space-separated parts,
`h:<hex>` or `r:<hex>*<count>` (the hex unit repeated), or `-` for no bytes."""

import json
import os
import re
import socket
import struct
import subprocess
import sys
import tempfile
import threading
import time
from pathlib import Path
from typing import Any

REPO = Path(__file__).resolve().parents[3]
OUT = Path(__file__).with_name("cases.txt")
MAX = 1024 * 1024
BOM = b"\xef\xbb\xbf"
WS = b" \t\n\r"
ORIGIN = b"chrome-extension://abcdefghijklmnopabcdefghijklmnop/"
OK = b'{"jsonrpc": "2.0", "id": 1, "result": {"role": "native_host"}}\n'

RUN = re.compile(rb"(.{1,8}?)\1{31,}", re.S)


def enc(data: bytes) -> str:
    if not data:
        return "-"
    parts, pos = [], 0
    for m in RUN.finditer(data):
        if m.start() > pos:
            parts.append("h:" + data[pos:m.start()].hex())
        unit = m.group(1)
        parts.append(f"r:{unit.hex()}*{len(m.group(0)) // len(unit)}")
        pos = m.end()
    if pos < len(data):
        parts.append("h:" + data[pos:].hex())
    return " ".join(parts)


def msg(body: bytes) -> tuple[str, bytes]:
    """A browser frame holding `body`."""
    return ("msg", body)


def raw(data: bytes) -> tuple[str, bytes]:
    """Bytes written to the relay's stdin as they are (a broken or oversized frame)."""
    return ("raw", data)


def compact(value: Any) -> bytes:
    """As a browser writes JSON: no spaces, non-ASCII as UTF-8."""
    return json.dumps(value, ensure_ascii=False, separators=(",", ":")).encode()


CASES: list[dict[str, Any]] = []


def case(name: str, *, origin: bytes | None = ORIGIN, reply: bytes | None = OK, stdin: list[tuple[str, bytes]] = [],
         up_lines: int = 0, daemon: bytes = b"", end: str = "stdin_eof", no_daemon: bool = False) -> None:
    CASES.append(dict(name=name, origin=origin, reply=reply, stdin=stdin, up_lines=up_lines, daemon=daemon, end=end,
                      no_daemon=no_daemon))


# ---------- the invented messages ----------

SIGNED_BODY = json.dumps({"text": "naïve \"q\" \\ / \n 😀 <b>", "n": 1.5, "url": "https://example.test/a?b=1&c=2"},
                         ensure_ascii=False)
SIGNED = compact({"install_id": "inst-1", "type": "event", "body_json": SIGNED_BODY, "seq": 7,
                  "mac": "q83vEjRWeJA9bXlzaWduYXR1cmU="})
SMALL = compact({"type": "ping", "seq": 1})
SMALL2 = compact({"type": "pong", "seq": 2})
DOWN1 = json.dumps({"seq": 1, "type": "cmd", "body": json.dumps({"op": "snapshot", "tab": 3, "q": "é😀"}),
                    "mac": "AAECAwQFBgc="}).encode() + b"\n"
DOWN2 = json.dumps({"type": "welcome", "gen": 4, "ratio": 0.25, "list": [1, None, True, "x"]}).encode() + b"\n"

case("both_directions", stdin=[msg(SMALL), msg(SIGNED)], up_lines=2, daemon=DOWN1 + DOWN2, end="daemon_eof")
case("up_unicode_and_escapes", stdin=[
    msg(SIGNED),
    msg('{"raw":"é 日本 😀 \x7f","esc":"\\u00e9 \\/ \\n \\ud83d\\ude00 \\u0041 \\"","é":[1,2.5,-3,true,null]}'.encode()),
])
case("up_lone_surrogates", stdin=[
    msg(rb'{"type":"t","body_json":"{\"s\":\"\\ud800\"}","lone":"\ud800 \udc00\ud800 \ud83d\ude00","k\udc00":1}'),
    msg(SMALL),
])
case("up_whitespace_and_newlines", stdin=[
    msg(b'\n{\r\n\t"a" :\n[ 1 ,\n2 ]\n,\n"b"\t:\t{ }\n,"s":"line\\nbreak"}\r\n '),
    msg(SMALL),
])
case("up_utf8_bom", stdin=[msg(BOM + b'{"bom":true}'), msg(SMALL)])
case("up_numbers_and_repeated_keys", stdin=[
    msg(b'{"a":1E2,"b":-0,"c":1e999,"d":1.0,"e":123456789012345678901234567890,"f":NaN,"g":-Infinity,'
        b'"h":0.1e-7,"a":2,"i":-0.0,"j":Infinity}'),
])
case("up_at_the_cap_ascii", stdin=[msg(b'{"a":"' + b"a" * (MAX - 8) + b'"}'), msg(SMALL)])
case("up_at_the_cap_non_ascii", stdin=[msg(b'{"a":"' + "é".encode() * ((MAX - 8) // 2) + b'"}'), msg(SMALL)])
case("up_oversized_frame", stdin=[msg(SMALL), raw(struct.pack("<I", MAX + 1) + b"x" * 100), msg(SMALL2)], end="self")
case("up_truncated_body", stdin=[msg(SMALL), raw(struct.pack("<I", 10) + b"abc")])
case("up_truncated_header", stdin=[msg(SMALL), raw(b"\x05\x00")])
case("up_nothing")
for label, bad in [
    ("cut_short", b'{"a":'), ("empty", b""), ("invalid_utf8", b'{"a":"\xff"}'), ("newline_in_string", b'{"a":"x\ny"}'),
    ("trailing_comma", b'{"a":1,}'), ("single_quotes", b"{'a':1}"), ("leading_zero", b'{"a":01}'),
    ("bad_escape", b'{"a":"\\x"}'), ("bad_hex", b'{"a":"\\u12G4"}'), ("two_values", b"{} {}"), ("word", b"nul"),
    ("array", b"[1,2]"), ("string", b'"str"'), ("number", b"7"), ("null", b"null"),
]:
    case(f"up_bad_{label}", stdin=[msg(SMALL), msg(bad), msg(SMALL2)], end="self")

case("hello_refused", reply=b'{"jsonrpc": "2.0", "id": 1, "error": {"code": -32001, "message": "unknown extension"}}\n',
     end="self")
case("hello_closed", reply=None, end="self")
case("hello_not_json", reply=b"nope\n", end="self")
case("hello_array", reply=b"[]\n", end="self")
case("hello_number", reply=b"5\n", end="self")
case("hello_result_only_inside", reply=b'{"error": {"result": 1}}\n', end="self")
case("hello_result_written_with_escapes", reply=b'{"r\\u0065sult": 1}\n', stdin=[msg(SMALL)])
case("no_daemon", no_daemon=True, end="self")
case("origin_missing", origin=None)
case("origin_needs_escaping", origin='chrome-extension://é"\\\x7f\t😀/'.encode())
case("origin_not_utf8", origin=b"chrome-extension://\xff\xed\xa0\x80\xe2\x82/")

case("down_canonical", daemon=DOWN1 + DOWN2 + json.dumps({"s": "\ud800 \udc00", "k\udfff": 1e-7}).encode() + b"\n"
     + json.dumps({"deep": [[[{"a": {"b": []}}]]], "f": 1e22, "i": -5}).encode() + b"\n", end="daemon_eof")
case("down_not_canonical", daemon=b' {"a":"\xc3\xa9", "b" : [1,2]}\r\n' + b'\t[1, "x"]\n' + b'"just a string"\n' + b"5\n"
     + BOM + b'{"bom":1}\n' + b'{"n":1E2,"k":1,"k":2}\n', end="daemon_eof")
case("down_last_line_without_newline", daemon=b'{"a": 1}\n{"b": 2}', end="daemon_eof")
case("down_at_the_cap_then_over", daemon=b'{"a": "' + b"a" * (MAX - 10) + b'"}\n'
     + b'{"a": "' + b"b" * (MAX - 9) + b'"}\n' + b'{"after": 1}\n', end="self")
case("down_not_json", daemon=b'{"a": 1}\nnot json\n{"b": 2}\n', end="daemon_eof")
case("down_blank_line", daemon=b'{"a": 1}\n\n{"b": 2}\n', end="daemon_eof")


# ---------- running the Python relay ----------

def wait(proc: "subprocess.Popen[bytes]", seconds: float) -> int | None:
    try:
        return proc.wait(seconds)
    except subprocess.TimeoutExpired:
        return None


def run(c: dict[str, Any]) -> dict[str, Any]:
    with tempfile.TemporaryDirectory() as home:
        (Path(home) / "run").mkdir()
        listener = None
        if not c["no_daemon"]:
            listener = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
            listener.bind(str(Path(home) / "run" / "ferb.sock"))
            listener.listen(1)
            listener.settimeout(20)
        argv: list[Any] = [sys.executable, "-m", "ferb.native_host.main"] + ([c["origin"]] if c["origin"] is not None else [])
        proc = subprocess.Popen(argv, cwd=REPO, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
                                env={"FERB_HOME": home, "PATH": "/usr/bin:/bin"})
        assert proc.stdin and proc.stdout
        up, down = bytearray(), bytearray()
        hello = b""
        lingered = False
        threads: list[threading.Thread] = []
        conn = None
        if listener is not None:
            conn, _ = listener.accept()
            conn.settimeout(20)
            while not hello.endswith(b"\n"):
                hello += conn.recv(1)
            conn.settimeout(None)
            if c["reply"] is None:
                conn.close()
                conn = None
            else:
                conn.sendall(c["reply"])

        def write_stdin() -> None:
            try:
                proc.stdin.write(b"".join(struct.pack("<I", len(b)) + b if kind == "msg" else b for kind, b in c["stdin"]))
                proc.stdin.flush()
            except OSError:
                pass

        def read_up() -> None:
            try:
                while chunk := conn.recv(65536):
                    up.extend(chunk)
            except OSError:
                pass

        def read_down() -> None:
            while chunk := proc.stdout.read(65536):
                down.extend(chunk)

        writer = threading.Thread(target=write_stdin)
        threads = [writer, threading.Thread(target=read_down)] + ([threading.Thread(target=read_up)] if conn else [])
        for t in threads:
            t.start()
        deadline = time.monotonic() + 30
        while up.count(b"\n") < c["up_lines"]:
            assert time.monotonic() < deadline, c["name"]
            time.sleep(0.005)
        if conn and c["daemon"]:
            try:
                conn.sendall(c["daemon"])
            except OSError:
                pass
        if c["end"] == "stdin_eof":
            writer.join()
            proc.stdin.close()
            code = wait(proc, 30)
        elif c["end"] == "daemon_eof":
            assert conn
            conn.shutdown(socket.SHUT_WR)
            code = wait(proc, 2)
            if code is None:
                lingered = True
                writer.join()
                proc.stdin.close()
                code = wait(proc, 30)
        else:
            code = wait(proc, 30)
        assert code is not None, c["name"]
        for t in threads:
            t.join()
        if conn:
            conn.close()
        if listener:
            listener.close()
        return dict(hello=hello, up=bytes(up), down=bytes(down), exit=code, lingered=lingered)


# ---------- the sender's bytes for what Python forwarded, checked with Python's parser ----------

def same_value(sender: bytes, python: bytes, where: str) -> None:
    assert json.dumps(json.loads(sender)).encode() == python, where


def raw_up(c: dict[str, Any], r: dict[str, Any]) -> bytes:
    lines = r["up"].split(b"\n")
    assert lines.pop() == b"", c["name"]
    out = []
    for kind, body in c["stdin"]:
        if kind != "msg" or len(out) == len(lines):
            break
        try:
            if not isinstance(json.loads(body), dict):
                break
        except ValueError:
            break
        sender = body.removeprefix(BOM).strip(WS).replace(b"\n", b" ")
        same_value(sender, lines[len(out)], c["name"])
        out.append(sender + b"\n")
    assert len(out) == len(lines), (c["name"], len(out), len(lines))
    return b"".join(out)


def raw_down(c: dict[str, Any], r: dict[str, Any]) -> bytes:
    frames, data = [], r["down"]
    while data:
        (n,) = struct.unpack("<I", data[:4])
        frames.append(data[4:4 + n])
        assert len(frames[-1]) == n, c["name"]
        data = data[4 + n:]
    out = []
    for line in c["daemon"].splitlines(keepends=True) if r["hello"] and c["reply"] else []:
        if len(out) == len(frames) or len(line) > MAX:
            break
        sender = line.removeprefix(BOM).strip(WS)
        same_value(sender, frames[len(out)], c["name"])
        out.append(struct.pack("<I", len(sender)) + sender)
    assert len(out) == len(frames), (c["name"], len(out), len(frames))
    return b"".join(out)


def main() -> None:
    text = ["# Written by rust/fixtures/native-host/make.py from ferb/native_host/main.py; do not edit (format: make.py)."]
    for c in CASES:
        r = run(c)
        stdin = b"".join(struct.pack("<I", len(b)) + b if kind == "msg" else b for kind, b in c["stdin"])
        text += ["", f"case {c['name']}",
                 f"origin {enc(c['origin']) if c['origin'] is not None else 'none'}",
                 f"no_daemon {int(c['no_daemon'])}",
                 f"reply {enc(c['reply']) if c['reply'] is not None else 'close'}",
                 f"stdin {enc(stdin)}",
                 f"up_lines {c['up_lines']}",
                 f"daemon {enc(c['daemon'])}",
                 f"end {c['end']}",
                 f"exit {r['exit']}",
                 f"py_lingered {int(r['lingered'])}",
                 f"py_hello {enc(r['hello'])}",
                 f"py_up {enc(r['up'])}",
                 f"py_down {enc(r['down'])}",
                 f"raw_up {enc(raw_up(c, r))}",
                 f"raw_down {enc(raw_down(c, r))}"]
        print(f"{c['name']}: exit {r['exit']}{' (lingered)' if r['lingered'] else ''}, "
              f"up {len(r['up'])} bytes, down {len(r['down'])} bytes")
    OUT.write_text("\n".join(text) + "\n")
    print(f"wrote {OUT} ({len(CASES)} cases)")


if __name__ == "__main__":
    main()
