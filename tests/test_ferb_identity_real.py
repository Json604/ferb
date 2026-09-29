"""Build step 6b, real browser (pytest -m real). Two parts, each driven by a test daemon with its own home (FERB's
native-host launcher is pointed at it for the run and restored afterwards):

1. `FERB_REAL_PROFILE="chrome:FERB Test"` — the test profile paired as a stand-in FERB identity, a temporary keychain,
   local pages only: 21.4 re-ported (lookalike host, scheme, cross-origin iframe refused before the Keychain is read;
   the allowed iframe works; focus stealing can't redirect the value), 21.21 (every line through the native host
   captured: the password never appears; a replayed seal is refused by the real extension), read_source refuses any
   page but FERB's inbox feed.
2. `FERB_REAL_IDENTITY=1` — FERB's own profile (owner-approved). It re-pairs that profile's extension with the
   test daemon, which replaces the key it shares with the installed daemon: afterwards the owner runs
   `ferb setup --identity` (confirming the re-pair) to give it back to FERB. The rest: pairing as FERB (its account check), the session
   checked through the extension, the Gmail and YouTube account extractors on real pages; with `FERB_OTP_CODE` +
   `FERB_OTP_SINCE`, a code the owner emailed to FERB is filled into a local page, sealed, never returned.
"""

import asyncio
import http.server
import re
import os
import tempfile
import threading
import time
from datetime import datetime
from pathlib import Path
from typing import Any, Iterator
from urllib.parse import parse_qs

import pytest

from ferb.act.browser.profiles import discover_all
from ferb.cli.client import Client
from ferb.clock.clock import SystemClock
from ferb.config import Config
from ferb.core.errors import DriverError, Refused
from ferb.core.types import BrowserAction, Identity, SecretRef, SecretSpec
from ferb.identity.secrets import KeychainSecretStore
from tests.browserkit import PASSWORD, REF
from tests.conftest import RunningDaemon
from tests.test_ext_real import LAUNCHER, restart_hosts

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


def daemon_on_launcher() -> Iterator[RunningDaemon]:
    if not LAUNCHER.exists():
        pytest.skip("run `ferb setup --update` first (installs the native host)")
    base = Path(tempfile.mkdtemp(prefix="ferbr-"))
    original = LAUNCHER.read_text()
    d = RunningDaemon(base / "home", clock=SystemClock())
    t0, events = time.monotonic(), []                                     # browser.* events, shown on failure
    publish = d.daemon._publish

    def record(topic: str, data: dict[str, Any]) -> None:
        if topic.startswith("browser."):
            events.append((round(time.monotonic() - t0, 1), topic, data.get("profile") or data.get("folder")))
        publish(topic, data)
    d.daemon._publish = record                                            # type: ignore[method-assign]
    assert d.daemon.hub is not None
    d.daemon.hub.publish = record
    d.events = events                                                     # type: ignore[attr-defined]
    # whatever home the launcher points at now (another test daemon may have it): ours for this run, then back
    LAUNCHER.write_text(re.sub(r'FERB_HOME="[^"]*"', f'FERB_HOME="{d.daemon.home}"', original))
    restart_hosts()
    try:
        yield d
    finally:
        LAUNCHER.write_text(original)
        restart_hosts()
        d.stop()


def tee(d: RunningDaemon) -> list[str]:
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
    restart_hosts()                            # connections made before the tap would go uncaptured
    return wire


# ---------- part 1: the FERB Test profile as a stand-in identity ----------

@pytest.fixture(scope="module")
def stand_in() -> Iterator[tuple[RunningDaemon, str, str]]:
    spec = os.environ.get("FERB_REAL_PROFILE")
    if not spec or ":" not in spec:
        pytest.skip('set FERB_REAL_PROFILE="chrome:<test profile>" (extension loaded there)')
    browser, name = spec.split(":", 1)
    p = next((p for p in discover_all() if p.browser == browser and p.display_name == name), None)
    if p is None:
        pytest.skip(f"{browser} profile {name!r} not found")
    yield from ((d, browser, p.folder) for d in daemon_on_launcher())


