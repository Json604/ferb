"""Read TCC grant facts and expose explicit setup permission requests. Must not silently prompt during capability checks.

TCC permission checks — never prompting — and open-settings helpers.

Grants belong to the responsible process: the daemon runs these checks on itself, never the
CLI's process. Automation can be *requested* (prompt) only from the daemon, so the grant attaches to it."""

import asyncio
import ctypes
import subprocess
import threading
from concurrent.futures import Future
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


_automation_jobs: dict[tuple[str, bool], Future[Status]] = {}
_automation_lock = threading.Lock()


def automation_check(bundle_id: str, ask: bool = False) -> Future[Status]:
    """One in-process native check per target/mode, away from the event loop.

    macOS can block even for ask=False. A daemon thread preserves FERB's TCC identity without keeping
    interpreter shutdown hostage. Cancellation stops waiting, not the native call; retries share that call.
    This thread only checks/requests permission: it never executes an AppleScript after a cancelled wait.
    """
    key = bundle_id, ask
    with _automation_lock:
        if key in _automation_jobs:
            return _automation_jobs[key]
        future: Future[Status] = Future()
        _automation_jobs[key] = future

        def check() -> None:
            try:
                future.set_result(automation(bundle_id, ask=ask))
            except BaseException as exc:
                future.set_exception(exc)
            finally:
                with _automation_lock:
                    _automation_jobs.pop(key, None)

        try:
            threading.Thread(target=check, name='ferb-automation-permission', daemon=True).start()
        except BaseException:
            _automation_jobs.pop(key, None)
            raise
        return future


async def automation_async(bundle_id: str, ask: bool = False) -> Status:
    # A caller timeout/cancellation must not cancel the shared, still-running native check.
    pending = asyncio.wrap_future(automation_check(bundle_id, ask))
    # If every caller stops waiting, a later native error must not become an unhandled-loop error.
    pending.add_done_callback(lambda done: None if done.cancelled() else done.exception())
    return await asyncio.shield(pending)


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
