"""Spike 14: detect the owner's mouse/keyboard input and make GUI actions yield (ARCH 10.5, section 7 step 11).

- Listen-only CGEventTap (needs Accessibility or Input Monitoring). Records only event type + timestamps, never
  key codes or positions.
- FERB's own synthetic events carry a private marker in kCGEventSourceUserData; anything without it = owner.
- Harmless synthetic events only: mouse-moved to the pointer's current location (no visible effect).
"""

import threading
import time

import Quartz

FERB_MARK = 0x46455242  # "FERB"
OWNER_TYPES = (Quartz.kCGEventMouseMoved, Quartz.kCGEventLeftMouseDown, Quartz.kCGEventLeftMouseUp,
               Quartz.kCGEventRightMouseDown, Quartz.kCGEventRightMouseUp, Quartz.kCGEventScrollWheel,
               Quartz.kCGEventLeftMouseDragged, Quartz.kCGEventKeyDown, Quartz.kCGEventFlagsChanged)


def uptime_ns() -> int:
    """Same clock as CGEventGetTimestamp (nanoseconds since boot, not counting sleep)."""
    return time.clock_gettime_ns(time.CLOCK_UPTIME_RAW)


class OwnerInputWatcher:
    def __init__(self, quiet_ms: float = 1500) -> None:
        self.quiet_ms = quiet_ms
        self.last_owner_ns: int | None = None
        self.owner_events: list[tuple[int, int, int]] = []   # (event type, event timestamp ns, seen ns)
        self.ferb_events = 0
        self._ready = threading.Event()
        self._loop = None
        threading.Thread(target=self._run, daemon=True).start()
        if not self._ready.wait(5) or self._tap is None:
            raise PermissionError("could not create event tap (Accessibility / Input Monitoring not granted?)")

    def _callback(self, proxy, etype, event, refcon):
        seen = uptime_ns()
        if etype in (Quartz.kCGEventTapDisabledByTimeout, Quartz.kCGEventTapDisabledByUserInput):
            Quartz.CGEventTapEnable(self._tap, True)
            return event
        if Quartz.CGEventGetIntegerValueField(event, Quartz.kCGEventSourceUserData) == FERB_MARK:
            self.ferb_events += 1
        else:
            self.last_owner_ns = seen
            self.owner_events.append((int(etype), int(Quartz.CGEventGetTimestamp(event)), seen))
        return event

    def _run(self) -> None:
        mask = 0
        for t in OWNER_TYPES:
            mask |= Quartz.CGEventMaskBit(t)
        self._tap = Quartz.CGEventTapCreate(Quartz.kCGSessionEventTap, Quartz.kCGHeadInsertEventTap,
                                            Quartz.kCGEventTapOptionListenOnly, mask, self._callback, None)
        if self._tap is None:
            self._ready.set()
            return
        src = Quartz.CFMachPortCreateRunLoopSource(None, self._tap, 0)
        self._loop = Quartz.CFRunLoopGetCurrent()
        Quartz.CFRunLoopAddSource(self._loop, src, Quartz.kCFRunLoopCommonModes)
        Quartz.CGEventTapEnable(self._tap, True)
        self._ready.set()
        Quartz.CFRunLoopRun()

    def owner_active(self) -> bool:
        last = self.last_owner_ns
        return last is not None and (uptime_ns() - last) / 1e6 < self.quiet_ms

    def stop(self) -> None:
        if self._loop is not None:
            Quartz.CFRunLoopStop(self._loop)


def _pointer():
    return Quartz.CGEventGetLocation(Quartz.CGEventCreate(None))


def post_still_mouse_move(ferb: bool) -> int:
    """A mouse-moved event to the pointer's current position. ferb=True marks it as FERB's own.
    Returns the uptime ns just before posting."""
    src = Quartz.CGEventSourceCreate(Quartz.kCGEventSourceStatePrivate if ferb else
                                     Quartz.kCGEventSourceStateHIDSystemState)
    ev = Quartz.CGEventCreateMouseEvent(src, Quartz.kCGEventMouseMoved, _pointer(), Quartz.kCGMouseButtonLeft)
    if ferb:
        Quartz.CGEventSetIntegerValueField(ev, Quartz.kCGEventSourceUserData, FERB_MARK)
    t = uptime_ns()
    Quartz.CGEventPost(Quartz.kCGHIDEventTap, ev)
    return t


def gui_action(watcher: OwnerInputWatcher, steps: int, step_ms: float, log: list) -> str:
    """A GUI action made of small steps; checks owner input before every step and stops ('yielded')."""
    for i in range(steps):
        if watcher.owner_active():
            log.append(("yielded", i, uptime_ns()))
            return "yielded"
        post_still_mouse_move(ferb=True)
        log.append(("step", i, uptime_ns()))
        time.sleep(step_ms / 1000)
    return "completed"


def hid_idle_seconds() -> float:
    return Quartz.CGEventSourceSecondsSinceLastEventType(Quartz.kCGEventSourceStateHIDSystemState,
                                                        Quartz.kCGAnyInputEventType)
