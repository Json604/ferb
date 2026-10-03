"""Build step 6a, offline: the FERB extension's protocol through the real native host and a real daemon (a fake
extension stands in for the browser). Pairing and 21.16 (unpaired installs can't act or report), MAC / replay /
token rules, protocol-version refusal, 21.15 (disconnect mid-command -> unknown -> reconciled, never replayed),
21.13 (stale element ids after navigation or reconnect), 21.12 (wrong account refused), ask-once account trust,
the site resolver, profile discovery, and the owner-only routing (never FERB's identity profile).
"""

import json
import subprocess
import sys
import time
from pathlib import Path
from typing import Any

import pytest

from ferb.act.browser.pairing import Binding
from ferb.act.browser.profiles import BrowserProfile, discover, open_known_profile, resolve
from ferb.act.browser.resolver import NeedsOwner, Resolver
from ferb.cli.client import Client
from ferb.core.types import Identity
from tests.conftest import RunningDaemon
from tests.fake_extension import FakeExtension
from tests.browserkit import OWNER, bd, call, chrome_data, pair, task  # noqa: F401 (fixture)


# ---------- pairing, 21.16 ----------

def test_reinstalled_extension_is_reported_without_claiming_a_profile(bd: Any) -> None:
    d, launched = bd
    old = pair(d, launched, "Work")
    old.wait_for(lambda: old.gen is not None)
    old.kill()
    old.wait_for(lambda: d.daemon.hub.bridges[('chrome', 'Profile 1')].conn is None)
    fresh = FakeExtension(d.daemon.home)
    fresh.connect()
    fresh.wait_for(lambda: fresh.install_id in d.daemon.hub.unpaired)
    c = Client(d.sock)
    try:
        report = c.call('browsers.list')
    finally:
        c.close()
        fresh.kill()
    assert report['unpaired_connections'] == {'chrome': 1}
    work = next(p for p in report['profiles'] if p['name'] == 'Work')
    assert work['paired'] and not work['connected']
    assert work['version'] == ''  # a disconnected runtime has no current version
    assert work['last_version']


def test_bad_pairing_hello_reports_repair_without_attaching(bd: Any) -> None:
    d, launched = bd
    ext = pair(d, launched, 'Work')
    ext.wait_for(lambda: ext.gen is not None)
    ext.kill()
    ext.key = b'x' * 32
    ext.connect()
    try:
        ext.wait_for(lambda: ext.refused is not None)
        assert 'pairing failed (bad_mac)' in ext.refused
        assert d.daemon.hub.bridges[('chrome', 'Profile 1')].conn is None
    finally:
        ext.kill()


def test_browser_cli_distinguishes_cached_version_and_unpaired_connection():
    import io
    from types import SimpleNamespace
    from ferb.cli.admin import browsers
    report = {'profiles': [{'browser': 'chrome', 'folder': 'Profile 2', 'name': 'scaler', 'email': '',
                           'ferb_identity': False, 'paired': True, 'connected': False, 'refused': None,
                           'version': '0.9.0'}], 'unpaired_connections': {'chrome': 1}}
    out = io.StringIO()
    browsers(SimpleNamespace(call=lambda _: report), None, None, None, out)
    assert 'last connected version 0.9.0' in out.getvalue()
    assert 'live extension connection(s) have no pairing' in out.getvalue()
    assert 'ferb browsers pair <profile> --browser chrome' in out.getvalue()

def test_pairing_binds_the_profile_and_unpaired_installs_cannot_act_or_report(bd: Any) -> None:
    d, launched = bd
    stranger = FakeExtension(d.daemon.home)
    stranger.connect()
    stranger.tab_event("created", 1, "https://evil.example/")                   # unpaired: dropped
    time.sleep(0.3)
    assert not d.daemon.hub.bridges                                             # nothing attributed to anyone
    ext = pair(d, launched, "Work")
    ext.wait_for(lambda: ext.gen is not None)
    b = d.run(_binding(d, "chrome", "Profile 1"))
    assert b and b.install_id == ext.install_id and b.identity is Identity.OWNER
    listed = {p["name"]: p for p in Client(d.sock).call("browsers.list")["profiles"]}
    assert listed["Work"]["paired"] and listed["Work"]["connected"] and not listed["Personal"]["paired"]
    g = task(d, "a", OWNER, {"research"})
    r = call(d, g, "browser.open", url="https://example.com/", profile="Personal")
    assert r["is_error"] and "isn't connected to FERB yet" in r["content"]["error"]   # unpaired: never routed


