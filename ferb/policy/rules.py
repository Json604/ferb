"""Own action metadata and stable ledger hashes. Must not classify request/UI text or authorize effects."""

import hashlib
import json
from dataclasses import dataclass
from typing import Any
from ferb.core.types import Identity, Target


# Agent-declared annotations; these do not grant authority or infer meaning from control labels.
EFFECTS = ("navigate", "edit", "send", "pay", "destroy")
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
