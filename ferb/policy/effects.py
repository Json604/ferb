"""Optional effect vocabulary and observed control-label classification for action metadata."""

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


def label_kind(label: str | None) -> str | None:
    """Use the control's name before its comma-separated descriptive preview.

    Accessibility often appends unread counts and message previews to a control name. Those words describe
    content, not the action. Keep all floors in the actual name (including "Video call" and "Confirm and pay").
    The declaration still covers meaning that a label cannot establish.
    """
    name = (label or "").split(",", 1)[0]
    matches = [FLOOR_WORDS[m.group(1).lower()] for m in _FLOOR.finditer(name)]
    return max(matches, key=lambda kind: RANK[floor_of(kind)]) if matches else None