def test_bad_mac_replay_and_reused_or_expired_tokens_are_refused(bd: Any) -> None:
    d, launched = bd
    ext = pair(d, launched, "Work")
    ext.wait_for(lambda: ext.gen is not None)
    n = len(d.daemon.hub.bridges[("chrome", "Profile 1")].tabs)
    ext.send("tab_event", {"event": "created", "tab_id": 7, "url": "https://a.example/"}, key=b"x" * 32)  # wrong key
    ext.send("tab_event", {"event": "created", "tab_id": 8, "url": "https://b.example/"}, seq=1)          # replay
    time.sleep(0.3)
    assert len(d.daemon.hub.bridges[("chrome", "Profile 1")].tabs) == n
    token = d.run(_issue(d, "Personal"))
    other = FakeExtension(d.daemon.home)
    other.connect()
    other.pair(token)
    other.wait_for(lambda: other.key is not None)
    third = FakeExtension(d.daemon.home)
    third.connect()
    third.pair(token)                                                            # single-use
    time.sleep(0.3)
    assert third.key is None
    late = d.run(_issue(d, "Personal"))
    d.call_soon(d.daemon.clock.advance, 301)                                     # expired
    third.pair(late)
    time.sleep(0.3)
    assert third.key is None


async def _binding(d: RunningDaemon, browser: str, folder: str) -> Binding | None:
    return d.daemon.pairings.for_profile(browser, folder)


async def _issue(d: RunningDaemon, name: str) -> str:
    p = next(p for p in d.daemon._profiles() if p.display_name == name)
    return d.daemon.pairings.issue_token(p.browser, p.folder)


def test_ferbs_identity_profile_is_never_paired_or_routed_as_owner(bd: Any) -> None:
    d, launched = bd
    with pytest.raises(Exception, match="FERB's own profile"):
        Client(d.sock).call("browsers.pair", {"profile": "FERB"})
    g = task(d, "f", OWNER, {"research"})
    r = call(d, g, "browser.open", url="https://example.com/", profile="FERB")
    assert r["is_error"] and "no profile called 'FERB'" in r["content"]["error"]


def test_protocol_mismatch_is_refused(bd: Any) -> None:
    d, launched = bd
    ext = pair(d, launched, "Work")
    ext.kill()
    old = FakeExtension(d.daemon.home, protocol=0)
    old.install_id, old.key, old.seq = ext.install_id, ext.key, ext.seq
    old.connect()
    old.wait_for(lambda: old.refused is not None)
    assert "reload the FERB extension" in (old.refused or "")


# ---------- the native host ----------

def test_native_host_refuses_a_foreign_extension_and_exits_when_the_daemon_goes(bd: Any) -> None:
    d, _ = bd
    env = {"FERB_HOME": str(d.daemon.home), "PATH": "/usr/bin:/bin"}
    r = subprocess.run([sys.executable, "-m", "ferb.native_host.main", "chrome-extension://" + "a" * 32 + "/"],
                       input=b"", capture_output=True, env=env, timeout=20)
    assert r.returncode == 2                                                    # refused by the daemon
    ext = FakeExtension(d.daemon.home)
    ext.connect()
    time.sleep(0.3)
    assert ext.proc and ext.proc.poll() is None
    d.stop()                                                                    # daemon goes away
    ext.proc.wait(10)
    assert ext.proc.returncode == 0                                             # host exits, never lingers
    d.stop = lambda: None                                                       # type: ignore[method-assign]


# ---------- reliability, 21.15 / 21.13 ----------

