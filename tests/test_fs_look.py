"""fs.look (HAND-517): a picture or a PDF page on disk reaches the AI as an image through the gateway's one image path:
masked like every tool image, checked against the protection lists like fs.read, nothing written outside the evidence
store."""

import base64
import os
from pathlib import Path

import pytest

from ferb.policy.protection import Protection
from ferb.sanitize import images
from tests.conftest import Answers, RunningDaemon, gateway_setup, gateway_tool_call

RECOGNIZE = images.recognize_text           # the real on-device recognition (the daemon fixture stubs it out)
CARD = "4111 1111 1111 1111"


def _drawn(path: Path, lines: list[str], kind: str = "public.png") -> None:
    """A white picture with each line drawn in large black text, written as `kind`."""
    import AppKit
    import Quartz
    w, h = 1400, 140 * len(lines) + 60
    rep = AppKit.NSBitmapImageRep.alloc().initWithBitmapDataPlanes_pixelsWide_pixelsHigh_bitsPerSample_samplesPerPixel_hasAlpha_isPlanar_colorSpaceName_bytesPerRow_bitsPerPixel_(
        None, w, h, 8, 4, True, False, AppKit.NSDeviceRGBColorSpace, 0, 0)
    AppKit.NSGraphicsContext.saveGraphicsState()
    AppKit.NSGraphicsContext.setCurrentContext_(AppKit.NSGraphicsContext.graphicsContextWithBitmapImageRep_(rep))
    AppKit.NSColor.whiteColor().set()
    AppKit.NSRectFill(((0, 0), (w, h)))
    for i, text in enumerate(reversed(lines)):
        AppKit.NSString.stringWithString_(text).drawAtPoint_withAttributes_(
            (30, 40 + 140 * i), {AppKit.NSFontAttributeName: AppKit.NSFont.systemFontOfSize_(72),
                                 AppKit.NSForegroundColorAttributeName: AppKit.NSColor.blackColor()})
    AppKit.NSGraphicsContext.restoreGraphicsState()
    dest = Quartz.CGImageDestinationCreateWithURL(AppKit.NSURL.fileURLWithPath_(str(path)), kind, 1, None)
    Quartz.CGImageDestinationAddImage(dest, rep.CGImage(), None)
    assert Quartz.CGImageDestinationFinalize(dest)


def _pdf(path: Path, pages: list[str]) -> None:
    """A PDF with one line of real text (a text layer) on each page."""
    import AppKit
    import Quartz
    ctx = Quartz.CGPDFContextCreateWithURL(AppKit.NSURL.fileURLWithPath_(str(path)),
                                          Quartz.CGRectMake(0, 0, 612, 792), None)
    for text in pages:
        Quartz.CGContextBeginPage(ctx, None)
        AppKit.NSGraphicsContext.saveGraphicsState()
        AppKit.NSGraphicsContext.setCurrentContext_(
            AppKit.NSGraphicsContext.graphicsContextWithCGContext_flipped_(ctx, False))
        AppKit.NSString.stringWithString_(text).drawAtPoint_withAttributes_(
            (60, 600), {AppKit.NSFontAttributeName: AppKit.NSFont.systemFontOfSize_(36)})
        AppKit.NSGraphicsContext.restoreGraphicsState()
        Quartz.CGContextEndPage(ctx)
    Quartz.CGPDFContextClose(ctx)


def _files(root: Path) -> set[str]:
    return {str(p) for p in root.rglob("*") if p.is_file()}


