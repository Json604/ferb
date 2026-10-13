"""L13 (owner, 2026-10-14: "no screenshots should be refused and go with redacted stuff"): browser windows, tab
screenshots and recording frames reach the model with every secret field painted over, and through the F5 sanitizer.
An image is withheld only when that masking or sanitizing itself fails. Offline: fake windows, a fake extension
bridge, the extension's own JavaScript in node; no real screen or browser is captured."""

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
from ferb.core.types import Identity, PageElement, PageSnapshot, TabRef
from tests.chromelab import _node, worker
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


# ---------- the gateway: no more browser / recording withholding ----------

def test_a_brave_window_screenshot_keeps_its_pixels_and_goes_through_redaction(daemon: RunningDaemon,
                                                                               monkeypatch: pytest.MonkeyPatch) -> None:
    shots, _ = _fake_mac(daemon, monkeypatch)
    seen = _ocr_log(monkeypatch)
    g = grant_task(daemon, "b", Identity.OWNER, {"screen"}, {"use_app"})
    r = gateway_call(daemon, g, "mac.screenshot", app="Brave")
    assert not r["is_error"] and "image_withheld" not in r["content"], r
    assert base64.b64decode(r["image"]["data"]) == shots.jpeg
    assert seen == [shots.jpeg]                                  # the sanitizer looked before the model did
    assert daemon.run(_evidence(daemon, "b")) == [("screenshot", True)]


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
    r = gateway_call(daemon, g, "mac.screenshot", app="Brave")
    assert not r["is_error"] and "image" not in r
    assert r["content"]["image_withheld"] == "image redaction unavailable"
    assert daemon.run(_evidence(daemon, "f")) == [("screenshot", False)]


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
    reply = {"data": base64.b64encode(page).decode(), "format": "jpeg",
             "mask": {"rects": [[10, 10, 30, 10]], "viewport": [100, 50]}}
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


def test_a_screenshot_in_parts_is_masked_once_whole() -> None:
    page = base64.b64encode(solid_jpeg(200, 100)).decode()
    half = len(page) // 2
    reply = {"parts": 2, "size": len(page), "format": "jpeg", "mask": {"rects": [[0, 0, 50, 25]], "viewport": [100, 50]}}
    d, ref, ops = _tab_driver(reply, [page[:half], page[half:]])
    jpeg = asyncio.run(d.screenshot(ref))
    assert ops == ["screenshot", "screenshot_part", "screenshot_part"]
    assert black(pixel(jpeg, 50, 25)) and white(pixel(jpeg, 150, 75))


@pytest.mark.parametrize("reply", [
    {"withheld": "the page changed documents during capture"},            # the extension couldn't place them
    {"data": "AAAA", "format": "jpeg"},                                  # no rects: nobody said where they are
    {"data": base64.b64encode(b"\xff\xd8\xff not a picture").decode(), "format": "jpeg",
     "mask": {"rects": [[0, 0, 5, 5]], "viewport": [100, 50]}},          # can't be painted
])
def test_a_tab_capture_is_withheld_only_when_masking_fails(reply: dict[str, Any]) -> None:
    from ferb.core.errors import Withheld
    d, ref, _ = _tab_driver(reply)
    with pytest.raises(Withheld, match="withheld"):
        asyncio.run(d.screenshot(ref))
    _, capture = d.recording_source(ref, "task")
    with pytest.raises(Withheld, match="withheld"):
        asyncio.run(capture())


def test_a_browser_recording_frame_is_masked() -> None:
    page = solid_jpeg(200, 100)
    reply = {"data": base64.b64encode(page).decode(), "mask": {"rects": [[50, 0, 50, 50]], "viewport": [100, 50]}}
    d, ref, ops = _tab_driver(reply)
    _, capture = d.recording_source(ref, "task")
    frame = asyncio.run(capture())
    assert ops == ["record_frame"] and black(pixel(frame, 150, 50)) and white(pixel(frame, 50, 50))


# ---------- native window captures: AX secret fields and the extension's rects ----------