def test_disconnect_mid_command_is_unknown_then_reconciled_never_replayed(bd: Any) -> None:
    d, launched = bd
    ext = pair(d, launched, "Work")
    ext.wait_for(lambda: ext.gen is not None)
    g = task(d, "r", OWNER, {"fill_form"}, sites=frozenset({"shop.example"}))
    d.daemon.config.set("policy", "trusted_accounts", ["shop.example @ chrome/Profile 1"])
    tab = call(d, g, "browser.open", url="https://shop.example/form")["content"]["tab"]
    snap = call(d, g, "browser.snapshot", tab=tab)["content"]
    ext.hold = True
    import asyncio
    fut = asyncio.run_coroutine_threadsafe(d.daemon.gateway.call(g, "browser.act", {"tab": tab, "element": snap["elements"][1]["id"],
                                                                                    "action": "click", "effect": "send",
                                                                                    "why": "submitting the form"}), d.loop)
    ext.wait_for(lambda: any(v == "started" for v in ext.seen.values()))
    cid = next(k for k, v in ext.seen.items() if v == "started")
    ext.kill()                                                                  # worker dies mid-command
    r = fut.result(10)
    assert r["is_error"] and "outcome unknown" in r["content"]["error"]
    assert d.run(_state(d, cid)) == "unknown"
    ext.seen[cid] = "done"                                                      # it did happen before dying
    ext.hold = False
    ext.connect()                                                               # re-announces seen ids
    ext.wait_for(lambda: ext.gen is not None and ext.gen >= 2)
    time.sleep(0.2)
    assert d.run(_state(d, cid)) == "done"
    assert len(ext.acts) == 0                                                   # never resent
    r2 = call(d, g, "browser.act", tab=tab, element=snap["elements"][1]["id"], action="click", effect="send",
              why="submitting the form")
    assert r2["is_error"] and "older browser connection" in r2["content"]["error"]   # 21.13 after reconnect


def test_missing_command_in_bounded_recovery_history_stays_unknown(bd: Any) -> None:
    d, launched = bd
    ext = pair(d, launched, "Work")
    ext.wait_for(lambda: ext.gen is not None)
    g = task(d, "n", OWNER, {"fill_form"}, sites=frozenset({"shop.example"}))
    d.daemon.config.set("policy", "trusted_accounts", ["shop.example @ chrome/Profile 1"])
    tab = call(d, g, "browser.open", url="https://shop.example/form")["content"]["tab"]
    snap = call(d, g, "browser.snapshot", tab=tab)["content"]
    ext.hold = True
    import asyncio
    fut = asyncio.run_coroutine_threadsafe(d.daemon.gateway.call(g, "browser.act", {"tab": tab, "element": snap["elements"][1]["id"],
                                                                                    "action": "click", "effect": "send",
                                                                                    "why": "submitting the form"}), d.loop)
    ext.wait_for(lambda: any(v == "started" for v in ext.seen.values()))
    cid = next(k for k, v in ext.seen.items() if v == "started")
    ext.kill()
    fut.result(10)
    ext.seen.pop(cid)                                                           # the worker never recorded it
    ext.hold = False
    ext.connect()
    ext.wait_for(lambda: ext.gen is not None and ext.gen >= 2)
    time.sleep(0.2)
    assert d.run(_state(d, cid)) == "unknown"


async def _state(d: RunningDaemon, cid: str) -> str:
    return str(d.daemon.ledger.get(cid).intent.state)


def test_element_ids_go_stale_after_navigation(bd: Any) -> None:
    d, launched = bd
    ext = pair(d, launched, "Work")
    ext.wait_for(lambda: ext.gen is not None)
    g = task(d, "s", OWNER, {"research"})
    tab = call(d, g, "browser.open", url="https://shop.example/form")["content"]["tab"]
    snap = call(d, g, "browser.snapshot", tab=tab)["content"]
    link = next(e["id"] for e in snap["elements"] if e["role"] == "link")
    assert not call(d, g, "browser.act", tab=tab, element=link, action="click")["is_error"]   # navigates
    time.sleep(0.2)
    r = call(d, g, "browser.act", tab=tab, element=link, action="click")
    assert r["is_error"] and ("changed since the snapshot" in r["content"]["error"]
                              or "'submit' was not requested" in r["content"]["error"])


