"""Own FERB's egress proxy for its shell (S2, D28): the only way off this Mac for a contained command. Must not read,
keep or judge content: per process it records each destination's host, port, byte counts and connections, never
bodies, headers, paths or query strings (rule 14).

- Each shell process gets its own credentials (its process id and a random token, kept in memory only), so a
  connection is attributed to the process (and task) that made it. A wrong, missing or forgotten credential gets 407
  and nothing is forwarded. The token is a secret value: the redactor scrubs it from the process's output, logs and
  spans. Another contained process can't read it (its environment isn't readable: measured, F5a Task 3 report).
- `allow(proc, host, port)` (the gateway: S4) is asked before any name is resolved, so a refused name never
  reaches a DNS server; a refusal is 403 and is recorded under `refused` with FERB's reason.
- The proxy resolves names itself (the sandbox has no DNS) and connects only to an address it checked. This Mac's
  own addresses are refused unless the owner configured executable-pinned local ports. Those explicit local
  destinations are checked before dial and against the actual connected peer before forwarding; their address and
  executable are recorded. Contained servers also remain reachable directly (sandbox.py, listeners.py).
- CONNECT tunnels any TCP port as opaque bytes (TLS stays end to end). Both directions start at once; the first
  client bytes are inspected inline. Non-TLS is forwarded with no delay. A TLS ClientHello's SNI (a hostname, a
  destination fact like the CONNECT host, never content) is recorded; once the tree has read outside content (and,
  failing closed, whenever FERB can't tell) the hello is held until reassembled (up to SNI_MAX, no timeout) and a
  connection is refused whose SNI names a host the owner hasn't named, FERB doesn't know and this tree's shell hadn't
  reached before that first outside read, or that has no readable SNI to a named host (domain fronting, S4). Residuals: an ECH outer name goes through refuse-then-ask like any other
  host; fronting with a matching SNI and a Host header inside TLS stays undetectable. A plain http:// request is
  forwarded byte for byte on its own connection, closed after the response, so each request is gated and recorded for
  its own host.
  Bytes are counted before they are passed on, so a record is complete by the time the client has the data. A
  connection that FERB's shell opened and a setsid child keeps after the shell exits goes on being counted: `forget`
  revokes the credential but the process's record is kept and reported only once its last connection closes.
- NO_PROXY keeps loopback direct by default; it is empty when owner-approved local ports need the proxy. Nothing is
  logged; byte counts are recorded, never capped.
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
import sys
import threading
import time
from typing import Any, Callable, Protocol
from urllib.parse import urlsplit

from ferb.act.shell.local import LocalAccess, LocalPeer, LocalRefused

Allow = Callable[[str, str, int], tuple[bool, str]]
Address = tuple[int, tuple[Any, ...]]                       # (family, sockaddr) as getaddrinfo gives it
Resolve = Callable[[str, int], list[Address]]
Report = Callable[[str, dict[str, Any]], None]
ENDED = "this command has ended; its proxy credential no longer opens connections"
OWN = "this Mac's own address: a command reaches this Mac's servers directly, and only ones started in FERB's shell"
# Not forwarded: they describe this hop, not the request (RFC 9110 7.6.1), or carry FERB's credential. Upgrade and
# Connection are kept for an upgrade request (a WebSocket handshake), handled apart.
HOP = {"proxy-authorization", "proxy-connection", "connection", "keep-alive", "te", "trailer", "upgrade", "expect",
       "proxy-authenticate"}
CHUNK = 65536
# S4: FERB's reasons for refusing a destination (the gateway sets them; the shell tool tells the worker what to do by
# matching these exact strings, so a sink refusal names the right host and a final No isn't offered as "ask again").
DEST_REFUSED_NEW = "a place you haven't named, after this task read outside content"
DEST_REFUSED_NO = "you said no to this place in this task"
DEST_NETWORK_APPROVAL = "this shell command has no network approval; request network=true for this exact command"
DEST_SNI_NONE = "a TLS connection whose server name FERB couldn't read"
SNI_MAX = 16 * 1024                                         # the most of a client's ClientHello FERB reassembles (S4)
UNAUTH_TIMEOUT = 10.0                                       # seconds from accept until a connection must authenticate
#                                                            (an absolute deadline: dripping bytes doesn't extend it)
MAX_UNAUTH = 1024                                           # connections not yet authenticated held at once; past it the
#                                                            oldest is closed, so a flood never locks out a new client
#                                                            (an authenticated tunnel leaves this set at once)
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


def local_host(host: str) -> bool:
    """Only explicit local destinations: no arbitrary DNS name gets resolved before the destination gate."""
    return host.rstrip(".").lower() == "localhost" or (_literal(host) and this_mac(host))


def local_addresses(host: str, port: int) -> list[Address]:
    # localhost never causes DNS or uses an owner-modifiable alias; other local names still go through the ordinary
    # before-DNS sink gate and own-address refusal. Explicit local IPs are dialled exactly as supplied.
    if host.rstrip(".").lower() == "localhost":
        return [(socket.AF_INET6, ("::1", port, 0, 0)), (socket.AF_INET, ("127.0.0.1", port))]
    family = socket.AF_INET6 if ":" in host else socket.AF_INET
    return [(family, (host, port))]


def _hang_up(conn: socket.socket) -> None:
    """Close a connection from another thread: shutdown wakes the handler blocked reading it."""
    try:
        conn.shutdown(socket.SHUT_RDWR)
    except OSError:
        pass


NOT_HELLO = "\x00not-a-client-hello"                       # a complete handshake record that isn't a ClientHello


def _sni_of(body: bytes) -> str:
    """The SNI host in a complete ClientHello body (after its 4-byte handshake header), lowercased; "" when none."""
    try:
        p = 2 + 32                                    # client_version, random
        p += 1 + body[p]                             # session id
        p += 2 + int.from_bytes(body[p:p + 2], "big")   # cipher suites
        p += 1 + body[p]                             # compression methods
        if p + 2 > len(body):
            return ""                                # no extensions
        end = p + 2 + int.from_bytes(body[p:p + 2], "big")
        p += 2
        while p + 4 <= end:
            etype = int.from_bytes(body[p:p + 2], "big")
            elen = int.from_bytes(body[p + 2:p + 4], "big")
            p += 4
            if etype == 0x00:                        # server_name: list len (2), type (1), name len (2), name
                nlen = int.from_bytes(body[p + 3:p + 5], "big")
                return body[p + 5:p + 5 + nlen].decode("ascii").lower()
            p += elen
    except (IndexError, ValueError, UnicodeDecodeError):
        return ""
    return ""


class HelloReader:
    """Reassembles a client's ClientHello across TLS handshake records, incrementally: each `feed` looks only at the
    new bytes, so a hello dribbled in many tiny records costs linear time. `feed` returns None while more bytes are
    needed, NOT_HELLO as soon as a complete record shows it isn't a ClientHello (or isn't a handshake record), "" for a
    complete ClientHello without an SNI, else the lowercased SNI host. Hostnames only, never application data."""

    def __init__(self) -> None:
        self.buf = bytearray()
        self.pos = 0                                  # the first byte of the next record not yet taken
        self.hs = bytearray()                         # handshake-message bytes, concatenated across records
        self.read = 0                                 # bytes fed so far

    def feed(self, data: bytes) -> str | None:
        self.buf += data
        self.read += len(data)
        while self.pos + 5 <= len(self.buf):
            if self.buf[self.pos] != 0x16:            # not a handshake record
                return NOT_HELLO
            rec_len = int.from_bytes(self.buf[self.pos + 3:self.pos + 5], "big")
            if self.pos + 5 + rec_len > len(self.buf):
                return None                           # this record isn't fully here yet
            self.hs += self.buf[self.pos + 5:self.pos + 5 + rec_len]
            self.pos += 5 + rec_len
            if self.hs and self.hs[0] != 0x01:        # a complete record whose message isn't a ClientHello
                return NOT_HELLO
            if len(self.hs) >= 4 and len(self.hs) >= 4 + int.from_bytes(self.hs[1:4], "big"):
                return _sni_of(bytes(self.hs[4:4 + int.from_bytes(self.hs[1:4], "big")]))
        return None


def parse_sni(buf: bytes) -> str | None:
    """The SNI host from a client's first bytes (HelloReader in one go): None while more bytes are needed, NOT_HELLO,
    "" for a complete ClientHello without an SNI, else the lowercased host."""
    return HelloReader().feed(buf)


def _literal(host: str) -> bool:
    try:
        ipaddress.ip_address(host.split("%", 1)[0])
    except ValueError:
        return False
    return True


class EgressProxy:
    def __init__(self, allow: Allow, *, resolve: Resolve = addresses, redactor: Secrets | None = None,
                 excluded: Callable[[str], bool] | None = None, on_report: Report | None = None,
                 inspect_sni: Callable[[str, str, int], bool] | None = None,
                 on_reached: Callable[[str, str, int], None] | None = None,
                 local_access: LocalAccess | None = None, allow_local: Allow | None = None) -> None:
        self.allow, self.resolve, self.redactor = allow, resolve, redactor
        # S4: whether to hold a CONNECT's TLS ClientHello to enforce the SNI (after outside content); default off
        self.inspect_sni = inspect_sni or (lambda proc, host, port: False)
        self.excluded = excluded or (lambda host: False)    # the owner's exclusion list: such a host is never shown
        self.on_report = on_report                          # a process is fully done: its final record (host/bytes)
        self.on_reached = on_reached                        # successful connection/SNI, before bytes are forwarded
        self.local_access = local_access
        self.allow_local = allow_local or allow              # daemon: standing local permission, final No still wins
        self._tokens: dict[str, str] = {}                   # proc id -> token, while FERB tracks the process
        self._facts: dict[str, dict[str, Any]] = {}
        self._active: dict[str, int] = {}                   # open connections per process (a tunnel may outlive it)
        self._ending: set[str] = set()                      # forgotten, waiting for its last connection to close
        self._pending: dict[socket.socket, float] = {}      # not yet authenticated: socket -> its absolute deadline
        self._lock = threading.Lock()
        self._wake = threading.Condition(self._lock)        # the reaper sleeps until the earliest deadline
        self._shut = False                                  # close() was called: nothing restarts serving
        proxy = self

        class Handler(http.server.BaseHTTPRequestHandler):
            protocol_version = "HTTP/1.1"
            rbufsize = 0                                     # unbuffered: a tunnel's first bytes stay on the socket

            def setup(self) -> None:
                super().setup()
                proxy._admit(self.connection)                # pending until it authenticates, with a deadline

            def finish(self) -> None:
                proxy._settled(self.connection)
                super().finish()

            def _authenticated(self) -> None:
                proxy._settled(self.connection)              # leaves the pending set: no deadline, no idle timeout

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

            def _open(self, host: str, port: int) -> tuple[str, socket.socket, bool] | None:
                """The checked connection for this request, or None after answering 407/403/502."""
                proc = self._who()
                if proc is None:
                    self._reply(407, "FERB: proxy credentials required", {"Proxy-Authenticate": 'Basic realm="FERB"'})
                    return None
                self._authenticated()
                host = host.lower()
                local = proxy.local_access if local_host(host) else None
                try:
                    if local is not None:
                        where = local_addresses(host, port)
                    else:
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
                local_error = None
                for family, sockaddr in where:
                    if not proxy._live(proc):                # forgotten while allow/DNS ran: the task has ended
                        self._reply(403, f"FERB: {ENDED}")
                        return None
                    upstream = None
                    peer = None
                    try:
                        if local is not None:
                            local.before_connect((str(sockaddr[0]), port))
                        upstream = socket.socket(family, socket.SOCK_STREAM)
                        upstream.settimeout(30)
                        upstream.connect(sockaddr)
                        upstream.settimeout(None)
                        if local is not None:
                            target, source = upstream.getpeername(), upstream.getsockname()
                            peer = local.connected((str(target[0]), int(target[1])),
                                                   (str(source[0]), int(source[1])))
                            # Only after both executable checks: a configured local port is standing permission;
                            # task ownership and an explicit tree No still apply. Other contained servers follow
                            # the ordinary gate here; their existing direct socket route is unchanged.
                            gate = proxy.allow_local if port in local.pins else proxy.allow
                            ok, why = gate(proc, host, port)
                            if not ok:
                                raise LocalRefused(why or "FERB refused this destination")
                    except LocalRefused as e:
                        local_error = str(e)
                        if upstream is not None:
                            upstream.close()
                        continue
                    except OSError:
                        if upstream is not None:
                            upstream.close()
                        continue
                    if not proxy._opened(proc, host, port, local_peer=peer):
                        upstream.close()
                        self._reply(403, f"FERB: {ENDED}")
                        return None
                    return proc, upstream, local is not None
                if local_error is not None:
                    proxy._note(proc, host, port, refused=local_error)
                    self._reply(403, f"FERB: {local_error}")
                    return None
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
                proc, upstream, local = opened
                host = host.lower()
                try:
                    self.send_response(200, "Connection established")
                    self.end_headers()
                    # S4 fronting: both directions start at once (server-first bytes flow immediately); the first client
                    # bytes are inspected inline (non-TLS forwarded with no delay; a TLS SNI checked after outside content)
                    proxy._pump(self.connection, upstream, proc, host, port,
                                sni_check=None if local else lambda: proxy.inspect_sni(proc, host, port))
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
                proc, upstream, _local = opened
                host = host.lower()
                target = (url.path or "/") + (f"?{url.query}" if url.query else "")
                drop = HOP - {"upgrade"} if upgrading else HOP
                # An upgrade asks for the switch but closes if refused, so a non-101 reply frees both sockets
                head = [f"{self.command} {target} HTTP/1.1", f"Host: {url.netloc.rpartition('@')[2]}",
                        "Connection: Upgrade, close" if upgrading else "Connection: close"]
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
                # Kept, never logged with request bytes (rule 14): only the type, which `ferb doctor` shows
                exc = sys.exc_info()[1]
                proxy._last_error = type(exc).__name__ if exc else "?"

            def get_request(self) -> Any:
                try:
                    return self.socket.accept()
                except OSError as e:
                    if e.errno == errno.EMFILE:              # out of descriptors: don't spin a hot accept loop
                        time.sleep(0.05)
                    raise

        self._last_error: str | None = None
        self._srv = Server(("127.0.0.1", 0), Handler)
        self._thread: threading.Thread | None = None
        self._restarts = 0
        threading.Thread(target=self._reap, daemon=True).start()
        self.ensure_serving()

    @property
    def port(self) -> int:
        return int(self._srv.server_port)

    def ensure_serving(self) -> bool:
        """Start the serving thread, or restart it if it died; True when it restarted a dead one. Never after close().
        While it is down the shell has no network at all, so it fails closed."""
        with self._lock:
            if self._shut or (self._thread is not None and self._thread.is_alive()):
                return False
            restarted = self._thread is not None
            self._restarts += restarted
            self._thread = threading.Thread(target=self._srv.serve_forever, daemon=True)
            self._thread.start()
        return restarted

    def probe(self) -> dict[str, Any]:
        """`ferb doctor`'s liveness check: an unauthenticated request must get 407. A dead serving thread is restarted
        first (and said so); the last handler error's type is included."""
        restarted = self.ensure_serving()
        alive = False
        if not self._shut:
            try:
                c = http.client.HTTPConnection("127.0.0.1", self.port, timeout=5)
                c.request("GET", "http://127.0.0.1/")
                alive = c.getresponse().status == 407
                c.close()
            except OSError:
                alive = False
        return {"alive": alive, "restarted": restarted, "restarts": self._restarts, "last_error": self._last_error}

    def _admit(self, conn: socket.socket) -> None:
        """A new connection: pending until it authenticates, by an absolute deadline. Past MAX_UNAUTH the oldest
        pending connection is closed, so a flood (even one dripping bytes) never locks out a new client."""
        with self._wake:
            self._pending[conn] = time.monotonic() + UNAUTH_TIMEOUT
            while len(self._pending) > MAX_UNAUTH:
                oldest = min(self._pending, key=self._pending.__getitem__)
                self._pending.pop(oldest)
                _hang_up(oldest)
            self._wake.notify()

    def _settled(self, conn: socket.socket) -> None:
        """Authenticated, or done: out of the pending set (an authenticated tunnel has no deadline or idle timeout)."""
        with self._wake:
            self._pending.pop(conn, None)

    def _reap(self) -> None:
        """Close each pending connection at its deadline; sleeps until the earliest one (no idle wakeups)."""
        with self._wake:
            while not self._shut:
                now = time.monotonic()
                for conn in [c for c, due in self._pending.items() if due <= now]:
                    self._pending.pop(conn)
                    _hang_up(conn)
                self._wake.wait(min(self._pending.values()) - now if self._pending else None)

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
        local = "" if self.local_access is not None else "localhost,127.0.0.1,::1"
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
        local: dict[tuple[str, int], list[dict[str, Any]]] = {}
        for (host, port, address, executable), count in sorted(f.get("local", {}).items()):
            shown = "excluded" if self.excluded(host) else host
            local.setdefault((shown, port), []).append({"address": "excluded" if shown == "excluded" else address,
                                                        "executable": executable, "connections": count})
        return {"destinations": [{"host": h, "port": p, **v,
                                  **({"local_connections": local[h, p]} if (h, p) in local else {})}
                                 for (h, p), v in dests.items()],
                "refused": [{"host": h, "port": p, "why": w, "count": c} for (h, p, w), c in refused.items()]}

    def _live(self, proc: str) -> bool:
        with self._lock:
            return proc in self._tokens

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

    def _opened(self, proc: str, host: str, port: int, *, local_peer: LocalPeer | None = None) -> bool:
        """Record a new connection; False when the credential was revoked meanwhile (the connection is refused: a
        connection that passed the credential check before `forget` must not re-create a record nobody reports)."""
        with self._lock:
            if proc not in self._tokens:
                return False
            f = self._facts.setdefault(proc, {"destinations": {}, "refused": {}})
            d = f["destinations"].setdefault((host, port), {"bytes_out": 0, "bytes_in": 0, "connections": 0})
            d["connections"] += 1
            if local_peer is not None:
                records = f.setdefault("local", {})
                key = (host, port, local_peer.address, local_peer.executable)
                records[key] = records.get(key, 0) + 1
            self._active[proc] = self._active.get(proc, 0) + 1
        if self.on_reached is not None:
            self.on_reached(proc, host, port)               # outside the proxy lock: the gateway runs on its loop
        return True

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

    def _record_sni(self, proc: str, sni: str, port: int) -> None:
        """Record a differing SNI as a destination fact (a hostname like the CONNECT host), no bytes of its own."""
        with self._lock:
            f = self._facts.get(proc)
            if f is None:
                return
            f["destinations"].setdefault((sni, port), {"bytes_out": 0, "bytes_in": 0, "connections": 0})
        if self.on_reached is not None:
            self.on_reached(proc, sni, port)

    def _sni_gate(self, chunk: bytes, client: socket.socket, proc: str, host: str, port: int,
                  sni_check: Callable[[], bool]) -> bytes | None:
        """Inspect the first client bytes of a CONNECT (S4 fronting). Not TLS (SMTP/IMAP/FTP/MySQL/SSH): forwarded at
        once. TLS before the tree read outside content: forwarded at once, the SNI recorded best-effort. TLS after:
        the ClientHello is held (reassembled incrementally across records, up to SNI_MAX, no timeout) and the SNI
        checked — forwarded if it matches the CONNECT host or is known/named, refused if it names an unknown host, if a
        complete record isn't a ClientHello, or if no SNI can be read. A complete hello without an SNI to an IP-literal
        CONNECT host is forwarded: RFC 6066 forbids an IP literal in the SNI. Returns the bytes to forward, or None."""
        if chunk[:1] != b"\x16":                         # not a TLS handshake: forward immediately
            return chunk
        reader = HelloReader()
        if not sni_check():                              # before any outside content: never hold; best-effort record
            sni = reader.feed(chunk)
            if sni and sni != NOT_HELLO and sni.rstrip(".") != host:
                self._record_sni(proc, sni, port)
            return chunk
        buf = bytearray(chunk)
        sni = reader.feed(chunk)
        while sni is None and len(buf) < SNI_MAX:        # a dribbled / split-record hello just waits; no timeout
            try:
                more = client.recv(CHUNK)
            except OSError:
                break
            if not more:
                break
            buf += more
            sni = reader.feed(more)
        if sni == "" and _literal(host):                 # no SNI to an IP literal is the protocol's rule, not fronting
            return bytes(buf)
        if not sni or sni == NOT_HELLO:                  # no server name, or not a ClientHello: fail closed
            self._note(proc, host, port, refused=DEST_SNI_NONE)
            return None
        if sni.rstrip(".") != host:
            ok, why = self.allow(proc, sni, port)
            if not ok:
                self._note(proc, sni, port, refused=why or "FERB refused this destination")
                return None
            self._record_sni(proc, sni, port)
        return bytes(buf)

    def _pump(self, client: socket.socket, upstream: socket.socket, proc: str, host: str, port: int,
              sni_check: Callable[[], bool] | None = None) -> None:
        def u2c() -> None:                               # server -> client, started at once (server-first bytes flow)
            try:
                while chunk := upstream.recv(CHUNK):
                    self._send(client, chunk, proc, host, port, "bytes_in")
            except OSError:
                pass
            finally:
                try:
                    client.shutdown(socket.SHUT_WR)
                except OSError:
                    pass
        t = threading.Thread(target=u2c, daemon=True)
        t.start()
        first = True
        try:
            while True:
                try:
                    chunk = client.recv(CHUNK)
                except OSError:
                    break
                if not chunk:
                    break
                if first and sni_check is not None:
                    first = False
                    gated = self._sni_gate(chunk, client, proc, host, port, sni_check)
                    if gated is None:                    # a fronting refusal: nothing forwarded, both sides torn down
                        _hang_up(upstream)
                        break
                    chunk = gated
                else:
                    first = False
                try:
                    self._send(upstream, chunk, proc, host, port, "bytes_out")
                except OSError:
                    break
        finally:
            try:
                upstream.shutdown(socket.SHUT_WR)
            except OSError:
                pass
        t.join()
        upstream.close()

    def close(self) -> None:
        """Stop serving, for good (nothing restarts it). Every record still held (a forgotten process whose tunnel is
        still open, or one never forgotten) is reported first with its bytes so far, marked `still_open` when a
        connection is still open. The serving loop notices within its 0.5 s poll; nothing waits for that."""
        with self._wake:
            if self._shut:
                return
            self._shut = True
            self._wake.notify()
            left = [(proc, self._active.get(proc, 0) > 0) for proc in self._facts]
            reports = [(proc, self._take(proc) | {"still_open": open_}) for proc, open_ in left]
            tokens = list(self._tokens.values())
            self._tokens.clear()
        for token in tokens:
            if self.redactor is not None:
                self.redactor.forget_secret_value(token)
        if self.on_report is not None:
            for proc, record in reports:
                self.on_report(proc, record)

        def stop() -> None:
            self._srv.shutdown()
            self._srv.server_close()
        threading.Thread(target=stop, daemon=True).start()
