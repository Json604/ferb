"""Own passive daemon permission diagnostics. Never post input, activate an app or move the cursor.
A readable AX surface/listen-only event tap is a capability fact, not proof of successful app interaction.
"""

import threading
from dataclasses import dataclass

import ApplicationServices as AX
import Quartz


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
    """Create a listen-only event tap without injecting input or retaining its content."""

    def __init__(self) -> None:
        self.tap = None
        self._loop = None
        ready = threading.Event()

        def cb(proxy, etype, event, refcon):  # noqa: ANN001
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


def event_tap() -> list[Result]:
    tap = _Tap()
    try:
        return [Result("event_tap", tap.tap is not None,
                       "listen-only tap created; no input posted" if tap.tap is not None else
                       "couldn't create a listen-only event tap (Accessibility/Input Monitoring unavailable)")]
    finally:
        tap.close()


def run_all() -> list[Result]:
    return [ax_read(), *event_tap()]
