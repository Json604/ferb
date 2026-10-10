"""D30/P15 plan vocabulary and mechanical feasibility checks; task choices belong to the planner."""

from pathlib import Path
from datetime import datetime
from collections import Counter, deque
from typing import Any, Literal

from pydantic import BaseModel, Field

from ferb.orchestrator.conversations import ConversationScope


class EventMatch(BaseModel):
    field: str
    value: str


class Step(BaseModel):
    id: str
    kind: Literal['do', 'map', 'reduce', 'wait', 'ask', 'converse']
    goal: str = ''
    depends_on: list[str] = Field(default_factory=list)
    inputs: list[str] = Field(default_factory=list)
    batches: list[list[str]] = Field(default_factory=list)
    worker_type: str = 'default'
    conditions: list[str] = Field(default_factory=list)
    until: str | None = None
    event: str | None = None
    event_match: list[EventMatch] = Field(default_factory=list)
    conversation: ConversationScope | None = None
    question: str = ''
    options: list[str] = Field(default_factory=list)


class SendScope(BaseModel):
    """Concrete bounds; semantic categories such as 'safe messages' cannot authorize a send."""
    capability: str
    identity: Literal['owner', 'ferb']
    account: str | None = None
    site: str | None = None
    recipients: list[str] = Field(default_factory=list)
    content: str | None = ''    # None is explicit owner approval of any body; omitted never means a wildcard


class Plan(BaseModel):
    steps: list[Step]
    input_paths: list[str] = Field(default_factory=list)
    big_job: bool = False
    time_estimate: str = ''
    cost_estimate: str = ''
    standing_sends: list[SendScope] = Field(default_factory=list)


def inventory(paths: list[str]) -> list[dict[str, Any]]:
    """Stat the declared source files, without reading content or following directory symlinks.

    Bytes are a conservative token upper bound, useful without sending the input to a tokenizer/model.
    The caller applies the same file protections as fs.stat before invoking this helper.
    """
    found: dict[str, dict[str, Any]] = {}
    for value in paths:
        root = Path(value).expanduser()
        files = sorted(root.rglob('*')) if root.is_dir() else [root]
        for file in files:
            if file.is_symlink() or not file.is_file():
                continue
            path = str(file.resolve())
            size = file.stat().st_size
            found[path] = {'path': path, 'bytes': size, 'tokens_upper_bound': size}
    return list(found.values())


def problems(plan: Plan, facts: list[dict[str, Any]], *, context_tokens: int) -> list[str]:
    """Return concrete repair facts; never invent a split, a worker limit, or a ranking criterion."""
    errors: list[str] = []
    ids = [step.id for step in plan.steps]
    by_id = {step.id: step for step in plan.steps}
    if len(set(ids)) != len(ids) or any(not x or ':' in x for x in ids):
        errors.append('Step ids must be unique, nonempty and contain no colon.')
    for step in plan.steps:
        missing = set(step.depends_on) - set(ids)
        if missing:
            errors.append(f'{step.id}: missing dependencies {sorted(missing)}')
        if step.kind == 'reduce' and not step.depends_on:
            errors.append(f'{step.id}: reduce needs source steps')
        if step.kind == 'wait' and bool(step.until) == bool(step.event):
            errors.append(f'{step.id}: wait needs one exact time or event')
        if step.until:
            try:
                if datetime.fromisoformat(step.until.replace('Z', '+00:00')).tzinfo is None:
                    raise ValueError('missing timezone')
            except ValueError:
                errors.append(f'{step.id}: wait time must be an ISO timestamp with timezone')
        if step.kind == 'converse' and step.conversation is None and not step.depends_on:
            errors.append(f'{step.id}: converse needs an exact account, app/site, thread, recipients, topic and hand-over condition')
        if step.kind == 'ask' and not step.question.strip():
            errors.append(f'{step.id}: ask needs an owner-facing question')
    outstanding = {key: set(step.depends_on) & set(ids) for key, step in by_id.items()}
    ready = deque(key for key, dependencies in outstanding.items() if not dependencies)
    visited: set[str] = set()
    while ready:
        key = ready.popleft()
        if key in visited:
            continue
        visited.add(key)
        for other, dependencies in outstanding.items():
            if key in dependencies:
                dependencies.remove(key)
                if not dependencies:
                    ready.append(other)
    if set(ids) - visited:
        errors.append('dependency cycle: ' + ', '.join(sorted(set(ids) - visited)))
    known = {item['path']: item for item in facts}
    maps = [step for step in plan.steps if step.kind == 'map']
    assigned: list[str] = []
    for step in maps:
        if not step.batches:
            errors.append(f'{step.id}: map needs explicit batches')
        for batch in step.batches:
            if not batch:
                errors.append(f'{step.id}: empty batch')
            assigned.extend(batch)
            unknown = set(batch) - set(known)
            if unknown:
                errors.append(f'{step.id}: inputs absent from inventory: {sorted(unknown)}')
            tokens = sum(known[p]['tokens_upper_bound'] for p in batch if p in known)
            if tokens > context_tokens:
                errors.append(f'{step.id}: batch needs up to {tokens} tokens; worker context has {context_tokens}')
    if maps:
        missing = set(known) - set(assigned)
        if missing:
            errors.append(f'unassigned inputs: {sorted(missing)}')
        repeated = {p for p, count in Counter(assigned).items() if count > 1}
        if repeated:
            errors.append(f'inputs assigned more than once: {sorted(repeated)}')
    if plan.big_job and (not plan.time_estimate.strip() or not plan.cost_estimate.strip()):
        errors.append('A big job needs time and cost estimates before owner approval.')
    if not plan.steps:
        errors.append('A plan needs at least one step.')
    return errors
