"""Own FERB's egress proxy for its shell (S2, D28): the only way off this Mac for a contained command. Must not read,
keep or judge content: per process it records each destination's host, port, byte counts and connections, never
bodies, headers, paths or query strings (rule 14).

- Each shell process gets its own credentials (its process id and a random token, kept in memory only), so a
  connection is attributed to the process (and task) that made it. A wrong, missing or forgotten credential gets 407
  and nothing is forwarded. The token is a secret value: the redactor scrubs it from the process's output, logs and
  spans. Another contained process can't read it (its environment isn't readable: measured, F5a Task 3 report).
- `allow(proc, host, port)` (the gateway: S4, Task 5) is asked before any name is resolved, so a refused name never
  reaches a DNS server; a refusal is 403 and is recorded under `refused` with FERB's reason.
- The proxy resolves names itself (the sandbox has no DNS) and connects only to an address it checked. This Mac's
  own addresses are refused: a contained command reaches the Mac's servers directly, and only those started inside
  FERB's shell (sandbox.py, listeners.py); the proxy must not reopen the others.
- CONNECT tunnels any TCP port as opaque bytes (TLS stays end to end). A plain http:// request is forwarded byte for
  byte on its own connection, closed after the response, so each request is gated and recorded for its own host.
  Bytes are counted before they are passed on, so a record is complete by the time the client has the data. A
  connection that FERB's shell opened and a setsid child keeps after the shell exits goes on being counted: `forget`
  revokes the credential but the process's record is kept and reported only once its last connection closes.
- Loopback traffic doesn't come here (NO_PROXY). Nothing is logged; byte counts are recorded, never capped.
- Abuse is bounded without an owner-visible limit: a connection that hasn't authenticated gets a short timeout, and
  only unauthenticated connections are capped; an authenticated tunnel has no idle timeout, so streaming APIs,
  WebSockets and SSE stay up, and there is no per-credential cap. A malformed request or an over-long response head
  is answered, never logged with its bytes (rule 14)."""

import base64
import errno
import hmac
import http.client
import http.server
import ipaddress
import secrets
import socket
import threading
from typing import Any, Callable, Protocol
from urllib.parse import urlsplit

Allow = Callable[[str, str, int], tuple[bool, str]]
Address = tuple[int, tuple[Any, ...]]                       # (family, sockaddr) as getaddrinfo gives it
Resolve = Callable[[str, int], list[Address]]
Report = Callable[[str, dict[str, Any]], None]
OWN = "this Mac's own address: a command reaches this Mac's servers directly, and only ones started in FERB's shell"
# Not forwarded: they describe this hop, not the request (RFC 9110 7.6.1), or carry FERB's credential. Upgrade and
# Connection are kept for an upgrade request (a WebSocket handshake), handled apart.
HOP = {"proxy-authorization", "proxy-connection", "connection", "keep-alive", "te", "trailer", "upgrade", "expect",
       "proxy-authenticate"}
CHUNK = 65536
UNAUTH_TIMEOUT = 10.0                                       # seconds an unauthenticated/incomplete request head may idle
MAX_UNAUTH = 1024                                           # connections not yet authenticated held at once (not a cap
#                                                            on legitimate use: an authenticated tunnel leaves this set
#                                                            at once, so only idle/stalled heads ever occupy it)
MAX_HEAD = 256 * 1024                                       # a response head larger than this is malformed: 502


class Secrets(Protocol):
    def add_secret_value(self, value: str) -> None: ...
    def forget_secret_value(self, value: str) -> None: ...


class Unreachable(Exception):
    """Why the proxy won't or can't connect to a destination (FERB's words; recorded under `refused`)."""


