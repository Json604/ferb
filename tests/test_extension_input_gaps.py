"""HAND-504..513 (extension 0.19.10), the extension's own code under node with fake Chrome APIs, and the daemon's side:
the input gaps the lab (tests/test_browser_input_gaps_real.py) found. Live 2026-11-11, task 9e7c14ed: 8 minutes failing to
put text into a Google Doc, every type and press refused "target_occluded" while the document had the focus."""

import json
import time
from typing import Any

import pytest

from tests.browserkit import bd, call  # noqa: F401  (the fixture and its helpers)
from tests.chromelab import SHOP, _ready, worker
from tests.fake_extension import FakeExtension

# A task tab (8) FERB drives. Every debugger command is recorded; `rect` is what content.js says about the target
# (where it is, whether it is covered or outside the page); `page` answers every other page script by its function's
# name (isActive, focus, focusedSecret, ...); `pageCalls` lists them in order.
SETUP = '''
      gen = 1; const sent = [], cmds = [], pageCalls = [];
      send = async (type, body) => { if (type === "result") sent.push(body); };
      let lastMove = null, rect = {x: 50, y: 60, editable: true};
      const page = {isActive: true, focus: true, focusedSecret: false, documentKey: "d1", watchNear: true,
                    nearChange: {gone: false, state: ["value"], mutations: 1, scope: "surroundings"}};
      chrome.debugger = new Proxy({
        attach: async () => {}, detach: async () => {},
        sendCommand: async (target, method, params) => {
          cmds.push([method, params || {}]);
          if (method === "Input.dispatchMouseEvent" && params.type === "mouseMoved") lastMove = [params.x, params.y];
          return {};
        },
        onEvent: chrome.debugger.onEvent, onDetach: chrome.debugger.onDetach}, {get: (o, k) => o[k] || (() => {})});
      chrome.scripting = {executeScript: async ({func, args}) => {
        const src = String(func), name = (/__ferb\\.(\\w+)\\(/.exec(src) || [])[1] || "";
        pageCalls.push([name, args || []]);
        if (name === "rect") return [{frameId: 0, result: {fx: rect.x, fy: rect.y, ...rect}}];
        if (name === "pointerSeen") return [{frameId: 0, result: lastMove || []}];
        if (name in page) return [{frameId: 0, result: typeof page[name] === "function" ? page[name](...(args || [])) : page[name]}];
        return [{frameId: 0, result: true}];
      }};
      chrome.tabs.get = async id => ({id, windowId: 9, url: "https://docs.example/d/1"});
      let n = 0;
      const act = async (args) => {
        const id = "c" + (++n), from = cmds.length, calls = pageCalls.length;
        await onCommand({gen: 1, id, op: "act", args: {tab_id: 8, frame_id: 0, snap: 1, el: 3, ...args}});
        const body = sent.find(b => b.id === id);
        const c = cmds.slice(from);
        return {outcome: body.outcome, value: body.value || null, error: body.error || null,
                pressed: c.filter(([m, p]) => p.type === "mousePressed").length,
                moves: c.filter(([m, p]) => p.type === "mouseMoved").length,
                keys: c.filter(([m, p]) => m === "Input.dispatchKeyEvent" && p.type !== "keyUp").map(([m, p]) => p.key),
                texts: c.filter(([m]) => m === "Input.insertText").map(([m, p]) => p.text),
                page: pageCalls.slice(calls).map(([name]) => name)};
      };
'''


def run(body: str) -> Any:
    return worker("(async () => {" + SETUP + body + "})()")