# decision: HAND-517
def test_a_picture_with_a_card_number_reaches_the_ai_masked_and_only_the_evidence_store_keeps_it(
        daemon: RunningDaemon, monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr("ferb.sanitize.images.recognize_text", RECOGNIZE)
    ws = daemon.daemon.home / "workspace"
    photo = ws / "receipt.png"
    _drawn(photo, ["Paid with card", CARD])
    assert any("4111" in r.text for r in RECOGNIZE(photo.read_bytes()))      # the card is readable in the file
    grant, _ = gateway_setup(daemon, {"fs:read"}, None, [])
    before = _files(daemon.daemon.home)
    r = gateway_tool_call(daemon, grant, "fs.look", path=str(photo))
    assert not r["is_error"] and r["content"]["format"] == "public.png" and r["content"]["width"] == 1400
    seen = base64.b64decode(r["image"]["data"])
    assert not any("4111" in t.text or "1111 1111" in t.text for t in RECOGNIZE(seen))   # masked before the AI
    assert any("Paid" in t.text for t in RECOGNIZE(seen))                    # the rest of the picture is there
    new = _files(daemon.daemon.home) - before
    evidence = str(daemon.daemon.home / "evidence")
    assert new and all(p.startswith(evidence) or p.endswith((".db-wal", ".db-shm", ".db", ".jsonl", ".log"))
                       for p in new), new


# decision: HAND-517, GATE-200
def test_a_look_into_a_protected_folder_asks_and_ferbs_own_files_are_refused(daemon: RunningDaemon,
                                                                            tmp_path: Path) -> None:
    fake = tmp_path / "fakehome"
    (fake / "Private").mkdir(parents=True)
    _drawn(fake / "Private" / "scan.png", ["passport"])
    daemon.daemon.gateway.roots.protection = Protection(fake, daemon.daemon.home, ["~/Private"], [], [])
    grant, _ = gateway_setup(daemon, {"fs:read"}, None, [])
    no = Answers(daemon, choice=2)
    try:
        r = gateway_tool_call(daemon, grant, "fs.look", path=str(fake / "Private" / "scan.png"))
    finally:
        no.stop()
    assert r["is_error"] and "image" not in r and len(no.asked) == 1
    assert no.asked[0].summary == "Let FERB read ~/Private?"
    home = daemon.daemon.home
    (home / "run").mkdir(exist_ok=True)
    _drawn(home / "run" / "shot.png", ["token"])
    r = gateway_tool_call(daemon, grant, "fs.look", path=str(home / "run" / "shot.png"))
    assert r["is_error"] and "FERB's own" in r["content"]["error"] and "image" not in r


# decision: HAND-517
def test_a_pdf_page_is_rendered_with_the_page_count_and_its_text(daemon: RunningDaemon) -> None:
    ws = daemon.daemon.home / "workspace"
    _pdf(ws / "deck.pdf", ["Cover page", "Second page plan", "Thanks"])
    grant, _ = gateway_setup(daemon, {"fs:read"}, None, [])
    r = gateway_tool_call(daemon, grant, "fs.look", path=str(ws / "deck.pdf"), page=2)
    assert not r["is_error"] and r["content"]["pages"] == 3 and r["content"]["page"] == 2
    assert "Second page plan" in r["content"]["text"] and r["image"]["mime"] == "image/jpeg"
    assert max(r["content"]["width"], r["content"]["height"]) == 1568          # the screenshot size (MAX_EDGE)
    r = gateway_tool_call(daemon, grant, "fs.look", path=str(ws / "deck.pdf"), page=4)
    assert r["is_error"] and r["content"]["error"] == "deck.pdf has 3 pages; there is no page 4" and "image" not in r


# decision: HAND-517
def test_a_text_file_and_a_broken_picture_are_plain_errors(daemon: RunningDaemon) -> None:
    ws = daemon.daemon.home / "workspace"
    (ws / "notes.txt").write_text("hello\n")
    (ws / "broken.jpg").write_bytes(b"\xff\xd8\xff\xe0" + os.urandom(64))
    grant, _ = gateway_setup(daemon, {"fs:read"}, None, [])
    for name in ("notes.txt", "broken.jpg"):
        r = gateway_tool_call(daemon, grant, "fs.look", path=str(ws / name))
        assert r["is_error"] and "image" not in r
        assert r["content"]["error"].startswith(f"FERB couldn't open {name} as a picture or a PDF")
