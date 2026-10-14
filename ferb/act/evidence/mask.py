"""Owns masking of known secret fields on a screenshot (V1, rule 14, L13). Must not decide what is secret: the boxes
come from the accessibility tree (a password field's frame) or the browser extension's secret-field rects; OCR
redaction of what a picture shows (OTP digits, card numbers) is F5's (ferb/sanitize/images.py).

Decoded, filled and re-encoded in memory with Quartz, the image library the screenshot path already uses
(ferb/act/mac/screenshot.py; FERB has no Pillow). The masked area is filled opaque black before encoding, so nothing
under it survives in the stored JPEG; JPEG's own ringing can leave a level or two of grey at the edges."""

import math
from dataclasses import dataclass
from typing import Any

from ferb.core.errors import Withheld

Box = tuple[int, int, int, int]                       # x, y, w, h in image pixels, top-left origin
Rect = tuple[float, float, float, float]              # x, y, w, h in points or CSS px


@dataclass(frozen=True)
class PageItem:
    """One thing a page's mask state names (the extension's maskState): a secret field of the top document
    (`field|…`) or a frame element in it (`frame|…`), by a key that stays the same while it lives. A frame element is
    painted whole unless its linked frame (`child`, the document it holds) and every frame inside that show no secret
    field (`paint` False); a child frame's fields are never placed one by one."""
    key: str
    rect: Rect
    paint: bool
    child: str | None = None


@dataclass(frozen=True)
class PageState:
    """A browser page's mask state at one moment: its top document, the visual viewport (what a capture shows: x, y
    in layout CSS px, width, height) and its items, in top-viewport CSS px."""
    top: str
    viewport: Rect
    items: tuple[PageItem, ...]

    @classmethod
    def parse(cls, raw: Any) -> "PageState | None":
        """The extension's report, or None when anything in it is malformed (never a page without secrets)."""
        if not isinstance(raw, dict) or not isinstance(raw.get("top"), str) or not isinstance(raw.get("items"), list):
            return None
        viewport = _rect(raw.get("viewport"))
        if viewport is None or viewport[2] <= 0 or viewport[3] <= 0:
            return None
        items = []
        for i in raw["items"]:
            rect = _rect(i.get("rect")) if isinstance(i, dict) else None
            if rect is None or not isinstance(i.get("key"), str) or not isinstance(i.get("paint"), bool) \
                    or not isinstance(i.get("child"), (str, type(None))):
                return None
            items.append(PageItem(i["key"], rect, i["paint"], i.get("child")))
        return cls(raw["top"], viewport, tuple(items))


def _rect(v: Any) -> Rect | None:
    if not isinstance(v, list) or len(v) != 4 or not all(
            isinstance(x, (int, float)) and not isinstance(x, bool) and math.isfinite(x) for x in v):
        return None
    return float(v[0]), float(v[1]), float(v[2]), float(v[3])


def page_masks(before: PageState, after: PageState) -> tuple[list[Rect], Rect]:
    """What one capture of a page must paint over (L13), from the looks before and after it, and the viewport the
    rects are in. Something at the same place in both looks is painted (a frame element painted in either look, or
    whose document changed, a reload, is painted whole). The capture is withheld when a secret field's place in it is
    unknown: something moved, or appeared or went outside what is painted in both looks, or the page navigated or
    resized."""
    if before.top != after.top or before.viewport != after.viewport:
        raise Withheld("pixels withheld: the page changed during capture")
    b, a = {i.key: i for i in before.items}, {i.key: i for i in after.items}
    still: list[Rect] = []
    loose: list[Rect] = []
    for key in sorted(b.keys() | a.keys()):
        x, y = b.get(key), a.get(key)
        if x is not None and y is not None:
            if x.rect != y.rect:
                raise Withheld("pixels withheld: a secret field or frame moved during capture")
            if x.paint or y.paint or x.child != y.child:
                still.append(x.rect)
        else:
            only = x if x is not None else y
            if only is not None and only.paint:
                loose.append(only.rect)
    for r in loose:
        if not any(_holds(s, r) for s in still):
            raise Withheld("pixels withheld: a secret field appeared or went during capture")
    return still + loose, before.viewport