def _brave(monkeypatch: pytest.MonkeyPatch, *answers: Any) -> tuple[Any, Any, Any, list[Any]]:
    """Brave's window at BOUNDS, captured at 2x, its page drawn under an 80-point toolbar (the AX web area); the
    extension answers for that window with each of `answers` in turn (then None: no profile could tell)."""
    d, ax, shots, *_ = vision(monkeypatch)
    shots.jpeg = solid_jpeg(*IMG)
    ax.tree = AxTree(title="Sign in", secret_rects=[], web_areas=[(100.0, 130.0, 800.0, 520.0)])
    asked: list[Any] = []
    queue = list(answers)

    async def secrets(browser: str, bounds: Any) -> Any:
        asked.append((browser, bounds))
        return queue.pop(0) if queue else None
    d.web_secrets = secrets
    return d, ax, shots, asked


@pytest.mark.parametrize("zoom", [1.0, 1.25])
def test_a_browser_window_capture_masks_the_fields_the_extension_marks_secret(monkeypatch: pytest.MonkeyPatch,
                                                                             zoom: float) -> None:
    from ferb.act.mac.driver import WebSecrets
    field = (20 / zoom, 40 / zoom, 100 / zoom, 20 / zoom)               # CSS px: (120, 170) points on screen
    page = WebSecrets(rects=(field,), viewport=(800 / zoom, 520 / zoom), complete=True)
    d, _, shots, asked = _brave(monkeypatch, page, page)
    _, shot, jpeg = asyncio.run(d.screenshot("Brave"))
    assert asked == [("brave", BOUNDS)] * 2 and shots.captured == [120]
    assert black(pixel(jpeg, 100, 255))                          # (40..240, 240..280) image px
    assert white(pixel(jpeg, 100, 300)) and white(pixel(jpeg, 20, 20)) and white(pixel(jpeg, 1500, 1100))


def test_a_shown_password_in_a_native_browser_capture_is_masked(monkeypatch: pytest.MonkeyPatch) -> None:
    """The extension can't tell (no paired profile): the AX inventory's password element (an AXTextField its
    metadata names a password field, as "Show password" leaves it) is painted over."""
    d, ax, shots, _ = _brave(monkeypatch)
    ax.tree.secret_rects = [(150.0, 200.0, 100.0, 20.0)]
    ax.secure = [True, True]
    _, _, jpeg = asyncio.run(d.screenshot("Brave"))
    assert black(pixel(jpeg, 150, 310)) and white(pixel(jpeg, 150, 360)) and white(pixel(jpeg, 20, 20))


def test_a_secret_field_appearing_during_capture_is_masked_not_refused(monkeypatch: pytest.MonkeyPatch) -> None:
    from ferb.act.mac.driver import WebSecrets
    late = WebSecrets(rects=((400.0, 300.0, 100.0, 50.0),), viewport=(800.0, 520.0), complete=True)
    d, ax, shots, _ = _brave(monkeypatch, WebSecrets((), (800.0, 520.0), True), late)
    ax.secure = [False, True]

    def appears() -> None:
        ax.tree.secret_rects = [(150.0, 200.0, 100.0, 20.0)]
    shots.on_capture = appears
    _, _, jpeg = asyncio.run(d.screenshot("Brave"))
    assert black(pixel(jpeg, 150, 310))                          # the AX field, from the walk after the capture
    assert black(pixel(jpeg, 900, 800))                          # the page's field: (500, 430) pt -> (800, 760) px
    assert white(pixel(jpeg, 20, 20))


def test_a_deep_browser_tree_is_covered_by_the_extensions_page_inventory(monkeypatch: pytest.MonkeyPatch) -> None:
    from ferb.act.mac.driver import WebSecrets
    from ferb.core.errors import Withheld
    page = WebSecrets(rects=(), viewport=(800.0, 520.0), complete=True)
    d, ax, shots, _ = _brave(monkeypatch, page, page)
    ax.tree.truncated_by = "depth"                               # the page's DOM was deeper than the AX walk
    asyncio.run(d.screenshot("Brave"))
    assert shots.captured == [120]
    unknown, ax2, shots2, _ = _brave(monkeypatch, WebSecrets((), None, False))
    ax2.tree.truncated_by = "depth"                              # nobody inventoried the page: can't be masked
    with pytest.raises(Withheld, match="inventory"):
        asyncio.run(unknown.screenshot("Brave"))
    assert shots2.captured == []


