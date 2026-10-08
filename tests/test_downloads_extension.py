"""D35, the extension's own side under node with fake Chrome APIs (no browser): which responses are downloads, the
held transfer (taken in parts, aborted, handed back to the browser), arming only tabs FERB drives, letting go,
navigation through a blank page, new tabs, the browser's own downloads, and content.js's link listener.
The browser-level behaviour was measured in the headless lab (task 10 report)."""

import base64
import json
import subprocess

import pytest

from tests.chromelab import EXT, _node, worker

PAUSED = '''
const paused = (id, url = 'https://docs.example/x?token=abc') => ({requestId: id,
  request: {url, headers: {'Upgrade-Insecure-Requests': '1'}}, responseStatusCode: 200,
  responseHeaders: [{name: 'Content-Disposition', value: 'attachment; filename="r\\u00e9sum\\u00e9.txt"'},
                    {name: 'Content-Type', value: 'text/plain'}, {name: 'Content-Length', value: '12'}]});
'''


def test_the_extension_holds_a_download_only_in_a_tab_ferb_drives_and_lets_it_go_after_the_last_part() -> None:
    out = worker('''(async () => {''' + PAUSED + '''
      const sent = [], cmds = []; let reads = 0;
      port = {postMessage() {}}; send = async (type, body) => { sent.push([type, body]); };
      cdp = async (tab, method, params, sid) => { cmds.push([method, params, sid || null]);
        if (method === 'Fetch.takeResponseBodyAsStream') return {stream: 'stream-1'};
        if (method === 'IO.read') { reads++; return reads === 1 ? {data: btoa('hello '), base64Encoded: true, eof: false}
                                                                : {data: 'w\\u00f6rld', base64Encoded: false, eof: true}; }
        return {}; };
      await onPaused({tabId: 5}, paused('owner-tab'));               // a tab FERB doesn't drive: untouched
      tabState(5).armed = true;
      await onPaused({tabId: 5, sessionId: 'frame-s'}, paused('task-tab'));
      const ev = sent.map(s => s[1]).find(b => b.event === 'download');
      const p1 = await commands.download_part({download_id: ev.download_id, offset: 0});
      const p2 = await commands.download_part({download_id: ev.download_id, offset: p1.next_offset});
      let again = null;
      try { await commands.download_part({download_id: ev.download_id, offset: 0}); } catch (e) { again = e.message; }
      return {cmds, ev, p1, p2, again, left: downloads.size};
    })()''')
    cmds = out["cmds"]
    assert cmds[0] == ["Fetch.continueRequest", {"requestId": "owner-tab"}, None]
    assert out["ev"]["tab_id"] == 5 and out["ev"]["name"] == "résumé.txt" and out["ev"]["size"] == 12
    assert out["ev"]["source"] == "attachment" and "abc" not in out["ev"]["source_url"]
    assert base64.b64decode(out["p1"]["data"]) == b"hello " and out["p1"]["eof"] is False
    assert base64.b64decode(out["p2"]["data"]) == "wörld".encode() and out["p2"]["eof"] is True
    assert out["p2"]["offset"] == 6 and out["p2"]["next_offset"] is None
    after = [c[0] for c in cmds[1:]]
    assert after == ["Fetch.takeResponseBodyAsStream", "IO.read", "IO.read", "IO.close", "Fetch.failRequest"]
    assert all(c[2] == "frame-s" for c in cmds[1:])                      # in the session the request belongs to
    assert cmds[-1][1] == {"requestId": "task-tab", "errorReason": "Aborted"}   # no browser download ever starts
    assert "unavailable" in out["again"] and out["left"] == 0


