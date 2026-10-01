"""Spike 19 detector: every desktop (Space) change and app activation, from macOS's own notifications — no polling,
so nothing slips between checks. Writes one JSON line per event to the given file until stopped.
Usage: python detector.py <out.jsonl>"""
import json
import sys
import time

import objc
from AppKit import NSObject, NSWorkspace, NSWorkspaceActiveSpaceDidChangeNotification, \
    NSWorkspaceDidActivateApplicationNotification
from PyObjCTools import AppHelper

OUT = open(sys.argv[1], "a", buffering=1)


def write(kind: str, app: str | None = None) -> None:
    OUT.write(json.dumps({"t": round(time.time(), 3), "event": kind, "app": app}) + "\n")


class Watcher(NSObject):
    def space_(self, note) -> None:                                   # noqa: N802 (Objective-C selector)
        front = NSWorkspace.sharedWorkspace().frontmostApplication()
        write("desktop_changed", str(front.bundleIdentifier()) if front else None)

    def activated_(self, note) -> None:                               # noqa: N802
        app = note.userInfo()["NSWorkspaceApplicationKey"]
        write("app_activated", str(app.bundleIdentifier()))


w = Watcher.new()
nc = NSWorkspace.sharedWorkspace().notificationCenter()
nc.addObserver_selector_name_object_(w, objc.selector(w.space_, signature=b"v@:@"),
                                     NSWorkspaceActiveSpaceDidChangeNotification, None)
nc.addObserver_selector_name_object_(w, objc.selector(w.activated_, signature=b"v@:@"),
                                     NSWorkspaceDidActivateApplicationNotification, None)
write("started")
AppHelper.runConsoleEventLoop(installInterrupt=True)
