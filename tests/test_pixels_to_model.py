"""L13 (owner, 2026-10-14: "no screenshots should be refused and go with redacted stuff"): browser windows, tab
screenshots and recording frames reach the model with every secret field painted over, and through the F5 sanitizer.
Where a native capture's secret fields can't be located or placed, the picture is left to the sanitizer's text
recognition and says so (live case 2026-10-19). An image is withheld only when painting or sanitizing itself fails. Offline: fake windows, a fake extension
bridge, the extension's own JavaScript in node (with Chromium's chrome.runtime: no getFrameId); no real screen or
browser is captured."""

import asyncio
import base64
import json
import shutil
import subprocess
from pathlib import Path
from types import SimpleNamespace
from typing import Any

import pytest

from ferb.act.evidence.mask import pixel
from ferb.act.mac.accessibility import AxTree
from ferb.core.errors import Withheld
from ferb.core.types import Identity, PageElement, PageSnapshot, TabRef
from tests.chromelab import EXT, _node, worker
from tests.conftest import RunningDaemon, gateway_request, gateway_setup, gateway_tool_call
from tests.mackit import BOUNDS, IMG, _fake_mac, gateway_call, grant_task, vision
from tests.support.images import ImageTool, black, solid_jpeg, white


def _ocr_log(monkeypatch: pytest.MonkeyPatch) -> list[bytes]:
    """Every image the F5 sanitizer's on-device text recognition looked at."""
    seen: list[bytes] = []
    monkeypatch.setattr("ferb.sanitize.images.recognize_text", lambda image: seen.append(image) or [])
    return seen


async def _evidence(d: RunningDaemon, task: str) -> list[tuple[str, bool]]:
    return [(r["kind"], r["has_image"]) for r in d.daemon.evidence.list(task)]


def _state(*fields: Any, top: str = "doc", viewport: Any = (0, 0, 100, 50), frames: Any = ()) -> dict[str, Any]:
    """A page's mask state as the extension reports it (background.js maskState): keyed items in viewport CSS px."""
    items = [{"key": f"field|{top}|{i}", "rect": list(r), "paint": True} for i, r in enumerate(fields)]
    return {"top": top, "viewport": list(viewport), "items": items + list(frames)}


def _same(state: dict[str, Any]) -> dict[str, Any]:
    return {"before": state, "after": state}


# ---------- the gateway: no more browser / recording / excluded-site withholding ----------

def test_a_brave_window_screenshot_keeps_its_pixels_and_goes_through_redaction(daemon: RunningDaemon,
                                                                               monkeypatch: pytest.MonkeyPatch) -> None:
    shots, _ = _fake_mac(daemon, monkeypatch)
    seen = _ocr_log(monkeypatch)
    daemon.daemon.mac.ax.tree = AxTree(secret_rects=[], has_web=True,              # type: ignore[union-attr]
                                       web_areas=[(100.0, 130.0, 800.0, 520.0)])   # AX read the page
    g = grant_task(daemon, "b", Identity.OWNER, {"screen"}, {"use_app"})
    r = gateway_call(daemon, g, "mac.screenshot", app="Brave")
    assert not r["is_error"] and "image_withheld" not in r["content"], r
    assert base64.b64decode(r["image"]["data"]) == shots.jpeg
    assert seen == [shots.jpeg]                                  # the sanitizer looked before the model did
    assert daemon.run(_evidence(daemon, "b")) == [("screenshot", True)]
    assert "Accessibility only" in r["content"]["masking"]       # no extension answered: said in the result


def test_recording_inspect_returns_its_sample_frames(daemon: RunningDaemon, monkeypatch: pytest.MonkeyPatch) -> None:
    seen = _ocr_log(monkeypatch)
    sheet_image = solid_jpeg(64, 36)
    sheet = ImageTool("recording.inspect", sheet_image,
                      {"path": "/tmp/r.mp4", "duration_s": 3.0, "sample_times_s": [0.0, 1.5, 3.0]},
                      reads=(("recording", "rec-1"),))
    grant, _ = gateway_setup(daemon, {"probe"}, gateway_request("look"), [sheet])
    r = gateway_tool_call(daemon, grant, "recording.inspect")
    assert not r["is_error"] and "image_withheld" not in r["content"], r
    assert base64.b64decode(r["image"]["data"]) == sheet_image and seen == [sheet_image]
    assert daemon.run(_evidence(daemon, "task-1")) == [("recording", True)]


def test_a_forced_sanitizer_failure_still_withholds_the_image(daemon: RunningDaemon,
                                                              monkeypatch: pytest.MonkeyPatch) -> None:
    _fake_mac(daemon, monkeypatch)

    def broken(image: bytes) -> list[Any]:
        raise RuntimeError("on-device text recognition failed")
    monkeypatch.setattr("ferb.sanitize.images.recognize_text", broken)
    g = grant_task(daemon, "f", Identity.OWNER, {"screen"}, {"use_app"})
    r = gateway_call(daemon, g, "mac.screenshot", app="Notes")
    assert not r["is_error"] and "image" not in r
    assert r["content"]["image_withheld"] == "image redaction unavailable"
    assert daemon.run(_evidence(daemon, "f")) == [("screenshot", False)]


def test_a_page_on_the_owners_excluded_sites_is_not_withheld(daemon: RunningDaemon,
                                                              monkeypatch: pytest.MonkeyPatch) -> None:
    """Owner, L13: "Yes, no exceptions". The exclusion list governs perception recording only."""
    from ferb.sanitize.redact import Redactor
    img = solid_jpeg(16, 12)
    shot = ImageTool("probe.screenshot", img, {"shot": "bs-2"}, reads=(("web", "https://bank.example/login"),),
                     site="bank.example")
    grant, _ = gateway_setup(daemon, {"probe"}, gateway_request("look"), [shot])
    assert isinstance(daemon.daemon.redact, Redactor)
    daemon.daemon.redact.configure({"bank.example"}, set())
    r = gateway_tool_call(daemon, grant, "probe.screenshot")
    assert not r["is_error"] and base64.b64decode(r["image"]["data"]) == img, r
    async def rows() -> list[dict[str, Any]]:
        return daemon.daemon.evidence.list("task-1")
    kept = daemon.run(rows())
    assert [(x["kind"], x["has_image"]) for x in kept] == [("screenshot", True)]
    assert kept[0]["origin"] == "external:web:excluded"          # the label still never names the host


# ---------- browser.screenshot: masked from the extension's own secret-field rects ----------

def _tab_driver(reply: dict[str, Any], parts: list[str] | None = None) -> tuple[Any, TabRef, list[str]]:
    from ferb.act.browser.ext_bridge import _Tab, _TabDriver
    ref = TabRef("device", "brave", "Default", 1, "t1")
    ops: list[str] = []

    async def call(op: str, args: dict[str, Any], **kw: Any) -> dict[str, Any]:
        ops.append(op)
        if op in ("screenshot", "record_frame") and "part" not in args:
            return reply
        return {"data": (parts or [])[args["part"]]}
    bridge = SimpleNamespace(gen=1, conn=object(), call=call, navs={})
    d = _TabDriver(None, lambda: [])                                       # type: ignore[arg-type]
    d._tabs["t1"] = _Tab(ref, bridge, 19, "task", "https://example.test/login", snap_gen=3)   # type: ignore[arg-type]
    return d, ref, ops


def test_a_truncated_snapshot_with_a_secret_field_yields_a_masked_screenshot_not_a_refusal() -> None:
    page = solid_jpeg(200, 100)                                  # a 100 x 50 CSS px viewport captured at 2x
    reply = {"data": base64.b64encode(page).decode(), "format": "jpeg", "mask": _same(_state((10, 10, 30, 10)))}
    d, ref, ops = _tab_driver(reply)
    secret = PageElement("s3e1", "0", "textbox", "Password", "filled", (10, 10, 30, 10), True, "password",
                         secret=True)

    async def snapshot(tab: TabRef) -> PageSnapshot:
        return PageSnapshot(tab, 3, "https://example.test/login", "Sign in", [secret], "", truncated=True)
    d.snapshot = snapshot
    shot, jpeg, width, height = asyncio.run(d.visual_snapshot(ref))
    assert ops == ["screenshot"] and shot.startswith("bs-") and (width, height) == (200, 100)
    assert black(pixel(jpeg, 40, 25))                            # (10..40, 10..20) CSS px -> (20..80, 20..40) px
    assert white(pixel(jpeg, 5, 5)) and white(pixel(jpeg, 150, 80))


def test_a_pinch_zoomed_page_is_masked_where_the_visual_viewport_shows_it() -> None:
    page = solid_jpeg(200, 100)                                  # the visual viewport: 50 x 25 CSS px at (40, 20)
    reply = {"data": base64.b64encode(page).decode(), "mask": _same(_state((50, 25, 10, 5), viewport=(40, 20, 50, 25)))}
    d, ref, _ = _tab_driver(reply)
    jpeg = asyncio.run(d.screenshot(ref))
    assert black(pixel(jpeg, 50, 25))                            # ((50-40)*4, (25-20)*4) .. +40, +20
    assert white(pixel(jpeg, 30, 15)) and white(pixel(jpeg, 100, 60))


