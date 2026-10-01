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
from datetime import timedelta
from pathlib import Path
from typing import Any, Iterator

import pytest

from ferb.act.browser.ext_protocol import FERB_EXTENSION_ID
from ferb.act.browser.pairing import Binding
from ferb.act.browser.profiles import BrowserProfile, discover, open_known_profile, resolve
from ferb.act.browser.resolver import NeedsOwner, Resolver
from ferb.cli.client import Client
from ferb.core.types import Identity, Task
from ferb.orchestrator.tree import TaskRecord
from ferb.policy.intent import RequestIntent
from tests.conftest import RunningDaemon
from tests.fake_extension import FakeExtension

PROFILES = {"Default": ("Personal", "me@gmail.com"), "Profile 1": ("Work", "me@work.com"),
            "Profile 22": ("FERB", "ferb@gmail.com")}


def chrome_data(root: Path) -> Path:
    d = root / "Chrome"
    d.mkdir(parents=True)
    cache = {f: {"name": n, "user_name": e} for f, (n, e) in PROFILES.items()}
    (d / "Local State").write_text(json.dumps({"profile": {"info_cache": cache, "last_used": "Profile 1"}}))
    return d


@pytest.fixture
def bd(daemon: RunningDaemon) -> Iterator[tuple[RunningDaemon, list[list[str]]]]:
    d = daemon.daemon
    launched: list[list[str]] = []
    d.browser_data_dirs = {"chrome": chrome_data(d.home.parent)}
    d.browser_launch = launched.append
    d.browser_default = lambda: "chrome"                                      # last used in the fake: Profile 1
    d.pair_load_wait_s = 0.0
    d.router.launch = launched.append                                         # type: ignore[union-attr]
    d.router.wake = None                                                      # type: ignore[union-attr]
    d.config.set("identity", "browser", "chrome")
    d.config.set("identity", "profile", "Profile 22")
    yield daemon, launched


def pair(d: RunningDaemon, launched: list[list[str]], name: str) -> FakeExtension:
    ext = FakeExtension(d.daemon.home)
    ext.connect()
    c = Client(d.sock, timeout=30)
    try:
        import threading
        out: dict[str, Any] = {}
        t = threading.Thread(target=lambda: out.update(c.call("browsers.pair", {"profile": name, "timeout_s": 10})))
        n = len(launched)
        t.start()
        ext.wait_for(lambda: len(launched) > n and "pair.html#token=" in launched[-1][-1])
        ext.pair(launched[-1][-1].split("#token=")[1])
        t.join(15)
    finally:
        c.close()
    assert out.get("paired") is True, out
    assert launched[-1][1].startswith("--profile-directory=")
    return ext


def task(d: RunningDaemon, tid: str, scopes: set[str], verbs: set[str] | None = None, sites: frozenset[str] = frozenset()) -> Any:
    t = Task(tid, None, "g", "dev", Identity.OWNER, scopes, "actor", f"req-{tid}", [],
             d.daemon.clock.now() + timedelta(minutes=10))

    async def go() -> Any:
        root = d.daemon.spans.start("task", task_id=t.id, input={})
        d.daemon.tasks.add(TaskRecord(t, d.daemon.clock.now(), root.id))
        if verbs is not None:
            d.daemon.engine.intents.record(RequestIntent.from_plan(f"req-{tid}", verbs, sites=sites))
        return d.daemon.tokens.validate(d.daemon.tokens.issue(t.id, f"w-{tid}", scopes, Identity.OWNER, ttl_s=600))
    return d.run(go())


def call(d: RunningDaemon, grant: Any, name: str, **args: Any) -> dict[str, Any]:
    return d.run(d.daemon.gateway.call(grant, name, args), timeout=30)          # type: ignore[no-any-return]


OWNER = {"browser:read", "browser:act"}


# ---------- pairing, 21.16 ----------

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
                                                                                    "action": "click"}), d.loop)
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
    r2 = call(d, g, "browser.act", tab=tab, element=snap["elements"][1]["id"], action="click")
    assert r2["is_error"] and "older browser connection" in r2["content"]["error"]   # 21.13 after reconnect


def test_command_never_received_reconciles_failed(bd: Any) -> None:
    d, launched = bd
    ext = pair(d, launched, "Work")
    ext.wait_for(lambda: ext.gen is not None)
    g = task(d, "n", OWNER, {"fill_form"})
    d.daemon.config.set("policy", "trusted_accounts", ["shop.example @ chrome/Profile 1"])
    tab = call(d, g, "browser.open", url="https://shop.example/form")["content"]["tab"]
    snap = call(d, g, "browser.snapshot", tab=tab)["content"]
    ext.hold = True
    import asyncio
    fut = asyncio.run_coroutine_threadsafe(d.daemon.gateway.call(g, "browser.act", {"tab": tab, "element": snap["elements"][1]["id"],
                                                                                    "action": "click"}), d.loop)
    ext.wait_for(lambda: any(v == "started" for v in ext.seen.values()))
    cid = next(k for k, v in ext.seen.items() if v == "started")
    ext.kill()
    fut.result(10)
    ext.seen.pop(cid)                                                           # the worker never recorded it
    ext.hold = False
    ext.connect()
    ext.wait_for(lambda: ext.gen is not None and ext.gen >= 2)
    time.sleep(0.2)
    assert d.run(_state(d, cid)) == "failed"


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

