"""Writes the fixtures the Rust crate `ferb-rpc` is checked against (ENG-3).

Run from the repo root:  uv run python rust/fixtures/rpc/make.py

- table.json: auth.py's role/method table, APP_ONLY and server.py's constants.
- frames.json: scenarios played against the real `ferb.daemon.server.Server` (in this process, on a socket in a
  temporary folder, with the stub handlers below), each step's reply recorded as the exact bytes the server wrote.
  The Rust integration test (crates/ferb-rpc/tests/golden.rs) registers the same stubs and replays every step.

Nothing here reads or writes ~/.ferb: FERB_HOME points at a temporary folder before anything of FERB is imported.
"""

import asyncio
import json
import os
import sys
import tempfile
from pathlib import Path
from typing import Any, AsyncIterator

_TMP = tempfile.mkdtemp(prefix="ferb-rpc-fx-")
os.environ["FERB_HOME"] = str(Path(_TMP) / "home")
sys.path.insert(0, str(Path(__file__).resolve().parents[3]))

from ferb.clock.clock import FakeClock  # noqa: E402
from ferb.core.errors import Refused  # noqa: E402
from ferb.core.types import Identity  # noqa: E402
from ferb.daemon import auth, server as srv  # noqa: E402

HERE = Path(__file__).resolve().parent
EXT = "jaclnphefmmgmpofnabffahdkbjjompm"
MAX = srv.MAX_MESSAGE


# ---------- the stub handlers (mirrored in crates/ferb-rpc/tests/golden.rs) ----------

async def ping(session: srv.Session, params: dict[str, Any]) -> dict[str, Any]:
    return {"pid": 4242, "role": session.role.value, "now": "2026-11-13T10:00:00+00:00", "stopped": False}


async def echo(session: srv.Session, params: dict[str, Any]) -> dict[str, Any]:
    return {"echo": params}


async def values(session: srv.Session, params: dict[str, Any]) -> dict[str, Any]:
    return {"text": "naïve — 日本 \U0001f600 \x7f \x01 \"q\" \\ / \n\t\r\b\f", "float": 1.5,
            "big": 1e16, "small": 1e-07, "whole": 2.0, "third": 0.1, "neg": -2.5e-05, "large": 123456789012345.0,
            "edge": 1234567890123456.0, "edge2": 12345678901234567.0, "tiny": 0.0001, "tinier": 0.00001,
            "avogadro": 6.02214076e23, "int": -7, "u64": 18446744073709551615,
            "list": [1, "a", None, True, False], "nested": {"k": {}}, "empty": []}


async def refuse(session: srv.Session, params: dict[str, Any]) -> dict[str, Any]:
    raise Refused("the check said no")


async def crash(session: srv.Session, params: dict[str, Any]) -> dict[str, Any]:
    raise RuntimeError("boom, with words that must not reach the caller")


async def size(session: srv.Session, params: dict[str, Any]) -> dict[str, Any]:
    return {"len": len(params.get("p", ""))}


async def preview(session: srv.Session, params: dict[str, Any]) -> dict[str, Any]:
    return {"frame": 1}


async def tool_call(session: srv.Session, params: dict[str, Any]) -> dict[str, Any]:
    assert session.grant is not None
    return {"task": session.grant.task_id, "worker": session.grant.worker_id, "left": srv.caller_left()}


async def count(session: srv.Session, params: dict[str, Any]) -> AsyncIterator[dict[str, Any]]:
    for i in range(int(params.get("n", 2))):
        yield {"i": i}


async def refusing_stream(session: srv.Session, params: dict[str, Any]) -> AsyncIterator[dict[str, Any]]:
    yield {"first": True}
    raise Refused("the stream said no")


async def crashing_stream(session: srv.Session, params: dict[str, Any]) -> AsyncIterator[dict[str, Any]]:
    raise ValueError("bad value")
    yield {}                                                   # pragma: no cover


async def native(reader: asyncio.StreamReader, writer: asyncio.StreamWriter) -> None:
    line = await reader.readline()
    writer.write(b"native:" + line)
    await writer.drain()


UNARY = {"ping": ping, "trace.get": echo, "about.get": values, "doctor.check": refuse, "memory.list": crash,
         "memory.show": size, "tasks.preview": preview, "tools.call": tool_call}
STREAMS = {"logs.subscribe": count, "notifications.subscribe": refusing_stream, "ps.attach": crashing_stream}
# in the table, never registered here: "ps.list" (cli), "tools.list" (mcp_shim)


