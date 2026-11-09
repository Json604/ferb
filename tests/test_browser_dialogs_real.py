"""HAND-442/443/446, REAL (pytest -m real): a throwaway headless Chrome (tests/chromelab: its own --user-data-dir, the
unpacked extension, a test daemon with its own home) and a local site; nothing of the owner's is touched.

- A page that asks before it is left ("Leave site?", beforeunload) after FERB typed into it: a navigate without the
  worker's choice ends at once with the dialog fact; with leave_page "leave" the page asks nothing (the page hook skips
  its listener), the tab loads the new address and Chrome never activates the tab (it does for the dialog: measured
  2026-11-09, headless, the tab activated ~5-10 ms before Page.javascriptDialogOpening).
- A link that opens a new tab with no opener link (rel=noopener, which FERB's link capture doesn't see when the page
  handles the click itself): the task's click's new tab is the task's and the opener's next result names it.

    uv run pytest -m real tests/test_browser_dialogs_real.py -s
"""

import asyncio
import http.server
import threading
from typing import Any, Iterator

import pytest

from ferb.core.errors import DriverError
from ferb.core.types import BrowserAction
from tests.chromelab import browser_lab
from tests.test_downloads_real import _live_task

pytestmark = [pytest.mark.real, pytest.mark.ferb_timeout(0)]

PAGES = {
    "/form": b"<!doctype html><title>Form</title><input aria-label=Name>"
             b"<script>addEventListener('beforeunload', e => { e.preventDefault(); e.returnValue = ''; });</script>",
    "/links": b"<!doctype html><title>Links</title><button onclick=\"window.open('/next?via=noopener', '_blank', "
              b"'noopener')\">Continue</button>",
    "/next": b"<!doctype html><title>Next</title><p>next page",
}


@pytest.fixture(scope="module")
def site() -> Iterator[str]:
    class H(http.server.BaseHTTPRequestHandler):
        def do_GET(self) -> None:  # noqa: N802
            body = PAGES.get(self.path.split("?")[0], b"x")
            self.send_response(200)
            self.send_header("content-type", "text/html")
            self.send_header("content-length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

        def log_message(self, *a: Any) -> None:
            pass
    srv = http.server.ThreadingHTTPServer(("127.0.0.1", 0), H)
    threading.Thread(target=srv.serve_forever, daemon=True).start()
    yield f"http://127.0.0.1:{srv.server_port}"
    srv.shutdown()


# decision: HAND-442, HAND-443, HAND-446
def test_leave_site_and_a_noopener_tab_in_a_real_chrome(site: str) -> None:
    with browser_lab(headless=True) as (d, chrome, front):
        window = int(chrome.evaluate(chrome.sw, "ferbWindow(true)"))
        driver = d.daemon.router
        d.run(_live_task(d, "lab-task"))

        async def act(tab: Any, label: str, kind: str = "click", text: str | None = None) -> None:
            snap = await driver.snapshot(tab)
            el = next(e for e in snap.elements if e.label == label)
            assert (await driver.act(driver.element_ref(tab, el.id), BrowserAction(kind, text=text), "")).ok

        def active(tab: Any) -> bool:
            cid = driver._tabs[tab.tab_id].chrome_id
            return bool(chrome.evaluate(chrome.sw, f"chrome.tabs.get({cid}).then(t => t.active)"))

        async def run() -> dict[str, Any]:
            out: dict[str, Any] = {}
            tab = await driver.open_for(f"{site}/form", "lab-task", "Lab", window_id=window)
            await act(tab, "Name", "type", "Kai")                     # the page now has the owner's activation
            loop = asyncio.get_running_loop()
            t0 = loop.time()
            try:
                await driver.navigate(tab, f"{site}/next?try=plain")
                out["plain"] = "navigated"
            except DriverError as e:
                out["plain"] = e.facts.get("reason"), e.facts.get("dialog")
            out["plain_s"] = loop.time() - t0
            out["dialog"] = await driver.dialog(tab, None)
            out["dialog_activated"] = active(tab)                     # Chrome brought the tab forward for it
            await driver.dialog(tab, "dismiss")                       # stay: the form is still there
            await act(tab, "Name", "type", "Kai again")
            other = await driver.open_for(f"{site}/next", "lab-task", "Lab", window_id=window)
            cid = driver._tabs[other.tab_id].chrome_id
            chrome.evaluate(chrome.sw, f"chrome.tabs.update({cid}, {{active: true}}).then(() => true)")   # lab only
            out["active_before"] = active(tab)
            t0 = loop.time()
            await driver.navigate(tab, f"{site}/next?try=leave", leave_page="leave")
            out["leave_s"] = loop.time() - t0
            out["leave_url"] = driver.tab_url(tab.tab_id)
            out["active_after"] = active(tab)
            links = await driver.open_for(f"{site}/links", "lab-task", "Lab", window_id=window)
            await act(links, "Continue")
            for _ in range(50):
                await asyncio.sleep(0.1)
                if found := [o for o in driver.opened_for(links) if o.get("tab")]:
                    out["opened"] = found
                    break
            return out

        out = d.run(run())
        assert out["plain"] == ("dialog_pending", {"type": "beforeunload"}) and out["plain_s"] < 3, out
        assert out["dialog"]["pending"]["type"] == "beforeunload" and out["dialog_activated"] is True
        assert "try=leave" in out["leave_url"] and out["leave_s"] < 5, out
        assert out["active_before"] is False and out["active_after"] is False, out   # no dialog: it stayed behind
        assert out["opened"] and "via=noopener" in out["opened"][0]["url"], out
        assert not front.took_focus(chrome.pid)
