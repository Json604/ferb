"""A throwaway window for the real screenshot + click tests (build step 9): a red main window showing a word, and a
small blue window floating over its top-left corner. Clicks are printed as "click <window> <x> <y>" (window
coordinates, top-left origin). Usage: python probe_window.py WORD; prints "ready <pid>" once shown."""

import sys

import AppKit
import objc
from Foundation import NSMakeRect, NSObject

W, H = 600.0, 400.0
OVER = 160.0


class ClickView(AppKit.NSView):
    def initWithFrame_name_color_(self, frame, name, color):  # type: ignore[no-untyped-def]
        self = objc.super(ClickView, self).initWithFrame_(frame)
        self.name, self.color = name, color
        return self

    def drawRect_(self, rect):  # type: ignore[no-untyped-def]
        self.color.set()
        AppKit.NSRectFill(self.bounds())

    def isFlipped(self) -> bool:
        return True

    def acceptsFirstMouse_(self, event) -> bool:  # type: ignore[no-untyped-def]
        return True

    def mouseDown_(self, event):  # type: ignore[no-untyped-def]
        p = self.convertPoint_fromView_(event.locationInWindow(), None)
        print(f"click {self.name} {p.x:.0f} {p.y:.0f}", flush=True)


def window(x: float, y_top: float, w: float, h: float, name: str, color, floating: bool):  # type: ignore[no-untyped-def]
    screen = AppKit.NSScreen.mainScreen().frame()
    style = AppKit.NSWindowStyleMaskBorderless
    win = AppKit.NSWindow.alloc().initWithContentRect_styleMask_backing_defer_(
        NSMakeRect(x, screen.size.height - y_top - h, w, h), style, AppKit.NSBackingStoreBuffered, False)
    win.setTitle_(f"FERB probe {name}")
    view = ClickView.alloc().initWithFrame_name_color_(NSMakeRect(0, 0, w, h), name, color)
    win.setContentView_(view)
    if floating:
        win.setLevel_(AppKit.NSFloatingWindowLevel)
    return win, view


class Delegate(NSObject):
    def applicationDidFinishLaunching_(self, note):  # type: ignore[no-untyped-def]
        word = sys.argv[1]
        self.main, view = window(200, 200, W, H, "main", AppKit.NSColor.redColor(), False)
        label = AppKit.NSTextField.labelWithString_(word)
        label.setFont_(AppKit.NSFont.boldSystemFontOfSize_(64))
        label.setTextColor_(AppKit.NSColor.blackColor())
        label.setFrame_(NSMakeRect(180, 200, 400, 90))
        view.addSubview_(label)
        self.over, _ = window(190, 190, OVER, OVER, "over", AppKit.NSColor.blueColor(), True)
        self.main.makeKeyAndOrderFront_(None)
        self.over.orderFront_(None)
        AppKit.NSApp.activateIgnoringOtherApps_(True)
        print(f"ready {AppKit.NSProcessInfo.processInfo().processIdentifier()}", flush=True)


if __name__ == "__main__":
    app = AppKit.NSApplication.sharedApplication()
    app.setActivationPolicy_(AppKit.NSApplicationActivationPolicyRegular)
    d = Delegate.alloc().init()
    app.setDelegate_(d)
    app.run()
