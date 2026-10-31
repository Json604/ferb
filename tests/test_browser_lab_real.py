"""Rework phase 3, browser, REAL (pytest -m real): a throwaway Chrome (its own --user-data-dir under a temp folder,
never the owner's browsers or profiles) with the unpacked FERB extension, a test daemon with its own home, and a
local page. Nothing but the lab Chrome is touched; ~/.ferb stays out.

Proves on this Mac that FERB's tab opens in a minimized window and stays inactive;
real (isTrusted) typing, clicks and keys land there; the page and a container scroll by wheel; a confirm() is
answered by the extension; a file lands in a file input; a screenshot renders; and the frontmost app never changed
while all of it happened (NSWorkspace polled every 50 ms, as in the probes).

    uv run pytest -m real tests/test_browser_lab_real.py -s
"""

import asyncio
import http.server
import json
import threading
import time
from pathlib import Path
from typing import Any, Iterator

import pytest

from ferb.core.types import BrowserAction
from tests.chromelab import ChromeLab, Frontmost, lab  # noqa: F401
from tests.conftest import RunningDaemon

pytestmark = pytest.mark.real
EXT_DIR = Path(__file__).parents[1] / "extensions" / "chromium"

PAGE = """<!doctype html><title>FERB lab</title>
<style>body{margin:0;padding:20px;font:16px sans-serif} #box{height:200px;overflow:auto;border:1px solid #000;width:300px}
#box div{height:50px} .tall{height:2500px}</style>
<form onsubmit="return false">
<input id=user name=user placeholder=User oninput="document.getElementById('o1').textContent='input trusted='+event.isTrusted+' value='+this.value">
<button type=button id=b onclick="document.getElementById('o0').textContent='clicked trusted='+event.isTrusted">Go</button>
<input id=t placeholder=Trusted onkeydown="document.getElementById('o2').textContent='key '+event.key+' trusted='+event.isTrusted">
<button type=button onclick="document.getElementById('o3').textContent='confirm='+confirm('Sure?')">Ask</button>
<input type=file id=f aria-label=Attach onchange="document.getElementById('o4').textContent='files='+this.files.length">
</form>
<p id=o0>not clicked</p><p id=o1>-</p><p id=o2>-</p><p id=o3>-</p><p id=o4>-</p>
<div id=custom style="width:90px;height:30px" onclick="this.textContent='Added '+event.isTrusted">ADD</div>
<div id=box aria-label=messages role=list>%s</div>
<div class=tall>tall filler</div><a href="#end" id=end>End link</a>
""" % "".join(f"<div role=listitem>message {i}</div>" for i in range(1, 21))

POINTER_PAGE = """<!doctype html><title>Pointer lab</title><style>body{margin:0}canvas{position:absolute;left:40px;top:40px}</style>
<canvas width=300 height=200 aria-label="Drawing surface"></canvas><p id=result style="position:absolute;top:260px">Ready</p>
<input type=range aria-label=Volume min=0 max=100 value=0 style="position:absolute;left:40px;top:320px;width:300px;margin:0">
<script>
const canvas=document.querySelector('canvas'); window.events=[];
for(const type of ['pointerdown','pointermove','pointerup','click','dblclick','contextmenu']) canvas.addEventListener(type,e=>{
  e.preventDefault(); events.push({type,x:Math.round(e.offsetX),y:Math.round(e.offsetY),buttons:e.buttons,trusted:e.isTrusted});
  if(type==='pointerup') document.querySelector('#result').textContent='Released at '+e.offsetX+','+e.offsetY;
});
</script>"""


@pytest.fixture(scope="module")
def site() -> Iterator[str]:
    class H(http.server.BaseHTTPRequestHandler):
        def do_GET(self) -> None:  # noqa: N802
            self.send_response(200)
            self.send_header("content-type", "text/html")
            self.end_headers()
            if self.path == "/pointer":
                page = POINTER_PAGE
            elif self.path == "/pointer-frame":
                page = f'<iframe style="position:absolute;left:200px;top:100px;width:500px;height:500px;border:0" src="http://localhost:{self.server.server_port}/pointer"></iframe>'
            elif self.path == "/frames":
                page = ('<title>Frames</title><iframe style="margin:40px" src="/inside"></iframe>'
                        f'<iframe style="margin:100px" src="http://localhost:{self.server.server_port}/inside"></iframe>')
            elif self.path == "/inside":
                page = ('<button onclick="this.textContent=event.isTrusted ? \'Reached\' : \'Untrusted\'">Frame action</button>'
                        f'<iframe src="http://127.0.0.1:{self.server.server_port}/deep"></iframe>')
            elif self.path == "/deep":
                page = '<button onclick="this.textContent=event.isTrusted ? \'Deep reached\' : \'Untrusted\'">Deep action</button>'
            else:
                page = PAGE
            self.wfile.write(page.encode())

        def log_message(self, *a: Any) -> None:
            pass
    srv = http.server.ThreadingHTTPServer(("127.0.0.1", 0), H)
    threading.Thread(target=srv.serve_forever, daemon=True).start()
    yield f"http://127.0.0.1:{srv.server_port}"
    srv.shutdown()


