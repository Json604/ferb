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
- supervisor-mode runs (the planner, the answer writer, distillation: no tools) use low reasoning effort and drop
  Codex's environment / permissions / apps / collaboration instruction blocks; tasks (worker mode) are unchanged;
- every run uses the pinned app-server in an empty configuration home with an ephemeral thread (codex_stream);
  final-answer deltas are gated by the observed tool catalog and the complete trace is checked before final output."""

import asyncio
import json
import os
import platform
import tempfile
from pathlib import Path
from typing import Any, AsyncIterator, Callable, Literal

from ferb.cli.mcp import mcp_name
from ferb.core.errors import InputTooLarge, ModelLimitsUnknown, Unsupported
from ferb.core.types import ModelLimits, ModelRequest, RunHandle, Capabilities, RuntimeEvent, Task
from ferb.sanitize.redact import Redactor

CODEX_VERSION = "0.157.1"
MODEL = "gpt-5.6-sol"                 # owner decision 2026-10-14: GPT-5.5 retires from Codex for ChatGPT sign-ins
# L28: Codex refuses a turn whose text input items together hold more characters (Unicode scalar values) than this,
# before any model sees it ("Input exceeds the maximum length of 1048576 characters."). It counts per turn/start (and
# per turn/steer), the instructions FERB gives thread/start apart.
MAX_INPUT_CHARS = 1 << 20
INPUT_CHARS_SOURCE = (
    "openai/codex tag rust-v0.157.1: MAX_USER_INPUT_TEXT_CHARS = 1 << 20 (codex-rs/protocol/src/user_input.rs:10), "
    "compared with the sum of a turn's text input items' chars().count() by turn/start and turn/steer "
    "(codex-rs/app-server/src/request_processors/turn_processor.rs:502-519, 551-571, 1045; UserInput::text_char_count, "
    "codex-rs/app-server-protocol/src/protocol/v2/turn.rs:517-527); codex exec's prompt goes through the same "
    "turn/start (codex-rs/exec/src/lib.rs:908, 1152); thread/start's baseInstructions/developerInstructions are not "
    "counted")
# Codex's npm platform package per Mac architecture (package-lock.json pins both): its folder and its binary's target
# triple. scripts/app-parts.sh ferb_arch_names names the same binary for FERB.app.
_CODEX_PACKAGES = {"arm64": ("codex-darwin-arm64", "aarch64-apple-darwin"),
                   "x86_64": ("codex-darwin-x64", "x86_64-apple-darwin")}


def native_bin(machine: str | None = None) -> Path:
    """The native Codex binary inside a node folder, for `machine` or this process's architecture (arm64, or x86_64
    on an Intel Mac and in an Intel FERB.app under Rosetta). Any other name is no Mac FERB runs on: it gets the arm64
    path, which is then simply not installed."""
    package, triple = _CODEX_PACKAGES.get(machine or platform.machine(), _CODEX_PACKAGES["arm64"])
    return Path(f"node_modules/@openai/{package}/vendor/{triple}/bin/codex")


_NATIVE_BIN = native_bin()
DISABLED = ["shell_tool", "unified_exec", "shell_snapshot", "multi_agent", "multi_agent_v2", "apps", "plugins",
            "remote_plugin", "tool_suggest", "skill_search", "hooks", "browser_use", "browser_use_external",
            "in_app_browser", "computer_use", "image_generation", "view_image", "goals", "sleep_tool",
            "code_mode_host", "skill_mcp_dependency_install"]
# Native tools Codex always offers: inert (MCP resource plumbing over FERB's server only; request_user_input fails
# closed in exec and client actions are rejected in app-server; apply_patch cannot write in the read-only sandbox).
SUPERVISOR_SETTINGS = ["-c", 'model_reasoning_effort="low"', "-c", "include_environment_context=false",
                       "-c", "include_permissions_instructions=false", "-c", "include_apps_instructions=false",
                       "-c", "include_collaboration_mode_instructions=false"]
# P20: no skills message for any role. Otherwise Codex adds a `<skills_instructions>` developer message right after
# FERB's instructions, naming the run's random temporary CODEX_HOME, so nothing after FERB's system prompt repeats
# across runs: ~611 tokens and 1-3 s per run (measured 2026-10-25, Codex 0.157.1). FERB loads no Codex skills.
SKILLS_OFF = ["-c", "skills.include_instructions=false", "-c", "skills.bundled.enabled=false"]
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


def _bundled_path(ferb_home: Path) -> Path:
    return ferb_home / "run" / f"codex-bundled-{CODEX_VERSION}.json"


def bundled_catalog(ferb_home: Path) -> Path | None:
    """The model catalog shipped with the pinned Codex (`codex debug models --bundled`), dumped once per Codex version
    into FERB's run folder. Codex itself prints it: offline, with an empty Codex home (no login, no owner settings).
    None when the pinned binary is missing or prints no catalog. It runs a process, so only the daemon's start (in a
    worker thread, before serving) and `ferb setup --codex-login` call it; a limits read never does (final review
    M8)."""
    out = _bundled_path(ferb_home)
    if out.exists():
        return out
    try:
        from ferb.device.install import repo_root
        binary = codex_binary(ferb_home, repo_root())
    except Unsupported:
        return None
    import subprocess
    with tempfile.TemporaryDirectory(prefix="ferb-codex-catalog-") as empty:
        env = {k: v for k, v in os.environ.items() if not k.startswith(("OPENAI_", "CODEX_"))}
        env.update(HOME=empty, CODEX_HOME=empty)
        try:
            done = subprocess.run([str(binary), "debug", "models", "--bundled"], capture_output=True, env=env,
                                  cwd=empty, check=False)
        except OSError:
            return None
    try:
        data = json.loads(done.stdout)
    except ValueError:
        return None
    if done.returncode != 0 or not isinstance(data, dict) or not isinstance(data.get("models"), list):
        return None
    out.parent.mkdir(parents=True, exist_ok=True)
    tmp = out.with_suffix(".tmp")
    tmp.write_text(json.dumps(data))
    tmp.chmod(0o600)
    tmp.replace(out)
    return out


def _catalog(ferb_home: Path) -> tuple[Any, str] | None:
    """Codex's model catalog for FERB's runs, and where it came from: the catalog Codex last fetched for FERB's own
    login (`codex-home/models_cache.json`), else the one shipped with the pinned binary, as `bundled_catalog` dumped
    it. Files only: reading it never runs Codex (M8). Never the owner's ~/.codex."""
    home = codex_home(ferb_home)
    sources = ([(home / "models_cache.json", "models_cache.json")] if home is not None else []) + [
        (_bundled_path(ferb_home), "the catalog shipped with the pinned Codex (debug models --bundled)")]
    for path, label in sources:
        try:
            return json.loads(path.read_text()), label
        except (OSError, ValueError):
            continue
    return None


