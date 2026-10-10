"""Shared support for tests whose calls meet FERB's own questions (D23 floor, D33, S4) on the way to what they test.

`FerbAnswers` answers only FERB's questions (the approvals the gateway raises as `ferb.confirm`), Yes or No, as they
appear, and records what it was asked; a worker's own owner.ask questions are left to the test. The confirmation
tests themselves (tests/test_confirmations.py) answer with `Answers` and check every question."""
import asyncio
from types import TracebackType
from typing import Any

from ferb.daemon.gateway import CONFIRM_ASK, NO, YES


class FerbAnswers:
    def __init__(self, d: Any, yes: bool = True) -> None:
        self.d, self.yes, self.asked = d, yes, []
        self._task = asyncio.run_coroutine_threadsafe(self._loop(), d.loop)

    async def _loop(self) -> None:
        approvals = self.d.daemon.state.approvals
        while True:
            for ap in approvals.pending():
                if ap.capability != CONFIRM_ASK:
                    continue
                try:
                    approvals.resolve(ap.id, True, ap.options.index(YES if self.yes else NO) + 1)
                except ValueError:                       # answered meanwhile (by the test's own Answers)
                    continue
                self.asked.append(ap.reason)
            await asyncio.sleep(0.02)

    def stop(self) -> None:
        self._task.cancel()

    def __enter__(self) -> "FerbAnswers":
        return self

    def __exit__(self, kind: type[BaseException] | None, exc: BaseException | None, tb: TracebackType | None) -> None:
        self.stop()
