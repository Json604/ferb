"""Run a locked-down Claude Agent SDK session for spike 2 and collect what happened.

Lockdown = no built-in tools (`tools=[]`), no inherited settings (`setting_sources=[]`; None would load
user + project settings), strict MCP config, no plugins/agents/skills, `dontAsk` so anything not
pre-allowed is denied, and MCP server `ferb` (spike 1's server) as the only tool source.
"""

from dataclasses import dataclass, field
from pathlib import Path

import anyio
from claude_agent_sdk import (AssistantMessage, ClaudeAgentOptions, ResultMessage, SystemMessage,
                              ToolUseBlock, UserMessage, query)

from proxy import RecordingProxy

SERVER = Path(__file__).resolve().parents[1] / "01_codex_mcp" / "server.py"
SDK_VERSION = "0.2.160"  # bundles its own Claude Code CLI, so this pins the CLI too

ALLOWED_TOOLS = {"mcp__ferb__get_ticket"}


@dataclass
class SdkRun:
    init: dict = field(default_factory=dict)
    tool_uses: list[ToolUseBlock] = field(default_factory=list)
    tool_results: list[str] = field(default_factory=list)
    texts: list[str] = field(default_factory=list)
    result: ResultMessage | None = None
    request_tools: set[str] = field(default_factory=set)
    system_text: str = ""

    def final_text(self) -> str:
        return self.texts[-1] if self.texts else ""

    def everything(self) -> str:
        return "\n".join(self.texts + self.tool_results)


def lockdown_options(workspace: Path, nonce: str, call_log: Path, base_url: str,
                     extra_tools: list[str] | None = None,
                     setting_sources: list | None = None) -> ClaudeAgentOptions:
    extra = extra_tools or []
    return ClaudeAgentOptions(
        tools=extra,                       # [] -> `--tools ""`: no built-in tools at all
        allowed_tools=sorted(ALLOWED_TOOLS) + extra,
        permission_mode="dontAsk",         # anything not pre-allowed is denied, never prompted
        setting_sources=setting_sources or [],
        strict_mcp_config=True,
        plugins=[], agents=None, skills=None,
        cwd=str(workspace), max_turns=10,
        env={"ANTHROPIC_BASE_URL": base_url},  # recording proxy; auth itself is untouched
        mcp_servers={"ferb": {"type": "stdio", "command": "uv",
                              "args": ["run", "--quiet", "--script", str(SERVER)],
                              "env": {"SPIKE_NONCE": nonce, "SPIKE_CALL_LOG": str(call_log)}}},
    )


def run_sdk(prompt: str, workspace: Path, *, nonce: str, call_log: Path,
            extra_tools: list[str] | None = None, setting_sources: list | None = None) -> SdkRun:
    """extra_tools / setting_sources relax the lockdown; only control runs pass them."""
    run = SdkRun()

    async def go(base_url: str) -> None:
        opts = lockdown_options(workspace, nonce, call_log, base_url, extra_tools, setting_sources)
        async for m in query(prompt=prompt, options=opts):
            if isinstance(m, SystemMessage) and m.subtype == "init":
                run.init = m.data
            elif isinstance(m, AssistantMessage):
                for b in m.content:
                    if isinstance(b, ToolUseBlock):
                        run.tool_uses.append(b)
                    elif hasattr(b, "text"):
                        run.texts.append(b.text)
            elif isinstance(m, UserMessage) and isinstance(m.content, list):
                run.tool_results += [str(getattr(b, "content", "")) for b in m.content]
            elif isinstance(m, ResultMessage):
                run.result = m

    with RecordingProxy() as proxy:
        anyio.run(go, proxy.url)
        run.request_tools = proxy.tool_names()
        run.system_text = proxy.system_text()
    return run
