"""L31 test support: a scripted loop. Rounds return queued results; the probe returns queued (value, error) pairs;
time is a counter the test advances; sleeps advance it instantly."""
import asyncio
from typing import Any

from ferb.clock.timers import POLL_S
from ferb.orchestrator.loops import LoopRunner, LoopStore
from ferb.orchestrator.plans import Step
from ferb.orchestrator.probes import MISSING, Probe


def probe(**k: Any) -> Probe:
    return Probe(**({'tool': 'test.state', 'args_json': '{}', 'field': 'playing', 'op': 'equals',
                     'value_json': 'false'} | k))


def round_result(action: str | None = 'continue', p: Probe | None = None, status: str = 'done',
                 finished_verified: bool = False, summary: str = 'Played it.') -> dict[str, Any]:
    return {'status': status, 'summary': summary, 'evidence': ['seen'], 'loop_action': action, 'probe': p,
            'finished_verified': finished_verified, 'question': ''}


class Harness:
    def __init__(self, db: Any, clock: Any, step: Step, rounds: list[dict[str, Any]],
                 reads: list[tuple[Any, str | None]], refuse: str | None = None) -> None:
        self.t = 1_000_000.0
        self.rounds, self.reads, self.refuse = list(rounds), list(reads), refuse
        self.reasons: list[dict[str, Any]] = []
        self.round_times: list[float] = []                 # when each round started, on the harness clock
        self.notices: list[str] = []
        self.statuses: list[dict[str, Any]] = []
        self.records: list[tuple[str, Any, str | None]] = []
        self.steer_count, self.stop = 0, False
        self.question: Any = None             # the owner's question as the runner sees it (ask, answer, close)
        self.events: list[tuple[int, dict[str, Any]]] = []
        self.store = LoopStore(db, clock)
        self.runner = LoopRunner(
            store=self.store, task_id='tree', step=step, round=self._round, arm=lambda p: self.refuse,
            probe=self._probe, record=lambda kind, p, v, e, o, n: self.records.append((kind, v, e)),
            event=(self._event if step.wakes_on else None), steers=lambda: self.steer_count, notify=self._notify,
            status=self.statuses.append, stopped=lambda: self.stop, now=lambda: self.t, sleep=self._sleep,
            wait_owner=self._wait_owner)

    async def _wait_owner(self, result: dict[str, Any] | None) -> dict[str, Any] | None:
        """The supervisor's one wait for the owner, as the runner sees it: the question asked once, then the owner's
        reply in chat (a correction), their answer, or the stop; the question closed when the wait ends."""
        q = self.question
        if q is not None:
            await q.ask(result)
        try:
            while not self.stop:
                if self.runner._steered():
                    return {'why': 'your message'}
                got = q.answer() if q is not None else None
                if got is not None:
                    return {**got, 'owner': True}
                await self.runner.sleep(POLL_S)
            return None
        finally:
            if q is not None:
                q.close()

    async def _round(self, reason: dict[str, Any]) -> dict[str, Any]:
        self.reasons.append(reason)
        self.round_times.append(self.t)
        self.t += 5
        if not self.rounds:
            self.stop = True
            return {'status': 'cancelled', 'summary': 'stopped', 'evidence': [], 'loop_action': None, 'probe': None,
                    'finished_verified': False, 'question': ''}
        return self.rounds.pop(0)

    async def _probe(self, p: Probe) -> tuple[Any, str | None, set[str]]:
        if not self.reads:
            self.stop = True
            return True, None, set()
        value, error = self.reads.pop(0)
        return (MISSING if error else value), error, {'external:test'}

    def _event(self, cursor: int) -> tuple[int, dict[str, Any]] | None:
        return self.events.pop(0) if self.events and self.events[0][0] > cursor else None

    async def _notify(self, text: str, model_written: bool = False) -> None:
        self.notices.append(text)

    async def _sleep(self, seconds: float) -> None:
        self.t += seconds
        await asyncio.sleep(0)

    def run(self) -> dict[str, Any]:
        return asyncio.run(asyncio.wait_for(self.runner.run(), 5))
