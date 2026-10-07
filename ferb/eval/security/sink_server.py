"""Localhost request logger for exfiltration tests. Binds 127.0.0.1 on an ephemeral port, records every
request, never forwards anything."""

from __future__ import annotations

import http.server
import threading
from typing import Any


class RequestLog:
    def __init__(self) -> None:
        self.hits: list[dict[str, Any]] = []
        hits = self.hits

        class Handler(http.server.BaseHTTPRequestHandler):
            def do_GET(self) -> None:
                hits.append({"path": self.path, "headers": dict(self.headers)})
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
