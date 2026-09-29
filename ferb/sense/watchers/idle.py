"""Owner idle / active (section 11), from the owner-input tap (FERB's own marked input never counts). Records only the
transitions; nothing about what was typed or clicked."""

import asyncio
import time
from typing import Callable

from ferb.sense.trail import Observation
from ferb.sense.watchers.base import Emit


class IdleWatcher:
    source = "idle"

    def __init__(self, last_owner_wall: Callable[[], float | None], tap_running: Callable[[], bool],
                 idle_after_s: float = 120.0, interval_s: float = 2.0,
                 now: Callable[[], float] = time.time) -> None:
        self.last_owner_wall, self.tap_running = last_owner_wall, tap_running
        self.idle_after_s, self.interval_s, self.now = idle_after_s, interval_s, now
        self.state: str | None = None

    def check(self, emit: Emit) -> None:
        if not self.tap_running():
            return                                               # can't tell: record nothing
        last = self.last_owner_wall()
        state = "active" if last is not None and self.now() - last < self.idle_after_s else "idle"
        if state != self.state:
            self.state = state
            emit(Observation(self.source, {"state": state}, {"actor": "owner"}))

    async def run(self, emit: Emit, stop: asyncio.Event) -> None:
        self.state = None
        while not stop.is_set():
            self.check(emit)
            try:
                await asyncio.wait_for(stop.wait(), self.interval_s)
            except asyncio.TimeoutError:
                pass
