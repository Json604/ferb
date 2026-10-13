"""Own authenticated local RPC transport. Must not plan tasks, truncate frames or choose recovery.

The first request on a connection must be `auth.hello {role, token?}`; afterwards only the role's methods are
allowed. A handler returns a result, or (streaming methods) an async iterator of notifications. While an MCP shim's call
runs, its connection is watched: a shim that leaves (its runtime gave up the call) is a fact the handler can read
(`caller_left`), and its result goes nowhere."""

import asyncio
import contextlib
import contextvars
import json
import os
import socket
import struct
from dataclasses import dataclass
from pathlib import Path
from typing import Any, AsyncIterator, Awaitable, Callable, Literal

from pydantic import BaseModel, ValidationError

from ferb.core.errors import AlreadyRunning, AuthError, FerbError, Unsupported
from ferb.daemon.auth import (RunGrant, RunTokens, Role, allowed, check_cli_peer, check_native_host,
                              check_voice_peer)

MAX_MESSAGE = 1024 * 1024

PARSE_ERROR, INVALID_REQUEST, METHOD_NOT_FOUND, INTERNAL = -32700, -32600, -32601, -32603
UNAUTHORIZED, TOO_LARGE, UNSUPPORTED, FAILED = -32001, -32002, -32003, -32004

SOL_LOCAL, LOCAL_PEERCRED = 0, 1   # <sys/un.h>


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


def exe_of(pid: int) -> str | None:
    """The executable path of a process (libproc), or None."""
    import ctypes
    import ctypes.util
    buf = ctypes.create_string_buffer(4096)
    try:
        lib = ctypes.CDLL(ctypes.util.find_library("proc") or "/usr/lib/libproc.dylib")
        n = lib.proc_pidpath(pid, buf, len(buf))
    except OSError:
        return None
    return buf.value.decode() if n > 0 else None


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
        # FERB.app: where it's installed and how to check its signature; unset = voice refused
        self.voice_app: Callable[[], str | None] = lambda: None
        self.voice_signed: Callable[[str], bool] = lambda path: False
        self._server: asyncio.base_events.Server | None = None
        self._writers: set[asyncio.StreamWriter] = set()

    async def start(self) -> None:
        self.path.parent.mkdir(parents=True, exist_ok=True)
        os.chmod(self.path.parent, 0o700)
        ensure_socket_free(self.path)
        old = os.umask(0o177)                       # socket file created 0600
        try:
            self._server = await asyncio.start_unix_server(self._connection, path=str(self.path),
                                                           limit=MAX_MESSAGE)
        finally:
            os.umask(old)

    async def close(self) -> None:
        """Stop accepting, then close open connections (e.g. a running `ferb logs`); since Python 3.12
        wait_closed() waits for every connection, so without this shutdown would hang."""
        if self._server:
            self._server.close()
            for w in list(self._writers):
                w.close()
            await self._server.wait_closed()
        self.path.unlink(missing_ok=True)

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
                session = Session(Role.CLI)
            elif hello.role is Role.MCP_SHIM:
                session = Session(Role.MCP_SHIM, self.tokens.validate(hello.token or ""))
            elif hello.role is Role.VOICE:
                expected = self.voice_app()
                if expected is None:
                    raise Unsupported("push-to-talk isn't set up (`ferb setup --voice`)")
                sock = writer.get_extra_info("socket")
                check_voice_peer(uid, self.owner_uid, exe_of(peer_pid(sock)), expected, self.voice_signed)
                session = Session(Role.VOICE)
            else:
                if self.native is None:
                    raise Unsupported("FERB's browser bridge isn't running")
                check_native_host(uid, self.owner_uid, hello.origin or "", self.extension_ids())
                session = Session(Role.NATIVE_HOST)
        except ValidationError as e:
            await self._send(writer, _error(req.id, INVALID_REQUEST, str(e.errors()[0]["msg"])))
            return None
        except AuthError as e:
            await self._send(writer, _error(req.id, UNAUTHORIZED, str(e)))
            return None
        except Unsupported as e:
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
        if req.method in self.unary:
            try:
                if session.role is Role.MCP_SHIM:
                    result = await self._watched(session, req, reader)
                    if result is None:
                        return False
                else:
                    result = await self.unary[req.method](session, req.params)
                await self._send(writer, _result(req.id, result))
            except FerbError as e:
                await self._send(writer, _error(req.id, FAILED, str(e)))
            except Exception as e:                  # failures are values, never a crashed connection
                await self._send(writer, _error(req.id, INTERNAL, f"{type(e).__name__}"))
            return True
        if req.method in self.streams:
            await self._stream(session, req, reader, writer)
            return True
        await self._send(writer, _error(req.id, METHOD_NOT_FOUND, f"no method {req.method}"))
        return True

    async def _watched(self, session: Session, req: Request, reader: asyncio.StreamReader) -> dict[str, Any] | None:
        """An MCP shim's call, watching its connection (a shim sends one call and then only closes it). When the shim
        leaves first, the call is not cancelled (an action already under way finishes and is recorded); it can see
        `caller_left()`, and its result goes nowhere: None."""
        left = asyncio.Event()
        context = contextvars.copy_context()
        context.run(_CALLER_LEFT.set, left)
        call = asyncio.create_task(self.unary[req.method](session, req.params), context=context)
        gone = asyncio.create_task(reader.read(1))
        done, _ = await asyncio.wait({call, gone}, return_when=asyncio.FIRST_COMPLETED)
        if call in done:
            gone.cancel()
            with contextlib.suppress(asyncio.CancelledError):
                await gone                    # the reader must be free before the connection reads again
            return call.result()
        left.set()
        with contextlib.suppress(Exception):
            await call
        return None

    async def _stream(self, session: Session, req: Request, reader: asyncio.StreamReader,
                      writer: asyncio.StreamWriter) -> None:
        """Send notifications until the stream ends or the client disconnects (client EOF ends it)."""
        async def pump() -> None:
            async with contextlib.aclosing(self.streams[req.method](session, req.params)) as stream:
                async for item in stream:
                    await self._send(writer, {"jsonrpc": "2.0", "method": f"{req.method}.event", "params": item})

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
            else:
                await self._send(writer, _error(req.id, FAILED, f"{type(exc).__name__}: {exc}"))
        else:
            task.cancel()
            with contextlib.suppress(asyncio.CancelledError):
                await task

    async def _send(self, writer: asyncio.StreamWriter, msg: dict[str, Any]) -> None:
        writer.write(json.dumps(msg).encode() + b"\n")
        await writer.drain()


def _result(id: Any, result: dict[str, Any]) -> dict[str, Any]:
    return {"jsonrpc": "2.0", "id": id, "result": result}


def _error(id: Any, code: int, message: str) -> dict[str, Any]:
    return {"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}}
