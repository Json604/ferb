"""Owns the pinned Codex CLI adapter and tool containment. Must not select task targets, authorize actions or expose tools outside the registered harness.

CodexRuntime: isolated app-server for steerable workers and typed supervisor output streaming.

Tool containment (measured in spikes 1 and 21): pinned Codex 0.157.1 and MODEL below, run without code mode / sub-agents via
FERB's own model catalog, user config and rules ignored,
every disableable native tool off, read-only sandbox (makes the non-removable apply_patch inert),
approval_policy=never. Worker mode's only tool source is `ferb mcp` (pre-approved); supervisor mode has none.
Codex runs with its own empty HOME to keep the owner's personal skills out. Ephemeral: nothing is written into the owner's ~/.codex sessions. The model-visible tool list is scanned from the
request trace on every run; anything outside the allowlist kills the run.

Speed (owner decisions, 2026-10-01; all within the owner's Codex subscription):
- FERB's own CODEX_HOME (`~/.ferb/codex-home`, from `ferb setup --codex-login`) so the owner's personal skills and
  settings never load; HOME is always `~/.ferb/codex-user`. There is no fallback to `~/.codex`: without that login
  the daemon refuses to build a Codex runtime and says to run `ferb setup --codex-login`;
- supervisor-mode runs (the planner, the answer writer, distillation: no tools) use low reasoning effort and replace
  Codex's built-in instructions with FERB's own (model_instructions_file) — about half the input tokens — and drop
  Codex's environment / permissions / apps / collaboration instruction blocks; tasks (worker mode) are unchanged;
- typed supervisor runs use the pinned app-server in an empty configuration home with an ephemeral thread;
  final-answer deltas are gated by the observed tool catalog and the complete trace is checked before final output;
- untyped supervisor exec runs end at `turn.completed`; app-server runs drain their full tool trace before final output."""

import asyncio
import json
import os
import secrets
import tempfile
from dataclasses import asdict
from pathlib import Path
from typing import Any, AsyncIterator, Literal

from ferb.cli.mcp import mcp_name
from ferb.core.errors import Unsupported
from ferb.core.types import ModelRequest, RunHandle, Capabilities, RuntimeEvent, Task
from ferb.runtimes.usage import from_codex
from ferb.runtimes.tooltrace import tools_in

CODEX_VERSION = "0.157.1"
MODEL = "gpt-5.5"
_NATIVE_BIN = Path("node_modules/@openai/codex-darwin-arm64/vendor/aarch64-apple-darwin/bin/codex")
DISABLED = ["shell_tool", "unified_exec", "shell_snapshot", "multi_agent", "multi_agent_v2", "apps", "plugins",
            "remote_plugin", "tool_suggest", "skill_search", "hooks", "browser_use", "browser_use_external",
            "in_app_browser", "computer_use", "image_generation", "view_image", "goals", "sleep_tool",
            "code_mode_host", "skill_mcp_dependency_install"]
# Native tools Codex always offers: inert (MCP resource plumbing over FERB's server only; request_user_input fails
# closed in exec and client actions are rejected in app-server; apply_patch cannot write in the read-only sandbox).
SUPERVISOR_SETTINGS = ["-c", 'model_reasoning_effort="low"', "-c", "include_environment_context=false",
                       "-c", "include_permissions_instructions=false", "-c", "include_apps_instructions=false",
                       "-c", "include_collaboration_mode_instructions=false"]
# Codex finds the owner's personal skills through HOME (~/.agents/skills), even with skill_search disabled and its own
# CODEX_HOME. An empty, owner-only HOME of its own keeps them out (verified 2026-10-07: no skill loads, auth unaffected,
# because auth lives in CODEX_HOME). FERB's MCP shim is unaffected: it gets FERB_HOME explicitly.
CODEX_USER = "codex-user"
BASE_TOOLS = frozenset({"list_mcp_resources", "list_mcp_resource_templates", "read_mcp_resource", "tool_search",
                        "request_user_input", "apply_patch"})


def codex_home(ferb_home: Path) -> Path | None:
    """FERB's own Codex home, once the owner has logged in there (`CODEX_HOME=~/.ferb/codex-home codex login`)."""
    home = ferb_home / "codex-home"
    return home if (home / "auth.json").exists() else None


# A model's catalog entry can switch on tools no feature flag turns off (spike 21, owner's move to GPT-6): code mode
# (`functions.exec` runs code), multi-agent v2 (`collaboration.*` sub-agents outside FERB's gateway) and experimental
# tools (ask the user, wait). FERB hands Codex its own copy of the catalog with these neutralized, for every model.
# supports_search_tool False keeps FERB's MCP tools directly visible instead of deferred behind tool_search (Codex
# 0.157.1, verified 2026-10-07); the side effects that brings (apps, plugins, sub-agents) stay in DISABLED.
CATALOG_LOCKDOWN: dict[str, Any] = {"tool_mode": None, "multi_agent_version": None,
                                    "multi_agent_reasoning_effort": None, "experimental_supported_tools": [],
                                    "supports_search_tool": False}


