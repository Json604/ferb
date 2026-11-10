"""Owns the one table of model runtimes FERB can run (D32): each runtime's name, how it is built, its model's documented
limits, how it bills, and how to tell it can run here. Must not choose a runtime for a task.

Every list of runtime names (the daemon's builder, the planner's worker types, `ferb runtimes`, the app's API keys, the
Keychain accounts, the doctor) reads this table. A runtime is available when its model is set (the owner's
`[models] <name>` or the runtime's default) and FERB's own sign-in (Codex, Claude) or the provider's
key (the direct APIs) is present. Prices are per model (ferb/runtimes/prices.py, shared by Codex and the OpenAI API) and
reach every runtime's usage through ferb/runtimes/usage.py."""

from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Awaitable, Callable, Literal

from ferb.core.errors import Unsupported
from ferb.core.types import ModelLimits

ROLES = ("orchestrator", "worker", "checker")
# Owner, 2026-11-04 (ORCH-528, replacing ORCH-311's 90 s): the planner's call has no overall limit; like every call it
# is stopped only by the silence guard (guards.STALL_S, 180 s with no event from the runtime, ferb/orchestrator/
# worker.py). Every other call that isn't a worker's (a dispatch, a question, a summary, distillation) also has one
# overall limit beside that guard; a worker has none (it may wait on the owner for as long as the owner takes).
CALL_LIMIT_S = 900.0


def call_limit(mode: str, planner: bool) -> float | None:
    """The overall time one call may take: none for the planner's (ORCH-528) or a worker's, CALL_LIMIT_S for another
    non-worker call."""
    if mode == "worker" or planner:
        return None
    return CALL_LIMIT_S



class SignedOut(Unsupported):
    """A runtime that can't start because FERB's own sign-in for it is missing (step 8b, ORCH-315): an account
    fact, which the orchestrator offers to work around with another signed-in runtime; never with a model's judgment."""
    account = "signed_out"

    def __init__(self, message: str, runtime: str) -> None:
        super().__init__(message)
        self.runtime = runtime


# How the owner knows each runtime (the question offering another one names it so)
LABELS = {"codex": "Codex", "claude": "Claude", "anthropic": "the Anthropic API", "openai": "the OpenAI API",
          "google": "the Google API"}


def label(name: str) -> str:
    return LABELS.get(name, name)

@dataclass
class RuntimeDeps:
    """What the daemon lends a runtime it builds: FERB's MCP shim, its redactor, the gateway for direct-API tool calls,
    Keychain keys, the Claude tool-list proxy, and the Codex hooks for an abandoned call and the planner cache (P20)."""
    config: Any
    shim_cmd: list[str]
    redact: Callable[[Any], Any]
    bridge: Callable[[], Any]
    api_key: Callable[[str], Awaitable[str | None]]
    proxy: Callable[[], Any]
    on_abandoned: Callable[[str, str], None] | None = None
    cache_refusal: Callable[[Any], str] | None = None
    cache_skipped: Callable[[str, str], None] | None = None
    cache_fallback: Callable[[str, str], None] | None = None
    device_id: str = "unknown-device"


@dataclass(frozen=True)
class Build:
    """One runtime to build for one call: who asks (role, mode), its tools and answer form, its model and time limit."""
    home: Path
    role: str
    mode: Literal["supervisor", "worker"]
    tool_names: list[str]
    schema: dict[str, Any]
    model: str
    deps: RuntimeDeps
    effort: str | None = None             # the effort this call sends (run_effort); None: the runtime's default
    planner: bool = False                # the planner's chat turn (P20 cache; no overall limit, ORCH-528; the
                                          # API loop's answer tool where its form is stated, RUN-447)
    limit_s: float | None = None


@dataclass(frozen=True)
class Runtime:
    name: str
    default_model: str                    # "" when the owner must choose one
    sign_in: Literal["codex", "claude", "key"]
    billing: Literal["subscription", "api"]
    build: Callable[[Build], Any] = field(repr=False)
    limits_of: Callable[[str, Path], ModelLimits] = field(repr=False)     # (model, FERB home) -> documented limits

    def limits(self, model: str | None, home: Path) -> ModelLimits:
        """The documented limits of `model` (None: the default); ModelLimitsUnknown when there are none (L28)."""
        from ferb.core.errors import ModelLimitsUnknown
        chosen = model or self.default_model
        if not chosen:
            raise ModelLimitsUnknown(f"no model is chosen for {self.name}, so FERB doesn't know its context window")
        return self.limits_of(chosen, home)

    def signed_in(self, home: Path) -> tuple[bool, str]:
        """FERB's own sign-in (Codex, Claude) or the provider's key in the Keychain, and how to fix it when missing.
        Presence only: no key or credential is read."""
        if self.sign_in == "codex":
            from ferb.device.install import repo_root
            from ferb.runtimes.codex_exec import codex_binary, codex_home
            try:
                codex_binary(home, repo_root())
            except Unsupported:
                return False, "FERB's pinned Codex isn't installed: set FERB up again"
            return (True, "") if codex_home(home) is not None else (
                False, "Codex isn't signed in for FERB: sign in to ChatGPT in Settings → AI")
        if self.sign_in == "claude":
            from ferb.runtimes.claude_login import claude_home
            return ((True, "") if claude_home(home) is not None else
                    (False, "Claude isn't signed in for FERB: sign in to Claude in Settings → AI"))
        from ferb.runtimes.keys import has_api_key
        return (True, "") if has_api_key(self.name) else (
            False, f"FERB has no {self.name} API key: add one in Settings → AI")

    def available(self, home: Path, model: str | None = None) -> tuple[bool, str]:
        """Whether this runtime can run here: signed in (or its key present) and a model set; the fix when not.
        `model`: the model to judge (default: the owner's configured one, else the runtime's default)."""
        ok, fix = self.signed_in(home)
        if not ok:
            return False, fix
        chosen = model if model is not None else model_for(_config(home), self.name)
        if not chosen:
            return False, f"no model is chosen for {self.name}"
        return True, ""


