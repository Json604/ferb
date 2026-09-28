"""Spike 4: FERB extension in Brave and Chrome, driven only through native messaging (no remote debugging).

Spare profiles only: Brave gets a fresh temporary profile; Chrome uses the persistent spare profile where the
owner loaded the extension by hand. The two native-host manifests exist only while the tests run.
"""

import http.server
import json
import threading
import time
from urllib.parse import parse_qs, urlsplit

import pytest

from browser_run import EXT_ID, ExtError, SpareBrowser, install_host, uninstall_host

PAGE = b"""<!doctype html><title>ferb spike 4</title>
<input id=field aria-label="query"><input id=pw type=password value="hunter2-not-a-secret">
<button id=btn onclick="log('click', event)">Go</button>
<script>
function log(kind, e) {
  fetch('/event?' + new URLSearchParams({kind, trusted: e.isTrusted, vis: document.visibilityState,
                                         key: e.key || ''}));
}
const f = document.getElementById('field');
f.addEventListener('mousedown', e => log('mousedown', e));
f.addEventListener('keydown', e => log('keydown', e));
</script>"""

events: list[dict] = []


@pytest.fixture(scope="session", autouse=True)
def native_host():
    install_host()
    yield
    uninstall_host()


@pytest.fixture(scope="session")
def local():
    class H(http.server.BaseHTTPRequestHandler):
        def do_GET(self):
            u = urlsplit(self.path)
            if u.path == "/event":
                events.append({k: v[0] for k, v in parse_qs(u.query).items()})
            self.send_response(200)
            self.send_header("content-type", "text/html")
            self.end_headers()
            if u.path == "/page":
                self.wfile.write(PAGE)

        def log_message(self, *a):
            pass

    srv = http.server.ThreadingHTTPServer(("127.0.0.1", 0), H)
    threading.Thread(target=srv.serve_forever, daemon=True).start()
    yield f"http://127.0.0.1:{srv.server_port}"
    srv.shutdown()


@pytest.fixture(scope="module", params=["brave", "chrome"])
def browser(request):
    b = SpareBrowser(request.param)
    yield b
    b.close()


def element(snap: dict, name: str) -> dict:
    return next(e for e in snap["elements"] if e["name"] == name)


def centre(el: dict) -> tuple[float, float]:
    x, y, w, h = el["rect"]
    return x + w / 2, y + h / 2


def attach_for_input(b: SpareBrowser, tab: int) -> None:
    """Hidden tabs drop dispatched input; focus emulation lets a background tab take it, once its input pipeline is
    live (probed below). Side effect: while attached, the page's own visibilityState reads "visible" (the tab stays
    inactive for the owner)."""
    b.cmd("attach_debugger", tabId=tab)
    b.cmd("cdp", tabId=tab, method="Emulation.setFocusEmulationEnabled", params={"enabled": True})
    # "visible" is not enough: until the tab's input pipeline is live, every dispatched event is dropped.
    # Probe with a harmless mouse move seen by an isolated-world listener; start real input only after it lands.
    end = time.monotonic() + 10
    while True:
        b.cmd("input_probe_arm", tabId=tab)
        b.cmd("cdp", tabId=tab, method="Input.dispatchMouseEvent", params={"type": "mouseMoved", "x": 1, "y": 1})
        time.sleep(0.05)
        if b.cmd("input_probe_seen", tabId=tab)["seen"]:
            return
        assert time.monotonic() < end, "background tab never started accepting input"
        time.sleep(0.2)


def click(b: SpareBrowser, tab: int, x: float, y: float) -> None:
    for kind in ("mousePressed", "mouseReleased"):
        b.cmd("cdp", tabId=tab, method="Input.dispatchMouseEvent",
              params={"type": kind, "x": x, "y": y, "button": "left", "clickCount": 1})


def type_text(b: SpareBrowser, tab: int, text: str) -> None:
    for ch in text:
        b.cmd("cdp", tabId=tab, method="Input.dispatchKeyEvent", params={"type": "keyDown", "key": ch, "text": ch})
        b.cmd("cdp", tabId=tab, method="Input.dispatchKeyEvent", params={"type": "keyUp", "key": ch})


