"""Owns recording the Claude provider tool list. Must not select task targets, authorize actions or expose tools outside the registered harness.

Tool-list proxy for FERB's Claude runtime only:

- bound to 127.0.0.1; used only as ANTHROPIC_BASE_URL of runs FERB starts;
- forwards only for a live run: the /run/<id>/<secret> prefix must name a run FERB registered (`base_url`) and not yet
  forgotten, with that run's random secret. Anything else gets 403, so a command in FERB's shell (which may reach
  loopback) can't use it to reach the API host (F5a Task 2 review);
- records ONLY the tool names of each request (per run) — never auth headers, prompts, replies or any other body
  content; nothing is logged or written to disk;
- forwards everything else unchanged to the real API."""

import hmac
import http.server
import json
import secrets
import threading

import httpx

UPSTREAM = "https://api.anthropic.com"
_HOP = {"host", "content-length", "connection", "accept-encoding", "transfer-encoding", "content-encoding"}


def tool_names(body: bytes) -> set[str]:
    """Names from a request body's `tools` list; the body itself is discarded by the caller."""
    try:
        data = json.loads(body)
    except (json.JSONDecodeError, UnicodeDecodeError):
        return set()
    tools = data.get("tools") if isinstance(data, dict) else None
    if not isinstance(tools, list):
        return set()
    return {str(t.get("name") or t.get("type") or "?") for t in tools if isinstance(t, dict)}


class ToolListProxy:
    def __init__(self, upstream: str = UPSTREAM) -> None:
        self.upstream = upstream
        self._seen: dict[str, set[str]] = {}
        self._live: dict[str, str] = {}                      # run id -> its secret, while the run lives
        self._lock = threading.Lock()
        proxy = self

        class Handler(http.server.BaseHTTPRequestHandler):
            protocol_version = "HTTP/1.1"

            def _forward(self) -> None:
                prefix, _, rest = self.path.lstrip("/").partition("/")
                run_id, _, rest = rest.partition("/") if prefix == "run" else ("", "", "")
                secret, _, upstream_path = rest.partition("/")
                with proxy._lock:
                    expected = proxy._live.get(run_id)
                if expected is None or not hmac.compare_digest(expected, secret):
                    self.send_error(403)
                    return
                body = self.rfile.read(int(self.headers.get("content-length") or 0))
                if body and upstream_path.startswith("v1/messages"):
                    names = tool_names(body)                   # the only thing kept
                    with proxy._lock:
                        proxy._seen.setdefault(run_id, set()).update(names)
                headers = {k: v for k, v in self.headers.items() if k.lower() not in _HOP}
                with httpx.Client(timeout=600) as c, c.stream(self.command, f"{proxy.upstream}/{upstream_path}",
                                                              headers=headers, content=body) as r:
                    self.send_response(r.status_code)
                    for k, v in r.headers.items():
                        if k.lower() not in _HOP:
                            self.send_header(k, v)
                    self.send_header("transfer-encoding", "chunked")
                    self.end_headers()
                    for chunk in r.iter_bytes():                  # decoded (content-encoding dropped above)
                        self.wfile.write(b"%x\r\n%s\r\n" % (len(chunk), chunk))
                        self.wfile.flush()
                    self.wfile.write(b"0\r\n\r\n")

            do_GET = do_POST = _forward

            def log_message(self, *a: object) -> None:       # never log requests
                pass

        self._srv = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        threading.Thread(target=self._srv.serve_forever, daemon=True).start()

    def base_url(self, run_id: str) -> str:
        """Registers the run (until `forget`) and returns its base URL, carrying a fresh secret."""
        with self._lock:
            secret = self._live.setdefault(run_id, secrets.token_urlsafe(24))
        return f"http://127.0.0.1:{self._srv.server_port}/run/{run_id}/{secret}"

    def seen(self, run_id: str) -> set[str]:
        with self._lock:
            return set(self._seen.get(run_id, set()))

    def forget(self, run_id: str) -> None:
        with self._lock:
            self._seen.pop(run_id, None)
            self._live.pop(run_id, None)

    def close(self) -> None:
        self._srv.shutdown()
        self._srv.server_close()
