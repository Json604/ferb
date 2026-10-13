"""Owns the Claude SDK adapter and tool containment. Must not select task targets, authorize actions or expose tools outside the registered harness.

ClaudeRuntime over the Claude Agent SDK — supervisor and worker modes.

Lockdown (spike 2): tools=[] (no built-in tools), permission_mode="dontAsk" (anything not pre-allowed is denied),
setting_sources=[] (None would load ~/.claude user + project settings), strict MCP, no plugins/agents/skills.
Telemetry off. Subscription login only: a run whose init reports an API key is refused (an inherited
ANTHROPIC_API_KEY would otherwise silently take over). The tool list the API actually receives is checked through
the local tool-list proxy after every turn."""

import asyncio
import secrets
import uuid
from dataclasses import asdict
from contextlib import suppress
from typing import Any, AsyncIterator, Literal

from ferb.cli.mcp import mcp_name
from ferb.core.errors import Unsupported
from ferb.core.types import ModelRequest, RunHandle, Capabilities, RuntimeEvent, Task
from ferb.runtimes.proxy import ToolListProxy
from ferb.runtimes.usage import from_anthropic

SDK_VERSION = "0.2.160"
STRUCTURED_OUTPUT = "StructuredOutput"                # tool the SDK adds when output_format is set


class ClaudeRuntime:
    name = "claude"
    capabilities = Capabilities("direct", "prefix", False, "native", False, True, True, True, "subscription")

    def __init__(self, mode: Literal["supervisor", "worker"], proxy: ToolListProxy, ferb_home: str,
                 shim_cmd: list[str] | None = None, tool_names: list[str] | None = None,
                 output_schema: dict[str, Any] | None = None, model: str | None = None) -> None:
        self.mode, self.proxy, self.ferb_home, self.shim_cmd = mode, proxy, ferb_home, shim_cmd
        self.output_schema, self.model = output_schema, model
        self.allowed = {f"mcp__ferb__{mcp_name(n)}" for n in (tool_names or [])}
        if output_schema:
            self.allowed.add(STRUCTURED_OUTPUT)        # the SDK's carrier for typed output (inert)

    def _options(self, run_id: str, run_token: str) -> Any:
        from claude_agent_sdk import ClaudeAgentOptions
        # CLAUDE_CODE_MCP_TOOL_IDLE_TIMEOUT=0: Claude Code otherwise aborts a tool call that is silent for 30 minutes;
        # an owner question waits as long as the owner takes (§8.1). "0 disables" is Claude Code's own wording.
        env = {"ANTHROPIC_BASE_URL": self.proxy.base_url(run_id), "DISABLE_TELEMETRY": "1",
               "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC": "1", "DISABLE_ERROR_REPORTING": "1",
               "ANTHROPIC_API_KEY": "", "ANTHROPIC_AUTH_TOKEN": "", "CLAUDE_CODE_MCP_TOOL_IDLE_TIMEOUT": "0"}
        mcp: dict[str, Any] = {}
        if self.mode == "worker":
            if not self.shim_cmd:
                raise ValueError("worker mode needs the ferb mcp shim")
            mcp = {"ferb": {"type": "stdio", "command": self.shim_cmd[0], "args": self.shim_cmd[1:],
                            "env": {"FERB_RUN_TOKEN": run_token, "FERB_HOME": self.ferb_home}}}
        return ClaudeAgentOptions(
            tools=[], allowed_tools=sorted(self.allowed - {STRUCTURED_OUTPUT}), permission_mode="dontAsk",
            setting_sources=[],
            strict_mcp_config=True, plugins=[], agents=None, skills=None, mcp_servers=mcp, env=env,
            model=self.model,
            include_partial_messages=self.mode == 'supervisor' and self.output_schema is not None,
            extra_args={"no-session-persistence": None, "replay-user-messages": None},   # no transcript (tool results, screenshots) on disk
            output_format={"type": "json_schema", "schema": self.output_schema} if self.output_schema else None)

    async def start(self, task: Task, request: ModelRequest, run_token: str) -> RunHandle:
        from claude_agent_sdk import ClaudeSDKClient
        run_id = "run-" + secrets.token_hex(6)
        client = ClaudeSDKClient(options=self._options(run_id, run_token))
        try:
            await client.connect()
            await client.query(f"{request.system}\n\n{request.context}")
        except BaseException:
            with suppress(Exception):
                await client.disconnect()
            if self.proxy is not None:
                self.proxy.forget(run_id)
            raise
        return RunHandle(run_id, self.name, task.id, {"client": client, "checked_auth": False})

    async def events(self, run: RunHandle) -> AsyncIterator[RuntimeEvent]:
        client = run.state["client"]
        if client is None or run.state.get("closed"):
            return
        try:
            async for event in self._events(run, client):
                yield event
        finally:
            run.state["closed"] = True

    async def _events(self, run: RunHandle, client: Any) -> AsyncIterator[RuntimeEvent]:
        from claude_agent_sdk import AssistantMessage, ResultMessage, SystemMessage, ToolUseBlock, UserMessage
        from claude_agent_sdk.types import StreamEvent
        structured_blocks: set[int] = set()
        # receive_response stops at the first ResultMessage, even with queued input.
        async for m in client.receive_messages():
            if isinstance(m, UserMessage):
                run.state.setdefault("pending_updates", set()).discard(m.uuid)
                continue
            if isinstance(m, SystemMessage) and m.subtype == "init":
                if m.data.get("apiKeySource", "none") != "none":
                    await self.cancel(run)
                    yield RuntimeEvent("violation", {"auth": f"API key source {m.data.get('apiKeySource')!r}; "
                                                             "only the subscription login is allowed"})
                    return
                run.state["checked_auth"] = True
            elif isinstance(m, StreamEvent):
                # The SDK's typed result is a StructuredOutput tool payload, not ordinary assistant commentary.
                if not run.state.get('checked_auth') or m.parent_tool_use_id is not None:
                    continue
                event = m.event
                kind = event.get('type')
                if kind == 'message_start':
                    structured_blocks.clear()
                elif kind == 'content_block_start':
                    block = event.get('content_block') or {}
                    if block.get('type') == 'tool_use' and block.get('name') == STRUCTURED_OUTPUT:
                        structured_blocks.add(event['index'])
                elif kind == 'content_block_delta' and event.get('index') in structured_blocks:
                    delta = event.get('delta') or {}
                    if delta.get('type') == 'input_json_delta' and delta.get('partial_json'):
                        yield RuntimeEvent('text_delta', {'text': delta['partial_json']})
                elif kind == 'content_block_stop':
                    structured_blocks.discard(event.get('index'))
            elif isinstance(m, AssistantMessage):
                for b in m.content:
                    if isinstance(b, ToolUseBlock):
                        if b.name == STRUCTURED_OUTPUT:
                            run.state["structured"] = b.input
                        else:
                            yield RuntimeEvent("tool_call", {"tool": b.name, "arguments": b.input})
                    elif hasattr(b, "text") and b.text:
                        yield RuntimeEvent("message", {"text": b.text})
            elif isinstance(m, ResultMessage):
                yield RuntimeEvent("usage", asdict(from_anthropic(self.model or "claude", dict(m.usage or {}),
                                                                  billing="subscription",
                                                                  provider_cost=m.total_cost_usd)))
                if getattr(m, "is_error", False):
                    run.state["closed"] = True
                    yield RuntimeEvent("error", {"message": m.result or str(getattr(m, "errors", None)) or m.subtype})
                    return
                extra = self.proxy.seen(run.id) - self.allowed
                if extra:
                    await self.cancel(run)
                    yield RuntimeEvent("violation", {"unexpected_tools": sorted(extra)})
                    return
                if run.state.get("pending_updates"):
                    # A late result from the previous input cannot finish accepted corrections.
                    run.state.pop("structured", None)
                    continue
                structured = getattr(m, "structured_output", None) or run.state.get("structured")
                text = m.result or ""
                if structured is not None:
                    import json
                    text = json.dumps(structured)
                run.state["closed"] = True
                yield RuntimeEvent("final", {"text": text, "tools_seen": sorted(self.proxy.seen(run.id))})
                return

    async def continue_(self, run: RunHandle, message: str) -> None:
        async with run.state.setdefault("input_lock", asyncio.Lock()):
            client = run.state.get("client")
            if client is None or run.state.get("closed"):
                raise Unsupported("the Claude run has already ended")
            ident = str(uuid.uuid4())
            pending = run.state.setdefault("pending_updates", set())
            pending.add(ident)

            async def frames() -> AsyncIterator[dict[str, Any]]:
                yield {"type": "user", "message": {"role": "user", "content": message},
                       "parent_tool_use_id": None, "uuid": ident}

            try:
                # The SDK writes to the same live CLI. UUID replay acknowledgements also
                # cover multiple queued inputs merged into a single model turn by Claude.
                await client.query(frames())
            except BaseException:
                pending.discard(ident)
                with suppress(Exception):
                    await self.cancel(run)
                raise

    async def cancel(self, run: RunHandle) -> None:
        run.state["closed"] = True
        client = run.state.get("client")
        if client is not None:
            try:
                await client.disconnect()
            finally:
                self.proxy.forget(run.id)
                run.state["client"] = None
