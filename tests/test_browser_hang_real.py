"""HAND-434/439, REAL (pytest -m real; the owner runs it): a throwaway windowed Chrome (tests/chromelab: its own
--user-data-dir, the unpacked extension, a test daemon with its own home) and a local site. Two past hangs:

- a task's first tab, which makes its collapsed group, was frozen by Chrome 2.0 s after its load, so every read of it
  timed out (lab 2026-11-08: 20 of 20); it must answer at once 3 s after `open`, its group still collapsed;
- a page with a frame that never finishes loading held every read until the command timed out (96.7 s); it must read
  whole, its 2,000-cell table included, in a few seconds.

    uv run pytest -m real tests/test_browser_hang_real.py -s
"""

import http.server
import threading
import time
from typing import Any, Iterator

import pytest

from tests.chromelab import browser_lab

pytestmark = [pytest.mark.real, pytest.mark.ferb_timeout(0)]
CELLS = 2000


@pytest.fixture(scope="module")
def site() -> Iterator[int]:
    class H(http.server.BaseHTTPRequestHandler):
        protocol_version = "HTTP/1.0"

        def do_GET(self) -> None:  # noqa: N802
            if self.path == "/never":                   # a frame whose page never finishes loading
                self.send_response(200)
                self.send_header("content-type", "text/html")
                self.end_headers()
                try:
                    self.wfile.write(b"<p>still loading")
                    self.wfile.flush()
                    time.sleep(90)
                except OSError:
                    pass
                return
            if self.path == "/heavy":
                rows = "".join("<tr>" + "".join(f"<td>r{r}c{c}</td>" for c in range(20)) + "</tr>"
                               for r in range(CELLS // 20))
                page = (f"<title>Heavy</title><h1>Heavy</h1><table>{rows}</table>"
                        + "".join(f'<iframe src="/frame" width=120 height=60></iframe>' for _ in range(10))
                        + '<iframe src="/never" width=120 height=60></iframe>')
            elif self.path == "/frame":
                page = "<button>in a frame</button>"
            else:
                page = "<title>Example</title><h1>Example Domain</h1><a href='https://www.iana.org/'>More</a>"
            body = page.encode()
            self.send_response(200)
            self.send_header("content-type", "text/html")
            self.end_headers()
            self.wfile.write(body)

        def log_message(self, *a: Any) -> None:
            pass
    srv = http.server.ThreadingHTTPServer(("127.0.0.1", 0), H)
    srv.daemon_threads = True
    threading.Thread(target=srv.serve_forever, daemon=True).start()
    yield srv.server_port
    srv.shutdown()


# decision: HAND-434
def test_a_tasks_first_tab_is_awake_and_answers_3_s_after_it_opens(site: int) -> None:
    with browser_lab(chrome_args=["--host-resolver-rules=MAP *.lab.test 127.0.0.1"]) as (d, chrome, front):
        router, sw = d.daemon.router, chrome.sw
        # an owner-like normal window with one ordinary tab: FERB's tabs open there (no FERB window in the profile)
        chrome.evaluate(sw, "chrome.windows.create({url: 'http://owner.lab.test:%d/', focused: false, state: 'normal'})"
                            ".then(w => w.id)" % site)
        time.sleep(1.5)
        for n in range(3):
            root = f"first-tab-{n}"
            tab = d.run(router.open_for(f"http://a{n}.lab.test:{site}/", root, "Lab"), timeout=60)
            cid = router._tabs[tab.tab_id].chrome_id
            time.sleep(3)                                  # Chrome froze it 2.0 s after its load
            state = chrome.evaluate(sw, f"chrome.tabs.get({cid}).then(async t => ({{frozen: !!t.frozen, collapsed: "
                                        "(await chrome.tabGroups.get(t.groupId)).collapsed}))")
            started = time.monotonic()
            snap = d.run(router.snapshot(tab), timeout=40)
            took = time.monotonic() - started
            d.run(router.release(root), timeout=60)
            assert state == {"frozen": False, "collapsed": True}, state       # HAND-201's group, still collapsed
            assert took < 5 and snap.title == "Example", (took, snap.title)
        assert not front.took_focus(chrome.pid)


# decision: HAND-434, HAND-439
def test_a_frame_that_never_loads_holds_no_read(site: int) -> None:
    with browser_lab(chrome_args=["--host-resolver-rules=MAP *.lab.test 127.0.0.1"]) as (d, chrome, front):
        router = d.daemon.router
        tab = d.run(router.open_for(f"http://h.lab.test:{site}/heavy", "heavy", "Lab"), timeout=60)
        started = time.monotonic()
        snap = d.run(router.snapshot(tab), timeout=40)
        took = time.monotonic() - started
        cells = [e for e in snap.elements if e.role == "cell"]
        timing = router.timing(tab) or {}
        d.run(router.release("heavy"), timeout=60)
        assert len(cells) == CELLS and took < 10, (len(cells), took, timing)
        assert (timing.get("read") or {}).get("adopted", 0) >= CELLS          # the batched pass adopted every cell
