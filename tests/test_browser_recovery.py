"""Run the real extension worker with deterministic Chrome APIs; no owner's browser is touched."""

import json
import subprocess
import pytest
from tests.chromelab import worker



def test_trusted_action_never_silently_becomes_page_events() -> None:
    result = worker('''(async () => {
      attach = async () => { throw new Error("debugger unavailable"); };
      inFrame = async () => ({ok: true, detail: "synthetic click"});
      try { return await commands.act({tab_id: 1, frame_id: 0, snap: 1, el: 0, kind: "click"}); }
      catch(e) { return {ok: false, detail: e.message}; }
    })()''')
    assert result["ok"] is False and "debugger unavailable" in result["detail"]


# The owner's US-visa login tab (2026-10-24): another extension (most likely "Check US Visa Slots") has a frame
# in the page. Chrome's two rules, as in its source: chrome.debugger.attach({tabId}) checks every frame in the tab and
# refuses the whole tab for one frame of another extension (ExtensionMayAttachToWebContents ->
# ExtensionMayAttachToURL -> kCannotAccessExtensionUrl); chrome.scripting.executeScript skips a frame it can't script
# among several and fails only when that frame is the only one targeted (ScriptingExecuteScriptFunction).
_OTHER_EXTENSION_FRAME = r'''
  const OWN = "jaclnphefmmgmpofnabffahdkbjjompm", OTHER = "beepaenfejnphdgnkmccjcfiieihhogl";
  const REFUSED = "Cannot access a chrome-extension:// URL of different extension";
  chrome.runtime.id = OWN;
  const frames = {0: "https://atlasauth.b2clogin.com/t/b2c_1a_atoproduction_atlas_susi/oauth2/v2.0/authorize",
                  4: "chrome-extension://" + OTHER + "/sidepanel.html"};
  const chromeWontTouch = url => url.startsWith("chrome-extension://") && new URL(url).host !== OWN;
  const named = [], ranIn = [], typed = [], sent = [];
  chrome.debugger.attach = async () => { if (Object.values(frames).some(chromeWontTouch)) throw new Error(REFUSED); };
  chrome.debugger.sendCommand = async () => { throw new Error("Debugger is not attached to the tab with id: 5."); };
  const pageWindow = {__ferb: {
    snapshot: g => ({url: frames[0], title: "Sign in", text: "Sign in", iframes: [], scrollers: [],
                     elements: [{id: 0, role: "textbox", label: "Email Address", rect: [0, 0, 200, 20], in_view: true}]}),
    act: (g, i, k, t) => { typed.push([g, i, k, t]); return {ok: true, detail: "typed (page events)"}; },
  }};
  chrome.scripting = {executeScript: async ({target, func, args}) => {
    if (target.frameIds) named.push(...target.frameIds);
    const ids = target.allFrames ? Object.keys(frames).map(Number) : target.frameIds;
    const results = [];
    for (const id of ids) {
      if (chromeWontTouch(frames[id])) { if (ids.length === 1) throw new Error(REFUSED); continue; }
      ranIn.push(id);
      globalThis.window = id === 0 ? pageWindow : {};
      results.push({frameId: id, result: func ? func(...(args || [])) : undefined});
    }
    return results;
  }};
  gen = 1; send = async (type, body) => { if (type === "result") sent.push(body); };
  const act = async (id, extra) => { await onCommand({gen: 1, id, op: "act", args: {tab_id: 5, frame_id: 0, snap: 1,
    el: 0, kind: "type", text: "owner@example.com", ...extra}}); return sent.find(b => b.id === id); };
'''


def test_another_extensions_frame_never_fails_input_on_the_pages_own_frames() -> None:
    """Real input needs FERB's debugger, which Chrome won't attach to such a tab: the act says so, as a known failure
    with nothing done (never "outcome unknown", which holds the retry for the owner), and never turns into page events
    by itself. Page events, asked for, reach the page's own field through scripting while the other extension's
    frame is never named or scripted."""
    result = worker('''(async () => {''' + _OTHER_EXTENSION_FRAME + '''
      const snap = await commands.snapshot({tab_id: 5, snap: 1});
      const trusted = await act("real");
      const typedByRealInput = typed.length;
      const page = await act("page", {page_mode: true});
      return {snap, trusted, typedByRealInput, page, typed, named, ranIn};
    })()''')
    assert [f["frame_id"] for f in result["snap"]["frames"]] == [0]      # the other extension's frame isn't the page's
    assert "different extension" in result["snap"]["frames"][0]["text"]
    trusted = result["trusted"]
    assert trusted["status"] == "ok" and trusted["outcome"] == "failed", trusted
    assert trusted["value"]["ok"] is False and trusted["value"]["reason"] == "debugger_refused"
    assert "different extension" in trusted["value"]["detail"] and "page_mode" in trusted["value"]["detail"]
    assert result["typedByRealInput"] == 0                               # never silently page events
    page = result["page"]
    assert page["status"] == "ok" and page["outcome"] == "done", page
    assert page["value"]["ok"] is True and page["value"]["detail"] == "typed (page events)"
    assert "different extension" in page["value"]["debugger_refused"]   # says what isn't watched in this tab
    assert result["typed"] == [[1, 0, "type", "owner@example.com"]]      # exactly the snapshot's field, once
    assert 4 not in result["named"] and 4 not in result["ranIn"]


