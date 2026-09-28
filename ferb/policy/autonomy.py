"""Autonomy levels (section 10.3/10.4): when does FERB ask? Rarely — the owner's instruction is the approval."""

from enum import Enum

from ferb.policy.rules import HARD_FLOOR, REVERSIBLE, ActionRequest


class Level(Enum):
    CAREFUL = "careful"
    BALANCED = "balanced"          # default
    HANDS_OFF = "hands-off"


def ask_reason(a: ActionRequest, level: Level, intent_ask: str | None, injection_on_path: bool,
               trusted_kinds: frozenset[str] = frozenset()) -> str | None:
    """Why this action needs the owner's confirmation, or None to proceed without asking."""
    if a.kind in HARD_FLOOR:
        return f"'{a.kind}' always needs confirmation (hard floor)"          # every level, even if requested
    if intent_ask:
        return intent_ask                                                   # surprising destination etc.
    if injection_on_path and a.kind not in REVERSIBLE:
        return "injection signals on this worker's path"
    if level is Level.CAREFUL and a.kind not in REVERSIBLE and a.kind not in trusted_kinds:
        return f"careful mode confirms '{a.kind}'"
    return None
