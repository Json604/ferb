"""Owner input (section 10.5): is the owner using the mouse or keyboard right now?

A listen-only event tap on its own run-loop thread. Every input event FERB posts carries a private marker
(`FERB_EVENT_MARK` in kCGEventSourceUserData), so any unmarked event is the owner's. The system idle timer can't be
used: FERB's own input resets it (spike 14). Needs Accessibility or Input Monitoring on the daemon's interpreter.
"""

import threading
import time
from typing import Any, Callable

FERB_EVENT_MARK = 0x46455242            # "FERB"
# Is this modifier down? Its own side's bit (NX_DEVICE*KEYMASK), so holding Left Option doesn't keep Right Option down.
MODIFIER_FLAG = {61: 0x40, 58: 0x20, 54: 0x10, 55: 0x08, 60: 0x04, 56: 0x02, 62: 0x2000, 59: 0x01,
                 63: 0x800000}   # right/left option, right/left command, right/left shift, right/left control, fn


class OwnerInput:
    def __init__(self, quiet_s: float = 1.5, now: Callable[[], float] = time.monotonic) -> None:
        self.quiet_s, self.now = quiet_s, now
        self.last_owner: float | None = None
        self.last_owner_wall: float | None = None       # wall-clock time of the same event (time header, idle)
        self.running = False
        self._tap: Any = None
        self._loop: Any = None
        # Push-to-talk (20.1): the owner's key events (keycode, down) go here; called on the tap's thread, so it must
        # only hand off (the daemon schedules onto its loop). Never sees FERB's own marked events.
        self.on_key: Callable[[int, bool], None] | None = None

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
                if self.on_key is not None and etype in (Quartz.kCGEventFlagsChanged, Quartz.kCGEventKeyDown):
                    code = int(Quartz.CGEventGetIntegerValueField(event, Quartz.kCGKeyboardEventKeycode))
                    if etype == Quartz.kCGEventKeyDown:
                        down = True
                    else:                                         # a modifier: down while its flag is set
                        down = bool(Quartz.CGEventGetFlags(event) & MODIFIER_FLAG.get(code, 0))
                    try:
                        self.on_key(code, down)
                    except Exception:
                        pass                                      # never let push-to-talk break the owner's tap
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


class InputGuard:
    """While FERB has an app forward (10.5, 9.4 step 3): the owner's key presses and clicks are held back — never
    delivered to FERB's app — and `on_owner` is told at once (FERB puts the owner's app back). A filtering tap, so it
    is enabled only for those windows; mouse movement passes through. FERB's own marked events always pass."""

    def __init__(self, on_owner: Callable[[], None] = lambda: None) -> None:
        self.on_owner = on_owner
        self.engaged = False
        self.held = 0                                             # owner events held back (tests, the trail)
        self._tap: Any = None

    def _start(self) -> bool:
        if self._tap is not None:
            return True
        import Quartz
        kinds = (Quartz.kCGEventKeyDown, Quartz.kCGEventKeyUp, Quartz.kCGEventLeftMouseDown, Quartz.kCGEventLeftMouseUp,
                 Quartz.kCGEventRightMouseDown, Quartz.kCGEventRightMouseUp, Quartz.kCGEventMouseMoved)
        mask = 0
        for k in kinds:
            mask |= Quartz.CGEventMaskBit(k)
        ready = threading.Event()
        blocking = (Quartz.kCGEventKeyDown, Quartz.kCGEventKeyUp, Quartz.kCGEventLeftMouseDown,
                    Quartz.kCGEventLeftMouseUp, Quartz.kCGEventRightMouseDown, Quartz.kCGEventRightMouseUp)

        def cb(proxy: Any, etype: int, event: Any, refcon: Any) -> Any:
            if etype in (Quartz.kCGEventTapDisabledByTimeout, Quartz.kCGEventTapDisabledByUserInput):
                if self.engaged and self._tap is not None:
                    Quartz.CGEventTapEnable(self._tap, True)
                return event
            if not self.engaged or Quartz.CGEventGetIntegerValueField(
                    event, Quartz.kCGEventSourceUserData) == FERB_EVENT_MARK:
                return event
            try:
                self.on_owner()
            except Exception:
                pass
            if etype in blocking:                                 # never lands in FERB's app
                self.held += 1
                return None
            return event

        def run() -> None:
            self._tap = Quartz.CGEventTapCreate(Quartz.kCGSessionEventTap, Quartz.kCGHeadInsertEventTap,
                                                Quartz.kCGEventTapOptionDefault, mask, cb, None)
            if self._tap is not None:
                src = Quartz.CFMachPortCreateRunLoopSource(None, self._tap, 0)
                Quartz.CFRunLoopAddSource(Quartz.CFRunLoopGetCurrent(), src, Quartz.kCFRunLoopCommonModes)
                Quartz.CGEventTapEnable(self._tap, False)          # off until FERB brings an app forward
            ready.set()
            if self._tap is not None:
                Quartz.CFRunLoopRun()

        threading.Thread(target=run, name="ferb-input-guard", daemon=True).start()
        ready.wait(5)
        return self._tap is not None

    def engage(self) -> bool:
        """On before FERB brings its app forward. False if the tap couldn't be made (no Accessibility)."""
        if not self._start():
            return False
        import Quartz
        self.engaged = True
        Quartz.CGEventTapEnable(self._tap, True)
        return True

    def release(self) -> None:
        """Off once the owner's app is back in front."""
        self.engaged = False
        if self._tap is not None:
            import Quartz
            Quartz.CGEventTapEnable(self._tap, False)