def test_wrong_or_missing_account_refuses_consequential_actions(bd: Any) -> None:
    d, launched = bd
    ext = pair(d, launched, "Personal")                                          # profile account me@gmail.com
    ext.wait_for(lambda: ext.gen is not None)
    g = task(d, "m", OWNER, {"email"} | {"fill_form"}, sites=frozenset({"mail.google.com"}))
    tab = call(d, g, "browser.open", url="https://mail.google.com/mail/u/0/", profile="Personal")["content"]["tab"]
    snap = call(d, g, "browser.snapshot", tab=tab)["content"]
    to = next(e["id"] for e in snap["elements"] if e["label"] == "To")
    ext.accounts["mail.google.com"] = "someone.else@gmail.com"
    r = call(d, g, "browser.act", tab=tab, element=to, action="type", text="hi")
    assert r["is_error"] and "wrong account" in r["content"]["error"] and not ext.acts
    ext.accounts["mail.google.com"] = None
    r = call(d, g, "browser.act", tab=tab, element=to, action="type", text="hi")
    assert r["is_error"] and "not signed in" in r["content"]["error"] and not ext.acts
    ext.accounts["mail.google.com"] = "me@gmail.com"
    assert not call(d, g, "browser.act", tab=tab, element=to, action="type", text="hi")["is_error"]
    assert len(ext.acts) == 1


def test_unverifiable_site_asks_once_per_site_and_profile(bd: Any) -> None:
    d, launched = bd
    ext = pair(d, launched, "Work")
    ext.wait_for(lambda: ext.gen is not None)
    g = task(d, "u", OWNER, {"fill_form"})
    tab = call(d, g, "browser.open", url="https://shop.example/form")["content"]["tab"]
    snap = call(d, g, "browser.snapshot", tab=tab)["content"]
    name = snap["elements"][0]["id"]
    from tests.conftest import Answers
    yes = Answers(d)
    try:
        r = call(d, g, "browser.act", tab=tab, element=name, action="type", text="Ada")
    finally:
        yes.stop()
    assert not r["is_error"] and len(ext.acts) == 1 and "can't check which account" in yes.asked[0].reason
    assert "shop.example @ chrome/Profile 1" in d.daemon.config.get("policy", "trusted_accounts")
    snap = call(d, g, "browser.snapshot", tab=tab)["content"]
    r = call(d, g, "browser.act", tab=tab, element=snap["elements"][0]["id"], action="type", text="Bob")
    assert not r["is_error"] and len(ext.acts) == 2 and len(yes.asked) == 1      # not asked again


# ---------- resolver + discovery (deterministic, before any model) ----------

def _resolver(bindings: list[Binding], hosts: list[tuple[str, str, str]], default: str | None = None,
              last: str | None = None) -> Resolver:
    profiles = [BrowserProfile("chrome", "Default", "Personal", "me@gmail.com"),
                BrowserProfile("chrome", "Profile 1", "Work", "me@work.com"),
                BrowserProfile("brave", "Default", "Brave me", ""),
                BrowserProfile("chrome", "Profile 22", "FERB", "ferb@gmail.com")]
    return Resolver(lambda: profiles, lambda: bindings, lambda: hosts, lambda: ("chrome", "Profile 22"),
                    prefer=lambda: ["brave", "chrome"], default_browser=lambda: default,
                    last_used=lambda b: last)


def test_resolver_rules() -> None:
    O = Identity.OWNER
    paired = [Binding("i1", "chrome", "Default", O), Binding("i2", "chrome", "Profile 1", O),
              Binding("i3", "brave", "Default", O), Binding("i4", "chrome", "Profile 22", Identity.FERB)]
    r = _resolver(paired, [("chrome", "Profile 1", "mail.google.com")])
    assert r.choose("mail.google.com").folder == "Profile 1"                    # already open there
    assert r.choose("mail.google.com", "Personal").folder == "Default"          # named profile wins
    with pytest.raises(NeedsOwner, match="several profiles"):
        _resolver(paired, [("chrome", "Default", "x.com"), ("chrome", "Profile 1", "x.com")]).choose("x.com")
    c = _resolver(paired, [("chrome", "Default", "x.com"), ("brave", "Default", "x.com")]).choose("x.com")
    assert (c.browser, c.folder) == ("brave", "Default")                        # prefer_for_actions
    assert _resolver(paired, [], "chrome", "Profile 1").choose("new.com").folder == "Profile 1"   # default + last used
    with pytest.raises(NeedsOwner, match="which browser profile"):
        _resolver(paired, []).choose("new.com")
    with pytest.raises(NeedsOwner, match="no profile called 'FERB'"):
        _resolver(paired, []).choose("x.com", "FERB")                          # identity profile never offered
    ferb_open = _resolver(paired, [("chrome", "Profile 22", "x.com")])
    with pytest.raises(NeedsOwner):
        ferb_open.choose("x.com")                                               # a FERB-profile tab isn't a match


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
    open_known_profile("chrome", "Profile 1", "https://x/", ps, launch=launched.append)
    assert launched[0] == ["open", "-g", "-b", "com.google.Chrome", "--args", "--profile-directory=Profile 1",
                           "https://x/"]                                 # background: never takes focus (spike 19)
    open_known_profile("chrome", "Profile 1", "https://x/", ps, launch=launched.append, background=False)
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


