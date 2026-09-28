"""Spike 6: frontmost app, window titles, Chrome/Brave web AX tree, TCC status — no prompts, no focus stealing.

Browsers run as separate instances on temporary profiles showing a local page; nothing is brought to the front.
TCC results belong to this process's *responsible app* (the terminal/editor running it), not to FERB.
"""

import http.server
import subprocess
import tempfile
import threading
import time

import pytest

import probes as P

TITLE = "ferb-ax-probe"
PAGE = f"<title>{TITLE}</title><button>FERB-PROBE-BUTTON</button><input aria-label='FERB-PROBE-FIELD'>".encode()
BROWSERS = {
    "chrome": ("com.google.Chrome", "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"),
    "brave": ("com.brave.Browser", "/Applications/Brave Browser.app/Contents/MacOS/Brave Browser"),
}
AX_ATTRIBUTE_UNSUPPORTED = -25205


@pytest.fixture(scope="module")
def page_url():
    class H(http.server.BaseHTTPRequestHandler):
        def do_GET(self):
            self.send_response(200)
            self.send_header("content-type", "text/html")
            self.end_headers()
            self.wfile.write(PAGE)

        def log_message(self, *a):
            pass

    srv = http.server.ThreadingHTTPServer(("127.0.0.1", 0), H)
    threading.Thread(target=srv.serve_forever, daemon=True).start()
    yield f"http://127.0.0.1:{srv.server_port}/"
    srv.shutdown()


@pytest.fixture(scope="module", params=list(BROWSERS))
def browser(request, page_url):
    _, binary = BROWSERS[request.param]
    p = subprocess.Popen([binary, f"--user-data-dir={tempfile.mkdtemp(prefix='ferb6-')}", "--no-first-run",
                          "--no-default-browser-check", page_url],
                         stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    end = time.monotonic() + 30
    while not any(TITLE in t for t in P.cg_window_titles(p.pid) + P.ax_window_titles(p.pid)):
        assert time.monotonic() < end, "browser window never showed the probe page"
        time.sleep(0.5)
    yield request.param, p.pid
    p.terminate()
    p.wait(timeout=15)


def test_frontmost_app_needs_no_permission():
    app = P.frontmost_app()
    assert app.pid > 0 and app.bundle_id


def test_tcc_checks_return_known_states_without_prompting():
    assert P.accessibility() in (P.Tcc.GRANTED, P.Tcc.DENIED)
    assert P.screen_recording() in (P.Tcc.GRANTED, P.Tcc.DENIED)
    assert P.full_disk_access() in (P.Tcc.GRANTED, P.Tcc.DENIED)
    assert P.automation("com.ferb.no-such-app") == P.Tcc.TARGET_NOT_RUNNING


@pytest.mark.skipif(P.accessibility() != P.Tcc.GRANTED, reason="Accessibility not granted")
def test_window_title_via_accessibility_without_activating(browser):
    name, pid = browser
    assert any(t.startswith(TITLE) for t in P.ax_window_titles(pid))
    assert P.frontmost_app().pid != pid  # never brought to the front


def test_screen_recording_status_matches_title_visibility(browser):
    """Without Screen Recording, other apps' window titles are withheld by CoreGraphics."""
    _, pid = browser
    visible = any(TITLE in t for t in P.cg_window_titles(pid))
    assert visible == (P.screen_recording() == P.Tcc.GRANTED)


@pytest.mark.skipif(P.accessibility() != P.Tcc.GRANTED, reason="Accessibility not granted")
def test_web_ax_tree_is_available_without_manual_accessibility(browser):
    """Chrome 154 / Brave 151: the web tree is built on demand for an AX client; AXManualAccessibility is gone.
    The first query can come back empty (measured: populated ~0.2 s later), so poll briefly instead."""
    name, pid = browser
    end, labels = time.monotonic() + 2, []
    while not any("FERB-PROBE-BUTTON" in l for l in labels):
        assert time.monotonic() < end, f"{name}: web AX tree still empty after 2 s"
        labels = [label for _, label in P.web_elements(pid)]
        time.sleep(0.05)
    assert any("FERB-PROBE-FIELD" in l for l in labels)
    assert P.set_manual_accessibility(pid) == AX_ATTRIBUTE_UNSUPPORTED, \
        f"{name}: AXManualAccessibility is supported again — revisit ARCHITECTURE 9.2"


def test_automation_status_for_running_browser(browser):
    name, _ = browser
    bundle_id, _ = BROWSERS[name]
    assert P.automation(bundle_id) in (P.Tcc.GRANTED, P.Tcc.DENIED, P.Tcc.NOT_DETERMINED)
