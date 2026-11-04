"""A fresh process's reading of the grants a running daemon sees only after a restart (APP-662). Must not prompt.

macOS caches Screen Recording (CGPreflightScreenCaptureAccess) and, on some Macs, Input Monitoring per process: a
switch the owner turns on while the daemon runs keeps reading off in the daemon until it restarts. A process started
now reads what TCC says now. The daemon starts this as exactly `python -I -m ferb.device.grant_probe` (the packaged
app's agent accepts only exact entry points, so it takes no arguments); as the daemon's own child it runs as FERB's
agent app, whose grants these are. It uses ctypes only (no PyObjC) so it starts quickly, reads nothing but the two
answers and prints `{"screen_recording": "granted"|"denied", "input_monitoring": "granted"|"denied"|...}`."""
import ctypes
import json
import sys

_LISTEN_EVENT = 1                                    # kIOHIDRequestTypeListenEvent
_HID = {0: "granted", 1: "denied", 2: "not_determined"}


def read() -> dict[str, str]:
    graphics = ctypes.CDLL("/System/Library/Frameworks/CoreGraphics.framework/CoreGraphics")
    graphics.CGPreflightScreenCaptureAccess.restype = ctypes.c_bool
    iokit = ctypes.CDLL("/System/Library/Frameworks/IOKit.framework/IOKit")
    iokit.IOHIDCheckAccess.argtypes = [ctypes.c_uint32]
    iokit.IOHIDCheckAccess.restype = ctypes.c_uint32
    return {"screen_recording": "granted" if graphics.CGPreflightScreenCaptureAccess() else "denied",
            "input_monitoring": _HID.get(iokit.IOHIDCheckAccess(_LISTEN_EVENT), "unknown")}


def main() -> int:
    if len(sys.argv) > 1:
        print(json.dumps({"error": "takes no arguments"}))
        return 64
    print(json.dumps(read()))
    return 0


if __name__ == "__main__":
    sys.exit(main())