def test_an_asleep_extension_in_a_loaded_profile_gets_a_wake_page_then_it_is_closed(
        bd: Any, monkeypatch: pytest.MonkeyPatch) -> None:
    import ferb.act.browser.ext_bridge as eb
    monkeypatch.setattr(eb, "WINDOWLESS_WAIT_S", 0.3)
    d, launched = bd
    ext = pair(d, launched, "Work")
    ext.wait_for(lambda: ext.gen is not None)
    ext.kill()                                                                  # worker asleep / disconnected
    d.run(_start_wake(d))
    closed: list[str] = []
    ext.op_wake_close = lambda a, cid: (closed.append(a["url"]) if ext.tabs else None, {"closed": 1})[1]  # type: ignore[method-assign]

    def on_launch(args: list[str]) -> None:
        launched.append(args)
        if "/wake/" in args[-1]:                                                # only a page restarts the worker
            ext.connect()
    d.daemon.router.launch = on_launch                                          # type: ignore[union-attr]
    g = task(d, "w", OWNER, {"research"})
    r = call(d, g, "browser.open", url="https://example.com/", profile="Work")
    assert not r["is_error"], r
    assert launched[-2][-1] == "--no-startup-window"                            # tried without a window first
    wake = launched[-1][-1]
    assert launched[-1][:2] == ["open", "-g"] and "--profile-directory=Profile 1" in launched[-1]   # in the background
    assert "/wake/" in wake and wake.startswith("http://127.0.0.1:")
    assert closed == [wake]                                                     # exactly that page, nothing else
    assert list(ext.tabs.values())[-1] == "https://example.com/"               # ...after FERB's own tab was open


async def _start_wake(d: RunningDaemon) -> None:
    d.daemon.router.wake = d.daemon.wake                                        # type: ignore[union-attr]


def test_youtube_account_is_read_from_its_account_page_in_a_closed_helper_tab(bd: Any) -> None:
    d, launched = bd
    ext = pair(d, launched, "Personal")                                          # profile account me@gmail.com
    ext.wait_for(lambda: ext.gen is not None)
    g = task(d, "y", OWNER, {"fill_form"}, sites=frozenset({"www.youtube.com"}))
    tab = call(d, g, "browser.open", url="https://www.youtube.com/watch?v=x", profile="Personal")["content"]["tab"]
    snap = call(d, g, "browser.snapshot", tab=tab)["content"]
    ext.accounts["www.youtube.com"] = "someone.else@gmail.com"
    before = set(ext.tabs)
    r = call(d, g, "browser.act", tab=tab, element=snap["elements"][0]["id"], action="click")
    assert r["is_error"] and "wrong account" in r["content"]["error"] and not ext.acts
    assert set(ext.tabs) == before                                             # the helper tab was closed
    ext.accounts["www.youtube.com"] = "me@gmail.com"
    snap = call(d, g, "browser.snapshot", tab=tab)["content"]
    assert not call(d, g, "browser.act", tab=tab, element=snap["elements"][0]["id"], action="click")["is_error"]


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



def test_waking_a_profile_never_leaves_the_owner_in_the_browser() -> None:
    """Owner's report (2026-10-01): a search woke FERB's Chrome and moved them to it. The wake now goes through
    `open -g`; and if the browser still ends up in front, the owner's app comes straight back."""
    from ferb.daemon.main import Daemon
    calls: list[str] = []

    class Apps:
        front = "com.google.Chrome"

        def frontmost(self) -> Any:
            return type("A", (), {"bundle_id": self.front})()

        def activate(self, b: str) -> bool:
            calls.append(b)
            return True
    import ferb.act.mac.apps as real
    fake = Apps()
    saved = (real.frontmost, real.activate)
    real.frontmost, real.activate = fake.frontmost, fake.activate          # type: ignore[assignment]
    try:
        d = Daemon.__new__(Daemon)
        d._journal = lambda: type("J", (), {"append": lambda *a: None})()  # type: ignore[method-assign]
        d._put_owner_back("com.microsoft.VSCode")
        assert calls == ["com.microsoft.VSCode"]                           # Chrome took the front: given back
        fake.front = "com.microsoft.VSCode"
        d._put_owner_back("com.microsoft.VSCode")
        assert calls == ["com.microsoft.VSCode"]                           # the owner is where they were: nothing
        fake.front = "com.google.Chrome"
        d._put_owner_back("com.google.Chrome")
        assert calls == ["com.microsoft.VSCode"]                           # the owner was in Chrome: leave it
    finally:
        real.frontmost, real.activate = saved                              # type: ignore[assignment]