@pytest.mark.parametrize("identity", ["owner", "ferb"])
def test_what_becomes_of_a_held_download_nobody_takes(identity: str) -> None:
    """Refused in a task tab: failed (no dialog). Not FERB's (released) or never pulled: back to the browser untouched,
    except in FERB's own profile, where every download is FERB's: failed. No daemon at all: never held."""
    out = worker('''(async () => {
      const cmds = [], sent = [];
      port = {postMessage() {}}; profileIdentity = %r;
      send = async (type, body) => { sent.push(body); };
      cdp = async (tab, method, params) => { cmds.push(method); return {}; };
      tabState(3).armed = true;
      const paused = (id) => ({requestId: id, request: {url: 'https://x/a.zip?' + id, headers: {}}, responseStatusCode: 200,
                               responseHeaders: [{name: 'content-type', value: 'application/zip'}]});
      const held = async (id) => { await onPaused({tabId: 3}, paused(id)); return sent.filter(b => b.event === 'download').pop().download_id; };
      const steps = {};
      steps.abort = await commands.download_part({download_id: await held('r1'), abort: true});
      steps.afterAbort = cmds.splice(0);
      steps.release = await commands.download_part({download_id: await held('r2'), release: true});
      steps.afterRelease = cmds.splice(0);
      DOWNLOAD_IDLE_MS = 30;                                     // nobody pulls it
      await held('r3'); await sleep(80);
      steps.afterIdle = cmds.splice(0);
      const before = sent.length; port = null;                   // the daemon is gone
      await onPaused({tabId: 3}, paused('r4'));
      steps.noDaemon = cmds.splice(0); steps.noDaemonEvents = sent.length - before;
      steps.left = downloads.size;
      return steps;
    })()''' % identity)
    assert out["abort"] == {"aborted": True} and out["afterAbort"] == ["Fetch.failRequest"]
    assert out["release"] == {"released": True, "handed_back": True} and out["afterRelease"] == ["Fetch.continueRequest"]
    gone = "Fetch.failRequest" if identity == "ferb" else "Fetch.continueRequest"
    assert out["afterIdle"] == [gone] and out["noDaemon"] == [gone] and out["noDaemonEvents"] == 0
    assert out["left"] == 0


def test_a_released_download_whose_body_ferb_already_took_can_only_be_failed() -> None:
    out = worker('''(async () => {
      const cmds = []; port = {postMessage() {}}; send = async () => {};
      cdp = async (tab, method) => { cmds.push(method); return method === 'Fetch.takeResponseBodyAsStream' ? {stream: 's'}
                                                        : method === 'IO.read' ? {data: btoa('ab'), base64Encoded: true, eof: false} : {}; };
      tabState(3).armed = true;
      await onPaused({tabId: 3}, {requestId: 'r', request: {url: 'https://x/a.zip', headers: {}}, responseStatusCode: 200,
                                  responseHeaders: [{name: 'content-type', value: 'application/zip'}]});
      const id = [...downloads.keys()][0];
      await commands.download_part({download_id: id, offset: 0});
      const r = await commands.download_part({download_id: id, release: true});
      return {r, cmds};
    })()''')
    assert out["r"]["handed_back"] is False
    assert out["cmds"][-2:] == ["IO.close", "Fetch.failRequest"]


def test_the_hello_lists_armed_tabs_and_disarm_lets_go_without_closing() -> None:
    out = worker('''(async () => {
      const posted = [], cmds = [];
      port = {postMessage(m) { posted.push(m); }}; outbox.length = 0;
      chrome.debugger = new Proxy({sendCommand: async (t, m) => { cmds.push(m); return {}; },
                                   detach: async () => { cmds.push('detach'); },
                                   onEvent: chrome.debugger.onEvent, onDetach: chrome.debugger.onDetach}, {get: (o, k) => o[k]});
      chrome.tabs.remove = async () => { cmds.push('remove'); };
      const t = tabState(7); t.attached = true; t.armed = true; tabState(8);
      await announceHello();
      const hello = JSON.parse(posted.find(m => m.type === 'hello').body_json);
      cdp = async (tab, method) => { cmds.push(method); return {}; };
      await onPaused({tabId: 7}, {requestId: 'r', request: {url: 'https://x/a.zip', headers: {}}, responseStatusCode: 200,
                                  responseHeaders: [{name: 'content-type', value: 'application/zip'}]});
      const r = await commands.disarm({tab_id: 7});
      return {armed: hello.armed, r, cmds, still: TABS.has(7), left: downloads.size};
    })()''')
    assert out["armed"] == [7]
    assert out["r"] == {"disarmed": True, "tab_id": 7} and not out["still"] and out["left"] == 0
    assert "Fetch.continueRequest" in out["cmds"] and "detach" in out["cmds"] and "remove" not in out["cmds"]
    assert out["cmds"].index("Fetch.continueRequest") < out["cmds"].index("detach")