def test_stand_in_identity_origin_rules_and_sealing(stand_in: tuple[RunningDaemon, str, str]) -> None:
    rd, browser, folder = stand_in
    d = rd.daemon
    site = Site()
    wire = tee(rd)
    d.config.set("identity", "browser", browser)
    d.config.set("identity", "profile", folder)
    specs: dict[str, SecretSpec] = {}
    fi = d.ferb_identity
    assert fi is not None and d.pairings is not None
    kc = _keychain()
    store = KeychainSecretStore(lambda: specs, kc)
    fi.secrets, fi._store = store, store
    scrubbed: list[str] = []
    fi.add_secret_value = lambda v: (scrubbed.append(v), d._add_secret_value(v))  # type: ignore[assignment]
    try:
        async def pair() -> str:                     # the test profile holds no account: pair it directly (test only)
            return d.pairings.issue_token(browser, folder, Identity.FERB)   # type: ignore[union-attr]
        token = rd.run(pair())
        from ferb.act.browser.profiles import open_known_profile
        from ferb.act.browser.ext_protocol import FERB_EXTENSION_ID
        open_known_profile(browser, folder, f"chrome-extension://{FERB_EXTENSION_ID}/pair.html#token={token}",
                           discover_all())
        end = time.monotonic() + 60
        while time.monotonic() < end and not ((br := d.hub.bridges.get((browser, folder))) and br.conn):   # type: ignore[union-attr]
            time.sleep(0.2)
        assert rd.run(_attach(d)), "the stand-in profile didn't pair (is the extension reloaded there?)"
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


# ---------- part 2: FERB's own profile ----------

@pytest.fixture(scope="module")
def ferb_profile() -> Iterator[RunningDaemon]:
    if not os.environ.get("FERB_REAL_IDENTITY"):
        pytest.skip("set FERB_REAL_IDENTITY=1 (the extension loaded in FERB's profile)")
    home = Config(Path.home() / ".ferb")
    prof, address = home.get("identity", "profile"), home.get("mailbox", "address")
    if not prof or not address:
        pytest.skip("FERB's identity profile isn't set up: `ferb setup --identity`")
    for d in daemon_on_launcher():
        d.daemon.config.set("identity", "browser", "chrome")
        d.daemon.config.set("identity", "profile", str(prof))
        d.daemon.config.set("mailbox", "address", str(address))
        yield d


def test_ferb_profile_pairs_signs_in_and_account_extractors_read_ferbs_address(ferb_profile: RunningDaemon) -> None:
    d = ferb_profile
    wire = tee(d)
    c = Client(d.sock, timeout=120)
    try:
        r = c.call("identity.pair", {"timeout_s": 60})
        assert r["paired"], (r, d.events)                                  # type: ignore[attr-defined]
        check = c.call("identity.check")
    finally:
        c.close()
    address = str(d.daemon.config.get("mailbox", "address")).lower()
    assert check["profile_ok"] and check["session"] is True, (check, d.events)   # type: ignore[attr-defined]
    accounts = d.run(_accounts(d.daemon), timeout=120)
    assert accounts["mail.google.com"] == {"supported": True, "account": address}, accounts
    assert accounts["www.youtube.com"]["supported"] and accounts["www.youtube.com"]["account"] in (address,), accounts
    code, since = os.environ.get("FERB_OTP_CODE"), os.environ.get("FERB_OTP_SINCE")
    if code and since:
        site = Site()
        try:
            ok = d.run(_otp(d.daemon, site, datetime.fromisoformat(since)), timeout=700)
        finally:
            site.srv.shutdown()
        assert ok and site.posted and site.posted[-1]["otp"] == [code]
        assert not any(code in line for line in wire), "the code crossed the native host in clear"


async def _accounts(d: Any) -> dict[str, Any]:
    fi, out = d.ferb_identity, {}
    for url in ("https://mail.google.com/mail/u/0/#inbox", "https://www.youtube.com/account"):
        tab = await fi.open_for(url, "acct")
        await asyncio.sleep(4)                                                  # let the app render its header
        t = fi._tab(tab)
        host = url.split("/")[2]
        out[host] = await t.bridge.call("account", {"tab_id": t.chrome_id}, timeout=15)
        await fi.close(tab)
    return out


async def _otp(d: Any, site: Site, since: datetime) -> bool:
    fi = d.ferb_identity
    ref = SecretRef("test/otp-check")
    specs = {ref.key: SecretSpec(ref, [site.L], "otp check")}
    fi.secrets = KeychainSecretStore(lambda: specs)
    tab = await fi.open_for(f"{site.L}/otp", "otp")
    await asyncio.sleep(0.5)
    s = await fi.snapshot(tab)
    el = fi.element_ref(tab, next(e.id for e in s.elements if e.label == "Code"))
    r = await fi.fill_otp(el, ref, since, None, timeout=600, poll=5)
    if r.ok:
        s = await fi.snapshot(tab)
        await fi.act(fi.element_ref(tab, next(e.id for e in s.elements if e.label == "Verify")), BrowserAction("click"), "")
        await asyncio.sleep(1)
    await fi.close(tab)
    return r.ok