def this_mac(ip: str) -> bool:
    """One of this Mac's own addresses: loopback, unspecified (0.0.0.0 reaches loopback), or bound to an interface
    (its LAN or VPN address): only an own address can be bound."""
    try:
        addr = ipaddress.ip_address(ip.split("%", 1)[0])
    except ValueError:
        return False
    if isinstance(addr, ipaddress.IPv6Address) and addr.ipv4_mapped is not None:
        addr = addr.ipv4_mapped
    if addr.is_loopback or addr.is_unspecified:
        return True
    family = socket.AF_INET if addr.version == 4 else socket.AF_INET6
    with socket.socket(family, socket.SOCK_STREAM) as s:
        try:
            s.bind((ip, 0))
        except OSError:
            return False
    return True


def addresses(host: str, port: int) -> list[Address]:
    """Where the proxy may connect for host:port: resolved here, never one of this Mac's own addresses (a name that
    resolves to one is refused outright, whatever else it resolves to)."""
    try:
        infos = socket.getaddrinfo(host, port, type=socket.SOCK_STREAM)
    except OSError as e:
        raise Unreachable(f"couldn't resolve the name ({type(e).__name__})") from None
    found: list[Address] = list(dict.fromkeys((int(family), tuple(sockaddr)) for family, _, _, _, sockaddr in infos))
    if any(this_mac(str(sockaddr[0])) for _, sockaddr in found):
        raise Unreachable(OWN)
    return found


def _literal(host: str) -> bool:
    try:
        ipaddress.ip_address(host.split("%", 1)[0])
    except ValueError:
        return False
    return True


