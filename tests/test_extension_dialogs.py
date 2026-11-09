"""HAND-442..444 (extension 0.19.7), the extension's own code under node with fake Chrome APIs, and the daemon's side.

Live 2026-11-09 (the EF application task): the worker's navigate away from a page it had typed into raised the page's
"Leave site?" (beforeunload) dialog. The navigate waited 20 s and returned the old address as if it had loaded, the
next wait and snapshot hung 10 s and 30 s in the keep-awake step, the dialog sat on the owner's screen about 70 s, and
Chrome brought the tab forward and un-minimized its window. Earlier in the same task a Gmail link's new tab never
reached the worker (no opener link), and a wait for "joinef" was met by Gmail's own search address. The real-browser
half (Chrome's own beforeunload, the page hook, a noopener tab's opener) is tests/test_browser_dialogs_real.py."""

import asyncio
import json
import subprocess
from types import SimpleNamespace
from typing import Any

import pytest

from ferb.act.browser.tools import OWNER_BROWSER, BrowserNavigate
from ferb.core.errors import DriverError
from tests.chromelab import EXT, _node, worker
from tests.test_hangs_and_previews import _answer, _bridge

# A task tab (8) FERB drives. Every debugger command is recorded; `dialogOn` makes the next chrome.tabs.update or
# remove raise the page's dialog of that type through the debugger's event, unless the page hook was told to leave
# (the ":leave" event: its beforeunload listeners are skipped, so no dialog). Tab loads report at once.
SETUP = '''
      gen = 1; port = {postMessage() {}, disconnect() {}};
      const sent = [], events = [], cmds = [], scripts = [], ups = [];
      send = async (type, body) => { if (type === "result") sent.push(body); else events.push(body); };
      let dialogOn = null, leaveTold = false, url = "https://forms.example/apply";
      const fire = (tabId, type) => chrome.debugger.onEvent.fire({tabId}, "Page.javascriptDialogOpening", {type, message: "m"});
      const targets = {8: {targetId: "T8"}, 20: {targetId: "T20", openerId: "T8"}, 21: {targetId: "T21", openerId: "T99"}};
      const attached = [];
      chrome.debugger = new Proxy({
        attach: async (t) => { attached.push(t.tabId); }, detach: async (t) => { attached.push(-t.tabId); },
        sendCommand: async (target, method, params) => {
          cmds.push(method + (method === "Page.handleJavaScriptDialog" ? ":" + params.accept : ""));
          if (method === "Target.getTargetInfo") return {targetInfo: targets[target.tabId]};
          if (method === "Page.getFrameTree") return {frameTree: null};
          return {};
        },
        onEvent: chrome.debugger.onEvent, onDetach: chrome.debugger.onDetach}, {get: (o, k) => o[k]});
      chrome.scripting = {executeScript: async (inj) => {
        scripts.push(inj.args || []);
        if ((inj.args || []).some(a => String(a).endsWith(":leave"))) leaveTold = true;
        return [{frameId: 0, result: true}];
      }};
      chrome.tabs.onUpdated = {addListener: (f) => ups.push(f), removeListener: (f) => ups.splice(ups.indexOf(f), 1)};
      chrome.tabs.get = async id => ({id, windowId: id === 23 ? 5 : 9, url, status: "complete", title: "S"});
      chrome.tabs.update = async (id, p) => {
        if (p.url) {
          if (dialogOn && !leaveTold) { fire(id, dialogOn); return {id}; }   // the navigation waits on the dialog
          url = p.url; for (const f of [...ups]) f(id, {status: "loading"}); for (const f of [...ups]) f(id, {status: "complete"});
        }
        return {id};
      };
      chrome.tabs.remove = async (id) => { if (dialogOn && !leaveTold) { fire(id, dialogOn); return new Promise(() => {}); } };
      chrome.windows = {get: async () => ({left: 0, top: 0, width: 800, height: 600, state: "minimized", type: "normal"})};
      let n = 0;
      const command = async (op, args, timeout_ms = 30000) => {
        const id = "c" + (++n), from = cmds.length, t0 = Date.now();
        const done = onCommand({gen: 1, id, op, args: {tab_id: 8, ...args}, timeout_ms});
        return {done, id, cmds: () => cmds.slice(from), body: () => sent.find(b => b.id === id), ms: () => Date.now() - t0};
      };
      const finish = async (c) => { await c.done; return {body: c.body(), cmds: c.cmds(), ms: c.ms()}; };
'''


