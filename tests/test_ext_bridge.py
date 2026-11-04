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

def test_file_chooser_event_reaches_real_bus_without_dropping_extension_connection(bd: Any) -> None:
    """Exercise authenticated native transport -> Hub -> daemon publisher -> real Bus, then another command."""
    import asyncio
    d, launched = bd
    ext = pair(d, launched, "Work")
    ext.wait_for(lambda: ext.gen is not None)

    async def receive() -> None:
        sub = d.daemon.bus.subscribe({"browser.file_chooser", "browser.disconnected"})
        bridge = d.daemon.hub.bridges[("chrome", "Profile 1")]
        connection = bridge.conn
        facts = {"event":"file_chooser", "tab_id":73, "frame_id":"child-frame", "session_id":"child-session",
                 "backend_node_id":71, "mode":"selectSingle", "reason":"native_file_chooser_intercepted",
                 "native_picker_shown":False, "transferred":False, "direct_upload_available":True,
                 "detail":"No source selected"}
        try:
            ext.send("event", facts)
            event = await asyncio.wait_for(anext(sub.__aiter__()), 3)
            assert event.topic == "browser.file_chooser", event
            assert event.data == {"profile":"chrome/Profile 1", **facts}
            assert bridge.conn is connection and connection is not None
            assert await bridge.call("ping", {}, timeout=2) == {"ok":True}
        finally:
            sub.close()

    try:
        d.run(receive(), timeout=8)
    finally:
        ext.kill()


def _pairings(verdict: Any) -> Any:
    """A pairing store that answers every message with `verdict` and knows no install."""
    from types import SimpleNamespace
    return SimpleNamespace(handle=lambda m: verdict, binding=lambda install_id: None,
                           record_version=lambda install_id, version: None, version=lambda install_id: "")


