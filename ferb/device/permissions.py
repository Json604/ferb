"""Read TCC grant facts and expose explicit setup permission requests. Must not silently prompt during capability checks.

TCC permission checks — never prompting — and open-settings helpers.

Grants belong to the responsible process: the daemon runs these checks on itself, never the
CLI's process. Automation can be *requested* (prompt) only from the daemon, so the grant attaches to it."""

import asyncio
import ctypes
import os
import subprocess
import threading
from concurrent.futures import Future
from enum import Enum
from pathlib import Path
from typing import Any, Callable, Iterable

import ApplicationServices as AX
import Quartz


class Status(Enum):
    GRANTED = "granted"
    DENIED = "denied"
    NOT_DETERMINED = "not_determined"
    TARGET_NOT_RUNNING = "target_not_running"
    UNKNOWN = "unknown"


def _ax_trusted(prompt: bool) -> bool:
    return bool(AX.AXIsProcessTrustedWithOptions({AX.kAXTrustedCheckOptionPrompt: prompt}))


def _ax_read_works() -> bool:
    """A real Accessibility read: the frontmost app's role. The trust flag can be stale; a read cannot."""
    from AppKit import NSWorkspace
    app = NSWorkspace.sharedWorkspace().frontmostApplication()
    if app is None:
        return False
    err, _ = AX.AXUIElementCopyAttributeValue(AX.AXUIElementCreateApplication(app.processIdentifier()), "AXRole", None)
    return err == 0


def _other_app_window_title_readable() -> bool:
    """A real Screen Recording read: another process's window title is only listed with the grant."""
    me = os.getpid()
    for w in Quartz.CGWindowListCopyWindowInfo(Quartz.kCGWindowListOptionAll, Quartz.kCGNullWindowID) or []:
        if w.get("kCGWindowOwnerPID") != me and w.get("kCGWindowName"):
            return True
    return False


_PROBES: dict[str, Callable[[], bool]] = {"accessibility": _ax_read_works,
                                         "screen_recording": _other_app_window_title_readable}


def _api_says_granted(name: str) -> bool:
    if name == "accessibility":
        return _ax_trusted(False)
    return bool(Quartz.CGPreflightScreenCaptureAccess())


def confirmed(name: str, probe: Callable[[str], bool] | None = None) -> bool:
    """A grant counts only when the plain API says so and a real read of what it protects works (the API alone can be
    stale). Never prompts. `probe` is injectable; it defaults to the real read for `name`."""
    if not _api_says_granted(name):
        return False
    try:
        return bool(probe(name)) if probe is not None else _PROBES[name]()
    except Exception:
        return False


def accessibility() -> Status:
    return Status.GRANTED if confirmed("accessibility") else Status.DENIED


def screen_recording() -> Status:
    """Note: macOS only reflects a new Screen Recording grant after the process restarts."""
    return Status.GRANTED if confirmed("screen_recording") else Status.DENIED


_iokit = ctypes.CDLL("/System/Library/Frameworks/IOKit.framework/IOKit")
_iokit.IOHIDCheckAccess.argtypes = [ctypes.c_uint32]
_iokit.IOHIDCheckAccess.restype = ctypes.c_uint32
_iokit.IOHIDRequestAccess.argtypes = [ctypes.c_uint32]
_iokit.IOHIDRequestAccess.restype = ctypes.c_bool
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
    "microphone": "Privacy_Microphone",
    "speech_recognition": "Privacy_SpeechRecognition",
    "hearing": "Privacy_SpeechRecognition",          # FERB Voice's Speech Recognition (L26)
    "clipboard": "Privacy_Pasteboard",               # Paste from Other Apps (macOS 15.4+; I3)
}


def pane_url(permission: str) -> str:
    """The exact System Settings anchor for one Privacy & Security pane."""
    return f"x-apple.systempreferences:com.apple.settings.PrivacySecurity.extension?{PANES[permission]}"


def notifications_pane_url(bundle_id: str) -> str:
    """System Settings → Notifications → that app."""
    return f"x-apple.systempreferences:com.apple.Notifications-Settings.extension?id={bundle_id}"