def run(body: str) -> dict[str, Any]:
    return worker("(async () => {" + SETUP + body + "})()")


# decision: HAND-442
def test_a_navigate_the_page_answers_with_a_dialog_ends_at_once_with_the_dialog_fact() -> None:
    """Live: the navigate waited 20 s and returned the old address, with no word of the dialog."""
    out = run('''
      await arm(8); dialogOn = "beforeunload";
      return await finish(await command("navigate", {url: "https://forms.example/next"}));
    ''')
    body, value = out["body"], out["body"]["value"]
    assert out["ms"] < 1500, out
    assert body["status"] == "ok" and body["outcome"] == "failed"         # nothing left the page yet: a known failure
    assert value["reason"] == "dialog_pending" and value["dialog"] == {"type": "beforeunload"}
    assert value["navigated"] is False and "m" not in value["dialog"]    # its text only through `dialog`
    assert not any(c.startswith("Page.handleJavaScriptDialog") for c in out["cmds"])   # code never answers it


# decision: HAND-442
def test_under_a_pending_dialog_reads_skip_the_wake_step_and_say_so_at_once() -> None:
    """Live: wait and snapshot each hung in the keep-awake step (10 s, 30 s) instead of naming the dialog."""
    out = run('''
      await arm(8); await (await command("snapshot", {snap: 1})).done;
      fire(8, "alert");
      const snap = await finish(await command("snapshot", {snap: 2}));
      const wait = await finish(await command("wait", {until: "text", value: "x"}));
      const act = await finish(await command("act", {kind: "click", snap: 2, el: 1}));
      return {snap, wait, act};
    ''')
    assert "Emulation.setFocusEmulationEnabled" not in out["snap"]["cmds"]
    assert out["snap"]["body"]["value"]["frames"][0]["text"].startswith("[Page blocked by pending dialog")
    for c in (out["wait"], out["act"]):
        assert c["ms"] < 1000 and c["body"]["status"] == "error" and c["body"]["error"].startswith("dialog_pending:")
        assert c["body"]["dialog"] == {"type": "alert"} and c["body"]["outcome"] == "failed"   # nothing was sent
        assert c["cmds"] == []


# decision: HAND-442
def test_a_dialog_opening_during_a_read_ends_it_at_once() -> None:
    out = run('''
      await arm(8);
      const c = await command("wait", {until: "url", value: "never-there"}, 10000);
      setTimeout(() => fire(8, "confirm"), 300);
      return await finish(c);
    ''')
    assert out["ms"] < 1500 and out["body"]["error"].startswith("dialog_pending:")
    assert out["body"]["dialog"] == {"type": "confirm"}


# decision: HAND-446
def test_leave_page_leave_tells_the_page_hook_and_accepts_a_dialog_raised_anyway() -> None:
    out = run('''
      await arm(8); dialogOn = "beforeunload";
      const told = await finish(await command("navigate", {url: "https://forms.example/next", leave_page: "leave"}));
      // the hook couldn't precede the page's listener (a page it reached late): the debugger answers within ms
      url = "https://forms.example/apply";
      chrome.scripting.executeScript = async (inj) => [{frameId: 0, result: true}];
      chrome.tabs.update = async (id, p) => {
        if (p.url) { fire(id, "beforeunload"); await new Promise(r => setTimeout(r, 20));
                     url = p.url; for (const f of [...ups]) f(id, {status: "loading"}); for (const f of [...ups]) f(id, {status: "complete"}); }
        return {id};
      };
      const late = await finish(await command("navigate", {url: "https://forms.example/last", leave_page: "leave"}));
      // an alert during a leave is not the leave's to answer: it ends the call as a fact
      return {told, late, leaveTold, scripts};
    ''')
    assert out["leaveTold"] and out["told"]["body"]["value"]["url"] == "https://forms.example/next"
    assert not any(c.startswith("Page.handleJavaScriptDialog") for c in out["told"]["cmds"])   # no dialog at all
    assert "Page.handleJavaScriptDialog:true" in out["late"]["cmds"]
    assert out["late"]["body"]["value"]["url"] == "https://forms.example/last" and out["late"]["ms"] < 1500