@pytest.mark.parametrize('pairing_error', [None, 'bad_mac', 'replay'])
def test_refusal_metadata_distinguishes_protocol_failure_from_pairing_failure(pairing_error):
    import asyncio
    from types import SimpleNamespace
    from unittest.mock import AsyncMock
    from ferb.act.browser.ext_bridge import Hub
    from ferb.act.browser.ext_protocol import ExtMessage, MIN_EXTENSION_VERSION, PROTOCOL_VERSION
    verdict = SimpleNamespace(accepted=pairing_error is None, reason=pairing_error, reply=None, binding=object())
    hub = Hub(_pairings(verdict), None, lambda *a: None, lambda: 'test-device')
    writes = []
    conn = SimpleNamespace(writer=SimpleNamespace(write=writes.append, drain=AsyncMock()), app=None)
    message = ExtMessage(install_id='test-install', type='hello', body_json=json.dumps({
        'version':MIN_EXTENSION_VERSION, 'protocol':PROTOCOL_VERSION - 1,
    }))
    asyncio.run(hub._handle(conn, message))
    reply = json.loads(writes[0])
    assert reply['type'] == 'refused'
    if pairing_error:
        assert 'required_protocol' not in reply and 'minimum_version' not in reply
    else:
        assert reply['required_protocol'] == PROTOCOL_VERSION
        assert reply['minimum_version'] == MIN_EXTENSION_VERSION


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
    report = {'profiles': [{'browser': 'chrome', 'folder': 'Profile 2', 'name': 'acme', 'email': '',
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
    assert r["is_error"] and "isn't paired with FERB yet" in r["content"]["error"]    # unpaired: never routed
    assert "browser_pair" in r["content"]["error"]                     # the worker's way: FERB asks the owner once


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
    assert "too old" in (old.refused or "")
    from ferb.act.browser.ext_protocol import MIN_EXTENSION_VERSION, PROTOCOL_VERSION
    refusal = next(json.loads(msg) for msg in old.wire if json.loads(msg).get('type') == 'refused')
    assert refusal['required_protocol'] == PROTOCOL_VERSION
    assert refusal['minimum_version'] == MIN_EXTENSION_VERSION


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
    # protocol 18: the tab is re-adopted by its browser id (the hello listed it); its element ids from before the
    # reconnect are stale, so nothing is replayed from them
    assert r2["is_error"] and "older snapshot" in r2["content"]["error"]
    assert not call(d, g, "browser.snapshot", tab=tab)["is_error"]


def test_missing_command_in_bounded_recovery_history_stays_unknown(bd: Any) -> None:
    d, launched = bd
    ext = pair(d, launched, "Work")
    ext.wait_for(lambda: ext.gen is not None)
    g = task(d, "n", OWNER, {"fill_form"}, sites=frozenset({"shop.example"}))
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
                BrowserProfile("chrome", "Profile 7", "FERB", "ferb@gmail.com")]
    return Resolver(lambda: profiles, lambda: bindings, lambda: ("chrome", "Profile 7"))


def test_resolver_rules() -> None:
    O = Identity.OWNER
    paired = [Binding("i1", "chrome", "Default", O), Binding("i2", "chrome", "Profile 1", O),
              Binding("i3", "brave", "Default", O), Binding("i4", "chrome", "Profile 7", Identity.FERB)]
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
    with pytest.raises(ValueError, match="needs_foreground"):
        open_known_profile("chrome", "Profile 1", "https://x/", ps, launch=launched.append, is_running=stopped)
    assert not launched
    open_known_profile("chrome", "Profile 1", None, ps, launch=launched.append, is_running=stopped)
    assert launched[0] == ["open", "-g", "-b", "com.google.Chrome", "--args", "--profile-directory=Profile 1",
                           "--no-startup-window"]
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


# decision: HAND-403
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
    ext.tab_windows.clear()                                                     # the woken profile has no window yet
    ext.ferb_window = None
    before = ext.windows_created
    r = call(d, g, "browser.open", url="https://example.com/", profile="Work")
    assert launched[-1] == ["open", "-g", "-b", "com.google.Chrome", "--args", "--profile-directory=Profile 1",
                            "--no-startup-window"]                              # background, no window at launch
    # Owner 2026-10-26: a profile with no window at all (just started): FERB makes its own minimized window once, then
    # works through the extension; the task's tab opens there instead of failing.
    assert not r["is_error"], r
    assert ext.windows_created == before + 1 and "https://example.com/" in ext.tabs.values()


def test_listing_a_closed_profiles_tabs_starts_it_in_the_background(bd: Any) -> None:
    """Live 2026-10-26 (task 7c1eca3a): the worker listed Home_Personal's tabs six times while Chrome had no profile
    loaded; the listing never started it, said "reload the FERB extension", and the task ended asking the owner to
    reload it. Listing a named profile's tabs starts it with no window, as browser.open does."""
    d, launched = bd
    ext = pair(d, launched, "Work")
    ext.wait_for(lambda: ext.gen is not None)
    ext.kill()                                                                  # the profile isn't running
    d.run(_start_wake(d))

    def on_launch(args: list[str]) -> None:
        launched.append(args)
        ext.connect()                                                           # loading the profile starts it
    d.daemon.router.launch = on_launch                                          # type: ignore[union-attr]
    ext.tabs.clear()
    ext.tab_windows.clear()                                                     # just started: no window, no tab
    before = ext.windows_created
    r = call(d, task(d, "w", OWNER, "use my Work profile"), "browser.tabs", profile="Work")
    assert not r["is_error"], r
    assert r["content"]["tabs"] == []
    assert launched[-1][-1] == "--no-startup-window" and ext.windows_created == before   # no window for a listing


def test_listing_every_profile_skips_a_closed_one_and_names_it(bd: Any) -> None:
    d, launched = bd
    ext = pair(d, launched, "Work")
    ext.wait_for(lambda: ext.gen is not None)
    ext.kill()
    ext.wait_for(lambda: d.daemon.hub.bridges[("chrome", "Profile 1")].conn is None)
    r = call(d, task(d, "w", OWNER, "what tabs do I have open"), "browser.tabs")
    assert not r["is_error"], r
    assert r["content"]["tabs"] == []
    closed = r["content"]["not_connected"]
    assert [c["profile"] for c in closed] == ["chrome/Profile 1"] and "browser_open" in closed[0]["why"]


# decision: HAND-403
def test_disconnected_profile_does_not_open_a_visible_wake_tab(
        bd: Any, monkeypatch: pytest.MonkeyPatch) -> None:
    """Neither the quiet start nor the profile's window brought its extension: a known failure that names both, and
    never a visible wake page."""
    import ferb.act.browser.ext_bridge as eb
    monkeypatch.setattr(eb, "WINDOWLESS_WAIT_S", 0.05)
    monkeypatch.setattr(eb, "WINDOW_WAIT_S", 0.05)
    d, launched = bd
    ext = pair(d, launched, "Work")
    ext.wait_for(lambda: ext.gen is not None)
    ext.kill()
    d.run(_start_wake(d))
    before = len(launched)
    d.daemon.router.launch = launched.append
    g = task(d, "w", OWNER, "use my Work profile")
    r = call(d, g, "browser.open", url="https://example.com/", profile="Work")
    assert r["is_error"] and "even with its window open" in r["content"]["error"]
    assert len(launched) == before + 2
    assert launched[before][-1] == "--no-startup-window"
    assert launched[-1][1:] == ["--profile-directory=Profile 1"]               # the profile's own window, no page
    assert not any("/wake/" in arg for argv in launched[before:] for arg in argv)


# decision: HAND-403
def test_a_profile_the_quiet_start_cant_reach_gets_its_window_and_the_task_goes_on(
        bd: Any, monkeypatch: pytest.MonkeyPatch) -> None:
    """The owner's rule (2026-10-26): "see if the browser is closed or open; if open then keep using through the
    extension; if not open, once that causes focus shift - ok - but then after opened continue using through the
    extension". The quiet start (no window) is tried first; when it doesn't bring the extension, the profile's window
    is opened and the task carries on through the extension."""
    import ferb.act.browser.ext_bridge as eb
    monkeypatch.setattr(eb, "WINDOWLESS_WAIT_S", 0.05)
    d, launched = bd
    ext = pair(d, launched, "Work")
    ext.wait_for(lambda: ext.gen is not None)
    ext.kill()
    d.run(_start_wake(d))

    def on_launch(args: list[str]) -> None:
        launched.append(args)
        if args[-1] != "--no-startup-window":
            ext.connect()                                                       # only the window brings it
    d.daemon.router.launch = on_launch                                          # type: ignore[union-attr]
    r = call(d, task(d, "w", OWNER, "use my Work profile"), "browser.open", url="https://example.com/", profile="Work")
    assert not r["is_error"], r
    assert launched[-2][-1] == "--no-startup-window" and launched[-1][1:] == ["--profile-directory=Profile 1"]
    assert "https://example.com/" in ext.tabs.values()


def test_taking_over_a_tab_reconnects_its_profile_first(bd: Any) -> None:
    """A tab listed before FERB's connection to that profile dropped (a restart) is still the owner's open tab: the
    profile reconnects through the same check before FERB takes it over."""
    d, launched = bd
    ext = pair(d, launched, "Work")
    ext.wait_for(lambda: ext.gen is not None)
    ext.tabs[41], ext.tab_windows[41] = "https://news.example/", 7
    g = task(d, "w", OWNER, "use my news tab")
    listed = call(d, g, "browser.tabs", profile="Work")["content"]["tabs"]
    ref = next(t["tab_ref"] for t in listed if t["url"] == "https://news.example/")
    ext.kill()
    ext.wait_for(lambda: d.daemon.hub.bridges[("chrome", "Profile 1")].conn is None)
    d.run(_start_wake(d))

    def on_launch(args: list[str]) -> None:
        launched.append(args)
        ext.connect()
    d.daemon.router.launch = on_launch                                          # type: ignore[union-attr]
    r = call(d, g, "browser.use_tab", tab_ref=ref)
    assert not r["is_error"], r


async def _start_wake(d: RunningDaemon) -> None:
    d.daemon.router.wake = d.daemon.wake                                        # type: ignore[union-attr]


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
  vm.runInContext("pairingPending = Date.now()", ctx);            // this profile's pairing page handed over a token
  const paired = ctx.onDaemon({ type: "paired", key: NEW });
  const events = [1, 2, 3].map((i) => ctx.send("tab_event", { event: "updated", tab_id: i }));   // racing the reply
  await Promise.all([paired, ...events]);
  await new Promise((r) => setTimeout(r, 50));
  // the daemon's welcome, signed for this connection (protocol 18): the queued events leave after it
  const nonce = vm.runInContext("link.nonce", ctx), body = JSON.stringify({ gen: 1, identity: "owner" });
  const wmac = crypto.createHmac("sha256", Buffer.from(NEW, "base64")).update(`${nonce}|1|welcome|${body}`).digest("base64");
  await ctx.onDaemon({ type: "welcome", seq: 1, body, mac: wmac });
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


def test_the_profile_that_owns_a_window_gives_its_tabs_page(bd: Any) -> None:
    """A native capture of a browser window (L13, window_secrets): every connected profile of that browser is asked;
    exactly one must own a window with those bounds, and it answers with its active tab's mask state; none, or two
    (whose page is on screen can't be told), means "can't tell"."""
    d, launched = bd
    work, personal = pair(d, launched, "Work"), pair(d, launched, "Personal")
    personal.windows[(10, 20, 800, 600)] = [[20, 40, 100, 20]]
    work.windows[(0, 0, 500, 400)] = []
    work.windows[(5, 5, 300, 200)] = None
    ask = d.daemon._web_secrets
    page = d.run(ask("chrome", (10.0, 20.0, 800.0, 600.0)))
    assert page.complete and [i.rect for i in page.state.items] == [(20.0, 40.0, 100.0, 20.0)]
    assert page.state.viewport == (0.0, 0.0, 1280.0, 900.0)
    blank = d.run(ask("chrome", (0.0, 0.0, 500.0, 400.0)))
    assert blank.complete and blank.state.items == ()
    unread = d.run(ask("chrome", (5.0, 5.0, 300.0, 200.0)))
    assert not unread.complete and "the page can't be read" in unread.why       # its page can't be read: why
    # Nobody can tell, and the capture's note says why (fix wave 4: the live case's "neither read" said nothing more)
    nowhere = d.run(ask("chrome", (1.0, 1.0, 5.0, 5.0)))
    assert not nowhere.complete and "no paired profile has a window" in nowhere.why
    unpaired = d.run(ask("brave", (10.0, 20.0, 800.0, 600.0)))
    assert not unpaired.complete and "no paired profile of brave is connected" in unpaired.why
    work.windows[(7, 7, 400, 300)] = "ambiguous"                             # two of its windows at that place
    twice = d.run(ask("chrome", (7.0, 7.0, 400.0, 300.0)))
    assert not twice.complete and "more than one window" in twice.why and not twice.late
    work.windows[(8, 8, 400, 300)] = "fails"                                 # no answer (review M6)
    silent = d.run(ask("chrome", (8.0, 8.0, 400.0, 300.0)))
    assert not silent.complete and "didn't answer" in silent.why and silent.late   # the capture won't ask again
    assert not nowhere.late and not unread.late
    work.windows[(10, 20, 800, 600)] = []                                    # two profiles, same bounds
    both = d.run(ask("chrome", (10.0, 20.0, 800.0, 600.0)))
    assert not both.complete and "more than one paired profile" in both.why
    # round 3 (m4): both maximized, each showing its own page: the captured window's title picks the profile
    work.window_titles[(10, 20, 800, 600)] = "Work inbox"
    personal.window_titles[(10, 20, 800, 600)] = "Job Application – Panthalassa"
    picked = d.run(ask("chrome", (10.0, 20.0, 800.0, 600.0), "Job Application –  Panthalassa"))
    assert picked.complete and [i.rect for i in picked.state.items] == [(20.0, 40.0, 100.0, 20.0)]
    assert not d.run(ask("chrome", (10.0, 20.0, 800.0, 600.0), "Neither page")).complete


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
    monkeypatch.setattr(eb, "WINDOW_WAIT_S", 0.01)
    driver = getattr(daemon.daemon, driver_name)  # retain actual daemon wiring of both browser drivers

    async def run() -> None:
        bridge = SimpleNamespace(
            binding=SimpleNamespace(browser="chrome", folder="Profile 7", install_id="test-install"),
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
    # live 2026-10-26 (task 7c1eca3a): "reload the FERB extension" first made the worker ask the owner to reload it,
    # when the profile simply wasn't running
    assert "isn't running" in result['content']['error'] and 'reload' not in result['content']['error']
    # Owner live run 4530ea34: the worker read this as "needs the foreground" and scraped browser data first. The
    # fact it needs: browser.open loads the profile and opens the tab itself (live 2026-10-26: "may switch Spaces and
    # so needs the owner's permission" made a worker ask; owner: "if not open once that causes focus shift - ok").
    error = result['content']['error']
    assert 'browser.open' in error and 'minimized' in error and 'nothing to ask the owner' in error
    assert 'permission' not in error


# ---------- D31 review round 1, finding 6: no web terminal goes unrecognized through an older extension ----------

def test_an_extension_older_than_the_terminal_fact_is_refused_at_hello() -> None:
    """An extension before 0.15.11 never reports `terminal`, so a web terminal would get no question: the minimum is
    0.15.11 (published by the owner), and an older one meets the existing "reload the FERB extension" refusal."""
    import asyncio
    from types import SimpleNamespace
    from unittest.mock import AsyncMock
    from ferb.act.browser.ext_bridge import Hub
    from ferb.act.browser.ext_protocol import MIN_EXTENSION_VERSION, PROTOCOL_VERSION, ExtMessage, version_ok
    verdict = SimpleNamespace(accepted=True, reason=None, reply=None, binding=object())
    hub = Hub(_pairings(verdict), None, lambda *a: None, lambda: "test-device")
    writes: list[bytes] = []
    conn = SimpleNamespace(writer=SimpleNamespace(write=writes.append, drain=AsyncMock()), app=None)
    asyncio.run(hub._handle(conn, ExtMessage(install_id="test-install", type="hello", body_json=json.dumps(
        {"version": "0.15.10", "protocol": PROTOCOL_VERSION}))))
    reply = json.loads(writes[0])
    assert reply["type"] == "refused" and "too old" in reply["reason"]
    assert reply["minimum_version"] == MIN_EXTENSION_VERSION == "0.19.0"     # protocol 21 raised it past 0.15.11
    assert version_ok("0.19.0") and not version_ok("0.18.0") and not version_ok("0.15.10")


def test_a_reinstalled_unpaired_extension_says_pair_it_with_browser_pair(
        bd: Any, monkeypatch: pytest.MonkeyPatch) -> None:
    """Live 2026-10-26: Home_Personal's extension was reinstalled, so it connected with no pairing; the windowless wake
    waited for the old install and the worker drove the owner's screen instead. The error now names the cause and the
    one fix: the owner opens that profile's window once (FERB pairs it there), never a route around it."""
    import ferb.act.browser.ext_bridge as eb
    monkeypatch.setattr(eb, "WINDOWLESS_WAIT_S", 0.05)
    d, launched = bd
    old = pair(d, launched, "Work")
    old.wait_for(lambda: old.gen is not None)
    old.kill()
    old.wait_for(lambda: d.daemon.hub.bridges[('chrome', 'Profile 1')].conn is None)
    fresh = FakeExtension(d.daemon.home)
    fresh.connect()
    fresh.wait_for(lambda: fresh.install_id in d.daemon.hub.unpaired)
    d.run(_start_wake(d))
    d.daemon.router.launch = launched.append
    before = len(launched)
    try:
        g = task(d, "w", OWNER, "use my Work profile")
        r = call(d, g, "browser.open", url="https://example.com/", profile="Work")
    finally:
        fresh.kill()
    assert all(argv[-1] == "--no-startup-window" for argv in launched[before:])   # no window: only the owner pairs
    assert r["is_error"]
    err = r["content"]["error"]
    assert "not paired" in err and "browser_pair profile=chrome/Profile 1" in err and "Mac tools" in err


async def _journal(d: RunningDaemon, kind: str) -> list[dict[str, Any]]:
    return [e.data for e in d.daemon.journal.of_kind(kind)]


def _on_loop(d: RunningDaemon, fn: Any, *args: Any) -> Any:
    """`fn(*args)` on the daemon's loop thread (its database connection lives there)."""
    async def go() -> Any:
        return fn(*args)
    return d.run(go())


# decision: HAND-406
def test_a_too_old_extension_is_journaled_and_named_even_after_a_restart(
        bd: Any, monkeypatch: pytest.MonkeyPatch) -> None:
    """Live 2026-11-01 to 11-04: every owner profile still ran extension 0.15.12-0.15.13 while FERB needed 0.19.0, no
    refusal was recorded, and the planner was told the profiles probably weren't running. A refused hello is journaled
    with its reason, and the version kept in the pairing record still names the cause after a restart, when the old
    extension may never get as far as a hello again: in the planner's facts and in the browser tool's error."""
    import ferb.act.browser.ext_bridge as eb
    monkeypatch.setattr(eb, "WINDOWLESS_WAIT_S", 0.05)
    monkeypatch.setattr(eb, "WINDOW_WAIT_S", 0.05)
    d, launched = bd
    ext = pair(d, launched, "Work")
    ext.wait_for(lambda: ext.gen is not None)
    ext.kill()
    old = FakeExtension(d.daemon.home, protocol=17, version="0.15.13")
    old.install_id, old.key, old.seq = ext.install_id, ext.key, ext.seq
    old.connect()
    try:
        old.wait_for(lambda: old.refused is not None)
    finally:
        old.kill()
    [row] = d.run(_journal(d, "browser.connection_refused"))
    assert row["reason"] == "extension_too_old" and row["profile"] == "chrome/Profile 1"
    assert row["version"] == "0.15.13" and "too old" in row["why"]

    d.daemon.hub.refused.clear()                                                # a restart forgets this run's refusals
    facts = _on_loop(d, d.daemon._live_facts)
    assert "too old (version 0.15.13" in facts and "isn't running" not in facts
    c = Client(d.sock, timeout=30)
    try:
        assert "too old (version 0.15.13" in c.call("setup.browsers")["problem"]      # the app's browsers message
    finally:
        c.close()
    d.run(_start_wake(d))
    d.daemon.router.launch = launched.append
    r = call(d, task(d, "w", OWNER, "use my Work profile"), "browser.open", url="https://example.com/",
             profile="Work")
    assert r["is_error"] and "too old (version 0.15.13" in r["content"]["error"]
    assert launched[-1][1:] == ["--profile-directory=Profile 1"]               # HAND-403's window step still ran


# decision: HAND-406
def test_a_native_host_the_daemon_refuses_is_journaled(bd: Any) -> None:
    """Below the hello: the native host's auth.hello from an extension id FERB doesn't know only made the host exit."""
    d, _ = bd
    env = {"FERB_HOME": str(d.daemon.home), "PATH": "/usr/bin:/bin"}
    origin = "chrome-extension://" + "a" * 32 + "/"
    r = subprocess.run([sys.executable, "-m", "ferb.native_host.main", origin],
                       input=b"", capture_output=True, env=env, timeout=20)
    assert r.returncode == 2
    [row] = d.run(_journal(d, "browser.connection_refused"))
    assert row["reason"] == "host" and row["origin"] == origin


# decision: HAND-407
def test_without_full_disk_access_a_paired_profile_still_resolves_and_starts_from_ferbs_records(bd: Any) -> None:
    """Live 2026-11-02/03: with the Full Disk Access gate closed, discovery returned nothing and every named browser
    call failed with "no profile called 'brave/Default'; found: none", while the same profile was paired. A paired
    profile resolves by its folder from FERB's own pairing record, a name discovery can't see says why, and the wake
    starts that profile."""
    d, launched = bd
    ext = pair(d, launched, "Work")
    ext.wait_for(lambda: ext.gen is not None)
    ext.kill()
    ext.wait_for(lambda: d.daemon.hub.bridges[('chrome', 'Profile 1')].conn is None)
    d.daemon.data_access = lambda: False                                        # the gate is closed (DMN-507)
    assert d.daemon._profiles() == []
    resolver = d.daemon.router.resolver                                         # type: ignore[union-attr]
    assert _on_loop(d, resolver.choose, "chrome/Profile 1").folder == "Profile 1"
    with pytest.raises(NeedsOwner, match="Full Disk Access"):
        _on_loop(d, resolver.choose, "Work")                                    # its name needs discovery
    g = task(d, "w", OWNER, "use my Work profile")
    listed = call(d, g, "browser.profiles")["content"]
    assert [p["profile"] for p in listed["profiles"]] == ["chrome/Profile 1"] and "Full Disk Access" in listed["note"]
    d.run(_start_wake(d))

    def on_launch(args: list[str]) -> None:
        launched.append(args)
        ext.connect()
    d.daemon.router.launch = on_launch                                          # type: ignore[union-attr]
    try:
        r = call(d, g, "browser.open", url="https://example.com/", profile="chrome/Profile 1")
    finally:
        ext.kill()
    assert not r["is_error"], r
    assert launched[-1][-2:] == ["--profile-directory=Profile 1", "--no-startup-window"]


# decision: HAND-407
def test_a_paired_folder_missing_from_its_browsers_discovery_is_never_started() -> None:
    """HAND-407's limit: a browser whose profiles were read keeps only those, since starting a folder that's gone
    would create it; a paired profile of a browser discovery found nothing in comes from FERB's records."""
    O = Identity.OWNER
    r = _resolver([Binding("i1", "chrome", "Profile 9", O), Binding("i2", "vivaldi", "Default", O)])
    with pytest.raises(NeedsOwner, match="no profile called"):
        r.choose("chrome/Profile 9")
    assert r.choose("vivaldi/Default").browser == "vivaldi"
