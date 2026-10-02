"""Effect classes (docs/DESIGN_ONE_WORKER.md section 3): the gate at the action.

Every act call declares an `effect` — navigate / edit / send / pay / destroy — and the gateway runs
`effective = max(declared, floor)`. The floor is code-derived from facts about the tool (its `ActionSpec.kind`) and
about the control (a form submit, a label that literally says Send / Pay / Delete …). It can only raise: the model
decides what an action means, the floor protects when it is wrong.

This module holds the one remaining label list. It is a floor, not a permission: a control whose label literally
says one of these words can't be declared harmless. Any other label is left to the model's declaration.
"""

import re

from ferb.policy.rules import Kind

EFFECTS = ("navigate", "edit", "send", "pay", "destroy")
RANK = {e: i for i, e in enumerate(EFFECTS)}

# Facts about the tools: what each action kind at least does.
FLOOR_BY_KIND: dict[str, str] = {
    Kind.READ: "navigate", Kind.NAVIGATE: "navigate", Kind.SEARCH: "navigate", Kind.OPEN_TAB: "navigate",
    Kind.DRAFT: "edit", Kind.FILL_FORM: "edit", Kind.CLICK: "edit", Kind.WRITE_FILE: "edit", Kind.MAC_INPUT: "edit",
    Kind.APPLESCRIPT: "edit", Kind.RUN_SHELL: "edit", Kind.TRASH: "edit", Kind.TIMER: "edit",
    Kind.FILL_SECRET: "edit", Kind.MAILBOX_READ: "navigate",
    Kind.SEND: "send", Kind.POST: "send", Kind.REPLY: "send", Kind.SUBMIT: "send", Kind.MAILBOX_SEND: "send",
    Kind.CALL: "send",
    Kind.PAY: "pay",
    Kind.DELETE_PERMANENT: "destroy", Kind.ACCOUNT_SECURITY: "destroy",
}

# The floor words (the spec's list, plus "call": a call reaches another person like a send does). A control whose
# label literally says one of these is at least that kind, whatever the model declared. Case-insensitive, whole
# words; labels in other languages don't hit the floor and are left to the declaration.
FLOOR_WORDS: dict[str, str] = {
    "send": Kind.SEND, "submit": Kind.SUBMIT, "call": Kind.CALL,
    "pay": Kind.PAY, "buy": Kind.PAY, "order": Kind.PAY,
    "delete": Kind.DELETE_PERMANENT, "remove": Kind.DELETE_PERMANENT, "unsubscribe": Kind.DELETE_PERMANENT,
    "sign out": Kind.ACCOUNT_SECURITY,
}
_FLOOR = re.compile(r"\b(" + "|".join(re.escape(w) for w in sorted(FLOOR_WORDS, key=len, reverse=True)) + r")\b",
                    re.I)


def floor_of(kind: str) -> str:
    """The effect class a kind is at least (unknown kinds count as edit: they act)."""
    return FLOOR_BY_KIND.get(kind, "edit")


def raise_to(declared: str | None, floor: str) -> str:
    """max(declared, floor); an absent or unknown declaration is the floor."""
    if declared not in RANK:
        return floor
    return declared if RANK[declared] >= RANK[floor] else floor


def label_kind(label: str | None) -> str | None:
    """Use the control's name before its comma-separated descriptive preview.

    Accessibility often appends unread counts and message previews to a control name. Those words describe
    content, not the action. Keep all floors in the actual name (including "Video call" and "Confirm and pay").
    The declaration still covers meaning that a label cannot establish.
    """
    name = (label or "").split(",", 1)[0]
    matches = [FLOOR_WORDS[m.group(1).lower()] for m in _FLOOR.finditer(name)]
    return max(matches, key=lambda kind: RANK[floor_of(kind)]) if matches else None



def is_valid(effect: object) -> bool:
    return isinstance(effect, str) and effect in RANK