def test_a_screenshot_in_parts_is_masked_once_whole() -> None:
    page = base64.b64encode(solid_jpeg(200, 100)).decode()
    half = len(page) // 2
    reply = {"parts": 2, "size": len(page), "format": "jpeg", "mask": _same(_state((0, 0, 50, 25)))}
    d, ref, ops = _tab_driver(reply, [page[:half], page[half:]])
    jpeg = asyncio.run(d.screenshot(ref))
    assert ops == ["screenshot", "screenshot_part", "screenshot_part"]
    assert black(pixel(jpeg, 50, 25)) and white(pixel(jpeg, 150, 75))


@pytest.mark.parametrize("reply", [
    {"withheld": "the page can't be read, so its secret fields can't be found"},   # the extension couldn't read it
    {"data": "AAAA", "format": "jpeg"},                                  # no states: nobody said where they are
    {"data": "AAAA", "mask": {"before": _state((1, 1, 5, 5)), "after": _state((2, 1, 5, 5))}},   # a field moved
    {"data": base64.b64encode(b"\xff\xd8\xff not a picture").decode(), "format": "jpeg",
     "mask": _same(_state((0, 0, 5, 5)))},                               # can't be painted
])
def test_a_tab_capture_is_withheld_only_when_masking_fails(reply: dict[str, Any]) -> None:
    d, ref, _ = _tab_driver(reply)
    with pytest.raises(Withheld, match="withheld"):
        asyncio.run(d.screenshot(ref))
    _, capture = d.recording_source(ref, "task")
    with pytest.raises(Withheld, match="withheld"):
        asyncio.run(capture())


def test_a_browser_recording_frame_is_masked() -> None:
    page = solid_jpeg(200, 100)
    reply = {"data": base64.b64encode(page).decode(), "mask": _same(_state((50, 0, 50, 50)))}
    d, ref, ops = _tab_driver(reply)
    _, capture = d.recording_source(ref, "task")
    frame = asyncio.run(capture())
    assert ops == ["record_frame"] and black(pixel(frame, 150, 50)) and white(pixel(frame, 50, 50))


# ---------- one capture's masks: the page before and after it ----------

def _merge(before: dict[str, Any], after: dict[str, Any]) -> Any:
    from ferb.act.evidence.mask import PageState, page_masks
    b, a = PageState.parse(before), PageState.parse(after)
    assert b is not None and a is not None
    return page_masks(b, a)


def test_a_still_page_is_masked_with_its_fields() -> None:
    """The top document's fields one by one; a frame element whose frame shows a secret field whole; one whose frame
    shows none is left as it is."""
    clean = {"key": "frame|doc|t1", "rect": [0, 30, 40, 20], "paint": False, "child": "kid"}
    secret = {"key": "frame|doc|t2", "rect": [50, 30, 40, 20], "paint": True, "child": "login"}
    page = _state((1, 1, 5, 5), frames=[clean, secret])
    rects, viewport = _merge(page, page)
    assert sorted(rects) == [(1.0, 1.0, 5.0, 5.0), (50.0, 30.0, 40.0, 20.0)] and viewport == (0.0, 0.0, 100.0, 50.0)


@pytest.mark.parametrize("before, after", [
    (_state((1, 1, 5, 5)), _state((3, 1, 5, 5))),                                     # a field moved
    (_state(), _state((1, 1, 5, 5))),                                                 # appeared mid-capture
    (_state((1, 1, 5, 5)), _state()),                                                 # went mid-capture
    (_state(top="doc"), _state(top="new")),                                           # the page navigated
    (_state(viewport=(0, 0, 100, 50)), _state(viewport=(0, 0, 90, 50))),              # resized
])
def test_a_field_whose_place_during_capture_is_unknown_withholds_the_capture(before: Any, after: Any) -> None:
    with pytest.raises(Withheld, match="withheld"):
        _merge(before, after)


@pytest.mark.parametrize("before, after", [
    ({"paint": True}, {"paint": False, "child": "kid"}),                  # the link landed between the looks
    ({"paint": False, "child": "kid"}, {"paint": True, "child": "kid"}),  # a secret field appeared in the frame
])
def test_a_frame_whose_link_or_secret_changed_during_capture_is_painted_whole(before: Any, after: Any) -> None:
    host = {"key": "frame|doc|t1", "rect": [0, 30, 40, 20]}
    rects, _ = _merge(_state(frames=[host | before]), _state(frames=[host | after]))
    assert rects == [(0.0, 30.0, 40.0, 20.0)]


def test_an_iframe_reloading_during_capture_is_painted_whole_not_withheld() -> None:
    old = [{"key": "frame|doc|t1", "rect": [0, 30, 40, 20], "paint": False, "child": "old"}]
    new = [{"key": "frame|doc|t1", "rect": [0, 30, 40, 20], "paint": False, "child": "new"}]
    rects, _ = _merge(_state(frames=old), _state(frames=new))
    assert rects == [(0.0, 30.0, 40.0, 20.0)]


# ---------- native window captures: AX secret fields and the extension's rects ----------

def _brave(monkeypatch: pytest.MonkeyPatch, *answers: Any) -> tuple[Any, Any, Any, list[Any]]:
    """Brave's window at BOUNDS, captured at 2x, its page drawn under an 80-point toolbar (the AX web area, read);
    the extension answers for that window with each of `answers` in turn (then None: no profile could tell)."""
    d, ax, shots, *_ = vision(monkeypatch)
    shots.jpeg = solid_jpeg(*IMG)
    ax.tree = AxTree(title="Sign in", secret_rects=[], has_web=True, web_areas=[(100.0, 130.0, 800.0, 520.0)])
    asked: list[Any] = []
    queue = list(answers)

    async def secrets(browser: str, bounds: Any) -> Any:
        asked.append((browser, bounds))
        return queue.pop(0) if queue else None
    d.web_secrets = secrets
    return d, ax, shots, asked


def _page(*fields: Any, viewport: Any = (0, 0, 800, 520)) -> Any:
    from ferb.act.evidence.mask import PageState
    from ferb.act.mac.driver import WebSecrets
    return WebSecrets(PageState.parse(_state(*fields, viewport=viewport)))


@pytest.mark.parametrize("zoom", [1.0, 1.25])
def test_a_browser_window_capture_masks_the_fields_the_extension_marks_secret(monkeypatch: pytest.MonkeyPatch,
                                                                             zoom: float) -> None:
    page = _page((20 / zoom, 40 / zoom, 100 / zoom, 20 / zoom), viewport=(0, 0, 800 / zoom, 520 / zoom))
    d, _, shots, asked = _brave(monkeypatch, page, page)
    _, shot, jpeg = asyncio.run(d.screenshot("Brave"))
    assert asked == [("brave", BOUNDS)] * 2 and shots.captured == [120] and shot.masking == ""
    assert black(pixel(jpeg, 100, 255))                          # (120, 170) pt -> (40..240, 240..280) image px
    assert white(pixel(jpeg, 100, 300)) and white(pixel(jpeg, 20, 20)) and white(pixel(jpeg, 1500, 1100))


def test_a_split_view_paints_the_page_fields_on_every_area_that_fits(monkeypatch: pytest.MonkeyPatch) -> None:
    page = _page((20, 40, 100, 20), viewport=(0, 0, 400, 520))
    d, ax, _, _ = _brave(monkeypatch, page, page)
    ax.tree.web_areas = [(100.0, 130.0, 400.0, 520.0), (500.0, 130.0, 400.0, 520.0)]    # two tabs side by side
    _, _, jpeg = asyncio.run(d.screenshot("Brave"))
    assert black(pixel(jpeg, 100, 255)) and black(pixel(jpeg, 900, 255))   # can't tell which half: both painted
    assert white(pixel(jpeg, 500, 255))


def test_a_shown_password_in_a_native_browser_capture_is_masked_from_ax_and_says_so(
        monkeypatch: pytest.MonkeyPatch) -> None:
    """No profile can tell (incognito, not paired): the AX inventory's password element (an AXTextField its
    metadata names a password field, as "Show password" leaves it) is painted over, and the shot says AX only."""
    d, ax, shots, _ = _brave(monkeypatch)
    ax.tree.secret_rects = [(150.0, 200.0, 100.0, 20.0)]
    ax.secure = [True, True]
    _, shot, jpeg = asyncio.run(d.screenshot("Brave"))
    assert black(pixel(jpeg, 150, 310)) and white(pixel(jpeg, 150, 360)) and white(pixel(jpeg, 20, 20))
    assert "Accessibility only" in shot.masking


