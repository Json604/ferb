"""Shared fixtures for browser tests: local test pages (two origins on one server), an Atom feed standing in for
FERB's inbox, and a temporary keychain holding a test password (created and deleted here; the owner's keychains
and the search list are never touched)."""

import asyncio
import http.server
import secrets
import subprocess
import threading
from datetime import datetime
from pathlib import Path
from typing import Any, Iterator

import pytest

from ferb.core.types import SecretRef

PASSWORD = "Corr3ct-Horse-" + secrets.token_hex(4)
REF = SecretRef("test/site/password")

LOGIN = b"""<title>Login</title><form method=post action=/welcome>
<input id=username name=username><input id=password name=password type=password>
<button type=submit>Sign in</button></form><a href=/search>Search page</a>"""
STEAL_TOP = b"""<input id=password type=password onfocus="document.getElementById('f').focus()">
<iframe id=f src="http://127.0.0.1:%d/steal_frame"></iframe>"""
STEAL_FRAME = b"""<input id=grab><script>window.onfocus=()=>document.getElementById('grab').focus()</script>"""
SEARCH = b"""<form role=search action=/results><input name=q><button>Go</button></form>
<form action=/signup><input name=email><input name=nick><button>Join</button></form>
<input type=hidden name=csrf value=HIDDEN-TOKEN>"""
OTP = b"""<form method=post action=/welcome><input id=code name=otp autocomplete=one-time-code>
<button>Verify</button></form>"""


def feed(entries: list[tuple[str, str, str, datetime]], address: str = "ferb@test.example") -> bytes:
    items = "".join(f"<entry><title>{t}</title><summary>{s}</summary><issued>{d.strftime('%Y-%m-%dT%H:%M:%SZ')}"
                    f"</issued><id>tag:mail,{secrets.token_hex(4)}</id><author><email>{a}</email></author></entry>"
                    for a, t, s, d in entries)
    return (f'<?xml version="1.0" encoding="UTF-8"?><feed version="0.3" xmlns="http://purl.org/atom/ns#">'
            f"<title>Gmail - Inbox for {address}</title><fullcount>{len(entries)}</fullcount>{items}</feed>").encode()


class Loop:
    def __init__(self) -> None:
        self.loop = asyncio.new_event_loop()
        threading.Thread(target=self.loop.run_forever, daemon=True).start()

    def run(self, coro: Any, timeout: float = 60) -> Any:
        return asyncio.run_coroutine_threadsafe(coro, self.loop).result(timeout)


@pytest.fixture(scope="module")
def loop() -> Iterator[Loop]:
    lp = Loop()
    yield lp
    lp.loop.call_soon_threadsafe(lp.loop.stop)


@pytest.fixture(scope="module")
def keychain(tmp_path_factory: pytest.TempPathFactory) -> Iterator[Path]:
    path = tmp_path_factory.mktemp("kc") / "ferb-test.keychain-db"
    before = subprocess.run(["security", "list-keychains"], capture_output=True, text=True).stdout
    kc_pass = secrets.token_hex(16)
    subprocess.run(["security", "create-keychain", "-p", kc_pass, str(path)], check=True)
    try:
        subprocess.run(["security", "unlock-keychain", "-p", kc_pass, str(path)], check=True)
        subprocess.run(["security", "add-generic-password", "-s", "ferb", "-a", REF.key, "-w", PASSWORD, str(path)],
                       check=True)
        yield path
    finally:
        subprocess.run(["security", "delete-keychain", str(path)], check=True)
        after = subprocess.run(["security", "list-keychains"], capture_output=True, text=True).stdout
        assert after == before, "temporary keychain left the search list changed"


@pytest.fixture(scope="module")
def site() -> Iterator[dict[str, Any]]:
    state: dict[str, Any] = {"feed": feed([]), "posted": []}

    class H(http.server.BaseHTTPRequestHandler):
        def do_GET(self) -> None:  # noqa: N802
            port = self.server.server_port
            path = self.path.split("?")[0]
            body = {"/login": LOGIN, "/top": b'<iframe src="http://127.0.0.1:%d/login"></iframe>' % port,
                    "/steal_top": STEAL_TOP % port, "/steal_frame": STEAL_FRAME, "/search": SEARCH, "/otp": OTP,
                    "/results": b"<p>results page</p>", "/feed": state["feed"]}.get(path, b"<p>blank</p>")
            self.send_response(200)
            self.send_header("content-type", "text/xml" if path == "/feed" else "text/html")
            self.end_headers()
            self.wfile.write(body)

        def do_POST(self) -> None:  # noqa: N802
            state["posted"].append(self.rfile.read(int(self.headers["content-length"])).decode())
            self.send_response(200)
            self.send_header("content-type", "text/html")
            self.end_headers()
            self.wfile.write(b"<p>Welcome back</p>")

        def log_message(self, *a: Any) -> None:
            pass

    srv = http.server.ThreadingHTTPServer(("127.0.0.1", 0), H)
    threading.Thread(target=srv.serve_forever, daemon=True).start()
    state["port"] = srv.server_port
    state["L"] = f"http://localhost:{srv.server_port}"
    state["I"] = f"http://127.0.0.1:{srv.server_port}"
    yield state
    srv.shutdown()


