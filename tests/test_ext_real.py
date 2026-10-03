"""Real extension regression in a disposable ChromeLab: local pages, trusted background input, password/URL
redaction and stale-ref refusal after this lab extension reconnects. No installed profiles or native hosts."""

import asyncio
import http.server
import json
import os
import threading
import time
from typing import Any, Iterator

import pytest

from tests.chromelab import lab  # noqa: F401

from ferb.core.errors import StaleRef
from ferb.core.types import BrowserAction

pytestmark = pytest.mark.real
PASSWORD = "hunter2-" + os.urandom(4).hex()

PAGE = """<!doctype html><title>FERB real test</title>
<form><input id=user name=user placeholder=User><input id=pw type=password name=pw>
<input type=hidden name=csrf value=HIDDEN-TOKEN>
<button type=button id=b onclick="document.getElementById('out').textContent='clicked trusted=' + event.isTrusted">Go</button>
<input id=t placeholder=Trusted onkeydown="window.__k = (window.__k || 0) + (event.isTrusted ? 1 : 0)"></form>
<p id=out>not clicked</p><script>document.getElementById('pw').value = '%s'</script>"""


@pytest.fixture(scope="module")
def site() -> Iterator[str]:
    class H(http.server.BaseHTTPRequestHandler):
        def do_GET(self) -> None:  # noqa: N802
            self.send_response(200)
            self.send_header("content-type", "text/html")
            self.end_headers()
            self.wfile.write((PAGE % PASSWORD).encode())

        def log_message(self, *a: Any) -> None:
            pass
    srv = http.server.ThreadingHTTPServer(("127.0.0.1", 0), H)
    threading.Thread(target=srv.serve_forever, daemon=True).start()
    yield f"http://127.0.0.1:{srv.server_port}"
    srv.shutdown()


def test_real_extension_end_to_end(lab: Any, site: str) -> None:
    rd, chrome, _ = lab
    d = rd.daemon
    router = d.router
    assert router is not None
    chrome.owner_tab(f"{site}/owner-window")
    name = "Lab"

    async def go() -> dict[str, Any]:
        out: dict[str, Any] = {}
        tab = await router.open_for(f"{site}/page?token=SECRET123&x=1", "t", name)
        t = router._tabs[tab.tab_id]
        await asyncio.sleep(0.5)
        out["active_after_open"] = t.bridge.tabs.get(t.chrome_id, ("", None))[1]
        s = await router.snapshot(tab)
        out["snapshot"] = json.dumps([e.__dict__ for e in s.elements]) + s.url + s.text
        user = next(e.id for e in s.elements if e.label == "User")
        await router.act(router.element_ref(tab, user), BrowserAction("type", "ada"), "")
        s = await router.snapshot(tab)
        out["typed"] = next(e.value for e in s.elements if e.label == "User")
        go_btn = next(e.id for e in s.elements if e.label == "Go")
        out["trusted_click"] = (await router.act(router.element_ref(tab, go_btn), BrowserAction("click", trusted=True), "")).ok
        s = await router.snapshot(tab)
        out["page_after_click"] = s.text
        tr = next(e.id for e in s.elements if e.label == "Trusted")
        out["trusted_type"] = (await router.act(router.element_ref(tab, tr), BrowserAction("type", "xyz", trusted=True), "")).ok
        s = await router.snapshot(tab)
        out["trusted_value"] = next(e.value for e in s.elements if e.label == "Trusted")
        tabs = await t.bridge.call("list_tabs", {})
        out["active_after_input"] = next(x["active"] for x in tabs["tabs"] if x["tab_id"] == t.chrome_id)
        # (the CDP allowlist has no daemon-facing op any more: tests/test_browser_background.py checks it in node)
        old_gen, keep = t.bridge.gen, router.element_ref(tab, user)
        chrome.reconnect()  # only the disposable extension relay disconnects
        for _ in range(100):
            await asyncio.sleep(0.1)
            if t.bridge.conn is not None and t.bridge.gen > old_gen:
                break
        out["reconnected"] = t.bridge.gen > old_gen and t.bridge.conn is not None
        try:
            await router.act(keep, BrowserAction("click"), "")
            out["stale_refused"] = False
        except StaleRef:
            out["stale_refused"] = True
        router._tabs[tab.tab_id] = t                                            # close it on the new connection
        t.ref = t.ref.__class__(t.ref.device_id, t.ref.browser, t.ref.profile, t.bridge.gen, t.ref.tab_id)
        await router.close(t.ref)
        return out

    out = rd.run(go(), timeout=180)
    assert out["active_after_open"] is False                                    # background tab
    assert PASSWORD not in out["snapshot"] and "HIDDEN-TOKEN" not in out["snapshot"]
    assert "SECRET123" not in out["snapshot"]
    assert "%5Bredacted%5D" in out["snapshot"] or "[redacted]" in out["snapshot"]           # URL token masked
    assert '"filled"' in out["snapshot"]
    assert out["typed"] == "ada"
    assert out["trusted_click"] and "clicked trusted=true" in out["page_after_click"]
    assert out["trusted_type"] and out["trusted_value"] == "xyz"
    assert out["active_after_input"] is False                                   # never activated
    assert out["reconnected"] and out["stale_refused"]