def _holds(a: Rect, b: Rect) -> bool:
    return a[0] <= b[0] and a[1] <= b[1] and a[0] + a[2] >= b[0] + b[2] and a[1] + a[3] >= b[1] + b[3]


def mask_secret_fields(image: bytes, rects: list[Box]) -> bytes:
    """The JPEG with each box filled opaque black, clamped to the image. ValueError when it can't be decoded."""
    img = _decode(image)
    import Quartz  # type: ignore[import-untyped]
    iw, ih = Quartz.CGImageGetWidth(img), Quartz.CGImageGetHeight(img)
    ctx = _canvas(img)
    Quartz.CGContextSetRGBFillColor(ctx, 0.0, 0.0, 0.0, 1.0)
    for x, y, w, h in rects:
        left, top, right, bottom = max(0, x), max(0, y), min(iw, x + w), min(ih, y + h)
        if right > left and bottom > top:                    # CoreGraphics' origin is bottom-left
            Quartz.CGContextFillRect(ctx, Quartz.CGRectMake(left, ih - bottom, right - left, bottom - top))
    from ferb.act.mac.screenshot import _jpeg                  # the screenshot path's own encoder and quality
    return _jpeg(Quartz.CGBitmapContextCreateImage(ctx))


def to_image_boxes(rects: list[tuple[float, float, float, float]], bounds: tuple[float, float, float, float],
                   size: tuple[int, int]) -> list[Box]:
    """Screen-point boxes of a window's controls -> image-pixel boxes on its capture at `size`, rounded outward so a
    mask covers the whole field; a box outside the window or with no area is dropped. No cap: every field counts."""
    bx, by, bw, bh = bounds
    if bw <= 0 or bh <= 0:
        return []
    kx, ky = size[0] / bw, size[1] / bh
    out: list[Box] = []
    for x, y, w, h in rects:
        left, top, right, bottom = max(x, bx), max(y, by), min(x + w, bx + bw), min(y + h, by + bh)
        if w <= 0 or h <= 0 or right <= left or bottom <= top:
            continue
        px, py = math.floor((left - bx) * kx), math.floor((top - by) * ky)
        out.append((px, py, math.ceil((right - bx) * kx) - px, math.ceil((bottom - by) * ky) - py))
    return out


def size(image: bytes) -> tuple[int, int]:
    """The image's width and height in pixels. ValueError when it can't be decoded."""
    import Quartz
    img = _decode(image)
    return int(Quartz.CGImageGetWidth(img)), int(Quartz.CGImageGetHeight(img))


def pixel(image: bytes, x: int, y: int) -> tuple[int, int, int]:
    """One pixel's RGB (top-left origin). For tests and inspection; nothing in FERB decides from pixels."""
    import Quartz
    out = Quartz.CGBitmapContextCreateImage(_canvas(_decode(image)))
    data = bytes(Quartz.CGDataProviderCopyData(Quartz.CGImageGetDataProvider(out)))
    i = y * Quartz.CGImageGetBytesPerRow(out) + x * 4
    return data[i], data[i + 1], data[i + 2]


def _decode(image: bytes) -> Any:
    import Quartz
    src = Quartz.CGImageSourceCreateWithData(image, None)
    img = Quartz.CGImageSourceCreateImageAtIndex(src, 0, None) if src is not None else None
    if img is None:
        raise ValueError("not an image Quartz can decode")
    return img


def _canvas(img: Any) -> Any:
    """An RGB bitmap (no alpha) holding the image, to draw on."""
    import Quartz
    iw, ih = Quartz.CGImageGetWidth(img), Quartz.CGImageGetHeight(img)
    ctx = Quartz.CGBitmapContextCreate(None, iw, ih, 8, 0, Quartz.CGColorSpaceCreateDeviceRGB(),
                                       Quartz.kCGImageAlphaNoneSkipLast)
    Quartz.CGContextDrawImage(ctx, Quartz.CGRectMake(0, 0, iw, ih), img)
    return ctx