def test_a_browser_page_nobody_read_is_sent_masked_by_text_recognition_and_says_so(
        monkeypatch: pytest.MonkeyPatch) -> None:
    """Owner, 2026-10-14 ("no screenshots should be refused; go with redacted stuff") and the live case of 2026-10-19
    (a Greenhouse page in Brave, "pixels withheld"): when neither the extension nor Accessibility can read the page,
    its secret fields can't be located, so the capture goes on to the OCR layer with a note saying so; the secret
    fields Accessibility found elsewhere in the window are still painted."""
    d, ax, shots, _ = _brave(monkeypatch)
    ax.tree.empty_web_areas = [(100.0, 130.0, 800.0, 520.0)]    # Accessibility shows the page area, empty
    ax.tree.secret_rects = [(150.0, 60.0, 100.0, 20.0)]         # a password field in the browser's own toolbar UI
    ax.secure = [True, True]
    _, shot, jpeg = asyncio.run(d.screenshot("Brave"))
    assert shots.captured == [120] and black(pixel(jpeg, 150, 30)) and white(pixel(jpeg, 600, 600))
    assert "couldn't be located" in shot.masking and "text recognition only" in shot.masking
    assert "neither the browser extension nor Accessibility" in shot.masking
    ax.tree.empty_web_areas, ax.tree.has_web = [], False        # no page area at all
    _, shot, _ = asyncio.run(d.screenshot("Brave"))
    assert "text recognition only" in shot.masking and shots.captured == [120, 120]


def test_a_mac_screenshot_of_a_page_nobody_read_reaches_the_model_through_text_recognition(
        daemon: RunningDaemon, monkeypatch: pytest.MonkeyPatch) -> None:
    shots, _ = _fake_mac(daemon, monkeypatch)
    seen = _ocr_log(monkeypatch)
    daemon.daemon.mac.ax.tree = AxTree(secret_rects=[], has_web=True, web_areas=[(100.0, 130.0, 800.0, 520.0)],
                                       empty_web_areas=[(100.0, 130.0, 800.0, 520.0)])  # type: ignore[union-attr]
    g = grant_task(daemon, "n", Identity.OWNER, {"screen"}, {"use_app"})
    r = gateway_call(daemon, g, "mac.screenshot", app="Brave")
    assert not r["is_error"] and "image_withheld" not in r["content"], r
    assert base64.b64decode(r["image"]["data"]) == shots.jpeg and seen == [shots.jpeg]   # OCR looked first
    assert "text recognition only" in r["content"]["masking"]
    assert daemon.run(_evidence(daemon, "n")) == [("screenshot", True)]


def test_the_note_says_why_neither_the_extension_nor_accessibility_read_the_page(monkeypatch: pytest.MonkeyPatch) -> None:
    """So the next live case explains itself from the trace: the extension's reason (here, two of the profile's
    windows at the captured window's place, as two maximized windows are) and what Accessibility showed."""
    from ferb.act.mac.driver import WebSecrets
    why = WebSecrets(None, "a paired profile has more than one window at this window's place")
    d, ax, _, _ = _brave(monkeypatch, *[why] * 6)                     # two looks for each of three captures
    ax.tree.empty_web_areas = [(100.0, 130.0, 800.0, 520.0)]
    _, shot, _ = asyncio.run(d.screenshot("Brave"))
    assert "more than one window at this window's place" in shot.masking and "page area is empty" in shot.masking
    assert "text recognition only" in shot.masking
    ax.tree.empty_web_areas, ax.tree.has_web = [], False
    _, shot, _ = asyncio.run(d.screenshot("Brave"))
    assert "shows no page area" in shot.masking
    ax.tree.has_web = True
    _, shot, _ = asyncio.run(d.screenshot("Brave"))                     # AX read it: masked from AX, and why
    assert "Accessibility only" in shot.masking and "more than one window" in shot.masking


def test_page_fields_the_extension_gave_for_one_look_only_are_still_painted(monkeypatch: pytest.MonkeyPatch) -> None:
    """The extension answered before the capture but not after (a timeout): the fields it named are painted where it
    saw them, on top of Accessibility's, rather than dropped."""
    from ferb.act.mac.driver import WebSecrets
    d, _, _, _ = _brave(monkeypatch, _page((20, 40, 100, 20)), WebSecrets(None, "a paired profile didn't answer"))
    _, shot, jpeg = asyncio.run(d.screenshot("Brave"))
    assert black(pixel(jpeg, 100, 255)) and white(pixel(jpeg, 100, 320))
    assert "Accessibility only" in shot.masking and "didn't answer" in shot.masking


def test_the_capture_reads_the_ax_window_with_the_captured_windows_title(monkeypatch: pytest.MonkeyPatch) -> None:
    """Two maximized windows share bounds: Accessibility's window is the one with the captured window's title."""
    d, ax, shots, *_ = vision(monkeypatch)
    asked: list[Any] = []
    ax.window_with_frame = lambda pid, bounds, title=None: asked.append(title) or "win"   # type: ignore[method-assign]
    asyncio.run(d.screenshot("Notes"))
    assert asked == ["w", "w"]                                       # FakeShots' window title, both looks


def test_an_ax_window_at_a_shared_place_is_told_apart_by_its_title(monkeypatch: pytest.MonkeyPatch) -> None:
    from ferb.act.mac import accessibility as axm
    monkeypatch.setattr(axm, "ax_windows", lambda pid: ["kaggle", "greenhouse"])
    monkeypatch.setattr(axm, "frame", lambda win: (0.0, 30.0, 1400.0, 900.0))
    monkeypatch.setattr(axm, "window_title", lambda win: {"kaggle": "Kaggle", "greenhouse": "Job – Panthalassa"}[win])
    assert axm.window_with_frame(1, (0.0, 30.0, 1400.0, 900.0)) is None                     # never the first
    assert axm.window_with_frame(1, (0.0, 30.0, 1400.0, 900.0), "Job – Panthalassa") == "greenhouse"
    assert axm.window_with_frame(1, (0.0, 30.0, 1400.0, 900.0), "Something else") is None
    monkeypatch.setattr(axm, "ax_windows", lambda pid: ["greenhouse"])
    assert axm.window_with_frame(1, (0.0, 30.0, 1400.0, 900.0), "renamed meanwhile") == "greenhouse"


def test_a_browser_page_area_that_hasnt_appeared_yet_is_read_again(monkeypatch: pytest.MonkeyPatch) -> None:
    """Chromium builds its web tree when an Accessibility client asks (Chrome 154 / Brave 151, spike 06; Brave 1.97 and
    Chrome 155 don't know AXManualAccessibility): the first walks can show no page area at all, not only an empty
    one. A browser's walk is read again for ~2 s in both cases; another app's is not."""
    import sys
    from types import SimpleNamespace
    from ferb.act.mac import accessibility as axm
    from tests.test_mac_snapshot import El, fake_ax
    fake_ax(monkeypatch)
    clock = [0.0]
    monkeypatch.setattr(axm, "time", SimpleNamespace(monotonic=lambda: clock[0],
                                                     sleep=lambda s: clock.__setitem__(0, clock[0] + s)))
    monkeypatch.setitem(sys.modules, "ApplicationServices",
                        SimpleNamespace(AXUIElementCreateApplication=lambda pid: El("AXApplication"),
                                        kAXValueCGPointType=1, kAXValueCGSizeType=2))
    monkeypatch.setattr(axm, "windows", lambda pid, win=None: [])
    page = El("AXWebArea", {"AXPosition": (0.0, 80.0), "AXSize": (800.0, 520.0)},
              children=[El("AXStaticText", {"AXValue": "Software Intern"})])
    no_area = El("AXWindow", children=[El("AXScrollArea")])                # the web view, its tree not built yet
    built = El("AXWindow", children=[El("AXScrollArea", children=[page])])
    walks = [no_area, no_area, built]
    monkeypatch.setattr(axm, "app_window", lambda pid: walks.pop(0) if len(walks) > 1 else walks[0])
    tree = axm.read_tree(1, browser=True)
    assert tree.has_web and tree.web_areas == [(0.0, 80.0, 800.0, 520.0)] and "Software Intern" in tree.text
    walks[:] = [no_area]
    clock[0] = 0.0
    tree = axm.read_tree(1, browser=True)                                 # never appears: gives up at ~2 s
    assert not tree.has_web and 2.0 <= clock[0] < 2.5
    clock[0] = 0.0
    axm.read_tree(1, browser=False)
    assert clock[0] == 0.0                                                # another app: one walk


def test_a_secret_field_appearing_during_capture_is_painted_where_it_appeared_and_says_so(
        monkeypatch: pytest.MonkeyPatch) -> None:
    """Owner: no screenshot is refused. A field whose place during the capture isn't certain is painted wherever a
    look saw it, and the note says the rest is left to text recognition (it used to withhold the capture)."""
    d, ax, shots, _ = _brave(monkeypatch, _page(), _page())
    ax.secure = [False, True]

    def appears() -> None:
        ax.tree.secret_rects = [(150.0, 200.0, 100.0, 20.0)]
    shots.on_capture = appears
    _, shot, jpeg = asyncio.run(d.screenshot("Brave"))
    assert black(pixel(jpeg, 150, 310)) and white(pixel(jpeg, 150, 360)) and white(pixel(jpeg, 20, 20))
    assert "appeared or went during the capture" in shot.masking and "text recognition" in shot.masking