# ---------- accounts, 21.12 + ask once ----------


# ---------- resolver + discovery (deterministic, before any model) ----------

def _resolver(bindings: list[Binding]) -> Resolver:
    profiles = [BrowserProfile("chrome", "Default", "Personal", "me@gmail.com"),
                BrowserProfile("chrome", "Profile 1", "Work", "me@work.com"),
                BrowserProfile("brave", "Default", "Brave me", ""),
                BrowserProfile("chrome", "Profile 22", "FERB", "ferb@gmail.com")]
    return Resolver(lambda: profiles, lambda: bindings, lambda: ("chrome", "Profile 22"))


def test_resolver_rules() -> None:
    O = Identity.OWNER
    paired = [Binding("i1", "chrome", "Default", O), Binding("i2", "chrome", "Profile 1", O),
              Binding("i3", "brave", "Default", O), Binding("i4", "chrome", "Profile 22", Identity.FERB)]
    r = _resolver(paired)
    with pytest.raises(NeedsOwner, match="select a profile"):
        r.choose(None)
    assert r.choose("Personal").folder == "Default"
    assert r.choose("Work").folder == "Profile 1"
    assert r.choose("brave/Default").browser == "brave"
    with pytest.raises(NeedsOwner, match="no profile called 'FERB'"):
        r.choose("FERB")  # identity profile uses its own tool family


def test_profile_discovery_resolution_and_only_known_folders_open(tmp_path: Path) -> None:
    d = chrome_data(tmp_path)
    ps = discover("chrome", d)
    assert {p.display_name for p in ps} == {"Personal", "Work", "FERB"}
    assert resolve("work", ps).candidates[0].folder == "Profile 1"
    assert resolve("me@work.com", ps).candidates[0].folder == "Profile 1"       # email
    assert resolve("me", ps).kind == "ambiguous"                                # two profiles' local part is "me"
    both = ps + [BrowserProfile("brave", "Default", "Work", "")]
    assert resolve("Work", both).kind == "ambiguous"                            # never first match
    assert "found:" in resolve("nobody", ps).message("nobody", ps)
    launched: list[list[str]] = []
    stopped, started = (lambda b: False), (lambda b: True)
    open_known_profile("chrome", "Profile 1", "https://x/", ps, launch=launched.append, is_running=stopped)
    assert launched[0] == ["open", "-g", "-b", "com.google.Chrome", "--args", "--profile-directory=Profile 1",
                           "https://x/"]                                 # background: never takes focus (spike 19)
    open_known_profile("chrome", "Profile 1", None, ps, launch=launched.append, is_running=started)
    assert launched[-1][1:] == ["--profile-directory=Profile 1", "--no-startup-window"]   # running: `open` can't
    assert launched[-1][0].endswith("Google Chrome")                                     # reach it; the binary can
    launched.pop()
    open_known_profile("chrome", "Profile 1", "https://x/", ps, launch=launched.append, background=False,
                       is_running=stopped)
    assert launched[1][1] == "--profile-directory=Profile 1"              # pairing: the owner must see the page
    with pytest.raises(ValueError, match="refusing to create"):
        open_known_profile("chrome", "Profile 99", "https://x/", ps, launch=launched.append)


# ---------- the extension's own allowlist (21.6), run in node ----------

