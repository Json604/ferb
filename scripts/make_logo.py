"""Regenerate every FERB logo file from the one master, assets/logo/ferb-logo.svg (owner, 2026-11-01: the extension's
logo is FERB's logo everywhere). The master draws its "f" as a path, so nothing depends on installed fonts. Rendering
uses macOS's own SVG support (AppKit, through the checkout's PyObjC) and iconutil; nothing else to install.

    uv run python scripts/make_logo.py

Writes, all from the master:
  assets/logo/ferb-logo-1024.png              the master at 1024 px (transparent margin, as the SVG draws it)
  assets/logo/AppIcon.icns                    FERB.app, its daemon helper, FERB Voice and the DMG's volume icon, on
                                              Apple's macOS icon grid (824 of 1024 px, centered, with a soft shadow)
  extensions/store/src/icon.svg               the master itself; icon-small.svg: the same, cropped to the square
  ui/mac/FERBNotch/Sources/FERBNotch/Resources/ferb-mark.svg   the master itself (the notch's handle)
  extensions/{chromium/icons,store/src}/icon{16,32,48,128}.png   16-48 from the cropped square, 128 with Chrome's margin
  site/logo.svg, site/favicon.ico, site/apple-touch-icon.png      the cropped square; the touch icon is full-bleed
"""

import re
import struct
import subprocess
import sys
import tempfile
from pathlib import Path

import AppKit
import Foundation

ROOT = Path(__file__).resolve().parent.parent
MASTER = ROOT / "assets" / "logo" / "ferb-logo.svg"
FULL_BOX = 'viewBox="0 0 128 128"'
SQUARE_BOX = 'viewBox="16 16 96 96"'          # the master's rounded square, without the margin
# Apple's macOS icon grid: the shape is 824 of 1024 px, centered, with a soft shadow below it
GRID_SHAPE, GRID_SHADOW_Y, GRID_SHADOW_BLUR, GRID_SHADOW_ALPHA = 824 / 1024, 10 / 1024, 20 / 1024, 0.35
ICONSET = [(16, 1), (16, 2), (32, 1), (32, 2), (128, 1), (128, 2), (256, 1), (256, 2), (512, 1), (512, 2)]


def _color(hex_rgb: str) -> AppKit.NSColor:
    r, g, b = (int(hex_rgb[i:i + 2], 16) / 255 for i in (1, 3, 5))
    return AppKit.NSColor.colorWithSRGBRed_green_blue_alpha_(r, g, b, 1.0)


def render(svg: str, size: int, *, inset: float = 0.0, shadow: bool = False, backdrop: list[str] | None = None) -> bytes:
    """svg drawn into a size x size PNG, `inset` (a fraction of size) in from each side; `backdrop`: a top-to-bottom
    gradient filling the whole canvas first (for full-bleed icons)."""
    image = AppKit.NSImage.alloc().initWithData_(Foundation.NSData.dataWithBytes_length_(svg.encode(), len(svg.encode())))
    if image is None:
        raise SystemExit("macOS couldn't read the master SVG")
    rep = AppKit.NSBitmapImageRep.alloc().initWithBitmapDataPlanes_pixelsWide_pixelsHigh_bitsPerSample_samplesPerPixel_hasAlpha_isPlanar_colorSpaceName_bytesPerRow_bitsPerPixel_(  # noqa: E501
        None, size, size, 8, 4, True, False, AppKit.NSDeviceRGBColorSpace, 0, 0)
    rep.setSize_((size, size))
    context = AppKit.NSGraphicsContext.graphicsContextWithBitmapImageRep_(rep)
    AppKit.NSGraphicsContext.saveGraphicsState()
    AppKit.NSGraphicsContext.setCurrentContext_(context)
    context.setImageInterpolation_(AppKit.NSImageInterpolationHigh)
    if backdrop:
        gradient = AppKit.NSGradient.alloc().initWithStartingColor_endingColor_(_color(backdrop[0]), _color(backdrop[-1]))
        gradient.drawInRect_angle_(((0, 0), (size, size)), -90)
    if shadow:
        drop = AppKit.NSShadow.alloc().init()
        drop.setShadowOffset_((0, -GRID_SHADOW_Y * size))
        drop.setShadowBlurRadius_(GRID_SHADOW_BLUR * size)
        drop.setShadowColor_(AppKit.NSColor.colorWithSRGBRed_green_blue_alpha_(0, 0, 0, GRID_SHADOW_ALPHA))
        drop.set()
    side = size * (1 - 2 * inset)
    image.drawInRect_fromRect_operation_fraction_(((size * inset, size * inset), (side, side)), Foundation.NSZeroRect,
                                                  AppKit.NSCompositingOperationSourceOver, 1.0)
    context.flushGraphics()
    AppKit.NSGraphicsContext.restoreGraphicsState()
    return bytes(rep.representationUsingType_properties_(AppKit.NSBitmapImageFileTypePNG, {}))


def ico(pngs: list[tuple[int, bytes]]) -> bytes:
    """A .ico holding PNG images (every browser reads PNG entries)."""
    out = struct.pack("<HHH", 0, 1, len(pngs))
    offset = 6 + 16 * len(pngs)
    for size, data in pngs:
        out += struct.pack("<BBBBHHII", size % 256, size % 256, 0, 0, 1, 32, len(data), offset)
        offset += len(data)
    return out + b"".join(data for _, data in pngs)


def write(path: Path, data: bytes | str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    if isinstance(data, str):
        path.write_text(data)
    else:
        path.write_bytes(data)
    print(path.relative_to(ROOT))


def main() -> None:
    master = MASTER.read_text()
    if master.count(FULL_BOX) != 1 or "<text" in master:
        raise SystemExit(f"{MASTER} must keep {FULL_BOX} and draw with paths, not text")
    square = master.replace(FULL_BOX, SQUARE_BOX)
    stops = re.findall(r'stop-color="(#[0-9A-Fa-f]{6})"', master)

    write(ROOT / "assets/logo/ferb-logo-1024.png", render(master, 1024))
    with tempfile.TemporaryDirectory() as tmp:
        iconset = Path(tmp) / "AppIcon.iconset"
        iconset.mkdir()
        for points, scale in ICONSET:
            name = f"icon_{points}x{points}{'@2x' if scale == 2 else ''}.png"
            (iconset / name).write_bytes(render(square, points * scale, inset=(1 - GRID_SHAPE) / 2, shadow=True))
        icns = ROOT / "assets/logo/AppIcon.icns"
        subprocess.run(["iconutil", "-c", "icns", "-o", str(icns), str(iconset)], check=True)
        print(icns.relative_to(ROOT))

    write(ROOT / "extensions/store/src/icon.svg", master)
    write(ROOT / "extensions/store/src/icon-small.svg", square)
    write(ROOT / "ui/mac/FERBNotch/Sources/FERBNotch/Resources/ferb-mark.svg", master)
    for folder in ("extensions/chromium/icons", "extensions/store/src"):
        for size in (16, 32, 48):
            write(ROOT / folder / f"icon{size}.png", render(square, size))
        write(ROOT / folder / "icon128.png", render(master, 128))

    write(ROOT / "site/logo.svg", square)
    write(ROOT / "site/favicon.ico", ico([(size, render(square, size)) for size in (16, 32, 48)]))
    write(ROOT / "site/apple-touch-icon.png", render(square, 180, backdrop=stops))


if __name__ == "__main__":
    if sys.platform != "darwin":
        raise SystemExit("make_logo.py renders with macOS's AppKit")
    main()