def test_a_debugger_failure_after_it_attached_is_still_unknown_and_never_page_events() -> None:
    """Only the browser's own refusal of chrome.debugger.attach (nothing attached, nothing sent) is a known failure;
    a failure once the debugger is attached keeps failing the act as before, page events included."""
    result = worker('''(async () => {''' + _OTHER_EXTENSION_FRAME + '''
      delete frames[4];                                                  // the debugger attaches...
      chrome.debugger.detach = async () => {};
      chrome.debugger.sendCommand = async () => { throw new Error("tab never became ready for input"); };
      const page = await act("page", {page_mode: true});
      return {page, typed};
    })()''')
    assert result["page"]["status"] == "error" and result["page"]["outcome"] == "unknown", result["page"]
    assert result["typed"] == []


def test_confirmed_tab_close_does_not_detach_an_already_destroyed_target() -> None:
    result = worker('''(async () => {
      let alive = true;
      const t = tabState(1); t.attached = true;
      chrome.tabs.remove = async () => {alive = false;};
      chrome.debugger = {sendCommand: async () => {if (!alive) throw Error('No tab with id: 1');}};
      try {return await commands.close({tab_id:1});}
      catch (e) {return {error:e.message};}
    })()''')
    assert result.get("closed") is True, result


def test_container_wheel_requires_an_uncovered_point_in_the_requested_scroller() -> None:
    from tests.chromelab import EXT, _node
    script = r'''
const vm = require('vm'), fs = require('fs');
let covered = true, offscreen = false;
const body = {scrollHeight:2000,scrollTop:0,clientHeight:600};
const sc = {isConnected:true,scrollHeight:1000,clientHeight:100,scrollTop:0,parentElement:body,
  contains: el => el === sc, getRootNode: () => doc,
  getBoundingClientRect: () => ({x:10,y:offscreen ? 900 : 10,width:100,height:100})};
const doc = {body,documentElement:body,scrollingElement:body,elementFromPoint: () => covered ? body : sc};
const ctx = vm.createContext({document:doc,window:{},crypto:globalThis.crypto,WeakRef,
  innerWidth:800,innerHeight:600,addEventListener(){},MutationObserver:class {observe(){}},
  ShadowRoot:class {},getComputedStyle:el => ({overflowY:el === sc ? 'auto' : 'visible'}), sc});
const source = fs.readFileSync(process.argv[1], 'utf8').replace('let containers = new Map();',
  "let containers = new Map([['c1', new WeakRef(sc)]]);");
vm.runInContext(source,ctx);
const call = cid => ctx.window.__ferb.scroller(cid);
const blocked = call('c1'); covered = false;
const matched = call('c1'), wrongPage = call(null); offscreen = true;
console.log(JSON.stringify({blocked,matched,wrongPage,offscreen:call('c1')}));
'''
    result = subprocess.run([_node(), "-e", script, str(EXT / "content.js")], capture_output=True, text=True, timeout=10)
    assert result.returncode == 0, result.stderr
    out = json.loads(result.stdout)
    assert "error" in out["blocked"] and "error" in out["offscreen"]
    assert "error" in out["wrongPage"]
    assert out["matched"]["px"] == 60 and out["matched"]["py"] == 60


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


