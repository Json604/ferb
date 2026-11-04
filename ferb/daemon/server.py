"""Own authenticated local RPC transport. Must not plan tasks, truncate frames or choose recovery.

The first request on a connection must be `auth.hello {role, token?}`; afterwards only the role's methods are
allowed. A handler returns a result, or (streaming methods) an async iterator of notifications. While an MCP shim's call
runs, its connection is watched: a shim that leaves (its runtime gave up the call) is a fact the handler can read
(`caller_left`), and its result goes nowhere. A runtime that drops a call without telling its shim (Codex) has its
adapter name the call (`abandon`), which ends it the same way and closes the shim's connection."""

import asyncio
import contextlib
import contextvars
import json
import os
import socket
import struct
import time
from dataclasses import dataclass
from pathlib import Path
from typing import Any, AsyncIterator, Awaitable, Callable, Literal

from pydantic import BaseModel, ValidationError

from ferb.core.errors import AlreadyRunning, AuthError, FerbError, Unsupported
from ferb.daemon.auth import APP_ONLY, RunGrant, RunTokens, Role, allowed, check_cli_peer, check_native_host

MAX_MESSAGE = 1024 * 1024

PARSE_ERROR, INVALID_REQUEST, METHOD_NOT_FOUND, INTERNAL = -32700, -32600, -32601, -32603
UNAUTHORIZED, TOO_LARGE, UNSUPPORTED, FAILED = -32001, -32002, -32003, -32004
APP_ONLY_REFUSED = -32005            # a FERB.app-only method called by another program (auth.APP_ONLY; step 8e: its own
                                     # code, so the app tells it from a daemon too old to know the method)

SOL_LOCAL, LOCAL_PEERCRED = 0, 1   # <sys/un.h>

# How long the notch waits for a reply (its socket read timeout, DaemonRPC.swift), and the CLI by default
# (cli/client.py). A later reply reaches neither, unless the caller chose a longer wait (`ferb chat` waits 30 minutes).
CLIENT_WAIT_S = 10.0
# Calls the notch waits longer for (seconds; its `wait:` calls): a failed task's report renders and redacts the task's
# whole record first (L35; NotchModel.reportWait), and Send posts it to the report relay (reports.SEND_TIMEOUT_S); a
# permission request and an Automation request wait for the owner's answer to macOS's prompt, and a runtime check
# makes one model call (L34 C, the setup window), as the intro's suggested task does (kb area app)
LONG_WAITS = {"reports.build": 120.0, "reports.send": 600.0, "setup.browser_pair": 90.0, "setup.browsers_pair_all": 90.0, "reports.diagnostic": 60.0, "setup.permission_request": 120.0, "automation.request": 120.0,
              "runtimes.check": 60.0, "intro.suggest": 60.0}
# Calls that end when their caller leaves (kb area app): an abandoned preview request stops its capture, and an
# abandoned intro suggestion its model call (a stream, intro.preview, already ends when its caller leaves)
CANCEL_WHEN_LEFT = frozenset({"tasks.preview", "intro.suggest"})
LEFT_POLL_S = 0.1
# The journal kinds the daemon records `on_internal` and `on_slow` under; `ferb stats` lists them
INTERNAL_ERROR, SLOW_CALL = "daemon.internal_error", "daemon.slow_call"
# Answered while the daemon is still starting (DMN-510): FERB.app's wait for its service and the CLI's wait for the
# daemon ask only this. Every other call waits until the start is done (`ready`).
ANSWERED_WHILE_STARTING = frozenset({"ping"})


class Request(BaseModel):
    jsonrpc: Literal["2.0"]
    id: int | str | None = None
    method: str
    params: dict[str, Any] = {}


class Hello(BaseModel):
    role: Role
    token: str | None = None
    origin: str | None = None             # native_host: the caller extension's origin


@dataclass
class Session:
    role: Role
    grant: RunGrant | None = None
    pid: int | None = None                    # a CLI's process, as its socket reports it (L12)
    token: bytes | None = None                # and its audit token (who it is, for APP_ONLY methods)
    app: bool | None = None                   # whether that process is FERB.app (checked once, on first need)