def test_letting_go_of_a_borrowed_tab_hands_its_held_downloads_back_closing_ferbs_own_fails_them() -> None:
    out = worker('''(async () => {
      const cmds = [];
      port = {postMessage() {}}; send = async () => {};
      chrome.debugger = new Proxy({sendCommand: async (t, m) => { cmds.push(m); return {}; },
                                   detach: async () => { cmds.push('detach'); },
                                   onEvent: chrome.debugger.onEvent, onDetach: chrome.debugger.onDetach}, {get: (o, k) => o[k]});
      chrome.tabs.get = async id => ({id, windowId: 9, url: 'https://s.example/'});
      chrome.tabs.remove = async () => { cmds.push('remove'); };
      const pause = (tab) => onPaused({tabId: tab}, {requestId: 'r' + tab, request: {url: 'https://s.example/a', headers: {}},
        responseStatusCode: 200, responseHeaders: [{name: 'Content-Type', value: 'application/zip'}]});
      for (const id of [8, 9]) { const t = tabState(id); t.attached = true; t.armed = true; await pause(id); }
      await commands.release_tab({tab_id: 8});
      const released = cmds.splice(0);
      await commands.close({tab_id: 9});
      return {released, closed: cmds, left: downloads.size};
    })()''')
    assert out["left"] == 0
    assert out["released"].index("Fetch.continueRequest") < out["released"].index("detach")
    assert out["closed"].index("Fetch.failRequest") < out["closed"].index("remove")


def test_a_failed_setup_awaits_letting_go_before_it_detaches() -> None:
    out = worker('''(async () => {
      const order = [];
      port = {postMessage() {}}; send = async () => {};
      arm = async (id) => { const t = tabState(id); t.attached = true; t.armed = true; return t; };
      cdp = async (tab, method) => { order.push(method); if (method === 'Page.setInterceptFileChooserDialog') throw Error('gone'); return {}; };
      dropDownloads = async () => { await sleep(20); order.push('dropped'); };
      chrome.debugger = new Proxy({detach: async () => { order.push('detach'); }, onEvent: chrome.debugger.onEvent,
                                   onDetach: chrome.debugger.onDetach}, {get: (o, k) => o[k]});
      try { await attach(4); } catch (e) {}
      return order;
    })()''')
    assert out.index("dropped") < out.index("detach")


def test_navigation_steps_through_a_blank_page_when_the_tab_cant_be_armed() -> None:
    out = worker('''(async () => {
      const loads = []; let arms = 0, fail = 1;
      chrome.tabs.get = async id => ({id, url: 'chrome://newtab/', title: 'New Tab'});
      load = async (id, url) => { loads.push(url); };
      arm = async (id) => { arms++; if (arms <= fail) throw Error('Cannot access a chrome:// URL'); return tabState(id); };
      const ok = await commands.navigate({tab_id: 3, url: 'https://x.example/f'});
      arms = 0; fail = 2; loads.length = 0;
      const refused = await commands.navigate({tab_id: 3, url: 'https://x.example/f'});
      const blankLoads = loads.slice();
      arm = async () => { throw Error('no debugger'); };
      chrome.windows = {get: async id => ({id})}; chrome.tabs.create = async () => ({id: 11});
      chrome.tabs.update = async () => {}; chrome.tabs.remove = async () => {};
      const opened = await commands.open({url: 'about:blank', window_id: 4}, 'op');
      return {ok, refused, blankLoads, opened};
    })()''')
    assert out["ok"]["tab_id"] == 3 and "navigated" not in out["ok"]
    assert out["refused"]["navigated"] is False and out["refused"]["reason"] == "download_capture_unavailable"
    assert "FERB opens itself" in out["refused"]["detail"] and out["blankLoads"] == ["about:blank"]
    assert out["opened"]["opened"] is False and out["opened"]["reason"] == "download_capture_unavailable"


