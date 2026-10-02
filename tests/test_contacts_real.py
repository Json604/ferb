"""Build step 16a, real (pytest -m real; Codex only): contacts are never guessed, on a local fake chat page in the
Chrome "FERB Test" profile (never FERB's own profile, never the owner's real apps or accounts).

A test daemon with its own home; FERB's native-host launcher points at it for the run and back at the installed
daemon (~/.ferb) afterwards. Only "FERB Test" is paired to the test daemon.
1. One contact matches "Rahul" -> the call goes ahead with the countdown naming "Rahul Verma (+91 98200 22222)",
   and the page records that call.
2. Two contacts match -> FERB asks "Which Rahul did you mean?" with both; the owner picks 1 -> Rahul Sharma is
   called, not the other.
"""

import dataclasses
import http.server
import os
import re
import shutil
import subprocess
import tempfile
import threading
import time
from pathlib import Path
from typing import Any, Iterator
from urllib.parse import parse_qs, urlsplit

import pytest

from ferb.cli.chat import render
from ferb.cli.client import Client
from ferb.clock.clock import SystemClock
from tests.conftest import Answers, RunningDaemon, without_owner_mac

pytestmark = pytest.mark.real
LAUNCHER = Path.home() / ".ferb" / "bin" / "ferb-native-host"
INSTALLED = Path.home() / ".ferb"
LINK: list[tuple[str, str, str]] = []
PEOPLE = {"one": [("Rahul Verma", "+91 98200 22222", "Gym"), ("Priya Nair", "+91 98300 33333", "Family")],
          "two": [("Rahul Sharma", "+91 98100 11111", "Work"), ("Rahul Verma", "+91 98200 22222", "Gym")]}
PAGE = """<!doctype html><meta charset=utf-8><title>Chat</title>
<h1>Chats</h1><div id=list>%s</div>
<div id=chat hidden><h2 id=who></h2><p id=detail></p>
<button type=button id=call onclick="fetch('/called?who=' + encodeURIComponent(current));
  document.getElementById('status').textContent = 'Calling ' + current">Video call</button>
<p id=status></p></div>
<script>let current = '';
function open_(n, d) { current = n; document.getElementById('chat').hidden = false;
  document.getElementById('who').textContent = n; document.getElementById('detail').textContent = d; }</script>"""
COMMENTS = ("<!doctype html><meta charset=utf-8><title>Video</title><h1>A video</h1><h2>Comments</h2>" +
            "".join(f"<p style='margin:40px 0'>Comment {i}: {'great video' if i < 60 else 'the last one'}</p>"
                    for i in range(1, 61)) +
            "<script>let t=0;addEventListener('scroll',()=>{clearTimeout(t);t=setTimeout(()=>"
            "fetch('/scrolled?y='+Math.round(scrollY)),200)})</script>")
ROW = """<button type=button onclick="open_('{n}', '{d}')">{n} · {d} · {s}</button><br>"""


@pytest.fixture(scope="module")
def site() -> Iterator[tuple[str, list[str]]]:
    called: list[str] = []

    class H(http.server.BaseHTTPRequestHandler):
        def do_GET(self) -> None:  # noqa: N802
            u = urlsplit(self.path)
            if u.path == "/called":
                called.append(parse_qs(u.query).get("who", [""])[0])
                body = b"ok"
            elif u.path == "/scrolled":                             # test_tabs_real: how far the owner's tab scrolled
                called.append("scrolled:" + parse_qs(u.query).get("y", ["0"])[0])
                body = b"ok"
            elif u.path == "/comments":
                body = COMMENTS.encode()
            else:
                rows = "".join(ROW.format(n=n, d=d, s=s) for n, d, s in PEOPLE.get(u.path.strip("/"), []))
                body = (PAGE % rows).encode()
            self.send_response(200)
            self.send_header("content-type", "text/html; charset=utf-8")
            self.end_headers()
            self.wfile.write(body)

        def log_message(self, *a: Any) -> None:
            pass
    srv = http.server.ThreadingHTTPServer(("127.0.0.1", 0), H)
    threading.Thread(target=srv.serve_forever, daemon=True).start()
    yield f"http://127.0.0.1:{srv.server_port}", called
    srv.shutdown()


def restart_hosts() -> None:
    """Only FERB's own relay processes; extensions reconnect through the launcher within seconds."""
    subprocess.run(["pkill", "-f", "ferb.native_host.main"], capture_output=True)
    time.sleep(3)