def _models(data: Any) -> list[Any] | None:
    models = data.get("models", data) if isinstance(data, dict) else data
    return models if isinstance(models, list) else None


def locked_catalog(ferb_home: Path) -> Path | None:
    """FERB's model catalog for Codex: Codex's own catalog (`_catalog`) with CATALOG_LOCKDOWN applied to every model,
    so Codex runs with exactly the catalog FERB reads each model's window from. None when Codex has none (then nothing
    changes; a model that brings extra tools is still caught and stopped)."""
    found = _catalog(ferb_home)
    if found is None:
        return None
    data = found[0]
    models = _models(data)
    if models is None:
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


def limits_for(ferb_home: Path, model: str) -> ModelLimits:
    """L28: the input window Codex runs this model with, as Codex reports it: its catalog's `context_window` times its
    `effective_context_window_percent` (272,000 x 95 % = 258,400 for gpt-5.6-sol in Codex 0.157.1's bundled catalog,
    the figure a Codex session reports as `model_context_window`:
    https://community.openai.com/t/why-does-codex-report-a-258-400-token-context-window-for-gpt-5-6-sol/1394346).
    Codex's docs don't say whether that window also holds the reply (https://learn.chatgpt.com/docs/config-file/
    config-reference, `model_context_window`: "Context window tokens available to the active model"), so FERB uses it
    as the input room and never subtracts the reply limit a second time. The reply limit itself is OpenAI's
    documented max output for the model (openai_api.LIMITS). Anything missing is an explicit error, never a guess."""
    from ferb.runtimes.openai_api import LIMITS, LIMITS_SOURCE
    found = _catalog(ferb_home)
    if found is None:
        raise ModelLimitsUnknown("Codex's model catalog isn't available: the pinned Codex didn't print one; run "
                                 "`ferb setup --update`")
    data, label = found
    entry = next((m for m in _models(data) or [] if isinstance(m, dict) and m.get("slug") == model), None)
    window = entry.get("context_window") if entry else None
    percent = entry.get("effective_context_window_percent") if entry else None
    if not isinstance(window, int) or window <= 0 or not isinstance(percent, (int, float)) or not 0 < percent <= 100:
        raise ModelLimitsUnknown(f"Codex's model catalog ({label}) reports no context window for {model!r}; choose a "
                                 "model Codex lists in config.toml [models] codex")
    if model not in LIMITS:
        raise ModelLimitsUnknown(f"OpenAI documents no max output FERB knows for {model!r}; choose one of "
                                 f"{sorted(LIMITS)} in config.toml [models] codex")
    return ModelLimits(model, int(window * percent / 100), LIMITS[model][1],
                       f"Codex {CODEX_VERSION} model catalog ({label}): context_window {window} x "
                       f"effective_context_window_percent {percent}; max output: {LIMITS_SOURCE}; max input "
                       f"characters: {INPUT_CHARS_SOURCE}",
                       window_includes_output=False, max_input_chars=MAX_INPUT_CHARS)