Unary = Callable[[Session, dict[str, Any]], Awaitable[dict[str, Any]]]
_CALLER_LEFT: contextvars.ContextVar[asyncio.Event | None] = contextvars.ContextVar("ferb_rpc_caller_left",
                                                                                   default=None)


def caller_left() -> bool:
    """Whether the MCP shim call this code runs for has lost its caller (the connection closed mid-call)."""
    left = _CALLER_LEFT.get()
    return left is not None and left.is_set()
Channel = Callable[[asyncio.StreamReader, asyncio.StreamWriter], Awaitable[None]]
Stream = Callable[[Session, dict[str, Any]], AsyncIterator[dict[str, Any]]]


LOCAL_PEERPID = 0x002


def peer_pid(sock: socket.socket) -> int:
    return int(sock.getsockopt(SOL_LOCAL, LOCAL_PEERPID))


def _peer(writer: asyncio.StreamWriter) -> int | None:
    """The connected process's pid, or None when the socket can't say."""
    try:
        return peer_pid(writer.get_extra_info("socket"))
    except (OSError, AttributeError, TypeError):
        return None


def peer_uid(sock: socket.socket) -> int:
    raw = sock.getsockopt(SOL_LOCAL, LOCAL_PEERCRED, 76)   # struct xucred: u_int version, uid_t uid, ...
    return int(struct.unpack_from("Ii", raw)[1])


def ensure_socket_free(path: Path) -> None:
    """A leftover socket file from a crash is removed; a live daemon behind it means we must not start."""
    if not path.exists():
        return
    probe = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    try:
        probe.connect(str(path))
    except (ConnectionRefusedError, FileNotFoundError):
        path.unlink(missing_ok=True)
        return
    finally:
        probe.close()
    raise AlreadyRunning(f"another daemon is answering on {path}")


