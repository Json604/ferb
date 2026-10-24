"""A loopback stand-in for a model service, for request-shape tests against the pinned CLIs (Claude Code, Codex).

It records each request body (an HTTP POST, or the first message of a WebSocket, which Codex 0.157.1 uses for
/v1/responses) and answers with an error, so no model runs, no credential is needed and nothing leaves the Mac. Only
bodies are kept, never headers."""
import base64
import hashlib
import http.server
import json
import struct
import threading
from typing import Any

_WS_GUID = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11"
_ERROR = {"type": "invalid_request_error", "message": "loopback test server: no model here"}


def _read_frame(rfile: Any) -> tuple[int | None, bytes, bool]:
    head = rfile.read(2)
    if len(head) < 2:
        return None, b"", True
    op, size = head[0] & 0x0F, head[1] & 0x7F
    if size == 126:
        size = struct.unpack(">H", rfile.read(2))[0]
    elif size == 127:
        size = struct.unpack(">Q", rfile.read(8))[0]
    mask = rfile.read(4) if head[1] & 0x80 else b""
    data = bytearray(rfile.read(size))
    if mask:
        for i in range(len(data)):
            data[i] ^= mask[i % 4]
    return op, bytes(data), bool(head[0] & 0x80)


def _text_frame(payload: bytes) -> bytes:
    size = len(payload)
    return (bytes([0x81, size]) if size < 126 else bytes([0x81, 126]) + struct.pack(">H", size)) + payload


class ModelLoopback:
    def __init__(self) -> None:
        self._bodies: list[str] = []
        self._lock = threading.Lock()
        recorder = self

        class Handler(http.server.BaseHTTPRequestHandler):
            protocol_version = "HTTP/1.1"

            def _record(self, body: bytes) -> None:
                with recorder._lock:
                    recorder._bodies.append(body.decode("utf-8", "replace"))

            def do_POST(self) -> None:
                self._record(self.rfile.read(int(self.headers.get("content-length") or 0)))
                reply = json.dumps({"type": "error", "error": _ERROR}).encode()
                self.send_response(400)
                self.send_header("content-type", "application/json")
                self.send_header("content-length", str(len(reply)))
                self.end_headers()
                self.wfile.write(reply)

            def do_GET(self) -> None:
                key = self.headers.get("Sec-WebSocket-Key")
                if not key:
                    self.send_response(404)
                    self.send_header("content-length", "0")
                    self.end_headers()
                    return
                self.send_response(101)
                self.send_header("Upgrade", "websocket")
                self.send_header("Connection", "Upgrade")
                self.send_header("Sec-WebSocket-Accept",
                                 base64.b64encode(hashlib.sha1((key + _WS_GUID).encode()).digest()).decode())
                self.end_headers()
                self.wfile.flush()
                message = b""
                while True:
                    op, data, fin = _read_frame(self.rfile)
                    if op is None or op == 8:
                        break
                    if op in (0, 1):
                        message += data
                        if fin:
                            self._record(message)
                            break
                self.wfile.write(_text_frame(json.dumps({"type": "error", "status": 400, "error": _ERROR}).encode())
                                 + bytes([0x88, 0]))
                self.wfile.flush()
                self.close_connection = True

            def log_message(self, *args: Any) -> None:
                pass

        self._server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self.port = self._server.server_port
        threading.Thread(target=self._server.serve_forever, daemon=True).start()

    @property
    def url(self) -> str:
        return f"http://127.0.0.1:{self.port}"

    def bodies(self) -> list[dict[str, Any]]:
        """Every recorded body that is a JSON object, in arrival order."""
        with self._lock:
            raw = list(self._bodies)
        out = []
        for text in raw:
            try:
                body = json.loads(text)
            except ValueError:
                continue
            if isinstance(body, dict):
                out.append(body)
        return out

    def close(self) -> None:
        self._server.shutdown()
        self._server.server_close()
