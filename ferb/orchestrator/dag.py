"""Plan validation and the task graph (section 7 step 3). Deterministic code, before any worker starts.

Checks: ids unique; dependencies and inputs name earlier subtasks; no cycles; fan-out <= 6 (subtasks under the
request); depth <= 2 (request -> subtask -> the verification / reconciliation workers under it; workers can't start
workers); identity + scope rules per subtask; readers ask only for read scopes and declare typed output fields;
actors' inputs exist on the reader they come from; every intent target appears in the owner's own words; the tree
budget stays within the configured maximums.
"""

import re
from dataclasses import dataclass

from ferb.orchestrator.schemas import KINDS, MAX_FIELD_CHARS, Plan, Subtask
from ferb.policy.intent import IMPLIES
from ferb.policy.rules import SCOPES_BY_IDENTITY
from ferb.core.types import Identity
from ferb.sanitize.untrusted import targets_in_own_words

MAX_FANOUT, MAX_DEPTH, MAX_DEADLINE_MIN = 6, 2, 120
READ_SCOPES = frozenset({"fs:read", "browser:read", "mac:read", "screen", "identity:browse", "mailbox:read"})
ID = re.compile(r"^[a-z][a-z0-9_-]{0,31}$")


@dataclass(frozen=True)
class Limits:
    """Tree budget maximums (config [budget]); a plan may lower them, never raise them."""
    max_minutes: int = 120
    max_tokens: int = 1_000_000
    max_cost_usd: float = 2.0
    max_tool_calls: int = 200
    max_workers: int = 12


DEFAULT_DEADLINE_MIN = 10


def normalize(plan: Plan, limits: Limits) -> None:
    """A model's formatting slip isn't the owner's question: an out-of-range subtask deadline becomes the default
    (none given) or the cap (too long)."""
    cap = min(MAX_DEADLINE_MIN, limits.max_minutes)
    for s in plan.subtasks:
        if s.deadline_minutes < 1:
            s.deadline_minutes = min(DEFAULT_DEADLINE_MIN, cap)
        elif s.deadline_minutes > cap:
            s.deadline_minutes = cap


def validate(plan: Plan, own_words: str, available_scopes: set[str], limits: Limits) -> list[str]:
    normalize(plan, limits)
    p: list[str] = []
    i = plan.intent
    if set(i.verbs) - set(IMPLIES):
        p.append(f"I don't know how to '{', '.join(sorted(set(i.verbs) - set(IMPLIES)))}'")
    if set(i.extra_kinds) - set(KINDS):
        p.append("the plan names an unknown kind of action")
    missing = targets_in_own_words(i.recipients + i.sites + i.accounts + i.paths, own_words)
    if missing:
        p.append("please name these yourself (I won't take them from pasted or quoted text): " + ", ".join(missing))
    if not plan.subtasks:
        p.append("the plan has no subtasks")
    if len(plan.subtasks) > MAX_FANOUT:
        p.append(f"too many subtasks ({len(plan.subtasks)}; at most {MAX_FANOUT})")
    ids = [s.id for s in plan.subtasks]
    if len(set(ids)) != len(ids) or not all(ID.match(x) for x in ids):
        p.append("subtask ids must be unique short names")
    by_id = {s.id: s for s in plan.subtasks}
    for s in plan.subtasks:
        p += _subtask(s, by_id, available_scopes, limits)
    if not p and _cycle(plan.subtasks):
        p.append("the subtasks depend on each other in a circle")
    b = plan.budget
    for name, value, cap in (("minutes", b.max_minutes, limits.max_minutes), ("tokens", b.max_tokens, limits.max_tokens),
                             ("cost", b.max_cost_usd, limits.max_cost_usd),
                             ("tool calls", b.max_tool_calls, limits.max_tool_calls),
                             ("workers", b.max_workers, limits.max_workers)):
        if value is not None and (value <= 0 or value > cap):
            p.append(f"the budget's {name} must be between 1 and {cap}")
    if b.max_workers is not None and b.max_workers < len(plan.subtasks) + 1:
        p.append(f"the budget's workers ({b.max_workers}) can't cover {len(plan.subtasks)} subtasks plus "
                 "verification")
    return p


def _subtask(s: Subtask, by_id: dict[str, Subtask], available: set[str], limits: Limits) -> list[str]:
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
    if not 1 <= s.deadline_minutes <= min(MAX_DEADLINE_MIN, limits.max_minutes):
        p.append(f"subtask {s.id}: the deadline must be 1-{min(MAX_DEADLINE_MIN, limits.max_minutes)} minutes")
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
