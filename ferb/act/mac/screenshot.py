"""Capture one exact window into memory and map image coordinates. Must not capture the desktop or choose a substitute window.

On-demand capture of one app window. Blocking pyobjc calls — callers run them
in a thread.

- Only the target window: ScreenCaptureKit's desktop-independent window filter renders that window's own content,
  never the rest of the screen and never windows on top of it.
- In memory only: the frame is encoded to JPEG bytes and handed back; nothing here writes a file.
- One capture at a time in this process, and one quiet retry when ScreenCaptureKit fails to start its capture
  (SCStreamErrorDomain -3811, "Failed to start stream"): live, those failures came alone, between successful captures
  of the same window seconds apart (38 of 269 screenshots, 2026-09-29 → 10-30), with no other FERB capture recorded
  at the same time.
- `window_at` answers "which normal window is topmost at this point": the fallback for checking that a click lands on
  the window the model saw when Accessibility's hit test (accessibility.hit) can't answer."""

import threading
import time
from dataclasses import dataclass
from typing import Any

MAX_EDGE = 1568               # long side of the image the model gets (px); larger windows are scaled down
JPEG_QUALITY = 0.8
TIMEOUT_S = 10.0
START_FAILED = -3811          # SCStreamErrorFailedToStart: "Failed to start stream due to audio/video capture failure"
RETRY_AFTER_S = 0.3
_capturing = threading.Lock()  # one ScreenCaptureKit capture at a time in this process


@dataclass(frozen=True)
class Window:
    id: int                                        # CGWindowID (== SCWindow.windowID)
    pid: int
    bounds: tuple[float, float, float, float]      # x, y, w, h in screen points (top-left origin)
    title: str
    layer: int = 0                                 # its window level (kCGWindowLayer): 0 for an app's normal windows
    alpha: float = 1.0                             # kCGWindowAlpha: 0 draws nothing and takes no click


def _info_windows(everywhere: bool = False, strict: bool = False) -> list[dict[str, Any]]:
    """On-screen windows front to back; `everywhere` also lists windows on other Spaces and full-screen apps. Quartz
    answering NULL reads as no windows, or with `strict` raises LookupError (round 4, N2: can't tell, never empty)."""
    import Quartz
    opts = (Quartz.kCGWindowListOptionAll if everywhere else Quartz.kCGWindowListOptionOnScreenOnly) \
        | Quartz.kCGWindowListExcludeDesktopElements
    listed = Quartz.CGWindowListCopyWindowInfo(opts, Quartz.kCGNullWindowID)
    if listed is None and strict:
        raise LookupError("the window server gave no window list")
    return list(listed or [])


def window_on_screen(pid: int, rect: tuple[float, float, float, float],
                     like: tuple[float, float, float, float] | None = None, title: str | None = None,
                     window: int | None = None) -> bool:
    """True when the frontmost normal window (layer 0, not transparent) at the rect's centre, in the window server's
    front-to-back on-screen order, is the target (L33: the cursor never points at an element in a hidden, other-Space
    or covered window; FERB's own overlay sits above layer 0, so it never counts). The target is a window of the
    process; with `window`, that window (a screenshot's); with `like`, the one window at those bounds (and with `title`,
    the one the window server names so: a browser window carries its tab's title), so the spot is in the very window
    that was matched; none or several there: False. Synchronous Quartz work: call it off the event loop."""
    cx, cy = rect[0] + rect[2] / 2, rect[1] + rect[3] / 2
    normal = [d for d in _info_windows()
              if int(d.get("kCGWindowLayer", 1)) == 0 and float(d.get("kCGWindowAlpha", 1)) > 0]

    def target(d: dict[str, Any]) -> bool:
        w = _window(d)
        return w.pid == pid and (window is None or w.id == window) and (like is None or (
            all(abs(a - b) <= 2 for a, b in zip(w.bounds, like))
            and (title is None or " ".join(w.title.split()) == " ".join(title.split()))))
    if like is not None and sum(1 for d in normal if target(d)) != 1:
        return False
    front = next((d for d in normal if _holds(_window(d).bounds, cx, cy)), None)
    return front is not None and target(front)


