"""Run the real extension worker with deterministic Chrome APIs; no owner's browser is touched."""
import json
import subprocess

from tests.test_browser_background import EXT, _node


def worker(script: str) -> dict:
    harness = r'''
const vm = require("vm"), fs = require("fs"), path = require("path");
const noop = new Proxy(function(){}, {get: () => noop});
const state = {s: {seen: {}, order: [], owned: [], windowId: 9}};
const chrome = new Proxy({
  storage: {local: {get: async () => ({ferb: {key: null}})},
            session: {get: async () => structuredClone(state), set: async x => Object.assign(state, x)}},
  runtime: new Proxy({getManifest: () => ({version: "test"})}, {get: (o,k) => o[k] || noop}),
  tabs: new Proxy({get: async () => ({windowId: 9})}, {get: (o,k) => o[k] || noop}),
}, {get: (o,k) => o[k] || noop});
const ctx = vm.createContext({chrome, crypto: globalThis.crypto, btoa, atob, TextEncoder, URL,
  setTimeout, clearTimeout, console, navigator: {}, structuredClone});
ctx.importScripts = f => vm.runInContext(fs.readFileSync(path.join(process.argv[1], f), "utf8"), ctx);
ctx.importScripts("background.js");
vm.runInContext(process.argv[2], ctx).then(x => { console.log(JSON.stringify(x)); process.exit(0); })
 .catch(e => { console.error(e); process.exit(1); });
'''
    result = subprocess.run([_node(), "-e", harness, str(EXT), script], capture_output=True, text=True, timeout=20)
    assert result.returncode == 0, result.stderr
    return json.loads(result.stdout)


def test_trusted_action_never_silently_becomes_page_events() -> None:
    result = worker('''(async () => {
      attach = async () => { throw new Error("debugger unavailable"); };
      inFrame = async () => ({ok: true, detail: "synthetic click"});
      try { return await commands.act({tab_id: 1, frame_id: 0, snap: 1, el: 0, kind: "click"}); }
      catch(e) { return {ok: false, detail: e.message}; }
    })()''')
    assert result["ok"] is False and "debugger unavailable" in result["detail"]


def test_covered_target_is_not_clicked() -> None:
    result = worker('''(async () => {
      attach = async () => tabState(1);
      topPoint = async () => ({x: 10, y: 20, occluded: true});
      let clicks = 0; pointerClick = async () => { clicks++; };
      const r = await trusted(1, {frame_id: 0, snap: 1, el: 0, kind: "click"});
      return {...r, clicks};
    })()''')
    assert result["ok"] is False and result["clicks"] == 0


def test_keypress_never_goes_to_a_different_focused_control() -> None:
    result = worker('''(async () => {
      attach = async () => tabState(1); topPoint = async () => ({x: 10, y: 20});
      inFrame = async () => false;
      let keys = 0; pressKey = async () => { keys++; return true; };
      const r = await trusted(1, {frame_id: 0, snap: 1, el: 0, kind: "press"});
      return {...r, keys};
    })()''')
    assert result["ok"] is False and result["keys"] == 0


def test_failed_and_timed_out_commands_are_not_reconciled_as_done() -> None:
    result = worker('''(async () => {
      gen = 1; send = async () => {};
      commands.act = async () => ({ok: false, detail: "no effect"});
      await onCommand({gen: 1, id: "failed", op: "act"});
      commands.act = async () => { throw new Error("timeout"); };
      await onCommand({gen: 1, id: "unknown", op: "act"});
      return (await session()).seen;
    })()''')
    assert result == {"failed": "failed", "unknown": "unknown"}


def test_ax_is_primary_on_a_busy_page_and_preserves_dom_secret_metadata() -> None:
    result = worker('''(async () => {
      inject = async () => {}; attach = async () => tabState(1);
      const elements = Array.from({length: 5}, (_, id) => ({id, role: "textbox", label: "DOM",
        binding: "000000000000000" + id, secret: id === 0, value: id === 0 ? "filled" : "", rect: [0,0,20,20]}));
      chrome.scripting = {executeScript: async () => [{frameId: 0, result: {elements, text: "page", iframes: []}}]};
      cdp = async (tab, method, args) => {
        if (method === "Page.getFrameTree") return {frameTree: {frame: {id: "root"}}};
        if (method === "Accessibility.getFullAXTree") return {nodes: [{role: {value: "textbox"}, name: {value: "Verification code"},
          value: {value: "123456"}, backendDOMNodeId: 7}]};
        if (method === "DOM.describeNode") return {node: {attributes: ["data-ferb-ax", elements[0].binding]}};
        return {};
      };
      const r = await commands.snapshot({tab_id: 1, snap: 1});
      return {frame: r.frames[0], leaked: JSON.stringify(r).includes("123456")};
    })()''')
    assert result["frame"]["elements"][0]["label"] == "Verification code"
    assert result["frame"]["elements"][0]["secret"] is True
    assert result["leaked"] is False
    assert len(result["frame"]["elements"]) == 5


def test_frame_offsets_use_identity_not_duplicate_urls_or_names() -> None:
    result = worker('''(async () => {
      const frames = [
        {frame_id: 0, elements: [], iframes: [
          {binding: "a", src: "same", rect: [10,20,100,100], in_view: true},
          {binding: "b", src: "same", rect: [300,400,100,100], in_view: true}]},
        {frame_id: 1, url: "same", parent_frame_id: 0, owner_binding: "b", elements: [{rect: [1,2,3,4], in_view: true}]},
        {frame_id: 2, url: "redirected", parent_frame_id: 0, owner_binding: "a", elements: []}];
      return composeFrames(frames);
    })()''')
    assert result == {"0": [0, 0], "1": [300, 400], "2": [10, 20]}
