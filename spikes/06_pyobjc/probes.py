"""Spike 6 probes: frontmost app, window titles, Chrome's AX tree, and TCC status — never prompting.

Every check here is read-only and must not show a permission dialog (askUserIfNeeded = false everywhere).
"""

import ctypes
from dataclasses import dataclass
from enum import Enum
from pathlib import Path

import ApplicationServices as AX
import Quartz
from AppKit import NSWorkspace


# ---------- frontmost app / window titles ----------

@dataclass(frozen=True)
class AppRef:
    bundle_id: str
    name: str
    pid: int


def frontmost_app() -> AppRef:  # NSWorkspace: no permission needed
    a = NSWorkspace.sharedWorkspace().frontmostApplication()
    return AppRef(str(a.bundleIdentifier() or ""), str(a.localizedName() or ""), int(a.processIdentifier()))


def ax_attr(el, name):
    err, val = AX.AXUIElementCopyAttributeValue(el, name, None)
    return val if err == 0 else None


def ax_window_titles(pid: int) -> list[str]:
    """Window titles via Accessibility (needs Accessibility). Doesn't activate the app."""
    app = AX.AXUIElementCreateApplication(pid)
    return [str(ax_attr(w, "AXTitle") or "") for w in (ax_attr(app, "AXWindows") or [])]


def ax_focused_window_title(pid: int) -> str | None:
    win = ax_attr(AX.AXUIElementCreateApplication(pid), "AXFocusedWindow")
    return None if win is None else str(ax_attr(win, "AXTitle") or "")


def cg_window_titles(pid: int) -> list[str]:
    """Window titles via CoreGraphics. Other apps' titles are withheld without Screen Recording."""
    wins = Quartz.CGWindowListCopyWindowInfo(Quartz.kCGWindowListOptionAll, Quartz.kCGNullWindowID)
    return [str(w.get("kCGWindowName", "")) for w in wins if w["kCGWindowOwnerPID"] == pid
            and w.get("kCGWindowLayer") == 0 and w.get("kCGWindowName")]


# ---------- Chrome's web AX tree ----------

def ax_walk(el, depth: int = 0, max_depth: int = 60):
    yield el
    if depth < max_depth:
        for child in ax_attr(el, "AXChildren") or []:
            yield from ax_walk(child, depth + 1, max_depth)


def web_elements(pid: int) -> list[tuple[str, str]]:
    """(role, title/description) of every element inside any AXWebArea of the app."""
    out: list[tuple[str, str]] = []
    for el in ax_walk(AX.AXUIElementCreateApplication(pid)):
        if ax_attr(el, "AXRole") == "AXWebArea":
            for e in ax_walk(el):
                label = " ".join(str(ax_attr(e, a) or "") for a in ("AXTitle", "AXDescription", "AXValue")).strip()
                out.append((str(ax_attr(e, "AXRole") or ""), label))
    return out


def set_manual_accessibility(pid: int, on: bool = True) -> int:
    """Chromium's switch to build the web AX tree without an assistive technology running. Returns AXError."""
    return AX.AXUIElementSetAttributeValue(AX.AXUIElementCreateApplication(pid), "AXManualAccessibility", on)


# ---------- TCC status (no prompts) ----------

class Tcc(Enum):
    GRANTED = "granted"
    DENIED = "denied"
    NOT_DETERMINED = "not_determined"
    TARGET_NOT_RUNNING = "target_not_running"
    UNKNOWN = "unknown"


def accessibility() -> Tcc:
    opts = {AX.kAXTrustedCheckOptionPrompt: False}
    return Tcc.GRANTED if AX.AXIsProcessTrustedWithOptions(opts) else Tcc.DENIED


def screen_recording() -> Tcc:
    return Tcc.GRANTED if Quartz.CGPreflightScreenCaptureAccess() else Tcc.DENIED


_cs = ctypes.CDLL("/System/Library/Frameworks/CoreServices.framework/CoreServices")


class _AEDesc(ctypes.Structure):
    _fields_ = [("descriptorType", ctypes.c_uint32), ("dataHandle", ctypes.c_void_p)]


def _fourcc(s: str) -> int:
    return int.from_bytes(s.encode(), "big")


_cs.AECreateDesc.argtypes = [ctypes.c_uint32, ctypes.c_void_p, ctypes.c_long, ctypes.POINTER(_AEDesc)]
_cs.AECreateDesc.restype = ctypes.c_int16
_cs.AEDisposeDesc.argtypes = [ctypes.POINTER(_AEDesc)]
_cs.AEDeterminePermissionToAutomateTarget.argtypes = [ctypes.POINTER(_AEDesc), ctypes.c_uint32, ctypes.c_uint32,
                                                      ctypes.c_bool]
_cs.AEDeterminePermissionToAutomateTarget.restype = ctypes.c_int32

_AUTOMATION = {0: Tcc.GRANTED, -1743: Tcc.DENIED, -1744: Tcc.NOT_DETERMINED, -600: Tcc.TARGET_NOT_RUNNING}


def automation(bundle_id: str) -> Tcc:
    """Apple Events permission for this process -> bundle_id, without asking (askUserIfNeeded = false)."""
    data = bundle_id.encode()
    desc = _AEDesc()
    if _cs.AECreateDesc(_fourcc("bund"), data, len(data), ctypes.byref(desc)) != 0:
        return Tcc.UNKNOWN
    try:
        status = _cs.AEDeterminePermissionToAutomateTarget(ctypes.byref(desc), _fourcc("****"), _fourcc("****"),
                                                           False)
    finally:
        _cs.AEDisposeDesc(ctypes.byref(desc))
    return _AUTOMATION.get(status, Tcc.UNKNOWN)


def full_disk_access() -> Tcc:
    """No API: probe a file only readable with Full Disk Access."""
    try:
        with open(Path.home() / "Library/Application Support/com.apple.TCC/TCC.db", "rb") as f:
            f.read(16)
        return Tcc.GRANTED
    except PermissionError:
        return Tcc.DENIED
    except FileNotFoundError:
        return Tcc.UNKNOWN