# decision: HAND-446
def test_leave_page_stay_dismisses_and_says_it_stayed_and_close_takes_the_choice_too() -> None:
    out = run('''
      await arm(8); dialogOn = "beforeunload";
      const stay = await finish(await command("navigate", {url: "https://forms.example/next", leave_page: "stay"}));
      await chrome.storage.local.set({ferb: {key: null}, ferbTabs: {}});
      await mutateRecords(r => { r.tabs[8] = {task: "x", root: "x", cid: "c0"}; });
      const close = await finish(await command("close", {leave_page: "stay"}));
      const plain = await finish(await command("close", {}));
      return {stay, close, plain};
    ''')
    assert out["stay"]["ms"] < 1500 and out["stay"]["body"]["value"]["reason"] == "stayed"
    assert "Page.handleJavaScriptDialog:false" in out["stay"]["cmds"]
    assert out["close"]["body"]["value"]["reason"] == "stayed" and out["close"]["ms"] < 1500
    assert out["plain"]["body"]["value"]["reason"] == "dialog_pending" and out["plain"]["ms"] < 1500


# decision: HAND-446
def test_the_page_hook_skips_the_pages_beforeunload_only_once_told_to_leave() -> None:
    """Listeners on the window run in the order they were added (measured), so the hook's goes first; the page's
    own listener, added after it, no longer runs once FERB says the worker chose to leave."""
    script = f'''
      const vm = require("vm"), p = require({json.dumps(str(EXT / "protocol.js"))});
      class HTMLElement {{ click() {{}} }}
      const win = new EventTarget(), doc = new EventTarget();
      win.open = () => null; doc.baseURI = "https://forms.example/";
      const ctx = vm.createContext({{window: win, document: doc, HTMLElement, HTMLAnchorElement: class extends HTMLElement {{}},
        HTMLAreaElement: class extends HTMLElement {{}}, EventTarget, CustomEvent, Event, URL, crypto, Date, setTimeout}});
      vm.runInContext(p.PAGE_HOOK, ctx);
      let ran = 0;
      win.addEventListener("beforeunload", (e) => {{ ran++; e.preventDefault(); }});
      const leave = () => {{ const e = new Event("beforeunload", {{cancelable: true}}); win.dispatchEvent(e); return e.defaultPrevented; }};
      const asked = leave();
      doc.dispatchEvent(new CustomEvent(p.FERB_TOKEN + ":leave"));
      const after = leave();
      console.log(JSON.stringify({{asked, after, ran}}));
    '''
    out = subprocess.run([_node(), "-e", script], capture_output=True, text=True, timeout=20)
    assert out.returncode == 0, out.stderr
    assert json.loads(out.stdout) == {"asked": True, "after": False, "ran": 1}


