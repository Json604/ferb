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
  Bytes are counted before they are passed on, so a record is complete by the time the client has the data.
- Loopback traffic doesn't come here (NO_PROXY). Nothing is logged; byte counts are recorded, never capped."""

import base64
import hmac
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
OWN = "this Mac's own address: a command reaches this Mac's servers directly, and only ones started in FERB's shell"
# Not forwarded: they describe this hop, not the request (RFC 9110 7.6.1), or carry FERB's credential
HOP = {"proxy-authorization", "proxy-connection", "connection", "keep-alive", "te", "trailer", "upgrade", "expect",
       "proxy-authenticate"}
CHUNK = 65536


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
                 excluded: Callable[[str], bool] | None = None) -> None:
        self.allow, self.resolve, self.redactor = allow, resolve, redactor
        self.excluded = excluded or (lambda host: False)    # the owner's exclusion list: such a host is never shown
        self._tokens: dict[str, str] = {}                   # proc id -> token, while FERB tracks the process
        self._facts: dict[str, dict[str, Any]] = {}
        self._lock = threading.Lock()
        proxy = self

        class Handler(http.server.BaseHTTPRequestHandler):
            protocol_version = "HTTP/1.1"
            rbufsize = 0                                     # unbuffered: a tunnel's first bytes stay on the socket

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
                    try:
                        upstream = socket.socket(family, socket.SOCK_STREAM)
                        upstream.settimeout(30)
                        upstream.connect(sockaddr)
                        upstream.settimeout(None)
                        proxy._note(proc, host, port)
                        return proc, upstream
                    except OSError:
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
                self.send_response(200, "Connection established")
                self.end_headers()
                proxy._pump(self.connection, upstream, proc, host.lower(), port)
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
                opened = self._open(host, port)
                if opened is None:
                    return
                proc, upstream = opened
                host = host.lower()
                target = (url.path or "/") + (f"?{url.query}" if url.query else "")
                head = [f"{self.command} {target} HTTP/1.1",
                        f"Host: {url.netloc.rpartition('@')[2]}", "Connection: close"]
                head += [f"{k}: {v}" for k, v in self.headers.items() if k.lower() not in HOP | {"host"}]
                try:
                    proxy._send(upstream, ("\r\n".join(head) + "\r\n\r\n").encode(), proc, host, port, "bytes_out")
                    self._body(upstream, proc, host, port)
                    self._response(upstream, proc, host, port)
                except OSError:
                    pass
                finally:
                    upstream.close()
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

            def _response(self, upstream: socket.socket, proc: str, host: str, port: int) -> None:
                """Interim (1xx) heads pass as they are; the final head says the connection closes; then the rest."""
                buf = b""
                while True:
                    while b"\r\n\r\n" not in buf:
                        chunk = upstream.recv(CHUNK)
                        if not chunk:
                            self.wfile.write(buf)
                            return
                        proxy._count(proc, host, port, "bytes_in", len(chunk))
                        buf += chunk
                    head, _, buf = buf.partition(b"\r\n\r\n")
                    lines = head.split(b"\r\n")
                    if lines[0].split(b" ")[1:2] and lines[0].split(b" ")[1].startswith(b"1"):
                        self.wfile.write(head + b"\r\n\r\n")
                        continue
                    kept = [lines[0]] + [h for h in lines[1:]
                                         if h.split(b":", 1)[0].strip().lower().decode("latin-1") not in HOP]
                    self.wfile.write(b"\r\n".join(kept + [b"Connection: close"]) + b"\r\n\r\n" + buf)
                    break
                while chunk := upstream.recv(CHUNK):
                    proxy._count(proc, host, port, "bytes_in", len(chunk))
                    self.wfile.write(chunk)

            do_GET = do_POST = do_PUT = do_DELETE = do_HEAD = do_PATCH = do_OPTIONS = _forward

            def log_message(self, *a: object) -> None:      # never log requests
                pass

        self._srv = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self._srv.daemon_threads = True
        threading.Thread(target=self._srv.serve_forever, daemon=True).start()

    @property
    def port(self) -> int:
        return int(self._srv.server_port)

    def credentials(self, proc_id: str) -> str:
        """This process's token (made on first use, until `forget`)."""
        with self._lock:
            token = self._tokens.get(proc_id)
            if token is None:
                token = self._tokens[proc_id] = secrets.token_urlsafe(24)
                self._facts.setdefault(proc_id, {"destinations": {}, "refused": []})
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
        "excluded"."""
        with self._lock:
            f = self._facts.get(proc_id) or {"destinations": {}, "refused": []}
            dests: dict[tuple[str, int], dict[str, int]] = {}
            for (host, port), v in sorted(f["destinations"].items()):
                shown = dests.setdefault(("excluded" if self.excluded(host) else host, port),
                                         {"bytes_out": 0, "bytes_in": 0, "connections": 0})
                for k in shown:
                    shown[k] += v[k]
            refused = [r | {"host": "excluded"} if self.excluded(r["host"]) else dict(r) for r in f["refused"]]
        return {"destinations": [{"host": h, "port": p, **v} for (h, p), v in dests.items()], "refused": refused}

    def forget(self, proc_id: str) -> None:
        """The process is done with: its credential stops working and its record goes (after it was reported)."""
        with self._lock:
            token = self._tokens.pop(proc_id, None)
            self._facts.pop(proc_id, None)
        if token is not None and self.redactor is not None:
            self.redactor.forget_secret_value(token)

    def _note(self, proc: str, host: str, port: int, *, refused: str | None = None) -> None:
        with self._lock:
            f = self._facts.setdefault(proc, {"destinations": {}, "refused": []})
            if refused is not None:
                f["refused"].append({"host": host, "port": port, "why": refused})
                return
            d = f["destinations"].setdefault((host, port), {"bytes_out": 0, "bytes_in": 0, "connections": 0})
            d["connections"] += 1

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
