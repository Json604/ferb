"""Shared watcher plumbing (section 6 `Watcher`): the run shape, the focused app / window as the system reports it,
and an FSEvents stream on its own run-loop thread. Blocking pyobjc calls stay in threads.

The frontmost app comes from Accessibility's system-wide focused application, read fresh on every poll (a daemon
has no AppKit run loop, so NSWorkspace's cached app state can go stale).
"""

import asyncio
import threading
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Callable, Protocol

from ferb.sense.trail import Observation

Emit = Callable[[Observation], None]


class Watcher(Protocol):
    source: str

    async def run(self, emit: Emit, stop: asyncio.Event) -> None: ...


@dataclass(frozen=True)
class Focused:
    pid: int
    bundle_id: str
    name: str


def focused_app() -> Focused | None:
    import ApplicationServices as AX
    from AppKit import NSRunningApplication
    err, app = AX.AXUIElementCopyAttributeValue(AX.AXUIElementCreateSystemWide(), "AXFocusedApplication", None)
    if err != 0 or app is None:
        return None
    err, pid = AX.AXUIElementGetPid(app, None)
    if err != 0:
        return None
    ra = NSRunningApplication.runningApplicationWithProcessIdentifier_(pid)
    if ra is None or not ra.bundleIdentifier():
        return None
    return Focused(int(pid), str(ra.bundleIdentifier()), str(ra.localizedName() or ""))


def focused_title(pid: int) -> str | None:
    from ferb.act.mac.accessibility import _attr, app_window
    win = app_window(pid)
    t = _attr(win, "AXTitle") if win is not None else None
    return str(t) if t else None


MAX_FAILED_READS = 20


async def poll(interval_s: float, stop: asyncio.Event, step: Callable[[], Any]) -> None:
    """Run `step` in a thread every interval until stopped. One failed read (an app answering Accessibility badly)
    is skipped; only a run of MAX_FAILED_READS failures stops the watcher (and doctor shows why)."""
    failures = 0
    while not stop.is_set():
        try:
            await asyncio.to_thread(step)
            failures = 0
        except Exception:
            failures += 1
            if failures >= MAX_FAILED_READS:
                raise
        try:
            await asyncio.wait_for(stop.wait(), interval_s)
        except asyncio.TimeoutError:
            pass


# FSEvents item flags
CREATED, REMOVED, RENAMED, MODIFIED, IS_DIR = 0x100, 0x200, 0x800, 0x1000, 0x20000


class FsStream:
    """File-level FSEvents for some folders; `on_event(path, flags)` is called on the stream's own thread."""

    def __init__(self, folders: list[Path], on_event: Callable[[str, int], None], latency_s: float = 0.3) -> None:
        self.folders, self.on_event, self.latency_s = folders, on_event, latency_s
        self._stream: Any = None
        self._loop: Any = None

    def start(self) -> bool:
        import FSEvents as F
        ready = threading.Event()
        started = False

        def cb(stream: Any, info: Any, n: int, paths: Any, flags: Any, ids: Any) -> None:
            for i in range(n):
                self.on_event(str(paths[i]), int(flags[i]))

        def run() -> None:
            nonlocal started
            import Quartz
            flags = (F.kFSEventStreamCreateFlagUseCFTypes | F.kFSEventStreamCreateFlagFileEvents
                     | F.kFSEventStreamCreateFlagNoDefer)
            self._stream = F.FSEventStreamCreate(None, cb, None, [str(p) for p in self.folders],
                                                 F.kFSEventStreamEventIdSinceNow, self.latency_s, flags)
            if self._stream is not None:
                self._loop = Quartz.CFRunLoopGetCurrent()
                F.FSEventStreamScheduleWithRunLoop(self._stream, self._loop, Quartz.kCFRunLoopDefaultMode)
                started = bool(F.FSEventStreamStart(self._stream))
            ready.set()
            if started:
                Quartz.CFRunLoopRun()

        threading.Thread(target=run, name="ferb-fsevents", daemon=True).start()
        ready.wait(5)
        return started

    def stop(self) -> None:
        import FSEvents as F
        import Quartz
        if self._stream is not None:
            F.FSEventStreamStop(self._stream)
            F.FSEventStreamInvalidate(self._stream)
            F.FSEventStreamRelease(self._stream)
            self._stream = None
        if self._loop is not None:
            Quartz.CFRunLoopStop(self._loop)
            self._loop = None
