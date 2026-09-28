"""Build step 6b, offline: FERB's identity profile on the extension (a fake extension through the real native host
and a real daemon). Pairing as identity FERB needs exactly FERB's account (test 20) and confirmation to re-pair;
secrets and codes travel sealed (21.21: never on the wire in clear, not replayable under another command or
origin); the origin is checked before the Keychain is read (21.4); a filled value is nowhere FERB writes (21.5);
FERB's tools exist only once the profile is paired, and never reach owner profiles (21.3).
"""

import base64
import json
import os
import subprocess
import threading
import time
from datetime import datetime, timedelta, timezone
from pathlib import Path
from typing import Any, Iterator

import pytest

from ferb.cli.client import Client, RpcError
from ferb.core.types import Budget, Identity, SecretSpec, Task
from ferb.identity.seal import seal, seal_key
from ferb.identity.secrets import KeychainSecretStore
from ferb.orchestrator.tree import TaskRecord
from ferb.policy.intent import RequestIntent
from tests.browserkit import PASSWORD, REF, feed
from tests.conftest import RunningDaemon
from tests.fake_extension import FakeExtension
from tests.test_ext_bridge import chrome_data

FERB_SCOPES = {"identity:browse", "secrets:fill", "mailbox:read"}
SITE = "https://login.example.com"


@pytest.fixture
def fd(daemon: RunningDaemon, keychain: Path) -> Iterator[tuple[RunningDaemon, list[list[str]], list[str]]]:
    d = daemon.daemon
    launched: list[list[str]] = []
    d.browser_data_dirs = {"chrome": chrome_data(d.home.parent)}
    d.browser_launch = launched.append
    for drv in (d.router, d.ferb_identity):
        drv.launch, drv.wake = launched.append, None                               # type: ignore[union-attr]
    d.config.set("identity", "browser", "chrome")
    d.config.set("identity", "profile", "Profile 22")
    d.config.set("mailbox", "address", "ferb@gmail.com")
    specs = {REF.key: SecretSpec(REF, [SITE], "test account")}
    store = KeychainSecretStore(lambda: specs, keychain)
    scrubbed: list[str] = []
    fi = d.ferb_identity
    assert fi is not None
    fi.secrets, fi._store = store, store
    fi.add_secret_value = lambda v: (scrubbed.append(v), d._add_secret_value(v))   # type: ignore[assignment]
    yield daemon, launched, scrubbed


def pair_identity(d: RunningDaemon, launched: list[list[str]], confirm: bool = False) -> tuple[FakeExtension, dict[str, Any]]:
    ext = FakeExtension(d.daemon.home)
    ext.connect()
    out: dict[str, Any] = {}
    c = Client(d.sock, timeout=30)

    def go() -> None:
        try:
            out.update(c.call("identity.pair", {"timeout_s": 10, "confirm": confirm}))
        except RpcError as e:
            out["error"] = str(e)
    t = threading.Thread(target=go)
    n = len(launched)
    t.start()
    try:
        ext.wait_for(lambda: len(launched) > n or "error" in out, 10)
        if len(launched) > n:
            ext.pair(launched[-1][-1].split("#token=")[1])
        t.join(15)
    finally:
        c.close()
    return ext, out


def task(d: RunningDaemon, tid: str, identity: Identity, scopes: set[str], verbs: set[str] | None) -> Any:
    t = Task(tid, None, "g", "dev", identity, scopes, "actor", f"req-{tid}", [],
             d.daemon.clock.now() + timedelta(minutes=10), Budget())

    async def go() -> Any:
        root = d.daemon.spans.start("task", task_id=t.id, input={})
        d.daemon.tasks.add(TaskRecord(t, d.daemon.clock.now(), root.id))
        if verbs is not None:
            d.daemon.engine.intents.record(RequestIntent.from_plan(f"req-{tid}", verbs))
        return d.daemon.tokens.validate(d.daemon.tokens.issue(t.id, f"w-{tid}", scopes, identity, ttl_s=600))
    return d.run(go())