@pytest.mark.parametrize('required, minimum, disk, expected', [
    ('FERB_PROTOCOL + 1', '0.11.1', '0.11.1', 1),
    ('FERB_PROTOCOL', '0.11.1', '0.11.1', 0),
    ('FERB_PROTOCOL - 1', '0.11.1', '0.11.1', 0),
    ('FERB_PROTOCOL + 1', '0.11.2', '0.11.1', 0),
    ('FERB_PROTOCOL + 1', '0.11.0', '0.11.0', 0),
    ('FERB_PROTOCOL + 1', '0.11.1', '0.11.1.bad', 0),
    ('null', '0.11.1', '0.11.1', 0),
])
def test_protocol_refusal_reloads_eligible_same_version_bundle_once(required, minimum, disk, expected):
    result = worker('''(async () => {
      const local = {ferb: {installId: 'existing', key: 'keep', seq: 42}, ferbKeptTabs: [19]};
      chrome.storage.local = {get: async () => structuredClone(local), set: async x => Object.assign(local, x)};
      chrome.runtime.getManifest = () => ({version: '0.11.1'});
      chrome.runtime.getURL = p => 'chrome-extension://test/' + p;
      let reloads = 0; chrome.runtime.reload = () => {reloads++;};
      globalThis.fetch = async (url, opts) => {
        if (!url.endsWith('/manifest.json') || opts.cache !== 'no-store') throw Error('wrong source');
        return {ok: true, json: async () => ({version: DISK})};
      };
      const refusal = {type:'refused', reason:'protocol mismatch', required_protocol: REQUIRED, minimum_version: MINIMUM};
      await onDaemon(refusal); await onDaemon(refusal); await checkExtensionUpdate();
      return {reloads, pairing:local.ferb, kept:local.ferbKeptTabs, local};
    })()'''.replace('REQUIRED', required).replace('MINIMUM', json.dumps(minimum)).replace('DISK', json.dumps(disk)))
    assert result['reloads'] == expected
    assert result['pairing'] == {'installId': 'existing', 'key': 'keep', 'seq': 42}
    assert result['kept'] == [19]
    # The guard is durable: restarting the worker with unchanged code must not create a reload loop.
    again = worker('''(async () => {
      const local = LOCAL;
      chrome.storage.local = {get: async () => structuredClone(local), set: async x => Object.assign(local, x)};
      chrome.runtime.getManifest = () => ({version:'0.11.1'});
      chrome.runtime.getURL = p => 'chrome-extension://test/' + p;
      let reloads = 0; chrome.runtime.reload = () => {reloads++;};
      globalThis.fetch = async () => ({ok:true, json:async () => ({version:DISK})});
      await onDaemon({type:'refused', reason:'still mismatched', required_protocol:REQUIRED, minimum_version:MINIMUM});
      return {reloads};
    })()'''.replace('LOCAL', json.dumps(result['local'])).replace('REQUIRED', required)
                   .replace('MINIMUM', json.dumps(minimum)).replace('DISK', json.dumps(disk)))
    assert again['reloads'] == 0


@pytest.mark.parametrize('busy', ['command', 'owned_tab', 'attached_tab', 'gesture', 'storage_race'])
def test_protocol_repair_waits_for_resources_then_retries_when_idle(busy):
    result = worker('''(async () => {
      const local = {}; let busy = true;
      chrome.storage.local = {get: async () => structuredClone(local), set: async x => {
        Object.assign(local, x);
        if (BUSY === 'storage_race' && busy && x.ferbUpdateAttempt) activeCommands = 1;
      }};
      chrome.runtime.getManifest = () => ({version:'0.11.1'});
      chrome.runtime.getURL = p => 'chrome-extension://test/' + p;
      let reloads = 0; chrome.runtime.reload = () => {reloads++;};
      globalThis.fetch = async () => ({ok:true, json:async () => ({version:'0.11.1'})});
      if (BUSY === 'command') activeCommands = 1;
      if (BUSY === 'owned_tab') ownedTabs = async () => busy ? [{tab_id:7}] : [];
      if (BUSY === 'attached_tab') tabState(7).attached = true;
      if (BUSY === 'gesture') tabState(7).gesture = {id:'drag'};
      await onDaemon({type:'refused', reason:'protocol mismatch', required_protocol:FERB_PROTOCOL + 1, minimum_version:'0.11.1'});
      const during = reloads;
      busy = false; activeCommands = 0; TABS.clear();
      await checkExtensionUpdate();
      return {during, after:reloads};
    })()'''.replace('BUSY', json.dumps(busy)))
    assert result == {'during': 0, 'after': 1}


