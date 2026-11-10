"""Identity transport integration in disposable ChromeLab, temporary keychain and local pages only.
Origin confinement, sealed password/code delivery and replay refusal use the real extension. Provider account
extractors and compose UI run against representative local DOM; actual Gmail/YouTube login and email delivery
are owner-supervised acceptance checks, never automatic tests against installed browser profiles."""

import asyncio
import http.server
import json
import os
import tempfile
import threading
import time
from datetime import datetime, timedelta, timezone
from pathlib import Path
from typing import Any, Iterator
from urllib.parse import parse_qs

import pytest

from ferb.core.errors import DriverError, Refused
from ferb.core.types import BrowserAction, SecretRef, SecretSpec
from ferb.identity.secrets import KeychainSecretStore
from tests.browserkit import ADDRESS, PASSWORD, REF, feed
from tests.conftest import RunningDaemon
from tests.chromelab import ChromeLab, browser_lab, wait_for

pytestmark = pytest.mark.real

LOGIN = b"""<!doctype html><title>Login</title><form method=post action=/welcome>
<input id=user name=user placeholder=User><input id=pw type=password name=pw placeholder=Password>
<button>Sign in</button></form>"""
STEAL = b"""<!doctype html><title>Steal</title><input id=pw type=password placeholder=Password
  onfocus="document.getElementById('grab').focus()"><input id=grab placeholder=Grab>"""
OTP = b"""<!doctype html><title>Code</title><form method=post action=/welcome>
<input id=code name=otp autocomplete=one-time-code placeholder=Code><button>Verify</button></form>"""


class Site:
    def __init__(self) -> None:
        self.posted: list[dict[str, list[str]]] = []
        site = self

        class H(http.server.BaseHTTPRequestHandler):
            def do_GET(self) -> None:  # noqa: N802
                port = self.server.server_port
                body = {"/login": LOGIN, "/steal": STEAL, "/otp": OTP,
                        "/top": b'<!doctype html><iframe src="http://127.0.0.1:%d/login" width=600 height=300></iframe>'
                        % port}.get(self.path.split("?")[0], b"<p>blank</p>")
                if self.path.startswith("/compose?"):
                    body = b"""<button onclick="fetch('/sent'+location.search).then(()=>document.body.textContent='Message sent')">Send</button>"""
                elif self.path.startswith("/sent?"):
                    site.posted.append(parse_qs(self.path.split("?", 1)[1]))
                    body = b"sent"
                self.send_response(200)
                self.send_header("content-type", "text/html")
                self.end_headers()
                self.wfile.write(body)

            def do_POST(self) -> None:  # noqa: N802
                site.posted.append(parse_qs(self.rfile.read(int(self.headers["content-length"])).decode()))
                self.send_response(200)
                self.send_header("content-type", "text/html")
                self.end_headers()
                self.wfile.write(b"<p>Welcome back</p>")

            def log_message(self, *a: Any) -> None:
                pass
        self.srv = http.server.ThreadingHTTPServer(("127.0.0.1", 0), H)
        threading.Thread(target=self.srv.serve_forever, daemon=True).start()
        self.L = f"http://localhost:{self.srv.server_port}"
        self.I = f"http://127.0.0.1:{self.srv.server_port}"


def tee(d: RunningDaemon, chrome: ChromeLab) -> list[str]:
    """Record every line crossing the native host, both directions, exactly as sent (safety test 21)."""
    wire: list[str] = []
    hub = d.daemon.hub
    assert hub is not None
    serve = hub.serve

    async def tapped(reader: asyncio.StreamReader, writer: asyncio.StreamWriter) -> None:
        class R:
            async def readline(self) -> bytes:
                line = await reader.readline()
                wire.append(line.decode(errors="replace"))
                return line

        class W:
            def write(self, data: bytes) -> None:
                wire.append(data.decode(errors="replace"))
                writer.write(data)

            async def drain(self) -> None:
                await writer.drain()
        await serve(R(), W())                                                   # type: ignore[arg-type]
    d.daemon.server.native = tapped
    old_gen = hub.bridges[("chrome", "Default")].gen
    chrome.reconnect()  # only this disposable extension reconnects through the tap
    wait_for(lambda: hub.bridges[("chrome", "Default")].gen > old_gen, 30, "lab relay did not reconnect")
    return wire


