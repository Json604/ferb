"""fs.look: a picture or one PDF page on disk, as an image the AI sees (HAND-517, owner 2026-11-12). Must not open
anything on screen, write a file or cap a file's size.

The file is opened once and checked as the file tools check it (the protection lists on the path it names, then on what
it really opened, F_GETPATH); its bytes are decoded in memory: a picture with ImageIO (PNG, JPEG, HEIC, TIFF, the first
frame of a GIF or WebP, anything ImageIO reads), a PDF page with PDFKit (the header `%PDF-` in its first KiB, a structural
fact, never its name). The picture is scaled as a screenshot is (mac/screenshot.py MAX_EDGE on the long side, a page
rendered to it) onto white (a transparent picture stays readable) and encoded with the screenshot's JPEG helper. The
gateway then masks it as every tool image (OCR masking; a masking failure withholds the pixels), keeps it in the
evidence store and hands it to the model. A file it can't decode is a plain error; nothing else is refused."""

import os
from typing import Any

from pydantic import BaseModel, Field

from ferb.act.fs.files import _Base, _err, _source, identity, opened_closed, resolve
from ferb.core.types import ActionSpec, RiskLevel, Target, ToolContext, ToolResult

PDF_MAGIC, PDF_HEAD = b"%PDF-", 1024     # the PDF header may follow some bytes; readers look in the first KiB


class Undecodable(Exception):
    """The file's bytes are no picture or PDF FERB can decode."""


def _bitmap(width: int, height: int) -> Any:
    """An opaque white RGB bitmap of this size, to draw a picture or a page on."""
    import Quartz
    space = Quartz.CGColorSpaceCreateWithName(Quartz.kCGColorSpaceSRGB)
    ctx = Quartz.CGBitmapContextCreate(None, width, height, 8, 0, space, Quartz.kCGImageAlphaNoneSkipLast)
    if ctx is None:
        raise Undecodable("couldn't make a picture of that size")
    Quartz.CGContextSetRGBFillColor(ctx, 1.0, 1.0, 1.0, 1.0)
    Quartz.CGContextFillRect(ctx, Quartz.CGRectMake(0, 0, width, height))
    Quartz.CGContextSetInterpolationQuality(ctx, Quartz.kCGInterpolationHigh)
    return ctx


def picture(data: bytes, max_edge: int) -> tuple[bytes, dict[str, Any]]:
    """A picture's first image, turned as its orientation says, scaled down to `max_edge` on its long side (never up),
    as JPEG, with its format and size as the file holds it."""
    import Quartz
    from Foundation import NSData
    source = Quartz.CGImageSourceCreateWithData(NSData.dataWithBytes_length_(data, len(data)), None)
    kind = Quartz.CGImageSourceGetType(source) if source is not None else None
    if source is None or kind is None or Quartz.CGImageSourceGetCount(source) < 1:
        raise Undecodable("not a picture")
    props = Quartz.CGImageSourceCopyPropertiesAtIndex(source, 0, None) or {}
    width, height = int(props.get("PixelWidth") or 0), int(props.get("PixelHeight") or 0)
    if width < 1 or height < 1:
        raise Undecodable("not a picture")
    image = Quartz.CGImageSourceCreateThumbnailAtIndex(source, 0, {
        Quartz.kCGImageSourceCreateThumbnailFromImageAlways: True,
        Quartz.kCGImageSourceCreateThumbnailWithTransform: True,          # EXIF orientation: as a viewer shows it
        Quartz.kCGImageSourceThumbnailMaxPixelSize: min(max_edge, max(width, height))})
    if image is None:
        raise Undecodable("not a picture")
    w, h = int(Quartz.CGImageGetWidth(image)), int(Quartz.CGImageGetHeight(image))
    ctx = _bitmap(w, h)
    Quartz.CGContextDrawImage(ctx, Quartz.CGRectMake(0, 0, w, h), image)
    from ferb.act.mac.screenshot import _jpeg
    frames = int(Quartz.CGImageSourceGetCount(source))
    facts: dict[str, Any] = {"format": str(kind), "pixels": [width, height], "width": w, "height": h}
    if frames > 1:
        facts["frames"], facts["shown"] = frames, "the first frame"
    return _jpeg(Quartz.CGBitmapContextCreateImage(ctx)), facts