def test_a_new_tab_from_a_task_tab_is_armed_and_reported_and_links_to_new_tabs_go_through_ferb() -> None:
    out = worker('''(async () => {
      const sent = [], armedTabs_ = [];
      port = {postMessage() {}}; send = async (type, body) => { sent.push(body); };
      arm = async (id) => { armedTabs_.push(id); const t = tabState(id); t.attached = true; t.armed = true; return t; };
      chrome.tabs.get = async id => ({id, windowId: 9});
      tabState(5).armed = true;
      await adoptPopup({id: 21, openerTabId: 5, pendingUrl: 'https://s.example/pop?token=zz'});
      await adoptPopup({id: 22, openerTabId: 6, pendingUrl: 'https://s.example/other'});   // not a tab FERB drives
      const linked = pageOpen(5, {url: 'https://s.example/next'}), stale = pageOpen(6, {url: 'https://s.example/n'});
      const script = pageOpen(5, {url: 'javascript:alert(1)'});
      await sleep(10);
      return {sent, armedTabs_, linked, stale, script, popupFlag: tabState(21).popup};
    })()''')
    assert out["armedTabs_"] == [21] and out["popupFlag"] is True
    popup = next(b for b in out["sent"] if b["event"] == "popup")
    assert popup["tab_id"] == 21 and popup["opener_tab_id"] == 5 and "zz" not in popup["url"]
    assert out["linked"] is True and out["stale"] is False and out["script"] is False
    opens = [b for b in out["sent"] if b["event"] == "open"]
    assert opens == [{"event": "open", "tab_id": 5, "url": "https://s.example/next", "window_id": 9}]


def test_a_browser_download_the_extension_could_not_intercept_is_reported_and_its_own_cancel_is_not() -> None:
    out = worker('''(async () => {
      const sent = [];
      port = {postMessage() {}}; send = async (type, body) => { sent.push(body); };
      cdp = async () => ({});
      const t = tabState(4); t.armed = true;
      const fire = (m, p) => chrome.debugger.onEvent.fire({tabId: 4}, m, p);
      // FERB held this one: the browser's "canceled" entry for it is not news
      await onPaused({tabId: 4}, {requestId: 'r', request: {url: 'https://s.example/held', headers: {}}, responseStatusCode: 200,
                                  responseHeaders: [{name: 'content-type', value: 'application/zip'}]});
      fire('Page.downloadWillBegin', {guid: 'g0', url: 'https://s.example/held', suggestedFilename: 'held.zip'});
      fire('Page.downloadProgress', {guid: 'g0', state: 'canceled'});
      // the browser's own: reported once, and again when it ends canceled (a dismissed Save dialog)
      fire('Page.downloadWillBegin', {guid: 'g1', url: 'blob:https://s.example/u?token=zz', suggestedFilename: 'e.csv'});
      fire('Page.downloadProgress', {guid: 'g1', state: 'inProgress'});
      fire('Page.downloadProgress', {guid: 'g1', state: 'inProgress'});
      fire('Page.downloadProgress', {guid: 'g1', state: 'canceled'});
      // canceled before anything else, not one FERB held
      fire('Page.downloadWillBegin', {guid: 'g2', url: 'https://s.example/x', suggestedFilename: 'x.txt'});
      fire('Page.downloadProgress', {guid: 'g2', state: 'canceled'});
      // a new tab's download that began before FERB attached: only progress is seen
      const p = tabState(6); p.armed = true; p.popup = true;
      chrome.debugger.onEvent.fire({tabId: 6}, 'Page.downloadProgress', {guid: 'g3', state: 'inProgress'});
      chrome.debugger.onEvent.fire({tabId: 6}, 'Page.downloadProgress', {guid: 'g3', state: 'completed'});
      await sleep(20);
      return sent.filter(b => b.uncaptured).map(b => [b.tab_id, b.name, b.detail, b.source_url]);
    })()''')
    assert [(tab, name, detail) for tab, name, detail, _ in out] == [
        (4, "e.csv", "started"), (4, "e.csv", "canceled"), (4, "x.txt", "canceled"), (6, "", "new_tab")]
    assert "zz" not in out[0][3]


