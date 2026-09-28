"""Spike 10: opening named profiles in a running browser, and the External Extensions offer per profile.

Default run uses spare --user-data-dir folders only. The real-Chrome check (FERB-only profile) runs with FERB_REAL=1.
"""

import http.server
import os
import secrets
import subprocess
import tempfile
import threading
import time
from pathlib import Path

import pytest

import open_profile as O
from external_ext import profile_state, write_external
from profiles import discover, resolve


@pytest.fixture(scope="module")
def url():
    class H(http.server.BaseHTTPRequestHandler):
        def do_GET(self):
            self.send_response(200)
            self.send_header("content-type", "text/html")
            self.end_headers()
            self.wfile.write(f"<title>{self.path.strip('/')}</title>".encode())

        def log_message(self, *a):
            pass

    srv = http.server.ThreadingHTTPServer(("127.0.0.1", 0), H)
    threading.Thread(target=srv.serve_forever, daemon=True).start()
    yield lambda token: f"http://127.0.0.1:{srv.server_port}/{token}"
    srv.shutdown()


def spare(browser: str, *args: str) -> tuple[Path, list[str]]:
    d = Path(tempfile.mkdtemp(prefix="ferb10t-"))
    return d, [O.BINARIES[browser], f"--user-data-dir={d}", "--no-first-run", "--no-default-browser-check", *args]


def run(cmd: list[str]) -> None:
    subprocess.run(cmd, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=30)


@pytest.mark.parametrize("browser", ["chrome", "brave"])
def test_named_profile_opens_in_running_instance_and_only_known_folders(browser, url):
    d, base = spare(browser)
    # Session 1: create a second profile. Local State is written lazily (on quit), so discover after quitting.
    p = subprocess.Popen(base + ["about:blank"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    time.sleep(4)
    run(base + ["--profile-directory=Profile 1", "about:blank"])
    time.sleep(2)
    p.terminate()
    p.wait(timeout=15)
    known = discover(browser, d)
    # Session 2: browser running, open the named profile into it.
    first = "ferb-" + secrets.token_hex(4)
    p = subprocess.Popen(base + [url(first)], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    try:
        assert O.wait_window(p.pid, first)
        assert {x.folder for x in known} >= {"Default", "Profile 1"}
        target = resolve("Person 1", known).candidates[0]
        token = "ferb-" + secrets.token_hex(4)
        took = O.open_known_profile(browser, target.folder, url(token), known, user_data_dir=str(d))
        assert O.wait_window(p.pid, token), "window didn't open in the running instance"
        assert took < 5 and p.poll() is None                   # forwarded; the running instance stays
        with pytest.raises(O.UnknownProfile):
            O.open_known_profile(browser, "Profile 9", url(token), known, user_data_dir=str(d))
        assert "Profile 9" not in {x.folder for x in discover(browser, d)}
    finally:
        p.terminate()
        p.wait(timeout=15)


@pytest.mark.parametrize("browser", ["chrome", "brave"])
def test_external_extension_is_offered_disabled_in_every_profile(browser):
    """File in <user-data-dir>/External Extensions -> each opened profile gets the extension, disabled (8192,
    external install) until the owner accepts the prompt."""
    d, base = spare(browser)
    p = subprocess.Popen(base + ["about:blank"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    time.sleep(4)
    run(base + ["--profile-directory=Profile 1", "about:blank"])
    time.sleep(3)
    p.terminate()
    p.wait(timeout=15)
    write_external(d / "External Extensions")
    p = subprocess.Popen(base + ["--profile-directory=Profile 1", "about:blank"],
                         stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    time.sleep(3)
    run(base + ["--profile-directory=Default", "about:blank"])
    time.sleep(40)  # startup providers + Web Store download + prefs flush
    p.terminate()
    p.wait(timeout=15)
    for prof in ("Default", "Profile 1"):
        state = profile_state(d, prof)
        assert state is not None, f"{browser}/{prof} never saw the extension"
        assert state["disable_reasons"] == [8192], f"{browser}/{prof}: {state}"


@pytest.mark.skipif(os.environ.get("FERB_REAL") != "1", reason="touches the owner's real Chrome; set FERB_REAL=1")
def test_real_chrome_ferb_profile_opens_and_closes_exact_window(url):
    known = discover("chrome")
    r = resolve("FERB", known, browser="chrome")
    assert r.kind == "one"
    pid = O.running_pid("chrome")
    if pid is None:
        pytest.skip("Chrome not running")
    token = "ferb-" + secrets.token_hex(4)
    O.open_known_profile("chrome", r.candidates[0].folder, url(token), known)
    w = O.wait_window(pid, token)
    assert w is not None and O.running_pid("chrome") == pid
    O.close_window(w)
    time.sleep(1)
    assert not any(token in O.title(x) for x in O.windows(pid))
