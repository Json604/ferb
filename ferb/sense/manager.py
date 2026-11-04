"""Start and stop enabled observation watchers. Must not execute agent tasks from raw observations.

Starts and stops every watcher. `ferb pause` stops all of them (the owner-input tap and the Mac's presence watcher
included) and the trail records nothing until `ferb resume`; the pause survives a restart (a marker file). While paused,
FERB can't tell when the owner is using the mouse or keyboard, so real input is refused (Accessibility actions still
work). The owner-input tap starts as soon as Input Monitoring is granted, not only at a start: it is checked again every
`tap_recheck_s` and each change (down and why, or back up) is reported once (SENSE-406)."""

import asyncio
from pathlib import Path
from typing import Any, Callable

from ferb.sense.trail import Observation, Trail
from ferb.sense.watchers.base import Watcher

# Why the owner-input tap isn't running (`Sense.tap_down`); while it isn't, FERB can't tell whether the owner is using
# the Mac (SENSE-406)
INPUT_MONITORING_OFF = "Input Monitoring isn't granted to FERB"
TAP_REFUSED = "macOS didn't create the input tap"


class Sense:
    def __init__(self, home: Path, trail: Trail, watchers: list[Watcher], owner_input: Any = None,
                 on_pause: Callable[[bool], None] = lambda paused: None,
                 on_fail: Callable[[str, str], None] = lambda source, error: None, presence: Any = None,
                 on_tap: Callable[[str], None] = lambda why: None) -> None:
        self.marker = home / "PAUSED"
        self.trail, self.watchers, self.owner_input, self.on_pause = trail, watchers, owner_input, on_pause
        self.presence = presence                                  # whether the Mac is in use (ORCH-317)
        self.on_fail = on_fail
        self.failed: dict[str, str] = {}                          # source -> why it last failed (doctor shows it)
        self.retry_s = 60.0                                      # a failed watcher tries again after this, doubling
        # the tap's permission is checked again this often: the notification watcher's Full Disk Access recheck
        self.tap_recheck_s = 30.0
        self.on_tap = on_tap                                      # the tap's state changed: why it's down, or ""
        self.tap_down = ""                                        # why the owner-input tap isn't running ("": it is)
        self._stop: asyncio.Event | None = None
        self._tasks: list[asyncio.Task[None]] = []
        self._tap_task: asyncio.Task[None] | None = None

    @property
    def paused(self) -> bool:
        return self.marker.exists()

    @property
    def running(self) -> list[str]:
        """Watchers running now: not ended, and not waiting to try again after a failure."""
        return [w.source for w, t in zip(self.watchers, self._tasks) if not t.done() and w.source not in self.failed]

    def emit(self, o: Observation) -> None:
        self.trail.record(o)

    async def start(self) -> None:
        if self.paused or self._tasks:
            return
        self._stop = asyncio.Event()
        if self.owner_input is not None:
            self._tap_report(self._tap_state())                  # before the watchers that read it (typed text)
            self._tap_task = asyncio.get_running_loop().create_task(self._watch_tap(self._stop))
        if self.presence is not None:
            self.presence.start()
        self._tasks = [asyncio.get_running_loop().create_task(self._guard(w)) for w in self.watchers]

    def _tap_state(self) -> str:
        """Blocking: start the owner-input tap if it's down and Input Monitoring is granted; why it isn't running
        now, or "". A tap whose grant was taken away is stopped (it would see nothing while saying all is quiet)."""
        oi = self.owner_input
        try:
            if not oi.allowed():                                   # no tap without the grant: macOS never prompts
                if oi.running:
                    oi.stop()
                return INPUT_MONITORING_OFF
            return "" if oi.running or oi.start() else TAP_REFUSED
        except Exception as e:                                     # never takes the other watchers down
            return f"{TAP_REFUSED}: {type(e).__name__}: {e}"

    def _tap_report(self, why: str) -> None:
        if why != self.tap_down:
            self.tap_down = why
            self.on_tap(why)

    async def _watch_tap(self, stop: asyncio.Event) -> None:
        """SENSE-406: the tap comes up once the grant is there (live 2026-11-01/02: granted after the start, it stayed
        down until a restart, and foreground input never yielded to the owner meanwhile)."""
        while not stop.is_set():
            try:
                await asyncio.wait_for(stop.wait(), self.tap_recheck_s)
                return
            except asyncio.TimeoutError:
                pass
            self._tap_report(await asyncio.to_thread(self._tap_state))

    async def _guard(self, w: Watcher) -> None:
        """Runs a watcher; one that fails tries again later (live 2026-10-27: a watcher stopped by a permission
        missing at startup stayed stopped after it was granted), pausing retry_s, doubling to 15 minutes. Each new
        reason is reported once."""
        assert self._stop is not None
        stop, wait, reported = self._stop, self.retry_s, ""
        while not stop.is_set():
            try:
                self.failed.pop(w.source, None)                 # trying (again): running until it fails
                await w.run(self.emit, stop)
                return
            except asyncio.CancelledError:
                raise
            except Exception as e:                              # one broken watcher never stops the others
                why = f"{type(e).__name__}: {e}"
                self.failed[w.source] = why
                if why != reported:
                    reported = why
                    self.trail.publish("sense.watcher_failed", {"source": w.source, "error": why})
                    self.on_fail(w.source, why)
            try:
                await asyncio.wait_for(stop.wait(), wait)
            except asyncio.TimeoutError:
                pass
            wait = min(wait * 2, 900.0)

    async def stop(self) -> None:
        if self._stop is not None:
            self._stop.set()
        for t in self._tasks:
            try:
                await asyncio.wait_for(t, 5)
            except (asyncio.TimeoutError, asyncio.CancelledError):
                t.cancel()
        self._tasks, self._stop = [], None
        if self._tap_task is not None:                            # a start under way ends first (at most its 5 s)
            await self._tap_task
            self._tap_task = None
        if self.owner_input is not None:
            self.owner_input.stop()
        if self.presence is not None:
            self.presence.stop()

    async def pause(self) -> None:
        self.marker.touch(mode=0o600)
        await self.stop()
        self.on_pause(True)

    async def resume(self) -> None:
        self.marker.unlink(missing_ok=True)
        await self.start()
        self.on_pause(False)