def test_held_data_reads_stay_out_of_the_command_journal() -> None:
    out = worker('''(async () => {
      gen = 1; send = async () => {};
      commands.download_part = async () => ({data: '', offset: 0, next_offset: null, eof: true});
      await onCommand({gen: 1, id: 'part-1', op: 'download_part', args: {}});
      commands.ping = async () => ({ok: true});
      await onCommand({gen: 1, id: 'ping-1', op: 'ping', args: {}});
      return (await session()).order;
    })()''')
    assert out == ["ping-1"]


def test_download_classification_follows_what_the_browser_would_do() -> None:
    out = worker('''(async () => {
      const h = (o) => Object.entries(o).map(([name, value]) => ({name, value}));
      const nav = {'Upgrade-Insecure-Requests': '1', Accept: 'text/html'};
      const p = (status, headers, req = nav, url = 'https://s.example/f') =>
        ({requestId: 'r', request: {url, headers: req}, responseStatusCode: status, responseHeaders: h(headers)});
      const expected = new Map();
      const kinds = [
        classify(p(200, {'Content-Disposition': 'attachment; filename=a.txt', 'Content-Type': 'text/plain'}), expected),
        classify(p(200, {'Content-Type': 'text/html; charset=utf-8'}), expected),
        classify(p(200, {'Content-Type': 'application/pdf'}), expected),
        classify(p(200, {'Content-Type': 'text/csv'}), expected),
        classify(p(200, {'Content-Type': 'application/zip'}), expected),
        classify(p(200, {'Content-Type': 'image/png'}), expected),
        classify(p(200, {'Content-Type': 'application/ld+json'}), expected),
        classify(p(404, {'Content-Disposition': 'attachment'}), expected),
        classify(p(200, {'Content-Type': 'text/html'}, {Referer: 'https://s.example/'}), expected),   // <a download>
      ];
      const redirect = classify(p(302, {Location: '/real.txt'}, {Referer: 'x'}, 'https://s.example/dl'), expected);
      const followed = classify(p(200, {'Content-Type': 'text/plain'}, nav, 'https://s.example/real.txt'), expected);
      const unrelated = classify(p(200, {'Content-Type': 'text/plain'}, nav, 'https://s.example/real.txt'), expected);
      return {kinds, redirect, followed, unrelated};
    })()''')
    assert out["kinds"] == ["attachment", None, None, "type", "type", None, None, None, "link"]
    assert out["redirect"] is None and out["followed"] == "link" and out["unrelated"] is None


def test_download_names_come_from_the_response_or_the_url() -> None:
    out = worker('''(async () => [
      downloadName([{name: 'Content-Disposition', value: "attachment; filename*=UTF-8''r%C3%A9sum%C3%A9.pdf; filename=x.pdf"}], 'https://a/b'),
      downloadName([{name: 'content-disposition', value: 'attachment; filename="a b.txt"'}], 'https://a/b'),
      downloadName([], 'https://a/files/report.csv?x=1'),
      downloadName([], 'https://a/'),
    ])()''')
    assert out == ["résumé.pdf", "a b.txt", "report.csv", "download"]


