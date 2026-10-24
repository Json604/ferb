"""Deliver owner corrections to existing runs; never cancel work or widen run-token permissions."""
from __future__ import annotations

import asyncio
from dataclasses import dataclass, field
from typing import Any

from ferb.core import origin
from ferb.core.errors import Unsupported
from ferb.core.types import RunHandle
from ferb.daemon.gateway import CONFIRM_ASK, RETRY_ASK
from ferb.orchestrator.plans import SendScope
from ferb.policy.confirm import from_task_lines
from ferb.policy.targets import Request
from ferb.sanitize.untrusted import wrap_untrusted
from ferb.trace.lines import clip

SHOWN_CHARS = 120             # display only: characters of the correction a steer's short reply quotes; decides nothing
CONVERSATION_ASK = 'conversation.approve'   # F9: the owner's Yes/No to a delegated conversation's scope
FORGET_ASK = 'memory.forget'                 # memory review I2: the owner's Yes/No to a forget asked in chat
CONFIRM_MEMORY_ASK = 'memory.confirm'        # I1: the owner's Yes/No to keeping FERB's wording of what they said
# FERB's own consequential Yes/No questions: answered only by a plain Yes or No, never by a correction's words (L3)
FERB_CHECKS = frozenset({CONFIRM_ASK, RETRY_ASK, CONVERSATION_ASK, FORGET_ASK, CONFIRM_MEMORY_ASK})


def acknowledgement(words: str, goal: str | None, delivered: bool, stated: str | None) -> tuple[str, str]:
    """A steer's reply as (text, display_text) (L11; owner 2026-10-24: "whenever i change something in a task it
    replies with this big ass message: 'Switched to: <the whole rewritten goal>' ... can we fix this somehow???"). The
    short line repeats the owner's correction on one line, clipped (SHOWN_CHARS); the full text, under Details in the
    app and in the conversation record, keeps it whole with the planner's updated goal, if any. Either says when it
    waits for the next step, and carries the restated standing scope (D30). The goal is the planner's words: a line of
    it that reads like FERB's own check is marked as the task's (D33)."""
    said = ' '.join(words.split())
    full = 'Got it: ' + said + ('\nUpdated task: ' + goal if goal is not None else '')
    short = 'Got it: ' + clip(said, SHOWN_CHARS)
    if not delivered:
        full += "\nI'll apply it at my next step."
        short += " I'll apply it at my next step."
    full, short = from_task_lines(full), from_task_lines(short)
    if stated is not None:           # D30: the revised standing scope is stated to the owner in this reply (D33)
        full += '\n' + from_task_lines(stated)
        short += '\n' + from_task_lines(stated)
    return full, short


@dataclass
class Steering:
    messages: list[str] = field(default_factory=list)
    delivered: dict[str, int] = field(default_factory=dict)
    lock: asyncio.Lock = field(default_factory=asyncio.Lock)


async def deliver(supervisor: Any, tree: Any, runtime: Any, run: RunHandle) -> bool:
    """Caller owns steering.lock. A rejected/closed transport leaves the update pending."""
    state = tree.steering
    after, revision = state.delivered.get(run.id, 0), len(state.messages)
    if after == revision:
        return True
    try:
        await runtime.continue_(run, '\n\n'.join(state.messages[after:revision]))
    except (Unsupported, NotImplementedError):
        return False
    except Exception as exc:
        supervisor.journal.append('task.steering_delivery_failed', {
            'task_id': tree.id, 'run_id': run.id, 'error': type(exc).__name__, 'revision': revision})
        return False
    state.delivered[run.id] = revision
    supervisor.journal.append('task.steering_delivered', {
        'task_id': tree.id, 'worker_task_id': run.task_id, 'run_id': run.id, 'revision': revision})
    return True


