"""Pass-through proxy for ANTHROPIC_BASE_URL that records what the model is sent.

Records request bodies only (tool list, system prompt); headers carry credentials and are never recorded.
"""

import http.server
import json
import threading

import httpx

UPSTREAM = "https://api.anthropic.com"
_HOP = {"host", "content-length", "connection", "accept-encoding", "transfer-encoding"}


class RecordingProxy:
    def __init__(self) -> None:
        self.bodies: list[dict] = []
        proxy = self

        class Handler(http.server.BaseHTTPRequestHandler):
            protocol_version = "HTTP/1.1"

            def _forward(self) -> None:
                raw = self.rfile.read(int(self.headers.get("content-length") or 0))
                if raw and "/messages" in self.path:
                    try:
                        proxy.bodies.append(json.loads(raw))
                    except json.JSONDecodeError:
                        pass
                headers = {k: v for k, v in self.headers.items() if k.lower() not in _HOP}
                with httpx.Client(timeout=300) as c, c.stream(
                        self.command, UPSTREAM + self.path, headers=headers, content=raw) as r:
                    self.send_response(r.status_code)
                    for k, v in r.headers.items():
                        if k.lower() not in _HOP | {"content-encoding"}:
                            self.send_header(k, v)
                    self.send_header("transfer-encoding", "chunked")
                    self.end_headers()
                    for chunk in r.iter_bytes():  # decoded; content-encoding is dropped above
                        self.wfile.write(b"%x\r\n%s\r\n" % (len(chunk), chunk))
                        self.wfile.flush()
                    self.wfile.write(b"0\r\n\r\n")

            do_POST = do_GET = _forward

            def log_message(self, *a) -> None:
                pass

        self._srv = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self.url = f"http://127.0.0.1:{self._srv.server_port}"

    def __enter__(self) -> "RecordingProxy":
        threading.Thread(target=self._srv.serve_forever, daemon=True).start()
        return self

    def __exit__(self, *exc) -> None:
        self._srv.shutdown()

    def tool_names(self) -> set[str]:
        return {t.get("name") or t.get("type") or json.dumps(t)[:80]
                for b in self.bodies for t in b.get("tools", [])}

    def system_text(self) -> str:
        parts = []
        for b in self.bodies:
            s = b.get("system", "")
            parts.append(s if isinstance(s, str) else " ".join(x.get("text", "") for x in s))
        return "\n".join(parts)