def test_extension_cdp_allowlist_refuses_cookies_targets_and_eval() -> None:
    import shutil
    node = shutil.which("node")
    if node is None:
        pytest.skip("node not installed")
    proto = Path(__file__).parents[1] / "extensions" / "chromium" / "protocol.js"
    script = f"""
const p = require({json.dumps(str(proto))});
const cases = [
  ["Network.getAllCookies", {{}}], ["Storage.getCookies", {{}}], ["Target.getTargets", {{}}],
  ["Target.attachToTarget", {{targetId: "x"}}], ["Runtime.evaluate", {{expression: "document.cookie"}}],
  ["Input.dispatchMouseEvent", {{type: "mouseMoved"}}], ["Input.dispatchMouseEvent", {{type: "bogus", x: 1, y: 1}}],
  ["Input.dispatchMouseEvent", {{type: "mousePressed", x: 1, y: 2}}], ["Input.insertText", {{text: "hi"}}],
  ["Emulation.setFocusEmulationEnabled", {{enabled: true}}], ["__proto__", {{}}], ["constructor", {{}}],
];
console.log(JSON.stringify(cases.map(([m, a]) => p.cdpAllowed(m, a))));
console.log(JSON.stringify(p.cdpFilter("Page.captureScreenshot", {{data: "x", extra: "cookie"}})));
"""
    out = subprocess.run([node, "-e", script], capture_output=True, text=True, timeout=20).stdout.splitlines()
    assert json.loads(out[0]) == [False, False, False, False, False, False, False, True, True, True, False, False]
    assert json.loads(out[1]) == {"data": "x"}


def test_a_profile_is_woken_with_no_window_first(bd: Any) -> None:
    """Spike 19 (owner watching): loading the profile with no window never moved them and its extension came online.
    That's tried first; no wake page, nothing to close."""
    d, launched = bd
    ext = pair(d, launched, "Work")
    ext.wait_for(lambda: ext.gen is not None)
    ext.kill()                                                                  # the profile isn't running
    d.run(_start_wake(d))

    def on_launch(args: list[str]) -> None:
        launched.append(args)
        ext.connect()                                                           # loading the profile starts it
    d.daemon.router.launch = on_launch                                          # type: ignore[union-attr]
    g = task(d, "w", OWNER, {"research"})
    r = call(d, g, "browser.open", url="https://example.com/", profile="Work")
    assert not r["is_error"], r
    assert launched[-1] == ["open", "-g", "-b", "com.google.Chrome", "--args", "--profile-directory=Profile 1",
                            "--no-startup-window"]                              # background, no window
    assert list(ext.tabs.values())[-1] == "https://example.com/"


def test_disconnected_profile_does_not_open_a_visible_wake_tab(
        bd: Any, monkeypatch: pytest.MonkeyPatch) -> None:
    import ferb.act.browser.ext_bridge as eb
    monkeypatch.setattr(eb, "WINDOWLESS_WAIT_S", 0.05)
    monkeypatch.setattr(eb, "WAKE_TIMEOUT_S", 0.05)
    d, launched = bd
    ext = pair(d, launched, "Work")
    ext.wait_for(lambda: ext.gen is not None)
    ext.kill()
    d.run(_start_wake(d))
    before = len(launched)
    d.daemon.router.launch = launched.append
    g = task(d, "w", OWNER, "use my Work profile")
    r = call(d, g, "browser.open", url="https://example.com/", profile="Work")
    assert r["is_error"] and "reload" in r["content"]["error"]
    assert len(launched) == before + 1
    assert launched[-1][-1] == "--no-startup-window"
    assert not any("/wake/" in arg for argv in launched[before:] for arg in argv)


async def _start_wake(d: RunningDaemon) -> None:
    d.daemon.router.wake = d.daemon.wake                                        # type: ignore[union-attr]


def test_explicit_account_lookup_reports_visible_account_without_extra_tabs(bd):
    d, launched = bd
    ext = pair(d, launched, "Personal")
    ext.wait_for(lambda: ext.gen is not None)
    g = task(d, "y", OWNER, {"fill_form"})
    alias = call(d, g, "browser.open", url="https://www.youtube.com/account", profile="Personal")["content"]["tab"]
    ext.accounts["www.youtube.com"] = "me@gmail.com"
    before = set(ext.tabs)
    async def lookup():
        tab = d.daemon.router.owned_by(alias, g.task_id)
        return await d.daemon.router.account_of(g.task_id, "www.youtube.com")
    assert d.run(lookup()) == "me@gmail.com"
    assert set(ext.tabs) == before


