"""Rework phase 3, browser (ARCHITECTURE.md), offline: the fake extension through the real native host and
a real daemon. Background by construction (one minimized FERB window per profile, tabs never active, the owner's
tab list never shows FERB's window), the debugger session per task tab (attached on first use, kept until close;
the owner's Cancel makes the act in flight unknown), dialogs owned by the extension, container scrolls, keepalive
pings, uploads, chunked screenshots, and the bridge fixes from the audit (timeouts reconciled, a second hello on
the same connection, close confirmed before the tab is forgotten, the YouTube helper tab read once).
"""

import asyncio
import json
import os
import subprocess
import time
from pathlib import Path
from typing import Any

import pytest

import ferb.act.browser.ext_bridge as eb
from ferb.act.browser.ext_bridge import OutcomeUnknown
from ferb.act.browser.ext_protocol import MIN_EXTENSION_VERSION, version_ok
from tests.fake_extension import FakeExtension
from tests.chromelab import EXT, SHOP, _ready, _node
from tests.browserkit import OWNER, bd, call, pair, task  # noqa: F401  (the fixture and its helpers)


def _bridge(d: Any) -> Any:
    return d.daemon.hub.bridges[("chrome", "Profile 1")]


async def _state(d: Any, cid: str) -> str:
    return str(d.daemon.ledger.get(cid).intent.state)


def _chrome_id(d: Any, tab: str) -> int:
    return int(d.daemon.router._tabs[tab].chrome_id)


# ---------- stage A: background by construction ----------

def test_tabs_open_inactive_in_one_minimized_ferb_window_that_the_owner_never_sees_listed(bd: Any) -> None:
    d, launched = bd
    ext, g = _ready(d, launched, verbs={"research"})
    ext.tabs[7], ext.tab_windows[7], ext.tab_active[7] = "https://news.example/", 1, True   # the owner's own window
    t1 = call(d, g, "browser.open", url=f"{SHOP}/a")["content"]["tab"]
    t2 = call(d, g, "browser.open", url=f"{SHOP}/b")["content"]["tab"]
    assert ext.windows_created == 1 and ext.window_state == "minimized"          # one window per profile, minimized
    for t in (t1, t2):
        cid = _chrome_id(d, t)
        assert ext.tab_windows[cid] == ext.ferb_window and ext.tab_active[cid] is False   # created inactive, in it
    ext.window_state = "normal"                                                  # the owner restored the window...
    call(d, g, "browser.open", url=f"{SHOP}/c")
    assert ext.windows_created == 1 and ext.window_state == "normal"             # preserve the owner's restored view
    listed = call(d, g, "browser.tabs")["content"]["tabs"]
    assert [t["site"] for t in listed] == ["news.example"]                       # FERB's window: never the owner's


# ---------- stage B: the debugger session per task tab ----------

def test_debugger_attaches_on_the_first_snapshot_and_stays_until_the_tab_closes(bd: Any) -> None:
    d, launched = bd
    ext, g = _ready(d, launched)
    tab = call(d, g, "browser.open", url=f"{SHOP}/form")["content"]["tab"]
    cid = _chrome_id(d, tab)
    snap = call(d, g, "browser.snapshot", tab=tab)["content"]
    assert cid in ext.attached and ext.attaches == 1                            # AX snapshot needs the debugger
    r = call(d, g, "browser.act", tab=tab, element=snap["elements"][0]["id"], action="type", text="Ada")
    assert not r["is_error"] and "(trusted)" in r["content"]["detail"]           # real input by default
    assert cid in ext.attached and ext.attaches == 1
    snap = call(d, g, "browser.snapshot", tab=tab)["content"]
    r = call(d, g, "browser.act", tab=tab, element=snap["elements"][1]["id"], action="click", effect="send",
             why="submitting the form the owner asked for")                      # a form's Submit is a send
    assert not r["is_error"], r                                                   # kept, not re-attached per action
    assert ext.attaches == 1
    snap = call(d, g, "browser.snapshot", tab=tab)["content"]
    r = call(d, g, "browser.act", tab=tab, element=snap["elements"][0]["id"], action="type", text="B", page_mode=True)
    assert "(page events)" in r["content"]["detail"] and ext.acts[-1]["page_mode"] is True
    call(d, g, "browser.close", tab=tab)
    assert cid not in ext.attached and cid not in ext.tabs                       # detached with the tab