def locked_catalog(ferb_home: Path) -> Path | None:
    """FERB's model catalog for Codex: Codex's own cached catalog with CATALOG_LOCKDOWN applied to every model. None
    when no cached catalog exists (then nothing changes; a model that brings extra tools is still caught and stopped)."""
    home = codex_home(ferb_home) or Path(os.environ.get("CODEX_HOME") or Path.home() / ".codex")
    src = home / "models_cache.json"
    try:
        data = json.loads(src.read_text())
    except (OSError, ValueError):
        return None
    models = data.get("models", data) if isinstance(data, dict) else data
    if not isinstance(models, list):
        return None
    for m in models:
        if isinstance(m, dict):
            m.update(CATALOG_LOCKDOWN)
    out = ferb_home / "run" / "codex-models.json"
    out.parent.mkdir(parents=True, exist_ok=True)
    tmp = out.with_suffix(".tmp")
    tmp.write_text(json.dumps(data))
    tmp.replace(out)
    return out


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
    # image_input True: verified live 2026-10-08 — a PNG returned by an MCP tool in FERB's shim format reached the model
    capabilities = Capabilities("direct", "prefix", False, "native", True, True, True, True, "subscription")

    def __init__(self, binary: Path, mode: Literal["supervisor", "worker"], workdir: Path, ferb_home: Path,
                 shim_cmd: list[str] | None = None, tool_names: list[str] | None = None,
                 output_schema: dict[str, Any] | None = None, model: str = MODEL, timeout_s: float | None = 900) -> None:
        self.binary, self.mode, self.workdir, self.ferb_home = binary, mode, workdir, ferb_home
        self.shim_cmd, self.output_schema, self.model, self.timeout_s = shim_cmd, output_schema, model, timeout_s
        self.allowed = BASE_TOOLS | {f"mcp__ferb.{mcp_name(n)}" for n in (tool_names or [])}

    def _mcp_settings(self, token_file: Path | None) -> list[str]:
        if not self.shim_cmd:
            raise ValueError("worker mode needs the ferb mcp shim")
        if token_file is None:
            raise ValueError("worker mode needs a run token file")
        # the token itself never goes on the command line (visible in `ps` to every local process): the shim
        # reads it from a file only the owner can read (audit 2026-10-02)
        env = "{" + ",".join(f'{k}={json.dumps(v)}' for k, v in {"FERB_RUN_TOKEN_FILE": str(token_file),
                                        "FERB_HOME": str(self.ferb_home)}.items()) + "}"
        return ["-c", f"mcp_servers.ferb.command={json.dumps(self.shim_cmd[0])}",
                "-c", f"mcp_servers.ferb.args={json.dumps(self.shim_cmd[1:])}",
                "-c", f"mcp_servers.ferb.env={env}",
                "-c", 'mcp_servers.ferb.default_tools_approval_mode="approve"']

    def _cmd(self, schema_file: Path | None, token_file: Path | None, instructions: Path | None = None) -> list[str]:
        cmd = [str(self.binary), "exec", "--json", "--ephemeral", "--skip-git-repo-check", "--ignore-user-config",
               "--ignore-rules", "--strict-config", "-s", "read-only", "-C", str(self.workdir), "-m", self.model,
               "-c", 'approval_policy="never"', "-c", 'web_search="disabled"']
        for f in DISABLED:
            cmd += ["--disable", f]
        if (catalog := locked_catalog(self.ferb_home)) is not None:      # no code mode / sub-agents for any model
            cmd += ["-c", f"model_catalog_json={json.dumps(str(catalog))}"]
        if self.mode == "worker":
            cmd += self._mcp_settings(token_file)
        if self.mode == "supervisor":
            cmd += SUPERVISOR_SETTINGS
            if instructions is not None:                       # FERB's instructions replace Codex's built-in ones
                cmd += ["-c", f"model_instructions_file={json.dumps(str(instructions))}"]
        if schema_file:
            cmd += ["--output-schema", str(schema_file)]
        return cmd

    async def start(self, task: Task, request: ModelRequest, run_token: str) -> RunHandle:
        system, context = request.system, request.context
        await check_version(self.binary)
        if self.mode == "worker" or self.output_schema is not None:
            from ferb.runtimes.codex_stream import start
            return await start(self, task, request, run_token)
        self.workdir.mkdir(parents=True, exist_ok=True)
        paths: list[Path] = []
        proc = None
        try:
            schema_file = None
            if self.output_schema:
                fd, name = tempfile.mkstemp(prefix="ferb-schema-", suffix=".json")
                paths.append(Path(name))
                with os.fdopen(fd, "w") as f:
                    json.dump(self.output_schema, f)
                schema_file = Path(name)
            token_file = None
            instructions = None
            if self.mode == "supervisor" and system:               # a stable prefix: FERB's instructions, then the context
                fd, iname = tempfile.mkstemp(prefix="ferb-instructions-", suffix=".md")
                paths.append(Path(iname))
                with os.fdopen(fd, "w") as f:
                    f.write(system)
                instructions = Path(iname)
            env = {**os.environ, "RUST_LOG": "trace"}
            user_home = self.ferb_home / CODEX_USER
            user_home.mkdir(mode=0o700, parents=True, exist_ok=True)
            env["HOME"] = str(user_home)
            home = codex_home(self.ferb_home)
            if home is not None:
                env["CODEX_HOME"] = str(home)                       # the owner's personal Codex setup never loads
            proc = await asyncio.create_subprocess_exec(
                *self._cmd(schema_file, token_file, instructions), "-", stdin=asyncio.subprocess.PIPE,
                stdout=asyncio.subprocess.PIPE, stderr=asyncio.subprocess.PIPE, env=env, limit=64 * 1024 * 1024)
            assert proc.stdin is not None
            proc.stdin.write((context if instructions is not None else f"{system}\n\n{context}").encode())
            await proc.stdin.drain()
            proc.stdin.close()
            return RunHandle("run-" + secrets.token_hex(6), self.name, task.id,
                             {"proc": proc, "schema_file": schema_file, "instructions": instructions,
                              "token_file": token_file, "seen": set(), "violation": None})
        except BaseException:
            if proc is not None and proc.returncode is None:
                try:
                    proc.kill()
                    await proc.wait()
                except ProcessLookupError:
                    pass
            for path in paths:
                path.unlink(missing_ok=True)
            raise

    async def events(self, run: RunHandle) -> AsyncIterator[RuntimeEvent]:
        if run.state.get("app_server"):
            from ferb.runtimes.codex_stream import events
            async for event in events(self, run):
                yield event
            return
        proc: asyncio.subprocess.Process = run.state["proc"]
        assert proc.stdout is not None and proc.stderr is not None
        watcher = asyncio.create_task(self._watch_trace(run, proc))
        last_message = ""
        completed = False
        turn_completed = False
        try:
            async with asyncio.timeout(self.timeout_s):
                async for raw in proc.stdout:
                    if run.state["violation"]:
                        break
                    line = raw.decode("utf-8").strip()
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
                        turn_completed = True
                        yield RuntimeEvent("usage", asdict(from_codex(self.model, dict(ev.get("usage", {})))))
                        if self.mode == "supervisor":            # no FERB tools: the answer is in, don't wait for
                            completed = True                     # Codex to exit (workers wait, so the tool-list
                            break                                # check sees the whole trace)
                    elif ev.get("type") in ("error", "turn.failed"):
                        yield RuntimeEvent("error", {"message": str(ev.get("message") or ev.get("error"))})
            if completed and not run.state["violation"]:
                asyncio.get_running_loop().create_task(self._reap(proc, watcher))
            else:
                await proc.wait()
                await watcher
        except TimeoutError:
            await self.cancel(run)
            yield RuntimeEvent("error", {"message": f"timed out after {self.timeout_s:.0f}s"})
            return
        finally:
            for key in ("schema_file", "instructions", "token_file"):
                if run.state.get(key):
                    Path(run.state[key]).unlink(missing_ok=True)
        if run.state["violation"]:
            yield RuntimeEvent("violation", {"unexpected_tools": sorted(run.state["violation"])})
            return
        if not turn_completed or (not completed and proc.returncode != 0):
            yield RuntimeEvent("error", {"message": f"Codex exited with code {proc.returncode}; "
                                          f"completed_turn={turn_completed}. The last message is only a partial report."})
            return
        yield RuntimeEvent("final", {"text": last_message, "returncode": proc.returncode,
                                     "tools_seen": sorted(run.state["seen"])})

    async def _reap(self, proc: asyncio.subprocess.Process, watcher: "asyncio.Task[None]") -> None:
        """After the answer: let Codex exit on its own (bounded), then stop watching its trace."""
        try:
            await asyncio.wait_for(proc.wait(), 10)
        except TimeoutError:
            proc.kill()
        watcher.cancel()

    async def _watch_trace(self, run: RunHandle, proc: asyncio.subprocess.Process) -> None:
        """Scan the request trace line by line; kill the run on any tool outside the allowlist."""
        assert proc.stderr is not None
        async for raw in proc.stderr:
            if b'"tools":[' not in raw and b" output=[" not in raw:
                continue
            # GPT-6 groups plain tools under a "functions" namespace: functions.apply_patch is apply_patch (and
            # functions.exec stays exec, which isn't allowed)
            seen = {n.removeprefix("functions.") for n in tools_in(raw.decode(errors="replace"))}
            run.state["seen"] |= seen
            extra = seen - self.allowed
            if extra:
                run.state["violation"] = extra
                await self.cancel(run)
                return

    async def continue_(self, run: RunHandle, message: str) -> None:
        if self.mode == "worker" and run.state.get("app_server"):
            from ferb.runtimes.codex_stream import steer
            await steer(run, message)
            return
        raise Unsupported("Only active Codex worker turns accept steering")

    async def cancel(self, run: RunHandle) -> None:
        if run.state.get("app_server"):
            from ferb.runtimes.codex_stream import cancel
            await cancel(run)
            return
        proc: asyncio.subprocess.Process = run.state["proc"]
        if proc.returncode is None:
            proc.kill()
            await proc.wait()
