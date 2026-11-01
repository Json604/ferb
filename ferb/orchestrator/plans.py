"""D30/P15 plan vocabulary and mechanical feasibility checks; task choices belong to the planner."""

import asyncio
from pathlib import Path
from datetime import datetime
from collections import Counter, deque
from collections.abc import Callable
from typing import Any, Literal

from pydantic import BaseModel, Field, model_validator

from ferb.orchestrator.conversations import ConversationScope


class EventMatch(BaseModel):
    field: str
    value: str


class Repeat(BaseModel):
    """A `do` step's repeat rule: the step is a loop (L31). Between rounds code reads the round's probe, this event,
    the owner's corrections and a backup timer; only a verified `end_when` (the owner's own end condition) or the
    owner's stop ends it. An empty `end_when`: only the owner's stop."""
    end_when: str = ''
    event: str | None = None
    event_match: list[EventMatch] = Field(default_factory=list)


STEP_KINDS = ('do', 'wait', 'ask', 'converse')


class Step(BaseModel):
    """One step of a task's plan. A single task runs as a one-step plan of one `do` step. A `do` step may carry an
    input list (each input is covered by one note, exactly once across the plan) and a repeat rule (a loop)."""
    id: str
    kind: Literal['do', 'wait', 'ask', 'converse']
    goal: str = ''
    depends_on: list[str] = Field(default_factory=list)
    inputs: list[str] = Field(default_factory=list)
    worker_type: str = 'default'
    conditions: list[str] = Field(default_factory=list)
    until: str | None = None
    event: str | None = None
    event_match: list[EventMatch] = Field(default_factory=list)
    conversation: ConversationScope | None = None
    question: str = ''
    options: list[str] = Field(default_factory=list)
    repeat: Repeat | None = None

    @model_validator(mode='before')
    @classmethod
    def _earlier_kinds(cls, data: Any) -> Any:
        """Plans saved before step kinds were folded (stored plans, a step written in the old vocabulary): `reduce` is
        a `do` step; `map` is a `do` step whose input list is its batches' inputs; `loop` is a `do` step whose repeat
        rule carries its end condition and event."""
        if not isinstance(data, dict) or data.get('kind') not in ('map', 'reduce', 'loop'):
            return data
        data = dict(data)
        kind = data.pop('kind')
        data['kind'] = 'do'
        batches = data.pop('batches', None) or []
        if kind == 'map':
            data['inputs'] = list(dict.fromkeys([*data.get('inputs', []), *(p for batch in batches for p in batch)]))
        if kind == 'loop':
            data['repeat'] = {'end_when': data.pop('end_when', '') or '', 'event': data.pop('event', None),
                              'event_match': data.pop('event_match', [])}
        return data

    @property
    def loops(self) -> bool:
        """A `do` step with a repeat rule: a loop (L31)."""
        return self.kind == 'do' and self.repeat is not None

    @property
    def end_when(self) -> str:
        """A loop's end in the owner's words; '' = until the owner says stop (L31)."""
        return self.repeat.end_when if self.repeat is not None else ''

    @property
    def wakes_on(self) -> str | None:
        """The plan event this step waits for (a wait) or wakes on (a loop)."""
        return self.repeat.event if self.repeat is not None else self.event

    @property
    def wakes_match(self) -> list[EventMatch]:
        return self.repeat.event_match if self.repeat is not None else self.event_match


class SendScope(BaseModel):
    """Concrete bounds; semantic categories such as 'safe messages' cannot authorize a send."""
    capability: str
    identity: Literal['owner', 'ferb']
    account: str | None = None
    site: str | None = None
    recipients: list[str] = Field(default_factory=list)
    content: str | None = ''    # None is explicit owner approval of any body; omitted never means a wildcard

    def line(self) -> str:
        """The scope as FERB states it to the owner (D30), on one line."""
        return " ".join((f"Send using {self.identity}'s account {self.account or ''} at {self.site or ''}, to "
                         f"{', '.join(self.recipients) or self.site}, through {self.capability}: " +
                         ('any message body' if self.content is None else self.content)).split())


