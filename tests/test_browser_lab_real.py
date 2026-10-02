"""Rework phase 3, browser, REAL (pytest -m real): a throwaway Chrome (its own --user-data-dir under a temp folder,
never the owner's browsers or profiles) with the unpacked FERB extension, a test daemon with its own home, and a
local page. Nothing but the lab Chrome is touched; ~/.ferb stays out.

Proves on this Mac what PLATFORM_BROWSER.md 13 promises: FERB's tab opens in a minimized window and stays inactive;
real (isTrusted) typing, clicks and keys land there; the page and a container scroll by wheel; a confirm() is
answered by the extension; a file lands in a file input; a screenshot renders; and the frontmost app never changed
while all of it happened (NSWorkspace polled every 50 ms, as in the probes).

    uv run pytest -m real tests/test_browser_lab_real.py -s
"""

import asyncio
import http.server
import json
import tempfile
import threading
import time
from pathlib import Path
from typing import Any, Iterator

import pytest

from ferb.act.browser.ext_protocol import FERB_EXTENSION_ID
from ferb.clock.clock import SystemClock
from ferb.core.types import BrowserAction
from tests.chromelab import ChromeLab, Frontmost, chrome_available, write_native_host
from tests.conftest import RunningDaemon, without_owner_mac

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
<div id=box aria-label=messages role=list>%s</div>
<div class=tall>tall filler</div><a href="#end" id=end>End link</a>
""" % "".join(f"<div role=listitem>message {i}</div>" for i in range(1, 21))


@pytest.fixture(scope="module")
def site() -> Iterator[str]:
    class H(http.server.BaseHTTPRequestHandler):
        def do_GET(self) -> None:  # noqa: N802
            self.send_response(200)
            self.send_header("content-type", "text/html")
            self.end_headers()
            self.wfile.write(PAGE.encode())

        def log_message(self, *a: Any) -> None:
            pass
    srv = http.server.ThreadingHTTPServer(("127.0.0.1", 0), H)
    threading.Thread(target=srv.serve_forever, daemon=True).start()
    yield f"http://127.0.0.1:{srv.server_port}"
    srv.shutdown()


@pytest.fixture(scope="module")
def lab() -> Iterator[tuple[RunningDaemon, ChromeLab, Frontmost]]:
    if not chrome_available():
        pytest.skip("Google Chrome (and node) are needed for the lab")
    base = Path(tempfile.mkdtemp(prefix="ferbl-"))          # short path: the daemon socket must fit in sun_path
    udd = base / "chrome"
    udd.mkdir()
    # a named profile in Local State so FERB's discovery finds the lab profile ("Lab" = the Default folder)
    (udd / "Local State").write_text(json.dumps({"profile": {"info_cache": {"Default": {"name": "Lab", "user_name": ""}},
                                                             "last_used": "Default"}}))
    d = RunningDaemon(base / "home", clock=SystemClock())
    without_owner_mac(d)                                                  # locked down: browser tools only
    dm = d.daemon
    dm.browser_data_dirs = {"chrome": udd}
    launched: list[list[str]] = []
    dm.browser_launch = launched.append                                   # never launch a real browser
    assert dm.router is not None
    dm.router.launch, dm.router.wake = launched.append, None
    dm.router.is_running = lambda b: True
    write_native_host(udd, d.sock, FERB_EXTENSION_ID)
    front = Frontmost()
    chrome = ChromeLab(udd)
    try:
        assert chrome.load_extension(EXT_DIR) == FERB_EXTENSION_ID        # the pinned id (manifest key)
        _wait(lambda: FERB_EXTENSION_ID and "chrome" in dm.hub.unpaired.values(), 30, "the extension never said hello")
        token = d.run(_issue(d))
        sw = chrome.worker_session(FERB_EXTENSION_ID)
        chrome.evaluate(sw, f"send('pair', {{token: {json.dumps(token)}}})")
        _wait(lambda: (br := dm.hub.bridges.get(("chrome", "Default"))) is not None and br.conn is not None, 30,
              "pairing never completed")
        chrome.sw = sw                                                   # type: ignore[attr-defined]
        yield d, chrome, front
    finally:
        chrome.close()
        d.stop()
        front.stop()


async def _issue(d: RunningDaemon) -> str:
    return d.daemon.pairings.issue_token("chrome", "Default")


def _wait(cond: Any, timeout: float, what: str) -> None:
    end = time.monotonic() + timeout
    while not cond():
        if time.monotonic() > end:
            raise AssertionError(what)
        time.sleep(0.1)


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
        out["dismissed"] = await router.act(router.element_ref(tab, ask.id), BrowserAction("click"), "")
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