def _config(home: Path) -> Any:
    from ferb.config import Config
    return Config(home)


def configured_model(cfg: Any, name: str) -> str | None:
    """The model the owner set for a runtime (`[models] <name>`; Claude's older `[runtimes] claude_model` too)."""
    model = cfg.get("models", name) or (cfg.get("runtimes", "claude_model") if name == "claude" else None)
    return str(model) if model else None


def model_for(cfg: Any, name: str) -> str:
    """The model a runtime runs: the owner's choice, else its default ("" when it has neither)."""
    entry = CATALOG.get(name)
    return configured_model(cfg, name) or (entry.default_model if entry else "")


def role_model(cfg: Any, name: str, role: str) -> str:
    """The model a role runs on runtime `name` (RUN-429): the owner's model for that role (`[role_models] <role>`, kept
    with the runtime it was chosen for, so it applies only while the role runs there), else the runtime's model."""
    chosen = cfg.get("role_models", role.partition(":")[0])
    if isinstance(chosen, dict) and chosen.get("runtime") == name and str(chosen.get("model") or "").strip():
        return str(chosen["model"]).strip()
    return model_for(cfg, name)


def configured_effort(cfg: Any, name: str) -> str:
    """The reasoning effort the owner set for a connection (`[efforts] <name>`); "" when none (the default applies)."""
    value = cfg.get("efforts", name)
    return str(value).strip() if isinstance(value, str) else ""


def role_effort(cfg: Any, name: str, role: str) -> str:
    """The effort the owner set for `role` on runtime `name`: the role's own (`[role_models] <role>` `effort`, kept with
    the runtime it was set for, so it applies only while the role runs there), else the connection's, else ""."""
    chosen = cfg.get("role_models", role.partition(":")[0])
    if isinstance(chosen, dict) and chosen.get("runtime") == name and str(chosen.get("effort") or "").strip():
        return str(chosen["effort"]).strip()
    return configured_effort(cfg, name)


# One-turn calls that aren't the planner's (the answer writer, distillation, intro suggestions, a check) and the
# planner when the owner set nothing: low effort on every runtime, Claude Code included (owner 2026-11-10)
ONE_TURN_EFFORT = "low"


def run_effort(cfg: Any, name: str, role: str, mode: str, planner: bool, asked: str | None) -> str | None:
    """The effort one call asks its runtime for (owner 2026-11-10): the planner's configured effort, else low; any other
    one-turn call low; a worker its configured effort, else the runtime's default (None: nothing is sent); a checker
    its configured effort, else what the caller asked (`asked`: "high" for a step that changed files), else the
    default. The owner's setting, when made, always wins."""
    if mode == "supervisor":
        return (role_effort(cfg, name, "orchestrator") or ONE_TURN_EFFORT) if planner else ONE_TURN_EFFORT
    kind = "checker" if role.partition(":")[0] == "checker" else "worker"
    return role_effort(cfg, name, kind) or asked or None


def effort_problem(name: str, effort: str) -> str:
    """Why `effort` can't be sent to runtime `name` as it stands (argument shape only; the runtime's own check call
    decides whether it takes it); "" when it can. Codex's "ultra" delegates to sub-agents, which FERB locks off
    (RUN-405), so it is not offered."""
    import re
    if not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9_-]{0,31}", effort):
        return "an effort level is one short word (letters, digits, - or _), at most 32 characters"
    if name == "codex" and effort.lower() == "ultra":
        return "Codex's ultra level hands work to sub-agents, which FERB keeps off; pick another level"
    return ""


def sign_in_fix(name: str) -> str:
    """The owner's step when a runtime's own call said its sign-in failed."""
    steps = {"codex": "sign in to ChatGPT again in Settings → AI", "claude": "sign in to Claude again in Settings → AI"}
    return steps.get(name, f"replace your {label(name).removeprefix('the ')} key in Settings → AI")


