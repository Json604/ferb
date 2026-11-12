"""HAND-504..513 (extension 0.19.10), the extension's own code under node with fake Chrome APIs, and the daemon's side:
the input gaps the lab (tests/test_browser_input_gaps_real.py) found. Live 2026-11-11, task 9e7c14ed: 8 minutes failing to
put text into a Google Doc, every type and press refused "target_occluded" while the document had the focus."""

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