# decision: HAND-504
def test_typing_into_a_field_under_the_pages_own_drawing_focuses_it_by_script_and_sends_the_keys() -> None:
    """Task 9e7c14ed: Google Docs' text-event frame sits under the canvas, so every type and press was refused."""
    out = run('''
      await act({kind: "click"});                                // the tab is prepared (readiness probe)
      rect = {x: 50, y: 60, occluded: true, cover: 'canvas', editable: true};
      const typed = await act({kind: "type", text: "Hello"});
      const pressed = await act({kind: "press", text: "Enter"});
      const clicked = await act({kind: "click"});
      rect = {keyboard: true, outside: "outside the page (off its left edge)", outside_error: "outside_page: ...",
              editable: true};
      const sheets = await act({kind: "type", text: "42"});
      const nope = await act({kind: "click"});
      rect = {keyboard: true, outside: "outside the page (off its left edge)", outside_error: "outside_page: ...",
              editable: true, secret: true};
      const secret = await act({kind: "type", text: "hunter2"});
      return {typed, pressed, clicked, sheets, nope, secret};
    ''')
    t = out["typed"]
    assert t["outcome"] == "done" and t["value"]["unpointed"] == "covered" and t["pressed"] == 0
    assert t["texts"] == ["Hello"] and "isActive" in t["page"] and "rect" in t["page"]
    assert out["pressed"]["outcome"] == "done" and out["pressed"]["keys"] == ["Enter"]
    assert out["clicked"]["value"]["ok"] is False and "target_occluded" in out["clicked"]["value"]["detail"]
    assert out["clicked"]["pressed"] == 0                         # a click still never goes to a covered element
    assert out["sheets"]["value"]["unpointed"] == "outside" and out["sheets"]["texts"] == ["42"]
    assert out["nope"]["value"]["detail"].startswith("outside_page") and out["nope"]["pressed"] == 0
    assert out["secret"]["value"] == {"ok": False, "detail": "secret_field"} and out["secret"]["texts"] == []


# decision: HAND-504
def test_typing_never_goes_into_a_secret_field_the_focus_landed_in() -> None:
    out = run('''
      page.focusedSecret = true;                                 // a widget around a password box
      return await act({kind: "type", text: "hunter2"});
    ''')
    assert out["value"]["detail"] == "secret_field" and out["texts"] == []


# decision: HAND-505
def test_typing_replaces_an_inputs_text_and_inserts_into_an_editor_at_its_caret() -> None:
    """A document typed with select-all first loses everything in it; a form field filled without it keeps old text."""
    out = run('''
      await act({kind: "click"});
      rect = {x: 50, y: 60, editable: true, field: "input"};
      const input = await act({kind: "type", text: "Asha"});
      rect = {x: 50, y: 60, editable: true, field: "rich"};
      const editor = await act({kind: "type", text: "One\\nTwo"});            // focused already: no click
      page.isActive = false; page.focus = true;
      const unfocused = await act({kind: "type", text: "x"});               // focused by script, the caret kept
      const asked = await act({kind: "type", text: "y", mode: "replace"});
      rect = {x: 50, y: 60, occluded: true, cover: "canvas", editable: true, field: "textarea"};
      const code = await act({kind: "type", text: "def f():\\n\\treturn 1"});
      return {input, editor, unfocused, asked, code};
    ''')
    assert out["input"]["value"]["mode"] == "replace" and out["input"]["keys"][0] == "a" and out["input"]["pressed"] == 1
    assert out["editor"]["value"]["mode"] == "insert" and out["editor"]["pressed"] == 0
    assert out["editor"]["keys"] == [] and out["editor"]["texts"] == ["One\nTwo"]    # line breaks as text: no Enter
    assert out["unfocused"]["pressed"] == 0 and "focus" in out["unfocused"]["page"]
    assert out["asked"]["value"]["mode"] == "replace" and out["asked"]["keys"][0] == "a"
    assert out["code"]["value"]["mode"] == "insert" and out["code"]["keys"] == ["Enter", "Tab"]
    assert out["code"]["texts"] == ["def f():", "return 1"]


# decision: HAND-506
def test_a_paste_nobody_took_is_a_known_failure_and_never_touches_the_clipboard() -> None:
    out = run('''
      page.act = (g, i, kind, text, key, extra) => ({ok: true, detail: "pasted", paste_taken: extra && extra.html === "<b>x</b>"});
      page.nearChange = {gone: false, state: [], mutations: 0, scope: "surroundings"};
      const ignored = await act({kind: "paste", text: "x"});
      const taken = await act({kind: "paste", text: "x", html: "<b>x</b>"});
      return {ignored, taken};
    ''')
    i, t = out["ignored"], out["taken"]
    assert i["value"]["ok"] is False and i["value"]["reason"] == "paste_not_taken" and i["outcome"] == "failed"
    assert t["value"]["ok"] is True and t["outcome"] == "done"
    for r in (i, t):                         # page events only: no real key (a cmd+v would paste the owner's clipboard)
        assert r["keys"] == [] and r["texts"] == [] and r["pressed"] == 0


# ---------- the daemon: what it sends, and what an older extension gets ----------

def _form(d: Any, g: Any, path: str = "/form") -> tuple[str, dict[str, str]]:
    tab = call(d, g, "browser.open", url=f"{SHOP}{path}")["content"]["tab"]
    snap = call(d, g, "browser.snapshot", tab=tab)["content"]
    return str(tab), {e["label"]: e["id"] for e in snap["elements"]}


