"""Observe window geometry and titles under observation policy. Must not raise, move or resize windows.

Focused window titles, when `[sense] window_titles` is on (default). Redacted like everything in
the pipeline. Browsers are skipped: their pages are recorded by the tab watcher, where the URL lets the domain
exclusions apply (a title alone can't be checked against them)."""

import asyncio
from typing import Any, Callable

from ferb.act.mac.browsers import is_browser
from ferb.sense.trail import Observation
from ferb.sense.watchers.base import Emit, Focused, focused_app, focused_title, poll


class WindowTitleWatcher:
    source = "window"

    def __init__(self, enabled: Callable[[], bool], interval_s: float = 1.0,
                 read: Callable[[], tuple[Focused, str | None] | None] | None = None) -> None:
        self.enabled, self.interval_s = enabled, interval_s
        self.read = read or _read
        self._last: tuple[str, str] | None = None

    async def run(self, emit: Emit, stop: asyncio.Event) -> None:
        self._last = None
        loop = asyncio.get_running_loop()

        def step() -> Any:
            if not self.enabled():
                return
            r = self.read()
            if r is None or r[1] is None or is_browser(r[0].bundle_id):
                return
            key = (r[0].bundle_id, r[1])
            if key == self._last:
                return
            self._last = key
            loop.call_soon_threadsafe(emit, Observation(self.source, {"app": r[0].name, "bundle_id": r[0].bundle_id,
                                                                      "title": r[1]}))
        await poll(self.interval_s, stop, step)


def _read() -> tuple[Focused, str | None] | None:
    f = focused_app()
    return (f, focused_title(f.pid)) if f else None
