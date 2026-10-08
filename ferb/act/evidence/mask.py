"""Owns masking of accessibility-known secret fields on a stored screenshot (V1, rule 14). Must not decide what is
secret: the boxes come from the accessibility tree (a password field's frame); full screenshot redaction (OTP digits,
notification-matched codes, CSS-masked fields) is F5.

Decoded, filled and re-encoded in memory with Quartz, the image library the screenshot path already uses
(ferb/act/mac/screenshot.py; FERB has no Pillow). The masked area is filled opaque black before encoding, so nothing
under it survives in the stored JPEG; JPEG's own ringing can leave a level or two of grey at the edges."""

import math
from typing import Any

Box = tuple[int, int, int, int]                       # x, y, w, h in image pixels, top-left origin


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
