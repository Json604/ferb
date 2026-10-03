"""ClaudeRuntime (section 5 AgentRuntime) over the Claude Agent SDK — supervisor and worker modes.

Lockdown (spike 2): tools=[] (no built-in tools), permission_mode="dontAsk" (anything not pre-allowed is denied),
setting_sources=[] (None would load ~/.claude user + project settings), strict MCP, no plugins/agents/skills.
Telemetry off. Subscription login only: a run whose init reports an API key is refused (an inherited
ANTHROPIC_API_KEY would otherwise silently take over). The tool list the API actually receives is checked through
the local tool-list proxy after every turn.
"""

import secrets
from typing import Any, AsyncIterator, Literal

from ferb.cli.mcp import mcp_name
from ferb.core.types import RunHandle, RuntimeEvent, RuntimeFeatures, Task
from ferb.runtimes.proxy import ToolListProxy

SDK_VERSION = "0.2.160"
STRUCTURED_OUTPUT = "StructuredOutput"                # tool the SDK adds when output_format is set


class ClaudeRuntime:
    name = "claude"
    features = RuntimeFeatures(live_messaging=True, continuation=True, per_call_hooks=True, usage_visibility=True,
                               native_tools_contained=True)

    def __init__(self, mode: Literal["supervisor", "worker"], proxy: ToolListProxy, ferb_home: str,
                 shim_cmd: list[str] | None = None, tool_names: list[str] | None = None,
                 output_schema: dict[str, Any] | None = None, model: str | None = None, max_turns: int | None = None) -> None:
        self.mode, self.proxy, self.ferb_home, self.shim_cmd = mode, proxy, ferb_home, shim_cmd
        self.output_schema, self.model, self.max_turns = output_schema, model, max_turns
        self.allowed = {f"mcp__ferb__{mcp_name(n)}" for n in (tool_names or [])}
        if output_schema:
            self.allowed.add(STRUCTURED_OUTPUT)        # the SDK's carrier for typed output (inert)

    def _options(self, run_id: str, run_token: str) -> Any:
        from claude_agent_sdk import ClaudeAgentOptions
        env = {"ANTHROPIC_BASE_URL": self.proxy.base_url(run_id), "DISABLE_TELEMETRY": "1",
               "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC": "1", "DISABLE_ERROR_REPORTING": "1",
               "ANTHROPIC_API_KEY": "", "ANTHROPIC_AUTH_TOKEN": ""}
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
            model=self.model, max_turns=self.max_turns,
            extra_args={"no-session-persistence": None},   # no transcript (tool results, screenshots) on disk
            output_format={"type": "json_schema", "schema": self.output_schema} if self.output_schema else None)

    async def start(self, task: Task, system: str, context: str, run_token: str) -> RunHandle:
        from claude_agent_sdk import ClaudeSDKClient
        run_id = "run-" + secrets.token_hex(6)
        client = ClaudeSDKClient(options=self._options(run_id, run_token))
        await client.connect()
        await client.query(f"{system}\n\n{context}")
        return RunHandle(run_id, self.name, task.id, {"client": client, "checked_auth": False})

    async def events(self, run: RunHandle) -> AsyncIterator[RuntimeEvent]:
        from claude_agent_sdk import AssistantMessage, ResultMessage, SystemMessage, ToolUseBlock
        client = run.state["client"]
        async for m in client.receive_response():
            if isinstance(m, SystemMessage) and m.subtype == "init":
                if m.data.get("apiKeySource", "none") != "none":
                    await self.cancel(run)
                    yield RuntimeEvent("violation", {"auth": f"API key source {m.data.get('apiKeySource')!r}; "
                                                             "only the subscription login is allowed"})
                    return
                run.state["checked_auth"] = True
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
                yield RuntimeEvent("usage", {**(m.usage or {}), "cost_usd": m.total_cost_usd})
                extra = self.proxy.seen(run.id) - self.allowed
                if extra:
                    await self.cancel(run)
                    yield RuntimeEvent("violation", {"unexpected_tools": sorted(extra)})
                    return
                structured = getattr(m, "structured_output", None) or run.state.get("structured")
                text = m.result or ""
                if structured is not None:
                    import json
                    text = json.dumps(structured)
                yield RuntimeEvent("final", {"text": text, "tools_seen": sorted(self.proxy.seen(run.id))})

    async def continue_(self, run: RunHandle, message: str) -> None:
        await run.state["client"].query(message)

    async def cancel(self, run: RunHandle) -> None:
        client = run.state.get("client")
        if client is not None:
            try:
                await client.disconnect()
            finally:
                self.proxy.forget(run.id)
                run.state["client"] = None