def test_ax_targets_in_same_process_and_cross_site_frames(lab: Any, site: str) -> None:
    d, chrome, _ = lab
    router = d.daemon.router

    async def go() -> None:
        tab = await router.open_for(f"{site}/frames", "frames", "Lab")
        try:
            snap = await router.snapshot(tab)
            assert "Accessibility view incomplete" not in snap.text, snap.text
            targets = [e for e in snap.elements if e.label == "Frame action"]
            assert len(targets) == 2
            assert len({e.frame_id for e in targets}) == 2
            for target in targets:
                r = await router.act(router.element_ref(tab, target.id), BrowserAction("click"), "")
                assert r.ok, r.detail
            snap = await router.snapshot(tab)
            assert len([e for e in snap.elements if e.label == "Reached"]) == 2
            deep = [e for e in snap.elements if e.label == "Deep action"]
            assert len(deep) == 2
            for target in deep:
                result = await router.act(router.element_ref(tab, target.id), BrowserAction("click"), "")
                assert result.ok, result.detail
            assert len([e for e in (await router.snapshot(tab)).elements if e.label == "Deep reached"]) == 2
        finally:
            await router.close(tab)
    d.run(go(), timeout=90)


def test_visual_target_becomes_a_normal_policy_checked_element(lab: Any, site: str) -> None:
    d, chrome, _ = lab
    router = d.daemon.router

    async def go() -> None:
        tab = await router.open_for(f"{site}/page", "visual", "Lab")
        try:
            shot, jpeg, width, height = await router.visual_snapshot(tab)
            assert jpeg[:3] == b"\xff\xd8\xff" and width > 0 and height > 0
            # Position comes from the test fixture's rendered geometry, as a model would read its screenshot.
            t = router._tabs[tab.tab_id]
            coords = chrome.evaluate(chrome.sw, f"chrome.scripting.executeScript({{target: {{tabId: {t.chrome_id}}}, "
                "func: () => {const r = document.getElementById('custom').getBoundingClientRect(); return [r.x+10, r.y+10];}}).then(r => r[0].result)")
            target = await router.point(tab, shot, *coords)
            assert target.label == "ADD", target.label
            result = await router.act(router.element_ref(tab, target.id), BrowserAction("click"), "")
            assert result.ok, result.detail
            assert "Added true" in (await router.snapshot(tab)).text
        finally:
            await router.close(tab)
    d.run(go(), timeout=90)


@pytest.mark.parametrize("path,offset", [("/pointer", (0, 0)), ("/pointer-frame", (200, 100))])
def test_exact_pointer_canvas_and_drag_in_background(lab: Any, site: str, path, offset) -> None:
    d, chrome, front = lab
    router = d.daemon.router
    before = len(front.changes)

    async def go():
        tab = await router.open_for(site + path, "pointer", "Lab")
        try:
            for kind in ("click", "dblclick", "rightclick", "hover", "drag"):
                shot, _, width, height = await router.visual_snapshot(tab)
                t = router._tabs[tab.tab_id]
                viewport = chrome.evaluate(chrome.sw, f"chrome.scripting.executeScript({{target:{{tabId:{t.chrome_id}}},func:()=>[innerWidth,innerHeight]}}).then(r=>r[0].result)")
                sx, sy = width / viewport[0], height / viewport[1]
                start = await router.point(tab, shot, (offset[0] + 57) * sx, (offset[1] + 63) * sy)
                end = await router.point(tab, shot, (offset[0] + 251) * sx, (offset[1] + 173) * sy) if kind == "drag" else None
                result = await router.act(router.element_ref(tab, start.id),
                                          BrowserAction(kind, end_element=end.id if end else None), "")
                assert result.ok, result.detail
                from ferb.core.errors import StaleRef
                with pytest.raises(StaleRef):
                    await router.act(router.element_ref(tab, start.id), BrowserAction("click"), "")
            t = router._tabs[tab.tab_id]
            frames = chrome.evaluate(chrome.sw, f"chrome.scripting.executeScript({{target:{{tabId:{t.chrome_id},allFrames:true}},world:'MAIN',func:()=>window.events||[]}})")
            events = [e for f in frames for e in f["result"]]
            assert any(e["type"] == "click" and (e["x"], e["y"]) == (17, 23) for e in events), json.dumps(events)
            assert any(e["type"] == "dblclick" for e in events)
            assert any(e["type"] == "contextmenu" for e in events)
            assert any(e["type"] == "pointermove" and e["buttons"] == 1 for e in events)
            assert (events[-1]["x"], events[-1]["y"]) == (211, 133)
            assert all(e["trusted"] for e in events)
        finally:
            await router.close(tab)
    d.run(go(), timeout=90)
    assert len(front.changes) == before