# ---------- scenarios ----------

def req(id: Any, method: str, params: Any = None, **extra: Any) -> dict[str, str]:
    msg: dict[str, Any] = {"jsonrpc": "2.0", "id": id, "method": method}
    if params is not None:
        msg["params"] = params
    msg.update(extra)
    return {"send": json.dumps(msg) + "\n"}


def text(line: str) -> dict[str, str]:
    return {"send": line}


HELLO = req(1, "auth.hello", {"role": "cli"})
SHIM = req(1, "auth.hello", {"role": "mcp_shim", "token": "$TOKEN"})
R: dict[str, Any] = {"recv": None}
EOF = {"eof": True}
TOKEN = {"task_id": "task-1.2", "worker_id": "w-1", "ttl_s": 60}


def padded(prefix: str, suffix: str, content_len: int) -> dict[str, Any]:
    """One frame: prefix + "a" * n + suffix + newline, its content (without the newline) `content_len` bytes."""
    return {"send_padded": {"prefix": prefix, "suffix": suffix, "content_len": content_len}}


FIT_PREFIX, FIT_SUFFIX = '{"jsonrpc":"2.0","id":7,"method":"memory.show","params":{"p":"', '"}}'

SCENARIOS: list[dict[str, Any]] = [
    {"name": "a call before auth.hello is refused and the connection ends",
     "steps": [req(1, "ping", {}), R, EOF]},
    {"name": "cli hello, ping, echo",
     "steps": [HELLO, R, req(2, "ping"), R, req(3, "trace.get", {"task_id": "x", "n": [1, 2, {"a": None}]}), R]},
    {"name": "values a result can carry are written as json.dumps writes them",
     "steps": [HELLO, R, req(2, "about.get"), R,
               req(3, "trace.get", {"s": "café \U0001f600", "f": 1.0, "e": 1e300, "z": 0.0, "nz": -0.0}), R]},
    {"name": "a refusal is -32004 with its words; a fault is -32603 with only its type",
     "steps": [HELLO, R, req(2, "doctor.check"), R, req(3, "memory.list"), R, req(4, "ping"), R]},
    {"name": "a method outside the role's list, and one in it that nobody registered",
     "steps": [HELLO, R, req(2, "nope"), R, req(3, "tools.call"), R, req(4, "ps.list"), R, req(5, "auth.hello", {"role": "cli"}), R]},
    {"name": "a FERB.app-only method from another program is -32005",
     "steps": [HELLO, R, req(2, "tasks.preview", {"task_id": "t"}), R, req(3, "intro.preview"), R]},
    {"name": "a FERB.app-only method from FERB.app is answered", "server": {"is_app": True},
     "steps": [HELLO, R, req(2, "tasks.preview", {"task_id": "t"}), R]},
    {"name": "a stream sends its events, then that it ended; the connection keeps serving",
     "steps": [HELLO, R, req(2, "logs.subscribe", {"n": 3}), R, R, R, R, req(3, "logs.subscribe", {"n": 0}), R,
               req(4, "ping"), R]},
    {"name": "a stream that fails says so with -32004 and the error's type",
     "steps": [HELLO, R, req(2, "notifications.subscribe"), R, R, req(3, "ps.attach"), R, req(4, "ping"), R]},
    {"name": "request ids: text, none, null, whole float, true, false, negative, large",
     "steps": [HELLO, R, req("abc", "ping"), R, text('{"jsonrpc":"2.0","method":"ping"}\n'), R, req(None, "ping"), R,
               text('{"jsonrpc":"2.0","id":4.0,"method":"ping"}\n'), R, req(True, "ping"), R, req(False, "ping"), R,
               req(-12, "ping"), R, req(9223372036854775807, "ping"), R, req("", "ping"), R,
               req("café", "ping"), R]},
    {"name": "requests that are not valid requests get -32600 with pydantic's words",
     "steps": [HELLO, R,
               text('{"jsonrpc":"1.0","id":2,"method":"ping"}\n'), R,
               text('{"id":2,"method":"ping"}\n'), R,
               text('{"jsonrpc":"2.0","id":2}\n'), R,
               text('{"jsonrpc":"2.0","id":2,"method":5}\n'), R,
               text('{"jsonrpc":"2.0","id":2,"method":null}\n'), R,
               text('{"jsonrpc":"2.0","id":2,"method":"ping","params":[]}\n'), R,
               text('{"jsonrpc":"2.0","id":2,"method":"ping","params":null}\n'), R,
               text('{"jsonrpc":"2.0","id":2,"method":"ping","params":"x"}\n'), R,
               text('{"jsonrpc":"2.0","id":1.5,"method":"ping"}\n'), R,
               text('{"jsonrpc":"2.0","id":[1],"method":"ping"}\n'), R,
               text('{"jsonrpc":"2.0","id":{},"method":"ping"}\n'), R,
               text('{"jsonrpc":2.0,"id":2,"method":"ping"}\n'), R,
               text('{"jsonrpc":null,"id":2,"method":"ping"}\n'), R,
               text('{"jsonrpc":"1.0","id":[1],"method":5,"params":3}\n'), R,
               text('[1]\n'), R, text('"x"\n'), R, text('5\n'), R, text('null\n'), R, text('true\n'), R,
               text('{}\n'), R,
               req(3, "ping"), R]},
    {"name": "lines that are not JSON get -32700 and the connection keeps serving",
     "steps": [HELLO, R, text("{not json\n"), R, text("\n"), R, text("   \n"), R, text('{"jsonrpc":"2.0",}\n'), R,
               text('{"jsonrpc":"2.0","id":1,"method":"ping"} trailing\n'), R, text("\r\n"), R,
               req(3, "ping"), R]},
    {"name": "spacing, extra fields, a repeated key and CRLF are accepted",
     "steps": [HELLO, R, text('  { "jsonrpc" : "2.0" , "id" : 2 , "method" : "ping" , "extra" : [1] }  \n'), R,
               text('{"jsonrpc":"2.0","id":3,"id":4,"method":"nope","method":"ping"}\n'), R,
               text('{"jsonrpc":"2.0","id":5,"method":"ping"}\r\n'), R,
               text('{"jsonrpc":"2.0","id":6,"method":"trace.get","params":{"a":1,"a":2,"b":"\\ud83d\\ude00"}}\n'), R]},
    {"name": "garbage before hello: not JSON and not a request keep the connection, a non-hello ends it",
     "steps": [text("{not json\n"), R, text('{"jsonrpc":"1.0"}\n'), R, req(5, "trace.get"), R, EOF]},
    {"name": "hello with an unknown role", "steps": [req(1, "auth.hello", {"role": "admin"}), R, EOF]},
    {"name": "hello without a role", "steps": [req("h", "auth.hello", {}), R, EOF]},
    {"name": "hello without params", "steps": [req(1, "auth.hello"), R, EOF]},
    {"name": "hello with a token that is not text",
     "steps": [req(1, "auth.hello", {"role": "mcp_shim", "token": 5}), R, EOF]},
    {"name": "hello with an origin that is not text",
     "steps": [req(1, "auth.hello", {"role": "native_host", "origin": ["x"]}), R, EOF]},
    {"name": "hello with a role that is not text", "steps": [req(1, "auth.hello", {"role": 1}), R, EOF]},
    {"name": "cli hello with extra fields and a null token",
     "steps": [req(1, "auth.hello", {"role": "cli", "token": None, "origin": None, "more": 1}), R, req(2, "ping"), R]},
    {"name": "cli hello from another uid is refused", "server": {"owner_uid_other": True},
     "steps": [HELLO, R, EOF]},
    {"name": "mcp_shim hello with an unknown token", "server": {"token": TOKEN},
     "steps": [req(1, "auth.hello", {"role": "mcp_shim", "token": "made-up"}), R, EOF]},
    {"name": "mcp_shim hello without a token",
     "steps": [req(1, "auth.hello", {"role": "mcp_shim"}), R, EOF]},
    {"name": "mcp_shim: its three methods only, and a revoked token stops at the next call",
     "server": {"token": TOKEN},
     "steps": [SHIM, R, req(2, "ping"), R, req(3, "trace.get", {"task_id": "t"}), R, req(4, "tools.list"), R,
               {"op": "revoke_all"}, req(5, "ping"), R, req(6, "nope"), R]},
    {"name": "mcp_shim: a call is answered and the connection keeps serving", "server": {"token": TOKEN},
     "steps": [SHIM, R, req(2, "tools.call", {"call_id": "c1", "name": "x"}), R, req(3, "ping"), R]},
    {"name": "mcp_shim: an expired token stops at the next call", "server": {"token": TOKEN},
     "steps": [SHIM, R, req(2, "ping"), R, {"op": "advance", "seconds": 60}, req(3, "ping"), R]},
    {"name": "mcp_shim hello with an expired token", "server": {"token": TOKEN},
     "steps": [{"op": "advance", "seconds": 61}, SHIM, R, EOF]},
    {"name": "mcp_shim: a tree's tokens go with the tree", "server": {"token": TOKEN},
     "steps": [SHIM, R, {"op": "revoke_tree", "tree": "task-1"}, req(2, "ping"), R]},
    {"name": "native_host hello when the browser bridge is not running",
     "steps": [req(1, "auth.hello", {"role": "native_host", "origin": f"chrome-extension://{EXT}/"}), R, EOF]},
    {"name": "native_host hello from an unknown extension", "server": {"native": True},
     "steps": [req(1, "auth.hello", {"role": "native_host", "origin": "chrome-extension://" + "a" * 32 + "/"}), R, EOF]},
    {"name": "native_host hello without an origin", "server": {"native": True},
     "steps": [req(1, "auth.hello", {"role": "native_host"}), R, EOF]},
    {"name": "native_host hello with a web origin", "server": {"native": True},
     "steps": [req(1, "auth.hello", {"role": "native_host", "origin": f"https://{EXT}/"}), R, EOF]},
    {"name": "native_host hello from another uid", "server": {"native": True, "owner_uid_other": True},
     "steps": [req(1, "auth.hello", {"role": "native_host", "origin": f"chrome-extension://{EXT}/"}), R, EOF]},
    {"name": "native_host: after hello the connection is the extension channel", "server": {"native": True},
     "steps": [req(1, "auth.hello", {"role": "native_host", "origin": f"chrome-extension://{EXT}/"}), R,
               text("anything at all\n"), R, EOF]},
    {"name": "a message over the cap is refused and the connection ends",
     "steps": [HELLO, R, padded("", "", MAX + 1), R, EOF]},
    {"name": "a message over the cap before hello",
     "steps": [padded("", "", MAX + 10), R, EOF]},
    {"name": "a message of exactly the cap is taken",
     "steps": [HELLO, R, padded(FIT_PREFIX, FIT_SUFFIX, MAX), R, req(8, "ping"), R]},
    {"name": "a message one byte over the cap is refused",
     "steps": [HELLO, R, padded(FIT_PREFIX, FIT_SUFFIX, MAX + 1), R, EOF]},
    {"name": "a last line without its newline is answered when the caller stops sending",
     "steps": [HELLO, R, text('{"jsonrpc":"2.0","id":9,"method":"ping"}'), {"op": "shut_wr"}, R, EOF]},
    {"name": "a line that is not UTF-8 ends the connection without an answer",
     "steps": [HELLO, R, {"send_hex": "7b22ff227d0a"}, EOF]},
    {"name": "while the daemon starts only ping is answered; the rest waits for the start",
     "server": {"starting": True},
     "steps": [HELLO, R, req(2, "ping"), R, req(3, "trace.get", {"a": 1}), {"silent_ms": 200}, {"op": "set_ready"},
               R, req(4, "nope"), R]},
    {"name": "while starting, the role and FERB.app checks still answer at once", "server": {"starting": True},
     "steps": [HELLO, R, req(2, "nope"), R, req(3, "tasks.preview"), R]},
    {"name": "a start that fails ends the connections that waited for it", "server": {"starting": True},
     "steps": [HELLO, R, req(2, "trace.get"), {"silent_ms": 100}, {"op": "close"}, EOF]},
]