def test_a_field_moving_during_capture_is_painted_where_it_was_before_and_after(monkeypatch: pytest.MonkeyPatch) -> None:
    d, ax, shots, *_ = vision(monkeypatch)
    shots.jpeg = solid_jpeg(*IMG)
    ax.tree = AxTree(secret_rects=[(150.0, 200.0, 100.0, 20.0)])
    ax.secure = [True, True]

    def moves() -> None:
        ax.tree.secret_rects = [(150.0, 230.0, 100.0, 20.0)]
    shots.on_capture = moves
    _, shot, jpeg = asyncio.run(d.screenshot("Notes"))
    assert black(pixel(jpeg, 150, 310)) and black(pixel(jpeg, 150, 370))         # (100..300, 300..340) and 360..400
    assert white(pixel(jpeg, 150, 350)) and white(pixel(jpeg, 150, 420))
    assert "moved, appeared or went during the capture" in shot.masking


def test_a_page_field_moving_during_a_native_capture_is_painted_where_each_look_saw_it(
        monkeypatch: pytest.MonkeyPatch) -> None:
    d, _, shots, _ = _brave(monkeypatch, _page((20, 40, 100, 20)), _page((20, 100, 100, 20)))
    _, shot, jpeg = asyncio.run(d.screenshot("Brave"))
    assert black(pixel(jpeg, 100, 255)) and black(pixel(jpeg, 100, 375)) and white(pixel(jpeg, 100, 320))
    assert "the page changed during the capture" in shot.masking and shots.captured == [120]


def test_a_window_accessibility_cant_read_is_left_to_text_recognition_and_says_so(
        monkeypatch: pytest.MonkeyPatch) -> None:
    d, ax, shots, *_ = vision(monkeypatch)
    ax.window_with_frame = lambda pid, bounds, title=None: None            # type: ignore[method-assign]
    _, shot, jpeg = asyncio.run(d.screenshot("Notes"))
    assert shots.captured == [110] and jpeg == shots.jpeg
    assert "couldn't be located" in shot.masking and "Accessibility couldn't read" in shot.masking
    assert "text recognition only" in shot.masking


def test_the_ax_inventory_is_walked_fresh_and_exposed_before_each_capture(monkeypatch: pytest.MonkeyPatch) -> None:
    d, ax, shots, *_ = vision(monkeypatch)
    walks: list[int] = []
    read = ax.read_tree

    def counted(*args: Any) -> AxTree:
        walks.append(1)
        return read(*args)
    ax.read_tree = counted
    asyncio.run(d.screenshot("Notes"))
    assert ax.calls == ["ax.expose"]                             # Electron builds its tree before the first look
    asyncio.run(d.snapshot("Notes"))
    walks.clear()
    asyncio.run(d.screenshot("Notes"))
    assert len(walks) == 2                                       # its own, before and after: never the snapshot's


def test_page_fields_that_cant_be_placed_on_the_window_are_left_to_text_recognition(
        monkeypatch: pytest.MonkeyPatch) -> None:
    page = _page((20, 40, 100, 20))
    d, ax, shots, _ = _brave(monkeypatch, page, page)
    ax.tree.web_areas = []                                       # Accessibility shows no page area to map onto
    _, shot, _ = asyncio.run(d.screenshot("Brave"))
    assert shots.captured == [120] and "couldn't be placed" in shot.masking and "text recognition only" in shot.masking


def test_a_capture_that_cant_be_painted_is_still_withheld(monkeypatch: pytest.MonkeyPatch) -> None:
    """What is left of withholding in a native capture: the picture exists but the boxes can't be painted on it."""
    d, ax, shots, *_ = vision(monkeypatch)
    ax.tree = AxTree(secret_rects=[(150.0, 200.0, 100.0, 20.0)])
    ax.secure = [True, True]

    def broken(image: bytes, boxes: Any) -> bytes:
        raise ValueError("not an image Quartz can decode")
    monkeypatch.setattr("ferb.act.evidence.mask.mask_secret_fields", broken)
    with pytest.raises(Withheld, match="couldn't be painted"):
        asyncio.run(d.screenshot("Notes"))


def test_window_recording_frames_are_masked_not_dropped(monkeypatch: pytest.MonkeyPatch) -> None:
    d, ax, shots, *_ = vision(monkeypatch)
    shots.jpeg = solid_jpeg(*IMG)
    _, shot, _ = asyncio.run(d.screenshot("Notes"))
    _, capture = d.recording_source("Notes", shot.id)
    ax.tree = AxTree(title="Sign in", secret_rects=[(150.0, 100.0, 100.0, 50.0)])
    ax.secure = [True, True]
    frame = asyncio.run(capture())
    assert black(pixel(frame, 200, 150)) and white(pixel(frame, 20, 20)) and white(pixel(frame, 500, 400))


def test_a_withheld_recording_frame_is_skipped_and_the_recording_goes_on(tmp_path: Path) -> None:
    from ferb.act.recording import RecordingManager
    ffmpeg = shutil.which("ffmpeg")
    if not ffmpeg:
        pytest.skip("recording requires ffmpeg")
    frame = subprocess.check_output([ffmpeg, "-v", "error", "-f", "lavfi", "-i", "color=red:s=320x180",
                                     "-frames:v", "1", "-f", "image2pipe", "-c:v", "mjpeg", "pipe:1"])
    step = ["ok"]

    async def capture() -> Any:
        if step[0] == "withheld":
            raise Withheld("pixels withheld: the window's secret-field inventory is incomplete")
        return (frame, "masked from Accessibility only") if step[0] == "ax" else frame

    async def check() -> None:
        clock = [0.0]
        m = RecordingManager(now=lambda: clock[0], sanitize=lambda image: image)
        r = await m.start("task", {"kind": "window", "window_id": 110}, tmp_path / "v.mp4", capture, fps=1)
        for at, how in ((1, "withheld"), (2, "ax")):
            step[0], clock[0] = how, at
            await m.sample(r["id"], "task")
        stopped = await m.stop(r["id"], "task")
        assert stopped["frames"] == 2 and stopped["frames_withheld"] == 1 and stopped["error"] is None
        assert "inventory" in stopped["withheld_reason"] and stopped["capture_complete"]
        assert stopped["masking"] == ["masked from Accessibility only"]
    asyncio.run(check())


def test_the_ax_inventory_includes_shown_password_fields_and_the_page_area(monkeypatch: pytest.MonkeyPatch) -> None:
    """Task 12's metadata rule for a password field: an AXTextField / AXTextArea / AXComboBox whose title,
    description, placeholder or (DOM) identifier says passw / pwd; a "Show password" checkbox is no field."""
    from ferb.act.mac import accessibility as ax
    from tests.test_mac_snapshot import El, fake_ax, walk
    fake_ax(monkeypatch)
    win = El("AXWindow", {"AXPosition": (0.0, 0.0), "AXSize": (800.0, 600.0)}, children=[
        El("AXGroup", children=[El("AXWebArea", {"AXPosition": (0.0, 80.0), "AXSize": (800.0, 520.0)}, children=[
            El("AXTextField", {"AXTitle": "Email", "AXPosition": (10.0, 100.0)}),
            El("AXTextField", {"AXTitle": "Enter your password", "AXValue": "Hunter2!x", "AXPosition": (10.0, 140.0)}),
            El("AXTextField", {"AXDOMIdentifier": "login_pwd", "AXPosition": (10.0, 180.0)}),
            El("AXCheckBox", {"AXTitle": "Show password", "AXPosition": (10.0, 220.0)}, acts=["AXPress"]),
            El("AXWebArea", {"AXPosition": (0.0, 300.0), "AXSize": (400.0, 200.0)}),   # an iframe's page, empty
        ])])])
    t = walk(win)
    assert t.secure is True
    assert t.secret_rects == [(10.0, 140.0, 30.0, 10.0), (10.0, 180.0, 30.0, 10.0)]
    assert ax.outermost(t.web_areas) == [(0.0, 80.0, 800.0, 520.0)]
    assert t.empty_web_areas == [(0.0, 300.0, 400.0, 200.0)]
    shown = next(n for n in t.nodes if n.label == "Enter your password")
    assert not shown.secret                                      # masking only; Task 12 redacts its value


def test_the_secret_inventory_walks_below_the_node_lists_depth(monkeypatch: pytest.MonkeyPatch) -> None:
    """The model's node list stops at MAX_DEPTH; the masking inventory doesn't (Electron trees are deep)."""
    from ferb.act.mac import accessibility as ax
    from tests.test_mac_snapshot import El, fake_ax, walk
    fake_ax(monkeypatch)
    monkeypatch.setattr(ax, "MAX_DEPTH", 3)
    leaf = El("AXTextField", {"AXPosition": (5.0, 6.0)}, subrole="AXSecureTextField")
    deep: El = leaf
    for _ in range(2000):                                        # far past any recursion limit
        deep = El("AXGroup", children=[deep])
    t = walk(El("AXWindow", children=[deep]))
    assert "depth" in t.truncated_by and len(t.nodes) <= 5        # the node list is cut, as before
    assert t.secure is True and t.secret_rects == [(5.0, 6.0, 30.0, 10.0)]


# ---------- the extension (Chromium's chrome.runtime: no getFrameId) ----------