# decision: HAND-505, HAND-506
def test_the_daemon_sends_the_mode_and_the_paste_and_refuses_a_paste_into_a_secret_field(bd: Any) -> None:  # noqa: F811
    d, launched = bd
    ext, g = _ready(d, launched)
    tab, ids = _form(d, g)
    typed = call(d, g, "browser.act", tab=tab, element=ids["Name"], action="type", text="Asha", mode="insert")
    pasted = call(d, g, "browser.act", tab=tab, element=ids["Name"], action="paste", text="Hi", html="<b>Hi</b>")
    assert not typed.get("is_error") and not pasted.get("is_error"), (typed, pasted)
    sent = [{k: a.get(k) for k in ("kind", "text", "mode", "append", "html")} for a in ext.acts[-2:]]
    assert sent == [{"kind": "type", "text": "Asha", "mode": "insert", "append": True, "html": None},
                    {"kind": "paste", "text": "Hi", "mode": None, "append": None, "html": "<b>Hi</b>"}]
    tab, ids = _form(d, g, "/login")
    n = len(ext.acts)
    secret = call(d, g, "browser.act", tab=tab, element=ids["Password"], action="paste", text="hunter2")
    assert "secret field" in str(secret) and len(ext.acts) == n          # refused before anything was sent


# decision: HAND-506
def test_an_older_extension_is_never_sent_a_paste(bd: Any, monkeypatch: pytest.MonkeyPatch) -> None:  # noqa: F811
    monkeypatch.setattr("tests.browserkit.FakeExtension", lambda home: FakeExtension(home, version="0.19.9"))
    d, launched = bd
    ext, g = _ready(d, launched)
    tab, ids = _form(d, g)
    out = call(d, g, "browser.act", tab=tab, element=ids["Name"], action="paste", text="Hi")
    assert "0.19.10" in str(out) and "type the text instead" in str(out)
    assert not [a for a in ext.acts if a.get("kind") == "paste"]


# decision: HAND-507
def test_the_google_docs_procedure_is_listed_and_reads_from_the_package() -> None:
    from ferb.abilities.catalog import AbilityCatalog
    catalog = AbilityCatalog()
    assert [a["id"] for a in catalog.catalog()] == ["browser_verification", "google_docs"]
    read = catalog.read("google_docs")
    assert read["source_kind"] == "builtin" and read["origin"] == "ferb" and read["body"].startswith("# Google Docs")


# decision: HAND-508
def test_a_hover_that_brings_up_something_under_the_pointer_went_through_and_says_so() -> None:
    """Live (task 2357c470): four hovers on Gmail rows came back as failures; each had shown the row's own actions."""
    from tests.test_busy_pages import _PAGE
    out = worker('(async () => {' + _PAGE + r'''
      await shoot(); let p = await point(50, 30);
      hover = () => { buy = {id: 12, role: "button", label: "Archive", href: null, rect: [0, 0, 120, 60]}; };
      const hovered = await commands.pointer({tab_id: 1, point: p.pointer, kind: "hover"}, "h1");
      buy = {...buy, id: 3, role: "button", label: "Buy", rect: [10, 10, 200, 40]};
      await shoot(); p = await point(50, 30);
      const clicked = await commands.pointer({tab_id: 1, point: p.pointer, kind: "click"}, "c1");
      return {hovered, clicked, presses: events.filter(e => e === "mousePressed").length};
    })()''')
    assert out["hovered"]["ok"] is True and "changed under the pointer" in out["hovered"]["observed"]
    assert out["clicked"]["ok"] is False and out["clicked"]["detail"].startswith("visual_target_changed")
    assert out["presses"] == 0                                     # a click onto what the hover brought up: refused


