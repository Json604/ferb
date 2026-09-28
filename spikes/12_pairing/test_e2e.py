"""Spike 12 end to end in the owner's real Chrome, test profiles "FERB Test 1" and "FERB Test 2" only (no Brave).

Needs: Chrome running; both profiles exist; "FERB spike 12" loaded unpacked in both. Opens the extension's own
pairing page in each profile; windows only appear in those two profiles.
"""

import http.server
import json
import secrets
import subprocess
import sys
import threading
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "10_profiles"))
from profiles import discover, resolve  # noqa: E402

from daemon_stub import EXT_ID, Daemon, install_host, uninstall_host  # noqa: E402

CHROME = "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"
NAMES = ("FERB Test 1", "FERB Test 2")
state: dict = {}


@pytest.fixture(scope="module")
def folders():
    known = discover("chrome")
    out = {}
    for name in NAMES:
        r = resolve(name, known, browser="chrome")
        if r.kind != "one":
            pytest.skip(r.message(name, known))
        out[name] = r.candidates[0].folder
    return out


@pytest.fixture(scope="module")
def daemon():
    install_host()
    d = Daemon()
    yield d
    d.close()
    uninstall_host()


@pytest.fixture(scope="module")
def page():
    class H(http.server.BaseHTTPRequestHandler):
        def do_GET(self):
            self.send_response(200)
            self.send_header("content-type", "text/html")
            self.end_headers()
            self.wfile.write(b"<title>ferb spike 12</title>")

        def log_message(self, *a):
            pass

    srv = http.server.ThreadingHTTPServer(("127.0.0.1", 0), H)
    threading.Thread(target=srv.serve_forever, daemon=True).start()
    yield f"http://127.0.0.1:{srv.server_port}/"
    srv.shutdown()


def chrome_running() -> bool:
    return subprocess.run(["pgrep", "-x", "Google Chrome"], capture_output=True).returncode == 0


def open_in(folder: str, url: str) -> None:
    """Hand a URL to the owner's *running* Chrome. Never starts Chrome, never waits on or kills the launcher:
    if Chrome isn't running the launcher would become the browser itself (and a timeout would kill it)."""
    if not chrome_running():
        pytest.fail("Chrome is not running: start it yourself, then rerun (the harness never starts or kills it)")
    subprocess.Popen([CHROME, f"--profile-directory={folder}", url], stdout=subprocess.DEVNULL,
                     stderr=subprocess.DEVNULL, start_new_session=True)


def pair_url(token: str) -> str:
    return f"chrome-extension://{EXT_ID}/pair.html#token={token}"


def test_pair_each_profile_and_binding_matches_the_token(daemon, folders):
    for name, folder in folders.items():
        token = daemon.pairing.issue_token("chrome", folder)
        state.setdefault("tokens", {})[name] = token
        open_in(folder, pair_url(token))
        ev = daemon.wait(lambda e: e.verdict.reason == "paired" and e.verdict.binding == ("chrome", folder), 60)
        state.setdefault("install", {})[name] = ev.msg["install_id"]
        ack = daemon.wait(lambda e: e.msg["type"] == "paired_ack" and e.msg["install_id"] == ev.msg["install_id"])
        assert ack.verdict.accepted and ack.verdict.binding == ("chrome", folder)   # key works
    assert len(set(state["install"].values())) == 2   # one install_id per profile


def test_signed_event_is_attributed_to_its_profile(daemon, folders, page):
    a = state["install"]["FERB Test 1"]
    marker = page + "?m=" + secrets.token_hex(4)
    daemon.act(a, {"type": "open", "id": 1, "url": marker})
    ev = daemon.wait(lambda e: e.msg["type"] == "tab_event" and marker in e.msg["body_json"])
    assert ev.verdict.accepted and ev.msg["install_id"] == a
    assert ev.verdict.binding == ("chrome", folders["FERB Test 1"])


def test_reusing_a_token_is_refused(daemon, folders):
    open_in(folders["FERB Test 2"], pair_url(state["tokens"]["FERB Test 2"]))
    ev = daemon.wait(lambda e: e.msg["type"] == "pair" and e.msg["install_id"] == state["install"]["FERB Test 2"])
    assert (ev.verdict.accepted, ev.verdict.reason) == (False, "unknown_token")


def test_unpaired_install_cannot_send_events_or_receive_actions(daemon, folders, page):
    b_old = state["install"]["FERB Test 2"]
    daemon.send_raw(b_old, {"type": "forget_pairing"})   # extension drops install_id + key (fresh install)
    marker = page + "?u=" + secrets.token_hex(4)
    open_in(folders["FERB Test 2"], marker)
    ev = daemon.wait(lambda e: e.msg["type"] == "tab_event" and marker in e.msg["body_json"])
    assert ev.msg["install_id"] != b_old
    assert (ev.verdict.accepted, ev.verdict.reason) == (False, "unpaired_install")
    with pytest.raises(PermissionError):
        daemon.act(ev.msg["install_id"], {"type": "open", "id": 2, "url": page})
