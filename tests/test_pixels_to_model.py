"""L13 (owner, 2026-10-14: "no screenshots should be refused and go with redacted stuff"): browser windows, tab
screenshots and recording frames reach the model with every secret field painted over, and through the F5 sanitizer.
An image is withheld only when that masking or sanitizing itself fails. Offline: fake windows, a fake extension
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
    frame = {"key": "frame|doc|t1", "rect": [0, 30, 40, 20], "paint": False, "child": "kid"}
    field = {"key": "field|kid|f", "rect": [5, 35, 10, 5], "paint": True}
    rects, viewport = _merge(_state((1, 1, 5, 5), frames=[frame, field]), _state((1, 1, 5, 5), frames=[frame, field]))
    assert sorted(rects) == [(1.0, 1.0, 5.0, 5.0), (5.0, 35.0, 10.0, 5.0)] and viewport == (0.0, 0.0, 100.0, 50.0)


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


def test_a_frame_linked_only_after_the_first_look_is_covered_by_its_whole_box() -> None:
    """The link handshake landed between the two looks: before, the frame is painted whole; after, its fields."""
    whole = {"key": "frame|doc|t1", "rect": [0, 30, 40, 20], "paint": True}
    linked = {"key": "frame|doc|t1", "rect": [0, 30, 40, 20], "paint": False, "child": "kid"}
    field = {"key": "field|kid|f", "rect": [5, 35, 10, 5], "paint": True}
    rects, _ = _merge(_state(frames=[whole]), _state(frames=[linked, field]))
    assert (0.0, 30.0, 40.0, 20.0) in rects


def test_an_iframe_reloading_during_capture_is_painted_whole_not_withheld() -> None:
    old = [{"key": "frame|doc|t1", "rect": [0, 30, 40, 20], "paint": False, "child": "old"},
           {"key": "field|old|f", "rect": [5, 35, 10, 5], "paint": True}]
    new = [{"key": "frame|doc|t1", "rect": [0, 30, 40, 20], "paint": False, "child": "new"},
           {"key": "field|new|g", "rect": [20, 40, 10, 5], "paint": True}]
    rects, _ = _merge(_state(frames=old), _state(frames=new))
    assert (0.0, 30.0, 40.0, 20.0) in rects


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


def test_a_browser_page_nobody_read_is_withheld(monkeypatch: pytest.MonkeyPatch) -> None:
    d, ax, shots, _ = _brave(monkeypatch)
    ax.tree.empty_web_areas = [(100.0, 130.0, 800.0, 520.0)]    # Accessibility shows the page area, empty
    with pytest.raises(Withheld, match="neither"):
        asyncio.run(d.screenshot("Brave"))
    assert shots.captured == []


def test_a_secret_field_appearing_during_capture_withholds_it(monkeypatch: pytest.MonkeyPatch) -> None:
    d, ax, shots, _ = _brave(monkeypatch, _page(), _page())
    ax.secure = [False, True]

    def appears() -> None:
        ax.tree.secret_rects = [(150.0, 200.0, 100.0, 20.0)]
    shots.on_capture = appears
    with pytest.raises(Withheld, match="during capture"):
        asyncio.run(d.screenshot("Brave"))


def test_a_field_moving_during_capture_withholds_it(monkeypatch: pytest.MonkeyPatch) -> None:
    d, ax, shots, *_ = vision(monkeypatch)
    ax.tree = AxTree(secret_rects=[(150.0, 200.0, 100.0, 20.0)])
    ax.secure = [True, True]

    def moves() -> None:
        ax.tree.secret_rects = [(150.0, 210.0, 100.0, 20.0)]
    shots.on_capture = moves
    with pytest.raises(Withheld, match="during capture"):
        asyncio.run(d.screenshot("Notes"))


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


def test_page_fields_that_cant_be_placed_on_the_window_withhold_the_frame(monkeypatch: pytest.MonkeyPatch) -> None:
    d, ax, shots, _ = _brave(monkeypatch, _page((20, 40, 100, 20)))
    ax.tree.web_areas = []                                       # Accessibility shows no page area to map onto
    with pytest.raises(Withheld, match="withheld"):
        asyncio.run(d.screenshot("Brave"))
    assert shots.captured == []


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
    getComputedStyle: el => ({paddingLeft: "0px", paddingTop: "0px", transform: el.transform || "none",
                              rotate: el.rotate || "none", scale: el.scale || "none"}),
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
      return await maskState(9);
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
    assert _painted(state) == sorted([[10, 20, 100, 30], [30, 40, 20, 10], [205, 305, 40, 10], [601, 502, 30, 10],
                                      [700, 100, 50, 50]])
    assert state["top"] == page["documentId"] and state["viewport"] == [0, 0, 1000, 800]


def _frames(*heard: Any, rects: Any = None) -> list[dict[str, Any]]:
    """Top-frame rows: frame elements E0, E1, ... at x = 0, 200, ... that heard these tokens, and their children."""
    top = {"documentId": "top", "own": None, "viewport": [0, 0, 1400, 800], "rects": [], "frames": [
        {"id": f"E{i}", "heard": h, "rect": [200 * i, 0, 100, 100], "inner": [200 * i, 0], "scaled": False}
        for i, h in enumerate(heard)]}
    kids = [{"documentId": doc, "own": own, "viewport": [0, 0, 100, 100], "rects": [{"id": "f", "rect": r}],
             "frames": []} for doc, own, r in (rects or [])]
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
                          ("F", "tF", [3, 3, 5, 5])])
    rows[0]["result"]["frames"][4]["scaled"] = True
    state = _mask_state(rows)
    # E0/E1 heard the same token: both whole; E2 heard nothing: whole; E3 heard one token twice: whole; E4 transformed;
    # E5 heard exactly one token nobody else heard, which exactly one frame owns: F is placed, its field painted
    assert _painted(state) == sorted([[0, 0, 100, 100], [200, 0, 100, 100], [400, 0, 100, 100], [600, 0, 100, 100],
                                      [800, 0, 100, 100], [1003, 3, 5, 5]])
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
      const noTop = await maskState(9);
      chrome.scripting = {executeScript: async () => { throw Error('Cannot access contents of the page'); }};
      return {noTop, unreadable: await maskState(9)};
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
        assert _painted(_mask_state(rows)) == [[205, 305, 40, 10]]       # placed exactly, no spurious black box
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
      const [one, two] = await Promise.all([maskState(4), maskState(4)]);
      return {order, one, two};
    })()''')
    rounds = [step.split(":")[1] for step in out["order"]]
    assert len(out["order"]) == 6 and rounds[:3] == [rounds[0]] * 3 and rounds[3:] == [rounds[3]] * 3
    assert rounds[0] != rounds[3] and "withheld" not in out["one"] and "withheld" not in out["two"]


def test_a_message_without_a_source_is_nobodys_announcement() -> None:
    out = _content(r'''
const page = frameWindow({viewport: [1000, 800], nodes: [el("IFRAME", [700, 100, 50, 50], {contentWindow: null})]});
page.__ferb.listenForFrames("round-1");
page.deliver({frameLinkRound: "round-1", frameLinkToken: "from-nowhere"}, null, true);
for (const run of queue.splice(0)) run();
console.log(JSON.stringify(page.__ferb.captureState("round-1")));
''')
    assert out["frames"][0]["heard"] == []                        # not the empty host's: it has no window to send


@pytest.mark.parametrize("transform, placed", [
    ({"transform": "matrix(-1, 0, 0, 1, 0, 0)"}, False),                       # flipped: same size, mirrored
    ({"transform": "matrix(-1, 0, 0, -1, 0, 0)"}, False),                      # turned 180 degrees
    ({"rotate": "90deg"}, False),                                              # a square turned 90 degrees
    ({"scale": "-1 1"}, False),
    ({"transform": "matrix3d(1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 0, 30, 40, 0, 1)"}, True),   # moved only: placeable
    ({"transform": "matrix(1, 0, 0, 1, 10, 20)"}, True),
])
@pytest.mark.parametrize("on", ["host", "ancestor"])
def test_a_host_transformed_on_itself_or_an_ancestor_is_painted_whole(transform: Any, placed: bool, on: str) -> None:
    """A size-preserving transform (a flip, a half turn, a quarter turn of a square) would put the field's mask where
    the field isn't; only a pure move keeps the offset right."""
    style = json.dumps(transform)
    out = _content(r'''
const topNodes = [];
const page = frameWindow({viewport: [1000, 800], nodes: topNodes});
const a = child(page, [el("INPUT", [5, 5, 40, 10], {type: "password"})], [200, 200]);
const box = Object.assign({tagName: "DIV", parentElement: null}, __ON__ === "ancestor" ? __STYLE__ : {});
topNodes.push(el("IFRAME", [200, 300, 200, 200], Object.assign({contentWindow: a.proxy, parentElement: box},
                                                              __ON__ === "host" ? __STYLE__ : {})));
console.log(JSON.stringify(capture([page, a.kid])));
'''.replace("__STYLE__", style).replace("__ON__", json.dumps(on)))
    assert out[0]["result"]["frames"][0]["scaled"] is (not placed)
    painted = _painted(_mask_state(out))
    assert painted == ([[205, 305, 40, 10]] if placed else [[200, 300, 200, 200]])


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