def press_enter(b: SpareBrowser, tab: int) -> None:
    enter = {"key": "Enter", "code": "Enter", "windowsVirtualKeyCode": 13}
    b.cmd("cdp", tabId=tab, method="Input.dispatchKeyEvent", params={"type": "keyDown", "text": "\r", **enter})
    b.cmd("cdp", tabId=tab, method="Input.dispatchKeyEvent", params={"type": "keyUp", **enter})


def poll_snapshot(b: SpareBrowser, tab: int, done, timeout: float = 30) -> dict:
    end, snap = time.monotonic() + timeout, {}
    while time.monotonic() < end:
        try:
            snap = b.cmd("snapshot", tabId=tab)
            if snap and done(snap):
                return snap
        except ExtError:
            pass  # page mid-navigation
        time.sleep(0.5)
    return snap


def test_native_messaging_round_trip(browser):
    assert browser.hello.extension_id == EXT_ID


def test_background_tab_open_and_sanitized_snapshot(browser, local):
    tab = browser.cmd("open", url=f"{local}/page")["tabId"]
    try:
        snap = browser.cmd("snapshot", tabId=tab)
        assert snap["visibility"] == "hidden"            # opened in the background
        assert snap["webdriver"] is False                # no remote debugging: not flagged as automated
        assert element(snap, "field")["label"] == "query"
        assert element(snap, "pw")["value"] == "[redacted]"
        assert "hunter2" not in json.dumps(snap)
    finally:
        browser.cmd("close", tabId=tab)


def test_trusted_click_and_typing_in_background_tab(browser, local):
    events.clear()
    tab = browser.cmd("open", url=f"{local}/page")["tabId"]
    try:
        x, y = centre(element(browser.cmd("snapshot", tabId=tab), "field"))
        attach_for_input(browser, tab)
        try:
            click(browser, tab, x, y)
            type_text(browser, tab, "hello")
            assert browser.cmd("tab_state", tabId=tab)["active"] is False   # never brought to the front
        finally:
            browser.cmd("detach_debugger", tabId=tab)
        snap = poll_snapshot(browser, tab, lambda s: element(s, "field")["value"] == "hello", timeout=10)
        assert element(snap, "field")["value"] == "hello"
        assert snap["visibility"] == "hidden"            # back to hidden once detached
        time.sleep(0.5)
        kinds = [(e["kind"], e.get("key", ""), e["trusted"]) for e in events]
        assert ("mousedown", "", "true") in kinds
        assert [k for k in kinds if k[0] == "keydown"] == [("keydown", c, "true") for c in "hello"]
    finally:
        browser.cmd("close", tabId=tab)


@pytest.mark.parametrize("method", ["Network.getAllCookies", "Runtime.evaluate", "Target.getTargets",
                                    "Storage.getCookies"])
def test_extension_refuses_non_allowlisted_cdp(browser, local, method):
    tab = browser.cmd("open", url=f"{local}/page")["tabId"]
    try:
        browser.cmd("attach_debugger", tabId=tab)
        try:
            with pytest.raises(ExtError, match="not allowed"):
                browser.cmd("cdp", tabId=tab, method=method, params={"expression": "document.cookie"})
        finally:
            browser.cmd("detach_debugger", tabId=tab)
    finally:
        browser.cmd("close", tabId=tab)


def test_bot_protected_site_trusted_typing_in_background_tab(browser):
    """Google Search (signed out, spare profile): type a query in a background tab and reach results,
    not the /sorry/ automated-traffic page."""
    tab = browser.cmd("open", url="https://www.google.com/?hl=en")["tabId"]
    try:
        snap = poll_snapshot(browser, tab, lambda s: any(e["name"] == "q" for e in s["elements"]))
        box = element(snap, "q")
        assert snap["visibility"] == "hidden"
        attach_for_input(browser, tab)
        try:
            click(browser, tab, *centre(box))
            type_text(browser, tab, "ferb spike four")
            press_enter(browser, tab)
        finally:
            browser.cmd("detach_debugger", tabId=tab)
        snap = poll_snapshot(browser, tab, lambda s: "/search" in s["url"] or "/sorry/" in s["url"])
        assert "/sorry/" not in snap["url"], "Google flagged the input as automated"
        assert "/search" in snap["url"] and "ferb+spike+four" in snap["url"]
    finally:
        browser.cmd("close", tabId=tab)
