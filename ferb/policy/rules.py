"""Own action metadata and stable ledger hashes. Must not classify request/UI text or authorize effects."""

import hashlib
import json
from dataclasses import dataclass
from typing import Any

from ferb.core import origin
from ferb.core.types import Identity, Target


# Agent-declared annotations; these do not grant authority or infer meaning from control labels.
EFFECTS = ("navigate", "edit", "send", "pay", "destroy")

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


# Reversible kinds never get a ledger row: nothing of the owner's changes.
REVERSIBLE = {Kind.READ, Kind.NAVIGATE, Kind.SEARCH, Kind.OPEN_TAB, Kind.DRAFT, Kind.TRASH, Kind.TIMER}
# Kinds whose result is content the tool read (S3): a result that names no source is outside content, unlabeled.
READING_KINDS = frozenset({Kind.READ, Kind.SEARCH, Kind.NAVIGATE, Kind.MAILBOX_READ})


def recorded_origins(span_kind: Any, action_kind: Any, has_output: bool, recorded: str | None) -> set[str]:
    """A span's origins as recorded (MP2, S3): one rule for every reader (episodes, task.evidence). A reading tool
    call, or a context source, that holds output but no recorded origin (every span from before F3) is unlabeled
    outside content; any other span without one adds nothing. A label, never a gate."""
    found = origin.split(recorded)
    if not found and has_output and (span_kind == "context.source" or
                                     (span_kind == "tool.call" and action_kind in READING_KINDS)):
        return {origin.external("unlabeled")}
    return found

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


def payload_hash(a: ActionRequest) -> str:
    """Binds ledger rows to the exact action: any change to target/content/amount changes it."""
    canon = json.dumps({"capability": a.capability, "kind": a.kind, "effect": a.effect, "identity": a.identity.value,
                        "account": a.target.account, "site": a.target.site,
                        "recipients": sorted(a.target.recipients), "path": a.target.path,
                        "amount": a.target.amount, "content": a.content}, sort_keys=True, separators=(",", ":"))
    return hashlib.sha256(canon.encode()).hexdigest()
