"""Owns timing facts for a model call and the work around it: how long each part took, in whole milliseconds, from an
injected monotonic clock (RUN-419, ORCH-526). Must not hold, record or judge any content, and never changes what a call
does: it only reads the clock when told something happened.

Every figure is a difference of rounded marks, so a set of parts adds up exactly to its total."""

from typing import Callable

Now = Callable[[], float]


def _ms(seconds: float) -> int:
    return int(round(seconds * 1000))


class Steps:
    """Named, consecutive steps of one piece of work: each `mark(name)` ends the step that began at the previous mark
    (or at the start). A name marked again adds to its step. `steps` add up to `total_ms`."""

    def __init__(self, now: Now) -> None:
        self._now = now
        self._t0 = now()
        self._at = 0                               # the last mark, in ms since the start
        self.steps: dict[str, int] = {}

    def mark(self, name: str) -> int:
        at = max(_ms(self._now() - self._t0), self._at)
        took, self._at = at - self._at, at
        self.steps[name] = self.steps.get(name, 0) + took
        return took

    @property
    def total_ms(self) -> int:
        return self._at

    def facts(self) -> dict[str, object]:
        return {"steps_ms": dict(self.steps), "total_ms": self._at}


class CallTimer:
    """One run's model calls, call by call, from the arrival times of its runtime's stream (RUN-419). A call begins when
    its request is handed over (`begin`) or when the previous call's tool result goes back to the model
    (`handed_back`); `sent` is when the provider request went out, where the runtime can see that (Codex's request
    trace); `output` is each piece of model output as it arrives; `done` is the call's usage report, and returns its
    timing:

    - runtime_ms: from the call's begin to the request going out (only when `sent` was seen): the runtime's own work
    - ttft_ms: from the request going out (else the begin) to the first output
    - generate_ms: from the first output to the last
    - tail_ms: from the last output to the usage report
    - total_ms: the sum of the above
    `by` names whose clock saw it: "runtime" (the adapter's own stream) or "arrival" (FERB's, at the events' arrival).
    A runtime whose usage report comes after a tool already ran (Codex) gets the next call's begin and request from
    what arrived since the last output."""

    def __init__(self, now: Now, by: str = "runtime") -> None:
        self._now, self.by = now, by
        self._begin: float = now()
        self._sent: float | None = None
        self._first: float | None = None
        self._last: float | None = None
        self._next_begin: float | None = None
        self._next_sent: float | None = None

    def begin(self) -> None:
        """This call's request is handed over now (an API request, Codex's turn start)."""
        self._begin, self._sent, self._first, self._last = self._now(), None, None, None
        self._next_begin = self._next_sent = None

    def sent(self) -> None:
        """The provider request went out now. One that goes out after this call's output belongs to the next call."""
        if self._first is None:
            self._sent = self._now()
        else:
            self._next_sent = self._now()

    def output(self) -> None:
        at = self._now()
        if self._first is None:
            self._first = at
        self._last = at

    def handed_back(self) -> None:
        """A tool result went back to the model now: the next call begins here."""
        if self._first is None:                   # this call is still waiting for its first output: it begins now
            self._begin = self._now()
        else:
            self._next_begin = self._now()

    def done(self) -> dict[str, object]:
        """The call's usage report arrived: its timing; the next call begins now unless a tool result already went
        back."""
        end = self._now()
        begin = self._begin
        sent = max(self._sent, begin) if self._sent is not None else begin
        first = max(self._first, sent) if self._first is not None else end
        last = max(self._last if self._last is not None else end, first)
        marks = [_ms(t - begin) for t in (sent, first, last, max(end, last))]
        facts: dict[str, object] = {"by": self.by}
        if self._sent is not None:
            facts["runtime_ms"] = marks[0]
        facts["ttft_ms"] = marks[1] - marks[0]
        facts["generate_ms"] = marks[2] - marks[1]
        facts["tail_ms"] = marks[3] - marks[2]
        facts["total_ms"] = marks[3]
        nxt = self._next_begin if self._next_begin is not None else end
        self._begin, self._sent = nxt, self._next_sent
        self._first = self._last = self._next_begin = self._next_sent = None
        return facts