class EgressProxy:
    def __init__(self, allow: Allow, *, resolve: Resolve = addresses, redactor: Secrets | None = None,
                 excluded: Callable[[str], bool] | None = None, on_report: Report | None = None) -> None:
        self.allow, self.resolve, self.redactor = allow, resolve, redactor
        self.excluded = excluded or (lambda host: False)    # the owner's exclusion list: such a host is never shown
        self.on_report = on_report                          # a process is fully done: its final record (host/bytes)
        self._tokens: dict[str, str] = {}                   # proc id -> token, while FERB tracks the process
        self._facts: dict[str, dict[str, Any]] = {}
        self._active: dict[str, int] = {}                   # open connections per process (a tunnel may outlive it)
        self._ending: set[str] = set()                      # forgotten, waiting for its last connection to close
        self._unauth = 0                                    # connections not yet authenticated (bounded)
        self._lock = threading.Lock()
        proxy = self

        class Handler(http.server.BaseHTTPRequestHandler):
            protocol_version = "HTTP/1.1"
            rbufsize = 0                                     # unbuffered: a tunnel's first bytes stay on the socket
            _counted = False                                 # this connection is in the unauthenticated count

            def setup(self) -> None:
                super().setup()
                self.connection.settimeout(UNAUTH_TIMEOUT)   # an unauthenticated/stalled head can't hold a slot forever
                with proxy._lock:
                    proxy._unauth += 1
                    self._counted = True
                    self._over = proxy._unauth > MAX_UNAUTH

            def handle(self) -> None:
                if self._over:                               # too many unauthenticated connections: drop this one at once
                    self._uncount()
                    return
                super().handle()

            def finish(self) -> None:
                self._uncount()
                super().finish()

            def _uncount(self) -> None:
                if self._counted:
                    self._counted = False
                    with proxy._lock:
                        proxy._unauth -= 1

            def _authenticated(self) -> None:
                self._uncount()                              # leaves the unauthenticated set; no idle timeout from here
                self.connection.settimeout(None)

            def _who(self) -> str | None:
                scheme, _, raw = self.headers.get("Proxy-Authorization", "").partition(" ")
                if scheme.lower() != "basic":
                    return None
                try:
                    user, _, token = base64.b64decode(raw.strip(), validate=True).decode().partition(":")
                except (ValueError, UnicodeDecodeError):
                    return None
                with proxy._lock:
                    expected = proxy._tokens.get(user)
                try:
                    return user if expected is not None and hmac.compare_digest(expected, token) else None
                except TypeError:                            # a non-ASCII token: not one FERB issued
                    return None

            def _reply(self, code: int, text: str = "", extra: dict[str, str] | None = None) -> None:
                body = (text + "\n").encode() if text else b""
                self.send_response(code)
                for k, v in (extra or {}).items():
                    self.send_header(k, v)
                self.send_header("Content-Type", "text/plain; charset=utf-8")
                self.send_header("Content-Length", str(len(body)))
                self.send_header("Connection", "close")
                self.end_headers()
                if self.command != "HEAD":
                    self.wfile.write(body)
                self.close_connection = True

            def _open(self, host: str, port: int) -> tuple[str, socket.socket] | None:
                """The checked connection for this request, or None after answering 407/403/502."""
                proc = self._who()
                if proc is None:
                    self._reply(407, "FERB: proxy credentials required", {"Proxy-Authenticate": 'Basic realm="FERB"'})
                    return None
                self._authenticated()
                host = host.lower()
                try:
                    if _literal(host) and this_mac(host):    # before asking: an address needs no DNS
                        raise Unreachable(OWN)
                    ok, why = proxy.allow(proc, host, port)
                    if not ok:
                        raise Unreachable(why or "FERB refused this destination")
                    where = proxy.resolve(host, port)
                except Unreachable as e:
                    proxy._note(proc, host, port, refused=str(e))
                    self._reply(403, f"FERB: {e}")
                    return None
                for family, sockaddr in where:
                    upstream = None
                    try:
                        upstream = socket.socket(family, socket.SOCK_STREAM)
                        upstream.settimeout(30)
                        upstream.connect(sockaddr)
                        upstream.settimeout(None)
                        proxy._opened(proc, host, port)
                        return proc, upstream
                    except OSError:
                        if upstream is not None:
                            upstream.close()
                self._reply(502, "FERB: couldn't connect to the destination")
                return None

            def do_CONNECT(self) -> None:
                try:
                    parts = urlsplit("//" + self.path)
                    host, port = parts.hostname, parts.port
                except ValueError:
                    host = port = None
                if not host or not port:
                    self._reply(400, "FERB: CONNECT needs host:port")
                    return
                opened = self._open(host, port)
                if opened is None:
                    return
                proc, upstream = opened
                host = host.lower()
                try:
                    self.send_response(200, "Connection established")
                    self.end_headers()
                    proxy._pump(self.connection, upstream, proc, host, port)
                finally:
                    upstream.close()
                    proxy._closed(proc)
                    self.close_connection = True

            def _forward(self) -> None:                     # plain http://host/... through the proxy
                try:
                    url = urlsplit(self.path)
                    host, port = url.hostname, url.port or 80
                except ValueError:
                    url, host, port = None, None, 0
                if url is None or url.scheme.lower() != "http" or not host:
                    self._reply(400, "FERB: send an absolute http:// URL, or CONNECT for anything else")
                    return
                conn = {t.strip().lower() for t in self.headers.get("Connection", "").split(",")}
                upgrading = "upgrade" in conn and bool(self.headers.get("Upgrade"))   # a WebSocket handshake, say
                opened = self._open(host, port)
                if opened is None:
                    return
                proc, upstream = opened
                host = host.lower()
                target = (url.path or "/") + (f"?{url.query}" if url.query else "")
                drop = HOP - {"upgrade", "connection"} if upgrading else HOP
                head = [f"{self.command} {target} HTTP/1.1", f"Host: {url.netloc.rpartition('@')[2]}"]
                if not upgrading:
                    head.append("Connection: close")
                head += [f"{k}: {v}" for k, v in self.headers.items() if k.lower() not in drop | {"host"}]
                try:
                    proxy._send(upstream, ("\r\n".join(head) + "\r\n\r\n").encode(), proc, host, port, "bytes_out")
                    self._body(upstream, proc, host, port)
                    self._response(upstream, proc, host, port, upgrading)
                except ValueError:                           # a malformed chunk size or Content-Length
                    self._reply(400, "FERB: the request framing is malformed")
                except OSError:
                    pass
                finally:
                    upstream.close()
                    proxy._closed(proc)
                    self.close_connection = True

            def _body(self, upstream: socket.socket, proc: str, host: str, port: int) -> None:
                """The request body exactly as framed (Content-Length or chunked), nothing after it."""
                if self.headers.get("Transfer-Encoding", "").lower().endswith("chunked"):
                    while True:
                        line = self.rfile.readline(CHUNK)
                        proxy._send(upstream, line, proc, host, port, "bytes_out")
                        size = int(line.split(b";", 1)[0].strip() or b"0", 16)
                        if size == 0:
                            while (trailer := self.rfile.readline(CHUNK)) not in (b"\r\n", b"\n", b""):
                                proxy._send(upstream, trailer, proc, host, port, "bytes_out")
                            proxy._send(upstream, b"\r\n", proc, host, port, "bytes_out")
                            return
                        self._copy(upstream, size + 2, proc, host, port)
                else:
                    self._copy(upstream, int(self.headers.get("Content-Length") or 0), proc, host, port)

            def _copy(self, upstream: socket.socket, n: int, proc: str, host: str, port: int) -> None:
                while n > 0 and (chunk := self.rfile.read(min(n, CHUNK))):
                    proxy._send(upstream, chunk, proc, host, port, "bytes_out")
                    n -= len(chunk)

            def _read_head(self, upstream: socket.socket, proc: str, host: str, port: int,
                           pre: bytes) -> tuple[bytes, bytes] | None:
                """One response head and the bytes read past it, or None when upstream closed first (its bytes are
                flushed). A head over MAX_HEAD is malformed: 502. The search looks only at newly read bytes: linear."""
                buf = bytearray(pre)
                cut = buf.find(b"\r\n\r\n")
                while cut == -1:
                    start = max(0, len(buf) - 3)              # "\r\n\r\n" may straddle the previous read's last 3 bytes
                    chunk = upstream.recv(CHUNK)
                    if not chunk:
                        self.wfile.write(bytes(buf))
                        return None
                    proxy._count(proc, host, port, "bytes_in", len(chunk))
                    buf += chunk
                    cut = buf.find(b"\r\n\r\n", start)
                    if cut == -1 and len(buf) > MAX_HEAD:
                        self._reply(502, "FERB: the upstream response head is too large")
                        return None
                return bytes(buf[:cut]), bytes(buf[cut + 4:])

            def _response(self, upstream: socket.socket, proc: str, host: str, port: int, upgrading: bool) -> None:
                """Interim (1xx) heads pass as they are; a 101 switches to a tunnel; the final head says the connection
                closes; then the body streams (SSE, chunked, Content-Length) until upstream closes."""
                rest = b""
                while True:
                    got = self._read_head(upstream, proc, host, port, rest)
                    if got is None:
                        return
                    head, rest = got
                    lines = head.split(b"\r\n")
                    fields = lines[0].split(b" ")
                    code = fields[1] if len(fields) > 1 and fields[1].isdigit() else b""
                    if code == b"101" and upgrading:         # a protocol switch: forward the head, then bridge both ways
                        self.wfile.write(head + b"\r\n\r\n" + rest)
                        proxy._pump(self.connection, upstream, proc, host, port)
                        return
                    if code.startswith(b"1"):                 # a genuine interim response: forward and read the next head
                        self.wfile.write(head + b"\r\n\r\n")
                        continue                             # `rest` carries the bytes past it into the next head
                    kept = [lines[0]] + [h for h in lines[1:]
                                         if h.split(b":", 1)[0].strip().lower().decode("latin-1") not in HOP]
                    self.wfile.write(b"\r\n".join(kept + [b"Connection: close"]) + b"\r\n\r\n" + rest)
                    break
                while chunk := upstream.recv(CHUNK):
                    proxy._count(proc, host, port, "bytes_in", len(chunk))
                    self.wfile.write(chunk)

            do_GET = do_POST = do_PUT = do_DELETE = do_HEAD = do_PATCH = do_OPTIONS = _forward

            def log_message(self, *a: object) -> None:      # never log requests
                pass

        class Server(http.server.ThreadingHTTPServer):
            daemon_threads = True

            def handle_error(self, request: Any, client_address: Any) -> None:
                import sys
                exc = sys.exc_info()[1]                       # the type only (rule 14): request bytes are never logged
                proxy._last_error = type(exc).__name__ if exc else "?"

            def get_request(self) -> Any:
                try:
                    return self.socket.accept()
                except OSError as e:
                    if e.errno == errno.EMFILE:              # out of descriptors: don't spin a hot accept loop
                        import time
                        time.sleep(0.05)
                    raise

        self._last_error: str | None = None
        self._Handler = Handler
        self._srv = Server(("127.0.0.1", 0), Handler)
        self._thread: threading.Thread | None = None
        self.ensure_serving()

    @property
    def port(self) -> int:
        return int(self._srv.server_port)

    def ensure_serving(self) -> None:
        """Start the serving thread, or restart it if it died (doctor calls this). While it is down the shell has no
        network at all, so it fails closed."""
        with self._lock:
            if self._thread is not None and self._thread.is_alive():
                return
            self._thread = threading.Thread(target=self._srv.serve_forever, daemon=True)
            self._thread.start()

    def probe(self) -> bool:
        """A liveness check: an unauthenticated request must get 407. Restarts a dead serving thread first."""
        self.ensure_serving()
        try:
            c = http.client.HTTPConnection("127.0.0.1", self.port, timeout=5)
            c.request("GET", "http://127.0.0.1/")
            ok = c.getresponse().status == 407
            c.close()
            return ok
        except OSError:
            return False

    def credentials(self, proc_id: str) -> str:
        """This process's token (made on first use, until `forget`)."""
        with self._lock:
            token = self._tokens.get(proc_id)
            if token is None:
                token = self._tokens[proc_id] = secrets.token_urlsafe(24)
                self._facts.setdefault(proc_id, {"destinations": {}, "refused": {}})
                new = True
            else:
                new = False
        if new and self.redactor is not None:
            self.redactor.add_secret_value(token)
        return token

    def env(self, proc_id: str) -> dict[str, str]:
        """The process's proxy variables: every common spelling, so curl, git, Python, Go, Codex and Node (with
        NODE_USE_ENV_PROXY) all use the proxy; a client that ignores them has no network (the sandbox)."""
        url = f"http://{proc_id}:{self.credentials(proc_id)}@127.0.0.1:{self.port}"
        local = "localhost,127.0.0.1,::1"
        return {**{k: url for k in ("HTTP_PROXY", "HTTPS_PROXY", "ALL_PROXY", "http_proxy", "https_proxy",
                                    "all_proxy")},
                "NO_PROXY": local, "no_proxy": local, "NODE_USE_ENV_PROXY": "1"}

    def facts(self, proc_id: str) -> dict[str, Any]:
        """What the process reached and what FERB refused it, as recorded so far; an excluded host shows as
        "excluded"; refusals are aggregated by (host, port, why) with a count."""
        with self._lock:
            return self._facts_locked(proc_id)

    def _facts_locked(self, proc_id: str) -> dict[str, Any]:
        f = self._facts.get(proc_id) or {"destinations": {}, "refused": {}}
        dests: dict[tuple[str, int], dict[str, int]] = {}
        for (host, port), v in sorted(f["destinations"].items()):
            shown = dests.setdefault(("excluded" if self.excluded(host) else host, port),
                                     {"bytes_out": 0, "bytes_in": 0, "connections": 0})
            for k in shown:
                shown[k] += v[k]
        refused: dict[tuple[str, int, str], int] = {}
        for (host, port, why), count in sorted(f["refused"].items()):
            refused[("excluded" if self.excluded(host) else host, port, why)] = \
                refused.get(("excluded" if self.excluded(host) else host, port, why), 0) + count
        return {"destinations": [{"host": h, "port": p, **v} for (h, p), v in dests.items()],
                "refused": [{"host": h, "port": p, "why": w, "count": c} for (h, p, w), c in refused.items()]}

    def forget(self, proc_id: str) -> None:
        """The process is done with: its credential stops working at once. Its record is kept and reported only once
        its last connection closes (a setsid child may still be streaming through a tunnel), so late bytes still
        count. With no connection open, it is reported now."""
        with self._lock:
            token = self._tokens.pop(proc_id, None)
            if proc_id not in self._facts:
                report = None
            elif self._active.get(proc_id, 0) > 0:
                self._ending.add(proc_id)
                report = None
            else:
                report = self._take(proc_id)
        if token is not None and self.redactor is not None:
            self.redactor.forget_secret_value(token)
        if report is not None and self.on_report is not None:
            self.on_report(proc_id, report)

    def _take(self, proc_id: str) -> dict[str, Any]:
        """The final record; drops the process (caller holds the lock)."""
        final = self._facts_locked(proc_id)
        self._facts.pop(proc_id, None)
        self._ending.discard(proc_id)
        self._active.pop(proc_id, None)
        return final

    def _opened(self, proc: str, host: str, port: int) -> None:
        with self._lock:
            f = self._facts.setdefault(proc, {"destinations": {}, "refused": {}})
            d = f["destinations"].setdefault((host, port), {"bytes_out": 0, "bytes_in": 0, "connections": 0})
            d["connections"] += 1
            self._active[proc] = self._active.get(proc, 0) + 1

    def _closed(self, proc: str) -> None:
        """A connection ended. If the process was forgotten and this was its last connection, report it now (with the
        late bytes counted)."""
        report = None
        with self._lock:
            if proc in self._active:
                self._active[proc] -= 1
                if self._active[proc] <= 0:
                    self._active.pop(proc, None)
                    if proc in self._ending:
                        report = self._take(proc)
        if report is not None and self.on_report is not None:
            self.on_report(proc, report)

    def _note(self, proc: str, host: str, port: int, *, refused: str) -> None:
        with self._lock:
            f = self._facts.setdefault(proc, {"destinations": {}, "refused": {}})
            f["refused"][(host, port, refused)] = f["refused"].get((host, port, refused), 0) + 1

    def _count(self, proc: str, host: str, port: int, key: str, n: int) -> None:
        with self._lock:
            f = self._facts.get(proc)
            if f is not None and (host, port) in f["destinations"]:
                f["destinations"][(host, port)][key] += n

    def _send(self, sock: socket.socket, data: bytes, proc: str, host: str, port: int, key: str) -> None:
        self._count(proc, host, port, key, len(data))
        sock.sendall(data)

    def _pump(self, client: socket.socket, upstream: socket.socket, proc: str, host: str, port: int) -> None:
        def copy(src: socket.socket, dst: socket.socket, key: str) -> None:
            try:
                while chunk := src.recv(CHUNK):
                    self._send(dst, chunk, proc, host, port, key)
            except OSError:
                pass
            finally:
                try:
                    dst.shutdown(socket.SHUT_WR)
                except OSError:
                    pass
        t = threading.Thread(target=copy, args=(upstream, client, "bytes_in"), daemon=True)
        t.start()
        copy(client, upstream, "bytes_out")
        t.join()
        upstream.close()

    def close(self) -> None:
        """Stop serving. The serving loop notices within its 0.5 s poll; nothing waits for that (daemon stops and
        tests would)."""
        def stop() -> None:
            self._srv.shutdown()
            self._srv.server_close()
        threading.Thread(target=stop, daemon=True).start()
