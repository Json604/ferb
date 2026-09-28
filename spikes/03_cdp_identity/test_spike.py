"""Spike 3 stage 1: CDP over a pipe to a FERB-owned Chrome profile; origin-bound fill_secret.

Real Chrome (headless), a temporary keychain (created and deleted here), and the public practice site
https://the-internet.herokuapp.com/login with its published test credentials.
"""

import http.server
import secrets
import subprocess
import threading
import time
from pathlib import Path

import pytest

from cdp_pipe import ChromePipe, Page
from fill import FillTarget, fill_secret
from secrets_kc import KeychainSecretStore, SecretRef, SecretSpec

PRACTICE = "https://the-internet.herokuapp.com"
PRACTICE_USER, PRACTICE_PASSWORD = "tomsmith", "SuperSecretPassword!"  # published on the login page
REF = SecretRef("ferb/practice/password")


# ---------- fixtures ----------

@pytest.fixture(scope="module")
def keychain(tmp_path_factory):
    path = tmp_path_factory.mktemp("kc") / "ferb-spike.keychain-db"
    before = subprocess.run(["security", "list-keychains"], capture_output=True, text=True).stdout
    kc_pass = secrets.token_hex(16)
    subprocess.run(["security", "create-keychain", "-p", kc_pass, str(path)], check=True)
    try:
        subprocess.run(["security", "unlock-keychain", "-p", kc_pass, str(path)], check=True)
        subprocess.run(["security", "add-generic-password", "-s", "ferb", "-a", REF.key,
                        "-w", PRACTICE_PASSWORD, str(path)], check=True)
        yield path
    finally:
        subprocess.run(["security", "delete-keychain", str(path)], check=True)
        after = subprocess.run(["security", "list-keychains"], capture_output=True, text=True).stdout
        assert after == before, "temporary keychain left the search list changed"


def store_for(keychain: Path, *origins: str) -> KeychainSecretStore:
    return KeychainSecretStore([SecretSpec(REF, list(origins), "practice")], keychain)


@pytest.fixture(scope="module")
def chrome(tmp_path_factory):
    c = ChromePipe(tmp_path_factory.mktemp("ferb-identity"))
    yield c
    c.close()


@pytest.fixture
def page(chrome):
    return Page(chrome)


FORM = b"""<form><input id=username><input id=password type=password></form>"""
STEAL_TOP = b"""<input id=password type=password
  onfocus="document.getElementById('f').focus()">
<iframe id=f src="http://127.0.0.1:%d/steal_frame"></iframe>"""
STEAL_FRAME = b"""<input id=grab><script>window.onfocus=()=>document.getElementById('grab').focus()</script>"""


@pytest.fixture(scope="module")
def local():
    """One server; http://localhost:P and http://127.0.0.1:P are different origins (and sites)."""
    class H(http.server.BaseHTTPRequestHandler):
        def do_GET(self):
            port = self.server.server_port
            body = {"/form": FORM,
                    "/top": b'<iframe src="http://127.0.0.1:%d/form"></iframe>' % port,
                    "/steal_top": STEAL_TOP % port,
                    "/steal_frame": STEAL_FRAME}.get(self.path, b"")
            self.send_response(200)
            self.send_header("content-type", "text/html")
            self.end_headers()
            self.wfile.write(body)

        def log_message(self, *a):
            pass

    srv = http.server.ThreadingHTTPServer(("127.0.0.1", 0), H)
    threading.Thread(target=srv.serve_forever, daemon=True).start()
    yield srv.server_port
    srv.shutdown()


# ---------- helpers ----------

def main_frame(page: Page) -> str:
    return page.send("Page.getFrameTree")["frameTree"]["frame"]["id"]


def frame_where(page: Page, url_part: str) -> tuple[str, str]:
    for _ in range(50):
        for fid, (session, info) in page.frames().items():
            if url_part in info.get("url", "") and info.get("securityOrigin"):
                return fid, session
        time.sleep(0.1)
    raise AssertionError(f"no frame with {url_part}")


def value(page: Page, selector: str, session: str | None = None) -> str:
    return page.eval(f"document.querySelector('{selector}').value", session)


# ---------- tests ----------