class Part(BaseModel):
    """One of the owner's asks in a message that asks for several things (ORCH-319), in the owner's
    order: not a plan step. Its words (`quote`, quote-checked), a short label for its row, and exactly one answer
    source: `direct` (answered in the first reply, no tools; "" when code's receipt for this turn's reminder, memory or
    schedule entry answers it) or `answer_step` (the step whose checked answer is the part's answer; SINGLE when the
    task has no plan). `conditions`: the indexes of the task's success conditions this part's answer must meet (each
    success condition belongs to exactly one part; a direct part has none)."""
    id: str
    quote: str
    label: str
    direct: str | None = None
    answer_step: str | None = None
    conditions: list[int] = Field(default_factory=list)


class Plan(BaseModel):
    steps: list[Step]
    input_paths: list[str] = Field(default_factory=list)
    big_job: bool = False
    time_estimate: str = ''
    cost_estimate: str = ''
    standing_sends: list[SendScope] = Field(default_factory=list)


async def inventory(paths: list[str], denied: Callable[[Path], object] | None = None) -> list[dict[str, Any]]:
    """Stat the declared source files, without reading content or following directory symlinks.

    Bytes are a conservative token upper bound, useful without sending the input to a tokenizer/model.
    The caller applies the same file protections as fs.stat to the roots before invoking this helper; a file found
    under them that `denied` names, or that macOS won't let FERB look at, is left out (a protected entry under
    /private/var/folders once ended the owner's turn). The walk runs in a worker thread: a home-sized tree must not hold
    the daemon's loop (RPCs, proxy callbacks, Stop) while it is stat'ed (L23).
    """
    return await asyncio.to_thread(_inventory, paths, denied)


def _inventory(paths: list[str], denied: Callable[[Path], object] | None) -> list[dict[str, Any]]:
    found: dict[str, dict[str, Any]] = {}
    for value in paths:
        root = Path(value).expanduser()
        try:
            files = sorted(root.rglob('*')) if root.is_dir() else [root]
        except OSError:                          # a root macOS won't let FERB look at is left out
            continue
        for file in files:
            try:
                if file.is_symlink() or not file.is_file():
                    continue
                path = str(file.resolve())
                if denied is not None and denied(Path(path)):
                    continue
                size = file.stat().st_size
            except OSError:      # macOS won't let FERB look at it (EPERM/EACCES), or it went: left out, as protected
                continue
            found[path] = {'path': path, 'bytes': size, 'tokens_upper_bound': size}
    return list(found.values())


def single(goal: str) -> Plan:
    """A single task as the plan it runs as: one `do` step (the task's own worker, checked against the task's own
    conditions)."""
    return Plan(steps=[Step(id=SINGLE, kind='do', goal=goal)])


SINGLE = 'worker'          # the one step of a single task's plan (its worker's task id is "<tree>.worker")
DONE = ('done', 'done_unverified')                        # a step that finished (confirmed, or done but unconfirmed)
ENDED = DONE + ('failed', 'blocked', 'cancelled', 'needs_user')   # never run again without the owner (reopen)


def card(parts: list[Part]) -> bool:
    """Several asks: the turn opens a card with a row per part that isn't answered directly. One ask (no parts, or one
    part) answers as a single task always did: one answer, no card, no count."""
    return len(parts) >= 2


def answer_steps(plan: Plan | None, parts: list[Part]) -> list[str]:
    """The steps whose checked answers are the task's answer (ORCH-319: no summing-up call after them): each part's
    answer step, in the owner's order; with no part naming one, a single task's one step, or a plan's last step (the one
    no other step depends on; a plan saved before step 8d may end in several)."""
    named = [p.answer_step for p in parts if p.answer_step]
    if named:
        return named
    if plan is None:
        return [SINGLE]
    needed = {d for step in plan.steps for d in step.depends_on}
    return [step.id for step in plan.steps if step.id not in needed]


