"""Owns the one table of model runtimes FERB can run (D32): each runtime's name, how it is built, its model's documented
limits, how it bills, and how to tell it can run here. Must not choose a runtime for a task.

Every list of runtime names (the daemon's builder, the planner's worker types, `ferb runtimes`, the app's API keys, the
Keychain accounts, the doctor) reads this table. A runtime is available when its model is set (the owner's
`[models] <name>` or the runtime's default; Claude has none) and FERB's own sign-in (Codex, Claude) or the provider's
key (the direct APIs) is present. Prices are per model (ferb/runtimes/prices.py, shared by Codex and the OpenAI API) and
reach every runtime's usage through ferb/runtimes/usage.py."""

from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Awaitable, Callable, Literal

from ferb.core.errors import Unsupported
from ferb.core.types import ModelLimits

ROLES = ("orchestrator", "worker", "checker")
# Owner, 2026-10-30 (ORCH-311): the planner's call has 90 s. Every other call that isn't a
# worker's (a dispatch, a question, a summary, distillation) has one overall limit, beside the 180 s silence guard in
# ferb/orchestrator/worker.py; a worker has none (it may wait on the owner for as long as the owner takes).
PLANNER_LIMIT_S = 90.0
CALL_LIMIT_S = 900.0


def call_limit(mode: str, planner: bool) -> float | None:
    """The overall time one call may take: the planner's, another non-worker call's, or none for a worker."""
    if mode == "worker":
        return None
    return PLANNER_LIMIT_S if planner else CALL_LIMIT_S



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
    cache_allowed: Callable[[Any], bool] | None = None
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
    effort: str | None = None
    planner: bool = False                 # the planner's chat turn (P20 cache, PLANNER_LIMIT_S)
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


def api_providers() -> tuple[str, ...]:
    """The runtimes that run on a provider API key from the Keychain."""
    return tuple(name for name, entry in CATALOG.items() if entry.sign_in == "key")


# ---------- factories: everything model-specific stays in the adapter ----------

def _build_codex(b: Build) -> Any:
    from ferb.device.install import repo_root
    from ferb.runtimes.codex_exec import CodexRuntime, codex_binary, codex_home
    if codex_home(b.home) is None:
        raise SignedOut("Codex isn't signed in for FERB: sign in to ChatGPT in Settings → AI", "codex")
    # P20 (owner, 2026-10-25; RUN-401): the planner's chat turn holds its stable context in Codex's developer role
    # under a fixed cache key. On by default since the live injection eval passed (2026-11-03, 72/72,
    # tests/security/test_planner_injection_real.py); `[runtimes] codex_planner_cache = false` turns it off.
    cache = b.planner and b.deps.config.get("runtimes", "codex_planner_cache", True) is not False
    return CodexRuntime(codex_binary(b.home, repo_root()), b.mode, b.home, shim_cmd=b.deps.shim_cmd,
                        tool_names=b.tool_names, output_schema=b.schema, model=b.model, timeout_s=b.limit_s,
                        on_abandoned=b.deps.on_abandoned, redact=b.deps.redact,
                        cache_role="planner" if cache else None, cache_scope=b.deps.device_id,
                        cache_allowed=b.deps.cache_allowed, on_cache_fallback=b.deps.cache_fallback,
                        effort=b.effort if b.mode == "worker" else None)


def _build_claude(b: Build) -> Any:
    from ferb.runtimes.claude_login import claude_home
    from ferb.runtimes.claude_sdk import ClaudeRuntime
    if claude_home(b.home) is None:          # never the owner's own ~/.claude login and billing
        raise SignedOut("Claude isn't signed in for FERB: sign in to Claude in Settings → AI", "claude")
    if not b.model:
        raise Unsupported("no model is chosen for Claude")
    return ClaudeRuntime(b.mode, b.deps.proxy(), str(b.home), shim_cmd=b.deps.shim_cmd, tool_names=b.tool_names,
                         output_schema=b.schema, model=b.model, effort=b.effort if b.mode == "worker" else None,
                         limit_s=b.limit_s)


def _api(provider: Callable[[str], Any]) -> Callable[[Build], Any]:
    def build(b: Build) -> Any:
        from ferb.runtimes.api import ApiRuntime
        return ApiRuntime(provider(b.model), mode=b.mode, role=b.role, tool_names=b.tool_names, output_schema=b.schema,
                          bridge=b.deps.bridge(), key=b.deps.api_key, effort=b.effort if b.mode == "worker" else None,
                          limit_s=b.limit_s)
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
    "claude": Runtime("claude", "", "claude", "subscription", _build_claude, _claude_limits),
    "anthropic": Runtime("anthropic", "claude-sonnet-5-5", "key", "api", _api(_anthropic),
                         _module_limits("ferb.runtimes.anthropic_api")),
    "openai": Runtime("openai", "gpt-6.1-sol", "key", "api", _api(_openai), _module_limits("ferb.runtimes.openai_api")),
    "google": Runtime("google", "gemini-3.8-flash", "key", "api", _api(_google),
                      _module_limits("ferb.runtimes.google_api")),
}
