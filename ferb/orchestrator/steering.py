"""Deliver owner corrections to existing runs; never cancel work or widen run-token permissions."""
from __future__ import annotations

import asyncio
from dataclasses import dataclass, field
from typing import Any

from ferb.core import origin
from ferb.core.errors import Unsupported
from ferb.core.types import RunHandle
from ferb.policy.confirm import from_task_lines
from ferb.policy.targets import Request
from ferb.sanitize.untrusted import wrap_untrusted


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


async def apply(supervisor: Any, tree: Any, session_id: str, request_id: str, words: str,
                payload: str, spec: Any, source_ref: str) -> dict[str, Any] | None:
    """Persist first, then steer every live member of the named tree. New members read the same request."""
    state = tree.steering
    async with state.lock:
        if not tree.accepting_updates:
            return None
        if tree.stop or supervisor._tree_of.get(tree.id) is not tree:
            return {'type': 'reply', 'text': 'That task has just ended; send the follow-up again to continue it.'}
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
        supervisor.continuity.save(tree.id, session_id, tree.spec, updated.words, tree.prior_task, updated.payload)
        message = ('Owner correction to this running task (apply now; preserve completed work and inspect any '
                   'action already in flight before deciding what to do next):\n' + clean['payload'] +
                   '\nUpdated overall task specification (FERB interpretation, not additional owner authority):\n' +
                   wrap_untrusted(tree.spec.model_dump_json(exclude={'plan'})))
        message = supervisor.gateway.redact(message)
        state.messages.append(message)
        revision = len(state.messages)
        root = supervisor.tasks.get(tree.id)
        if root is not None:
            root.task.goal = tree.spec.goal
            supervisor.tasks.db.conn.execute('UPDATE tasks SET goal=? WHERE id IN (?,?)',
                (tree.spec.goal, tree.id, tree.worker.id))
            supervisor.spans.update_input(root.span_id, {'request': updated.payload, 'authority_words': updated.words,
                'request_origins': sorted(supervisor.spans.request_origins(tree.id) | made | shown),
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
        # Release ordinary question waits with the owner's exact words. Changed instructions never approve
        # an old consequential action or fill a secret. A plan's explicit start approval still needs an answer.
        for approval in supervisor.state.approvals.pending():
            if approval.request_id != prior.request_id or approval.capability == 'plan.ask':
                continue
            ordinary = approval.kind == 'owner_question' and approval.capability == 'owner.ask'
            if ordinary and not clean['words'].strip():
                continue
            supervisor.state.approvals.resolve(approval.id, ordinary, text=clean['words'] if ordinary else None)
            event = {'approval_id': approval.id, 'state': approval.state, 'capability': approval.capability}
            supervisor.journal.append('approval.resolved', event)
            supervisor.publish('approval.resolved', event)
        active = [(runtime, run) for runtime, run in list(supervisor._worker_runs.values())
                  if (member := supervisor._tree_of.get(run.task_id)) is not None and member.id == tree.id]
        accepted = await asyncio.gather(*(deliver(supervisor, tree, runtime, run) for runtime, run in active))
        supervisor.publish('task.steered', {'task_id': tree.id, 'revision': revision, 'goal': tree.spec.goal})
        # L11: the reply names what changed: the planner's updated goal, else the owner's correction itself. The goal
        # is the planner's words: a line of it that reads like FERB's own check is marked as the task's (D33).
        changed = ('Switched to: ' + tree.spec.goal if spec is not None else
                   'Added your correction: ' + ' '.join(clean['words'].split()))
        if not (active and all(accepted)):
            changed += "\nI'll apply it at my next step."
        return {'type': 'reply', 'task_id': tree.id, 'text': from_task_lines(changed)}