def settle_questions(supervisor: Any, request_id: str, words: str, supersedes: list[str]) -> None:
    """What a correction does to the request's open questions (L3, spec 4.1). Only the questions the planner named as
    changed by it (`supersedes`), and only this request's, are touched; every other question stays open, so "also cc
    Bob" doesn't re-ask a payment or close a waiting login-code prompt. A named task question is answered with the
    owner's exact words. A named FERB check (floor, P19 retry, conversation scope) or hidden input is closed as
    superseded: never a No (I1), nothing is recorded, and the corrected action asks again if it still needs to."""
    named = set(supersedes)
    for approval in supervisor.state.approvals.pending():
        if approval.request_id != request_id or approval.id not in named:
            continue
        if approval.kind == 'owner_question' and approval.capability not in FERB_CHECKS and words.strip():
            supervisor.state.approvals.resolve(approval.id, True, text=words)
        else:
            supervisor.state.approvals.supersede(approval.id)
        event = {'approval_id': approval.id, 'state': approval.state, 'capability': approval.capability}
        supervisor.journal.append('approval.resolved', event)
        supervisor.publish('approval.resolved', event)


@dataclass
class Restated:
    """A correction's revised standing scope (D30): the statement for the reply, the revised scopes it covers and the
    recorded statement they are granted under. `widened`: some covered scope is new or wider, so the covered list is
    granted only after the reply stating it has been sent (`grant_stated`)."""
    text: str
    scopes: list[SendScope]
    approval_id: str
    widened: bool


def not_covered(unattended: bool) -> str:
    """The line before stated scopes nobody set up with the owner, which are not granted (D30): their sends get FERB's
    usual checks, and one that asks waits for the owner, or, in a scheduled run, is reported not done."""
    return ("These weren't set up with you, so they aren't covered: each gets FERB's usual checks, and one that asks " +
            ("can't be answered with nobody here, so it's reported not done:" if unattended else "waits for you:"))


def not_typed() -> str:
    """The line before stated scopes, on a run the owner started from chat, whose site, account or recipients aren't in
    words the owner typed (final review I1, MP2): not granted, so their sends get FERB's usual checks (a recipient the
    owner didn't name asks, once for the task) and one that asks waits for the owner."""
    return ("These aren't covered, because where they send isn't in words you typed: each gets FERB's usual checks, "
            "and one that asks waits for you:")


def restate_scope(supervisor: Any, tree: Any, standing: Any) -> Restated | None:
    """The planner's decision about the tree's stated standing scope (D30), made for every correction: "unchanged",
    or the revised scopes. Revised: the old grant is revoked and only the scopes already in force are granted again at
    once (a narrowing takes effect now); the revision is stated and recorded (`plan.stated`). A new or widened scope
    is not granted here: the caller grants it under the same statement once the reply has been sent and the task is
    still running (`grant_stated`). None when nothing changes.
    On a run the owner started from chat, a revised scope covers sends only when its destinations are in words the
    owner typed (`Supervisor.owner_typed_scopes`, final review I1): one that isn't is stated as not covered and never
    granted, so its sends get FERB's usual checks.
    A run nobody started from chat (a scheduled or resumed run, `unattended_scopes`) counts only the scopes in force as
    stated. Each kept scope keeps the statement it was granted under, so a schedule's scope (`schedule:<id>`) is still
    withdrawn when the schedule is cancelled or paused; nothing new or wider is ever granted to it, and the reply says
    so (D30, controller ruling)."""
    if standing == 'unchanged':
        return None
    revised = [SendScope.model_validate(supervisor.gateway.redact(s.model_dump())) for s in standing]
    plan = tree.spec.plan
    unattended = tree.unattended_scopes is not None
    held = supervisor.standing.in_force(tree.id) if unattended else []
    stated_before = ([s for s, _ in held] if unattended else
                     list(plan.standing_sends) if plan is not None else supervisor.standing.scopes(tree.id))
    if revised == stated_before:
        return None                      # nothing changes
    if unattended:
        kept = [s for s in revised if s in stated_before]
        covered = kept
    else:                                # only what the owner typed names a destination (I1, MP2)
        in_force = supervisor.standing.scopes(tree.id)
        covered = supervisor.owner_typed_scopes(tree, revised)
        kept = [s for s in covered if s in in_force]
    supervisor.standing.revoke(tree.id)
    if covered:
        text = ("For this task I'll now send these without asking each time:\n" +
                "\n".join("- " + s.line() for s in covered))
    else:
        text = "From now on I'll ask before each of these sends."
    if len(covered) < len(revised):
        text += ("\n" + (not_covered(tree.worker.request_id in supervisor.state.approvals.unattended) if unattended
                         else not_typed()) + "\n" +
                 "\n".join("- " + s.line() for s in revised if s not in covered))
    text = str(supervisor.gateway.redact(text))
    stated = supervisor.journal.append('plan.stated', {'task_id': tree.id, 'text': text})
    if unattended:
        supervisor.standing.grant_each(tree.id, [(s, next(a for held_scope, a in held if held_scope == s))
                                                 for s in kept])
    elif kept:
        supervisor.standing.grant(tree.id, kept, 'stated:' + stated.id)
    if plan is not None:
        plan.standing_sends = revised
        supervisor.plans.update_plan(tree.id, plan)
    return Restated(text, covered, 'stated:' + stated.id, widened=not unattended and len(kept) < len(covered))