def test_pointer_refuses_dom_changes_since_screenshot(lab: Any, site: str) -> None:
    d, chrome, _ = lab
    router = d.daemon.router

    async def go():
        tab = await router.open_for(site + "/pointer", "stale-pointer", "Lab")
        try:
            shot, _, _, _ = await router.visual_snapshot(tab)
            point = await router.point(tab, shot, 57, 63)
            t = router._tabs[tab.tab_id]
            chrome.evaluate(chrome.sw, f"chrome.scripting.executeScript({{target:{{tabId:{t.chrome_id}}},func:()=>document.querySelector('canvas').style.left='80px'}})")
            result = await router.act(router.element_ref(tab, point.id), BrowserAction("click"), "")
            assert not result.ok and "visual_stale" in result.detail
        finally:
            await router.close(tab)
    d.run(go(), timeout=90)


def test_pointer_drag_operates_a_real_slider(lab: Any, site: str) -> None:
    d, chrome, _ = lab
    router = d.daemon.router
    async def go():
        tab = await router.open_for(site + "/pointer", "slider", "Lab")
        try:
            shot, _, width, height = await router.visual_snapshot(tab)
            t = router._tabs[tab.tab_id]
            dims = chrome.evaluate(chrome.sw, f"chrome.scripting.executeScript({{target:{{tabId:{t.chrome_id}}},func:()=>{{const r=document.querySelector('input').getBoundingClientRect();return [innerWidth,innerHeight,r.x,r.y,r.width,r.height]}}}}).then(r=>r[0].result)")
            vw, vh, x, y, w, h = dims
            start = await router.point(tab, shot, (x + 8) * width / vw, (y + h / 2) * height / vh)
            end = await router.point(tab, shot, (x + w - 8) * width / vw, (y + h / 2) * height / vh)
            r = await router.act(router.element_ref(tab, start.id), BrowserAction("drag", end_element=end.id), "")
            assert r.ok, r.detail
            value = chrome.evaluate(chrome.sw, f"chrome.scripting.executeScript({{target:{{tabId:{t.chrome_id}}},func:()=>document.querySelector('input').value}}).then(r=>r[0].result)")
            assert int(value) >= 95
        finally:
            await router.close(tab)
    d.run(go(), timeout=90)


