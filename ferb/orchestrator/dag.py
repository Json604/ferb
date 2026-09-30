"""Plan validation and the task graph (section 7 step 3). Deterministic code, before any worker starts.

Checks: ids unique; dependencies and inputs name earlier subtasks; no cycles; fan-out <= 6 (subtasks under the
request); depth <= 2 (request -> subtask -> the verification / reconciliation workers under it; workers can't start
workers); identity + scope rules per subtask; readers ask only for read scopes and declare typed output fields;
actors' inputs exist on the reader they come from; every intent target appears in the owner's own words. No budgets:
runaway guards (guards.py) catch stuck trees instead.
"""

import re
from typing import Any

from ferb.orchestrator.schemas import KINDS, MAX_FIELD_CHARS, Plan, Subtask
from ferb.policy.intent import IMPLIES
from ferb.policy.rules import SCOPES_BY_IDENTITY
from ferb.core.types import Identity
from ferb.policy.names import resolve_apps, site_named
from ferb.sanitize.untrusted import targets_in_own_words

MAX_FANOUT, MAX_DEPTH = 6, 2
READ_SCOPES = frozenset({"fs:read", "browser:read", "mac:read", "screen", "trail:read", "identity:browse",
                         "mailbox:read"})
ID = re.compile(r"^[a-z][a-z0-9_-]{0,31}$")


def validate(plan: Plan, own_words: str, available_scopes: set[str],
             apps: list[tuple[str, str]] | None = None, owner_browser: bool | None = None) -> list[str]:
    """`owner_browser`: whether the owner's words point at their own browser (None: not checked)."""
    p: list[str] = []
    p += validate_intent(plan.intent, own_words, apps)
    if not plan.subtasks:
        p.append("the plan has no subtasks")
    if len(plan.subtasks) > MAX_FANOUT:
        p.append(f"too many subtasks ({len(plan.subtasks)}; at most {MAX_FANOUT})")
    ids = [s.id for s in plan.subtasks]
    if len(set(ids)) != len(ids) or not all(ID.match(x) for x in ids):
        p.append("subtask ids must be unique short names")
    by_id = {s.id: s for s in plan.subtasks}
    for s in plan.subtasks:
        p += _subtask(s, by_id, available_scopes)
        if owner_browser is False and s.identity == "owner" and {"browser:read", "browser:act"} & set(s.scopes):
            p.append(f"subtask {s.id}: the owner didn't ask for their own browser — work on websites in FERB's own "
                     "browser (identity \"ferb\"), or ask the owner if it must be theirs")
    if not p and _cycle(plan.subtasks):
        p.append("the subtasks depend on each other in a circle")
    return p


def validate_intent(i: Any, own_words: str, apps: list[tuple[str, str]] | None = None) -> list[str]:
    """The intent's verbs and targets: every target must be in the owner's own words (names count, 14a)."""
    p = []
    if set(i.verbs) - set(IMPLIES):
        p.append(f"I don't know how to '{', '.join(sorted(set(i.verbs) - set(IMPLIES)))}'")
    if set(i.extra_kinds) - set(KINDS):
        p.append("the plan names an unknown kind of action")
    missing = targets_in_own_words(i.recipients + i.accounts + i.paths, own_words)
    missing += [s for s in i.sites if not site_named(s, own_words)]            # "Slack" names slack.com
    missing += resolve_apps(i.apps, apps or [], own_words)[1]                  # "Brave" names the Brave app
    if missing:
        p.append("please name these yourself (I won't take them from pasted or quoted text): " + ", ".join(missing))
    return p


def _subtask(s: Subtask, by_id: dict[str, Subtask], available: set[str]) -> list[str]:
    p = []
    identity = Identity(s.identity)
    extra = set(s.scopes) - SCOPES_BY_IDENTITY[identity]
    if extra:
        p.append(f"subtask {s.id}: a {identity.value} task can't have {', '.join(sorted(extra))}")
    if set(s.scopes) - available:
        p.append(f"subtask {s.id}: FERB can't do {', '.join(sorted(set(s.scopes) - available))} yet")
    if s.role == "reader":
        if set(s.scopes) - READ_SCOPES:
            p.append(f"subtask {s.id}: a reader only reads (no {', '.join(sorted(set(s.scopes) - READ_SCOPES))})")
        if not s.output_fields:
            p.append(f"subtask {s.id}: a reader must declare the typed fields it returns")
    elif s.output_fields:
        p.append(f"subtask {s.id}: only readers return fields")
    names = [f.name for f in s.output_fields]
    if len(set(names)) != len(names) or not all(ID.match(n) for n in names):
        p.append(f"subtask {s.id}: field names must be unique short names")
    for f in s.output_fields:
        if f.max_chars is not None and not 1 <= f.max_chars <= MAX_FIELD_CHARS:
            p.append(f"subtask {s.id}: field {f.name} may hold at most {MAX_FIELD_CHARS} characters")
    for d in s.depends_on:
        if d not in by_id or d == s.id:
            p.append(f"subtask {s.id}: depends on unknown subtask {d}")
    for ref in s.inputs:
        src = by_id.get(ref.from_)
        if src is None or ref.from_ not in s.depends_on:
            p.append(f"subtask {s.id}: input {ref.from_}.{ref.field} must come from a subtask it depends on")
        elif src.role != "reader" or ref.field not in {f.name for f in src.output_fields}:
            p.append(f"subtask {s.id}: {ref.from_} doesn't return a field {ref.field}")
    return p


def _cycle(subtasks: list[Subtask]) -> bool:
    deps = {s.id: set(s.depends_on) for s in subtasks}
    seen: set[str] = set()
    while True:
        ready = {t for t, d in deps.items() if t not in seen and d <= seen}
        if not ready:
            return len(seen) != len(deps)
        seen |= ready


def ready(subtasks: list[Subtask], finished: set[str], running: set[str]) -> list[Subtask]:
    return [s for s in subtasks if s.id not in finished and s.id not in running and set(s.depends_on) <= finished]