def test_the_owners_cancel_on_the_debugger_bar_makes_the_act_in_flight_unknown(bd: Any) -> None:
    d, launched = bd
    ext, g = _ready(d, launched)
    tab = call(d, g, "browser.open", url=f"{SHOP}/form")["content"]["tab"]
    cid = _chrome_id(d, tab)
    snap = call(d, g, "browser.snapshot", tab=tab)["content"]
    ext.hold = True
    fut = asyncio.run_coroutine_threadsafe(d.daemon.gateway.call(g, "browser.act", {"tab": tab, "element": snap["elements"][1]["id"],
                                                                                    "action": "click", "effect": "send",
                                                                                    "why": "submitting the form"}), d.loop)
    ext.wait_for(lambda: any(m["op"] == "act" for m in ext.held))
    intent = next(m["id"] for m in ext.held if m["op"] == "act")
    ext.cancel_debugger(cid)                                                     # the owner pressed Cancel
    r = fut.result(10)
    assert r["is_error"] and "outcome unknown" in r["content"]["error"]
    assert d.run(_state(d, intent)) == "unknown"
    assert _bridge(d).detached[cid] == "canceled_by_user"                        # reported to the daemon
    ext.release_held()                                                           # (so the daemon can close the tab)


def test_keepalive_pings_while_a_task_tab_is_open(bd: Any, monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr(eb, "KEEPALIVE_S", 0.1)
    d, launched = bd
    ext, g = _ready(d, launched, verbs={"research"})
    assert ext.pings == 0
    tab = call(d, g, "browser.open", url=f"{SHOP}/a")["content"]["tab"]
    ext.wait_for(lambda: ext.pings >= 3, 3)                                      # the worker is kept awake (<= 20 s)
    call(d, g, "browser.close", tab=tab)
    time.sleep(0.3)
    n = ext.pings
    time.sleep(0.5)
    assert ext.pings == n                                                        # nothing open: no more pings


def test_kept_class_tab_survives_task_cleanup_and_reconnect(bd: Any) -> None:
    d, launched = bd
    ext, g = _ready(d, launched, verbs={"research"})
    tab = call(d, g, "browser.open", url=f"{SHOP}/class")["content"]["tab"]
    cid = _chrome_id(d, tab)
    call(d, g, "browser.snapshot", tab=tab)
    kept = call(d, g, "browser.keep_open", tab=tab)
    assert not kept['is_error'] and kept['content']['kept_open']
    assert cid not in ext.attached and cid in ext.tabs
    call(d, g, "browser.snapshot", tab=tab)  # a verifier's fresh read may attach again
    assert cid in ext.attached
    report = d.run(d.daemon.router.release(g.task_id))
    assert report[0]['kept_open'] and report[0]['released'] and not report[0]['closed']
    assert ext.tabs[cid] == f"{SHOP}/class" and cid not in ext.attached
    ext.kill()
    ext.connect()
    ext.wait_for(lambda: ext.gen is not None and ext.gen >= 2)
    assert ext.tabs[cid] == f"{SHOP}/class" and cid in ext.kept
    listed = call(d, g, 'browser.tabs')['content']['tabs']
    assert any(t['tab_ref'].endswith(f':{cid}') for t in listed)


# ---------- stage C/D: snapshot semantics, container scroll, dialogs, uploads, screenshots ----------

def test_snapshot_reports_viewport_and_scrollers_and_scroll_targets_a_container(bd: Any) -> None:
    d, launched = bd
    ext, g = _ready(d, launched, verbs={"research"})
    tab = call(d, g, "browser.open", url=f"{SHOP}/chat")["content"]["tab"]
    snap = call(d, g, "browser.snapshot", tab=tab)["content"]
    assert snap["scroll"]["y"] == 0 and snap["scrollers"][0]["id"] == "c1"
    rows = {e["label"]: e for e in snap["elements"]}
    assert "in_view" not in rows["message 1"] and rows["message 1"]["container"] == "c1"
    assert rows["message 2"]["in_view"] is False and rows["message 2"]["offscreen"] == "below"
    r = call(d, g, "browser.scroll", tab=tab, direction="down", container="c1")["content"]
    assert r == {"y": 340, "page_height": 2000, "at_end": False, "viewport_h": 400, "container": "c1", "frame": 0}
    assert ext.acts == [] and _chrome_id(d, tab) in ext.attached                # a real wheel, through the debugger
    r = call(d, g, "browser.scroll", tab=tab, direction="down")["content"]
    assert r["y"] == 765 and "container" not in r                                # the page itself
    r = call(d, g, "browser.scroll", tab=tab, direction="down", container="c9")
    assert r["is_error"] and "no container c9" in r["content"]["error"]


def test_dialogs_are_dismissed_unless_the_act_declared_one(bd: Any) -> None:
    d, launched = bd
    ext, g = _ready(d, launched)
    tab = call(d, g, "browser.open", url=f"{SHOP}/dialog")["content"]["tab"]
    snap = call(d, g, "browser.snapshot", tab=tab)["content"]
    confirm = next(e["id"] for e in snap["elements"] if e["label"] == "Confirm")
    r = call(d, g, "browser.act", tab=tab, element=confirm, action="click")
    assert not r["is_error"], r
    assert ext.dialogs[-1]["handled"] == "dismissed"
    snap = call(d, g, "browser.snapshot", tab=tab)["content"]
    assert snap["dismissed_dialogs"] == ["confirm: Sure? (dismissed)"]          # the worker learns what happened
    confirm = next(e["id"] for e in snap["elements"] if e["label"] == "Confirm")
    r = call(d, g, "browser.act", tab=tab, element=confirm, action="click", expect_dialog="accept")
    assert not r["is_error"] and ext.dialogs[-1]["handled"] == "accepted"
    assert ext.acts[-1]["expect_dialog"] == "accept"


def test_uploads_go_through_the_stamped_file_field_and_only_real_files(bd: Any, tmp_path: Path) -> None:
    d, launched = bd
    ext, g = _ready(d, launched)
    f = tmp_path / "cv.pdf"
    f.write_bytes(b"%PDF-1.4 test")
    tab = call(d, g, "browser.open", url=f"{SHOP}/upload")["content"]["tab"]
    snap = call(d, g, "browser.snapshot", tab=tab)["content"]
    field = next(e["id"] for e in snap["elements"] if e.get("type") == "file")
    r = call(d, g, "browser.act", tab=tab, element=field, action="upload", files=[str(f)])
    assert not r["is_error"], r
    assert ext.uploads[-1]["paths"] == [str(f)] and "1 file(s)" in r["content"]["detail"]
    snap = call(d, g, "browser.snapshot", tab=tab)["content"]
    field = next(e["id"] for e in snap["elements"] if e.get("type") == "file")
    r = call(d, g, "browser.act", tab=tab, element=field, action="upload", files=[str(tmp_path / "nope.pdf")])
    assert r["is_error"] and "no such file" in r["content"]["error"] and len(ext.uploads) == 1


def test_screenshots_arrive_in_parts_under_the_message_limit(bd: Any) -> None:
    d, launched = bd
    ext, g = _ready(d, launched, verbs={"research"})
    ext.screenshot_bytes = 1_000_000                                            # ~1.33 MB of base64: never one line
    tab = call(d, g, "browser.open", url=f"{SHOP}/a")["content"]["tab"]
    router = d.daemon.router
    ref = router.owned_by(tab, "x")
    shot = d.run(router.screenshot(ref), timeout=30)
    assert len(shot) == 1_000_000
    assert max(len(w) for w in ext.wire) < 1024 * 1024                          # the native host's line limit


# ---------- stage E: bridge correctness ----------

def test_a_timed_out_command_is_reconciled_from_the_next_hello(bd: Any) -> None:
    d, launched = bd
    ext, g = _ready(d, launched)
    tab = call(d, g, "browser.open", url=f"{SHOP}/form")["content"]["tab"]
    br, cid = _bridge(d), _chrome_id(d, tab)

    async def intend() -> str:
        row = d.daemon.ledger.intend("x", "browser.act", {"site": "shop.example"}, "h")
        d.daemon.ledger.mark(row.id, "executing")
        return str(row.id)

    async def timed_out(iid: str) -> None:
        try:
            await br.call("act", {"tab_id": cid, "frame_id": 0, "snap": 1, "el": 1, "kind": "click"}, timeout=0.2, cid=iid)
        except OutcomeUnknown:
            d.daemon.ledger.mark(iid, "unknown", ["timed out"])                  # what the gateway does
            return
        raise AssertionError("should have timed out")
    iid = d.run(intend())
    ext.hold = True
    d.run(timed_out(iid), timeout=20)
    assert iid in br.lost
    ext.seen[iid] = "done"                                                       # the worker did finish it
    ext.kill()
    ext.connect()
    ext.wait_for(lambda: ext.gen is not None and ext.gen >= 2)
    time.sleep(0.2)
    assert d.run(_state(d, iid)) == "done" and iid not in br.lost


def test_a_second_hello_on_the_same_connection_keeps_in_flight_commands(bd: Any) -> None:
    d, launched = bd
    ext, g = _ready(d, launched)
    tab = call(d, g, "browser.open", url=f"{SHOP}/form")["content"]["tab"]
    snap = call(d, g, "browser.snapshot", tab=tab)["content"]
    gen0 = _bridge(d).gen
    ext.hold = True
    fut = asyncio.run_coroutine_threadsafe(d.daemon.gateway.call(g, "browser.act", {"tab": tab, "element": snap["elements"][0]["id"],
                                                                                    "action": "type", "text": "Ada"}), d.loop)
    ext.wait_for(lambda: any(m["op"] == "act" for m in ext.held))
    ext.hello()                                                                  # again, same connection
    time.sleep(0.3)
    ext.release_held()
    r = fut.result(10)
    assert not r["is_error"], r                                                  # answered, not stranded
    assert _bridge(d).gen == gen0 and _bridge(d).conn is not None


def test_close_forgets_the_tab_only_once_the_browser_confirmed(bd: Any) -> None:
    d, launched = bd
    ext, g = _ready(d, launched, verbs={"research"})
    tab = call(d, g, "browser.open", url=f"{SHOP}/a")["content"]["tab"]
    router = d.daemon.router
    ext.hold = True
    fut = asyncio.run_coroutine_threadsafe(router.close(router.owned_by(tab, "x")), d.loop)
    ext.wait_for(lambda: any(m["op"] == "close" for m in ext.held))
    ext.kill()                                                                   # no confirmation ever comes
    with pytest.raises(OutcomeUnknown):
        fut.result(10)
    assert tab in router._tabs                                                   # still FERB's until Chrome says
    ext.hold = False
    ext.connect()
    ext.wait_for(lambda: ext.gen is not None and ext.gen >= 2)
    tab2 = call(d, g, "browser.open", url=f"{SHOP}/b")["content"]["tab"]
    call(d, g, "browser.close", tab=tab2)
    assert tab2 not in router._tabs and _chrome_id(d, tab) not in ext.attached


def test_actions_do_not_open_account_helper_tabs_or_block_account_mismatch(bd):
    d, launched = bd
    ext = pair(d, launched, "Personal")
    ext.wait_for(lambda: ext.gen is not None)
    g = task(d, "y", OWNER, {"fill_form"})
    tab = call(d, g, "browser.open", url="https://www.youtube.com/watch?v=x", profile="Personal")["content"]["tab"]
    ext.accounts["www.youtube.com"] = "someone.else@gmail.com"
    opens = ext.opens
    for _ in range(3):
        snap = call(d, g, "browser.snapshot", tab=tab)["content"]
        assert not call(d, g, "browser.act", tab=tab, element=snap["elements"][0]["id"], action="click")["is_error"]
    assert ext.opens == opens


def test_an_older_extension_is_refused_until_reloaded(bd: Any) -> None:
    d, launched = bd
    ext = pair(d, launched, "Work")
    ext.kill()
    old = FakeExtension(d.daemon.home, version="0.6.9")
    old.install_id, old.key, old.seq = ext.install_id, ext.key, ext.seq
    old.connect()
    old.wait_for(lambda: old.refused is not None)
    assert "reload the FERB extension" in (old.refused or "") and MIN_EXTENSION_VERSION in (old.refused or "")
    assert version_ok(MIN_EXTENSION_VERSION) and not version_ok("0.10.2") and version_ok("0.12.0") and not version_ok("0.11.1") and not version_ok("0.6.9")
    assert json.loads((EXT / "manifest.json").read_text())["version"] == MIN_EXTENSION_VERSION   # bumped together


# ---------- the extension's own code, run in node ----------


def test_extension_allowlist_covers_exactly_the_session_setup_input_dialogs_and_uploads() -> None:
    node = _node()
    script = f"""
const p = require({json.dumps(str(EXT / "protocol.js"))});
const cases = [
  ["Target.setAutoAttach", {{autoAttach: true, waitForDebuggerOnStart: false, flatten: true}}],
  ["Target.setAutoAttach", {{autoAttach: true, waitForDebuggerOnStart: true, flatten: true}}],
  ["Target.attachToTarget", {{targetId: "x", flatten: true}}], ["Target.getTargets", {{}}],
  ["Emulation.setDeviceMetricsOverride", {{width: 1280, height: 900, deviceScaleFactor: 1, mobile: false}}],
  ["Emulation.setDeviceMetricsOverride", {{width: 10, height: 10, deviceScaleFactor: 1, mobile: false}}],
  ["Page.enable", {{}}], ["Page.handleJavaScriptDialog", {{accept: false}}], ["Page.handleJavaScriptDialog", {{}}],
  ["Input.dispatchKeyEvent", {{type: "keyDown", key: "a", modifiers: 4, commands: ["selectAll"]}}],
  ["Input.dispatchKeyEvent", {{type: "keyDown", key: "a", commands: ["evil"]}}],
  ["Input.dispatchMouseEvent", {{type: "mouseWheel", x: 1, y: 1, deltaX: 0, deltaY: 400}}],
  ["Input.dispatchMouseEvent", {{type: "mousePressed", x: 1, y: 1, button: "right", clickCount: 1}}],
  ["Input.dispatchMouseEvent", {{type: "mousePressed", x: 1, y: 1, button: "back"}}],
  ["DOM.getDocument", {{depth: 0}}], ["DOM.getDocument", {{depth: 2}}], ["DOM.getDocument", {{depth: 0, pierce: true}}],
  ["DOM.querySelector", {{nodeId: 1, selector: '[data-ferb-node="0123456789abcdef"]'}}],
  ["DOM.querySelector", {{nodeId: 1, selector: 'input[type=password]'}}],
  ["DOM.setFileInputFiles", {{nodeId: 3, files: ["/tmp/a.pdf"]}}], ["DOM.setFileInputFiles", {{nodeId: 3, files: ["a.pdf"]}}],
  ["DOM.getBoxModel", {{backendNodeId: 5}}], ["Accessibility.getFullAXTree", {{depth: 60}}],
  ["Runtime.evaluate", {{expression: "1"}}], ["Network.getAllCookies", {{}}], ["Storage.getCookies", {{}}],
  ["Page.captureScreenshot", {{format: "jpeg", quality: 70}}], ["Page.captureScreenshot", {{format: "webp"}}],
];
console.log(JSON.stringify(cases.map(([m, a]) => p.cdpAllowed(m, a))));
console.log(JSON.stringify(p.cdpFilter("DOM.getBoxModel", {{model: {{content: [1]}}, extra: 1}})));
"""
    out = subprocess.run([node, "-e", script], capture_output=True, text=True, timeout=20).stdout.splitlines()
    assert json.loads(out[0]) == [True, False, False, False, True, False, True, True, False, True, False, True, True,
                                  False, True, False, False, True, False, True, False, True, True, False, False,
                                  False, True, False]
    assert json.loads(out[1]) == {"model": {"content": [1]}}


def test_key_table_gives_codes_modifiers_text_and_mac_commands() -> None:
    node = _node()
    script = f"""
const p = require({json.dumps(str(EXT / "protocol.js"))});
console.log(JSON.stringify(["Enter", "cmd+a", "A", "!", "shift+Tab", "ctrl+Enter", "cmd+shift+z", "Backspace",
  "alt+left", "esc", "bogus", "hyper+a", "cmd+ArrowRight", " "].map((s) => p.keyEvent(s))));
"""
    out = json.loads(subprocess.run([node, "-e", script], capture_output=True, text=True, timeout=20).stdout)
    enter, cmd_a, upper, bang, stab, centr, redo, bs, altl, esc, bogus, hyper, cmdr, space = out
    assert enter == {"key": "Enter", "code": "Enter", "keyCode": 13, "text": "\r", "modifiers": 0}
    assert cmd_a == {"key": "a", "code": "KeyA", "keyCode": 65, "modifiers": 4, "commands": ["selectAll"]}   # no text
    assert upper["modifiers"] == 8 and upper["text"] == "A" and upper["code"] == "KeyA"
    assert bang == {"key": "!", "code": "Digit1", "keyCode": 49, "text": "!", "modifiers": 8}
    assert stab == {"key": "Tab", "code": "Tab", "keyCode": 9, "modifiers": 8}
    assert centr["modifiers"] == 2 and "text" not in centr
    assert redo["modifiers"] == 12 and redo["commands"] == ["redo"]
    assert bs["commands"] == ["deleteBackward"] and altl["commands"] == ["moveWordLeft"]
    assert esc["commands"] == ["cancelOperation"] and cmdr["commands"] == ["moveToEndOfLine"]
    assert bogus is None and hyper is None
    assert space == {"key": " ", "code": "Space", "keyCode": 32, "text": " ", "modifiers": 0}


OUTBOX_HARNESS = r"""
const vm = require("vm"), fs = require("fs"), path = require("path");
const dir = process.argv[1], KEY = process.argv[2];
const store = { ferb: { installId: "inst-1", key: KEY, seq: 0 } }, posted = [];
let connects = 0;
const later = (v) => new Promise((r) => setTimeout(() => r(v), 2));
const noop = new Proxy(function () {}, { get: () => noop, apply: () => undefined });
const chrome = new Proxy({
  storage: { local: { get: (k) => later({ [k]: store[k] && JSON.parse(JSON.stringify(store[k])) }),
                      set: (o) => { Object.assign(store, JSON.parse(JSON.stringify(o))); return later(); } },
             session: { get: () => later({}), set: () => later() } },
  runtime: new Proxy({ connectNative: () => { if (connects++ === 0) throw new Error("host down");
                                             return { postMessage: (m) => posted.push(m), onMessage: noop, onDisconnect: noop }; },
                       getManifest: () => ({ version: "t" }) }, { get: (t, k) => (k in t ? t[k] : noop) }),
}, { get: (t, k) => (k in t ? t[k] : noop) });
const ctx = vm.createContext({ chrome, crypto: globalThis.crypto, btoa, atob, TextEncoder, console, setTimeout,
                               clearTimeout, navigator: {}, URL });
ctx.importScripts = (f) => vm.runInContext(fs.readFileSync(path.join(dir, f), "utf8"), ctx);
vm.runInContext(fs.readFileSync(path.join(dir, "background.js"), "utf8"), ctx);
(async () => {
  await new Promise((r) => setTimeout(r, 20));                      // the first connect failed: nothing posted
  await ctx.send("tab_event", { event: "updated", tab_id: 1 });    // queued while disconnected
  await ctx.send("tab_event", { event: "updated", tab_id: 2 });
  await ctx.ensurePort();                                           // reconnect: hello goes out
  await new Promise((r) => setTimeout(r, 50));
  console.log(JSON.stringify(posted.map((m) => [m.type, m.seq])));
})();
"""


def test_queued_messages_leave_before_the_reconnect_hello_in_seq_order() -> None:
    """The hello on reconnect used to jump the outbox: the daemon then dropped the queued messages as replays."""
    node = _node()
    import base64
    key = base64.b64encode(os.urandom(32)).decode()
    r = subprocess.run([node, "-e", OUTBOX_HARNESS, str(EXT), key], capture_output=True, text=True, timeout=20)
    out = json.loads(r.stdout or "null") or pytest.fail(r.stderr)
    assert out == [["tab_event", 1], ["tab_event", 2], ["hello", 3]], out


def test_dead_ops_are_gone_and_the_op_lists_match() -> None:
    from ferb.act.browser.ext_protocol import Op
    py = set(Op.__args__)
    js = set(json.loads(subprocess.run([_node(), "-e", f'console.log(JSON.stringify(require({json.dumps(str(EXT / "protocol.js"))}).OPS))'],
                                       capture_output=True, text=True, timeout=20).stdout))
    assert py == js
    assert not {"attach_debugger", "detach_debugger", "cdp"} & py
    src = (EXT / "background.js").read_text()
    assert "WALKER" not in Path("ferb/act/browser/snapshot.py").read_text()
    assert 'state: "normal"' not in src and "focused: true" not in src            # never shown, never focused
    assert 'state: "minimized", focused: false' in src and "active: false" in src
    assert "autoDiscardable: false" in src
