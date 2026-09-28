"""CodexRuntime (section 5 AgentRuntime) over `codex exec --json` — supervisor and worker modes.

Lockdown (spike 1, section 7): pinned Codex 0.157.1 and gpt-5.5 (non-code-mode), user config and rules ignored,
every disableable native tool off, read-only sandbox (makes the non-removable apply_patch inert),
approval_policy=never. Worker mode's only tool source is `ferb mcp` (pre-approved); supervisor mode has none.
Ephemeral: nothing is written into the owner's ~/.codex sessions. The model-visible tool list is scanned from the
request trace on every run; anything outside the allowlist kills the run.
"""

import asyncio
import json
import os
import secrets
import tempfile
from pathlib import Path
from typing import Any, AsyncIterator, Literal

from ferb.cli.mcp import mcp_name
from ferb.core.errors import Unsupported
from ferb.core.types import RunHandle, RuntimeEvent, RuntimeFeatures, Task
from ferb.runtimes.tooltrace import tools_in

CODEX_VERSION = "0.157.1"
MODEL = "gpt-5.5"
_NATIVE_BIN = Path("node_modules/@openai/codex-darwin-arm64/vendor/aarch64-apple-darwin/bin/codex")
DISABLED = ["shell_tool", "unified_exec", "shell_snapshot", "multi_agent", "multi_agent_v2", "apps", "plugins",
            "remote_plugin", "tool_suggest", "skill_search", "hooks", "browser_use", "browser_use_external",
            "in_app_browser", "computer_use", "image_generation", "view_image", "goals", "sleep_tool",
            "code_mode_host"]
# Native tools Codex always offers: inert (MCP resource plumbing over FERB's server only; request_user_input fails
# closed in exec; apply_patch can't write in the read-only sandbox).
BASE_TOOLS = frozenset({"list_mcp_resources", "list_mcp_resource_templates", "read_mcp_resource", "tool_search",
                        "request_user_input", "apply_patch"})


def codex_binary(ferb_home: Path, repo: Path | None = None) -> Path:
    """The pinned native binary: ~/.ferb/node (installed daemon) or the repo's node_modules (development).
    The native binary, not the node wrapper: launchd has no node on PATH."""
    for root in (ferb_home / "node", repo) if repo else (ferb_home / "node",):
        p = root / _NATIVE_BIN
        if p.exists():
            return p
    raise Unsupported("pinned Codex not installed: run `ferb setup --update` (or `npm install` in the repo)")


async def check_version(binary: Path) -> None:
    proc = await asyncio.create_subprocess_exec(str(binary), "--version", stdout=asyncio.subprocess.PIPE)
    out, _ = await proc.communicate()
    if out.decode().split()[-1:] != [CODEX_VERSION]:
        raise Unsupported(f"Codex {out.decode().strip()} is not the pinned {CODEX_VERSION}; "
                          "rerun spike 1's tests before changing the pin")