def call(d: RunningDaemon, grant: Any, name: str, **args: Any) -> dict[str, Any]:
    return d.run(d.daemon.gateway.call(grant, name, args), timeout=30)          # type: ignore[no-any-return]


def el(snap: dict[str, Any], **match: Any) -> str:
    return next(str(e["id"]) for e in snap["elements"] if all(e.get(k) == v for k, v in match.items()))


def nowhere(d: RunningDaemon, value: str, ext: FakeExtension, *results: Any) -> None:
    assert not any(value in w for w in ext.wire), "value crossed the native host in clear"   # 21.21
    assert value not in json.dumps(results)                                             # never to the worker
    db = d.daemon.home / "ferb.db"
    for f in (db, db.with_name("ferb.db-wal")):
        if f.exists():
            assert value.encode() not in f.read_bytes()                                 # spans, journal, ledger
    for f in (d.daemon.home / "logs").glob("*") if (d.daemon.home / "logs").exists() else []:
        assert value not in f.read_text(errors="ignore")


def test_pairing_needs_exactly_ferbs_account_and_confirmation_to_repair(fd: Any) -> None:
    d, launched, _ = fd
    g = task(d, "a", Identity.FERB, FERB_SCOPES, None)
    assert not [t for t in d.daemon.gateway.tools_for(g) if t["name"].startswith("ferb_browser.")]   # not yet
    d.daemon.config.set("mailbox", "address", "someone@else.com")
    _, out = pair_identity(d, launched)
    assert "refusing to pair" in out.get("error", "")                          # test 20
    d.daemon.config.set("mailbox", "address", "ferb@gmail.com")
    ext, out = pair_identity(d, launched)
    assert out.get("paired") is True, out
    names = {t["name"] for t in d.daemon.gateway.tools_for(g)}
    assert {"ferb_browser.open", "ferb_browser.fill_secret", "ferb_browser.fill_otp"} <= names
    _, again = pair_identity(d, launched)
    assert "needs your confirmation" in again.get("error", "")
    owner = task(d, "o", Identity.OWNER, {"browser:read", "browser:act"}, {"research"})
    r = call(d, owner, "browser.open", url="https://x.example/", profile="FERB")
    assert r["is_error"]                                                       # 21.3: never an owner profile


def test_sealed_sign_in_never_puts_the_password_on_the_wire(fd: Any) -> None:
    d, launched, scrubbed = fd
    ext, _ = pair_identity(d, launched)
    g = task(d, "s", Identity.FERB, FERB_SCOPES, {"sign_in"})
    results = [call(d, g, "ferb_browser.open", url=f"{SITE}/login")]
    tab = results[0]["content"]["tab"]
    results.append(call(d, g, "ferb_browser.snapshot", tab=tab))
    snap = results[-1]["content"]
    results.append(call(d, g, "ferb_browser.fill_secret", tab=tab, element=el(snap, type="password"), secret=REF.key))
    assert not results[-1]["is_error"], results[-1]
    results.append(call(d, g, "ferb_browser.snapshot", tab=tab))
    assert el(results[-1]["content"], type="password", sensitive=True, filled=True)
    assert PASSWORD in ext.values.values() and scrubbed == [PASSWORD]
    assert ext.fills and "ct" in ext.fills[-1]["sealed"]                       # it travelled sealed
    nowhere(d, PASSWORD, ext, results)


def test_origin_is_checked_before_the_keychain_is_read(fd: Any) -> None:
    d, launched, scrubbed = fd
    ext, _ = pair_identity(d, launched)
    g = task(d, "f", Identity.FERB, FERB_SCOPES, {"sign_in"})
    tab = call(d, g, "ferb_browser.open", url=f"{SITE}/framed")["content"]["tab"]
    snap = call(d, g, "ferb_browser.snapshot", tab=tab)["content"]
    r = call(d, g, "ferb_browser.fill_secret", tab=tab, element=el(snap, sensitive=True), secret=REF.key)
    assert r["is_error"] and "evil.example" in r["content"]["error"] and "isn't allowed" in r["content"]["error"]
    assert scrubbed == [] and ext.fills == []                                  # no Keychain read, nothing sent


