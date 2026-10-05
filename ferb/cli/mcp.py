"""Translate MCP requests to daemon tool RPC and back. Must not bypass gateway validation or duplicate orchestration.

`ferb mcp`: the stdio MCP shim runtimes spawn. No logic: it lists the tools the daemon says this
worker may use and forwards each call, with FERB_RUN_TOKEN, to the gateway. Every decision happens in the daemon.

Tool names the model sees use `_` instead of `.` (OpenAI function names allow only [a-zA-Z0-9_-])."""

import json
import os
import sys
from typing import Any

import anyio
import mcp.types as t
from mcp.server.lowlevel import Server
from mcp.server.stdio import stdio_server

from ferb.cli.client import Client, RpcError
from ferb.core.errors import NotRunning
from ferb.paths import socket_path


def mcp_name(capability: str) -> str:
    return capability.replace(".", "_")


SHIM_TIMEOUT_S = 3600.0


class Shim:
    def __init__(self) -> None:
        token = os.environ.get("FERB_RUN_TOKEN")
        token_file = os.environ.get("FERB_RUN_TOKEN_FILE")
        if not token and token_file:                  # Codex: the token lives in a 0600 file, never in argv
            try:
                token = open(token_file).read().strip()
            except OSError:
                token = None
        if not token:
            raise SystemExit("ferb mcp: FERB_RUN_TOKEN is not set (only runtimes started by FERB may use this)")
        self._token = token
        self._names: dict[str, str] = {}          # mcp name -> capability name

    def _client(self) -> Client:
        # Tool calls can wait on the owner or an external process. This is a transport timeout;
        # task progress and explicit cancellation are owned by the daemon.
        return Client(socket_path(), role="mcp_shim", token=self._token, timeout=SHIM_TIMEOUT_S)

    def _call(self, method: str, params: dict[str, Any] | None = None) -> dict[str, Any]:
        c = self._client()
        try:
            return c.call(method, params or {})
        finally:
            c.close()

    async def list_tools(self, ctx: Any, params: Any) -> t.ListToolsResult:
        tools = await anyio.to_thread.run_sync(lambda: self._call("tools.list")["tools"])
        self._names = {mcp_name(x["name"]): x["name"] for x in tools}
        return t.ListToolsResult(tools=[t.Tool(name=mcp_name(x["name"]), description=x["description"],
                                               input_schema=x["input_schema"]) for x in tools])

    async def call_tool(self, ctx: Any, params: t.CallToolRequestParams) -> t.CallToolResult:
        name = self._names.get(params.name, params.name.replace("_", ".", 1))
        try:
            r = await anyio.to_thread.run_sync(
                lambda: self._call("tools.call", {"name": name, "arguments": params.arguments or {}}))
        except (RpcError, NotRunning) as e:
            return t.CallToolResult(content=[t.TextContent(type="text", text=f"FERB refused: {e}")], is_error=True)
        text = json.dumps(r["content"], ensure_ascii=False) + "\n" + r["footer"]
        parts: list[t.TextContent | t.ImageContent | t.AudioContent | t.ResourceLink | t.EmbeddedResource] = [
            t.TextContent(type="text", text=text)]
        if r.get("image"):
            parts.append(t.ImageContent(type="image", data=r["image"]["data"], mime_type=r["image"]["mime"]))
        return t.CallToolResult(content=parts, is_error=bool(r["is_error"]))


async def serve() -> None:
    shim = Shim()
    server: Server[Any] = Server("ferb", version="1", on_list_tools=shim.list_tools, on_call_tool=shim.call_tool)
    async with stdio_server() as (read, write):
        await server.run(read, write, server.create_initialization_options())


def main() -> None:
    try:
        anyio.run(serve)
    except KeyboardInterrupt:
        sys.exit(130)


if __name__ == "__main__":
    main()
