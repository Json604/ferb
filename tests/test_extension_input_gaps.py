"""HAND-504..513 (extension 0.19.10), the extension's own code under node with fake Chrome APIs, and the daemon's side:
the input gaps the lab (tests/test_browser_input_gaps_real.py) found. Live 2026-11-11, task 9e7c14ed: 8 minutes failing to
put text into a Google Doc, every type and press refused "target_occluded" while the document had the focus."""

from typing import Any

from tests.chromelab import worker

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