WORKER_HARNESS = r"""
const vm = require("vm"), fs = require("fs"), path = require("path"), crypto = require("crypto");
const dir = process.argv[1], OLD = process.argv[2], NEW = process.argv[3];
const store = { ferb: { installId: "inst-1", key: OLD, seq: 5 } }, posted = [];
const later = (v) => new Promise((r) => setTimeout(() => r(v), 5));       // storage is async, like Chrome's
const noop = new Proxy(function () {}, { get: () => noop, apply: () => undefined });
const chrome = new Proxy({
  storage: { local: { get: (k) => later({ [k]: store[k] && JSON.parse(JSON.stringify(store[k])) }),
                      set: (o) => { Object.assign(store, JSON.parse(JSON.stringify(o))); return later(); } },
             session: { get: () => later({}), set: () => later() } },
  runtime: new Proxy({ connectNative: () => ({ postMessage: (m) => posted.push(m), onMessage: noop, onDisconnect: noop }),
                       getManifest: () => ({ version: "t" }) }, { get: (t, k) => (k in t ? t[k] : noop) }),
}, { get: (t, k) => (k in t ? t[k] : noop) });
const ctx = vm.createContext({ chrome, crypto: globalThis.crypto, btoa, atob, TextEncoder, console, setTimeout,
                               clearTimeout, navigator: {}, URL });
ctx.importScripts = (f) => vm.runInContext(fs.readFileSync(path.join(dir, f), "utf8"), ctx);
vm.runInContext(fs.readFileSync(path.join(dir, "background.js"), "utf8"), ctx);
const mac = (key, m) => crypto.createHmac("sha256", Buffer.from(key, "base64"))
  .update(`${m.install_id}|${m.seq}|${m.type}|${m.body_json}`).digest("base64");
(async () => {
  await ctx.ensurePort();
  const paired = ctx.onDaemon({ type: "paired", key: NEW });
  const events = [1, 2, 3].map((i) => ctx.send("tab_event", { event: "updated", tab_id: i }));   // racing the reply
  await Promise.all([paired, ...events]);
  await new Promise((r) => setTimeout(r, 100));
  const signed = posted.filter((m) => m.mac), i = signed.findIndex((m) => mac(NEW, m) === m.mac);
  console.log(JSON.stringify({ stored_key_new: store.ferb.key === NEW,
    after_pairing: signed.slice(i).map((m) => ({ seq: m.seq, new_key: mac(NEW, m) === m.mac, type: m.type })) }));
})();
"""


def test_worker_keeps_the_new_key_and_orders_seqs_when_sends_race_the_paired_reply() -> None:
    """The real background.js under node with async storage: tab events sent while the "paired" reply is stored must
    not write the old key back (every later message would fail its MAC and the profile would never connect), and
    messages leave in strictly increasing seq order."""
    import base64
    import os
    import shutil
    node = shutil.which("node")
    if node is None:
        pytest.skip("node not installed")
    ext = Path(__file__).parents[1] / "extensions" / "chromium"
    old, new = (base64.b64encode(os.urandom(32)).decode() for _ in range(2))
    r = subprocess.run([node, "-e", WORKER_HARNESS, str(ext), old, new], capture_output=True, text=True, timeout=20)
    out = json.loads(r.stdout or "null") or pytest.fail(r.stderr)
    assert out["stored_key_new"], out
    after = out["after_pairing"]
    assert after and all(m["new_key"] for m in after), after
    assert [m["seq"] for m in after] == list(range(1, len(after) + 1)), after