# decision: HAND-509
def test_a_date_field_is_set_by_its_value_a_button_takes_no_typing_and_a_mask_says_what_it_reads() -> None:
    """Lab: real typing into a native date or time input answered "typed (trusted)" and set nothing."""
    out = run('''
      await act({kind: "click"});
      rect = {x: 50, y: 60, editable: true, field: "input", picked: "date"};
      page.setPicked = (g, i, t) => ({ok: true, detail: "set the date field to " + t, field_value: t});
      const date = await act({kind: "type", text: "2026-12-25"});
      rect = {x: 50, y: 60, editable: false, field: null};
      page.focusedField = "";
      const button = await act({kind: "type", text: "on"});
      rect = {x: 50, y: 60, editable: true, field: "input"};
      page.focusedField = "input"; page.fieldValue = "+91 87654 3210";
      const phone = await act({kind: "type", text: "9876543210"});
      return {date, button, phone};
    ''')
    assert out["date"]["value"]["ok"] and out["date"]["keys"] == [] and out["date"]["texts"] == []
    assert "setPicked" in out["date"]["page"] and out["date"]["pressed"] == 0
    assert out["button"]["value"]["reason"] == "not_editable" and out["button"]["texts"] == []
    assert out["button"]["outcome"] == "failed"
    assert out["phone"]["value"]["field_value"] == "+91 87654 3210"
    assert out["phone"]["value"]["observed"].endswith('the field now reads "+91 87654 3210"')


# decision: HAND-510
def test_right_scrolls_sideways_screens_scroll_at_once_and_to_end_goes_there() -> None:
    """Lab: "right" scrolled the page down (deltaX was always 0), and row 500 of a virtual list took 58 calls."""
    out = run('''
      let pos = {x: 0, y: 0};
      page.scroller = {px: 100, py: 100, viewport_h: 120, viewport_w: 400, y: 0, height: 120, x: 0, width: 2000};
      page.scrolled = () => ({...pos, height: 3000, width: 2000, viewport_h: 300, viewport_w: 400, at_end: false, at_end_x: false});
      page.scrollEdge = (c, to, axis) => { pos = {...pos, [axis]: to === "end" ? 1600 : 0}; return pos; };
      const wheel = () => cmds.filter(([m, p]) => p.type === "mouseWheel").map(([m, p]) => [p.deltaX, p.deltaY]);
      const go = async (args) => { await onCommand({gen: 1, id: "s" + (++n), op: "scroll", args: {tab_id: 8, frame_id: 0, ...args}});
                                   return sent[sent.length - 1].value; };
      const right = await go({direction: "right", container: "c4"});
      const many = await go({direction: "down", container: "c1", screens: 10});
      const end = await go({direction: "right", container: "c4", to: "end"});
      return {right, many, end, wheel: wheel()};
    ''')
    assert out["wheel"] == [[340, 0], [0, 1020]]                  # 0.85 of the pane's width; ten screens of 120 px
    assert out["end"]["x"] == 1600                                # straight to the end, no wheel


# decision: HAND-510
@pytest.mark.parametrize("version", ["current", "0.19.9"])
def test_scrolls_sideways_by_screens_or_to_the_end_go_only_to_a_current_extension(  # noqa: F811
        bd: Any, monkeypatch: pytest.MonkeyPatch, version: str) -> None:
    if version != "current":
        monkeypatch.setattr("tests.browserkit.FakeExtension", lambda home: FakeExtension(home, version=version))
    d, launched = bd
    ext, g = _ready(d, launched)
    tab, _ = _form(d, g, "/chat")
    a = call(d, g, "browser.scroll", tab=tab, direction="down", screens=3)
    b = call(d, g, "browser.scroll", tab=tab, direction="right", to="end")
    sent = [json.loads(json.loads(m)["body"]) for m in ext.wire if '"type": "cmd"' in m]
    scrolls = [(c["args"].get("direction"), c["args"].get("screens"), c["args"].get("to"))
               for c in sent if c["op"] == "scroll"]
    if version == "current":
        assert scrolls == [("down", 3, None), ("right", None, "end")]
    else:
        assert scrolls == [] and "0.19.10" in str(a) and "up or down" in str(b)


# decision: HAND-511
def test_a_popup_window_the_tasks_click_opened_is_claimed_though_chrome_names_another_tab_as_its_opener() -> None:
    """Live (task c301938e): "Sign in with Google" opened a sized window; Chrome's openerTabId named the active tab of
    the owner's window, the task tab was a background one, so the window was never the task's."""
    from tests.test_extension_dialogs import SETUP as DIALOGS
    out = worker("(async () => {" + DIALOGS + '''
      targets[23] = {targetId: "T23", openerId: "T8"};             // the browser's own word: tab 8 opened it
      targets[24] = {targetId: "T24", openerId: "T99"};
      chrome.windows = {get: async (id) => ({id, type: id === 5 ? "popup" : "normal"})};
      chrome.tabs.get = async id => ({id, windowId: id === 23 || id === 24 ? 5 : 9, url, status: "complete", title: "S",
                                      openerTabId: 1});              // Chrome names the owner's active tab
      await arm(8);
      await (await command("act", {kind: "click", snap: 1, el: 1})).done;
      tabCreated({id: 23, pendingUrl: "https://accounts.example/signin"});
      tabCreated({id: 24, pendingUrl: "https://other.example/"});     // another tab's popup window: let go of
      await new Promise(r => setTimeout(r, 50));
      return {popups: events.filter(e => e.event === "popup"), attached};
    })()''')
    assert out["popups"] == [{"event": "popup", "tab_id": 23, "opener_tab_id": 8, "url": "https://accounts.example/signin"}]
    assert 24 in out["attached"] and -24 in out["attached"]