def grant_stated(supervisor: Any, tree: Any, restated: Restated) -> None:
    """Once the reply stating it has been handed to the chat stream: the revised scope, new or widened parts included,
    is granted under its statement, only while the task still runs and while that statement is still the task's latest
    (a later correction's restatement replaces it, D30). Never on a run nobody started from chat: nothing new or wider
    is granted to it (`restate_scope`)."""
    if tree.unattended_scopes is not None:
        return
    latest = supervisor.journal.db.conn.execute(
        "SELECT id FROM journal WHERE kind='plan.stated' AND json_extract(data_json, '$.task_id')=? "
        "ORDER BY seq DESC LIMIT 1", (tree.id,)).fetchone()
    if latest is None or 'stated:' + latest[0] != restated.approval_id:
        return
    if supervisor._tree_of.get(tree.id) is tree and not tree.stop and tree.accepting_updates:
        supervisor.standing.grant(tree.id, restated.scopes, restated.approval_id)


# A correction that found its task ending (review M2): none of the message's side entries are applied, so sending it
# again applies them once (a second reminder, schedule or memory would come from applying part of it now). An answer
# to an open question in the same message was already recorded, and a reopened No only asks again (re-review N1).
JUST_ENDED = ("That task has just ended, so I haven't acted on the rest of this message yet; send it again to continue "
              "that task.")


