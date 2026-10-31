"""Run `codex exec --json` locked down for spike 1 and collect what happened.

Lockdown = user config and rules ignored, strict config, every disableable native
tool feature off, read-only sandbox (enforces the rest), MCP server `ferb` as the
only tool source, MCP calls pre-approved so no prompt is needed.
"""

import json
import os
import subprocess
from dataclasses import dataclass, field
from pathlib import Path

SERVER = Path(__file__).with_name("server.py")

# Pinned in the repo's package.json; upgrading means rerunning these tests first.
CODEX = Path(__file__).resolve().parents[2] / "node_modules" / ".bin" / "codex"
CODEX_VERSION = "0.166.0"


def codex_version() -> str:
    out = subprocess.run([str(CODEX), "--version"], capture_output=True, text=True, check=True).stdout
    return out.split()[-1]

# Non-code-mode model: no `exec` (JS tool runner) and no `collaboration.*` subagents.
# All gpt-5.6/6 catalog entries are code_mode_only with built-in multi-agent v2.
WORKER_MODEL = "gpt-5.5"

DISABLED_FEATURES = [
    "shell_tool", "unified_exec", "shell_snapshot",
    "multi_agent", "multi_agent_v2",
    "apps", "plugins", "remote_plugin", "tool_suggest", "skill_search", "hooks",
    "browser_use", "browser_use_external", "in_app_browser", "computer_use",
    "image_generation", "view_image",
    "goals", "sleep_tool", "code_mode_host",
]

# Every tool the model may see. Anything else appearing (e.g. after a Codex update) fails the test.
ALLOWED_TOOLS = {
    "mcp__ferb.get_ticket",  # MCP tools arrive as namespace `mcp__<server>` after tool_search loads them
    # Built-in MCP plumbing: read only over configured MCP servers (only `ferb`).
    "list_mcp_resources", "list_mcp_resource_templates", "read_mcp_resource", "tool_search",
    # Asks the owner; fails closed in exec (no interactive user).
    "request_user_input",
    # Native file writer we cannot disable; the read-only sandbox rejects its writes.
    "apply_patch",
}


@dataclass
class CodexRun:
    events: list[dict]
    tools_seen: set[str]
    returncode: int
    stderr_tail: str
    items: list[dict] = field(default_factory=list)

    def items_of(self, kind: str) -> list[dict]:
        return [i for i in self.items if i.get("type") == kind]

    def final_text(self) -> str:
        msgs = self.items_of("agent_message")
        return msgs[-1]["text"] if msgs else ""


def run_codex(prompt: str, workspace: Path, *, nonce: str, call_log: Path,
              shell_enabled: bool = False, sandbox: str = "read-only",
              timeout: float = 240) -> CodexRun:
    disabled = [f for f in DISABLED_FEATURES
                if not (shell_enabled and f in ("shell_tool", "unified_exec"))]
    env_toml = "{" + f'SPIKE_NONCE="{nonce}",SPIKE_CALL_LOG="{call_log}"' + "}"
    cmd = [
        str(CODEX), "exec", "--json", "--ephemeral", "--skip-git-repo-check",
        "--ignore-user-config", "--ignore-rules", "--strict-config",
        "-s", sandbox, "-C", str(workspace), "-m", WORKER_MODEL,
        "-c", 'approval_policy="never"',
        "-c", 'web_search="disabled"',
        "-c", 'mcp_servers.ferb.command="uv"',
        "-c", f'mcp_servers.ferb.args=["run","--quiet","--script","{SERVER}"]',
        "-c", f"mcp_servers.ferb.env={env_toml}",
        "-c", 'mcp_servers.ferb.default_tools_approval_mode="approve"',
    ]
    for f in disabled:
        cmd += ["--disable", f]
    cmd.append(prompt)

    env = {**os.environ, "RUST_LOG": "trace"}
    proc = subprocess.run(cmd, stdin=subprocess.DEVNULL, capture_output=True,
                          text=True, timeout=timeout, env=env)
    events = [json.loads(line) for line in proc.stdout.splitlines() if line.startswith("{")]
    items = [e["item"] for e in events if e.get("type") == "item.completed"]
    return CodexRun(events=events, tools_seen=tools_in_trace(proc.stderr),
                    returncode=proc.returncode, stderr_tail=proc.stderr[-2000:], items=items)


# Tool lists the model sees: the `"tools":[...]` of each request, and `tool_search`
# results (logged as `output=[...]`), which load deferred tools such as MCP namespaces.
_TOOL_LIST_MARKERS = ('"tools":', " output=")


def tools_in_trace(trace: str) -> set[str]:
    """Union of tool names from every model-visible tool list found in the trace log."""
    dec = json.JSONDecoder()
    names: set[str] = set()
    i = 0
    while True:
        hits = [(p, m) for m in _TOOL_LIST_MARKERS if (p := trace.find(m + "[", i)) >= 0]
        if not hits:
            return names
        pos, marker = min(hits)
        try:
            arr, end = dec.raw_decode(trace, pos + len(marker))
        except json.JSONDecodeError:
            i = pos + 1
            continue
        if marker == '"tools":' or _looks_like_tools(arr):
            names |= _flatten(arr)
            i = end  # skip nested namespace arrays; _flatten already covered them
        else:
            i = pos + 1


def _looks_like_tools(arr: list) -> bool:
    return bool(arr) and all(isinstance(t, dict) and t.get("type") in ("function", "namespace", "custom")
                             for t in arr)


def _flatten(tools: list[dict]) -> set[str]:
    out: set[str] = set()
    for t in tools:
        if not isinstance(t, dict):
            continue
        if t.get("tools"):  # namespace: report children with the namespace prefix
            out |= {f"{t['name']}.{n}" for n in _flatten(t["tools"])}
        else:  # hosted tools (e.g. tool_search) have a type but no name; never drop an entry
            out.add(t.get("name") or t.get("type") or json.dumps(t)[:80])
    return out