CONTENT_FRAMES = r'''
const vm = require("vm"), fs = require("fs");
const source = fs.readFileSync(process.argv[1], "utf8");
const queue = [];                                     // postMessage is asynchronous: delivered after the call returns
function frameWindow(opts) {
  const listeners = {};
  const g = {console, crypto: globalThis.crypto, innerWidth: opts.viewport[0], innerHeight: opts.viewport[1],
    visualViewport: opts.visual || null,
    getComputedStyle: (el, pseudo) => {              // as Chromium computes it: getPropertyValue by CSS name
      const s = Object.assign({"-webkit-box-reflect": "none", filter: "none"},
                              pseudo ? (el.pseudo || {})[pseudo] : el.style);
      return {getPropertyValue: name => name in s ? s[name] : ""};
    },
    MutationObserver: class { observe() {} }, location: {origin: "https://example.test", href: "https://example.test/"},
    addEventListener: (type, fn) => { (listeners[type] = listeners[type] || []).push(fn); },
    chrome: {runtime: {sendMessage() {}}, dom: {openOrClosedShadowRoot: el => el.closedRoot || el.shadowRoot || null}},
    document: {querySelectorAll: () => opts.nodes}};
  g.window = g;
  g.top = opts.top || g;
  g.parent = opts.parent || g;
  g.deliver = (data, source, isTrusted) => queue.push(() =>
    (listeners.message || []).forEach(fn => fn({data, source, isTrusted: isTrusted !== false})));
  vm.createContext(g);
  vm.runInContext(source, g);
  return g;
}
const posted = [];                                    // every message a child frame posted to its parent
function child(page, nodes, viewport) {               // a child frame of `page`: its window as the page sees it
  const proxy = {};
  const up = {postMessage: (data, origin) => { posted.push(data); page.deliver(data, proxy, true); }};
  const kid = frameWindow({viewport, nodes, top: page, parent: up});
  return {kid, proxy};
}
function capture(frames) {                            // the background's three rounds, then each frame's state
  const round = "round-1";
  frames.forEach(f => f.__ferb.listenForFrames(round));
  frames.forEach(f => f.__ferb.announceFrame(round));
  for (const run of queue.splice(0)) run();
  return frames.map((f, i) => ({frameId: i, result: f.__ferb.captureState(round)}));
}
const el = (tag, r, extra) => Object.assign({tagName: tag, type: "text", clientLeft: 0, clientTop: 0,
  offsetWidth: r[2], offsetHeight: r[3], checkVisibility: () => true,
  getBoundingClientRect: () => ({x: r[0], y: r[1], width: r[2], height: r[3]})}, extra || {});
function slotted(host, style, open) {    // `host` is a light child of x, drawn through x's shadow tree: wrapper > slot
  const x = el("DIV", [0, 0, 1000, 800]), wrapper = el("DIV", [0, 0, 1000, 800], {style, parentElement: null});
  const slot = el("SLOT", [0, 0, 0, 0], {parentElement: wrapper, assignedElements: () => [host]});
  const root = {host: x, querySelectorAll: sel => sel === "slot" ? [slot] : [wrapper, slot]};
  wrapper.getRootNode = () => root;
  Object.assign(host, {parentElement: x, assignedSlot: open ? slot : null});   // a closed root hides assignedSlot
  Object.assign(x, open ? {shadowRoot: root} : {closedRoot: root, shadowRoot: null});
  return x;
}
''' + "\n"


def _content(script: str) -> Any:
    ran = subprocess.run([_node(), "-e", CONTENT_FRAMES + script, str(EXT / "content.js")],
                         capture_output=True, text=True, timeout=20)
    assert ran.returncode == 0, ran.stderr
    return json.loads(ran.stdout)


def _mask_state(rows: Any) -> Any:
    """The background's maskState over these frames' captureState rows (the link rounds answer nothing)."""
    return worker('''(async () => {
      inject = async () => {};
      const rows = ROWS;
      chrome.scripting = {executeScript: async args => args.func.toString().includes("captureState") ? rows :
        rows.map(r => ({frameId: r.frameId, result: true}))};
      return await maskState(9, Date.now() + 5000);
    })()'''.replace("ROWS", json.dumps(rows)))


def _painted(state: dict[str, Any]) -> list[list[float]]:
    return sorted(i["rect"] for i in state["items"] if i["paint"])


def _covered(field: list[float], state: dict[str, Any]) -> bool:
    x, y, w, h = field
    return any(r[0] <= x and r[1] <= y and r[0] + r[2] >= x + w and r[1] + r[3] >= y + h for r in _painted(state))


def test_frames_announce_themselves_to_their_parent_and_the_background_places_them() -> None:
    """Chromium has no chrome.runtime.getFrameId: each child frame posts its own fresh token to its parent; the parent
    hears it only as a trusted message from one of its frame elements' windows (an <object> too)."""
    out = _content(r'''
const topNodes = [];
const page = frameWindow({viewport: [1000, 800], nodes: topNodes});
const a = child(page, [el("INPUT", [5, 5, 40, 10], {type: "password"})], [400, 200]);
const b = child(page, [el("INPUT", [1, 2, 30, 10], {type: "password"})], [300, 100]);
const closed = el("DIV", [0, 0, 1, 1], {shadowRoot: null,
  closedRoot: {querySelectorAll: () => [el("INPUT", [30, 40, 20, 10], {type: "password"})]}});
topNodes.push(el("INPUT", [10, 20, 100, 30], {type: "password"}),
  el("IFRAME", [200, 300, 400, 200], {contentWindow: a.proxy}),
  el("OBJECT", [600, 500, 300, 100], {contentWindow: b.proxy}),            // an <object> hosts a frame too
  el("IFRAME", [700, 100, 50, 50], {contentWindow: null}), closed);         // nothing loaded in it
page.__ferb.listenForFrames("round-1");
page.deliver({frameLinkRound: "round-1", frameLinkToken: "forged"}, a.proxy, false);   // synthetic: rejected
page.deliver({frameLinkRound: "round-1", frameLinkToken: "forged"}, page, true);       // from the page itself
const rows = capture([page, a.kid, b.kid]);
console.log(JSON.stringify({rows, posted}));
''')
    rows = out["rows"]
    page = rows[0]["result"]
    assert [f["heard"] for f in page["frames"]] == [[rows[1]["result"]["own"]], [rows[2]["result"]["own"]], []]
    assert all("ferb" not in json.dumps(m).lower() for m in out["posted"])                  # neutral message keys
    state = _mask_state(rows)
    # the top document's own fields exactly (a closed shadow root's too); a child frame's never one by one: the
    # <iframe> and <object> holding a password field are painted whole, as is the host nothing loaded in
    assert _painted(state) == sorted([[10, 20, 100, 30], [30, 40, 20, 10], [200, 300, 400, 200], [600, 500, 300, 100],
                                      [700, 100, 50, 50]])
    assert [i.get("child") for i in state["items"] if i["key"].startswith("frame|")] == [
        rows[1]["result"]["documentId"], rows[2]["result"]["documentId"], None]          # which frame each holds
    assert state["top"] == page["documentId"] and state["viewport"] == [0, 0, 1000, 800]


def _frames(*heard: Any, rects: Any = None) -> list[dict[str, Any]]:
    """Top-frame rows: frame elements E0, E1, ... at x = 0, 200, ... that heard these tokens, and their children."""
    top = {"documentId": "top", "own": None, "viewport": [0, 0, 1400, 800], "rects": [], "frames": [
        {"id": f"E{i}", "heard": h, "rect": [200 * i, 0, 100, 100], "redrawn": False} for i, h in enumerate(heard)]}
    kids = [{"documentId": doc, "own": own, "viewport": [0, 0, 100, 100], "frames": [],
             "rects": [{"id": "f", "rect": r, "redrawn": False}] if r else []} for doc, own, r in (rects or [])]
    return [{"frameId": i, "result": f} for i, f in enumerate([top] + kids)]


@pytest.mark.parametrize("heard, withheld", [
    ((["tA", "tB"], []), False),          # the page blocked B's own message and had decoy A (in E0) post B's token too
    ((["tB"], []), True),                 # ... and blocked A's own message too: A's own field can't be placed
    ((["tA"], ["tB", "tX"]), False),      # a script in B's page posted a second token as B
    ((["tA", "tB"], ["tB"]), False),      # decoy A also posted B's token
    ((["tA"], ["tB", "tB"]), False),      # B's token repeated
])
def test_a_forged_link_can_only_add_black(heard: Any, withheld: bool) -> None:
    """B (the frame with the password field) really sits in E1, decoy A in E0. Whatever a page forges with messages,
    B's field ends up under black where B really is, or the capture is withheld: never unmasked, never moved."""
    rows = _frames(*heard, rects=[("B", "tB", [10, 10, 40, 10]), ("A", "tA", [50, 50, 10, 10])])
    state = _mask_state(rows)
    assert ("withheld" in state) is withheld, state
    if not withheld:
        assert _covered([210, 10, 40, 10], state)                # B's field where B really is (inside E1's box)