# decision: HAND-443
def test_a_tab_the_tasks_click_opened_without_an_opener_link_is_claimed_when_the_browser_names_the_opener() -> None:
    """Live: Gmail's "Continue application" link opened a tab that neither browser.tabs nor the click's result showed."""
    out = run('''
      await arm(8);
      tabCreated({id: 30, pendingUrl: "https://apply.example/before"});    // no input yet: not even looked at
      await new Promise(r => setTimeout(r, 20));
      const before = attached.slice();
      await (await command("act", {kind: "click", snap: 1, el: 1})).done;  // the task's input in tab 8
      tabCreated({id: 20, pendingUrl: "https://apply.example/continue"});  // opened by tab 8 (openerId T8)
      tabCreated({id: 21, pendingUrl: "https://other.example/"});          // opened by some other tab
      tabCreated({id: 22, pendingUrl: "about:blank"});                     // FERB's own open: blank first
      tabCreated({id: 23, pendingUrl: "https://mine.example/"});           // the owner's, in their own window
      await new Promise(r => setTimeout(r, 50));
      const ready = await commands.popup_ready({tab_id: 20});
      return {before, attached, popups: events.filter(e => e.event === "popup"), ready};
    ''')
    assert 30 not in out["before"]
    assert out["popups"] == [{"event": "popup", "tab_id": 20, "opener_tab_id": 8, "url": "https://apply.example/continue"}]
    assert 21 in out["attached"] and -21 in out["attached"]               # looked at, let go of at once
    assert 22 not in out["attached"] and 23 not in out["attached"]       # never touched
    assert out["ready"] == {"ready": True, "told": False}


# decision: HAND-444
def test_a_url_wait_looks_at_the_pages_own_address_not_its_query() -> None:
    """Live: a wait for "joinef" was met by Gmail's #search/from%3Ajoinef.com while the tab was still Gmail."""
    script = (f"const p = require({json.dumps(str(EXT / 'protocol.js'))}); const g = "
              "'https://mail.google.com/mail/u/0/#search/from%3Ajoinef.com+newer_than%3A1d'; console.log(JSON.stringify(["
              "p.urlHas(g, 'joinef'), p.urlHas('https://apply.joinef.com/app/x/?verification&a=1', 'joinef'), "
              "p.urlHas('https://apply.joinef.com/app/x/', 'https://apply.joinef.com/app'), "
              "p.urlHas('https://shop.example/cart?step=2', 'step=2'), p.urlHas(g, '#search'), "
              "p.urlHas('https://x.example/caf%C3%A9', 'café'), p.urlHas(g, ''), p.urlHas('about:blank', 'blank')]))")
    out = subprocess.run([_node(), "-e", script], capture_output=True, text=True, timeout=20)
    assert json.loads(out.stdout) == [False, True, True, False, True, True, True, True]
    # a value holding "?" asks about the query
    script2 = (f"const p = require({json.dumps(str(EXT / 'protocol.js'))}); "
               "console.log(JSON.stringify(p.urlHas('https://shop.example/cart?step=2', '?step=2')))")
    assert json.loads(subprocess.run([_node(), "-e", script2], capture_output=True, text=True, timeout=20).stdout)


# decision: HAND-446
def test_the_daemon_names_the_dialog_and_carries_the_workers_leave_choice() -> None:
    async def check() -> None:
        br = _bridge()
        with pytest.raises(DriverError) as held:
            await _answer(br, "wait", {"tab_id": 3, "until": "load"},
                          {"status": "error", "error": "dialog_pending: the page shows a alert dialog", "outcome": "failed",
                           "dialog": {"type": "alert"}})
        assert held.value.facts == {"reason": "dialog_pending", "dialog": {"type": "alert"}}

        calls: list[dict[str, Any]] = []
        ref = SimpleNamespace(tab_id="t1")

        class Driver:
            def owned_by(self, tab: str, owner: str, borrow: bool = True) -> Any:
                return ref

            async def navigate(self, tab: Any, url: str, **kw: Any) -> None:
                calls.append(kw)

            def tab_url(self, tab_id: str) -> str:
                return "https://forms.example/next"

        tool = BrowserNavigate(Driver(), OWNER_BROWSER)                      # type: ignore[arg-type]
        ctx = SimpleNamespace(task_id="x", observe_only=False)
        await tool.run(tool.Input(tab="t1", url="https://forms.example/next", leave_page="leave"), ctx)  # type: ignore[arg-type]
        await tool.run(tool.Input(tab="t1", url="https://forms.example/next"), ctx)                      # type: ignore[arg-type]
        assert calls == [{"leave_page": "leave"}, {}]
    asyncio.run(check())