def pdf_page(data: bytes, number: int, max_edge: int) -> tuple[bytes, dict[str, Any]]:
    """One page of a PDF (1-based), turned as the page says, rendered with its long side `max_edge`, as JPEG, with how
    many pages the PDF has and the page's text layer (empty for a scan)."""
    import objc
    import Quartz
    from Foundation import NSData
    with objc.autorelease_pool():
        doc = Quartz.PDFDocument.alloc().initWithData_(NSData.dataWithBytes_length_(data, len(data)))
        if doc is None:
            raise Undecodable("not a PDF FERB can read")
        if doc.isLocked():
            raise Undecodable("the PDF is locked with a password")
        pages = int(doc.pageCount())
        if not 1 <= number <= pages:
            raise IndexError(pages)
        page = doc.pageAtIndex_(number - 1)
        ref = page.pageRef() if page is not None else None
        if ref is None:
            raise Undecodable("not a PDF FERB can read")
        box = Quartz.CGPDFPageGetBoxRect(ref, Quartz.kCGPDFCropBox)
        pw, ph = float(box.size.width), float(box.size.height)
        if Quartz.CGPDFPageGetRotationAngle(ref) % 180:
            pw, ph = ph, pw
        if pw <= 0 or ph <= 0:
            raise Undecodable("the page has no size")
        k = max_edge / max(pw, ph)
        w, h = max(1, round(pw * k)), max(1, round(ph * k))
        ctx = _bitmap(w, h)
        Quartz.CGContextScaleCTM(ctx, k, k)
        Quartz.CGContextConcatCTM(ctx, Quartz.CGPDFPageGetDrawingTransform(
            ref, Quartz.kCGPDFCropBox, Quartz.CGRectMake(0, 0, pw, ph), 0, True))
        Quartz.CGContextDrawPDFPage(ctx, ref)
        from ferb.act.mac.screenshot import _jpeg
        jpeg = _jpeg(Quartz.CGBitmapContextCreateImage(ctx))
        text = str(page.string() or "").strip()
    return jpeg, {"format": "com.adobe.pdf", "page": number, "pages": pages, "width": w, "height": h, "text": text}


class FsLook(_Base):
    name = "fs.look"
    description = ("See a picture or one page of a PDF on disk, as an image: a photo, screenshot or scan (PNG, JPEG, "
                   "HEIC, TIFF, GIF, WebP) or how a PDF page looks; a PDF page also comes with its text, when it has "
                   "any. A file the owner attached in chat is a path you can look at with this. It opens nothing on "
                   "screen.")
    scopes, risk, reversible = {"fs:read"}, RiskLevel.LOW, True
    observation = True                                 # only reads: the final checker may use it (V3, W4)
    reads_content = True                               # reads what the file shows: a checker's look at it (L39 M8)
    probe_safe = True  # reads state, moves nothing, returns at once: a loop's probe may use it (L31)
    undone = "nothing was read"

    class Input(BaseModel):
        path: str = Field(description="The picture or PDF (absolute, ~, or relative to FERB's workspace)")
        page: int = Field(1, ge=1, description="A PDF's page, from 1 (the result says how many pages it has)")

    def describe(self, args: BaseModel) -> ActionSpec:
        return ActionSpec("read", Target(path=self.describe_path(args)))

    def _run(self, args: BaseModel, ctx: ToolContext) -> ToolResult:
        from ferb.act.mac.screenshot import MAX_EDGE
        p = resolve(str(getattr(args, "path")), ctx, must_exist=True)
        if not p.is_file():
            return _err(f"{p} is not a file")
        fd = os.open(p, os.O_RDONLY | os.O_CLOEXEC)
        with os.fdopen(fd, "rb") as stream:
            refusal = opened_closed(ctx, stream.fileno(), self.access)
            if refusal:
                return _err(refusal)
            data = stream.read()
        page = int(getattr(args, "page"))
        try:
            if PDF_MAGIC in data[:PDF_HEAD]:
                jpeg, facts = pdf_page(data, page, MAX_EDGE)
            elif page != 1:
                return _err(f"{p.name} is a picture, not a PDF: it has no page {page}")
            else:
                jpeg, facts = picture(data, MAX_EDGE)
        except IndexError as e:
            pages = e.args[0]
            return _err(f"{p.name} has {pages} page{'s' if pages != 1 else ''}; there is no page {page}")
        except Undecodable as e:
            return _err(f"FERB couldn't open {p.name} as a picture or a PDF ({e}); nothing was shown")
        return ToolResult({**identity(p), **facts}, image=jpeg, read_sources=(_source(p, ctx),))