def test_unlinked_or_doubly_heard_frames_are_painted_whole() -> None:
    rows = _frames(["tA"], ["tA"], [], ["tD", "tD"], ["tE"], ["tF"],
                   rects=[("A1", "tA", [1, 1, 5, 5]), ("D", "tD", [1, 1, 5, 5]), ("E", "tE", [2, 2, 5, 5]),
                          ("F", "tF", None)])
    state = _mask_state(rows)
    # E0/E1 heard the same token: both whole; E2 heard nothing: whole; E3 heard one token twice: whole; E4 and E5
    # each heard exactly one token nobody else heard, which exactly one frame owns: E4's frame shows a secret field,
    # so E4 is painted whole; E5's shows none, so E5 is left showing it
    assert _painted(state) == sorted([[0, 0, 100, 100], [200, 0, 100, 100], [400, 0, 100, 100], [600, 0, 100, 100],
                                      [800, 0, 100, 100]])
    assert {"key": "frame|top|E5", "rect": [1000, 0, 100, 100], "paint": False, "child": "F"} in state["items"]
    assert "withheld" not in state


def test_a_frame_whose_secret_fields_cant_be_placed_withholds_the_capture() -> None:
    """An <embed> has no contentWindow: the frame in it is heard by no element. With a password field it withholds
    the capture (nothing could be shown to cover it); without one it is just painted around."""
    out = _content(r'''
const topNodes = [];
const page = frameWindow({viewport: [1000, 800], nodes: topNodes});
const inside = child(page, [el("INPUT", [1, 2, 30, 10], {type: "password"})], [300, 100]);
const plain = child(page, [], [300, 100]);
topNodes.push(el("EMBED", [100, 100, 300, 100], {}));
console.log(JSON.stringify({secret: capture([page, inside.kid]), plain: capture([page, plain.kid])}));
''')
    assert "withheld" in _mask_state(out["secret"])
    plain = _mask_state(out["plain"])
    assert "withheld" not in plain and _painted(plain) == [[100, 100, 300, 100]]   # the <embed> box itself
    rows = _frames(rects=[("lost", "tL", [1, 1, 5, 5])])                          # nobody heard it at all
    assert "withheld" in _mask_state(rows)
    rows = _frames()
    rows[0]["result"]["frames"] = []
    worker_rows = rows + [{"frameId": 1, "result": {"documentId": "x", "own": "tx", "viewport": [0, 0, 1, 1],
                                                    "rects": [], "frames": []}}]
    assert "withheld" not in _mask_state(worker_rows)                              # no secret field: no harm


def test_the_mask_state_is_withheld_only_when_the_page_cant_be_read() -> None:
    out = worker('''(async () => {
      inject = async () => {};
      chrome.scripting = {executeScript: async args => args.func.toString().includes("captureState") ?
        [{frameId: 3, result: {documentId: 'kid', own: 't', viewport: [0, 0, 1, 1], rects: [], frames: []}}] : []};
      const noTop = await maskState(9, Date.now() + 5000);
      chrome.scripting = {executeScript: async () => { throw Error('Cannot access contents of the page'); }};
      return {noTop, unreadable: await maskState(9, Date.now() + 5000)};
    })()''')
    assert "withheld" in out["noTop"] and "withheld" in out["unreadable"]


def test_overlapping_rounds_each_keep_their_own_links() -> None:
    """Two looks at one tab can overlap (a recording's frames while a worker screenshots it): each round keeps what
    its own announcements said, and a round's state is dropped once reported."""
    out = _content(r'''
const topNodes = [];
const page = frameWindow({viewport: [1000, 800], nodes: topNodes});
const a = child(page, [el("INPUT", [5, 5, 40, 10], {type: "password"})], [400, 200]);
topNodes.push(el("IFRAME", [200, 300, 400, 200], {contentWindow: a.proxy}));
const frames = [page, a.kid];
for (const r of ["r1", "r2"]) frames.forEach(f => f.__ferb.listenForFrames(r));
for (const r of ["r1", "r2"]) frames.forEach(f => f.__ferb.announceFrame(r));
for (const run of queue.splice(0)) run();
const looks = ["r1", "r2"].map(r => frames.map((f, i) => ({frameId: i, result: f.__ferb.captureState(r)})));
const again = page.__ferb.captureState("r1");                         // reported once: gone
console.log(JSON.stringify({looks, again}));
''')
    for rows in out["looks"]:
        page, kid = rows[0]["result"], rows[1]["result"]
        assert page["frames"][0]["heard"] == [kid["own"]] and kid["own"]
        assert _mask_state(rows)["items"] == [{"key": f"frame|{page['documentId']}|{page['frames'][0]['id']}",
                                               "rect": [200, 300, 400, 200], "paint": True,
                                               "child": kid["documentId"]}]    # linked exactly: no second black box
    assert out["looks"][0][1]["result"]["own"] != out["looks"][1][1]["result"]["own"]
    assert out["again"]["frames"][0]["heard"] == []


def test_looks_at_one_tab_run_one_at_a_time() -> None:
    out = worker('''(async () => {
      inject = async () => {};
      const order = [];
      chrome.scripting = {executeScript: async args => {
        const step = ["listenForFrames", "announceFrame", "captureState"].find(n => args.func.toString().includes(n));
        order.push(step + ":" + args.args[0]);
        await new Promise(resolve => setTimeout(resolve, 5));
        return step === "captureState" ? [{frameId: 0, result: {documentId: 'top', own: null,
          viewport: [0, 0, 10, 10], rects: [], frames: []}}] : [];
      }};
      const [one, two] = await Promise.all([maskState(4, Date.now() + 5000), maskState(4, Date.now() + 5000)]);
      return {order, one, two};
    })()''')
    rounds = [step.split(":")[1] for step in out["order"]]
    assert len(out["order"]) == 6 and rounds[:3] == [rounds[0]] * 3 and rounds[3:] == [rounds[3]] * 3
    assert rounds[0] != rounds[3] and "withheld" not in out["one"] and "withheld" not in out["two"]


def test_a_stuck_look_holds_its_tab_only_until_its_deadline() -> None:
    """A look that never finishes (a frame that never answers) holds its tab only until its command's deadline; a look
    waits for its turn only until its own deadline and is skipped, never run late, when that passes first."""
    out = worker('''(async () => {
      inject = async () => {};
      const calls = {4: [], 5: [], 6: []}, first = {};
      chrome.scripting = {executeScript: args => {
        const tab = args.target.tabId, round = args.args[0];
        calls[tab].push(round);
        if (!(tab in first)) first[tab] = round;
        if (round === first[tab]) return new Promise(() => {});              // each tab's first look never comes back
        const step = ["listenForFrames", "announceFrame", "captureState"].find(n => args.func.toString().includes(n));
        return Promise.resolve(step === "captureState" ? [{frameId: 0, result: {documentId: 'top', own: null,
          viewport: [0, 0, 10, 10], rects: [], frames: []}}] : []);
      }};
      const now = Date.now(), gaveUp = new Promise(resolve => setTimeout(() => resolve('still waiting'), 3000));
      maskState(4, now + 100);                                                // stuck
      const next = maskState(4, now + 5000);                                  // its turn: the stuck look's deadline
      maskState(5, now + 2000);                                               // stuck
      const skipped = maskState(5, now + 50).then(r => ({r, after: Date.now() - now}));   // its deadline comes first
      const late = await maskState(6, now - 1);                               // already past its deadline
      return {late, skipped: await Promise.race([skipped, gaveUp]), next: await Promise.race([next, gaveUp]),
              calls: [calls[4].length, new Set(calls[4]).size, calls[5].length, calls[6].length]};
    })()''')
    assert out["next"] == {"top": "top", "viewport": [0, 0, 10, 10], "items": []}, out
    assert "withheld" in out["skipped"]["r"] and out["skipped"]["after"] < 1000, out    # not the stuck look's 2 s
    assert "withheld" in out["late"], out
    assert out["calls"] == [4, 2, 1, 0]          # a skipped look never touches the page; the next one runs whole


def test_every_look_waits_only_until_its_commands_deadline() -> None:
    """The deadline is the command's own (executeCommand's _timeout_ms), not a new number."""
    out = worker('''(async () => {
      attach = async id => tabState(id);
      cdp = async () => ({data: 'jpeg'});
      const left = [];
      maskState = async (tab, deadline) => {
        left.push(deadline - Date.now());
        return {top: 'd', viewport: [0, 0, 10, 10], items: []};
      };
      chrome.windows = {getAll: async () => [{id: 1, left: 0, top: 0, width: 10, height: 10, state: 'normal'}]};
      chrome.tabs.query = async () => [{id: 7}];
      await commands.screenshot({tab_id: 3, _timeout_ms: 4000});
      await commands.record_frame({tab_id: 3, _timeout_ms: 4000});
      await commands.window_secrets({bounds: [0, 0, 10, 10], _timeout_ms: 4000});
      return left;
    })()''')
    assert len(out) == 5 and all(3000 < ms <= 4000 for ms in out), out