def app_panes(bundle_id: str) -> dict[str, str]:
    """The panes for FERB.app's own rows (L34 C), which FERB.app reads and requests itself."""
    return {"notifications": notifications_pane_url(bundle_id), "microphone": pane_url("microphone"),
            "speech_recognition": pane_url("speech_recognition")}


def open_pane(permission: str) -> None:
    """Open System Settings on the right Privacy pane (FERB can't grant anything itself)."""
    subprocess.run(["open", pane_url(permission)], check=False)


def check_all() -> dict[str, str]:
    return {"accessibility": accessibility().value, "screen_recording": screen_recording().value,
            "input_monitoring": input_monitoring().value, "full_disk_access": full_disk_access().value}


# ---------- L34 C: the setup window's rows (also `ferb setup`) ----------

# Every FERB permission is needed and none is ever labelled optional (owner, 2026-10-26): setup lists them all with
# one Allow all; Automation is asked per app at first use. `required` stays on the rows for older apps (always true).
DAEMON_PERMISSIONS = ("accessibility", "screen_recording", "full_disk_access", "input_monitoring")
SETUP_VERSION = 27                      # `[setup] permissions_version` once every permission was granted
REQUESTABLE = frozenset({"accessibility", "screen_recording", "input_monitoring", "clipboard", "hearing"})
# a new grant of these reaches a running process only after it restarts (Screen Recording: CGPreflight… stays false;
# Input Monitoring likewise on a friend's Macs, 2026-10-27; Full Disk Access may need it too): the window restarts the
# service when FERB comes back to the front after the Grant, and offers "Restart FERB to check"
RESTART_TO_CHECK = frozenset({"screen_recording", "input_monitoring", "full_disk_access"})
FDA_WAITS = "FERB's shell sandbox and network proxy must be working first"


def fda_ready(containment: dict[str, Any]) -> bool:
    """CQ5: Full Disk Access only once containment exists: the shell's sandbox profile works, the shell runs inside it,
    and the egress proxy is there. `proxy_alive` counts when it was probed (`doctor.check`); the daemon's setup rows use
    the facts it started with, with no probe per poll."""
    return bool(containment.get("profile_ok") and containment.get("shell") and containment.get("proxy")
                and containment.get("proxy_alive", True))


def hearing_row_status(hearing: dict[str, Any]) -> tuple[str, str]:
    """(status, blocked) of FERB Voice's Speech Recognition from the daemon's hearing probe (`_hearing_status`)."""
    if not hearing.get("installed"):
        return Status.UNKNOWN.value, "FERB Voice isn't in this FERB.app; download FERB again"
    if not hearing.get("signed"):
        return Status.UNKNOWN.value, "FERB Voice's signature isn't the one setup recorded; set up FERB again"
    speech = str(hearing.get("speech") or "")
    if speech == "granted":
        return Status.GRANTED.value, ""
    if speech in ("denied", "restricted"):
        return Status.DENIED.value, ""
    if speech == "not_determined":
        return Status.NOT_DETERMINED.value, ""
    return Status.UNKNOWN.value, ""


def rows(grants: dict[str, str], containment: dict[str, Any],
         hearing: dict[str, Any] | None = None) -> list[dict[str, Any]]:
    """The daemon's own grants as setup rows: `process` "daemon" is FERB's agent app (the daemon's TCC identity);
    "voice" is FERB Voice, included only when its hearing probe was asked for."""
    ready = fda_ready(containment)
    out: list[dict[str, Any]] = [
        {"id": p, "status": grants.get(p, Status.UNKNOWN.value), "process": "daemon", "required": True,
         "pane": pane_url(p), "requestable": p in REQUESTABLE, "restart_to_check": p in RESTART_TO_CHECK,
         "blocked": "" if p != "full_disk_access" or ready else FDA_WAITS}
        for p in DAEMON_PERMISSIONS]
    if hearing is not None:
        status, blocked = hearing_row_status(hearing)
        out.append({"id": "hearing", "status": status, "process": "voice", "required": True,
                    "pane": pane_url("hearing"), "requestable": True, "restart_to_check": False, "blocked": blocked})
    return out