def test_cdp_is_over_pipe_with_no_port(chrome, tmp_path_factory):
    assert chrome.send("Browser.getVersion")["product"].startswith(("Chrome/", "HeadlessChrome/"))
    pids = subprocess.run(["pgrep", "-f", "--", "--remote-debugging-pipe"], capture_output=True,
                          text=True).stdout.split()
    assert str(chrome.proc.pid) in pids
    listening = subprocess.run(["lsof", "-a", "-p", ",".join(pids), "-iTCP", "-sTCP:LISTEN"],
                               capture_output=True, text=True).stdout
    assert listening == ""


def test_login_on_practice_site(page, keychain, chrome):
    page.goto(f"{PRACTICE}/login")
    page.eval(f"document.querySelector('#username').value = '{PRACTICE_USER}'")
    r = fill_secret(page, FillTarget(main_frame(page), "#password"), REF, store_for(keychain, PRACTICE))
    assert (r.ok, r.reason, r.audit_key) == (True, "ok", REF.key)
    page.eval("document.querySelector('button[type=submit]').click()")
    page.chrome.wait_event("Page.loadEventFired", page.session)
    assert "You logged into a secure area!" in page.eval("document.body.innerText")
    assert PRACTICE_PASSWORD not in repr(r) + " ".join(chrome.log)


def test_refuses_lookalike_origin(page, keychain, local):
    page.goto(f"http://127.0.0.1:{local}/form")
    r = fill_secret(page, FillTarget(main_frame(page), "#password"), REF,
                    store_for(keychain, f"http://localhost:{local}"))
    assert (r.ok, r.reason) == (False, "origin_not_allowed")
    assert value(page, "#password") == ""


def test_refuses_scheme_mismatch(page, keychain, local):
    page.goto(f"http://127.0.0.1:{local}/form")
    r = fill_secret(page, FillTarget(main_frame(page), "#password"), REF,
                    store_for(keychain, f"https://127.0.0.1:{local}"))
    assert (r.ok, r.reason) == (False, "origin_not_allowed")
    assert value(page, "#password") == ""


def test_refuses_cross_origin_iframe(page, keychain, local):
    """Top page is allowed; the password field lives in a frame from another origin."""
    page.goto(f"http://localhost:{local}/top")
    store = store_for(keychain, f"http://localhost:{local}")
    fid, session = frame_where(page, "/form")
    r = fill_secret(page, FillTarget(fid, "#password"), REF, store)
    assert (r.ok, r.reason) == (False, "origin_not_allowed")
    r = fill_secret(page, FillTarget(main_frame(page), "#password"), REF, store)  # doesn't pierce frames
    assert (r.ok, r.reason) == (False, "no_element")
    assert value(page, "#password", session) == ""


def test_focus_stealing_cannot_redirect_secret(page, keychain, local):
    """The allowed page moves focus into an attacker frame; the secret still lands only in the target."""
    page.goto(f"http://localhost:{local}/steal_top")
    fid, session = frame_where(page, "/steal_frame")
    page.eval("document.querySelector('#password').focus()")
    assert page.eval("document.activeElement.id") == "f"  # focus really was stolen into the frame
    r = fill_secret(page, FillTarget(main_frame(page), "#password"), REF,
                    store_for(keychain, f"http://localhost:{local}"))
    assert r.ok
    assert value(page, "#grab", session) == ""
    assert value(page, "#password") == PRACTICE_PASSWORD


def test_unknown_secret_refused(page, keychain):
    r = fill_secret(page, FillTarget(main_frame(page), "#password"), SecretRef("ferb/nope"),
                    store_for(keychain, PRACTICE))
    assert (r.ok, r.reason) == (False, "unknown_secret")


def test_control_iframe_fill_works_when_its_origin_is_allowed(page, keychain, local):
    """Proves the iframe refusal is the origin check, not failure to reach the cross-origin frame."""
    page.goto(f"http://localhost:{local}/top")
    fid, session = frame_where(page, "/form")
    assert session != page.session  # cross-origin frame runs in its own (out-of-process) target
    r = fill_secret(page, FillTarget(fid, "#password"), REF, store_for(keychain, f"http://127.0.0.1:{local}"))
    assert r.ok
    assert value(page, "#password", session) == PRACTICE_PASSWORD