def test_looks_at_one_tab_stay_one_at_a_time_when_a_waiting_look_gives_up() -> None:
    """A holds the tab, B gives up waiting while A still runs, C must still wait for A: the chain stays serial."""
    out = worker('''(async () => {
      inject = async () => {};
      const log = [];
      let first = null;
      chrome.scripting = {executeScript: async args => {
        const round = args.args[0];
        if (first === null) first = round;
        const who = round === first ? "A" : "C";
        const step = ["listenForFrames", "announceFrame", "captureState"].find(n => args.func.toString().includes(n));
        log.push(who + ":" + step);
        if (who === "A") await new Promise(resolve => setTimeout(resolve, 150));   // A takes a while, well in time
        return step === "captureState" ? [{frameId: 0, result: {documentId: 'top', own: null,
          viewport: [0, 0, 10, 10], rects: [], frames: []}}] : [];
      }};
      const now = Date.now();
      const looks = [maskState(4, now + 5000), maskState(4, now + 50), maskState(4, now + 5000)];
      const [a, b, c] = await Promise.all(looks);
      return {log, a, b, c};
    })()''')
    assert out["log"] == ["A:listenForFrames", "A:announceFrame", "A:captureState",
                          "C:listenForFrames", "C:announceFrame", "C:captureState"], out["log"]
    assert "withheld" in out["b"] and "withheld" not in out["a"] and "withheld" not in out["c"]


def test_a_message_without_a_source_is_nobodys_announcement() -> None:
    out = _content(r'''
const page = frameWindow({viewport: [1000, 800], nodes: [el("IFRAME", [700, 100, 50, 50], {contentWindow: null})]});
page.__ferb.listenForFrames("round-1");
page.deliver({frameLinkRound: "round-1", frameLinkToken: "from-nowhere"}, null, true);
for (const run of queue.splice(0)) run();
console.log(JSON.stringify(page.__ferb.captureState("round-1")));
''')
    assert out["frames"][0]["heard"] == []                        # not the empty host's: it has no window to send


def _scene(build: str, kid: str = 'el("INPUT", [5, 5, 40, 10], {type: "password"})') -> Any:
    """The top frame's and its one child's captureState rows. `build` puts the child's frame element (`host`, holding
    `a.proxy`) and whatever surrounds it into topNodes; rects are what getBoundingClientRect reports in the browser,
    every transform, zoom and motion path already applied."""
    return _content(r'''
const topNodes = [];
const page = frameWindow({viewport: [1000, 800], nodes: topNodes});
const a = child(page, [__KID__], [200, 200]);
__BUILD__
console.log(JSON.stringify(capture([page, a.kid])));
'''.replace("__KID__", kid).replace("__BUILD__", build))


@pytest.mark.parametrize("build, painted", [
    # round-3 re-review: a flip on a shadow-tree wrapper around the <slot> the iframe is drawn through
    ('const host = el("IFRAME", [200, 300, 200, 200], {contentWindow: a.proxy});'
     'topNodes.push(slotted(host, {transform: "matrix(-1, 0, 0, 1, 0, 0)"}, false), host);', [200, 300, 200, 200]),
    ('const host = el("IFRAME", [200, 300, 200, 200], {contentWindow: a.proxy});'
     'topNodes.push(slotted(host, {transform: "matrix(-1, 0, 0, 1, 0, 0)"}, true), host);', [200, 300, 200, 200]),
    # a CSS motion path turns the host; computed `transform` stays none
    ('const host = el("IFRAME", [200, 300, 200, 200], {contentWindow: a.proxy,'
     ' style: {offsetPath: "path(\'M 0 0 H 100\')", offsetRotate: "auto 90deg"}}); topNodes.push(host);',
     [200, 300, 200, 200]),
    # CSS zoom on an ancestor: the host's box is drawn twice as big, its offset size agrees with it
    ('const box = el("DIV", [0, 0, 1000, 800], {style: {zoom: "2"}});'
     'const host = el("IFRAME", [200, 300, 400, 400], {contentWindow: a.proxy, parentElement: box});'
     'topNodes.push(box, host);', [200, 300, 400, 400]),
    ('const host = el("IFRAME", [200, 300, 200, 200], {contentWindow: a.proxy,'
     ' style: {transform: "matrix(-1, 0, 0, 1, 0, 0)"}}); topNodes.push(host);', [200, 300, 200, 200]),
    ('const host = el("IFRAME", [210, 320, 200, 200], {contentWindow: a.proxy,'
     ' style: {transform: "matrix(1, 0, 0, 1, 10, 20)"}}); topNodes.push(host);', [210, 320, 200, 200]),
    # partly out of view: the part of the host the viewport shows
    ('const host = el("IFRAME", [900, 700, 400, 400], {contentWindow: a.proxy}); topNodes.push(host);',
     [900, 700, 100, 100]),
], ids=["slot-flipped-closed", "slot-flipped-open", "motion-path", "zoomed-ancestor", "flipped-host", "moved-host",
        "partly-in-view"])
def test_a_secret_field_in_a_child_frame_paints_its_whole_host(build: str, painted: list[int]) -> None:
    """Controller ruling, round 4: a child frame's fields are never placed one by one (CSS on the host's ancestors can
    move the frame where no offset says: a transform around a <slot>, a motion path, zoom). Its host's own box, which
    already includes everything the browser applies, is painted whole instead."""
    rows = _scene(build)
    state = _mask_state(rows)
    assert "withheld" not in state and _painted(state) == [painted]
    host = state["items"][0]
    assert host["child"] == rows[1]["result"]["documentId"]       # the link still names the frame inside


def test_a_secret_field_deeper_in_frames_paints_the_top_documents_host_whole() -> None:
    out = _content(r'''
function nest(inner) {                     // top > iframe H1 > frame C1 > (iframe H2 > frame C2) + `inner` elements
  const topNodes = [], midNodes = [];
  const page = frameWindow({viewport: [1000, 800], nodes: topNodes});
  const c1 = child(page, midNodes, [500, 400]);
  const c2 = child(c1.kid, inner.c2, [200, 100]);
  topNodes.push(el("IFRAME", [100, 100, 500, 400], {contentWindow: c1.proxy}));
  midNodes.push(el("IFRAME", [10, 10, 200, 100], {contentWindow: c2.proxy}), ...inner.c1);
  return capture([page, c1.kid, c2.kid]);
}
const secret = () => el("INPUT", [1, 1, 30, 10], {type: "password"});
console.log(JSON.stringify({
  deep: nest({c1: [], c2: [secret()]}),                                       // the field two frames down
  middle: nest({c1: [secret()], c2: []}),                                     // in the frame between
  clean: nest({c1: [], c2: []}),                                              // nothing to hide anywhere
  unlinked: nest({c1: [el("IFRAME", [300, 10, 50, 50], {contentWindow: null})], c2: []}),   // nothing linked in it
  hidden: nest({c1: [el("IFRAME", [300, 10, 0, 0], {contentWindow: null})], c2: []}),       // ... but it has no area
}));
''')
    whole = [[100, 100, 500, 400]]
    for case in ("deep", "middle", "unlinked"):
        state = _mask_state(out[case])
        assert "withheld" not in state and _painted(state) == whole, case     # never placed in a frame's frame
    for case in ("clean", "hidden"):
        state = _mask_state(out[case])
        assert state["items"] == [{"key": state["items"][0]["key"], "rect": [100, 100, 500, 400], "paint": False,
                                   "child": out[case][1]["result"]["documentId"]}], case


def test_a_top_document_field_is_masked_exactly_whatever_its_ancestors_do() -> None:
    """The top document's own fields keep precise masks: getBoundingClientRect is already the box the field is drawn
    in, in viewport px, whatever transforms its ancestors or a shadow tree apply."""
    rows = _scene('''
const turned = el("DIV", [0, 0, 600, 600], {style: {transform: "matrix(0.7, 0.7, -0.7, 0.7, 0, 0)"}});
const wrapper = el("DIV", [0, 0, 600, 600], {style: {transform: "matrix(-1, 0, 0, 1, 0, 0)"}, parentElement: null});
const shadowed = el("INPUT", [400, 50, 120, 20], {type: "password", parentElement: wrapper});
const root = {host: null, querySelectorAll: () => [wrapper, shadowed]};
wrapper.getRootNode = () => root;
const x = el("DIV", [0, 0, 1000, 800], {closedRoot: root, shadowRoot: null});
root.host = x;
topNodes.push(turned, el("INPUT", [37, 61, 141, 141], {type: "password", parentElement: turned}), x);
''', kid="")
    state = _mask_state(rows)
    assert "withheld" not in state and _painted(state) == [[37, 61, 141, 141], [400, 50, 120, 20]]


COPIERS = {"reflect": '{"-webkit-box-reflect": "below 0px"}',
           "svg-filter": '{filter: \'url("#shift")\'}'}       # an feOffset/feTile filter redraws content elsewhere