def test_successful_handshake_clears_protocol_repair_request():
    result = worker('''(async () => {
      activeCommands = 1;
      await onDaemon({type:'refused', reason:'protocol mismatch', required_protocol:FERB_PROTOCOL + 1, minimum_version:'0.11.1'});
      await onDaemon({type:'welcome', gen:2});
      chrome.runtime.getManifest = () => ({version:'0.11.1'});
      chrome.runtime.getURL = p => 'chrome-extension://test/' + p;
      let reloads = 0; chrome.runtime.reload = () => {reloads++;};
      globalThis.fetch = async () => ({ok:true, json:async () => ({version:'0.11.1'})});
      activeCommands = 0;
      await checkExtensionUpdate();
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


def test_concurrent_commands_claim_ids_once_and_keep_all_recovery_records() -> None:
    result = worker('''(async () => {
      gen = 1; send = async () => {}; let ran = 0;
      commands.ping = async () => {ran++; await sleep(5); return {ok: true};};
      await Promise.all(['a', 'b', 'a'].map(id => onCommand({gen: 1, id, op: 'ping'})));
      return {ran, state: await session()};
    })()''')
    assert result['ran'] == 2
    assert result['state']['order'] == ['a', 'b']
    assert result['state']['seen'] == {'a': 'done', 'b': 'done'}


def test_concurrent_opens_share_one_window_without_losing_owned_tabs() -> None:
    result = worker('''(async () => {
      await saveSession({seen: {}, order: [], owned: [], windowId: null});
      let windows = 0, nextTab = 20;
      chrome.windows = {create: async () => {windows++; await sleep(5); return {id: 9};},
        get: async () => ({id: 9, state: 'minimized'})};
      chrome.tabs.create = async () => ({id: ++nextTab, windowId: 9});
      chrome.tabs.get = async id => ({id, windowId: 9, url: 'https://example.test/'});
      chrome.tabs.update = async () => {};
      ferbGroup = async () => {}; waitComplete = async () => {};
      await Promise.all([commands.open({url:'https://example.test/',allow_foreground:true}, 'a'), commands.open({url:'https://example.test/',allow_foreground:true}, 'b')]);
      return {windows, state: await session()};
    })()''')
    assert result['windows'] == 1
    assert {x['id'] for x in result['state']['owned']} == {'a', 'b'}


def test_container_scroll_failure_never_scrolls_a_different_surface() -> None:
    result = worker('''(async () => {
      attach = async () => {throw Error('debugger unavailable');};
      let scripted = 0; chrome.scripting = {executeScript: async () => {scripted++; return [{result: {y:700}}];}};
      try {await commands.scroll({tab_id:1, frame_id:0, container:'c1', direction:'down'});}
      catch(e) {return {error:e.message, scripted};}
      return {scripted};
    })()''')
    assert result == {'error': 'debugger unavailable', 'scripted': 0}


def test_scrolling_iframe_container_uses_its_frame_and_current_top_coordinates() -> None:
    result = worker('''(async () => {
      attach = async () => tabState(1);
      tabState(1).frameRefs = new Map([[7, {parent_frame_id:0, owner_binding:'frame-seven'}]]);
      const frames = [], events = [];
      inFrame = async (tab, frame, fn, args) => {
        frames.push(frame);
        if (frame === 0) return {x:300, y:400, occluded:false};
        return {px:20, py:30, viewport_h:100, y:85, height:1000, at_end:false};
      };
      cdp = async (tab, method, args) => {events.push(args);};
      const result = await commands.scroll({tab_id:1, frame_id:7, container:'c1', direction:'down'});
      return {frames, events, result};
    })()''')
    assert result['events'][0]['x'] == 320 and result['events'][0]['y'] == 430
    assert result['result']['frame_id'] == 7 and result['result']['container'] == 'c1'
    assert result['frames'].count(7) >= 3


def test_release_keeps_tab_open_detaches_and_survives_extension_session_reset() -> None:
    result = worker('''(async () => {
      const local = {};
      chrome.storage.local = {get: async () => structuredClone(local), set: async x => Object.assign(local,x)};
      await saveSession({seen:{}, order:[], owned:[{id:'opened',tabId:22}], windowId:9});
      const calls = []; chrome.debugger = {
        sendCommand: async (target, method) => {calls.push(method);},
        detach: async target => {calls.push('detach');}
      };
      chrome.tabs.get = async id => ({id, windowId:9, url:'https://class.example/', active:true});
      chrome.tabs.query = async () => [await chrome.tabs.get(22)];
      chrome.tabs.remove = async () => {throw Error('must never close kept tab');};
      tabState(22).attached = true;
      const released = await commands.release_tab({tab_id:22, keep_open:true});
      const owned = (await session()).owned;
      await saveSession({seen:{}, order:[], owned:[], windowId:null});
      TABS.clear();
      const listed = await commands.list_tabs();
      const closed = await commands.close({tab_id:22});
      return {released, calls, owned, listed, closed};
    })()''')
    assert result['released']['kept_open'] and result['released']['released']
    assert result['calls'] == ['Emulation.setFocusEmulationEnabled', 'detach']
    assert result['owned'] == []
    assert result['listed']['tabs'][0]['kept_open'] is True
    assert result['closed']['closed'] is False


def test_release_failure_keeps_debugger_tracked_and_does_not_claim_success() -> None:
    result = worker('''(async () => {
      tabState(22).attached = true;
      chrome.tabs.get = async id => ({id});
      chrome.debugger = {sendCommand: async () => {}, detach: async () => {throw Error('transport failed');}};
      try {await commands.release_tab({tab_id:22, keep_open:false});}
      catch(e) {return {error:e.message, tracked:TABS.has(22)};}
      return {tracked:TABS.has(22)};
    })()''')
    assert result == {'error': 'transport failed', 'tracked': True}
