"""On-demand capture of one app window (section 3 act/mac, build step 9). Blocking pyobjc calls — callers run them
in a thread.

- Only the target window: ScreenCaptureKit's desktop-independent window filter renders that window's own content,
  never the rest of the screen and never windows on top of it.
- In memory only: the frame is encoded to JPEG bytes and handed back; nothing here writes a file.
- `window_at` answers "which normal window is topmost at this point": the fallback for checking that a click lands on
  the window the model saw when Accessibility's hit test (accessibility.hit) can't answer.
"""

import threading
from dataclasses import dataclass
from typing import Any

MAX_EDGE = 1568               # long side of the image the model gets (px); larger windows are scaled down
JPEG_QUALITY = 0.8
TIMEOUT_S = 10.0


@dataclass(frozen=True)
class Window:
    id: int                                        # CGWindowID (== SCWindow.windowID)
    pid: int
    bounds: tuple[float, float, float, float]      # x, y, w, h in screen points (top-left origin)
    title: str


def _info_windows() -> list[dict[str, Any]]:
    import Quartz
    opts = Quartz.kCGWindowListOptionOnScreenOnly | Quartz.kCGWindowListExcludeDesktopElements
    return list(Quartz.CGWindowListCopyWindowInfo(opts, Quartz.kCGNullWindowID) or [])   # front to back


def _window(d: dict[str, Any]) -> Window:
    b = d.get("kCGWindowBounds") or {}
    return Window(int(d["kCGWindowNumber"]), int(d.get("kCGWindowOwnerPID", -1)),
                  (float(b.get("X", 0)), float(b.get("Y", 0)), float(b.get("Width", 0)), float(b.get("Height", 0))),
                  str(d.get("kCGWindowName") or ""))


MIN_SIDE = 50                 # smaller layer-0 windows are helpers / strips, not something to look at


def front_window(pid: int, like: tuple[float, float, float, float] | None = None) -> Window | None:
    """The app's window to capture: the on-screen normal (layer 0) window matching `like` (its Accessibility window's
    frame) if there is one, else its frontmost one of a real size."""
    mine = [_window(d) for d in _info_windows()
            if int(d.get("kCGWindowOwnerPID", -1)) == pid and int(d.get("kCGWindowLayer", 1)) == 0
            and float(d.get("kCGWindowAlpha", 1)) > 0]
    mine = [w for w in mine if w.bounds[2] >= MIN_SIDE and w.bounds[3] >= MIN_SIDE]
    if like is not None:
        match = next((w for w in mine if all(abs(a - b) <= 2 for a, b in zip(w.bounds, like))), None)
        if match is not None:
            return match
    return mine[0] if mine else None


def window(window_id: int) -> Window | None:
    return next((_window(d) for d in _info_windows() if int(d["kCGWindowNumber"]) == window_id), None)


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
        raise RuntimeError(str(box["e"].localizedDescription()))
    return box["v"]


def capture(w: Window, max_edge: int = MAX_EDGE) -> tuple[bytes, int, int]:
    """JPEG bytes of exactly this window, and the image size (px)."""
    import ScreenCaptureKit as SCK
    content = _wait(lambda h: SCK.SCShareableContent.getShareableContentExcludingDesktopWindows_onScreenWindowsOnly_completionHandler_(
        True, True, h))
    sc = next((x for x in content.windows() if int(x.windowID()) == w.id), None)
    if sc is None:
        raise LookupError("that window is no longer on screen")
    flt = SCK.SCContentFilter.alloc().initWithDesktopIndependentWindow_(sc)
    cfg = SCK.SCStreamConfiguration.alloc().init()
    scale = float(flt.pointPixelScale() or 1.0)
    fw, fh = w.bounds[2] * scale, w.bounds[3] * scale
    k = min(1.0, max_edge / max(fw, fh))
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
