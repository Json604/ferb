"""Aggregate budgets per task tree (section 7 step 10) and worker heartbeats (step 5).

Everything a tree spends counts: worker runs (including verification, reconciliation and retries), tool calls,
tokens (Codex reports tokens), cost (Claude reports cost), wall time. Going over stops the tree.

A worker that shows no sign of life for STALL_S is stalled and cancelled — but time spent waiting on the owner
(an undo window, real input pausing while the owner uses the mouse/keyboard) doesn't count: while any wait is open
the worker is paused, and the clock restarts when the last wait closes.
"""

from dataclasses import dataclass, field
from typing import Callable

from ferb.orchestrator.dag import Limits
from ferb.orchestrator.schemas import PlanBudget

STALL_S = 180.0


@dataclass
class TreeBudget:
    minutes: float
    tokens: int
    cost_usd: float
    tool_calls: int
    workers: int
    started: float
    used_tokens: int = 0
    used_cost: float = 0.0
    used_calls: int = 0
    used_workers: int = 0

    @classmethod
    def from_plan(cls, b: PlanBudget, limits: Limits, now: float) -> "TreeBudget":
        return cls(float(min(b.max_minutes or limits.max_minutes, limits.max_minutes)),
                   min(b.max_tokens or limits.max_tokens, limits.max_tokens),
                   min(b.max_cost_usd or limits.max_cost_usd, limits.max_cost_usd),
                   min(b.max_tool_calls or limits.max_tool_calls, limits.max_tool_calls),
                   min(b.max_workers or limits.max_workers, limits.max_workers), now)

    def usage(self, tokens: int = 0, cost: float = 0.0) -> None:
        self.used_tokens += tokens
        self.used_cost += cost

    def over(self, now: float) -> str | None:
        if now - self.started > self.minutes * 60:
            return f"time budget ({self.minutes:.0f} min) used up"
        if self.used_tokens > self.tokens:
            return f"token budget ({self.tokens}) used up"
        if self.used_cost > self.cost_usd:
            return f"cost budget (${self.cost_usd:.2f}) used up"
        if self.used_calls > self.tool_calls:
            return f"tool-call budget ({self.tool_calls}) used up"
        if self.used_workers > self.workers:
            return f"worker budget ({self.workers}) used up"
        return None

    def can_start_worker(self) -> bool:
        return self.used_workers < self.workers


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
