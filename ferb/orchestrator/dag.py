"""Task checks before a worker starts (docs/DESIGN_ONE_WORKER.md section 1). Deterministic code.

What's left of plan validation: the identity floor (a "ferb" task never gets owner scopes — `SCOPES_BY_IDENTITY` —
and can't name owner folders or browsers as targets), and that FERB has something usable for that identity right
now. Targets are informational: never a reason to refuse. No budgets: runaway guards (guards.py) catch stuck tasks.
"""

import re

from ferb.core.types import Identity
from ferb.orchestrator.schemas import TaskSpec
from ferb.policy.rules import SCOPES_BY_IDENTITY, with_reads

READ_SCOPES = frozenset({"fs:read", "browser:read", "mac:read", "screen", "trail:read", "identity:browse",
                         "mailbox:read"})
ID = re.compile(r"^[a-z][a-z0-9_-]{0,31}$")


def scopes_for(identity: Identity, available: set[str]) -> set[str]:
    """Everything this identity may use that FERB has right now (the worker gets all of it, reads included)."""
    return with_reads(set(SCOPES_BY_IDENTITY[identity]) & available)


def validate(task: TaskSpec, available_scopes: set[str], workspace: str = "") -> list[str]:
    """Problems that send the task back to the model once (schema / identity), else nothing."""
    p: list[str] = []
    try:
        identity = Identity(task.identity)
    except ValueError:
        return ["identity must be \"owner\" or \"ferb\""]
    if not scopes_for(identity, available_scopes) - {"time"}:
        p.append(f"FERB has nothing usable for a {identity.value} task right now")
    if identity is Identity.FERB:
        if task.targets.paths and workspace and any(not _inside(x, workspace) for x in task.targets.paths):
            p.append("a ferb task may only use files inside FERB's workspace; the owner's folders need identity "
                     "\"owner\"")
    return p


def _inside(path: str, root: str) -> bool:
    import os
    p = os.path.normpath(os.path.expanduser(path))
    r = os.path.normpath(root)
    return p == r or p.startswith(r.rstrip("/") + "/") or not os.path.isabs(p)
