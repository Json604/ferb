"""Observe frontmost-app changes and attach timing context. Must not activate apps or infer permission.

Frontmost app changes: app name + bundle id only."""

import asyncio
from typing import Any, Callable

from ferb.sense.trail import Observation
from ferb.sense.watchers.base import Emit, Focused, focused_app, poll


class AppFocusWatcher:
    source = "app_focus"

    def __init__(self, interval_s: float = 0.5, read: Callable[[], Focused | None] = focused_app,
                 on_change: Callable[[Focused], None] = lambda f: None) -> None:
        self.interval_s, self.read, self.on_change = interval_s, read, on_change
        self._last: str | None = None

    async def run(self, emit: Emit, stop: asyncio.Event) -> None:
        self._last = None
        loop = asyncio.get_running_loop()

        def step() -> Any:
            f = self.read()
            if f is None or f.bundle_id == self._last:
                return
            self._last = f.bundle_id
            loop.call_soon_threadsafe(self._changed, emit, f)
        await poll(self.interval_s, stop, step)

    def _changed(self, emit: Emit, f: Focused) -> None:
        emit(Observation(self.source, {"app": f.name, "bundle_id": f.bundle_id}))
        self.on_change(f)