def test_everything_happens_in_a_minimized_window_and_the_owner_is_never_moved(
        lab: tuple[RunningDaemon, ChromeLab, Frontmost], site: str, tmp_path: Path) -> None:
    d, chrome, front = lab
    router = d.daemon.router
    assert router is not None
    upload = tmp_path / "note.txt"
    upload.write_text("hello")
    out: dict[str, Any] = {}

    def el(s: Any, **match: Any) -> Any:
        return next(e for e in s.elements if all(getattr(e, k) == v for k, v in match.items()))

    async def go() -> None:
        out["on_screen_before"] = chrome.on_screen()
        tab = await router.open_for(f"{site}/page", "t", "Lab")
        t = router._tabs[tab.tab_id]
        out["cdp_window_after_open"] = chrome.window_state("/page")
        out["on_screen_after_open"] = chrome.on_screen()
        s = await router.snapshot(tab)
        out["elements"] = [(e.role, e.label, e.in_view, e.offscreen, e.container) for e in s.elements]
        out["scroll"], out["scrollers"], out["text"] = s.scroll, s.scrollers, s.text
        user, go_btn, trusted, ask = (el(s, label="User"), el(s, label="Go"), el(s, label="Trusted"), el(s, label="Ask"))
        out["type"] = await router.act(router.element_ref(tab, user.id), BrowserAction("type", "ada"), "")
        out["click"] = await router.act(router.element_ref(tab, go_btn.id), BrowserAction("click"), "")
        out["press"] = await router.act(router.element_ref(tab, trusted.id), BrowserAction("press", "Enter"), "")
        out["dismissed"] = await router.act(router.element_ref(tab, ask.id), BrowserAction("click", expect_dialog="dismiss"), "")
        s = await router.snapshot(tab)
        out["after_input"] = s.text
        out["dialogs"] = s.dialogs
        out["user_value"] = el(s, label="User").value
        ask = el(s, label="Ask")
        out["accepted"] = await router.act(router.element_ref(tab, ask.id), BrowserAction("click", expect_dialog="accept"), "")
        s = await router.snapshot(tab)
        out["after_accept"] = s.text
        attach = el(s, label="Attach")
        out["upload"] = await router.act(router.element_ref(tab, attach.id), BrowserAction("upload", files=(str(upload),)), "")
        s = await router.snapshot(tab)
        out["after_upload"] = s.text
        box = next(sc for sc in s.scrollers if sc["label"] == "messages")
        out["box_before"] = box
        out["box_scroll"] = await router.scroll(tab, "down", box["id"])
        out["page_scroll"] = await router.scroll(tab, "down")
        s = await router.snapshot(tab)
        out["box_after"] = next(sc for sc in s.scrollers if sc["label"] == "messages")
        out["scroll_after"] = s.scroll
        out["end_link"] = (el(s, label="End link").in_view, el(s, label="End link").offscreen)
        out["shot"] = (await router.screenshot(tab))[:3]
        tabs = await t.bridge.call("list_tabs", {})
        mine = next(x for x in tabs["tabs"] if x["tab_id"] == t.chrome_id)
        out["tab"] = mine
        out["window"] = chrome.ext_window(chrome.sw, mine["window_id"])    # type: ignore[attr-defined]
        out["on_screen_after_input"] = chrome.on_screen()
        await router.close(tab)
        out["closed"] = tab.tab_id not in router._tabs

    d.run(go(), timeout=180)
    changes = front.stop()
    print("\nLAB RESULT", json.dumps({k: (v if isinstance(v, (str, int, float, bool, list, dict, tuple, type(None)))
                                         else str(v)) for k, v in out.items()}, default=str)[:6000])
    print("FRONTMOST CHANGES", changes, "lab chrome pid", chrome.pid)
    assert out["window"] == {"state": "minimized", "focused": False}       # FERB's window, as Chrome sees it
    assert out["on_screen_after_open"] == [] and out["on_screen_after_input"] == []   # and as macOS shows it
    assert out["tab"]["active"] is False and out["tab"]["ferb"] is True
    assert out["type"].ok and "trusted" in out["type"].detail and out["user_value"] == "ada"
    assert "input trusted=true value=ada" in out["after_input"]                   # the page saw real input
    assert "clicked trusted=true" in out["after_input"]
    assert "key Enter trusted=true" in out["after_input"]
    assert "confirm=false" in out["after_input"] and out["dialogs"] == ["confirm: Sure? (dismissed)"]
    assert "confirm=true" in out["after_accept"]
    assert out["upload"].ok and "files=1" in out["after_upload"]
    assert out["box_before"]["y"] == 0 and out["box_scroll"]["y"] > 0 and out["box_after"]["y"] == out["box_scroll"]["y"]
    assert out["page_scroll"]["y"] > 0 and out["scroll_after"]["y"] == out["page_scroll"]["y"]
    assert out["scroll"]["y"] == 0 and out["scroll_after"]["viewport_h"] == 900   # the emulated viewport, once attached
    assert any(r == "listitem" and c for r, _l, _v, _o, c in out["elements"])   # rows, with their container
    assert out["shot"] == b"\xff\xd8\xff"                                       # a jpeg, from a minimized window
    assert out["closed"]
    assert not front.took_focus(chrome.pid), changes                            # the lab Chrome never came forward
    assert changes == [], changes                                               # nothing moved the owner at all