def test_page_fields_that_cant_be_placed_on_the_window_withhold_the_frame(monkeypatch: pytest.MonkeyPatch) -> None:
    from ferb.act.mac.driver import WebSecrets
    from ferb.core.errors import Withheld
    page = WebSecrets(rects=((20.0, 40.0, 100.0, 20.0),), viewport=(800.0, 520.0), complete=True)
    d, ax, shots, _ = _brave(monkeypatch, page)
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
    from ferb.core.errors import Withheld
    ffmpeg = shutil.which("ffmpeg")
    if not ffmpeg:
        pytest.skip("recording requires ffmpeg")
    frame = subprocess.check_output([ffmpeg, "-v", "error", "-f", "lavfi", "-i", "color=red:s=320x180",
                                     "-frames:v", "1", "-f", "image2pipe", "-c:v", "mjpeg", "pipe:1"])
    unmaskable = [False]

    async def capture() -> bytes:
        if unmaskable[0]:
            raise Withheld("pixels withheld: the window's secret-field inventory is incomplete")
        return frame

    async def check() -> None:
        clock = [0.0]
        m = RecordingManager(now=lambda: clock[0], sanitize=lambda image: image)
        r = await m.start("task", {"kind": "window", "window_id": 110}, tmp_path / "v.mp4", capture, fps=1)
        unmaskable[0] = True
        clock[0] = 1
        await m.sample(r["id"], "task")
        unmaskable[0] = False
        clock[0] = 2
        await m.sample(r["id"], "task")
        stopped = await m.stop(r["id"], "task")
        assert stopped["frames"] == 2 and stopped["frames_withheld"] == 1 and stopped["error"] is None
        assert "inventory" in stopped["withheld_reason"] and stopped["capture_complete"]
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
            El("AXWebArea", {"AXPosition": (0.0, 300.0), "AXSize": (400.0, 200.0)}),   # an iframe's page
        ])])])
    t = walk(win)
    assert t.secure is True
    assert t.secret_rects == [(10.0, 140.0, 30.0, 10.0), (10.0, 180.0, 30.0, 10.0)]
    assert ax.outermost(t.web_areas) == [(0.0, 80.0, 800.0, 520.0)]
    shown = next(n for n in t.nodes if n.label == "Enter your password")
    assert not shown.secret                                      # masking only; Task 12 redacts its value


# ---------- the extension: rects, never a refusal ----------