def _holds(bounds: tuple[float, float, float, float], x: float, y: float) -> bool:
    bx, by, bw, bh = bounds
    return bx <= x <= bx + bw and by <= y <= by + bh


def _window(d: dict[str, Any]) -> Window:
    b = d.get("kCGWindowBounds") or {}
    return Window(int(d["kCGWindowNumber"]), int(d.get("kCGWindowOwnerPID", -1)),
                  (float(b.get("X", 0)), float(b.get("Y", 0)), float(b.get("Width", 0)), float(b.get("Height", 0))),
                  str(d.get("kCGWindowName") or ""), int(d.get("kCGWindowLayer", 0)),
                  float(d.get("kCGWindowAlpha", 1)))


def front_window(pid: int, like: tuple[float, float, float, float] | None = None,
                 title: str | None = None) -> Window | None:
    """The unique process-owned normal window matching exact bounds, or None. Never choose by order or size. When
    several share the bounds (maximized windows), `title` (the window's own) picks the one the window server names so."""
    mine = [_window(d) for d in _info_windows(True)
            if int(d.get("kCGWindowOwnerPID", -1)) == pid and int(d.get("kCGWindowLayer", 1)) == 0
            and float(d.get("kCGWindowAlpha", 1)) > 0]
    if like is not None:
        mine = [w for w in mine if all(abs(a - b) <= 2 for a, b in zip(w.bounds, like))]
    if len(mine) > 1 and title:
        mine = [w for w in mine if " ".join(w.title.split()) == " ".join(title.split())]
    return mine[0] if len(mine) == 1 else None


def normal_windows() -> list[Window]:
    """On-screen normal windows (layer 0, not transparent, with an area), front to back: the first is the one in front
    of the owner. Menus, the Dock and overlays sit on higher layers (L12: the front window at send)."""
    out = []
    for d in _info_windows():
        w = _window(d)
        if int(d.get("kCGWindowLayer", 1)) == 0 and float(d.get("kCGWindowAlpha", 1)) > 0 and w.bounds[2] > 0 \
                and w.bounds[3] > 0:
            out.append(w)
    return out


def on_screen(window_id: int) -> bool:
    return any(int(d["kCGWindowNumber"]) == window_id for d in _info_windows())


def window(window_id: int) -> Window | None:
    return next((_window(d) for d in _info_windows(True) if int(d["kCGWindowNumber"]) == window_id), None)


def windows_at(x: float, y: float) -> list[Window] | None:
    """Every on-screen window at this point, all levels, front to back, with its level and alpha (fix round 3, H1: the
    notch's panel sits above layer 0); None when the window list can't be read, NULL included (round 4, N2)."""
    try:
        found = []
        for d in _info_windows(strict=True):         # every level; the caller decides what takes clicks
            w = _window(d)
            bx, by, bw, bh = w.bounds
            if bx <= x < bx + bw and by <= y < by + bh:
                found.append(w)
        return found
    except Exception:
        return None


def window_at(x: float, y: float, max_layer: int = 0) -> Window | None:
    """The topmost visible window at this point among layers <= max_layer (the fallback when Accessibility can't
    hit-test: higher layers hold full-screen click-through overlays the window list can't tell apart)."""
    for d in _info_windows():
        if float(d.get("kCGWindowAlpha", 1)) <= 0 or int(d.get("kCGWindowLayer", 0)) > max_layer:
            continue
        w = _window(d)
        bx, by, bw, bh = w.bounds
        if bx <= x < bx + bw and by <= y < by + bh:
            return w
    return None


class CaptureError(RuntimeError):
    """A ScreenCaptureKit failure with its error domain and code (I7), for the result, the trace and the journal."""

    def __init__(self, message: str, domain: str = "", code: int | None = None) -> None:
        super().__init__(message)
        self.domain, self.code = domain, code


# userInfo keys that explain a failure; anything else in it (objects, paths) is left out.
_REASONS = ("NSLocalizedFailureReason", "NSLocalizedRecoverySuggestion", "NSDebugDescription")


