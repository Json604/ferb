"""L31 test support: a scripted loop. Rounds return queued results; the probe returns queued (value, error) pairs;
time is a counter the test advances; sleeps advance it instantly."""
import asyncio
from typing import Any

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
        self.events: list[tuple[int, dict[str, Any]]] = []
        self.store = LoopStore(db, clock)
        self.runner = LoopRunner(
            store=self.store, task_id='tree', step=step, round=self._round, arm=lambda p: self.refuse,
            probe=self._probe, record=lambda kind, p, v, e, o, n: self.records.append((kind, v, e)),
            event=(self._event if step.event else None), steers=lambda: self.steer_count, notify=self._notify,
            status=self.statuses.append, stopped=lambda: self.stop, now=lambda: self.t, sleep=self._sleep)

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

    async def _notify(self, text: str) -> None:
        self.notices.append(text)

    async def _sleep(self, seconds: float) -> None:
        self.t += seconds
        await asyncio.sleep(0)

    def run(self) -> dict[str, Any]:
        return asyncio.run(asyncio.wait_for(self.runner.run(), 5))
