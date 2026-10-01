"""On-demand capture of one app window (section 3 act/mac, build step 9). Blocking pyobjc calls — callers run them
in a thread.

- Only the target window: ScreenCaptureKit's desktop-independent window filter renders that window's own content,
  never the rest of the screen and never windows on top of it.
- In memory only: the frame is encoded to JPEG bytes and handed back; nothing here writes a file.
- Marks (owner request, 2026-10-01): a snapshot's image can carry each control's number drawn on its box, so the model
  sees the same thing it acts on ("12" on the image = element s<gen>e12). Drawn on the in-memory image before encoding.
- `window_at` answers "which normal window is topmost at this point": the fallback for checking that a click lands on
  the window the model saw when Accessibility's hit test (accessibility.hit) can't answer.
"""

import threading
from dataclasses import dataclass
from typing import Any

MAX_EDGE = 1568               # long side of the image the model gets (px); larger windows are scaled down
JPEG_QUALITY = 0.8
TIMEOUT_S = 10.0
MAX_MARKS = 200

Mark = tuple[str, tuple[float, float, float, float]]          # (label, x, y, w, h)


def to_pixels(marks: list[Mark], bounds: tuple[float, float, float, float], size: tuple[int, int]) -> list[Mark]:
    """Screen-point boxes -> image-pixel boxes for a window captured at `size`; boxes outside the window or with no
    area are dropped (a control scrolled out of view has no place on the image)."""
    bx, by, bw, bh = bounds
    kx, ky = size[0] / bw, size[1] / bh
    out: list[Mark] = []
    for label, (x, y, w, h) in marks:
        if w <= 0 or h <= 0 or x + w <= bx or y + h <= by or x >= bx + bw or y >= by + bh:
            continue
        left, top = max(x, bx), max(y, by)
        right, bottom = min(x + w, bx + bw), min(y + h, by + bh)
        out.append((label, ((left - bx) * kx, (top - by) * ky, (right - left) * kx, (bottom - top) * ky)))
    return out[:MAX_MARKS]


@dataclass(frozen=True)
class Window:
    id: int                                        # CGWindowID (== SCWindow.windowID)
    pid: int
    bounds: tuple[float, float, float, float]      # x, y, w, h in screen points (top-left origin)
    title: str


def _info_windows(everywhere: bool = False) -> list[dict[str, Any]]:
    """On-screen windows front to back; `everywhere` also lists windows on other Spaces and full-screen apps."""
    import Quartz
    opts = (Quartz.kCGWindowListOptionAll if everywhere else Quartz.kCGWindowListOptionOnScreenOnly) \
        | Quartz.kCGWindowListExcludeDesktopElements
    return list(Quartz.CGWindowListCopyWindowInfo(opts, Quartz.kCGNullWindowID) or [])


def _window(d: dict[str, Any]) -> Window:
    b = d.get("kCGWindowBounds") or {}
    return Window(int(d["kCGWindowNumber"]), int(d.get("kCGWindowOwnerPID", -1)),
                  (float(b.get("X", 0)), float(b.get("Y", 0)), float(b.get("Width", 0)), float(b.get("Height", 0))),
                  str(d.get("kCGWindowName") or ""))


MIN_SIDE = 50                 # smaller layer-0 windows are helpers / strips, not something to look at


def front_window(pid: int, like: tuple[float, float, float, float] | None = None) -> Window | None:
    """The app's window to capture: the on-screen normal (layer 0) window matching `like` (its Accessibility window's
    frame) if there is one, else its frontmost one of a real size."""
    for everywhere in (False, True):                  # this Space first; then other Spaces / full screen (14a)
        mine = [_window(d) for d in _info_windows(everywhere)
                if int(d.get("kCGWindowOwnerPID", -1)) == pid and int(d.get("kCGWindowLayer", 1)) == 0
                and float(d.get("kCGWindowAlpha", 1)) > 0]
        mine = [w for w in mine if w.bounds[2] >= MIN_SIDE and w.bounds[3] >= MIN_SIDE]
        if everywhere:
            mine = [w for w in mine if w.title] or mine   # real windows have titles; helpers often don't
        if like is not None:
            match = next((w for w in mine if all(abs(a - b) <= 2 for a, b in zip(w.bounds, like))), None)
            if match is not None:
                return match
        if mine:
            return mine[0]
    return None


