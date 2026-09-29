"""Build step 6a, real browser (pytest -m real): the FERB extension loaded unpacked in ONE test profile, driven by a
test daemon with its own home. FERB's installed native-host launcher is pointed at the test daemon for the run and
restored afterwards. Local pages only (127.0.0.1); no real sites, no accounts.

    FERB_REAL_PROFILE="chrome:FERB Test" uv run pytest -m real tests/test_ext_real.py

Checks: pairing via the extension's own page; background tab (never activated); snapshot sanitized in the page
(password value, hidden input, URL token); page-mode typing; trusted click + typing in the background tab via the
debugger (every event isTrusted, tab stays inactive); the native host dying -> reconnect with a new generation ->
old element ids refused (21.13); the extension refuses non-allowlisted CDP (21.6).
"""

import asyncio
import http.server
import re
import json
import os
import subprocess
import tempfile
import threading
import time
from pathlib import Path
from typing import Any, Iterator

import pytest

from ferb.act.browser.profiles import discover_all
from ferb.cli.client import Client
from ferb.clock.clock import SystemClock
from ferb.core.errors import DriverError, StaleRef
from ferb.core.types import BrowserAction
from tests.conftest import RunningDaemon

pytestmark = pytest.mark.real
LAUNCHER = Path.home() / ".ferb" / "bin" / "ferb-native-host"
PASSWORD = "hunter2-" + os.urandom(4).hex()

PAGE = """<!doctype html><title>FERB real test</title>
<form><input id=user name=user placeholder=User><input id=pw type=password name=pw>
<input type=hidden name=csrf value=HIDDEN-TOKEN>
<button type=button id=b onclick="document.getElementById('out').textContent='clicked trusted=' + event.isTrusted">Go</button>
<input id=t placeholder=Trusted onkeydown="window.__k = (window.__k || 0) + (event.isTrusted ? 1 : 0)"></form>
<p id=out>not clicked</p><script>document.getElementById('pw').value = '%s'</script>"""


@pytest.fixture(scope="module")
def target() -> tuple[str, str]:
    spec = os.environ.get("FERB_REAL_PROFILE")
    if not spec or ":" not in spec:
        pytest.skip('set FERB_REAL_PROFILE="chrome:<test profile name>" (a test profile with the extension loaded)')
    browser, name = spec.split(":", 1)
    return browser, name


@pytest.fixture(scope="module")
def rd(target: tuple[str, str]) -> Iterator[RunningDaemon]:
    if not LAUNCHER.exists():
        pytest.skip("run `ferb setup --update` first (installs the native host)")
    base = Path(tempfile.mkdtemp(prefix="ferbr-"))
    original = LAUNCHER.read_text()
    d = RunningDaemon(base / "home", clock=SystemClock())
    LAUNCHER.write_text(re.sub(r'FERB_HOME="[^"]*"', f'FERB_HOME="{d.daemon.home}"', original))
    restart_hosts()                    # an extension already connected keeps its old host (and daemon) otherwise
    try:
        yield d
    finally:
        LAUNCHER.write_text(original)                          # the installed daemon gets its browsers back
        restart_hosts()
        d.stop()


def restart_hosts() -> None:
    """Only FERB's own relay processes; the extension reconnects through the launcher within seconds."""
    subprocess.run(["pkill", "-f", "ferb.native_host.main"], capture_output=True)
    time.sleep(3)


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


def test_real_extension_end_to_end(rd: RunningDaemon, target: tuple[str, str], site: str) -> None:
    browser, name = target
    d = rd.daemon
    assert any(p.display_name == name and p.browser == browser for p in discover_all()), \
        f"{browser} profile {name!r} isn't in Local State yet (the browser writes it lazily)"
    # the owner has a window of that profile open (a blank local page), as when they use it; FERB's tab must stay
    # in the background of it
    from ferb.act.browser.profiles import open_known_profile
    p = next(p for p in discover_all() if p.display_name == name and p.browser == browser)
    open_known_profile(browser, p.folder, f"{site}/owner-window", discover_all())
    time.sleep(2)
    c = Client(rd.sock, timeout=90)
    r = c.call("browsers.pair", {"profile": name, "browser": browser, "timeout_s": 60})
    c.close()
    assert r["paired"], r
    router = d.router
    assert router is not None

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
        await t.bridge.call("attach_debugger", {"tab_id": t.chrome_id})
        try:
            await t.bridge.call("cdp", {"tab_id": t.chrome_id, "method": "Network.getAllCookies", "params": {}})
            out["cookies_refused"] = False
        except DriverError as e:
            out["cookies_refused"] = "not allowed" in str(e)
        finally:
            await t.bridge.call("detach_debugger", {"tab_id": t.chrome_id})
        old_gen, keep = t.bridge.gen, router.element_ref(tab, user)
        subprocess.run(["pkill", "-f", "ferb.native_host.main"], capture_output=True)   # the host dies
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
    assert out["cookies_refused"]
    assert out["reconnected"] and out["stale_refused"]