@pytest.fixture(scope="module")
def stand_in() -> Iterator[Any]:
    with browser_lab(ferb_identity=True) as isolated:
        yield isolated


def test_stand_in_identity_origin_rules_and_sealing(stand_in: Any) -> None:
    rd, chrome, _ = stand_in
    d = rd.daemon
    site = Site()
    wire = tee(rd, chrome)
    specs: dict[str, SecretSpec] = {}
    fi = d.ferb_identity
    assert fi is not None and d.pairings is not None
    kc = _keychain()
    store = KeychainSecretStore(lambda: specs, kc)
    fi.secrets, fi._store = store, store
    scrubbed: list[str] = []                       # every Keychain read of a value
    reveal = store._reveal

    async def counted(ref: Any) -> str:
        value = await reveal(ref)
        scrubbed.append(value)
        return value
    store._reveal = counted                        # type: ignore[method-assign]
    try:
        assert rd.run(_attach(d)), "the disposable identity profile did not attach"
        out = rd.run(_part1(d, site, specs, scrubbed), timeout=240)
    finally:
        _drop_keychain(kc)
        site.srv.shutdown()
    assert out["login_ok"] and site.posted and site.posted[0]["pw"] == [PASSWORD]
    for case in ("lookalike", "scheme", "iframe"):
        assert "isn't allowed" in out[case], (case, out[case])
    assert out["refused_before_keychain"]
    assert out["iframe_allowed_ok"] and site.posted[-1]["pw"] == [PASSWORD]
    assert out["steal_ok"] and out["grab_value"] == ""
    assert out["replay"] == "seal_refused"
    assert "only for FERB's inbox feed" in out["read_source"]
    assert wire and not any(PASSWORD in line for line in wire), "the password crossed the native host in clear"


async def _attach(d: Any) -> bool:
    return bool(d._attach_identity_if_paired())


async def _part1(d: Any, site: Site, specs: dict[str, SecretSpec], scrubbed: list[str]) -> dict[str, Any]:
    fi = d.ferb_identity
    out: dict[str, Any] = {}

    def allow(*origins: str) -> None:
        specs[REF.key] = SecretSpec(REF, list(origins), "test account")

    async def field(url: str, label: str) -> tuple[Any, Any]:
        tab = await fi.open_for(url, "t")
        await asyncio.sleep(0.5)
        s = await fi.snapshot(tab)
        return tab, fi.element_ref(tab, next(e.id for e in s.elements if e.label == label))

    allow(site.L)
    tab, pw = await field(f"{site.L}/login", "Password")
    out["login_ok"] = (await fi.fill_secret(pw, REF)).ok
    s = await fi.snapshot(tab)
    await fi.act(fi.element_ref(tab, next(e.id for e in s.elements if e.label == "Sign in")),
                 BrowserAction("click"), "")
    await asyncio.sleep(1)
    await fi.close(tab)
    before = len(scrubbed)
    for case, url, origins in (("lookalike", f"{site.I}/login", (site.L,)),
                               ("scheme", f"{site.L}/login", (site.L.replace("http:", "https:"),)),
                               ("iframe", f"{site.L}/top", (site.L,))):
        allow(*origins)
        tab, pw = await field(url, "Password")
        try:
            await fi.fill_secret(pw, REF)
            out[case] = "FILLED"
        except Refused as e:
            out[case] = str(e)
        await fi.close(tab)
    out["refused_before_keychain"] = len(scrubbed) == before
    allow(site.I)                                                          # control: the iframe's own origin
    tab, pw = await field(f"{site.L}/top", "Password")
    out["iframe_allowed_ok"] = (await fi.fill_secret(pw, REF)).ok
    s = await fi.snapshot(tab)
    await fi.act(fi.element_ref(tab, next(e.id for e in s.elements if e.label == "Sign in")),
                 BrowserAction("click"), "")
    await asyncio.sleep(1)
    await fi.close(tab)
    allow(site.L)
    tab, pw = await field(f"{site.L}/steal", "Password")
    out["steal_ok"] = (await fi.fill_secret(pw, REF)).ok
    s = await fi.snapshot(tab)
    out["grab_value"] = next(e.value for e in s.elements if e.label == "Grab")
    t = fi._tab(tab)                                                       # replay the last seal under a new id
    from ferb.identity.seal import seal, seal_key
    key = d.pairings.key(t.bridge.binding.install_id)
    sealed = seal(seal_key(key), "not-the-password", "the-original-command", site.L)
    frame, snap, local, _ = next(v for v in t.elements.values() if v[3].label == "Password")
    r = await t.bridge.call("fill", {"tab_id": t.chrome_id, "frame_id": frame, "snap": snap, "el": local,
                                     "sealed": sealed, "origin": site.L}, cid="replay-" + os.urandom(4).hex())
    out["replay"] = r.get("detail")
    try:
        await t.bridge.call("read_source", {"tab_id": t.chrome_id})
        out["read_source"] = "READ"
    except DriverError as e:
        out["read_source"] = str(e)
    await fi.close(tab)
    return out


