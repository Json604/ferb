"""Runaway guards per task tree (section 7 step 10) and worker heartbeats (step 5).

No token or dollar budgets (owner decision). The guards only catch stuck tasks and stay invisible unless one stops
something:
- the same failing action (same tool, same arguments) 3 times -> that worker stops;
- a loop between steps: the same sequence of 2-4 calls (identical arguments AND identical results) 5 times in a row ->
  that worker stops (one call repeated is polling, e.g. waiting for a page, and isn't stopped unless it keeps failing;
  scrolling through a page that keeps changing returns new results each time, so it isn't a loop);
- generous tree caps: 4 hours, 50 worker runs, 1,000 tool calls -> the tree stops.

A worker that shows no sign of life for STALL_S is stalled and cancelled — but time spent waiting on the owner
(an undo window, real input pausing while the owner uses the mouse/keyboard) doesn't count: while any wait is open
the worker is paused, and the clock restarts when the last wait closes.
"""

import hashlib
import json
import re
from collections import defaultdict, deque
from dataclasses import dataclass, field
from typing import Any, Callable

STALL_S = 180.0
MAX_MINUTES, MAX_WORKER_RUNS, MAX_TOOL_CALLS = 240, 50, 1000
REPEATED_FAILURES, LOOP_REPEATS, LOOP_MAX_PERIOD = 3, 5, 4
GUARD_NOTE = "a safety guard against stuck tasks"


def stable_observation(value: Any, key: str = "") -> Any:
    """Discard snapshot bookkeeping, preserving the actual page/app state and action target ordinal."""
    if isinstance(value, dict):
        return {k: stable_observation(v, k) for k, v in value.items() if k not in {"generation", "snap_gen"}}
    if isinstance(value, list):
        return [stable_observation(v, key) for v in value]
    if key in {"id", "element"} and isinstance(value, str) and re.fullmatch(r"s[0-9]+e[0-9]+", value):
        return re.sub(r"^s[0-9]+", "s", value)
    return value


def observation_digest(value: Any) -> str:
    return hashlib.sha256(json.dumps(stable_observation(value), sort_keys=True, default=str).encode()).hexdigest()[:16]


def call_key(tool: str, args: dict[str, Any]) -> str:
    return tool + ":" + observation_digest({k: v for k, v in args.items() if k != "why"})


@dataclass
class TreeGuard:
    started: float
    tool_calls: int = 0
    worker_runs: int = 0
    _failed: dict[str, dict[str, int]] = field(default_factory=lambda: defaultdict(lambda: defaultdict(int)))
    _recent: dict[str, deque[str]] = field(default_factory=lambda: defaultdict(
        lambda: deque(maxlen=LOOP_REPEATS * LOOP_MAX_PERIOD)))
    _observations: dict[tuple[str, str], str] = field(default_factory=dict)
    _mutations: dict[tuple[str, str, str], int] = field(default_factory=lambda: defaultdict(int))

    def call(self, worker: str, tool: str, args: dict[str, Any], failed: bool, result: str = "") -> str | None:
        """Record one finished tool call (`result`: a digest of what it returned); a reason if this worker must stop."""
        self.tool_calls += 1
        key = call_key(tool, args)
        surface = str(args.get("tab") or args.get("app") or "")
        if tool.endswith(".snapshot") and not failed and result:
            self._observations[worker, surface] = result
        if args.get("effect") in {"send", "pay", "destroy"} and not failed:
            observation = self._observations.get((worker, surface))
            if observation:
                repeat = (worker, key, observation)
                self._mutations[repeat] += 1
                if self._mutations[repeat] >= 2:
                    return f"it repeated a consequential action without observed progress ({tool})"
        if failed:
            self._failed[worker][key] += 1
            if self._failed[worker][key] >= REPEATED_FAILURES:
                return f"it kept repeating the same failing action ({tool})"
        recent = self._recent[worker]
        recent.append(key + ":" + result)                  # same call, same answer: that's what a loop repeats
        for period in range(2, LOOP_MAX_PERIOD + 1):      # one call repeated is polling, not a loop
            n = period * LOOP_REPEATS
            if len(recent) >= n:
                tail = list(recent)[-n:]
                steps = tail[:period]
                if len(set(steps)) > 1 and all(tail[i] == steps[i % period] for i in range(n)):
                    return f"it was going round in a loop ({tool})"
        return None

    def over(self, now: float) -> str | None:
        if now - self.started > MAX_MINUTES * 60:
            return f"it ran for over {MAX_MINUTES // 60} hours"
        if self.tool_calls > MAX_TOOL_CALLS:
            return f"it made over {MAX_TOOL_CALLS} tool calls"
        if self.worker_runs > MAX_WORKER_RUNS:
            return f"it started over {MAX_WORKER_RUNS} worker runs"
        return None

    def can_start_worker(self) -> bool:
        return self.worker_runs < MAX_WORKER_RUNS


@dataclass
class _Beat:
    last: float
    waits: int = 0


@dataclass
class Activity:
    """Per-worker liveness. The gateway and real input report waits on the owner; runtimes report events."""
    now: Callable[[], float]
    _beats: dict[str, _Beat] = field(default_factory=dict)

    def start(self, worker: str) -> None:
        self._beats[worker] = _Beat(self.now())

    def beat(self, worker: str) -> None:
        b = self._beats.get(worker)
        if b is not None:
            b.last = self.now()

    def waiting(self, worker: str, on: bool) -> None:
        b = self._beats.get(worker)
        if b is None:
            return
        b.waits = max(0, b.waits + (1 if on else -1))
        b.last = self.now()                      # the stall clock starts over when a wait opens or closes

    def stalled(self, worker: str, stall_s: float = STALL_S) -> bool:
        b = self._beats.get(worker)
        return b is not None and b.waits == 0 and self.now() - b.last > stall_s

    def end(self, worker: str) -> None:
        self._beats.pop(worker, None)