def test_the_content_script_reports_secret_field_rects_and_where_child_frames_sit() -> None:
    source = Path("extensions/chromium/content.js").read_text()
    capture = source[source.index("  function captureState("):source.index("  function snapshot(")]
    script = r'''
const chrome = {runtime: {getFrameId: el => { if (el.fid === 'throw') throw Error('no frame'); return el.fid; }}};
const documentId = 'doc-1', innerWidth = 1000, innerHeight = 800;
const getComputedStyle = el => ({paddingLeft: el.pad || '0px', paddingTop: el.pad || '0px'});
const isSecret = el => !!el.secret, maskedEditable = el => false;
const mk = o => Object.assign({tagName: 'INPUT', clientLeft: o.border || 0, clientTop: o.border || 0,
  offsetWidth: o.r[2], offsetHeight: o.r[3], checkVisibility: () => o.shown !== false,
  getBoundingClientRect: () => ({x: o.r[0], y: o.r[1], width: o.r[2], height: o.r[3]})}, o);
const inner = mk({secret: true, r: [30, 40, 20, 10]});
const host = mk({tagName: 'DIV', r: [0, 0, 1, 1], shadowRoot: {querySelectorAll: () => [inner]}});
const nodes = [mk({secret: true, r: [10, 20, 100, 30]}), mk({secret: true, r: [0, 0, 100, 30], shown: false}),
  mk({secret: true, r: [5, 5, 0, 0]}), mk({secret: false, r: [50, 50, 100, 30]}),
  mk({tagName: 'IFRAME', r: [200, 300, 400, 200], border: 2, pad: '1px', fid: 5}),
  mk({tagName: 'IFRAME', r: [700, 100, 50, 50], fid: -1}), mk({tagName: 'FRAME', r: [0, 0, 10, 10], fid: 'throw'}),
  mk({tagName: 'IFRAME', r: [0, 600, 100, 100], fid: 6, offsetWidth: 50}), host];
const document = {querySelectorAll: () => nodes};
''' + capture + "\nconsole.log(JSON.stringify(captureState()));\n"
    out = json.loads(subprocess.run([_node(), "-e", script], capture_output=True, text=True, check=True).stdout)
    assert out["documentId"] == "doc-1" and out["viewport"] == [1000, 800]
    assert out["rects"] == [[10, 20, 100, 30], [30, 40, 20, 10]]          # shown, sized secret fields; shadow too
    assert out["frames"] == [
        {"frame_id": 5, "rect": [200, 300, 400, 200], "inner": [203, 303], "scaled": False},
        {"frame_id": -1, "rect": [700, 100, 50, 50], "inner": [700, 100], "scaled": False},
        {"frame_id": -1, "rect": [0, 0, 10, 10], "inner": [0, 0], "scaled": False},
        {"frame_id": 6, "rect": [0, 600, 100, 100], "inner": [0, 600], "scaled": True}]


def test_the_background_places_every_frames_fields_in_the_top_viewport() -> None:
    out = worker('''(async () => {
      inject = async () => {};
      const rows = [
        {frameId: 0, result: {documentId: 'top', viewport: [1000, 800], rects: [[10, 20, 100, 30]], frames: [
          {frame_id: 5, rect: [200, 300, 400, 200], inner: [202, 302], scaled: false},
          {frame_id: -1, rect: [700, 100, 50, 50], inner: [700, 100], scaled: false},
          {frame_id: 6, rect: [0, 600, 100, 100], inner: [0, 600], scaled: true},
          {frame_id: 8, rect: [900, 900, 50, 50], inner: [900, 900], scaled: false}]}},
        {frameId: 5, result: {documentId: 'child', viewport: [396, 196], rects: [[10, 10, 50, 20], [390, 190, 40, 40]],
          frames: []}},
        {frameId: 6, result: {documentId: 'scaled', viewport: [50, 50], rects: [[1, 1, 5, 5]],
          frames: [{frame_id: 9, rect: [0, 0, 10, 10], inner: [0, 0], scaled: false}]}},
        {frameId: 8, result: {documentId: 'below', viewport: [50, 50], rects: [[1, 1, 5, 5]], frames: []}},
        {frameId: 9, result: {documentId: 'in-scaled', viewport: [10, 10], rects: [[1, 1, 2, 2]], frames: []}}];
      chrome.scripting = {executeScript: async () => rows};
      const placed = await maskState(9);
      rows.push({frameId: 7, result: {documentId: 'lost', viewport: [10, 10], rects: [[0, 0, 5, 5]], frames: []}});
      const lost = await maskState(9);
      chrome.scripting = {executeScript: async () => rows.filter(r => r.frameId !== 0)};
      const noTop = await maskState(9);
      chrome.scripting = {executeScript: async () => { throw Error('Cannot access contents of the page'); }};
      const unreadable = await maskState(9);
      return {placed, lost, noTop, unreadable};
    })()''')
    placed = out["placed"]
    assert placed["viewport"] == [1000, 800]
    assert sorted(placed["rects"]) == sorted([[10, 20, 100, 30], [212, 312, 50, 20], [592, 492, 8, 8],
                                              [700, 100, 50, 50], [0, 600, 100, 100]])   # frame 8 is off the page
    assert [d[0] for d in placed["documents"]] == [0, 5, 6, 8, 9]
    assert "withheld" in out["lost"] and "withheld" in out["noTop"] and "withheld" in out["unreadable"]