def _keychain() -> Path:
    import secrets as _s
    import subprocess
    path = Path(tempfile.mkdtemp(prefix="ferbkc-")) / "t.keychain-db"
    pw = _s.token_hex(16)
    subprocess.run(["security", "create-keychain", "-p", pw, str(path)], check=True)
    subprocess.run(["security", "unlock-keychain", "-p", pw, str(path)], check=True)
    subprocess.run(["security", "add-generic-password", "-s", "ferb", "-a", REF.key, "-w", PASSWORD, str(path)],
                   check=True)
    return path


def _drop_keychain(path: Path) -> None:
    import subprocess
    subprocess.run(["security", "delete-keychain", str(path)], check=True)


async def _otp(d: Any, site: Site, since: datetime) -> bool:
    fi = d.ferb_identity
    ref = SecretRef("test/otp-check")
    specs = {ref.key: SecretSpec(ref, [site.L], "otp check")}
    fi.secrets = KeychainSecretStore(lambda: specs)
    tab = await fi.open_for(f"{site.L}/otp", "otp")
    await asyncio.sleep(0.5)
    s = await fi.snapshot(tab)
    el = fi.element_ref(tab, next(e.id for e in s.elements if e.label == "Code"))
    inventory = await fi.otp_candidates(el, ref)
    assert len(inventory["candidates"]) == 1
    r = await fi.fill_otp(el, ref, inventory["candidates"][0]["candidate"])
    if r.ok:
        s = await fi.snapshot(tab)
        await fi.act(fi.element_ref(tab, next(e.id for e in s.elements if e.label == "Verify")), BrowserAction("click"), "")
        await asyncio.sleep(1)
    await fi.close(tab)
    return r.ok



def test_local_inbox_code_is_filled_sealed(stand_in: Any, monkeypatch: pytest.MonkeyPatch) -> None:
    rd, chrome, _ = stand_in
    d, site = rd.daemon, Site()
    wire = tee(rd, chrome)
    d.config.set("mailbox", "address", ADDRESS)
    code = "839472"
    now = datetime.now(timezone.utc)

    async def read_local_feed(url: str) -> str:
        return feed([("login@test.example", "Login code", code, now)], ADDRESS)

    monkeypatch.setattr(d.ferb_identity, "_read_feed", read_local_feed)
    try:
        assert rd.run(_otp(d, site, now - timedelta(seconds=2)), timeout=60)
        assert site.posted[-1]["otp"] == [code]
        assert wire and not any(code in line for line in wire), "the code crossed the native host in clear"
    finally:
        site.srv.shutdown()


def test_compose_confirmation_on_local_mail_ui(stand_in: Any) -> None:
    """The test agent selects the fixture Send control; production code does not search for or choose it."""
    rd, _, _ = stand_in
    site = Site()
    fi = rd.daemon.ferb_identity
    async def send_fixture():
        from urllib.parse import urlencode
        args={"to":ADDRESS,"su":"Local self-test","body":"Local test body","view":"cm","fs":"1"}
        tab=await fi.open_for(f"{site.L}/compose?"+urlencode(args),"mail-test")
        await asyncio.sleep(.5)
        snap=await fi.snapshot(tab)
        candidates=[e for e in snap.elements if e.label == "Send"]
        assert len(candidates) == 1
        await fi.act(fi.element_ref(tab,candidates[0].id),BrowserAction("click"),"")
        await fi.close(tab)
    try:
        rd.run(send_fixture(),timeout=60)
        wait_for(lambda: bool(site.posted), 5, "local send did not reach the fixture")
        assert site.posted == [{"to": [ADDRESS], "su": ["Local self-test"], "body": ["Local test body"],
                                "view": ["cm"], "fs": ["1"]}]
    finally:
        site.srv.shutdown()