def api_providers() -> tuple[str, ...]:
    """The runtimes that run on a provider API key from the Keychain."""
    return tuple(name for name, entry in CATALOG.items() if entry.sign_in == "key")


# ---------- factories: everything model-specific stays in the adapter ----------

def _build_codex(b: Build) -> Any:
    from ferb.device.install import repo_root
    from ferb.runtimes.codex_exec import CodexRuntime, codex_binary, codex_home
    if codex_home(b.home) is None:
        raise SignedOut("Codex isn't signed in for FERB: sign in to ChatGPT in Settings → AI", "codex")
    # P20 (owner, 2026-10-25; RUN-401): every planner turn resumes a thread under a fixed cache key, an owner's chat
    # turn with its stable context in Codex's developer role (RUN-416). On by default since the live injection eval passed (2026-11-03, 72/72,
    # tests/security/test_planner_injection_real.py); `[runtimes] codex_planner_cache = false` turns it off.
    cache = b.planner and b.deps.config.get("runtimes", "codex_planner_cache", True) is not False
    return CodexRuntime(codex_binary(b.home, repo_root()), b.mode, b.home, shim_cmd=b.deps.shim_cmd,
                        tool_names=b.tool_names, output_schema=b.schema, model=b.model, timeout_s=b.limit_s,
                        on_abandoned=b.deps.on_abandoned, redact=b.deps.redact,
                        cache_role="planner" if cache else None, cache_scope=b.deps.device_id,
                        cache_refusal=b.deps.cache_refusal, on_cache_skipped=b.deps.cache_skipped,
                        on_cache_fallback=b.deps.cache_fallback,
                        effort=b.effort)


def _build_claude(b: Build) -> Any:
    from ferb.runtimes.claude_login import claude_home
    from ferb.runtimes.claude_sdk import ClaudeRuntime
    if claude_home(b.home) is None:          # never the owner's own ~/.claude login and billing
        raise SignedOut("Claude isn't signed in for FERB: sign in to Claude in Settings → AI", "claude")
    if not b.model:
        raise Unsupported("no model is chosen for Claude")
    return ClaudeRuntime(b.mode, b.deps.proxy(), str(b.home), shim_cmd=b.deps.shim_cmd, tool_names=b.tool_names,
                         output_schema=b.schema, model=b.model, effort=b.effort,
                         limit_s=b.limit_s)


def _api(provider: Callable[[str], Any]) -> Callable[[Build], Any]:
    def build(b: Build) -> Any:
        from ferb.runtimes.api import ApiRuntime
        return ApiRuntime(provider(b.model), mode=b.mode, role=b.role, tool_names=b.tool_names, output_schema=b.schema,
                          bridge=b.deps.bridge(), key=b.deps.api_key, effort=b.effort,
                          limit_s=b.limit_s, planner=b.planner)
    return build


def _anthropic(model: str) -> Any:
    from ferb.runtimes.anthropic_api import AnthropicProvider
    return AnthropicProvider(model)


def _openai(model: str) -> Any:
    from ferb.runtimes.openai_api import OpenAIProvider
    return OpenAIProvider(model)


def _google(model: str) -> Any:
    from ferb.runtimes.google_api import GoogleProvider
    return GoogleProvider(model)


def _codex_limits(model: str, home: Path) -> ModelLimits:
    from ferb.runtimes.codex_exec import limits_for
    return limits_for(home, model)


def _claude_limits(model: str, home: Path) -> ModelLimits:
    from ferb.runtimes.claude_sdk import limits_for
    return limits_for(model)


def _module_limits(module: str) -> Callable[[str, Path], ModelLimits]:
    def limits(model: str, home: Path) -> ModelLimits:
        import importlib
        return importlib.import_module(module).limits_for(model)  # type: ignore[no-any-return]
    return limits


CATALOG: dict[str, Runtime] = {
    # owner, 2026-10-14: GPT-5.5 retires from Codex for ChatGPT sign-ins
    "codex": Runtime("codex", "gpt-5.6-sol", "codex", "subscription", _build_codex, _codex_limits),
    # RUN-429 (owner 2026-11-08: run model "Claude Opus 5.5"): a Claude sign-in works out of the box; the id is one the
    # pinned Claude Code (2.1.310, claude-agent-sdk 0.2.174) names in its own binary, and anthropic_api.LIMITS documents
    "claude": Runtime("claude", "claude-opus-5-5", "claude", "subscription", _build_claude, _claude_limits),
    "anthropic": Runtime("anthropic", "claude-sonnet-5-5", "key", "api", _api(_anthropic),
                         _module_limits("ferb.runtimes.anthropic_api")),
    "openai": Runtime("openai", "gpt-6.1-sol", "key", "api", _api(_openai), _module_limits("ferb.runtimes.openai_api")),
    "google": Runtime("google", "gemini-3.8-flash", "key", "api", _api(_google),
                      _module_limits("ferb.runtimes.google_api")),
}
