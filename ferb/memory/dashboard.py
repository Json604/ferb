"""Owner-launched localhost dashboard. Ephemeral bearer token, exact Host/Origin, no request logging or CORS."""
from __future__ import annotations

import hashlib
import hmac
import json
import secrets
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from typing import Any, Callable

ALLOWED = frozenset({'inventory', 'search', 'get', 'confirm', 'edit', 'forget', 'rollback', 'digest', 'identity', 'retire', 'lesson_feedback'})


class Dashboard:
    def __init__(self, call: Callable[[dict[str, Any]], dict[str, Any]]) -> None:
        self.token = secrets.token_urlsafe(32)
        self.call = call
        app = self
        class Handler(BaseHTTPRequestHandler):
            def log_message(self, *args: Any) -> None:
                pass

            def reply(self, status: int, body: bytes, content_type: str) -> None:
                self.send_response(status)
                self.send_header('Content-Type', content_type)
                self.send_header('Content-Length', str(len(body)))
                self.send_header('Cache-Control', 'no-store')
                self.send_header('Referrer-Policy', 'no-referrer')
                self.send_header('X-Content-Type-Options', 'nosniff')
                self.send_header('Content-Security-Policy', "default-src 'none'; script-src 'self'; style-src 'self'; "
                                 "connect-src 'self'; img-src 'none'; frame-ancestors 'none'; base-uri 'none'; form-action 'none'")
                self.end_headers()
                self.wfile.write(body)

            def valid_host(self) -> bool:
                return self.headers.get('Host') == app.host

            def do_GET(self) -> None:
                if not self.valid_host():
                    self.reply(403, b'Forbidden', 'text/plain')
                    return
                files = {'/': ('dashboard.html', 'text/html; charset=utf-8'),
                         '/app.js': ('dashboard.js', 'text/javascript; charset=utf-8'),
                         '/style.css': ('dashboard.css', 'text/css; charset=utf-8')}
                found = files.get(self.path)
                if not found:
                    self.reply(404, b'Not found', 'text/plain')
                    return
                self.reply(200, Path(__file__).with_name(found[0]).read_bytes(), found[1])

            def do_POST(self) -> None:
                auth = self.headers.get('Authorization', '')
                if (not self.valid_host() or self.path != '/api' or
                    self.headers.get('Origin') != app.origin or
                    not hmac.compare_digest(auth, 'Bearer ' + app.token) or
                    self.headers.get('Content-Type') != 'application/json'):
                    self.reply(403, b'Forbidden', 'text/plain')
                    return
                try:
                    size = int(self.headers.get('Content-Length', '-1'))
                    if size < 0 or self.headers.get('Transfer-Encoding'):
                        raise ValueError('requires a content length')
                    value = json.loads(self.rfile.read(size))
                    if not isinstance(value, dict) or value.get('op') not in ALLOWED:
                        raise ValueError('unsupported memory operation')
                    result = app.call(value)
                    self.reply(200, json.dumps(result, ensure_ascii=False, default=str).encode(), 'application/json')
                except Exception as e:
                    # Do not reflect arbitrary request bytes or daemon exception internals into the page.
                    self.reply(400, json.dumps({'error': str(e)}).encode(), 'application/json')
        self.server = ThreadingHTTPServer(('127.0.0.1', 0), Handler)
        self.host = '127.0.0.1:' + str(self.server.server_address[1])
        self.origin = 'http://' + self.host
        self.url = self.origin + '/#' + self.token

    def serve(self) -> None:
        try:
            self.server.serve_forever()
        finally:
            self.server.server_close()

    def close(self) -> None:
        self.server.shutdown()
        self.server.server_close()