class Server:
    def __init__(self, path: Path, tokens: RunTokens, owner_uid: int | None = None) -> None:
        self.path = path
        self.tokens = tokens
        self.owner_uid = os.getuid() if owner_uid is None else owner_uid
        self.unary: dict[str, Unary] = {}
        self.streams: dict[str, Stream] = {}
        self.native: Channel | None = None               # extension message channel
        self.extension_ids: Callable[[], frozenset[str]] = lambda: frozenset()
        self._server: asyncio.base_events.Server | None = None
        self._writers: set[asyncio.StreamWriter] = set()
        # (task id, the runtime's call id) -> each running MCP shim call so named: (caller-left flag, its connection),
        # oldest first (a runtime may repeat a call id; each call can still be released)
        self._shim_calls: dict[tuple[str, str], list[tuple[asyncio.Event, asyncio.StreamWriter]]] = {}
        # Told what a reply can't carry, for the daemon to record (L36: a notch answer that failed left no trace): a
        # handler's exception that isn't one of FERB's own errors (its method and the exception; the reply names only
        # its type), and an owner-side call that took `CLIENT_WAIT_S` or longer (its method and seconds)
        self.on_internal: Callable[[str, BaseException], None] = lambda method, error: None
        self.on_slow: Callable[[str, float], None] = lambda method, seconds: None
        # HAND-406: a native host's auth.hello refused (why, the origin it gave): the extension never gets as far as a
        # hello, and the host only exits, so the daemon records it
        self.on_native_refused: Callable[[str, str], None] = lambda why, origin: None
        # whether a cli connection's process is FERB.app (its audit token -> bool; app_peer.AppPeers in the daemon):
        # the gate of auth.APP_ONLY methods. Fails closed until the daemon sets it.
        self.is_app: Callable[[bytes | None], bool] = Server.is_app_default
        # DMN-510: the daemon serves before its slow start-up work (Codex's first run on a new Mac) and sets this when
        # the start is done; until then only ANSWERED_WHILE_STARTING is answered and every other call waits. Set by
        # default: a server nobody marks as starting answers everything.
        self.ready = asyncio.Event()
        self.ready.set()
        self._closing = False

    @staticmethod
    def is_app_default(token: bytes | None) -> bool:
        """No check in place: nobody is FERB.app (fails closed)."""
        return False

    def _native_refused(self, req: Request, why: str) -> None:
        if req.params.get("role") == Role.NATIVE_HOST.value:
            self._tell(self.on_native_refused, why, str(req.params.get("origin") or "")[:120])

    @staticmethod
    def _tell(hook: Callable[..., None], *args: Any) -> None:
        """A recording hook never changes a reply or ends a connection."""
        with contextlib.suppress(Exception):
            hook(*args)

    def abandon(self, task_id: str, call_id: str) -> bool:
        """The runtime of `task_id` gave up its call `call_id` without telling the shim (Codex sends no cancellation):
        the call sees `caller_left()`, as if the shim had left, and the shim's connection is closed, so its waiting
        thread ends. Only that task's own call, the oldest still running under that id (one per report); False when
        no such call is running."""
        calls = self._shim_calls.get((task_id, call_id))
        if not calls:
            return False
        left, writer = calls.pop(0)
        if not calls:
            del self._shim_calls[(task_id, call_id)]
        left.set()
        writer.close()
        return True

    async def start(self) -> None:
        self.path.parent.mkdir(parents=True, exist_ok=True)
        os.chmod(self.path.parent, 0o700)
        ensure_socket_free(self.path)
        # The socket file is made 0600 between bind and listen, before anyone can connect. Never through the umask:
        # it is process-wide, so every other thread (and this loop, while the server start yields) would create its
        # files and folders under it too (a 0600 folder can't be entered).
        sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        bound = False
        try:
            sock.bind(str(self.path))
            bound = True
            os.chmod(self.path, 0o600)
            self._server = await asyncio.start_unix_server(self._connection, sock=sock, limit=MAX_MESSAGE)
        except BaseException:
            sock.close()
            if bound:                               # only the file this start made
                self.path.unlink(missing_ok=True)
            raise

    async def close(self) -> None:
        """Stop accepting, then close open connections (e.g. a running `ferb logs`); since Python 3.12
        wait_closed() waits for every connection, so without this shutdown would hang."""
        self._closing = True
        self.ready.set()                          # calls waiting for a start that won't finish end (`_dispatch`)
        if self._server:
            self._server.close()
            for w in list(self._writers):
                w.close()
            await self._server.wait_closed()
            self.path.unlink(missing_ok=True)     # only its own socket: never another daemon's (a failed start)

    # ---------- per connection ----------

    async def _connection(self, reader: asyncio.StreamReader, writer: asyncio.StreamWriter) -> None:
        session: Session | None = None
        self._writers.add(writer)
        try:
            uid = peer_uid(writer.get_extra_info("socket"))
            while True:
                try:
                    line = await reader.readline()
                except (asyncio.LimitOverrunError, ValueError):
                    await self._send(writer, _error(None, TOO_LARGE, f"message exceeds {MAX_MESSAGE} bytes"))
                    return
                if not line:
                    return
                try:
                    req = Request.model_validate(json.loads(line))
                except json.JSONDecodeError:
                    await self._send(writer, _error(None, PARSE_ERROR, "invalid JSON"))
                    continue
                except ValidationError as e:
                    await self._send(writer, _error(None, INVALID_REQUEST, str(e.errors()[0]["msg"])))
                    continue
                if session is None:
                    session = await self._hello(req, uid, writer)
                    if session is None:
                        return
                    if session.role is Role.NATIVE_HOST and self.native is not None:
                        await self.native(reader, writer)   # the rest of the connection is extension messages
                        return
                    continue
                if await self._dispatch(session, req, reader, writer) is False:
                    return
        except (ConnectionResetError, BrokenPipeError):
            return
        finally:
            self._writers.discard(writer)
            writer.close()

    async def _hello(self, req: Request, uid: int, writer: asyncio.StreamWriter) -> Session | None:
        if req.method != "auth.hello":
            await self._send(writer, _error(req.id, UNAUTHORIZED, "authenticate first (auth.hello)"))
            return None
        try:
            hello = Hello.model_validate(req.params)
            if hello.role is Role.CLI:
                check_cli_peer(uid, self.owner_uid)
                from ferb.daemon.app_peer import peer_token
                session = Session(Role.CLI, pid=_peer(writer), token=peer_token(writer.get_extra_info("socket")))
            elif hello.role is Role.MCP_SHIM:
                session = Session(Role.MCP_SHIM, self.tokens.validate(hello.token or ""))
            else:
                if self.native is None:
                    raise Unsupported("FERB's browser bridge isn't running")
                check_native_host(uid, self.owner_uid, hello.origin or "", self.extension_ids())
                session = Session(Role.NATIVE_HOST)
        except ValidationError as e:
            await self._send(writer, _error(req.id, INVALID_REQUEST, str(e.errors()[0]["msg"])))
            return None
        except AuthError as e:
            self._native_refused(req, str(e))
            await self._send(writer, _error(req.id, UNAUTHORIZED, str(e)))
            return None
        except Unsupported as e:
            self._native_refused(req, str(e))
            await self._send(writer, _error(req.id, UNSUPPORTED, str(e)))
            return None
        await self._send(writer, _result(req.id, {"role": session.role.value}))
        return session

    async def _dispatch(self, session: Session, req: Request, reader: asyncio.StreamReader,
                        writer: asyncio.StreamWriter) -> bool:
        """Answer one request; False when the caller has left (the connection ends)."""
        if session.role is Role.MCP_SHIM:
            try:                                   # re-check on every call: revoked/expired tokens stop at once
                if session.grant is None:
                    raise AuthError("mcp_shim session without a run token")
                session.grant = self.tokens.recheck(session.grant)
            except AuthError as e:
                await self._send(writer, _error(req.id, UNAUTHORIZED, str(e)))
                return True
        if not allowed(session.role, req.method):
            await self._send(writer, _error(req.id, UNAUTHORIZED, f"{req.method} not allowed for {session.role.value}"))
            return True
        if req.method in APP_ONLY and not self._app(session):
            await self._send(writer, _error(req.id, APP_ONLY_REFUSED, f"{req.method} is for FERB.app only"))
            return True
        if not self.ready.is_set() and req.method not in ANSWERED_WHILE_STARTING:
            await self.ready.wait()
            if self._closing:                     # the start failed or the daemon is stopping: nothing to answer with
                return False
        if req.method in self.unary:
            started = time.monotonic()
            try:
                if session.role is Role.MCP_SHIM:
                    result = await self._watched(session, req, reader, writer)
                    if result is None:
                        return False
                elif req.method in CANCEL_WHEN_LEFT:
                    result = await self._while_caller_stays(session, req, reader)
                    if result is None:
                        return False
                else:
                    result = await self.unary[req.method](session, req.params)
                reply = _line(_result(req.id, result))
            except FerbError as e:
                await self._send(writer, _error(req.id, FAILED, str(e)))
            except Exception as e:                  # failures are values, never a crashed connection: a handler's
                self._tell(self.on_internal, req.method, e)      # own pipe error too (review Q-I1)
                await self._send(writer, _error(req.id, INTERNAL, f"{type(e).__name__}"))
            else:                                   # a reply's write failing is the caller leaving (`_connection`)
                await self._write(writer, reply)
            finally:
                # an MCP shim waits for its call however long it runs; the notch and the CLI stop waiting
                if session.role is not Role.MCP_SHIM and (seconds := time.monotonic() - started) >= LONG_WAITS.get(
                        req.method, CLIENT_WAIT_S):
                    self._tell(self.on_slow, req.method, seconds)
            return True
        if req.method in self.streams:
            await self._stream(session, req, reader, writer)
            return True
        await self._send(writer, _error(req.id, METHOD_NOT_FOUND, f"no method {req.method}"))
        return True

    def _app(self, session: Session) -> bool:
        """Whether this connection's process is FERB.app (auth.APP_ONLY), checked once per connection."""
        if session.app is None:
            try:
                session.app = session.role is Role.CLI and bool(self.is_app(session.token))
            except Exception:
                session.app = False
        return session.app

    async def _while_caller_stays(self, session: Session, req: Request,
                                  reader: asyncio.StreamReader) -> dict[str, Any] | None:
        """A call that ends when its caller leaves (CANCEL_WHEN_LEFT): the connection's end of input, seen without
        reading anything a caller sends after the call, cancels it. None: the caller left."""
        call = asyncio.ensure_future(self.unary[req.method](session, req.params))
        try:
            while True:
                done, _ = await asyncio.wait({call}, timeout=LEFT_POLL_S)
                if done:
                    return call.result()
                if reader.at_eof():
                    call.cancel()
                    with contextlib.suppress(asyncio.CancelledError, Exception):
                        await call
                    return None
        finally:
            if not call.done():
                call.cancel()

    async def _watched(self, session: Session, req: Request, reader: asyncio.StreamReader,
                       writer: asyncio.StreamWriter) -> dict[str, Any] | None:
        """An MCP shim's call, watching its connection (a shim sends one call and then only closes it). When the shim
        leaves first, or its runtime's adapter abandons the call, the call is not cancelled (an action already under
        way finishes and is recorded); it can see `caller_left()`, and its result goes nowhere: None."""
        left = asyncio.Event()
        call_id = req.params.get("call_id")
        key = (session.grant.task_id, call_id) if session.grant is not None and isinstance(call_id, str) else None
        if key is not None:
            self._shim_calls.setdefault(key, []).append((left, writer))
        context = contextvars.copy_context()
        context.run(_CALLER_LEFT.set, left)
        call = asyncio.create_task(self.unary[req.method](session, req.params), context=context)
        gone = asyncio.create_task(reader.read(1))
        try:
            done, _ = await asyncio.wait({call, gone}, return_when=asyncio.FIRST_COMPLETED)
        finally:
            if key is not None:
                calls = [c for c in self._shim_calls.get(key, []) if c[0] is not left]
                if calls:
                    self._shim_calls[key] = calls
                else:
                    self._shim_calls.pop(key, None)
        if call in done and not left.is_set():
            gone.cancel()
            with contextlib.suppress(asyncio.CancelledError):
                await gone                    # the reader must be free before the connection reads again
            return call.result()
        left.set()
        gone.cancel()
        with contextlib.suppress(Exception, asyncio.CancelledError):
            await gone
        with contextlib.suppress(Exception):
            await call
        return None

    async def _stream(self, session: Session, req: Request, reader: asyncio.StreamReader,
                      writer: asyncio.StreamWriter) -> None:
        """Send notifications until the stream ends or the client disconnects (client EOF ends it)."""
        async def pump() -> None:
            async with contextlib.aclosing(self.streams[req.method](session, req.params)) as stream:
                async for item in stream:
                    try:
                        await self._send(writer, {"jsonrpc": "2.0", "method": f"{req.method}.event", "params": item})
                    except (ConnectionResetError, BrokenPipeError):
                        raise _ClientLeft() from None     # writing to this client failed: it left (review Q-I1)

        task = asyncio.create_task(pump())
        eof = asyncio.create_task(reader.read(1))
        done, _ = await asyncio.wait({task, eof}, return_when=asyncio.FIRST_COMPLETED)
        if task in done:
            eof.cancel()
            with contextlib.suppress(asyncio.CancelledError):
                await eof                     # the reader must be free before the connection reads again
            exc = task.exception()
            if exc is None:
                await self._send(writer, _result(req.id, {"ended": True}))
            elif not isinstance(exc, _ClientLeft):
                if not isinstance(exc, FerbError):
                    self._tell(self.on_internal, req.method, exc)
                await self._send(writer, _error(req.id, FAILED, f"{type(exc).__name__}: {exc}"))
        else:
            task.cancel()
            with contextlib.suppress(asyncio.CancelledError):
                await task

    async def _send(self, writer: asyncio.StreamWriter, msg: dict[str, Any]) -> None:
        await self._write(writer, _line(msg))

    @staticmethod
    async def _write(writer: asyncio.StreamWriter, line: bytes) -> None:
        writer.write(line)
        await writer.drain()


class _ClientLeft(Exception):
    """A stream's client closed its connection while an event was being written to it."""


def _line(msg: dict[str, Any]) -> bytes:
    return json.dumps(msg).encode() + b"\n"


def _result(id: Any, result: dict[str, Any]) -> dict[str, Any]:
    return {"jsonrpc": "2.0", "id": id, "result": result}


def _error(id: Any, code: int, message: str) -> dict[str, Any]:
    return {"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}}