class CodexRuntime:
    name = "codex"
    features = RuntimeFeatures(live_messaging=False, continuation=False, per_call_hooks=False,
                               usage_visibility=True, native_tools_contained=True)

    def __init__(self, binary: Path, mode: Literal["supervisor", "worker"], workdir: Path, ferb_home: Path,
                 shim_cmd: list[str] | None = None, tool_names: list[str] | None = None,
                 output_schema: dict[str, Any] | None = None, model: str = MODEL, timeout_s: float = 900) -> None:
        self.binary, self.mode, self.workdir, self.ferb_home = binary, mode, workdir, ferb_home
        self.shim_cmd, self.output_schema, self.model, self.timeout_s = shim_cmd, output_schema, model, timeout_s
        self.allowed = BASE_TOOLS | {f"mcp__ferb.{mcp_name(n)}" for n in (tool_names or [])}

    def _cmd(self, schema_file: Path | None, run_token: str) -> list[str]:
        cmd = [str(self.binary), "exec", "--json", "--ephemeral", "--skip-git-repo-check", "--ignore-user-config",
               "--ignore-rules", "--strict-config", "-s", "read-only", "-C", str(self.workdir), "-m", self.model,
               "-c", 'approval_policy="never"', "-c", 'web_search="disabled"']
        for f in DISABLED:
            cmd += ["--disable", f]
        if self.mode == "worker":
            if not self.shim_cmd:
                raise ValueError("worker mode needs the ferb mcp shim")
            env = "{" + ",".join(f'{k}="{v}"' for k, v in {"FERB_RUN_TOKEN": run_token,
                                                            "FERB_HOME": str(self.ferb_home)}.items()) + "}"
            cmd += ["-c", f"mcp_servers.ferb.command={json.dumps(self.shim_cmd[0])}",
                    "-c", f"mcp_servers.ferb.args={json.dumps(self.shim_cmd[1:])}",
                    "-c", f"mcp_servers.ferb.env={env}",
                    "-c", 'mcp_servers.ferb.default_tools_approval_mode="approve"']
        if schema_file:
            cmd += ["--output-schema", str(schema_file)]
        return cmd

    async def start(self, task: Task, system: str, context: str, run_token: str) -> RunHandle:
        await check_version(self.binary)
        self.workdir.mkdir(parents=True, exist_ok=True)
        schema_file = None
        if self.output_schema:
            fd, name = tempfile.mkstemp(prefix="ferb-schema-", suffix=".json")
            with os.fdopen(fd, "w") as f:
                json.dump(self.output_schema, f)
            schema_file = Path(name)
        proc = await asyncio.create_subprocess_exec(
            *self._cmd(schema_file, run_token), "-", stdin=asyncio.subprocess.PIPE, stdout=asyncio.subprocess.PIPE,
            stderr=asyncio.subprocess.PIPE, env={**os.environ, "RUST_LOG": "trace"}, limit=64 * 1024 * 1024)
        assert proc.stdin is not None
        proc.stdin.write(f"{system}\n\n{context}".encode())
        await proc.stdin.drain()
        proc.stdin.close()
        return RunHandle("run-" + secrets.token_hex(6), self.name, task.id,
                         {"proc": proc, "schema_file": schema_file, "seen": set(), "violation": None})

    async def events(self, run: RunHandle) -> AsyncIterator[RuntimeEvent]:
        proc: asyncio.subprocess.Process = run.state["proc"]
        assert proc.stdout is not None and proc.stderr is not None
        watcher = asyncio.create_task(self._watch_trace(run, proc))
        last_message = ""
        try:
            async with asyncio.timeout(self.timeout_s):
                async for raw in proc.stdout:
                    if run.state["violation"]:
                        break
                    line = raw.decode(errors="replace").strip()
                    if not line.startswith("{"):
                        continue
                    ev = json.loads(line)
                    item = ev.get("item", {})
                    if ev.get("type") == "item.completed" and item.get("type") == "agent_message":
                        last_message = item.get("text", "")
                        yield RuntimeEvent("message", {"text": last_message})
                    elif ev.get("type") == "item.completed" and item.get("type") == "mcp_tool_call":
                        yield RuntimeEvent("tool_call", {"tool": item.get("tool"), "status": item.get("status"),
                                                         "arguments": item.get("arguments")})
                    elif ev.get("type") == "turn.completed":
                        yield RuntimeEvent("usage", dict(ev.get("usage", {})))
                    elif ev.get("type") in ("error", "turn.failed"):
                        yield RuntimeEvent("error", {"message": str(ev.get("message") or ev.get("error"))})
            await proc.wait()
            await watcher
        except TimeoutError:
            await self.cancel(run)
            yield RuntimeEvent("error", {"message": f"timed out after {self.timeout_s:.0f}s"})
            return
        finally:
            if run.state.get("schema_file"):
                Path(run.state["schema_file"]).unlink(missing_ok=True)
        if run.state["violation"]:
            yield RuntimeEvent("violation", {"unexpected_tools": sorted(run.state["violation"])})
            return
        yield RuntimeEvent("final", {"text": last_message, "returncode": proc.returncode,
                                     "tools_seen": sorted(run.state["seen"])})

    async def _watch_trace(self, run: RunHandle, proc: asyncio.subprocess.Process) -> None:
        """Scan the request trace line by line; kill the run on any tool outside the allowlist."""
        assert proc.stderr is not None
        async for raw in proc.stderr:
            if b'"tools":[' not in raw and b" output=[" not in raw:
                continue
            seen = tools_in(raw.decode(errors="replace"))
            run.state["seen"] |= seen
            extra = seen - self.allowed
            if extra:
                run.state["violation"] = extra
                await self.cancel(run)
                return

    async def continue_(self, run: RunHandle, message: str) -> None:
        raise Unsupported("codex exec runs are one-shot; the supervisor re-sends the chat history each turn")

    async def cancel(self, run: RunHandle) -> None:
        proc: asyncio.subprocess.Process = run.state["proc"]
        if proc.returncode is None:
            proc.kill()
            await proc.wait()
