"""Owns the list of model runtimes FERB can run and their capability facts (D32). Must not choose a runtime for a task.

Each entry: its default model, its capabilities for a model, and whether it can run on this Mac (with the fix if not)."""

from dataclasses import dataclass
from pathlib import Path
from typing import Callable

from ferb.core.types import Capabilities, ModelLimits

ROLES = ("orchestrator", "worker", "checker")


@dataclass(frozen=True)
class RuntimeInfo:
    name: str
    default_model: str
    capabilities: Callable[[str], Capabilities]
    available: Callable[[Path], tuple[bool, str]]     # (usable on this Mac, how to fix when not)
    # (model or None for the default, FERB home) -> the model's documented limits; ModelLimitsUnknown when none (L28)
    limits: Callable[[str | None, Path], ModelLimits]


def _codex_available(home: Path) -> tuple[bool, str]:
    from ferb.core.errors import Unsupported
    from ferb.runtimes.codex_exec import codex_binary, codex_home
    try:
        from ferb.device.install import repo_root
        codex_binary(home, repo_root())
    except Unsupported:
        return False, "run `ferb setup --update`"
    return (True, "") if codex_home(home) is not None else (False, "run `ferb setup --codex-login`")


def _codex_caps(model: str) -> Capabilities:
    from ferb.runtimes.codex_exec import CodexRuntime
    return CodexRuntime.capabilities


def _claude_caps(model: str) -> Capabilities:
    from ferb.runtimes.claude_sdk import ClaudeRuntime
    return ClaudeRuntime.capabilities


def _codex_limits(model: str | None, home: Path) -> ModelLimits:
    from ferb.runtimes.codex_exec import MODEL, limits_for
    return limits_for(home, model or MODEL)


def _claude_limits(model: str | None, home: Path) -> ModelLimits:
    from ferb.runtimes.claude_sdk import limits_for
    return limits_for(model)


CATALOG: dict[str, RuntimeInfo] = {
    "codex": RuntimeInfo("codex", "gpt-5.5", _codex_caps, _codex_available, _codex_limits),
    "claude": RuntimeInfo("claude", "", _claude_caps, lambda home: (True, ""), _claude_limits),
}


def _api_available(provider: str) -> Callable[[Path], tuple[bool, str]]:
    def check(home: Path) -> tuple[bool, str]:
        from ferb.runtimes.keys import has_api_key
        return (True, "") if has_api_key(provider) else (False, f"run `ferb setup --key {provider}`")
    return check


def _anthropic_caps(model: str) -> Capabilities:
    from ferb.runtimes.anthropic_api import AnthropicProvider
    return AnthropicProvider.capabilities


def _openai_caps(model: str) -> Capabilities:
    from ferb.runtimes.openai_api import capabilities_for
    return capabilities_for(model)


def _google_caps(model: str) -> Capabilities:
    from ferb.runtimes.google_api import GoogleProvider
    return GoogleProvider.capabilities


def _api_limits(provider: str, default: str) -> Callable[[str | None, Path], ModelLimits]:
    def limits(model: str | None, home: Path) -> ModelLimits:
        import importlib
        module = importlib.import_module({"anthropic": "ferb.runtimes.anthropic_api", "openai": "ferb.runtimes.openai_api",
                                          "google": "ferb.runtimes.google_api"}[provider])
        return module.limits_for(model or default)  # type: ignore[no-any-return]
    return limits


CATALOG.update({
    "anthropic": RuntimeInfo("anthropic", "claude-sonnet-5-5", _anthropic_caps, _api_available("anthropic"),
                             _api_limits("anthropic", "claude-sonnet-5-5")),
    "openai": RuntimeInfo("openai", "gpt-6.1-sol", _openai_caps, _api_available("openai"),
                          _api_limits("openai", "gpt-6.1-sol")),
    "google": RuntimeInfo("google", "gemini-3.8-flash", _google_caps, _api_available("google"),
                          _api_limits("google", "gemini-3.8-flash")),
})