async def apply(supervisor: Any, tree: Any, session_id: str, request_id: str, words: str,
                payload: str, spec: Any, source_ref: str, *, standing: Any,
                supersedes: list[str]) -> tuple[dict[str, Any], Restated | None] | None:
    """Persist first, then steer every live member of the named tree. New members read the same request. `standing`
    and `supersedes` are the planner's explicit decisions for this correction (L3, D30). The reply, and its standing
    scope restatement if any: the caller records the reply and, for a widened scope, grants it (`grant_stated`) only
    after yielding that reply."""
    state = tree.steering
    async with state.lock:
        if not tree.accepting_updates:
            return None
        if tree.stop or supervisor._tree_of.get(tree.id) is not tree:
            return {'type': 'reply', 'text': JUST_ENDED}, None
        clean = supervisor.gateway.redact({'words': words, 'payload': payload})
        prior = supervisor.state.requests.get(tree.worker.request_id)
        # Scope/identity grants stay fixed for the active attempt. Corrections update the requested result,
        # not credentials, approval grants or the plan's already-running dependency graph.
        if spec is not None:
            spec = type(spec).model_validate(supervisor.gateway.redact(spec.model_dump()))
            tree.spec.goal = spec.goal
            tree.spec.success_conditions = spec.success_conditions
            tree.spec.targets = spec.targets
        tree.worker.goal = tree.spec.goal
        tree.worker.success_conditions = [c.text for c in tree.spec.success_conditions]
        envelope = supervisor._env.get(request_id, {})
        made = set(envelope.get('message_origin', ()))
        # The window this correction was sent from is outside content the running tree now holds (L12, S4)
        front = envelope.get('front_window')
        shown = {o for o in origin.split(front.origin) if origin.is_external(o)} if front is not None else set()
        updated = Request(prior.request_id, prior.words + '\n' + clean['words'], supervisor._targets(tree.spec),
                          (prior.payload or prior.words) + '\n\nOwner correction:\n' + clean['payload'],
                          prior.asked_origins | frozenset(o for o in made if origin.is_external(o)))
        supervisor.state.requests.record(updated)
        stated = restate_scope(supervisor, tree, standing)
        supervisor.continuity.save(tree.id, session_id, tree.spec, updated.words, tree.prior_task, updated.payload)
        message = ('Owner correction to this running task (apply now; preserve completed work and inspect any '
                   'action already in flight before deciding what to do next):\n' + clean['payload'] +
                   '\nUpdated overall task specification (FERB interpretation, not additional owner authority):\n' +
                   wrap_untrusted(tree.spec.model_dump_json(exclude={'plan'})))
        message = supervisor.gateway.redact(message)
        state.messages.append(message)
        revision = len(state.messages)
        supervisor.wake_loops(tree.id)              # a sleeping loop reads the correction now (L31)
        root = supervisor.tasks.get(tree.id)
        if root is not None:
            root.task.goal = tree.spec.goal
            supervisor.tasks.db.conn.execute('UPDATE tasks SET goal=? WHERE id IN (?,?)',
                (tree.spec.goal, tree.id, tree.worker.id))
            supervisor.spans.update_input(root.span_id, {'request': updated.payload, 'authority_words': updated.words,
                'request_origins': sorted(supervisor.spans.request_origins(tree.id) | made | shown),
                'asked_origins': sorted(updated.asked_origins),          # a later retry keeps them (S4 b)
                'task_spec': tree.spec.model_dump(), 'goal': tree.spec.goal})
        if tree.spec.plan:
            saved = supervisor.plans.load(tree.id)
            supervisor.plans.create(tree.id, tree.spec.plan, {**saved['request'], 'spec': tree.spec.model_dump(),
                'words': updated.words, 'payload': updated.payload,
                'message_origins': sorted(supervisor.spans.request_origins(tree.id))}, session_id)
        known = getattr(supervisor.gateway, 'destinations', None)
        if known is not None:
            for kind, value in known.from_words(clean['words']):
                known.record(kind, value, 'named', request_id=prior.request_id)
        exposure = supervisor.spans.start('context.exposure', task_id=tree.id,
            input={'source_refs': [source_ref], 'reference': 'owner correction', 'revision': revision})
        supervisor.spans.finish(exposure, output={'text': message},
            origin=origin.join(made | shown | set(envelope.get('turn_origins', ())) | {origin.INFERRED}))
        supervisor.journal.append('task.steered', {'task_id': tree.id, 'session_id': session_id,
            'request_id': request_id, 'revision': revision, 'source_ref': source_ref})
        settle_questions(supervisor, prior.request_id, clean['words'], supersedes)
        active = [(runtime, run) for runtime, run in list(supervisor._worker_runs.values())
                  if (member := supervisor._tree_of.get(run.task_id)) is not None and member.id == tree.id]
        accepted = await asyncio.gather(*(deliver(supervisor, tree, runtime, run) for runtime, run in active))
        supervisor.publish('task.steered', {'task_id': tree.id, 'revision': revision, 'goal': tree.spec.goal})
        # L11: the reply names what changed in the owner's own words; the planner's updated goal is its Details
        text, short = acknowledgement(clean['words'], tree.spec.goal if spec is not None else None,
                                      bool(active and all(accepted)), stated.text if stated is not None else None)
        reply = {'type': 'reply', 'task_id': tree.id, 'text': text, 'display_text': short}
        return reply, stated