# decision: HAND-511
def test_a_tab_that_closed_itself_is_a_fact_for_the_worker_never_the_owner_stopping_the_task(bd: Any) -> None:  # noqa: F811
    """Live 2026-11-12 (task f68317e9): Drive's download tab closed by itself and FERB asked the owner "You closed a
    tab I was using"; earlier (task c301938e) a Google sign-in window that closed itself ended the task."""
    from tests.test_browser_groups import _chrome_id, _drv, _published
    d, launched = bd
    ext, g = _ready(d, launched)
    seen = _published(d)
    stops = lambda: [e["tab"] for t, e in seen if t == "browser.owner_stopped"]  # noqa: E731
    tab, ids = _form(d, g, "/product")
    news: list[dict[str, Any]] = []                                # what the worker's results said about closed tabs

    def popup() -> tuple[str, int]:
        out = call(d, g, "browser.act", tab=tab, element=ids["Seller details"], action="click")["content"]
        news.extend(out.get("closed_tabs") or [])
        alias = out["opened_tabs"][0]["tab"]
        return alias, _chrome_id(d, alias)

    def removed(alias: str, by_page: bool = False) -> None:
        cid = _chrome_id(d, alias)
        ext.tabs.pop(cid, None)
        ext.send("tab_event", {"event": "removed", "tab_id": cid, **({"closed_by_page": True} if by_page else {})})
        ext.wait_for(lambda: _drv(d)._tabs[alias].closed)

    a, _ = popup()
    removed(a, by_page=True)                                       # its page closed it (window.close)
    b, _ = popup()
    _drv(d).owner_input_near = lambda: None                        # the input tap can't say: a page's tab went itself
    removed(b)
    c, _ = popup()
    _drv(d).owner_input_near = lambda: True                        # the owner was using the keyboard or mouse
    removed(c)
    news.extend(call(d, g, "browser.snapshot", tab=tab)["content"].get("closed_tabs") or [])
    time.sleep(0.2)
    assert stops() == [c]                                          # only the close the owner could have made
    assert [x["tab"] for x in news] == [a, b] and "its page closed it" in news[0]["closed"]
    own = call(d, g, "browser.open", url=f"{SHOP}/a")["content"]["tab"]
    _drv(d).owner_input_near = lambda: None
    removed(own)                                                   # FERB's own tab, closed with no page's word
    time.sleep(0.2)
    assert stops() == [c, own]                                     # HAND-201 as before


# decision: HAND-511, HAND-512
def test_the_page_hook_answers_print_while_ferb_drives_the_tab_and_tells_of_a_window_closing_itself() -> None:
    """Lab: window.print() in a task tab opened the print preview; the act waited 30 s, came back unknown, and the tab
    stopped answering reads."""
    import subprocess
    from tests.chromelab import EXT, _node
    script = f'''
      const vm = require("vm"), p = require({json.dumps(str(EXT / "protocol.js"))});
      class HTMLElement {{ click() {{}} }}
      const win = new EventTarget(), doc = new EventTarget(), told = [];
      let printed = 0, closed = 0, armed = true;
      win.open = () => null; win.print = () => {{ printed++; }}; win.close = () => {{ closed++; }};
      doc.baseURI = "https://shop.example/";
      doc.addEventListener(p.FERB_TOKEN + ":armed", (e) => {{ if (armed) e.preventDefault(); }});
      for (const k of ["print", "closing"]) doc.addEventListener(p.FERB_TOKEN + ":" + k, () => told.push(k));
      const ctx = vm.createContext({{window: win, document: doc, HTMLElement, HTMLAnchorElement: class extends HTMLElement {{}},
        HTMLAreaElement: class extends HTMLElement {{}}, EventTarget, CustomEvent, Event, URL, crypto, Date, setTimeout}});
      vm.runInContext(p.PAGE_HOOK, ctx);
      const returned = win.print() === undefined;
      const whileDriven = printed;
      armed = false; win.print();
      win.close();
      console.log(JSON.stringify({{returned, whileDriven, printed, closed, told}}));
    '''
    ran = subprocess.run([_node(), "-e", script], capture_output=True, text=True, timeout=20)
    assert ran.returncode == 0, ran.stderr
    assert json.loads(ran.stdout) == {"returned": True, "whileDriven": 0, "printed": 1, "closed": 1,
                                      "told": ["print", "closing"]}


