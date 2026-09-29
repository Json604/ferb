"""Starts and stops every watcher (section 11). `ferb pause` stops all of them — the owner-input tap included — and
the trail records nothing until `ferb resume`; the pause survives a restart (a marker file). While paused, FERB can't
tell when the owner is using the mouse or keyboard, so real input is refused (Accessibility actions still work).
"""

import asyncio
from pathlib import Path
from typing import Any, Callable

from ferb.sense.trail import Observation, Trail
from ferb.sense.watchers.base import Watcher


class Sense:
    def __init__(self, home: Path, trail: Trail, watchers: list[Watcher], owner_input: Any = None,
                 on_pause: Callable[[bool], None] = lambda paused: None) -> None:
        self.marker = home / "PAUSED"
        self.trail, self.watchers, self.owner_input, self.on_pause = trail, watchers, owner_input, on_pause
        self._stop: asyncio.Event | None = None
        self._tasks: list[asyncio.Task[None]] = []

    @property
    def paused(self) -> bool:
        return self.marker.exists()

    @property
    def running(self) -> list[str]:
        return [w.source for w, t in zip(self.watchers, self._tasks) if not t.done()]

    def emit(self, o: Observation) -> None:
        self.trail.record(o)

    async def start(self) -> None:
        if self.paused or self._tasks:
            return
        if self.owner_input is not None:
            self.owner_input.start()
        self._stop = asyncio.Event()
        self._tasks = [asyncio.get_running_loop().create_task(self._guard(w)) for w in self.watchers]

    async def _guard(self, w: Watcher) -> None:
        assert self._stop is not None
        try:
            await w.run(self.emit, self._stop)
        except asyncio.CancelledError:
            raise
        except Exception as e:                                  # one broken watcher never stops the others
            self.trail.publish("sense.watcher_failed", {"source": w.source, "error": f"{type(e).__name__}: {e}"})

    async def stop(self) -> None:
        if self._stop is not None:
            self._stop.set()
        for t in self._tasks:
            try:
                await asyncio.wait_for(t, 5)
            except (asyncio.TimeoutError, asyncio.CancelledError):
                t.cancel()
        self._tasks, self._stop = [], None
        if self.owner_input is not None:
            self.owner_input.stop()

    async def pause(self) -> None:
        self.marker.touch(mode=0o600)
        await self.stop()
        self.on_pause(True)

    async def resume(self) -> None:
        self.marker.unlink(missing_ok=True)
        await self.start()
        self.on_pause(False)