def test_the_extension_allows_only_the_download_mechanics_it_uses() -> None:
    proto = EXT / "protocol.js"
    script = f"""
const p = require({json.dumps(str(proto))});
const cases = [
  ["Fetch.enable", {{patterns: p.DOWNLOAD_PATTERNS}}],
  ["Fetch.enable", {{patterns: [{{urlPattern: "*", requestStage: "Request"}}]}}],
  ["Fetch.enable", {{}}],
  ["Fetch.continueRequest", {{requestId: "r"}}],
  ["Fetch.continueRequest", {{requestId: "r", url: "https://evil.example/"}}],
  ["Fetch.failRequest", {{requestId: "r", errorReason: "Aborted"}}],
  ["Fetch.failRequest", {{requestId: "r", errorReason: "Failed"}}],
  ["Fetch.takeResponseBodyAsStream", {{requestId: "r"}}],
  ["Fetch.fulfillRequest", {{requestId: "r", responseCode: 200}}],
  ["Fetch.getResponseBody", {{requestId: "r"}}],
  ["IO.read", {{handle: "s", size: p.DOWNLOAD_PART}}],
  ["IO.read", {{handle: "s"}}],
  ["IO.read", {{handle: "s", size: 1, offset: 0}}],
  ["IO.close", {{handle: "s"}}],
  ["IO.resolveBlob", {{objectId: "o"}}],
  ["Browser.setDownloadBehavior", {{behavior: "deny"}}],
  ["Page.setDownloadBehavior", {{behavior: "deny"}}],
];
console.log(JSON.stringify(cases.map(([m, a]) => p.cdpAllowed(m, a))));
console.log(JSON.stringify([p.cdpFilter("IO.read", {{data: "x", base64Encoded: true, eof: false, extra: 1}}),
                            p.cdpFilter("Fetch.takeResponseBodyAsStream", {{stream: "s", other: 2}})]));
"""
    out = subprocess.run([_node(), "-e", script], capture_output=True, text=True, timeout=20).stdout.splitlines()
    assert json.loads(out[0]) == [True, False, False, True, False, True, False, True, False, False, True, False, False,
                                  True, False, False, False]
    assert json.loads(out[1]) == [{"data": "x", "base64Encoded": True, "eof": False}, {"stream": "s"}]


