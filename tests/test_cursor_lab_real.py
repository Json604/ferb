"""L33 review (9), REAL (pytest -m real): FERB's cursor's page prediction against where the element really is after the
act, in a disposable headless Chrome (its own temporary profile, the unpacked FERB extension, a test daemon and a local
page; nothing else is touched). A plain element must land where FERB said (within 2 CSS px), also on a page that
scrolls smoothly (the centring scroll is instant, so real input aims at the element); an element the act's scroll
moves in ways the snapshot can't tell (fixed, sticky, a body that scrolls by itself, hidden sideways overflow, a
carousel) gets no point at all.

    uv run pytest -m real tests/test_cursor_lab_real.py -s
"""

import http.server
import json
import threading
from typing import Any, Iterator

import pytest

from ferb.core.types import BrowserAction
from tests.chromelab import browser_lab

pytestmark = pytest.mark.real

FILL = "<div style='height:1500px'></div>"
TARGET = "<button id=x onclick=\"this.textContent='hit '+event.isTrusted\">Target</button>"
PAGES = {
    "/normal": "<style>body{margin:0}</style>" + FILL + TARGET + FILL,
    "/smooth": "<style>html{scroll-behavior:smooth}body{margin:0}</style>" + FILL + TARGET + FILL,
    "/end": "<style>body{margin:0}</style><div style='height:2000px'></div>" + TARGET + "<div style='height:100px'></div>",
    "/fixed": "<style>body{margin:0}</style><div style='position:fixed;top:300px;left:10px'>" + TARGET + "</div>"
              "<div style='height:3000px'></div>",
    "/sticky": "<style>body{margin:0}</style><div style='height:1200px'></div><div style='position:sticky;top:0;"
               "background:#fff'>" + TARGET + "</div><div style='height:3000px'></div>",
    "/body": "<style>html{overflow:hidden;height:100%}body{margin:0;overflow:auto;height:100%}</style>"
             + FILL + TARGET + FILL,
    "/hiddenx": "<style>body{margin:0}</style><div style='width:300px;overflow-x:hidden;white-space:nowrap;"
                "margin-top:100px'><span style='display:inline-block;width:500px;height:10px'></span>" + TARGET + "</div>",
    "/carousel": "<style>body{margin:0}</style><div style='width:400px;overflow-x:auto;white-space:nowrap;"
                 "margin-top:100px'>" + "".join(f"<button style='width:300px'>Slide {i}</button>" for i in (1, 2))
                 + TARGET.replace(">Target<", " style='width:300px'>Target<") + "<button style='width:300px'>Slide 4"
                 "</button></div>",
}
PLACED = ("/normal", "/smooth", "/end")


@pytest.fixture(scope="module")
def site() -> Iterator[str]:
    class H(http.server.BaseHTTPRequestHandler):
        def do_GET(self) -> None:  # noqa: N802
            self.send_response(200)
            self.send_header("content-type", "text/html")
            self.end_headers()
            self.wfile.write(f"<!doctype html><title>Cursor lab</title>{PAGES.get(self.path, '')}".encode())

        def log_message(self, *a: Any) -> None:
            pass
    srv = http.server.ThreadingHTTPServer(("127.0.0.1", 0), H)
    threading.Thread(target=srv.serve_forever, daemon=True).start()
    yield f"http://127.0.0.1:{srv.server_port}"
    srv.shutdown()


def test_the_cursor_predicts_where_a_click_puts_its_element_or_says_nothing(site: str) -> None:
    with browser_lab(headless=True) as (d, chrome, _):
        window = int(chrome.evaluate(chrome.sw, "ferbWindow(true)"))   # this headless lab's own FERB window
        router = d.daemon.router
        seen: dict[str, Any] = {}

        async def case(path: str) -> None:
            tab = await router.open_for(site + path, "cursor-lab", "Lab", window_id=window)
            try:
                snap = await router.snapshot(tab)
                target = next(e for e in snap.elements if e.label == "Target")
                ref = router.element_ref(tab, target.id)
                predicted = router.page_rect(ref, BrowserAction("click"))
                result = await router.act(ref, BrowserAction("click"), "")
                chrome_id = router._tabs[tab.tab_id].chrome_id
                real = chrome.evaluate(chrome.sw, "chrome.scripting.executeScript({target: {tabId: %d}, func: () => {"
                                       "const e = document.getElementById('x'), r = e.getBoundingClientRect();"
                                       "return [r.x, r.y, r.width, r.height, e.textContent];}}).then(r => r[0].result)"
                                       % chrome_id)
                seen[path] = {"snapshot": list(target.rect), "predicted": list(predicted) if predicted else None,
                              "real": real[:4], "clicked": real[4], "ok": result.ok, "detail": result.detail}
            finally:
                await router.close(tab)

        for path in PAGES:
            d.run(case(path), timeout=60)
        print(json.dumps(seen, indent=1))
        for path in PLACED:
            got = seen[path]
            assert got["predicted"] is not None, (path, got)
            # where it is (its width changes: the click retitles it "hit true")
            assert all(abs(got["predicted"][i] - got["real"][i]) <= 2 for i in (0, 1, 3)), (path, got)
            assert got["clicked"] == "hit true", (path, got)      # real input hit the element it aimed at
        assert seen["/normal"]["real"][1] != seen["/normal"]["snapshot"][1]   # the act did scroll it
        for path in set(PAGES) - set(PLACED):
            assert seen[path]["predicted"] is None, (path, seen[path])
