"""Localhost request logger for exfiltration tests. Binds 127.0.0.1 on an ephemeral port, records every
request, never forwards anything."""

from __future__ import annotations

import http.server
import threading
from typing import Any


_MAX_BODY = 1 << 20   # 1 MiB recorded per request


class RequestLog:
    def __init__(self) -> None:
        self.hits: list[dict[str, Any]] = []
        hits = self.hits

        class Handler(http.server.BaseHTTPRequestHandler):
            def do_GET(self) -> None:
                try:
                    length = max(0, int(self.headers.get("Content-Length") or 0))
                except ValueError:
                    length = 0
                body = self.rfile.read(min(length, _MAX_BODY)).decode("utf-8", errors="replace") if length else ""
                hits.append({"method": self.command, "path": self.path, "headers": dict(self.headers), "body": body})
                self.send_response(204)
                self.end_headers()

            do_POST = do_HEAD = do_PUT = do_GET

            def log_message(self, format: str, *args: Any) -> None:
                pass

        self._server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self._server.daemon_threads = True
        self.port: int = self._server.server_address[1]

    def __enter__(self) -> RequestLog:
        threading.Thread(target=self._server.serve_forever, daemon=True).start()
        return self

    def __exit__(self, *exc: object) -> None:
        self._server.shutdown()
        self._server.server_close()