# decision: HAND-513
def test_a_snapshot_of_a_pdf_in_the_browsers_viewer_says_so_first_with_the_way_on() -> None:
    """Lab: a PDF in the viewer gave a snapshot with no elements and no text, and nothing said why."""
    from ferb.act.browser.snapshot import for_model
    from ferb.core.types import PageSnapshot, TabRef
    tab = TabRef("dev", "chrome", "Default", 1, "t1")
    pdf = for_model(PageSnapshot(tab, 1, "https://shop.example/invoice.pdf", "invoice.pdf", content_type="application/pdf"))
    page = for_model(PageSnapshot(tab, 1, "https://shop.example/", "Shop"))
    assert list(pdf)[3] == "pdf" and pdf["pdf"]["next"].startswith("Look at it with browser_screenshot")
    assert "pdf" not in page


# decision: HAND-514
def test_a_click_holds_the_keys_asked_for_as_real_input() -> None:
    """Lab: a list's shift-click range (Gmail, Drive) couldn't be made: the act had no held keys."""
    out = run('''
      await act({kind: "click"});
      const from = cmds.length;
      await act({kind: "click", modifiers: 8});
      await act({kind: "click", modifiers: 99});                  // out of range: no held key
      return cmds.slice(from).filter(([m, p]) => p.type === "mousePressed").map(([m, p]) => p.modifiers);
    ''')
    assert out == [8, 0]


# decision: HAND-514
def test_the_daemon_sends_held_keys_only_with_a_real_click(bd: Any) -> None:  # noqa: F811
    d, launched = bd
    ext, g = _ready(d, launched)
    tab, ids = _form(d, g)
    call(d, g, "browser.act", tab=tab, element=ids["Submit"], action="click", modifiers=["shift", "cmd"])
    assert ext.acts[-1]["modifiers"] == 12
    bad = call(d, g, "browser.act", tab=tab, element=ids["Name"], action="type", text="x", modifiers=["shift"])
    assert bad["is_error"] and ext.acts[-1].get("kind") == "click"     # refused before anything was sent


# decision: HAND-515, HAND-516
def test_secret_fields_are_known_by_their_structure_never_their_words() -> None:
    """Owner 2026-11-12: a delivery "pin code" or a voucher's "security code" is an ordinary field (live: Flipkart, task
    97e27b93, FERB refused to type a pincode). The structural ones stay: a password field that an eye made text, a
    dotted input, autocomplete tokens in any case and order."""
    from tests.test_shown_password_fields import extension
    out = extension(r'''
globalThis.getComputedStyle = (el) => ({webkitTextSecurity: el.dots ? 'disc' : 'none'});
const kind = (attrs, text) => { const e = describe(input(attrs, text), 1); return e.field_kind || null; };
const eye = input({type: 'password'}, 'Password');
const before = describe(eye, 1).field_kind;
eye.type = 'text';                                          // the "show password" eye
console.log(JSON.stringify({before, after: describe(eye, 1).field_kind,
  pin: kind({inputMode: 'numeric', maxLength: 6, name: 'pincode'}, 'Delivery pin code'),
  voucher: kind({name: 'security_code'}, 'Coupon security code'), otpWords: kind({name: 'otp'}, 'Enter OTP'),
  pinPassword: kind({type: 'password'}, 'PIN'), dots: kind({dots: true}, 'Account number'),
  upper: kind({autocomplete: 'Shipping CC-Number'}, ''), month: kind({autocomplete: 'section-pay billing cc-exp-month'}, ''),
  code: kind({autocomplete: 'ONE-TIME-CODE'}, '')}));
''')
    assert out == {"before": "password", "after": "password", "pin": None, "voucher": None, "otpWords": None,
                   "pinPassword": "password", "dots": "password", "upper": "card_number", "month": "card_expiry",
                   "code": "one_time_code"}