@pytest.mark.parametrize("case", ["secret", "navigation", "unreadable"])
@pytest.mark.parametrize("op", ["screenshot", "record_frame"])
def test_extension_captures_carry_masks_and_are_withheld_only_when_masking_fails(op: str, case: str) -> None:
    out = worker('''(async () => {
      attach = async id => tabState(id); let reads = 0, captures = 0;
      maskState = async () => {
        reads++;
        if (CASE === 'unreadable') return {withheld: "the page's top frame can't be read"};
        return {documents: [[0, CASE === 'navigation' && reads > 1 ? 'new' : 'doc']], viewport: [100, 50],
                rects: reads > 1 ? [[5, 5, 10, 10]] : [[1, 1, 2, 2]]};
      };
      cdp = async () => { captures++; return {data: 'jpeg-bytes'}; };
      const st = tabState(19); st.shot = 'previous'; st.visual = {snap: 7};
      const out = await commands[OP]({tab_id: 19});
      return {out, captures};
    })()'''.replace("CASE", json.dumps(case)).replace("OP", json.dumps(op)))
    if case == "secret":
        assert out["out"]["data"] == "jpeg-bytes"
        assert out["out"]["mask"] == {"rects": [[1, 1, 2, 2], [5, 5, 10, 10]], "viewport": [100, 50]}
    else:
        assert "data" not in out["out"] and out["out"]["withheld"]
    assert out["captures"] == (0 if case == "unreadable" else 1)


def test_the_extension_reports_a_windows_secret_rects_for_native_captures() -> None:
    out = worker('''(async () => {
      chrome.windows = {getAll: async () => [{id: 1, left: 10, top: 20, width: 800, height: 600},
        {id: 2, left: 0, top: 0, width: 500, height: 400}, {id: 3, left: 0, top: 0, width: 500, height: 400}]};
      let queried = [];
      chrome.tabs.query = async ({windowId}) => { queried.push(windowId); return [{id: windowId * 10}]; };
      maskState = async tab => tab === 10 ? {documents: [[0, 'd']], rects: [[1, 2, 3, 4]], viewport: [800, 520]}
                                          : {withheld: "the page's top frame can't be read"};
      const page = await commands.window_secrets({bounds: [10, 20, 800, 600]});
      const ambiguous = await commands.window_secrets({bounds: [0, 0, 500, 400]});
      const missing = await commands.window_secrets({bounds: [1, 1, 5, 5]});
      chrome.windows = {getAll: async () => [{id: 4, left: 0, top: 0, width: 500, height: 400}]};
      const unreadable = await commands.window_secrets({bounds: [0, 0, 500, 400]});
      return {page, ambiguous, missing, unreadable, queried};
    })()''')
    assert out["page"] == {"matched": True, "complete": True, "rects": [[1, 2, 3, 4]], "viewport": [800, 520]}
    assert out["ambiguous"] == {"matched": False, "reason": "ambiguous_window"}
    assert out["missing"] == {"matched": False, "reason": "window_not_found"}
    assert out["unreadable"]["matched"] and out["unreadable"]["complete"] is False
    assert out["queried"] == [1, 4]                              # an ambiguous window's tabs are never looked at


def test_a_windows_answer_is_read_strictly() -> None:
    from ferb.act.mac.driver import WebSecrets
    assert WebSecrets.from_reply({"matched": True, "complete": True, "rects": [[1, 2, 3, 4]],
                                  "viewport": [800, 520]}) == WebSecrets(((1.0, 2.0, 3.0, 4.0),), (800.0, 520.0), True)
    for broken in ({"matched": True, "complete": True, "rects": [[1, 2, 3]], "viewport": [800, 520]},
                   {"matched": True, "complete": True, "rects": [], "viewport": [0, 520]},
                   {"matched": True, "complete": True, "rects": [["1", 2, 3, 4]], "viewport": [800, 520]},
                   {"matched": True, "complete": False}):
        assert WebSecrets.from_reply(broken).complete is False      # malformed or unknown: nobody could tell