def part_problems(parts: list[Part], plan: Plan | None, conditions: int) -> list[str]:
    """Structural facts about the planner's parts (ORCH-319), told back for repair; the quote check is the
    supervisor's (`memory.items.quoted`, against the owner's typed words). Each part has exactly one answer source; an
    answer step is a step of the plan (SINGLE without one) that isn't a wait or an ask, and answers one part only;
    every success condition belongs to exactly one part, a direct part has none; every step leads to a part's answer
    (so no step sums up or combines answers already posted). Without parts, a plan ends in one step whose checked answer
    is the answer."""
    errors: list[str] = []
    steps = {step.id: step for step in plan.steps} if plan is not None else {}
    if not parts:
        if plan is not None and len(answer_steps(plan, parts)) != 1:
            errors.append('A plan for one ask ends in exactly one step (no other step depends on it): its checked '
                          'answer is the answer, and every other step leads to it. For several asks, list each in '
                          'parts with its own answer_step.')
        return errors
    ids = [p.id for p in parts]
    if len(set(ids)) != len(ids) or any(not i.strip() or ':' in i for i in ids):
        errors.append('Part ids must be unique, nonempty and contain no colon.')
    if all(p.direct is not None for p in parts):
        errors.append('Every part is answered directly: answer with a reply instead of a task.')
    owners: dict[int, list[str]] = {}
    answering: dict[str, list[str]] = {}
    for p in parts:
        if (p.direct is None) == (p.answer_step is None):
            errors.append(f'part {p.id}: give exactly one of direct (answered now) or answer_step.')
        if not p.label.strip():
            errors.append(f'part {p.id}: needs a short label for its row.')
        if p.answer_step is not None:
            answering.setdefault(p.answer_step, []).append(p.id)
            if plan is None and p.answer_step != SINGLE:
                errors.append(f'part {p.id}: a task without a plan answers with its one step, answer_step '
                              f'"{SINGLE}".')
            elif plan is not None and p.answer_step not in steps:
                errors.append(f'part {p.id}: answer_step {p.answer_step!r} is not a step of the plan.')
            elif plan is not None and steps[p.answer_step].kind in ('wait', 'ask'):
                errors.append(f"part {p.id}: a {steps[p.answer_step].kind} step can't answer a part.")
            if not p.conditions:
                errors.append(f'part {p.id}: name the success conditions its answer must meet (their indexes).')
        elif p.conditions:
            errors.append(f'part {p.id}: a part answered directly has no success conditions.')
        for index in p.conditions:
            owners.setdefault(index, []).append(p.id)
    for step_id, named in answering.items():
        if len(named) > 1:
            errors.append(f'step {step_id} answers several parts ({", ".join(named)}): each part has its own '
                          'answer step.')
    for index, named in sorted(owners.items()):
        if not 0 <= index < conditions:
            errors.append(f"parts {', '.join(named)}: success condition {index} doesn't exist.")
        elif len(named) > 1:
            errors.append(f'success condition {index} belongs to several parts ({", ".join(named)}).')
    if missing := [i for i in range(conditions) if i not in owners]:
        errors.append(f'success conditions {missing} belong to no part: every condition belongs to exactly one.')
    if plan is not None:
        leads = set(answering) & set(steps)
        grew = True
        while grew:                                  # every step an answer step depends on, transitively
            before = len(leads)
            leads |= {d for key in list(leads) for d in steps[key].depends_on if d in steps}
            grew = len(leads) > before
        if idle := [key for key in steps if key not in leads]:
            errors.append(f"steps {idle} lead to no part's answer: each part's answer is posted on its own once its "
                          "answer step is checked, so no step sums up or combines them (FERB adds the done count).")
    return errors


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
        if step.repeat is not None and step.kind != 'do':
            errors.append(f'{step.id}: only a do step repeats')
        if step.inputs and step.kind != 'do':
            errors.append(f'{step.id}: only a do step takes an input list')
        if step.loops and not step.goal.strip():
            errors.append(f'{step.id}: a repeating step needs a goal')
        if step.loops and step.until:
            errors.append(f"{step.id}: a repeating step's end goes in repeat.end_when; until is a wait's time")
        for dependency in step.depends_on:
            source = by_id.get(dependency)
            if source is not None and source.loops and not source.end_when.strip():
                errors.append(f'{step.id}: depends on repeating step {dependency}, which runs until the owner says '
                              'stop, so it could never start')
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
    # An input list: every inventoried input is covered by exactly one do step, each step's inputs fit its worker
    known = {item['path']: item for item in facts}
    listed = [step for step in plan.steps if step.inputs]
    assigned: list[str] = []
    for step in listed:
        assigned.extend(step.inputs)
        unknown = set(step.inputs) - set(known)
        if unknown:
            errors.append(f'{step.id}: inputs absent from inventory: {sorted(unknown)}')
        tokens = sum(known[p]['tokens_upper_bound'] for p in step.inputs if p in known)
        if tokens > context_tokens:
            errors.append(f'{step.id}: its inputs need up to {tokens} tokens; worker context has {context_tokens}')
    if listed:
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
