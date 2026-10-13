"""Dependency execution with durable map coverage and waits; all actions still use ordinary workers/gateway."""
import asyncio
from collections.abc import Awaitable, Callable
from datetime import datetime, timezone
from typing import Any

from ferb.clock.timers import recheck_in
from ferb.orchestrator.plans import Step
from ferb.orchestrator.plan_store import PlanStore

Execute = Callable[[Step, list[str], dict[str, Any]], Awaitable[dict[str, Any]]]


class PlanExecutor:
    def __init__(self, store: PlanStore, execute: Execute, stopped: Callable[[], bool]) -> None:
        self.store, self.execute, self.stopped = store, execute, stopped
        self._wake = asyncio.Event()

    def notify(self, event: str, payload: dict[str, Any]) -> None:
        self.store.event(event, payload)
        self._wake.set()

    def wake(self) -> None:
        self._wake.set()

    async def _wait(self, task_id: str, step: Step) -> dict[str, Any]:
        state = self.store.steps(task_id)[step.id]
        cursor = state['output'].get('event_cursor', self.store.event_cursor())
        self.store.finish(task_id, step.id, {'status': 'waiting', 'event_cursor': cursor})
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
                now = datetime.now(timezone.utc)
                if at <= now:
                    return {'status': 'done', 'summary': step.goal}
                try:                               # on the wall clock, like durable timers: sleep pauses monotonic
                    await asyncio.wait_for(self._wake.wait(), recheck_in(at, now))
                except TimeoutError:
                    pass
        return {'status': 'cancelled', 'summary': 'stopped by you'}

    async def _one(self, task_id: str, step: Step, inputs: list[str], prior: dict[str, Any]) -> dict[str, Any]:
        self.store.begin(task_id, step.id)
        try:
            result = await (self._wait(task_id, step) if step.kind == 'wait' else self.execute(step, inputs, prior))
        except asyncio.CancelledError:
            self.store.begin(task_id, step.id, 'interrupted')
            raise
        except Exception as exc:
            result = {'status': 'failed', 'summary': f'{type(exc).__name__}: {exc}'}
        self.store.finish(task_id, step.id, result)
        return result

    async def _step(self, task_id: str, step: Step, prior: dict[str, Any]) -> dict[str, Any]:
        if step.kind != 'map':
            return await self._one(task_id, step, step.inputs, prior)
        self.store.begin(task_id, step.id)
        states = self.store.steps(task_id)
        pending = []
        for index, batch in enumerate(step.batches):
            key = f'{step.id}:{index}'
            if states[key]['status'] != 'done':
                unit = step.model_copy(update={'id': key, 'kind': 'do', 'inputs': batch, 'batches': []})
                pending.append(self._one(task_id, unit, batch, {**prior, 'previous_attempt': states[key]['output']}))
        if pending:
            await asyncio.gather(*pending)
        states = self.store.steps(task_id)
        results = [states[f'{step.id}:{i}']['output'] for i in range(len(step.batches))]
        assigned = [p for batch in step.batches for p in batch]
        notes = [n for result in results for n in result.get('input_notes', [])]
        covered = [n.get('input') for n in notes if n.get('note') and n.get('evidence')]
        missing = [p for p in assigned if covered.count(p) != 1]
        good = all(r.get('status') == 'done' for r in results) and not missing
        result = {'status': 'done' if good else 'blocked', 'summary': step.goal if good else
                  f'Cannot reduce yet: missing or duplicate evidence notes for {missing}',
                  'input_notes': notes, 'batches': results,
                  'conversation_proposals': [proposal for result in results for proposal in result.get('conversation_proposals', [])]}
        self.store.finish(task_id, step.id, result)
        return result

    async def run(self, task_id: str) -> dict[str, Any]:
        plan = self.store.load(task_id)['plan']
        jobs: dict[asyncio.Task[dict[str, Any]], str] = {}
        self.store.set_status(task_id, 'running')
        try:
            while not self.stopped():
                states = self.store.steps(task_id)
                unfinished = [step for step in plan.steps if states[step.id]['status'] != 'done']
                if not unfinished:
                    self.store.set_status(task_id, 'steps_done')
                    return {'status': 'done', 'steps': states}
                active = set(jobs.values())
                for step in unfinished:
                    if step.id in active or states[step.id]['status'] in ('failed', 'blocked', 'cancelled'):
                        continue
                    if all(states[d]['status'] == 'done' for d in step.depends_on):
                        prior = {d: states[d]['output'] for d in step.depends_on}
                        prior['previous_attempt'] = states[step.id]['output']
                        jobs[asyncio.create_task(self._step(task_id, step, prior))] = step.id
                if not jobs:
                    self.store.set_status(task_id, 'blocked')
                    return {'status': 'blocked', 'steps': states}
                completed, _ = await asyncio.wait(jobs, return_when=asyncio.FIRST_COMPLETED)
                for job in completed:
                    jobs.pop(job)
                    await job
            self.store.set_status(task_id, 'cancelled')
            return {'status': 'cancelled', 'steps': self.store.steps(task_id)}
        except asyncio.CancelledError:
            self.store.set_status(task_id, 'suspended' if not self.stopped() else 'cancelled')
            raise
        finally:
            for job in jobs:
                job.cancel()
            await asyncio.gather(*jobs, return_exceptions=True)
