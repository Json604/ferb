"""Shared support for tests whose calls meet FERB's own questions (D23 floor, D33, S4) on the way to what they test.

`FerbAnswers` answers only FERB's questions (the approvals the gateway raises as `ferb.confirm`), Yes or No, as they
appear; a worker's own owner.ask questions are left to the test. It is strict: a test declares the questions it
expects (`expect(rule, n)`), and `verify` (run by the `ferb_yes` fixture at teardown) fails on any question FERB asked
that wasn't expected, or one expected that wasn't asked, so a regression in the floor can't hide behind it. The
confirmation tests themselves (tests/test_confirmations.py) answer with `Answers` and check every question."""
import asyncio
import json
from collections import Counter
from types import TracebackType
from typing import Any

from ferb.daemon.gateway import CONFIRM_ASK, NO, YES


class FerbAnswers:
    def __init__(self, d: Any, yes: bool = True) -> None:
        self.d, self.yes, self.asked = d, yes, []
        self.expected: Counter[str] = Counter()
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

    def expect(self, rule: str, n: int = 1) -> "FerbAnswers":
        """The test expects FERB to ask about `rule` n (more) times; every rule a question covers counts once."""
        self.expected[rule] += n
        return self

    def rules(self) -> Counter[str]:
        """The rules FERB's questions covered so far, from their owner.ask spans (`args.confirm`)."""
        async def go() -> Counter[str]:
            rows = self.d.daemon.db.conn.execute(
                "SELECT input_json FROM spans WHERE kind = 'tool.call' AND json_extract(input_json, '$.tool') = "
                "'owner.ask' AND json_extract(input_json, '$.ferb_question') = 1").fetchall()
            return Counter(c["rule"] for r in rows for c in json.loads(r["input_json"])["args"]["confirm"])
        return self.d.run(go())                          # type: ignore[no-any-return]

    def verify(self) -> None:
        got = self.rules()
        assert got == self.expected, f"FERB asked about {dict(got)}; the test expected {dict(self.expected)}"

    def stop(self) -> None:
        self._task.cancel()

    def __enter__(self) -> "FerbAnswers":
        return self

    def __exit__(self, kind: type[BaseException] | None, exc: BaseException | None, tb: TracebackType | None) -> None:
        self.stop()
