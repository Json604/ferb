"""Own the background test's detector process: what macOS itself says happened (space changes, app activations),
appended as JSON lines to the log it is given. Must not act on the Mac.

`python -I -m ferb.device.detector <log>`, started by `ferb watch` (the command line's own interpreter, never FERB.app's
agent): a module rather than code passed on a command line."""
import json
import sys
import time


def main(argv: list[str] | None = None) -> None:
    import objc
    from AppKit import (NSObject, NSWorkspace, NSWorkspaceActiveSpaceDidChangeNotification,
                        NSWorkspaceDidActivateApplicationNotification)
    from PyObjCTools import AppHelper
    out = open((argv if argv is not None else sys.argv[1:])[0], "a", buffering=1)

    def write(kind: str, app: str | None = None, **extra: str) -> None:
        out.write(json.dumps({"t": round(time.time(), 3), "event": kind, "app": app, **extra}) + "\n")

    class Watcher(NSObject):  # type: ignore[misc]
        def space_(self, note: object) -> None:
            front = NSWorkspace.sharedWorkspace().frontmostApplication()
            write("desktop_changed", str(front.bundleIdentifier()) if front else None)

        def act_(self, note: object) -> None:
            write("app_activated", str(note.userInfo()["NSWorkspaceApplicationKey"].bundleIdentifier()))  # type: ignore[attr-defined]

    watcher = Watcher.new()
    center = NSWorkspace.sharedWorkspace().notificationCenter()
    center.addObserver_selector_name_object_(watcher, objc.selector(watcher.space_, signature=b"v@:@"),
                                             NSWorkspaceActiveSpaceDidChangeNotification, None)
    center.addObserver_selector_name_object_(watcher, objc.selector(watcher.act_, signature=b"v@:@"),
                                             NSWorkspaceDidActivateApplicationNotification, None)
    write("started", executable=sys.executable)            # which process wrote this log
    AppHelper.runConsoleEventLoop(installInterrupt=True)


if __name__ == "__main__":
    main()
