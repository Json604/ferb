"""Self-test run inside the daemon (section 8 step 5): the process that will really act proves it can.

- AX read: the Dock's accessibility tree (a system process; never the owner's frontmost app).
- Event tap: a listen-only tap can be created (owner-input watcher, section 10.5).
- One input event: a *marked* mouse-moved to where the pointer already is (no visible effect), seen by our tap.
Browser self-tests arrive with build steps 5 and 6.
"""

import threading
import time
from dataclasses import dataclass

import ApplicationServices as AX
import Quartz

from ferb.sense.watchers.owner_input import FERB_EVENT_MARK  # FERB's own input events (section 10.5)


@dataclass(frozen=True)
class Result:
    name: str
    ok: bool
    detail: str


def ax_read() -> Result:
    """Read the Dock's AX tree: a system process that's always running and holds none of the owner's content.
    Never the frontmost app (that would read whatever the owner is working in — and could be Brave)."""
    from AppKit import NSRunningApplication
    apps = NSRunningApplication.runningApplicationsWithBundleIdentifier_("com.apple.dock")
    if not apps:
        return Result("ax_read", False, "Dock not running")
    el = AX.AXUIElementCreateApplication(apps[0].processIdentifier())
    err, children = AX.AXUIElementCopyAttributeValue(el, "AXChildren", None)
    if err != 0:
        return Result("ax_read", False, f"AX error {err} reading the Dock")
    return Result("ax_read", True, f"Dock accessibility tree readable ({len(children or [])} top-level elements)")


class _Tap:
    """Listen-only tap on mouse-moved events, counting FERB-marked ones, on its own run loop thread."""

    def __init__(self) -> None:
        self.marked = 0
        self.tap = None
        self._loop = None
        ready = threading.Event()

        def cb(proxy, etype, event, refcon):  # noqa: ANN001
            if Quartz.CGEventGetIntegerValueField(event, Quartz.kCGEventSourceUserData) == FERB_EVENT_MARK:
                self.marked += 1
            return event

        def run() -> None:
            self.tap = Quartz.CGEventTapCreate(Quartz.kCGSessionEventTap, Quartz.kCGHeadInsertEventTap,
                                               Quartz.kCGEventTapOptionListenOnly,
                                               Quartz.CGEventMaskBit(Quartz.kCGEventMouseMoved), cb, None)
            if self.tap is not None:
                src = Quartz.CFMachPortCreateRunLoopSource(None, self.tap, 0)
                self._loop = Quartz.CFRunLoopGetCurrent()
                Quartz.CFRunLoopAddSource(self._loop, src, Quartz.kCFRunLoopCommonModes)
                Quartz.CGEventTapEnable(self.tap, True)
            ready.set()
            if self.tap is not None:
                Quartz.CFRunLoopRun()

        threading.Thread(target=run, daemon=True).start()
        ready.wait(5)

    def close(self) -> None:
        if self.tap is not None:
            Quartz.CGEventTapEnable(self.tap, False)
            Quartz.CFMachPortInvalidate(self.tap)
        if self._loop is not None:
            Quartz.CFRunLoopStop(self._loop)


def post_marked_still_move() -> None:
    src = Quartz.CGEventSourceCreate(Quartz.kCGEventSourceStatePrivate)
    here = Quartz.CGEventGetLocation(Quartz.CGEventCreate(None))
    ev = Quartz.CGEventCreateMouseEvent(src, Quartz.kCGEventMouseMoved, here, Quartz.kCGMouseButtonLeft)
    Quartz.CGEventSetIntegerValueField(ev, Quartz.kCGEventSourceUserData, FERB_EVENT_MARK)
    Quartz.CGEventPost(Quartz.kCGHIDEventTap, ev)


def event_tap_and_input() -> list[Result]:
    tap = _Tap()
    try:
        if tap.tap is None:
            return [Result("event_tap", False, "couldn't create a listen-only event tap "
                                               "(needs Accessibility or Input Monitoring)"),
                    Result("input_event", False, "skipped: no event tap to confirm it")]
        post_marked_still_move()
        end = time.monotonic() + 1.0
        while tap.marked == 0 and time.monotonic() < end:
            time.sleep(0.01)
        return [Result("event_tap", True, "listen-only tap created"),
                Result("input_event", tap.marked > 0,
                       "marked input event posted and seen" if tap.marked else
                       "posted input event never arrived (Accessibility needed to post events)")]
    finally:
        tap.close()


def run_all() -> list[Result]:
    return [ax_read(), *event_tap_and_input()]
