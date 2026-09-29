"""Owner input (section 10.5): is the owner using the mouse or keyboard right now?

A listen-only event tap on its own run-loop thread. Every input event FERB posts carries a private marker
(`FERB_EVENT_MARK` in kCGEventSourceUserData), so any unmarked event is the owner's. The system idle timer can't be
used: FERB's own input resets it (spike 14). Needs Accessibility or Input Monitoring on the daemon's interpreter.
"""

import threading
import time
from typing import Any, Callable

FERB_EVENT_MARK = 0x46455242            # "FERB"


class OwnerInput:
    def __init__(self, quiet_s: float = 1.5, now: Callable[[], float] = time.monotonic) -> None:
        self.quiet_s, self.now = quiet_s, now
        self.last_owner: float | None = None
        self.last_owner_wall: float | None = None       # wall-clock time of the same event (time header, idle)
        self.running = False
        self._tap: Any = None
        self._loop: Any = None

    def owner_event(self) -> None:
        self.last_owner = self.now()
        self.last_owner_wall = time.time()

    def active(self) -> bool:
        """The owner touched input within the quiet period: GUI work must wait."""
        return self.last_owner is not None and self.now() - self.last_owner < self.quiet_s

    def start(self) -> bool:
        if self.running:
            return True
        import Quartz
        kinds = (Quartz.kCGEventMouseMoved, Quartz.kCGEventLeftMouseDown, Quartz.kCGEventLeftMouseUp,
                 Quartz.kCGEventRightMouseDown, Quartz.kCGEventLeftMouseDragged, Quartz.kCGEventKeyDown,
                 Quartz.kCGEventKeyUp, Quartz.kCGEventScrollWheel, Quartz.kCGEventFlagsChanged)
        mask = 0
        for k in kinds:
            mask |= Quartz.CGEventMaskBit(k)
        ready = threading.Event()

        tap: list[Any] = []                                       # this start's tap, for its own callback

        def cb(proxy: Any, etype: int, event: Any, refcon: Any) -> Any:
            if etype in (Quartz.kCGEventTapDisabledByTimeout, Quartz.kCGEventTapDisabledByUserInput):
                if tap and self.running:                          # the system turned it off: back on (not if stopping)
                    Quartz.CGEventTapEnable(tap[0], True)
                return event
            if Quartz.CGEventGetIntegerValueField(event, Quartz.kCGEventSourceUserData) != FERB_EVENT_MARK:
                self.owner_event()
            return event

        def run() -> None:
            self._tap = Quartz.CGEventTapCreate(Quartz.kCGSessionEventTap, Quartz.kCGHeadInsertEventTap,
                                                Quartz.kCGEventTapOptionListenOnly, mask, cb, None)
            if self._tap is not None:
                tap.append(self._tap)
                src = Quartz.CFMachPortCreateRunLoopSource(None, self._tap, 0)
                self._loop = Quartz.CFRunLoopGetCurrent()
                Quartz.CFRunLoopAddSource(self._loop, src, Quartz.kCFRunLoopCommonModes)
                Quartz.CGEventTapEnable(self._tap, True)
                self.running = True
            ready.set()
            if self._tap is not None:
                Quartz.CFRunLoopRun()

        threading.Thread(target=run, name="ferb-owner-input", daemon=True).start()
        ready.wait(5)
        return self.running

    def stop(self) -> None:
        import Quartz
        self.running = False                                      # first: the callback must not re-enable it
        if self._tap is not None:
            Quartz.CGEventTapEnable(self._tap, False)
            Quartz.CFMachPortInvalidate(self._tap)
        if self._loop is not None:
            Quartz.CFRunLoopStop(self._loop)
        self._tap = self._loop = None
