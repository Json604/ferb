"""Capture one exact window into memory and map image coordinates. Must not capture the desktop or choose a substitute window.

On-demand capture of one app window. Blocking pyobjc calls — callers run them
in a thread.

- Only the target window: ScreenCaptureKit's desktop-independent window filter renders that window's own content,
  never the rest of the screen and never windows on top of it.
- In memory only: the frame is encoded to JPEG bytes and handed back; nothing here writes a file.
- Marks (owner request, 2026-10-01): a snapshot's image can carry each control's number drawn on its box, so the model
  sees the same thing it acts on ("12" on the image = element s<gen>e12). Drawn on the in-memory image before encoding.
- `window_at` answers "which normal window is topmost at this point": the fallback for checking that a click lands on
  the window the model saw when Accessibility's hit test (accessibility.hit) can't answer."""

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


def front_window(pid: int, like: tuple[float, float, float, float] | None = None) -> Window | None:
    """The unique process-owned normal window matching exact bounds, or None. Never choose by order or size."""
    mine = [_window(d) for d in _info_windows(True)
            if int(d.get("kCGWindowOwnerPID", -1)) == pid and int(d.get("kCGWindowLayer", 1)) == 0
            and float(d.get("kCGWindowAlpha", 1)) > 0]
    if like is not None:
        mine = [w for w in mine if all(abs(a - b) <= 2 for a, b in zip(w.bounds, like))]
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


def capture(w: Window, max_edge: int | None = MAX_EDGE, marks: list[Mark] | None = None) -> tuple[bytes, int, int]:
    """JPEG bytes of exactly this window, and the image size (px). `marks` (screen points) are drawn on it. A minimized
    window is refused by name before ScreenCaptureKit is asked; FERB never unminimizes it (background first, I7)."""
    from ferb.act.mac import accessibility
    from ferb.core.errors import Refused
    if accessibility.window_minimized(w.pid, w.bounds):
        raise Refused("that window is minimized, and FERB doesn't bring windows back on its own (it works in the "
                      "background): it can be captured once it's shown again")
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
