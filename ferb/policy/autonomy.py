"""Autonomy levels (section 10.3/10.4): when does FERB ask? Rarely — the owner's instruction is the approval.

By effect class (docs/DESIGN_ONE_WORKER.md section 3): navigate and edit run; send asks once per (request, effect,
target) unless the owner's own message names the target; pay and destroy ask every time."""

from enum import Enum

from ferb.policy.rules import ActionRequest, irreversible


class Level(Enum):
    CAREFUL = "careful"
    BALANCED = "balanced"          # default
    HANDS_OFF = "hands-off"


def _where(a: ActionRequest) -> str:
    t = a.target
    people = ", ".join(r for r in t.recipients if r)
    if t.contact:
        people = str(t.contact)
    where = people or t.site or t.account or t.path or "this"
    if people and t.site:
        where = f"{people} on {t.site}"
    return where


def ask_reason(a: ActionRequest, level: Level, injection_on_path: bool,
               trusted_kinds: frozenset[str] = frozenset()) -> str | None:
    """Why this action needs the owner's confirmation, or None to proceed without asking."""
    shown = f": “{a.content[:200]}”" if a.content else ""
    said = f" — {a.why}" if a.why else ""
    if irreversible(a):
        if a.flagged_irreversible and a.effect not in ("pay", "destroy"):
            return "this can't be undone, so FERB asks first"                   # flagged by Jev
        return f"'{a.effect}' can't be undone, so FERB always asks ({_where(a)}{shown}){said}"   # the floor
    if a.effect == "send" and not a.owner_named:
        return f"{_where(a)}{shown}{said}"
    if injection_on_path and a.effect != "navigate":
        return "injection signals on this worker's path"
    if level is Level.CAREFUL and a.effect != "navigate" and a.kind not in trusted_kinds:
        return f"careful mode confirms '{a.kind}'"
    return None
