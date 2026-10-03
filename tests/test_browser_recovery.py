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


def test_refusal_does_not_permanently_disable_reconnection() -> None:
    result = worker('''(async () => {
      let connected = 0;
      chrome.runtime.connectNative = () => { connected++; return {
        onMessage: {addListener() {}}, onDisconnect: {addListener() {}}, postMessage() {}, disconnect() {}}; };
      port = null; refused = "old protocol";
      await ensurePort();
      return {connected};
    })()''')
    assert result['connected'] == 1


def test_local_extension_update_waits_for_command_and_preserves_pairing() -> None:
    result = worker('''(async () => {
      const local = {ferb: {installId: "existing", key: "keep", seq: 42}};
      chrome.storage.local = {get: async () => structuredClone(local), set: async x => Object.assign(local, x)};
      chrome.runtime.getManifest = () => ({version: "0.10.0"});
      chrome.runtime.getURL = p => "chrome-extension://test/" + p;
      let reloads = 0; chrome.runtime.reload = () => {reloads++;};
      globalThis.fetch = async (url, opts) => {
        if (!url.endsWith('/manifest.json') || opts.cache !== 'no-store') throw Error('wrong source');
        return {ok: true, json: async () => ({version: "0.10.1"})};
      };
      activeCommands = 1;
      await checkExtensionUpdate(); const busyReloads = reloads;
      activeCommands = 0;
      await checkExtensionUpdate(); await checkExtensionUpdate();
      return {busyReloads, reloads, pairing: local.ferb};
    })()''')
    assert result == {'busyReloads': 0, 'reloads': 1,
                      'pairing': {'installId': 'existing', 'key': 'keep', 'seq': 42}}


def test_update_does_not_reload_for_same_or_older_bundle() -> None:
    result = worker('''(async () => {
      chrome.runtime.getManifest = () => ({version: "0.10.0"});
      chrome.runtime.getURL = p => "chrome-extension://test/" + p;
      let reloads = 0; chrome.runtime.reload = () => {reloads++;};
      for (const version of ["0.10.0", "0.9.9"]) {
        globalThis.fetch = async () => ({ok: true, json: async () => ({version})});
        await checkExtensionUpdate();
      }
      return {reloads};
    })()''')
    assert result == {'reloads': 0}


def test_update_waits_between_commands_while_task_tab_is_still_owned() -> None:
    result = worker('''(async () => {
      chrome.runtime.getManifest = () => ({version: "0.10.0"});
      chrome.runtime.getURL = p => "chrome-extension://test/" + p;
      let reloads = 0; chrome.runtime.reload = () => {reloads++;};
      globalThis.fetch = async () => ({ok: true, json: async () => ({version: "0.10.1"})});
      ownedTabs = async () => [{id: 'task', tab_id: 7}];
      await checkExtensionUpdate();
      return {reloads};
    })()''')
    assert result == {'reloads': 0}


def test_update_rechecks_command_started_during_async_storage_write() -> None:
    result = worker('''(async () => {
      const local = {};
      chrome.storage.local = {get: async () => structuredClone(local), set: async x => {
        Object.assign(local, x); if(x.ferbUpdateAttempt) activeCommands = 1;
      }};
      chrome.runtime.getManifest = () => ({version: "0.10.0"});
      chrome.runtime.getURL = p => "chrome-extension://test/" + p;
      let reloads = 0; chrome.runtime.reload = () => {reloads++;};
      globalThis.fetch = async () => ({ok: true, json: async () => ({version: "0.10.1"})});
      await checkExtensionUpdate();
      return {reloads, attempt: local.ferbUpdateAttempt};
    })()''')
    assert result == {'reloads': 0, 'attempt': None}
