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
    TIMER = "timer"                                       # a reminder / timer for the owner (cancelable)
    ACCOUNT_SECURITY = "account_security"                 # passwords, 2FA, recovery settings, account deletion
    CALL = "call"                                         # starting a voice / video call (build step 16)
    CLICK = "click"                                       # a plain click on a page control (no floor word, no form)


# Reversible kinds never get a ledger row (section 10.2): nothing of the owner's changes.
REVERSIBLE = {Kind.READ, Kind.NAVIGATE, Kind.SEARCH, Kind.OPEN_TAB, Kind.DRAFT, Kind.TRASH, Kind.TIMER}
# The irreversible floor (10.3): ask every single time, every autonomy level; nothing can remove a kind from it.
HARD_FLOOR = {Kind.PAY, Kind.DELETE_PERMANENT, Kind.ACCOUNT_SECURITY}
HARD_EFFECTS = {"pay", "destroy"}                               # the same floor, by effect class
UNDO_WINDOW = {Kind.SEND, Kind.POST, Kind.REPLY, Kind.MAILBOX_SEND, Kind.CALL}   # held ~10 s before executing
OUTBOUND = {Kind.SEND, Kind.POST, Kind.REPLY, Kind.FILL_FORM, Kind.SUBMIT, Kind.MAILBOX_SEND, Kind.WRITE_FILE,
            Kind.PAY, Kind.CLICK, Kind.MAC_INPUT}                 # moves data somewhere (data-flow checks)


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
    flagged_irreversible: bool = False       # Jev marked this action irreversible: ask every time, bound to it
    effect: str = "edit"                     # the effective effect class: max(declared, floor) (effects.py)
    owner_named: bool = False                # the owner's own message names where this goes (targets.owner_named)
    why: str = ""                            # the worker's one-sentence reason (shown with an ask)


def scope_key(a: ActionRequest) -> str:
    """What one approval covers (10.3): this request, this effect class, this target — whatever the content."""
    canon = json.dumps({"request": a.request_id, "effect": a.effect, "identity": a.identity.value,
                        "account": a.target.account, "site": a.target.site, "recipients": sorted(a.target.recipients),
                        "path": a.target.path, "amount": a.target.amount}, sort_keys=True, separators=(",", ":"))
    return hashlib.sha256(canon.encode()).hexdigest()


def irreversible(a: ActionRequest) -> bool:
    """The floor (pay / destroy, by kind or by declared effect), plus what Jev flagged: asks every time, one call at
    a time, bound to the exact action."""
    return a.kind in HARD_FLOOR or a.effect in HARD_EFFECTS or a.flagged_irreversible


def payload_hash(a: ActionRequest) -> str:
    """Binds approvals and ledger rows to the exact action: any change to target/content/amount changes it."""
    canon = json.dumps({"capability": a.capability, "kind": a.kind, "effect": a.effect, "identity": a.identity.value,
                        "account": a.target.account, "site": a.target.site,
                        "recipients": sorted(a.target.recipients), "path": a.target.path,
                        "amount": a.target.amount, "content": a.content}, sort_keys=True, separators=(",", ":"))
    return hashlib.sha256(canon.encode()).hexdigest()


# ---------- identity + scopes (section 10.1) ----------

# Acting needs seeing: an act scope brings its read scope (a worker with browser:act must open and read the page).
IMPLIED_READ = {"browser:act": "browser:read", "mac:act": "mac:read", "fs:write": "fs:read"}


def with_reads(scopes: set[str]) -> set[str]:
    return scopes | {IMPLIED_READ[s] for s in scopes if s in IMPLIED_READ}


SCOPES_BY_IDENTITY: dict[Identity, frozenset[str]] = {
    # FERB tasks: FERB's own browser profile, its secrets, its mailbox, files in FERB's workspace only.
    Identity.FERB: frozenset({"identity:browse", "secrets:fill", "mailbox:read", "mailbox:send",
                              "fs:read", "fs:write", "time"}),
    # OWNER tasks: owner browsers, Mac control, shell, files. Never FERB's browser, secrets or mailbox.
    Identity.OWNER: frozenset({"browser:read", "browser:act", "mac:read", "mac:act", "applescript", "shell",
                               "fs:read", "fs:write", "screen", "trail:read", "time"}),
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
    """A child never gets more authority than its parent (scopes, and no later deadline). Identity is plan
    validation's job."""
    extra = child.scopes - parent.scopes
    if extra:
        return f"child asks for scopes its parent lacks: {', '.join(sorted(extra))}"
    if parent.deadline is not None and (child.deadline is None or child.deadline > parent.deadline):
        return "child runs past its parent's time limit"
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