def test_a_sealed_value_cannot_be_replayed_under_another_command_or_origin(fd: Any) -> None:
    d, launched, _ = fd
    ext, _ = pair_identity(d, launched)
    g = task(d, "r", Identity.FERB, FERB_SCOPES, {"sign_in"})
    tab = call(d, g, "ferb_browser.open", url=f"{SITE}/login")["content"]["tab"]
    snap = call(d, g, "ferb_browser.snapshot", tab=tab)["content"]
    call(d, g, "ferb_browser.fill_secret", tab=tab, element=el(snap, type="password"), secret=REF.key)
    first = ext.fills[-1]
    chrome_tab = next(iter(ext.tabs))
    base = {"tab_id": chrome_tab, "frame_id": 0, "snap": 1, "el": 1, "sealed": first["sealed"]}
    assert ext.op_fill({**base, "origin": first["origin"]}, "another-command")["detail"] == "seal_refused"
    assert ext.op_fill({**base, "origin": "https://evil.example"}, first["id"])["detail"] == "seal_refused"


def test_otp_feed_and_code_travel_sealed(fd: Any) -> None:
    d, launched, scrubbed = fd
    ext, _ = pair_identity(d, launched)
    now = datetime.now(timezone.utc)
    ext.feed_xml = feed([("noreply@login.example.com", "Your code", "Your sign-in code is 482913.", now)],
                        address="ferb@gmail.com")
    g = task(d, "o", Identity.FERB, FERB_SCOPES, {"sign_in"})
    tab = call(d, g, "ferb_browser.open", url=f"{SITE}/login")["content"]["tab"]
    snap = call(d, g, "ferb_browser.snapshot", tab=tab)["content"]
    r = call(d, g, "ferb_browser.fill_otp", tab=tab, element=el(snap, type="password"), secret=REF.key)
    assert not r["is_error"], r
    assert "482913" in ext.values.values() and "482913" in scrubbed
    nowhere(d, "482913", ext, r)                                               # not even inside the feed
    assert "Your sign-in code" not in "".join(ext.wire)                         # email content sealed as well


def test_seal_opens_in_webcrypto_only_for_its_command_and_origin() -> None:
    import shutil
    node = shutil.which("node")
    if node is None:
        pytest.skip("node not installed")
    pk = os.urandom(32)
    s = seal(seal_key(pk), "pä$$ word", "cmd-1", "https://a.example")
    tampered = {**s, "ct": base64.b64encode(bytes([base64.b64decode(s["ct"])[0] ^ 1]) + base64.b64decode(s["ct"])[1:]).decode()}
    proto = Path(__file__).parents[1] / "extensions" / "chromium" / "protocol.js"
    js = f"""
const p = require({json.dumps(str(proto))}), pk = {json.dumps(base64.b64encode(pk).decode())};
const s = {json.dumps(s)}, bad = {json.dumps(tampered)};
(async () => {{
  const r = [await p.unseal(pk, s, "cmd-1", "https://a.example")];
  for (const [x, c, o] of [[s, "cmd-2", "https://a.example"], [s, "cmd-1", "https://b.example"], [bad, "cmd-1", "https://a.example"]])
    r.push(await p.unseal(pk, x, c, o).then(() => "OPENED", () => "refused"));
  const back = await p.seal(pk, "feed <xml>", "r-1", p.FEED_URL);
  console.log(JSON.stringify({{r, back}}));
}})();"""
    out = json.loads(subprocess.run([node, "-e", js], capture_output=True, text=True, timeout=20).stdout)
    assert out["r"] == ["pä$$ word", "refused", "refused", "refused"]
    from ferb.identity.seal import unseal
    assert unseal(seal_key(pk), out["back"], "r-1", "https://mail.google.com/mail/feed/atom") == "feed <xml>"