@pytest.fixture(scope="module")
def rd(site: tuple[str, list[str]]) -> Iterator[RunningDaemon]:
    spec = os.environ.get("FERB_REAL_PROFILE", "chrome:FERB Test")
    browser, name = spec.split(":", 1)
    if name != "FERB Test":
        pytest.skip("this test only runs in the \"FERB Test\" profile")
    if not LAUNCHER.exists():
        pytest.skip("run `ferb setup --update` first (installs the native host)")
    base = Path(tempfile.mkdtemp(prefix="fct-", dir="/tmp"))
    d = RunningDaemon(base / "home", clock=SystemClock())
    d.daemon.config.set("runtimes", "orchestrator", "codex")
    d.daemon.config.set("runtimes", "worker", "codex")
    d.daemon.config.set("gates", "owner_profiles", True)
    without_owner_mac(d)                                           # locked down (CLAUDE.md): the owner's Mac stays out
    for cap in d.daemon.registry.capabilities.values():            # local pages for the owner-browser tools (test)
        if cap.name.startswith("browser.") and hasattr(cap, "family"):
            cap.family = dataclasses.replace(cap.family, allow_local=True)
    text = LAUNCHER.read_text()
    LAUNCHER.write_text(re.sub(r'FERB_HOME="[^"]*"', f'FERB_HOME="{d.daemon.home}"', text))
    restart_hosts()
    try:
        from ferb.act.browser.profiles import discover_all, open_known_profile
        p = next(p for p in discover_all() if p.display_name == name and p.browser == browser)
        open_known_profile(browser, p.folder, f"{site[0]}/blank", discover_all())
        time.sleep(2)
        c = Client(d.sock, timeout=90)
        r = c.call("browsers.pair", {"profile": name, "browser": browser, "timeout_s": 60})
        c.close()
        assert r["paired"], r
        d.run(_reload(d))
        real_pub = d.daemon.hub.publish                            # connection events, for failure messages

        def pub(topic: str, data: Any) -> None:
            if topic.startswith("browser."):
                LINK.append((time.strftime("%H:%M:%S"), topic, str(data)[:80]))
            real_pub(topic, data)
        d.daemon.hub.publish = pub
        for br in d.daemon.hub.bridges.values():
            br.publish = pub
        d.daemon.supervisor.apps = lambda: [("com.google.Chrome", "Google Chrome")]   # type: ignore[union-attr]
        yield d
    finally:                                                          # the installed daemon gets its browsers back
        LAUNCHER.write_text(re.sub(r'FERB_HOME="[^"]*"', f'FERB_HOME="{INSTALLED}"', LAUNCHER.read_text()))
        restart_hosts()
        d.stop()
        shutil.rmtree(base, ignore_errors=True)


async def _reload(d: RunningDaemon) -> None:
    d.daemon._load_config()


def chat(d: RunningDaemon, text: str) -> list[dict[str, Any]]:
    c = Client(d.sock, timeout=900)
    try:
        return list(c.stream("chat.send", {"session_id": "s", "text": text}))
    finally:
        c.close()


def problems(d: RunningDaemon, since: str = "") -> list[Any]:
    async def go() -> list[Any]:
        out: list[Any] = [e.data for e in d.daemon._journal().entries() if e.kind in ("task.repair", "chat.failed")]
        out += [(r["input_json"][:300], (r["output_json"] or "")[:200], r["error"]) for r in d.daemon.db.conn.execute(
            "SELECT input_json, output_json, error FROM spans WHERE kind = 'tool.call' AND started_at >= ? ORDER BY started_at",
            (since,))]
        return out + [("links", LINK[-12:])]
    return d.run(go())


def shown(evs: list[dict[str, Any]]) -> str:
    return "\n".join(line for e in evs if (line := render(e)))


def test_one_matching_contact_is_called_with_the_countdown(rd: RunningDaemon, site: tuple[str, list[str]]) -> None:
    url, called = site
    called.clear()
    watch = Answers(rd, choice=None)
    try:
        evs = chat(rd, f"in my FERB Test profile, open {url}/one and video call Rahul")
    finally:
        watch.stop()
    out = shown(evs)
    assert re.search(r"📞 Calling Rahul Verma \(\+91 98200 22222[^)]*\) in 10 s", out), (out, problems(rd))  # detail as shown
    assert called == ["Rahul Verma"], (called, out)
    assert not [ap for ap in watch.asked if ap.options], "one match: nothing to ask"


def test_two_matching_contacts_are_asked_about(rd: RunningDaemon, site: tuple[str, list[str]]) -> None:
    url, called = site
    called.clear()
    since = rd.daemon.clock.now().isoformat()
    pick = Answers(rd, choice=1)
    try:
        evs = chat(rd, f"in my FERB Test profile, open {url}/two and video call Rahul")
    finally:
        pick.stop()
    asked = [ap for ap in pick.asked if ap.options]
    assert asked and asked[0].reason == "Which Rahul did you mean?", (pick.asked, called, shown(evs), problems(rd, since))
    assert {o.split(" · ")[0] for o in asked[0].options} == {"Rahul Sharma", "Rahul Verma"}, asked[0].options
    first = asked[0].options[0].split(" · ")[0]
    assert called == [first], (called, shown(evs), problems(rd, since))                     # the one the owner picked, nobody else
