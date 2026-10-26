"""Observe recent owner input through a listen-only tap. Must not suppress or replay input.

Marked FERB events do not count as owner activity or push-to-talk; field capture receives their FERB mark.
Recent activity is a fact for yielding, never a saved focus target."""

import threading
import time
from typing import Any, Callable

FERB_EVENT_MARK = 0x46455242            # "FERB"
# Is this modifier down? Its own side's bit (NX_DEVICE*KEYMASK), so holding Left Option doesn't keep Right Option down.
MODIFIER_FLAG = {61: 0x40, 58: 0x20, 54: 0x10, 55: 0x08, 60: 0x04, 56: 0x02, 62: 0x2000, 59: 0x01,
                 63: 0x800000}   # right/left option, right/left command, right/left shift, right/left control, fn


def _input_monitoring_granted() -> bool:
    from ferb.device.permissions import Status, input_monitoring
    try:
        return input_monitoring() == Status.GRANTED
    except Exception:                                  # no IOKit answer: not known to be granted
        return False


class OwnerInput:
    def __init__(self, quiet_s: float = 1.5, now: Callable[[], float] = time.monotonic,
                 allowed: Callable[[], bool] = _input_monitoring_granted) -> None:
        """`allowed`: whether Input Monitoring is granted. Creating the tap without it makes macOS ask "would like to
        receive keystrokes" (live 2026-10-26: on setup's first screen), so it waits for the grant."""
        self.quiet_s, self.now, self.allowed = quiet_s, now, allowed
        self.last_owner: float | None = None
        self.last_owner_wall: float | None = None       # wall-clock time of the same event (time header, idle)
        self.running = False
        self._tap: Any = None
        self._loop: Any = None
        # Push-to-talk: the owner's key events (keycode, down) go here; called on the tap's thread, so it must
        # only hand off (the daemon schedules onto its loop). Never sees FERB's own marked events.
        self.on_key: Callable[[int, bool], None] | None = None
        # F7 capture owns Secure Input/field checks, aggregation and redaction before anything is emitted.
        self.on_text: Callable[[Any, bool], None] | None = None

    def owner_event(self) -> None:
        self.last_owner = self.now()
        self.last_owner_wall = time.time()

    def active(self) -> bool:
        """The owner touched input within the quiet period: GUI work must wait."""
        return self.last_owner is not None and self.now() - self.last_owner < self.quiet_s

    def start(self) -> bool:
        if self.running:
            return True
        if not self.allowed():                            # no tap, so macOS never prompts; setup asks for it
            return False
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
            marked = Quartz.CGEventGetIntegerValueField(event, Quartz.kCGEventSourceUserData) == FERB_EVENT_MARK
            if self.on_text is not None and etype == Quartz.kCGEventKeyDown:
                try:
                    self.on_text(event, marked)
                except Exception:
                    pass                           # capture must never disturb input delivery
            if not marked:
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