CONTENT_HARNESS = r"""
const vm = require('vm'), fs = require('fs');
const listeners = {}, messages = [], passed = [];
class HTMLAnchorElement { constructor(href, attrs = {}) { this.href = href; this.attrs = attrs; this.isConnected = true; }
  hasAttribute(n) { return n in this.attrs; } getAttribute(n) { return n in this.attrs ? this.attrs[n] : null; }
  click() { passed.push({href: this.href, target: this.target || null, download: this.download === undefined ? null : this.download}); } }
const blob = {size: 9, type: 'text/csv', slice: (a, b) => ({arrayBuffer: async () => new TextEncoder().encode('a,b\n1,2\n!').slice(a, b).buffer})};
const mode = process.argv[2];                                    // armed | unarmed | stale
const ctx = vm.createContext({document: {readyState: 'complete', createElement: () => new HTMLAnchorElement(''),
    querySelector: () => null}, window: {}, crypto: globalThis.crypto, WeakRef, TextEncoder, CSS: {escape: s => s},
  btoa, atob, innerWidth: 800, innerHeight: 600, location: {origin: 'https://s.example', href: 'https://s.example/'},
  addEventListener: (t, fn, c) => { (listeners[t] = listeners[t] || []).push(fn); }, setTimeout,
  MutationObserver: class { observe() {} }, ShadowRoot: class {}, HTMLAnchorElement,
  URL: Object.assign(function () {}, {createObjectURL: () => 'blob:https://s.example/again', revokeObjectURL() {}}),
  fetch: async (u) => ({blob: async () => blob}),
  chrome: {runtime: {sendMessage: (m, cb) => { messages.push(m); if (cb) cb(mode === 'armed' ? true : mode === 'stale' ? false : false); }}}});
if (mode === 'stale') ctx.chrome.runtime.sendMessage = (m, cb) => { messages.push(m); if (cb) cb(m.ferbArmed ? true : false); };
vm.runInContext(fs.readFileSync(process.argv[1], 'utf8'), ctx);
const click = (type, a, extra = {}) => { const ev = {type, prevented: false, defaultPrevented: !!extra.cancelled,
  composedPath: () => [a], preventDefault() { this.prevented = true; }, button: 0, ...extra};
  for (const fn of listeners[type] || []) fn(ev); return ev.prevented; };
(async () => {
  const A = (href, attrs) => new HTMLAnchorElement(href, attrs);
  const clicks = {
    blob: click('click', A('blob:https://s.example/1', {download: 'r.csv'})),
    plain: click('click', A('https://s.example/plain', {})),
    data: click('click', A('data:text/csv;base64,YQ==', {download: 'd.csv'})),
    blank: click('click', A('https://s.example/next', {target: '_blank'})),
    cmd: click('click', A('https://s.example/cmd', {}), {metaKey: true}),
    middle: click('auxclick', A('https://s.example/mid', {}), {button: 1}),
    httpDownload: click('click', A('https://s.example/file.zip', {download: '', target: '_blank'})),
    pageCancelled: click('click', A('https://s.example/spa', {target: '_blank'}), {cancelled: true}),
  };
  await new Promise(r => setTimeout(r, 20));
  const caught = messages.filter(m => m.ferbDownload).map(m => m.ferbDownload);
  const opens = messages.filter(m => m.ferbOpen).map(m => m.ferbOpen.url);
  const part = mode === 'armed' && caught.length ? await ctx.window.__ferb.downloadPart(caught[0].id, 0, 4) : null;
  console.log(JSON.stringify({clicks, caught, opens, part, passed}));
  process.exit(0);                                               // a pending revoke timer must not hold node open
})();
"""


def _content(mode: str) -> dict:
    r = subprocess.run([_node(), "-e", CONTENT_HARNESS, str(EXT / "content.js"), mode], capture_output=True, text=True,
                       timeout=20)
    assert r.returncode == 0, r.stderr
    return json.loads(r.stdout)


def test_the_page_listener_takes_blob_and_data_links_and_new_tab_links_while_ferb_drives_the_tab() -> None:
    out = _content("armed")
    assert out["clicks"] == {"blob": True, "plain": False, "data": True, "blank": True, "cmd": True, "middle": True,
                             "httpDownload": False, "pageCancelled": False}
    assert [c["name"] for c in out["caught"]] == ["r.csv", "d.csv"] and out["caught"][0]["size"] == 9
    assert out["caught"][1]["source_url"] == "data:text/csv"       # never the data itself as a "URL"
    assert out["opens"] == ["https://s.example/next", "https://s.example/cmd", "https://s.example/mid"]
    assert base64.b64decode(out["part"]["data"]) == b"a,b\n" and out["part"]["eof"] is False
    assert out["passed"] == []                                     # nothing handed back: FERB has them all


def test_the_page_listener_does_nothing_in_a_tab_ferb_isnt_driving() -> None:
    out = _content("unarmed")
    assert not any(out["clicks"].values()) and out["caught"] == [] and out["opens"] == [] and out["passed"] == []


def test_a_stale_page_listener_hands_the_owners_click_back_to_the_browser() -> None:
    """content.js still armed, but the service worker no longer holds the tab: the owner's blob/data download and new
    tab go to the browser as they would have, never cancelled or dropped."""
    out = _content("stale")
    passed = {p["href"]: p for p in out["passed"]}
    assert passed["blob:https://s.example/again"]["download"] == "r.csv"         # the held Blob, as a fresh URL
    assert passed["data:text/csv;base64,YQ=="]["download"] == "d.csv"
    assert passed["https://s.example/next"]["target"] == "_blank"
    assert {"https://s.example/cmd", "https://s.example/mid"} <= set(passed)
