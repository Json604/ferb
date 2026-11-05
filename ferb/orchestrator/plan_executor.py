"""Dependency execution with durable step state and waits; all actions still use ordinary workers/gateway. One path for
every task: a single task is a one-step plan (plans.single)."""
import asyncio
from collections.abc import Awaitable, Callable
from datetime import datetime, timezone
from typing import Any

from ferb.clock.timers import recheck_in
from ferb.orchestrator.plans import DONE, ENDED, Step
from ferb.orchestrator.plan_store import PlanStore

Execute = Callable[[Step, list[str], dict[str, Any]], Awaitable[dict[str, Any]]]
# The plan runner's step lifecycle (ORCH-531): (task id, step id, status, facts) to the supervisor's one step-state path.
OnStep = Callable[[str, str, str, dict[str, Any]], None]

# How a step that ended reads in the lifecycle: finished, couldn't be done, or ended without finishing.
_ENDED_AS = {'done': 'done', 'done_unverified': 'done', 'failed': 'failed', 'blocked': 'failed',
             'cancelled': 'not_finished', 'needs_user': 'not_finished'}


def _utc_now() -> datetime:
    return datetime.now(timezone.utc)


class PlanExecutor:
    """Runs a stored plan: every step whose dependencies finished (done, or done but unconfirmed), independent steps
    at once. A step that ended without finishing (failed, blocked, cancelled, or needs the owner in a run nobody
    attends) is never launched again by this run; only the owner's continue reopens it (PlanStore.reopen). `stopped`
    is the supervisor's one stop check; `now` its injected clock (waits are on the wall clock). `on_step` hears each
    step's lifecycle (ORCH-531): waiting (on the steps it depends on, or a wait step's time or event), starting, and how
    it ended (done, failed, not_finished); a step whose dependency ended unfinished, or can never run because of one,
    is `wont_run` at once."""

    def __init__(self, store: PlanStore, execute: Execute, stopped: Callable[[], bool],
                 now: Callable[[], datetime] = _utc_now, on_step: OnStep | None = None) -> None:
        self.store, self.execute, self.stopped, self.now = store, execute, stopped, now
        self.on_step = on_step
        self._wake = asyncio.Event()

    def _tell(self, task_id: str, step: Step, status: str, **facts: Any) -> None:
        if self.on_step is not None:
            self.on_step(task_id, step.id, status, {'loop': step.loops, **facts})

    def wake(self) -> None:
        self._wake.set()

    async def _wait(self, task_id: str, step: Step) -> dict[str, Any]:
        state = self.store.steps(task_id)[step.id]
        cursor = state['output'].get('event_cursor', self.store.event_cursor())
        self.store.finish(task_id, step.id, {'status': 'waiting', 'event_cursor': cursor})
        self._tell(task_id, step, 'waiting', **({'event': step.event} if step.event else {'until': step.until}))
        while not self.stopped():
            self._wake.clear()
            if step.event:
                payload = self.store.event_after(step.event, cursor, step.event_match)
                if payload is not None:
                    return {'status': 'done', 'summary': step.goal, 'event': payload}
                await self._wake.wait()
            else:
                at = datetime.fromisoformat(str(step.until).replace('Z', '+00:00'))
                if at.tzinfo is None:
                    raise ValueError('A wait time must include its timezone')
                now = self.now()
                if at <= now:
                    return {'status': 'done', 'summary': step.goal}
                try:                               # on the wall clock, like durable timers: sleep pauses monotonic
                    await asyncio.wait_for(self._wake.wait(), recheck_in(at, now))
                except TimeoutError:
                    pass
        return {'status': 'cancelled', 'summary': 'stopped by you'}

    async def _step(self, task_id: str, step: Step, prior: dict[str, Any]) -> dict[str, Any]:
        self.store.begin(task_id, step.id)
        self._tell(task_id, step, 'starting')
        try:
            result = await (self._wait(task_id, step) if step.kind == 'wait' else self.execute(step, step.inputs, prior))
        except asyncio.CancelledError:
            self.store.begin(task_id, step.id, 'interrupted')
            raise
        except Exception as exc:
            result = {'status': 'failed', 'summary': f'{type(exc).__name__}: {exc}'}
        self.store.finish(task_id, step.id, result)
        self._tell(task_id, step, _ENDED_AS.get(str(result.get('status')), 'failed'))
        return result

    def _blocked(self, plan_steps: list[Step], states: dict[str, dict[str, Any]]) -> dict[str, list[str]]:
        """Steps that can never run in this run, each with the dependencies that stop it: one that ended unfinished, or
        one that is itself blocked."""
        ended = {key for key, state in states.items() if state['status'] in ENDED and state['status'] not in DONE}
        blocked: dict[str, list[str]] = {}
        changed = True
        while changed:
            changed = False
            for step in plan_steps:
                if step.id in blocked or states[step.id]['status'] in ENDED:
                    continue
                stops = [d for d in step.depends_on if d in ended or d in blocked]
                if stops:
                    blocked[step.id] = stops
                    changed = True
        return blocked

    async def run(self, task_id: str) -> dict[str, Any]:
        plan = self.store.load(task_id)['plan']
        jobs: dict[asyncio.Task[dict[str, Any]], str] = {}
        self.store.set_status(task_id, 'running')
        try:
            while not self.stopped():
                states = self.store.steps(task_id)
                unfinished = [step for step in plan.steps if states[step.id]['status'] not in DONE]
                if not unfinished:
                    return {'status': 'done', 'steps': states}
                active = set(jobs.values())
                blocked = self._blocked(plan.steps, states)
                for step in unfinished:
                    if step.id in active or states[step.id]['status'] in ENDED:
                        continue
                    if step.id in blocked:
                        self._tell(task_id, step, 'wont_run', waits_on=blocked[step.id])
                    elif all(states[d]['status'] in DONE for d in step.depends_on):
                        prior = {d: states[d]['output'] for d in step.depends_on}
                        prior['previous_attempt'] = states[step.id]['output']
                        jobs[asyncio.create_task(self._step(task_id, step, prior))] = step.id
                    else:
                        self._tell(task_id, step, 'waiting',
                                   waits_on=[d for d in step.depends_on if states[d]['status'] not in DONE])
                if not jobs:
                    waiting = any(states[s.id]['status'] == 'needs_user' for s in unfinished)
                    return {'status': 'needs_user' if waiting else 'blocked', 'steps': states}
                completed, _ = await asyncio.wait(jobs, return_when=asyncio.FIRST_COMPLETED)
                for job in completed:
                    jobs.pop(job)
                    await job
            return {'status': 'cancelled', 'steps': self.store.steps(task_id)}
        finally:
            for job in jobs:
                job.cancel()
            await asyncio.gather(*jobs, return_exceptions=True)