def test_the_profile_that_owns_a_window_says_whether_its_tab_has_a_password_field(bd: Any) -> None:
    """Step 9 screenshot check (9.4 window_password): every connected profile of that browser is asked; only the
    one whose window has exactly those bounds answers; nobody matching means "can't tell"."""
    d, launched = bd
    work, personal = pair(d, launched, "Work"), pair(d, launched, "Personal")
    personal.windows[(10, 20, 800, 600)] = True
    work.windows[(0, 0, 500, 400)] = False
    ask = d.daemon._web_password
    assert d.run(ask("chrome", (10.0, 20.0, 800.0, 600.0))) is True
    assert d.run(ask("chrome", (0.0, 0.0, 500.0, 400.0))) is False
    assert d.run(ask("chrome", (1.0, 1.0, 5.0, 5.0))) is None
    assert d.run(ask("brave", (10.0, 20.0, 800.0, 600.0))) is None


@pytest.mark.parametrize("driver_name", ["router", "ferb_identity"])
@pytest.mark.parametrize("operation", ["open", "wake"])
@pytest.mark.parametrize("destination", ["com.brave.Browser", "com.google.Chrome"])
@pytest.mark.parametrize("fails", [False, True])
def test_browser_wait_preserves_the_owners_new_app(
        daemon: RunningDaemon, monkeypatch: pytest.MonkeyPatch,
        driver_name: str, operation: str, destination: str, fails: bool) -> None:
    """An owner app switch during an async browser step must survive success and failure alike.

    Regression: Chrome search completion reactivated VS Code after the owner switched to Brave.
    The same race applies when the owner chooses a different profile of the target browser.
    Only fake app APIs and a fake extension response are used; no real Mac input is sent.
    """
    import asyncio
    from types import SimpleNamespace
    import ferb.act.browser.ext_bridge as eb
    import ferb.act.mac.apps as apps

    front = SimpleNamespace(bundle_id="com.microsoft.VSCode")
    activations: list[str] = []

    def activate(bundle: str) -> bool:
        activations.append(bundle)
        front.bundle_id = bundle
        return True

    monkeypatch.setattr(apps, "frontmost", lambda: front)
    monkeypatch.setattr(apps, "activate", activate)
    monkeypatch.setattr(daemon.daemon, "watch_owner_input", True)
    monkeypatch.setattr(eb, "WINDOWLESS_WAIT_S", 0.01)
    driver = getattr(daemon.daemon, driver_name)  # retain actual daemon wiring of both browser drivers

    async def run() -> None:
        bridge = SimpleNamespace(
            binding=SimpleNamespace(browser="chrome", folder="Profile 22", install_id="test-install"),
            device_id="test-device", gen=1, wake_url=None, hold=lambda: None, unhold=lambda: None,
            conn=None, connected=asyncio.Event(), name="test Chrome")

        async def open_tab(*args: Any, **kwargs: Any) -> dict[str, Any]:
            await asyncio.sleep(0)  # the owner can choose a new app while the command is pending
            front.bundle_id = destination
            if fails:
                raise eb.DriverError("test open failed")
            return {"tab_id": 1, "url": "https://example.com/"}

        def launch_profile(*args: Any, **kwargs: Any) -> None:
            front.bundle_id = destination
            if not fails:
                bridge.conn = object()
                bridge.connected.set()

        bridge.call = open_tab
        monkeypatch.setattr(eb, "open_known_profile", launch_profile)
        if operation == "wake":
            driver.wake = object()
            pending = driver._connected(bridge)
        else:
            pending = driver._open_in(bridge, "https://example.com/", "test-task")
        if fails:
            with pytest.raises(eb.DriverError):
                await pending
        else:
            await pending

    daemon.run(run())
    assert activations == []
    assert front.bundle_id == destination


def test_tab_listing_reports_disconnected_pairing_instead_of_empty_tabs(bd):
    d, launched = bd
    ext = pair(d, launched, 'Work')
    ext.kill()
    deadline = time.monotonic() + 3
    while d.daemon.hub.bridges[('chrome','Profile 1')].conn is not None and time.monotonic() < deadline:
        time.sleep(.02)
    grant = task(d, 'disconnected', OWNER, 'use my Work profile')
    result = call(d, grant, 'browser.tabs', profile='Work')
    assert result['is_error']
    assert 'paired but not connected' in result['content']['error']
    assert 'reload' in result['content']['error']
