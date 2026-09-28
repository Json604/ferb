"""TCC permission checks (section 8 step 1) — never prompting — and open-settings helpers.

Grants belong to the responsible process: the daemon runs these checks on itself (section 8 step 6), never the
CLI's process. Automation can be *requested* (prompt) only from the daemon, so the grant attaches to it.
"""

import ctypes
import subprocess
from enum import Enum
from pathlib import Path

import ApplicationServices as AX
import Quartz


class Status(Enum):
    GRANTED = "granted"
    DENIED = "denied"
    NOT_DETERMINED = "not_determined"
    TARGET_NOT_RUNNING = "target_not_running"
    UNKNOWN = "unknown"


def accessibility() -> Status:
    return Status.GRANTED if AX.AXIsProcessTrustedWithOptions({AX.kAXTrustedCheckOptionPrompt: False}) \
        else Status.DENIED


def screen_recording() -> Status:
    """Note: macOS only reflects a new Screen Recording grant after the process restarts."""
    return Status.GRANTED if Quartz.CGPreflightScreenCaptureAccess() else Status.DENIED


_iokit = ctypes.CDLL("/System/Library/Frameworks/IOKit.framework/IOKit")
_iokit.IOHIDCheckAccess.argtypes = [ctypes.c_uint32]
_iokit.IOHIDCheckAccess.restype = ctypes.c_uint32
_LISTEN_EVENT = 1                                    # kIOHIDRequestTypeListenEvent
_HID = {0: Status.GRANTED, 1: Status.DENIED, 2: Status.NOT_DETERMINED}


def input_monitoring() -> Status:
    return _HID.get(_iokit.IOHIDCheckAccess(_LISTEN_EVENT), Status.UNKNOWN)


def full_disk_access() -> Status:
    """No API: probe a file only readable with Full Disk Access."""
    try:
        with open(Path.home() / "Library/Application Support/com.apple.TCC/TCC.db", "rb") as f:
            f.read(16)
        return Status.GRANTED
    except PermissionError:
        return Status.DENIED
    except OSError:
        return Status.UNKNOWN


_cs = ctypes.CDLL("/System/Library/Frameworks/CoreServices.framework/CoreServices")


class _AEDesc(ctypes.Structure):
    _fields_ = [("descriptorType", ctypes.c_uint32), ("dataHandle", ctypes.c_void_p)]


_cs.AECreateDesc.argtypes = [ctypes.c_uint32, ctypes.c_void_p, ctypes.c_long, ctypes.POINTER(_AEDesc)]
_cs.AECreateDesc.restype = ctypes.c_int16
_cs.AEDisposeDesc.argtypes = [ctypes.POINTER(_AEDesc)]
_cs.AEDeterminePermissionToAutomateTarget.argtypes = [ctypes.POINTER(_AEDesc), ctypes.c_uint32, ctypes.c_uint32,
                                                      ctypes.c_bool]
_cs.AEDeterminePermissionToAutomateTarget.restype = ctypes.c_int32
_AE = {0: Status.GRANTED, -1743: Status.DENIED, -1744: Status.NOT_DETERMINED, -600: Status.TARGET_NOT_RUNNING}


def _fourcc(s: str) -> int:
    return int.from_bytes(s.encode(), "big")


def automation(bundle_id: str, ask: bool = False) -> Status:
    """Apple Events permission for this process -> bundle_id. ask=True shows the system prompt (daemon only;
    the target app must be running). ask=False never prompts."""
    data = bundle_id.encode()
    desc = _AEDesc()
    if _cs.AECreateDesc(_fourcc("bund"), data, len(data), ctypes.byref(desc)) != 0:
        return Status.UNKNOWN
    try:
        rc = _cs.AEDeterminePermissionToAutomateTarget(ctypes.byref(desc), _fourcc("****"), _fourcc("****"), ask)
    finally:
        _cs.AEDisposeDesc(ctypes.byref(desc))
    return _AE.get(rc, Status.UNKNOWN)


PANES = {
    "accessibility": "Privacy_Accessibility",
    "screen_recording": "Privacy_ScreenCapture",
    "input_monitoring": "Privacy_ListenEvent",
    "full_disk_access": "Privacy_AllFiles",
    "automation": "Privacy_Automation",
}


def open_pane(permission: str) -> None:
    """Open System Settings on the right Privacy pane (FERB can't grant anything itself)."""
    subprocess.run(["open", f"x-apple.systempreferences:com.apple.preference.security?{PANES[permission]}"],
                   check=False)


def check_all() -> dict[str, str]:
    return {"accessibility": accessibility().value, "screen_recording": screen_recording().value,
            "input_monitoring": input_monitoring().value, "full_disk_access": full_disk_access().value}