def on_screen(window_id: int) -> bool:
    return any(int(d["kCGWindowNumber"]) == window_id for d in _info_windows())


def window(window_id: int) -> Window | None:
    return next((_window(d) for d in _info_windows(True) if int(d["kCGWindowNumber"]) == window_id), None)


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


def capture(w: Window, max_edge: int = MAX_EDGE, marks: list[Mark] | None = None) -> tuple[bytes, int, int]:
    """JPEG bytes of exactly this window, and the image size (px). `marks` (screen points) are drawn on it."""
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
    k = min(1.0, max_edge / max(fw, fh))
    width, height = max(1, int(fw * k)), max(1, int(fh * k))
    cfg.setWidth_(width)
    cfg.setHeight_(height)
    cfg.setShowsCursor_(False)
    image = _wait(lambda h: SCK.SCScreenshotManager.captureImageWithFilter_configuration_completionHandler_(flt, cfg, h))
    if marks:
        image = _draw_marks(image, to_pixels(marks, w.bounds, (width, height)))
    return _jpeg(image), width, height


def _draw_marks(image: Any, marks: list[Mark]) -> Any:
    """Each control's box outlined and its number on a small tag at the box's top-left (CoreGraphics, in memory)."""
    import CoreText  # type: ignore[import-untyped]
    import Quartz  # type: ignore[import-untyped]
    from AppKit import NSColor, NSFont  # type: ignore[import-untyped]
    from Foundation import NSAttributedString  # type: ignore[import-untyped]
    iw, ih = Quartz.CGImageGetWidth(image), Quartz.CGImageGetHeight(image)
    ctx = Quartz.CGBitmapContextCreate(None, iw, ih, 8, 0, Quartz.CGColorSpaceCreateDeviceRGB(),
                                       Quartz.kCGImageAlphaPremultipliedLast)
    Quartz.CGContextDrawImage(ctx, Quartz.CGRectMake(0, 0, iw, ih), image)
    font = NSFont.boldSystemFontOfSize_(max(11.0, ih / 70))
    for label, (x, y, w, h) in marks:
        cy = ih - y - h                                         # CoreGraphics' origin is bottom-left
        Quartz.CGContextSetRGBStrokeColor(ctx, 0.95, 0.15, 0.35, 0.9)
        Quartz.CGContextSetLineWidth(ctx, 2.0)
        Quartz.CGContextStrokeRect(ctx, Quartz.CGRectMake(x, cy, w, h))
        text = NSAttributedString.alloc().initWithString_attributes_(
            label, {"NSFont": font, "NSColor": NSColor.whiteColor()})
        line = CoreText.CTLineCreateWithAttributedString(text)
        bounds = CoreText.CTLineGetTypographicBounds(line, None, None, None)
        tw = float(bounds[0] if isinstance(bounds, tuple) else bounds)   # pyobjc: (width, ascent, descent, leading)
        th = font.pointSize() + 4
        tag_y = min(ih - th, cy + h - th)                         # the tag sits at the box's top-left corner
        Quartz.CGContextSetRGBFillColor(ctx, 0.95, 0.15, 0.35, 0.95)
        Quartz.CGContextFillRect(ctx, Quartz.CGRectMake(x, tag_y, tw + 6, th))
        Quartz.CGContextSetTextPosition(ctx, x + 3, tag_y + 3)
        CoreText.CTLineDraw(line, ctx)
    return Quartz.CGBitmapContextCreateImage(ctx)


def _jpeg(image: Any) -> bytes:
    import Quartz
    from Foundation import NSMutableData
    data = NSMutableData.data()
    dest = Quartz.CGImageDestinationCreateWithData(data, "public.jpeg", 1, None)
    Quartz.CGImageDestinationAddImage(dest, image, {Quartz.kCGImageDestinationLossyCompressionQuality: JPEG_QUALITY})
    if not Quartz.CGImageDestinationFinalize(dest):
        raise RuntimeError("couldn't encode the screenshot")
    return bytes(data)
