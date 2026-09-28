"""Scopes, identity rules (section 10.1), hard floor, destructive patterns — and the action vocabulary every
policy check shares. Imports core only.
"""

import hashlib
import json
import re
from dataclasses import dataclass, field

__all__ = ["DataLabel", "Target"]  # re-exported: they live in core since build step 4
from pathlib import PurePosixPath

from ferb.core.types import DataLabel, Identity, Target, Task


# ---------- action vocabulary ----------

class Kind:
    READ, NAVIGATE, SEARCH, OPEN_TAB, DRAFT = "read", "navigate", "search", "open_tab", "draft"
    FILL_FORM, SUBMIT, SEND, POST, REPLY = "fill_form", "submit", "send", "post", "reply"
    PAY, DELETE_PERMANENT, TRASH = "pay", "delete_permanent", "trash"
    WRITE_FILE, RUN_SHELL, APPLESCRIPT, MAC_INPUT = "write_file", "run_shell", "applescript", "mac_input"
    FILL_SECRET, MAILBOX_READ, MAILBOX_SEND = "fill_secret", "mailbox_read", "mailbox_send"


# Reversible actions just run (section 10.2) once identity/scope/exposure pass.
REVERSIBLE = {Kind.READ, Kind.NAVIGATE, Kind.SEARCH, Kind.OPEN_TAB, Kind.DRAFT, Kind.TRASH}
HARD_FLOOR = {Kind.PAY, Kind.DELETE_PERMANENT}                 # always ask, every autonomy level
UNDO_WINDOW = {Kind.SEND, Kind.POST, Kind.REPLY, Kind.MAILBOX_SEND}   # held ~10 s before executing
OUTBOUND = {Kind.SEND, Kind.POST, Kind.REPLY, Kind.FILL_FORM, Kind.SUBMIT, Kind.MAILBOX_SEND, Kind.WRITE_FILE,
            Kind.PAY}                                           # moves data somewhere (data-flow checks)


@dataclass(frozen=True)
class ActionRequest:
    """What the gateway asks policy about (capability metadata + this call's arguments)."""
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
    data_labels: frozenset[DataLabel] = field(default_factory=frozenset)
    unverified_account: str | None = None    # "site @ browser/folder" whose signed-in account FERB can't check (9.3)


def payload_hash(a: ActionRequest) -> str:
    """Binds approvals and ledger rows to the exact action: any change to target/content/amount changes it."""
    canon = json.dumps({"capability": a.capability, "kind": a.kind, "identity": a.identity.value,
                        "account": a.target.account, "site": a.target.site,
                        "recipients": sorted(a.target.recipients), "path": a.target.path,
                        "amount": a.target.amount, "content": a.content}, sort_keys=True, separators=(",", ":"))
    return hashlib.sha256(canon.encode()).hexdigest()


# ---------- identity + scopes (section 10.1) ----------

SCOPES_BY_IDENTITY: dict[Identity, frozenset[str]] = {
    # FERB tasks: FERB's own browser profile, its secrets, its mailbox, files in FERB's workspace only.
    Identity.FERB: frozenset({"identity:browse", "secrets:fill", "mailbox:read", "mailbox:send",
                              "fs:read", "fs:write"}),
    # OWNER tasks: owner browsers, Mac control, shell, files. Never FERB's browser, secrets or mailbox.
    Identity.OWNER: frozenset({"browser:read", "browser:act", "mac:read", "mac:act", "applescript", "shell",
                               "fs:read", "fs:write", "screen"}),
}


def check_identity(a: ActionRequest, ferb_workspace: str) -> str | None:
    """Returns a violation reason, or None."""
    forbidden = a.scopes - SCOPES_BY_IDENTITY[a.identity]
    if forbidden:
        return f"{a.identity.value} task may not use {', '.join(sorted(forbidden))}"
    if a.identity is Identity.FERB and a.target.path is not None:
        if not _inside(a.target.path, ferb_workspace):
            return "FERB task may only touch files inside FERB's workspace"
    return None


def _inside(path: str, root: str) -> bool:
    p, r = PurePosixPath(path), PurePosixPath(root)
    if ".." in p.parts:
        return False
    return p == r or r in p.parents


def check_scopes(a: ActionRequest) -> str | None:
    missing = a.scopes - a.task_scopes
    return f"task lacks scopes {', '.join(sorted(missing))}" if missing else None


def check_child_authority(parent: Task, child: Task) -> str | None:
    """A child never gets more authority than its parent (scopes and budget). Identity is plan validation's job."""
    extra = child.scopes - parent.scopes
    if extra:
        return f"child asks for scopes its parent lacks: {', '.join(sorted(extra))}"
    for axis in ("max_seconds", "max_tokens", "max_cost_usd", "max_steps"):
        pv, cv = getattr(parent.budget, axis), getattr(child.budget, axis)
        if pv is not None and (cv is None or cv > pv):
            return f"child budget {axis} exceeds parent's"
    return None


# ---------- destructive patterns (used by the shell guard, build step 7) ----------

DESTRUCTIVE_SHELL = [re.compile(p) for p in (
    r"\brm\s+(-[a-zA-Z]*r[a-zA-Z]*f|-[a-zA-Z]*f[a-zA-Z]*r)\b",   # rm -rf / -fr
    r"\bgit\s+push\b.*(--force\b|-f\b)",
    r"\bgit\s+reset\s+--hard\b",
    r"\bgit\s+clean\s+-[a-zA-Z]*f",
    r"\bmkfs(\.\w+)?\b",
    r"\bdd\s+.*\bof=/dev/",
    r"\bdiskutil\s+(erase|zero|secureErase)",
    r"\bsudo\b",
    r":\(\)\s*\{\s*:\|:&\s*\};:",                               # fork bomb
    r">\s*/dev/(disk|rdisk)\d",
    r"\bchmod\s+-R\s+777\b",
    r"\bsrm\b|\bshred\b",
)]


def destructive_shell(cmd: str) -> bool:
    return any(p.search(cmd) for p in DESTRUCTIVE_SHELL)