def check_input(text: str) -> None:
    """The text Codex will count against MAX_INPUT_CHARS, refused here by name rather than sent to be rejected
    (context assembly already fits it; this is the transport's own guard)."""
    if len(text) > MAX_INPUT_CHARS:
        raise InputTooLarge(len(text), MAX_INPUT_CHARS, "this Codex input")


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

    def __init__(self, binary: Path, mode: Literal["supervisor", "worker"], ferb_home: Path,
                 shim_cmd: list[str] | None = None, tool_names: list[str] | None = None,
                 output_schema: dict[str, Any] | None = None, model: str = MODEL, timeout_s: float | None = 900,
                 on_abandoned: Callable[[str, str], None] | None = None,
                 redact: Callable[[Any], Any] | None = None, cache_role: str | None = None,
                 cache_scope: str = "", cache_allowed: Callable[[Task], bool] | None = None,
                 on_cache_fallback: Callable[[str, str], None] | None = None, effort: str | None = None) -> None:
        self.binary, self.mode, self.ferb_home = binary, mode, ferb_home
        # L39: a worker-mode run's reasoning effort when the caller asks for one (a checker of a step that changed
        # files: "high"); None keeps the model's default. Supervisor runs stay low (SUPERVISOR_SETTINGS)
        self.effort = effort
        # Codex's own error text passes through this before it is raised or reported (rule 14): the daemon's redactor,
        # which knows the values the owner stored, else the secret pattern classes
        self.redact = redact or Redactor()
        self.shim_cmd, self.output_schema, self.model, self.timeout_s = shim_cmd, output_schema, model, timeout_s
        # Told (task id, "<thread>/<call id>") when Codex gives up a FERB tool call on its own: it sends the shim no
        # cancellation (measured, 0.157.1), so the daemon would otherwise keep waiting for a caller that is gone
        self.on_abandoned = on_abandoned
        # P20 (owner, 2026-10-25; `[runtimes] codex_planner_cache`): the role whose app-server turns resume FERB's
        # template thread under a cache key fixed per `cache_scope` (this Mac), role and model, its stable context in the
        # developer role (codex_stream.start); None for today's ephemeral thread. `cache_allowed` decides per turn
        # (the daemon: only an owner's chat turn); `on_cache_fallback` is told (task id, why) when a turn that would
        # resume takes today's path instead.
        self.cache_role, self.cache_scope = cache_role, cache_scope
        self.cache_allowed, self.on_cache_fallback = cache_allowed, on_cache_fallback
        self.allowed = BASE_TOOLS | {f"mcp__ferb.{mcp_name(n)}" for n in (tool_names or [])}

    @property
    def limits(self) -> ModelLimits:
        return limits_for(self.ferb_home, self.model)

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

    async def start(self, task: Task, request: ModelRequest, run_token: str) -> RunHandle:
        """Every run is an app-server run (codex_stream): the instructions go apart in thread/start, so Codex counts
        the context alone."""
        check_input(request.context)
        await check_version(self.binary)
        from ferb.runtimes.codex_stream import start
        return await start(self, task, request, run_token)

    async def events(self, run: RunHandle) -> AsyncIterator[RuntimeEvent]:
        from ferb.runtimes.codex_stream import events
        async for event in events(self, run):
            yield event

    async def continue_(self, run: RunHandle, message: str) -> None:
        if self.mode == "worker":
            from ferb.runtimes.codex_stream import steer
            await steer(run, message)
            return
        raise Unsupported("Only active Codex worker turns accept steering")

    async def cancel(self, run: RunHandle) -> None:
        from ferb.runtimes.codex_stream import cancel
        await cancel(run)
