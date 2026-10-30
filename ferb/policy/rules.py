"""Own action metadata and stable ledger hashes. Must not classify request/UI text or authorize effects."""

import dataclasses
import hashlib
import json
from dataclasses import dataclass
from typing import Any
from ferb.core.types import ActionSpec, Identity, Target


# Agent-declared annotations; these do not grant authority or infer meaning from control labels. On a generic input
# (GENERIC_INPUT) a consequential effect is checked as its kind (`declare`): it can only add a question.
EFFECTS = ("navigate", "edit", "send", "pay", "destroy", "account_security")
# The agent's own notes on a call (why, an effect, a declared purpose, who it says a call/send is for): recorded,
# never part of what the call is, so changing them can't make a repeat a new action (P19).
ANNOTATIONS = ("why", "effect", "purpose", "contact")

class Kind:
    READ, NAVIGATE, SEARCH, OPEN_TAB, DRAFT = "read", "navigate", "search", "open_tab", "draft"
    FILL_FORM, SUBMIT, SEND, POST, REPLY = "fill_form", "submit", "send", "post", "reply"
    PAY, DELETE_PERMANENT, TRASH = "pay", "delete_permanent", "trash"
    WRITE_FILE, RUN_SHELL, APPLESCRIPT, MAC_INPUT = "write_file", "run_shell", "applescript", "mac_input"
    FILL_SECRET, MAILBOX_READ, MAILBOX_SEND = "fill_secret", "mailbox_read", "mailbox_send"
    TIMER = "timer"                                       # a reminder / timer for the owner (cancelable)
    ACCOUNT_SECURITY = "account_security"                 # passwords, 2FA, recovery settings, account deletion
    CALL = "call"                                         # starting a voice / video call
    CLICK = "click"                                       # a plain click on a page control (no floor word, no form)
    SNAPSHOT = "snapshot"                                 # a local disk snapshot, a recovery point (S1)
    STOP_PROCESS = "stop_process"                         # ending one of the task's own processes (shell.kill)


# W2: input whose physical kind says nothing of what it does (a click, typing or a key, an app's control or menu item,
# an app script). What it does is the agent's declared effect, which the gateway checks as that kind (`declare`).
GENERIC_INPUT = frozenset({Kind.CLICK, Kind.FILL_FORM, Kind.MAC_INPUT, Kind.APPLESCRIPT})
EFFECT_KINDS = {"send": Kind.SEND, "pay": Kind.PAY, "destroy": Kind.DELETE_PERMANENT,
                "account_security": Kind.ACCOUNT_SECURITY}
ALWAYS_DECLARED = frozenset({Kind.PAY, Kind.ACCOUNT_SECURITY})   # honoured on any acting call, not only an input


def declared_kinds(spec: ActionSpec) -> frozenset[str]:
    """Every kind a call's declarations add: the capability's declared purpose and the agent's declared effect."""
    return frozenset(k for k in (spec.declared, spec.effect_kind) if k)


def declared_of(recorded: Any) -> set[str]:
    """The declared kinds a tool.call span records (`declared`: one kind as a string, several as a list)."""
    if isinstance(recorded, str):
        return {recorded} if recorded else set()
    return {str(k) for k in recorded} if isinstance(recorded, (list, tuple, set, frozenset)) else set()


def declare(spec: ActionSpec, effect: str, recipients: tuple[str, ...] = (), account: str | None = None,
            amount: float | None = None, item: str | None = None, currency: str | None = None) -> ActionSpec:
    """W2: a generic input checked as the effect the agent declares for it (send, pay, destroy, account_security),
    with the destination it declares added to its target: the account it posts, sends or pays from, a message's
    recipients, a payment's amount, what a delete removes. A payment or an account-security change declared on any
    other acting call is honoured too (L3 keeps both asks); send and destroy there stay notes (the shell has its own
    boundaries). It only adds: the capability's own kind, declared purpose and target stay, every declared kind is
    kept (`declared_kinds`), and an effect that names no consequential kind, or a call that changes nothing, is
    unchanged. Code reads the declaration only, never the page's or app's words (rule 6). It is the agent's note, never
    what the call is: P19 hashes the call as the capability described it."""
    kind = EFFECT_KINDS.get(effect.strip().lower())
    if kind is None or spec.kind in REVERSIBLE or (spec.kind not in GENERIC_INPUT and kind not in ALWAYS_DECLARED):
        return spec
    t = spec.target
    target = dataclasses.replace(
        t, recipients=tuple(dict.fromkeys([*t.recipients, *(r.strip() for r in recipients if r.strip())])),
        account=t.account or (account or "").strip() or None, amount=t.amount if t.amount is not None else amount,
        currency=t.currency or (currency or "").strip()[:8] or None, item=t.item or (item or "").strip() or None)
    return dataclasses.replace(spec, effect_kind=kind, target=target)


# Reversible kinds never get a ledger row: nothing of the owner's changes.
REVERSIBLE = {Kind.READ, Kind.NAVIGATE, Kind.SEARCH, Kind.OPEN_TAB, Kind.DRAFT, Kind.TRASH, Kind.TIMER,
              Kind.SNAPSHOT}
# Kinds whose result is content the tool read (S3): a result that names no source is outside content, unlabeled.
READING_KINDS = frozenset({Kind.READ, Kind.SEARCH, Kind.NAVIGATE, Kind.MAILBOX_READ})

@dataclass(frozen=True)
class ActionRequest:
    """Recorded action metadata, with no authorization verdict."""
    capability: str
    kind: str
    identity: Identity
    scopes: frozenset[str]                   # scopes this capability needs
    task_id: str
    worker_id: str
    request_id: str
    task_scopes: frozenset[str]              # what the worker's task was granted
    target: Target = Target()
    content: str | None = None
    effect: str = ""                         # optional agent annotation
    why: str = ""                            # the worker's one-sentence reason, recorded in the trace


def payload_hash(a: ActionRequest, args: dict[str, Any] | None = None) -> str:
    """Binds ledger rows to the exact call: any change to target/content/amount, or to the call's validated arguments
    (`args`, e.g. a click point, a menu path, an element), changes it. Left out: the agent's notes (`ANNOTATIONS`), and
    the kind, which describe() derives from the capability and arguments already bound here (a declared purpose
    would otherwise vary it). The caller leaves out arguments the target already binds in normalized form (a path)."""
    call_args = {k: v for k, v in (args or {}).items() if k not in ANNOTATIONS}
    canon = json.dumps({"capability": a.capability, "identity": a.identity.value,
                        "account": a.target.account, "site": a.target.site, "contact": a.target.contact,
                        "recipients": sorted(a.target.recipients), "path": a.target.path,
                        "amount": a.target.amount, "content": a.content, "args": call_args},
                       sort_keys=True, separators=(",", ":"), default=str)
    return hashlib.sha256(canon.encode()).hexdigest()