@pytest.mark.parametrize("copier", sorted(COPIERS))
@pytest.mark.parametrize("build, kid, withheld", [
    # the secret-bearing frame's host, drawn through a shadow tree whose wrapper around the <slot> copies it
    ('const host = el("IFRAME", [200, 300, 200, 200], {contentWindow: a.proxy});'
     'topNodes.push(slotted(host, __STYLE__, false), host);', None, True),
    ('const host = el("IFRAME", [200, 300, 200, 200], {contentWindow: a.proxy});'
     'topNodes.push(slotted(host, __STYLE__, true), host);', None, True),
    ('const box = el("DIV", [0, 0, 600, 600], {style: __STYLE__});'
     'const host = el("IFRAME", [200, 300, 200, 200], {contentWindow: a.proxy, parentElement: box});'
     'topNodes.push(box, host);', None, True),
    # a host nothing is linked in (painted whole: it may hold anything), under a copying ancestor
    ('const box = el("DIV", [0, 0, 600, 600], {style: __STYLE__});'
     'topNodes.push(box, el("IFRAME", [700, 100, 50, 50], {contentWindow: null, parentElement: box}));', "", True),
    # a top-document field: its copying ancestor, itself, or the host of the shadow tree it is in
    ('const box = el("DIV", [0, 0, 600, 600], {style: __STYLE__});'
     'topNodes.push(box, el("INPUT", [10, 20, 100, 30], {type: "password", parentElement: box}));', "", True),
    ('topNodes.push(el("INPUT", [10, 20, 100, 30], {type: "password", style: __STYLE__}));', "", True),
    ('const field = el("INPUT", [10, 20, 100, 30], {type: "password", parentElement: null});'
     'const x = el("DIV", [0, 0, 600, 600], {style: __STYLE__, shadowRoot: null});'
     'const root = {host: x, querySelectorAll: () => [field]}; field.getRootNode = () => root; x.closedRoot = root;'
     'topNodes.push(x);', "", True),
    # a <details>' content box (::details-content) around a field, open or closed (CSS can show closed content)
    ('const details = el("DETAILS", [0, 0, 600, 600], {open: true, pseudo: {"::details-content": __STYLE__}});'
     'const summary = el("SUMMARY", [0, 0, 600, 20], {parentElement: details});'
     'const field = el("INPUT", [10, 40, 100, 30], {type: "password", parentElement: details});'
     'details.children = [summary, field]; topNodes.push(details, summary, field);', "", True),
    ('const details = el("DETAILS", [0, 0, 600, 600], {open: false, pseudo: {"::details-content": __STYLE__}});'
     'const field = el("INPUT", [10, 40, 100, 30], {type: "password", parentElement: details});'
     'details.children = [field]; topNodes.push(details, field);', "", True),
    # nothing to hide is copied: a host whose frame shows no secret field, a copying box beside the field, a field
    # in the <summary>, which isn't in the content box
    ('const box = el("DIV", [0, 0, 600, 600], {style: __STYLE__});'
     'const host = el("IFRAME", [200, 300, 200, 200], {contentWindow: a.proxy, parentElement: box});'
     'topNodes.push(box, host);', "", False),
    ('topNodes.push(el("DIV", [0, 0, 600, 600], {style: __STYLE__}),'
     'el("INPUT", [610, 20, 100, 30], {type: "password"}));', "", False),
    ('const details = el("DETAILS", [0, 0, 600, 600], {open: true, pseudo: {"::details-content": __STYLE__}});'
     'const summary = el("SUMMARY", [0, 0, 600, 20], {parentElement: details});'
     'const field = el("INPUT", [10, 2, 100, 16], {type: "password", parentElement: summary});'
     'details.children = [summary]; topNodes.push(details, summary, field);', "", False),
], ids=["slot-closed", "slot-open", "host-ancestor", "unlinked-host", "field-ancestor", "field-itself",
        "field-in-shadow-tree", "details-content", "details-content-closed", "clean-host", "beside-the-field",
        "in-the-summary"])
def test_a_secret_drawn_again_elsewhere_withholds_the_capture(build: str, kid: str | None, withheld: bool,
                                                               copier: str) -> None:
    """-webkit-box-reflect, or an SVG filter (feOffset/feTile with a user-space region), draws a copy of an element
    and everything in it outside its box, where no mask goes: a secret field or a host painted whole under one
    withholds the capture."""
    build = build.replace("__STYLE__", COPIERS[copier])
    state = _mask_state(_scene(build) if kid is None else _scene(build, kid))
    assert ("withheld" in state) is withheld, state


@pytest.mark.parametrize("style, withheld", [
    ('{filter: "drop-shadow(rgb(0, 0, 0) 4px 4px 8px)"}', False),   # plain filters: the accepted residual
    ('{filter: "blur(2px)"}', False),
    ('{"-webkit-box-reflect": null}', True),                         # an answer that isn't a string
    ('{"-webkit-box-reflect": ""}', True),                           # or isn't one Chromium gives: unknown
    ('{filter: null}', True),
])
def test_a_style_that_cant_be_read_counts_as_a_copy(style: str, withheld: bool) -> None:
    state = _mask_state(_scene('const box = el("DIV", [0, 0, 600, 600], {style: __STYLE__});'
                               'topNodes.push(box, el("INPUT", [10, 20, 100, 30], {type: "password",'
                               ' parentElement: box}));'.replace("__STYLE__", style), ""))
    assert ("withheld" in state) is withheld, state


def test_a_pinch_zoom_reports_the_visual_viewport() -> None:
    rows = _content(r'''
const page = frameWindow({viewport: [1000, 800], visual: {offsetLeft: 40, offsetTop: 20, width: 500, height: 400},
  nodes: [el("INPUT", [50, 25, 10, 5], {type: "password"})]});
console.log(JSON.stringify([page.__ferb.captureState()]));
''')
    assert rows[0]["viewport"] == [40, 20, 500, 400]


@pytest.mark.parametrize("case", ["secret", "unreadable"])
@pytest.mark.parametrize("op", ["screenshot", "record_frame"])
def test_extension_captures_carry_the_page_before_and_after(op: str, case: str) -> None:
    out = worker('''(async () => {
      attach = async id => tabState(id); let reads = 0, captures = 0;
      maskState = async () => {
        reads++;
        if (CASE === 'unreadable') return {withheld: "the page can't be read, so its secret fields can't be found"};
        return {top: 'doc', viewport: [0, 0, 100, 50],
                items: [{key: 'field|doc|f', rect: [reads, 1, 2, 2], paint: true}]};
      };
      cdp = async () => { captures++; return {data: 'jpeg-bytes'}; };
      const st = tabState(19); st.shot = 'previous'; st.visual = {snap: 7};
      const out = await commands[OP]({tab_id: 19});
      return {out, captures};
    })()'''.replace("CASE", json.dumps(case)).replace("OP", json.dumps(op)))
    if case == "secret":
        assert out["out"]["data"] == "jpeg-bytes"
        assert [out["out"]["mask"][k]["items"][0]["rect"] for k in ("before", "after")] == [[1, 1, 2, 2], [2, 1, 2, 2]]
    else:
        assert "data" not in out["out"] and out["out"]["withheld"]
    assert out["captures"] == (0 if case == "unreadable" else 1)


def test_the_extension_reports_a_windows_page_for_native_captures() -> None:
    out = worker('''(async () => {
      chrome.windows = {getAll: async () => [{id: 1, left: 10, top: 20, width: 800, height: 600, state: 'normal'},
        {id: 9, left: 10, top: 20, width: 800, height: 600, state: 'minimized'},
        {id: 2, left: 0, top: 0, width: 500, height: 400}, {id: 3, left: 0, top: 0, width: 500, height: 400}]};
      let queried = [];
      chrome.tabs.query = async ({windowId}) => { queried.push(windowId); return [{id: windowId * 10}]; };
      const page = {top: 'd', viewport: [0, 0, 800, 520], items: [{key: 'field|d|f', rect: [1, 2, 3, 4], paint: true}]};
      maskState = async tab => tab === 10 ? page : {withheld: "the page can't be read"};
      const one = await commands.window_secrets({bounds: [10, 20, 800, 600]});
      const ambiguous = await commands.window_secrets({bounds: [0, 0, 500, 400]});
      const missing = await commands.window_secrets({bounds: [1, 1, 5, 5]});
      chrome.windows = {getAll: async () => [{id: 4, left: 0, top: 0, width: 500, height: 400}]};
      const unreadable = await commands.window_secrets({bounds: [0, 0, 500, 400]});
      return {one, ambiguous, missing, unreadable, queried, page};
    })()''')
    assert out["one"] == {"matched": True, "complete": True, "state": out["page"]}   # the minimized one is no match
    assert out["ambiguous"] == {"matched": False, "reason": "ambiguous_window"}
    assert out["missing"] == {"matched": False, "reason": "window_not_found"}
    assert out["unreadable"]["matched"] and out["unreadable"]["complete"] is False
    assert out["queried"] == [1, 4]                              # an ambiguous window's tabs are never looked at


def test_a_windows_answer_is_read_strictly() -> None:
    from ferb.act.mac.driver import WebSecrets
    good = WebSecrets.from_reply({"matched": True, "complete": True, "state": _state((1, 2, 3, 4))})
    assert good.complete and good.state is not None and good.state.items[0].rect == (1.0, 2.0, 3.0, 4.0)
    for broken in ({"matched": True, "complete": True, "state": _state((1, 2, 3))},
                   {"matched": True, "complete": True, "state": _state(viewport=(0, 0, 0, 520))},
                   {"matched": True, "complete": True, "state": _state(("1", 2, 3, 4))},
                   {"matched": True, "complete": True, "state": {"top": "d", "viewport": [0, 0, 1, 1],
                                                                 "items": [{"key": "k", "rect": [0, 0, 1, 1]}]}},
                   {"matched": True, "complete": False}):
        assert WebSecrets.from_reply(broken).complete is False      # malformed or unknown: nobody could tell