def capture_error(error: Any) -> CaptureError:
    """An NSError from ScreenCaptureKit, kept whole enough to diagnose: its text, domain, code, the reasons in its
    userInfo and its underlying error's domain and code (the live "Failed to start stream due to audio/video capture
    failure" came with no code at all)."""
    text = str(error.localizedDescription())
    domain, code = str(error.domain()), int(error.code())
    info = error.userInfo() or {}
    parts = [f"{domain} {code}"] + [str(info.get(key)) for key in _REASONS if info.get(key)]
    under = info.get("NSUnderlyingError")
    if under is not None and hasattr(under, "domain") and hasattr(under, "code"):
        parts.append(f"underlying {under.domain()} {under.code()}")
    return CaptureError(f"{text} ({'; '.join(parts)})", domain, code)


def _wait(start: Any) -> Any:
    """Run a ScreenCaptureKit call that answers through a completion handler; return its (value, error)."""
    done, box = threading.Event(), {}

    def handler(value: Any, error: Any) -> None:
        box["v"], box["e"] = value, error
        done.set()
    start(handler)
    if not done.wait(TIMEOUT_S):
        raise TimeoutError("the screen capture didn't answer")
    if box.get("e") is not None:
        raise capture_error(box["e"])
    return box["v"]


def capture(w: Window, max_edge: int | None = MAX_EDGE) -> tuple[bytes, int, int]:
    """JPEG bytes of exactly this window, and the image size (px). A minimized window is refused by name before
    ScreenCaptureKit is asked; FERB never unminimizes it (background first, I7). One capture at a time; a capture that
    fails to start is tried once more after RETRY_AFTER_S (a read: nothing is delivered twice)."""
    from ferb.act.mac import accessibility
    from ferb.core.errors import Refused
    if accessibility.window_minimized(w.pid, w.bounds):
        raise Refused("that window is minimized, and FERB doesn't bring windows back on its own (it works in the "
                      "background): it can be captured once it's shown again")
    with _capturing:
        try:
            return _capture_once(w, max_edge)
        except CaptureError as e:
            if e.code != START_FAILED:
                raise
        time.sleep(RETRY_AFTER_S)
        return _capture_once(w, max_edge)


def _capture_once(w: Window, max_edge: int | None) -> tuple[bytes, int, int]:
    import ScreenCaptureKit as SCK
    content = _wait(lambda h: SCK.SCShareableContent.getShareableContentExcludingDesktopWindows_onScreenWindowsOnly_completionHandler_(
        True, False, h))                              # a window on another Space / full screen can be captured too
    sc = next((x for x in content.windows() if int(x.windowID()) == w.id), None)
    if sc is None:
        raise LookupError("that window is no longer on screen")
    flt = SCK.SCContentFilter.alloc().initWithDesktopIndependentWindow_(sc)
    cfg = SCK.SCStreamConfiguration.alloc().init()
    scale = float(flt.pointPixelScale() or 1.0)
    fw, fh = w.bounds[2] * scale, w.bounds[3] * scale
    k = min(1.0, max_edge / max(fw, fh)) if max_edge is not None else 1.0
    width, height = max(1, int(fw * k)), max(1, int(fh * k))
    cfg.setWidth_(width)
    cfg.setHeight_(height)
    cfg.setShowsCursor_(False)
    image = _wait(lambda h: SCK.SCScreenshotManager.captureImageWithFilter_configuration_completionHandler_(flt, cfg, h))
    return _jpeg(image), width, height


def _jpeg(image: Any) -> bytes:
    import Quartz
    from Foundation import NSMutableData
    data = NSMutableData.data()
    dest = Quartz.CGImageDestinationCreateWithData(data, "public.jpeg", 1, None)
    Quartz.CGImageDestinationAddImage(dest, image, {Quartz.kCGImageDestinationLossyCompressionQuality: JPEG_QUALITY})
    if not Quartz.CGImageDestinationFinalize(dest):
        raise RuntimeError("couldn't encode the screenshot")
    return bytes(data)