# ---------- playing them ----------

async def play(scenario: dict[str, Any], folder: Path) -> dict[str, Any]:
    opts = scenario.get("server", {})
    clock = FakeClock()
    tokens = auth.RunTokens(clock)
    server = srv.Server(folder / "s.sock", tokens, owner_uid=(os.getuid() + 1) if opts.get("owner_uid_other") else None)
    server.unary.update(UNARY)                                   # type: ignore[arg-type]
    server.streams.update(STREAMS)                               # type: ignore[arg-type]
    if opts.get("native"):
        server.native = native
        server.extension_ids = lambda: frozenset({EXT})
    if opts.get("is_app"):
        server.is_app = lambda token: True
    if opts.get("starting"):
        server.ready.clear()
    token = ""
    if "token" in opts:
        t = opts["token"]
        token = tokens.issue(t["task_id"], t["worker_id"], set(), Identity.OWNER, ttl_s=t["ttl_s"])
    await server.start()
    reader, writer = await asyncio.open_unix_connection(str(folder / "s.sock"), limit=8 * MAX)
    steps: list[dict[str, Any]] = []
    try:
        for step in scenario["steps"]:
            step = dict(step)
            if "send" in step:
                writer.write(step["send"].replace("$TOKEN", token).encode())
                await writer.drain()
            elif "send_hex" in step:
                writer.write(bytes.fromhex(step["send_hex"]))
                await writer.drain()
            elif "send_padded" in step:
                p = step["send_padded"]
                fill = p["content_len"] - len(p["prefix"]) - len(p["suffix"])
                writer.write((p["prefix"] + "a" * fill + p["suffix"] + "\n").encode())
                await writer.drain()
            elif "recv" in step:
                line = await asyncio.wait_for(reader.readline(), 5)
                assert line.endswith(b"\n"), (scenario["name"], line)
                step["recv"] = line.decode("ascii")              # json.dumps writes ASCII only
            elif "eof" in step:
                assert await asyncio.wait_for(reader.read(1), 5) == b"", scenario["name"]
            elif "silent_ms" in step:
                try:
                    got = await asyncio.wait_for(reader.read(1), step["silent_ms"] / 1000)
                except asyncio.TimeoutError:
                    got = None
                assert got is None, (scenario["name"], got)
            elif step["op"] == "revoke_all":
                tokens.revoke_all()
            elif step["op"] == "revoke_tree":
                tokens.revoke_tree(step["tree"])
            elif step["op"] == "advance":
                clock.advance(step["seconds"])
            elif step["op"] == "set_ready":
                server.ready.set()
            elif step["op"] == "shut_wr":
                writer.write_eof()
            elif step["op"] == "close":
                await server.close()
            else:
                raise AssertionError(step)
            steps.append(step)
    finally:
        writer.close()
        await server.close()
    return {"name": scenario["name"], "server": opts, "steps": steps}


