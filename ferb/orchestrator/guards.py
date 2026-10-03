"""Worker liveness and normalized observation digests. No fixed loop, action or task-duration caps."""

import hashlib
import json
import re
from dataclasses import dataclass, field
from typing import Any, Callable

STALL_S = 180.0


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