def clipboard_row(state: str | None) -> dict[str, Any]:
    """I3 as a setup row (owner decision 2, 2026-10-25; needed like every other): whether macOS lets the daemon read the clipboard
    without asking, from the pasteboard's accessBehavior (never its contents). "default": macOS hasn't asked yet, so
    Grant lets it ask once (`setup.permission_request {permission: "clipboard"}`); "ask" and "deny" need FERB set to
    Allow in Paste from Other Apps; "unrestricted" is macOS before 15.4."""
    status = {"allow": Status.GRANTED.value, "unrestricted": Status.GRANTED.value,
              "default": Status.NOT_DETERMINED.value, "ask": Status.DENIED.value,
              "deny": Status.DENIED.value}.get(state or "", Status.UNKNOWN.value)
    return {"id": "clipboard", "status": status, "process": "daemon", "required": True, "pane": pane_url("clipboard"),
            "requestable": state == "default", "restart_to_check": False, "blocked": ""}


def missing(rows: Iterable[dict[str, Any]]) -> list[tuple[str, str]]:
    """(id, status) of every row not granted: setup is done only when this is empty (the setup window's Done and
    `ferb setup` both use it; `[setup] permissions_version` is never written otherwise)."""
    return [(str(r["id"]), str(r["status"])) for r in rows if r["status"] != Status.GRANTED.value]


def report_rows(report: dict[str, Any]) -> list[dict[str, Any]]:
    """The daemon's own rows from a `doctor.check` reply asked with `hearing: true`: its four grants, FERB Voice's
    Speech Recognition and the clipboard (FERB.app's own Notifications and Microphone are the app's to check)."""
    grants = {p: str(report.get("permissions", {}).get(p, Status.UNKNOWN.value)) for p in DAEMON_PERMISSIONS}
    return rows(grants, report.get("containment") or {}, report.get("hearing") or {}) + [
        clipboard_row(report.get("clipboard"))]


def record_done(config: Any) -> None:
    """Setup recorded as done (only once `missing` is empty); a later version is never lowered."""
    current = int(config.get("setup", "permissions_version", 0) or 0)
    config.set("setup", "permissions_version", max(SETUP_VERSION, current))


def _request_screen_capture() -> None:
    Quartz.CGRequestScreenCaptureAccess()


def _request_listen_event() -> None:
    _iokit.IOHIDRequestAccess(_LISTEN_EVENT)


def request(permission: str) -> Status:
    """Ask macOS for one grant on the owner's Grant (setup only): macOS shows its own prompt and lists FERB in that
    pane. Run in the daemon, so the grant attaches to the daemon's process. Full Disk Access has no request API (the
    owner adds FERB in the pane); Automation is per app (`automation(bundle_id, ask=True)`); hearing is FERB Voice's
    own request (`speech_request_argv`); the clipboard is one read by the daemon (`clipboard.request_access`)."""
    if permission == "accessibility":
        _ax_trusted(True)
        return accessibility()
    if permission == "screen_recording":
        _request_screen_capture()
        return screen_recording()
    if permission == "input_monitoring":
        _request_listen_event()
        return input_monitoring()
    raise ValueError(f"{permission!r} has no macOS request; add FERB in its System Settings pane")


def speech_request_argv(app: Path) -> list[str]:
    """FERB Voice asks macOS once for Speech Recognition (L26); `open -W` returns after the owner answered."""
    return ["/usr/bin/open", "-n", "-W", str(app), "--args", "request-speech"]


def automation_candidates(apps: Iterable[Any], running: set[str]) -> list[tuple[str, str]]:
    """(name, bundle id) of the scriptable apps running now: macOS can ask about an app only while it runs."""
    return [(a.name, a.bundle_id) for a in apps if a.scriptable and a.bundle_id in running]


def grant_target(bundle: Path | None, executable: str) -> str:
    """What the owner switches on in Privacy & Security for the daemon's grants: FERB's agent app inside FERB.app,
    or the real interpreter a checkout's daemon runs on."""
    from ferb.device.install import BUNDLE_AGENT_APP
    return str(bundle / BUNDLE_AGENT_APP) if bundle is not None else os.path.realpath(executable)