def table() -> dict[str, Any]:
    return {
        "roles": {role.value: sorted(methods) for role, methods in auth.METHODS.items()},
        "app_only": sorted(auth.APP_ONLY),
        "max_message": srv.MAX_MESSAGE,
        "codes": {"PARSE_ERROR": srv.PARSE_ERROR, "INVALID_REQUEST": srv.INVALID_REQUEST,
                  "METHOD_NOT_FOUND": srv.METHOD_NOT_FOUND, "INTERNAL": srv.INTERNAL,
                  "UNAUTHORIZED": srv.UNAUTHORIZED, "TOO_LARGE": srv.TOO_LARGE, "UNSUPPORTED": srv.UNSUPPORTED,
                  "FAILED": srv.FAILED, "APP_ONLY_REFUSED": srv.APP_ONLY_REFUSED},
        "client_wait_s": srv.CLIENT_WAIT_S,
        "long_waits": dict(sorted(srv.LONG_WAITS.items())),
        "cancel_when_left": sorted(srv.CANCEL_WHEN_LEFT),
        "answered_while_starting": sorted(srv.ANSWERED_WHILE_STARTING),
        "journal_kinds": {"INTERNAL_ERROR": srv.INTERNAL_ERROR, "SLOW_CALL": srv.SLOW_CALL},
        "dev_requirement": __import__("ferb.daemon.app_peer", fromlist=["x"]).DEV_REQUIREMENT,
        "app_id": __import__("ferb.paths", fromlist=["x"]).APP_ID,
    }


async def main() -> None:
    played = []
    for i, scenario in enumerate(SCENARIOS):
        folder = Path(_TMP) / f"s{i}"
        folder.mkdir()
        played.append(await play(scenario, folder))
    (HERE / "table.json").write_text(json.dumps(table(), indent=1) + "\n")
    (HERE / "frames.json").write_text(json.dumps({"stub_extension_id": EXT, "scenarios": played}, indent=1) + "\n")
    t = table()
    print(f"table.json: {len(t['roles']['cli'])} cli, {len(t['roles']['mcp_shim'])} mcp_shim, "
          f"{len(t['roles']['native_host'])} native_host, {len(t['app_only'])} FERB.app-only")
    print(f"frames.json: {len(played)} scenarios, {sum('recv' in s for p in played for s in p['steps'])} frames")


if __name__ == "__main__":
    import shutil
    try:
        asyncio.run(main())
    finally:
        shutil.rmtree(_TMP, ignore_errors=True)
