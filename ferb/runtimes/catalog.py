"""Owns the list of model runtimes FERB can run and their capability facts (D32). Must not choose a runtime for a task.

Each entry: its default model, its capabilities for a model, and whether it can run on this Mac (with the fix if not).
The direct-API runtimes join in Task 10 of the F2 plan."""

from dataclasses import dataclass
from pathlib import Path
from typing import Callable

from ferb.core.types import Capabilities

ROLES = ("orchestrator", "worker", "checker")


@dataclass(frozen=True)
class RuntimeInfo:
    name: str
    default_model: str
    capabilities: Callable[[str], Capabilities]
    available: Callable[[Path], tuple[bool, str]]     # (usable on this Mac, how to fix when not)


def _codex_available(home: Path) -> tuple[bool, str]:
    from ferb.core.errors import Unsupported
    from ferb.runtimes.codex_exec import codex_binary, codex_home
    try:
        codex_binary(home)
    except Unsupported:
        return False, "run `ferb setup --update`"
    return (True, "") if codex_home(home) is not None else (False, "run `ferb setup --codex-login`")


def _codex_caps(model: str) -> Capabilities:
    from ferb.runtimes.codex_exec import CodexRuntime
    return CodexRuntime.capabilities


def _claude_caps(model: str) -> Capabilities:
    from ferb.runtimes.claude_sdk import ClaudeRuntime
    return ClaudeRuntime.capabilities


CATALOG: dict[str, RuntimeInfo] = {
    "codex": RuntimeInfo("codex", "gpt-5.5", _codex_caps, _codex_available),
    "claude": RuntimeInfo("claude", "", _claude_caps, lambda home: (True, "")),
}
