"""D35, the extension's own side under node with fake Chrome APIs (no browser): which responses are downloads, the
held transfer (taken in parts, aborted, handed back to the browser), arming only tabs FERB drives, letting go,
navigation through a blank page, new tabs, the browser's own downloads, and content.js's link listener.
The browser-level behaviour was measured in the headless lab (task 10 report)."""

import base64
import json
import re
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
    gone = "Fetch.failRequest" if identity == "ferb" else "Fetch.continueRequest"   # FERB's profile: never the browser's
    assert out["abort"] == {"aborted": True} and out["afterAbort"] == ["Fetch.failRequest"]
    assert out["release"] == {"released": True, "handed_back": identity == "owner"} and out["afterRelease"] == [gone]
    assert out["afterIdle"] == [gone] and out["noDaemon"] == [gone] and out["noDaemonEvents"] == 0
    assert out["left"] == 0


def test_a_download_handed_back_from_a_tab_ferb_keeps_holding_leaves_its_debugger_and_focus_emulation() -> None:
    """L31, owner 2026-10-25 ("give them back between rounds"): between a loop's rounds the daemon hands the owner's
    download in the held tab back to the browser and opens their new-tab link as an ordinary tab. Neither touches the
    tab's debugger session or its focus emulation, which keep the page running."""
    out = worker('''(async () => {
      const cmds = [], sent = [], created = [];
      port = {postMessage() {}}; profileIdentity = 'owner';
      send = async (type, body) => { sent.push(body); };
      cdp = async (tab, method, params) => { cmds.push(method); return {}; };
      chrome.debugger = {sendCommand: async (t, m) => { cmds.push(m); }, detach: async () => { cmds.push('detach'); }};
      chrome.tabs.get = async id => ({id, windowId: 5, index: 2});
      chrome.tabs.create = async (props) => { created.push(props); return {id: 77, windowId: props.windowId}; };
      const st = tabState(3); st.attached = true; st.armed = true; st.ready = true;
      await onPaused({tabId: 3}, {requestId: 'r1', request: {url: 'https://x/mine.pdf', headers: {}},
        responseStatusCode: 200, responseHeaders: [{name: 'content-type', value: 'application/pdf'},
                                                   {name: 'content-disposition', value: 'attachment'}]});
      const id = sent.filter(b => b.event === 'download').pop().download_id;
      cmds.splice(0);
      const back = await commands.download_part({download_id: id, release: true});
      const opened = await commands.open_plain({tab_id: 3, url: 'https://x/p/mine', active: true});
      return {back, cmds, opened, created, attached: TABS.get(3).attached, armed: TABS.get(3).armed};
    })()''')
    assert out["back"] == {"released": True, "handed_back": True} and out["cmds"] == ["Fetch.continueRequest"]
    assert out["opened"]["opened"] is True and out["created"][0]["url"] == "https://x/p/mine"
    assert out["attached"] is True and out["armed"] is True                   # still held, still running


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
      await mutateRecords(r => { r.tabs[9] = {task: 'x', root: 'x', cid: 'c9'}; });   // tab 9 is one FERB opened
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


TOKEN = "00112233445566778899aabbccddeeff"
OTHER = "ffeeddccbbaa99887766554433221100"


def test_only_a_new_popup_proven_by_its_token_and_its_opener_is_claimed_and_links_to_new_tabs_go_through_ferb() -> None:
    """A claim needs a NEW tab (chrome.tabs.onCreated, then its first URL report) at exactly about:blank#ferb-<token>
    for a token a live task tab relayed, and the popup's real opener (CDP Target.getTargetInfo openerId, measured in
    the lab) must be that task tab. A tab that existed before, or a popup of some other tab, is never FERB's."""
    out = worker('''(async () => {
      const sent = [], armed_ = [], cdps = [], detached = [];
      port = {postMessage() {}}; send = async (type, body) => { sent.push(body); };
      arm = async (id) => { const t = tabState(id); if (!t.armed) armed_.push(id); t.attached = true; t.armed = true; return t; };
      const OPENER = {21: 'T5', 22: 'T5', 23: 'T200', 25: 'T5'};        // what the browser says opened each new tab
      cdp = async (tab, method) => { cdps.push([tab, method]);
        if (method !== 'Target.getTargetInfo') return {};
        if (tab === 24) throw Error('not allowed');
        return {targetInfo: {targetId: 'T' + tab, openerId: OPENER[tab]}}; };
      chrome.debugger = new Proxy({attach: async () => {}, detach: async ({tabId}) => { detached.push(tabId); },
        sendCommand: async () => ({}), onEvent: chrome.debugger.onEvent, onDetach: chrome.debugger.onDetach}, {get: (o, k) => o[k]});
      chrome.tabs.get = async id => ({id, windowId: 9});
      tabState(5).armed = true;
      // relay first, then the new tab (a popup window with features is just another new tab with that URL)
      const relayed = pageHookPopup(5, 0, {token: '%(T)s', url: 'https://s.example/pop?token=zz'});
      tabCreated({id: 21}); tabUpdated(21, 'about:blank#ferb-%(T)s');
      // the new tab first, then the relay
      tabCreated({id: 22, pendingUrl: 'about:blank#ferb-%(O)s'});
      pageHookPopup(5, 0, {token: '%(O)s', url: 'https://s.example/other'});
      await sleep(20);
      // the self-navigation attack, both orders: a tab that existed before navigates itself to the magic URL while a
      // cooperating frame in task tab 5 relays the token
      pageHookPopup(5, 0, {token: 'abababababababababababababababab', url: 'https://evil.example/'});
      tabUpdated(77, 'about:blank#ferb-abababababababababababababababab');
      tabUpdated(79, 'about:blank#ferb-cdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcd');
      pageHookPopup(5, 0, {token: 'cdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcd', url: 'https://evil.example/2'});
      // a NEW popup the owner's own tab 200 opened at the magic URL, with the token relayed from task tab 5: the
      // browser names tab 200 as its opener, so it is let go of (attached only to ask, then detached)
      pageHookPopup(5, 0, {token: 'efefefefefefefefefefefefefefefef', url: 'https://evil.example/3'});
      tabCreated({id: 23}); tabUpdated(23, 'about:blank#ferb-efefefefefefefefefefefefefefefef');
      // the opener can't be confirmed at all: no claim
      pageHookPopup(5, 0, {token: '12121212121212121212121212121212', url: 'https://s.example/4'});
      tabCreated({id: 24}); tabUpdated(24, 'about:blank#ferb-12121212121212121212121212121212');
      // a new tab whose first URL was something else, later at the magic URL: not its first URL report
      pageHookPopup(5, 0, {token: '34343434343434343434343434343434', url: 'https://s.example/5'});
      tabCreated({id: 25, pendingUrl: 'https://owner.example/their-link'}); tabUpdated(25, 'about:blank#ferb-34343434343434343434343434343434');
      tabCreated({id: 26}); tabUpdated(26, 'https://owner.example/x'); tabUpdated(26, 'about:blank#ferb-34343434343434343434343434343434');
      // a relay from a tab FERB doesn't drive, and a non-http URL, are refused outright
      const stale = pageHookPopup(6, 0, {token: '56565656565656565656565656565656', url: 'https://s.example/x'});
      const script = pageHookPopup(5, 0, {token: '78787878787878787878787878787878', url: 'javascript:alert(1)'});
      const linkedP = pageOpen(5, {url: 'https://s.example/next'}), staleOpen = await pageOpen(6, {url: 'https://s.example/n'});
      const bgP = pageOpen(5, {url: 'https://s.example/bg', background: true});
      await sleep(30);
      for (const b of sent.filter(b => b.event === 'open')) await commands.open_seen({open_id: b.open_id});   // the daemon
      const linked = await linkedP, bg = await bgP;
      return {sent, armed_, cdps, detached, relayed, stale, script, linked, staleOpen, bg, t21: tabState(21).popupFor,
              tabs: [...TABS.keys()]};
    })()''' % {"T": TOKEN, "O": OTHER})
    assert out["armed_"] == [21, 22]
    assert not ({77, 79, 23, 24, 25, 26} & set(out["tabs"])) and set(out["detached"]) == {23, 24}
    popups = [b for b in out["sent"] if b["event"] == "popup"]
    assert [(b["tab_id"], b["opener_tab_id"]) for b in popups] == [(21, 5), (22, 5)] and "zz" not in popups[0]["url"]
    assert out["t21"] == {"opener": 5, "frameId": 0, "token": TOKEN}
    # each claim asked the popup and the opener who opened it, before anything else happened to the popup
    asked = [c[0] for c in out["cdps"] if c[1] == "Target.getTargetInfo"]
    assert asked[0] == 21 and {21, 22, 23, 24, 5} <= set(asked) and out["cdps"][0][1] == "Target.getTargetInfo"
    assert out["relayed"] is True and out["stale"] is False and out["script"] is False
    assert out["linked"] is True and out["staleOpen"] is False and out["bg"] is True
    opens = [b for b in out["sent"] if b["event"] == "open"]
    assert [(b["url"], b["background"]) for b in opens] == [("https://s.example/next", False), ("https://s.example/bg", True)]


def test_popup_relays_and_page_holds_are_bounded_and_a_token_is_relayed_once() -> None:
    out = worker('''(async () => {
      port = {postMessage() {}}; send = async () => {};
      for (const id of [1, 2, 3, 4, 5, 6]) tabState(id).armed = true;
      const tok = (n) => n.toString(16).padStart(32, '0');
      for (let i = 0; i < 30; i++) pageHookPopup(1, 0, {token: tok(i), url: 'https://s.example/' + i});
      const perOpener = [...popupRelays.values()].filter(r => r.opener === 1).length;
      const oldestDropped = !popupRelays.has(tok(0)) && popupRelays.has(tok(29));
      const again = pageHookPopup(2, 0, {token: tok(29), url: 'https://evil.example/replaced'});   // the first relay for a token wins
      const kept = popupRelays.get(tok(29));
      for (let t = 2; t <= 6; t++) for (let i = 0; i < 20; i++) pageHookPopup(t, 0, {token: tok(1000 * t + i), url: 'https://s.example/'});
      const total = popupRelays.size;
      // page holds (content.js's blob/data links): a flood is dropped, never handed to the browser
      const answers = [];
      for (let i = 0; i < 25; i++) answers.push(pageDownload(3, 0, {id: 'h' + i, name: 'x', size: 1}));
      return {perOpener, oldestDropped, total, again, kept: kept && kept.url, answers, held: [...downloads.values()].filter(tr => tr.page).length};
    })()''')
    assert out["perOpener"] == 20 and out["oldestDropped"] is True and out["total"] == 100
    assert out["again"] is False and out["kept"] == "https://s.example/29"
    assert out["answers"] == [True] * 20 + ["drop"] * 5 and out["held"] == 20


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
  ["Target.getTargetInfo", {{}}],
  ["Target.getTargetInfo", {{targetId: "ABC"}}],
  ["Target.getTargets", {{}}],
];
console.log(JSON.stringify(cases.map(([m, a]) => p.cdpAllowed(m, a))));
console.log(JSON.stringify([p.cdpFilter("IO.read", {{data: "x", base64Encoded: true, eof: false, extra: 1}}),
                            p.cdpFilter("Fetch.takeResponseBodyAsStream", {{stream: "s", other: 2}}),
                            p.cdpFilter("Target.getTargetInfo", {{targetInfo: {{targetId: "A", openerId: "B", url: "https://x/?token=1", title: "t"}}}})]));
"""
    out = subprocess.run([_node(), "-e", script], capture_output=True, text=True, timeout=20).stdout.splitlines()
    assert json.loads(out[0]) == [True, False, False, True, False, True, False, True, False, False, True, False, False,
                                  True, False, False, False, True, False, False]
    assert json.loads(out[1]) == [{"data": "x", "base64Encoded": True, "eof": False}, {"stream": "s"},
                                  {"targetInfo": {"targetId": "A", "openerId": "B"}}]          # never the URL or title


CONTENT_HARNESS = r"""
const vm = require('vm'), fs = require('fs');
const listeners = {capture: {}, bubble: {}}, messages = [], passed = [];
class HTMLAnchorElement { constructor(href, attrs = {}) { this.href = href; this.attrs = attrs; this.isConnected = true; }
  hasAttribute(n) { return n in this.attrs; } getAttribute(n) { return n in this.attrs ? this.attrs[n] : null; }
  click() { passed.push({href: this.href, target: this.target || null, download: this.download === undefined ? null : this.download}); } }
const blob = {size: 9, type: 'text/csv', slice: (a, b) => ({arrayBuffer: async () => new TextEncoder().encode('a,b\n1,2\n!').slice(a, b).buffer})};
const mode = process.argv[2];                                    // armed | unarmed | stale | base | orphaned | invalidating
const base = mode === 'base' ? new HTMLAnchorElement('', {target: '_blank'}) : null;
const docListeners = {};
const HOOK = 'abcdefabcdef';                                     // the service worker's FERB_TOKEN (protocol 18)
const ctx = vm.createContext({document: {readyState: 'complete', createElement: () => new HTMLAnchorElement(''),
    querySelector: (sel) => sel === 'base[target]' ? base : null,
    addEventListener: (t, fn) => { (docListeners[t] = docListeners[t] || []).push(fn); },
    dispatchEvent: (e) => { for (const fn of docListeners[e.type] || []) fn(e); return !e.defaultPrevented; }},
  window: {}, crypto: globalThis.crypto, WeakRef, TextEncoder, CSS: {escape: s => s},
  CustomEvent: class { constructor(type, init = {}) { this.type = type; this.detail = init.detail; this.defaultPrevented = false; }
                       preventDefault() { this.defaultPrevented = true; } },
  btoa, atob, innerWidth: 800, innerHeight: 600, location: {origin: 'https://s.example', href: 'https://s.example/'},
  addEventListener: (t, fn, c) => { const phase = c === true ? 'capture' : 'bubble'; (listeners[phase][t] = listeners[phase][t] || []).push(fn); },
  setTimeout, MutationObserver: class { observe() {} }, ShadowRoot: class {}, HTMLAnchorElement,
  URL: Object.assign(function () {}, {createObjectURL: () => 'blob:https://s.example/again', revokeObjectURL() {}}),
  fetch: async (u) => ({blob: async () => blob}),
  // orphaned: the extension reloaded under this document (no runtime id, every message throws); invalidating: the id
  // still reads, but sending throws
  chrome: {runtime: {id: mode === 'orphaned' ? undefined : 'ferb',
                     sendMessage: (m, cb) => { if ((mode === 'orphaned' || mode === 'invalidating') && !m.ferbArmed)
                                                 throw new Error('Extension context invalidated.');
                                               messages.push(m);
                                               const answer = mode === 'unarmed' ? false : mode === 'stale' ? !!m.ferbArmed : true;
                                               if (cb) cb(m.ferbArmed ? {armed: answer, token: HOOK} : answer); },
                     onMessage: {addListener: (fn) => { onMessage.push(fn); }}}}});
const onMessage = [];
vm.runInContext(fs.readFileSync(require('path').join(require('path').dirname(process.argv[1]), 'mask.js'), 'utf8'), ctx);
vm.runInContext(fs.readFileSync(process.argv[1], 'utf8'), ctx);
// capture listeners, then the page's own handler (cancel / stopPropagation), then bubble listeners unless stopped
const click = (type, a, o = {}) => { const ev = {type, prevented: false, defaultPrevented: false, isTrusted: o.trusted !== false,
  composedPath: () => [a], preventDefault() { this.prevented = true; this.defaultPrevented = true; }, button: o.button || 0,
  metaKey: !!o.metaKey, shiftKey: !!o.shiftKey};
  for (const fn of listeners.capture[type] || []) fn(ev);
  if (o.pageCancels) ev.defaultPrevented = true;
  if (!o.pageStops) for (const fn of listeners.bubble[type] || []) fn(ev);
  return ev.prevented; };
(async () => {
  const A = (href, attrs) => new HTMLAnchorElement(href, attrs);
  const clicks = {
    blob: click('click', A('blob:https://s.example/1', {download: 'r.csv'})),
    blobStopped: click('click', A('blob:https://s.example/2', {download: 's.csv'}), {pageStops: true, trusted: false}),
    plain: click('click', A('https://s.example/plain', {})),
    data: click('click', A('data:text/csv;base64,YQ==', {download: 'd.csv'})),
    blank: click('click', A('https://s.example/next', {target: '_blank'})),
    untrusted: click('click', A('https://s.example/script', {target: '_blank'}), {trusted: false}),
    named: click('click', A('https://s.example/named', {target: 'results'})),
    baseTarget: click('click', A('https://s.example/viabase', {})),
    cmd: click('click', A('https://s.example/cmd', {}), {metaKey: true}),
    middle: click('auxclick', A('https://s.example/mid', {}), {button: 1}),
    httpDownload: click('click', A('https://s.example/file.zip', {download: '', target: '_blank'})),
    pageCancelled: click('click', A('https://s.example/spa', {target: '_blank'}), {pageCancels: true}),
    cmdShift: click('click', A('https://s.example/cmdshift', {}), {metaKey: true, shiftKey: true}),
  };
  const armedAnswer = !ctx.document.dispatchEvent(new ctx.CustomEvent(HOOK + ':armed', {cancelable: true}));
  const fixedName = !ctx.document.dispatchEvent(new ctx.CustomEvent('ferb-armed', {cancelable: true}));   // protocol 17
  const T = '00112233445566778899aabbccddeeff';
  ctx.document.dispatchEvent(new ctx.CustomEvent(HOOK + ':popup', {detail: JSON.stringify({token: T, url: 'https://s.example/pop'})}));
  ctx.document.dispatchEvent(new ctx.CustomEvent(HOOK + ':popup', {detail: JSON.stringify({token: 't1', url: 'https://s.example/short'})}));
  ctx.document.dispatchEvent(new ctx.CustomEvent(HOOK + ':popup', {detail: JSON.stringify({token: T.toUpperCase(), url: 'https://s.example/upper'})}));
  ctx.document.dispatchEvent(new ctx.CustomEvent(HOOK + ':popup', {detail: JSON.stringify({token: T, url: 'https://s.example/' + 'a'.repeat(9000)})}));
  const readyEvents = []; ctx.document.addEventListener(HOOK + ':ready', (e) => readyEvents.push(e.detail));
  const replies = []; for (const fn of onMessage) fn({ferbPopupReady: T}, {}, (v) => replies.push(v));
  if (mode === 'armed') for (let i = 0; i < 30; i++) click('click', A('blob:https://s.example/flood' + i, {download: 'f.csv'}));
  await new Promise(r => setTimeout(r, 20));
  const caught = messages.filter(m => m.ferbDownload).map(m => m.ferbDownload);
  const opens = messages.filter(m => m.ferbOpen).map(m => m.ferbOpen.url);
  const openBg = messages.filter(m => m.ferbOpen).map(m => m.ferbOpen.background);
  const popups = messages.filter(m => m.ferbPopup).map(m => ({token: m.ferbPopup.token, url: m.ferbPopup.url.length > 100 ? m.ferbPopup.url.length : m.ferbPopup.url}));
  const flood = messages.filter(m => m.ferbDownload && m.ferbDownload.name === 'f.csv').length;
  const dropped = messages.filter(m => m.ferbDropped).length;
  const part = mode === 'armed' && caught.length ? await ctx.window.__ferb.downloadPart(caught[0].id, 0, 4) : null;
  // exit only once the answer is written: node writes to a pipe asynchronously on macOS (see the large-answer test)
  process.stdout.write(JSON.stringify({clicks, caught: caught.filter(c => c.name !== 'f.csv'), opens, openBg, popups, part, passed, armedAnswer, readyEvents, replies, flood, dropped}) + "\n",
                       () => process.exit(0));                   // a pending revoke timer must not hold node open
})();
"""


def _content(mode: str) -> dict:
    r = subprocess.run([_node(), "-e", CONTENT_HARNESS, str(EXT / "content.js"), mode], capture_output=True, text=True,
                       timeout=20)
    assert r.returncode == 0, r.stderr
    return json.loads(r.stdout)


def test_the_page_listener_takes_blob_and_data_links_and_trusted_new_tab_links_while_ferb_drives_the_tab() -> None:
    out = _content("armed")
    assert out["clicks"] == {"blob": True, "blobStopped": True, "plain": False, "data": True, "blank": True,
                             "untrusted": False, "named": False, "baseTarget": False, "cmd": True, "middle": True,
                             "httpDownload": False, "pageCancelled": False, "cmdShift": True}
    assert [c["name"] for c in out["caught"]] == ["r.csv", "s.csv", "d.csv"]   # a page's stopPropagation can't hide one
    assert out["caught"][2]["source_url"] == "data:text/csv"       # never the data itself as a "URL"
    assert out["opens"] == ["https://s.example/next", "https://s.example/cmd", "https://s.example/mid",
                            "https://s.example/cmdshift"]
    # cmd/ctrl or middle click: a background tab, as the browser would; with shift the browser opens it in the foreground
    assert out["openBg"] == [False, True, True, False]
    assert base64.b64decode(out["part"]["data"]) == b"a,b\n" and out["part"]["eof"] is False
    assert out["passed"] == []                                     # nothing handed back: FERB has them all
    assert out["armedAnswer"] is True                              # the page hook's question, answered
    # relays: the token must be exactly 32 hex characters, the URL is cut to 8192 characters before IPC
    assert out["popups"] == [{"token": "00112233445566778899aabbccddeeff", "url": "https://s.example/pop"},
                             {"token": "00112233445566778899aabbccddeeff", "url": 8192}]
    assert out["readyEvents"] == ["00112233445566778899aabbccddeeff"] and out["replies"] == [True]   # popup_ready answered
    assert out["flood"] == 17                     # 20 holds in all (the 3 above and 17 of the flood); the rest dropped, never native
    assert out["dropped"] == 1                    # the 13 dropped saves: one note to the worker, not one per save


def test_a_base_target_blank_link_is_a_new_tab_link() -> None:
    assert _content("base")["clicks"]["baseTarget"] is True


def test_the_page_listener_does_nothing_in_a_tab_ferb_isnt_driving() -> None:
    out = _content("unarmed")
    assert not any(out["clicks"].values()) and out["caught"] == [] and out["opens"] == [] and out["passed"] == []
    assert out["armedAnswer"] is False and out["popups"] == [] and out["flood"] == 0 and out["dropped"] == 0


def test_a_stale_page_listener_hands_the_owners_click_back_to_the_browser() -> None:
    """content.js still armed, but the service worker no longer holds the tab: the owner's blob/data download and new
    tab go to the browser as they would have, never cancelled or dropped."""
    out = _content("stale")
    passed = {p["href"]: p for p in out["passed"]}
    assert passed["blob:https://s.example/again"]["download"] in ("r.csv", "s.csv")   # the held Blob, as a fresh URL
    assert passed["data:text/csv;base64,YQ=="]["download"] == "d.csv"
    assert passed["https://s.example/next"]["target"] == "_blank"
    assert {"https://s.example/cmd", "https://s.example/mid", "https://s.example/cmdshift"} <= set(passed)


HOOK_HARNESS = r"""
const vm = require('vm');
const p = require(process.argv[1]);
const log = [];
class EventTarget { constructor() { this.l = {}; }
  addEventListener(t, f) { (this.l[t] = this.l[t] || []).push(f); }
  removeEventListener(t, f) { this.l[t] = (this.l[t] || []).filter((x) => x !== f); }
  dispatchEvent(e) { for (const f of (this.l[e.type] || []).slice()) f(e); return !e.defaultPrevented; } }
class HTMLElement extends EventTarget { constructor(href = '', attrs = {}) { super(); this.href = href; this.attrs = attrs; this.isConnected = false; }
  click() { log.push(['native-click', this.href, this.isConnected]); }
  hasAttribute(n) { return n in this.attrs; } remove() { this.isConnected = false; log.push(['removed']); } }
class HTMLAnchorElement extends HTMLElement {}
class HTMLAreaElement extends HTMLElement {}
class CustomEvent { constructor(t, i = {}) { this.type = t; this.detail = i.detail; this.cancelable = !!i.cancelable; this.defaultPrevented = false; }
  preventDefault() { if (this.cancelable) this.defaultPrevented = true; } }
const document = new EventTarget();
document.baseURI = process.argv[3];                              // an http page that isn't localhost works the same
document.documentElement = {appendChild(el) { el.isConnected = true; log.push(['appended']); }};
const popups = [];
const window = {open: (u, t, f) => {
  // a popup like the browser's: reading location.href of a document another origin now holds throws, a closed one is `closed`
  const w = {navigations: [], location: {}, closed: false, foreign: false}; let href = u;
  Object.defineProperty(w.location, 'href', {get: () => { if (w.foreign) throw new Error('SecurityError'); return href; },
                                             set: (v) => { w.navigations.push(v); href = v; }});
  popups.push(w); log.push(['native-open', u, t === undefined ? null : t, f === undefined ? null : f]); return w; }};
let armed = process.argv[2] === 'armed';
document.addEventListener(p.FERB_TOKEN + ':armed', (e) => { if (armed) e.preventDefault(); });
const relayed = [];
document.addEventListener(p.FERB_TOKEN + ':popup', (e) => relayed.push(JSON.parse(e.detail)));
const ctx = vm.createContext({window, document, EventTarget, HTMLElement, HTMLAnchorElement, HTMLAreaElement, CustomEvent,
                              URL, crypto: {getRandomValues: (a) => globalThis.crypto.getRandomValues(a)}, setTimeout});
vm.runInContext(p.PAGE_HOOK, ctx);
vm.runInContext(p.PAGE_HOOK, ctx);                               // installed twice: wraps once
(async () => {
  const out = {};
  const mark = () => log.splice(0);
  new HTMLAnchorElement('blob:https://s.example/1', {download: ''}).click();
  out.unseenClick = mark();
  new HTMLAnchorElement('blob:https://s.example/2', {download: ''}).dispatchEvent({type: 'click'});
  out.unseenDispatch = mark();
  new HTMLAreaElement('data:text/plain,x', {download: 'a.txt'}).click();
  out.area = mark();
  new HTMLAnchorElement('https://s.example/f', {download: ''}).click();
  out.httpLink = mark();
  const w1 = window.open('/file.zip');                            // nobody says ready: the hook loads it after 1.5 s
  out.open = mark(); out.relayed = relayed.splice(0);
  const w2 = window.open('/oauth', '_blank', 'popup,width=400,height=300');   // a popup window with features
  out.openWindow = mark();
  const r2 = relayed.splice(0)[0];
  document.dispatchEvent(new CustomEvent(p.FERB_TOKEN + ':ready', {detail: r2 ? r2.token : ''}));
  document.dispatchEvent(new CustomEvent(p.FERB_TOKEN + ':ready', {detail: r2 ? r2.token : ''}));   // twice: still one load
  out.readyNav = w2 ? w2.navigations.slice() : null;
  // the page navigates its own popup before FERB says ready (or before the fallback): the page's URL wins
  const w3 = window.open('/placeholder'); const r3 = relayed.splice(0)[0];
  w3.location.href = 'https://elsewhere.example/the-pages-choice';
  document.dispatchEvent(new CustomEvent(p.FERB_TOKEN + ':ready', {detail: r3 ? r3.token : ''}));
  out.pageWinsReady = w3.navigations.slice();
  const w4 = window.open('/placeholder2'); relayed.splice(0);
  w4.location.href = 'https://elsewhere.example/fallback-choice'; w4.foreign = true;   // committed cross-origin
  const w5 = window.open('/closed-before-ready'); const r5 = relayed.splice(0)[0];
  w5.closed = true;
  document.dispatchEvent(new CustomEvent(p.FERB_TOKEN + ':ready', {detail: r5 ? r5.token : ''}));
  out.closedNav = w5.navigations.slice(); mark();
  window.open(); window.open(''); window.open('   '); out.empty = mark();
  window.open('https://s.example/named', 'results'); out.named = mark();
  window.open('https://s.example/nop', '_blank', 'noopener'); out.noopener = mark();
  window.open('javascript:alert(1)'); out.script = mark();
  await new Promise((r) => setTimeout(r, 1700));
  out.fallbackNav = w1 ? w1.navigations : null;
  out.readyNavAfter = w2 ? w2.navigations : null;
  out.pageWinsReadyAfter = w3.navigations; out.pageWinsFallback = w4.navigations; out.closedNavAfter = w5.navigations;
  console.log(JSON.stringify(out));
})();
"""


def _hook(mode: str, base: str = "https://s.example/page") -> dict:
    r = subprocess.run([_node(), "-e", HOOK_HARNESS, str(EXT / "protocol.js"), mode, base], capture_output=True,
                       text=True, timeout=20)
    assert r.returncode == 0, r.stderr
    return json.loads(r.stdout)


@pytest.mark.parametrize("base", ["https://s.example/page", "http://lab.test:8080/page"])
def test_the_page_hook_brings_unseen_saves_in_and_binds_popups_to_a_token_while_ferb_drives(base: str) -> None:
    out = _hook("armed", base)
    origin = base.rsplit("/", 1)[0]
    assert out["unseenClick"] == [["appended"], ["native-click", "blob:https://s.example/1", True], ["removed"]]
    # a page's `new MouseEvent("click")` can't be cancelled: the hook clicks it itself, connected, so content.js can
    assert out["unseenDispatch"] == [["appended"], ["native-click", "blob:https://s.example/2", True], ["removed"]]
    assert out["area"][0] == ["appended"]
    assert out["httpLink"] == [["native-click", "https://s.example/f", False]]     # the debugger holds http(s) ones
    [[op, url, target, features]] = out["open"]
    assert op == "native-open" and target == "_blank" and features is None
    assert re.fullmatch(r"about:blank#ferb-[0-9a-f]{32}", url)                    # what the extension claims, exactly
    assert out["relayed"] == [{"token": url.split("ferb-")[1], "url": f"{origin}/file.zip"}]
    assert out["openWindow"][0][3] == "popup,width=400,height=300"               # features kept: a popup window
    assert out["readyNav"] == [f"{origin}/oauth"]                                 # the page loads it when ready...
    assert out["readyNavAfter"] == [f"{origin}/oauth"]                            # ...exactly once
    assert out["fallbackNav"] == [f"{origin}/file.zip"]                           # ...or itself after 1.5 s, once
    # a popup the page navigated itself meanwhile is left where the page sent it (ready path and fallback alike)
    assert out["pageWinsReady"] == ["https://elsewhere.example/the-pages-choice"]
    assert out["pageWinsReadyAfter"] == ["https://elsewhere.example/the-pages-choice"]
    assert out["pageWinsFallback"] == ["https://elsewhere.example/fallback-choice"]
    assert out["closedNav"] == [] and out["closedNavAfter"] == []                # closed before ready: nothing, no throw
    assert out["empty"] == [["native-open", None, None, None], ["native-open", "", None, None],
                            ["native-open", "   ", None, None]]                   # empty URLs: native, untouched
    assert out["named"] == [["native-open", "https://s.example/named", "results", None]]
    assert out["noopener"] == [["native-open", "https://s.example/nop", "_blank", "noopener"]]
    assert out["script"] == [["native-open", "javascript:alert(1)", None, None]]


def test_the_page_hook_is_native_when_ferb_isnt_driving_the_tab() -> None:
    out = _hook("unarmed")
    assert out["unseenClick"] == [["native-click", "blob:https://s.example/1", False]]
    assert out["unseenDispatch"] == [] and out["area"] == [["native-click", "data:text/plain,x", False]]
    assert out["open"] == [["native-open", "/file.zip", None, None]] and out["relayed"] == []
    assert out["fallbackNav"] == []                                               # the browser loaded it, not the hook


def test_the_page_hook_is_pinned_and_installed_and_removed_with_the_tabs_arming() -> None:
    proto = EXT / "protocol.js"
    out = json.loads(subprocess.run([_node(), "-e", f"""
const p = require({json.dumps(str(proto))});
console.log(JSON.stringify([
  p.cdpAllowed("Page.addScriptToEvaluateOnNewDocument", {{source: p.PAGE_HOOK, runImmediately: true}}),
  p.cdpAllowed("Page.addScriptToEvaluateOnNewDocument", {{source: p.PAGE_HOOK + " ", runImmediately: true}}),
  p.cdpAllowed("Page.addScriptToEvaluateOnNewDocument", {{source: p.PAGE_HOOK}}),
  p.cdpAllowed("Page.removeScriptToEvaluateOnNewDocument", {{identifier: "1"}}),
  p.cdpAllowed("Runtime.evaluate", {{expression: p.PAGE_HOOK}}),
  /Function\\.prototype\\.toString|\\.toString\\s*=/.test(p.PAGE_HOOK)]));
"""], capture_output=True, text=True, timeout=20).stdout)
    assert out == [True, False, False, True, False, False]                 # exact source; no masking
    calls = worker('''(async () => {
      const cmds = [];
      port = {postMessage() {}}; send = async () => {};
      chrome.debugger = new Proxy({attach: async () => {}, sendCommand: async (t, m, p) => { cmds.push(m);
          return m === 'Page.addScriptToEvaluateOnNewDocument' ? {identifier: 'h1'} : {}; },
        detach: async () => { cmds.push('detach'); }, onEvent: chrome.debugger.onEvent, onDetach: chrome.debugger.onDetach},
        {get: (o, k) => o[k]});
      await arm(12);
      const armed = cmds.splice(0);
      await detach(12);
      return {armed, released: cmds};
    })()''')
    assert "Page.addScriptToEvaluateOnNewDocument" in calls["armed"]
    assert calls["released"].index("Page.removeScriptToEvaluateOnNewDocument") < calls["released"].index("detach")


def test_a_claimed_popup_is_never_navigated_by_ferb_its_opener_page_is_told_when_it_is_the_tasks() -> None:
    out = worker('''(async () => {
      const sent = [], told = [], loads = [];
      port = {postMessage() {}}; send = async (type, body) => { sent.push(body); };
      arm = async (id) => { const t = tabState(id); t.attached = true; t.armed = true; return t; };
      load = async (id, url) => { loads.push([id, url]); };
      chrome.tabs.sendMessage = async (tab, m, o) => { told.push([tab, m, o]); return true; };   // content.js answers
      cdp = async (tab, method) => method === 'Target.getTargetInfo' ? {targetInfo: {targetId: 'T' + tab, openerId: tab === 31 ? 'T5' : undefined}} : {};
      tabState(5).armed = true;
      pageHookPopup(5, 2, {token: '%s', url: 'https://s.example/f.zip'});
      tabCreated({id: 31}); tabUpdated(31, 'about:blank#ferb-%s');
      await sleep(10);
      const ready = await commands.popup_ready({tab_id: 31});
      const again = await commands.popup_ready({tab_id: 31});
      return {popup: sent.find(b => b.event === 'popup'), told, loads, ready, again};
    })()''' % (TOKEN, TOKEN))
    assert out["popup"]["opener_tab_id"] == 5 and out["popup"]["tab_id"] == 31 and "load_url" not in out["popup"]
    assert out["told"] == [[5, {"ferbPopupReady": TOKEN}, {"frameId": 2}]] and out["loads"] == []
    assert out["ready"] == {"ready": True, "told": True} and out["again"] == {"ready": False}


def test_in_ferbs_own_profile_nothing_held_is_handed_to_the_browser() -> None:
    """Every download in FERB's own profile is FERB's: what it can't take is dropped or failed, never given to the
    browser's flow (which could open a dialog)."""
    out = worker('''(async () => {
      const scripted = [];
      profileIdentity = 'ferb'; port = null;
      chrome.scripting = {executeScript: async (o) => { scripted.push(o.args); return []; }};
      const unarmed = pageDownload(9, 0, {id: 'x', name: 'n'});
      tabState(9).armed = true; port = {postMessage() {}}; send = async () => {};
      await armPage(9, false);
      return {unarmed, scripted};
    })()''')
    assert out["unarmed"] == "drop" and [a[:2] for a in out["scripted"]] == [[False, True]]   # disarm, drop=true


# ---------- final wave (D35) ----------

def test_a_link_to_a_new_tab_waits_briefly_for_the_daemon_then_goes_to_the_browser() -> None:
    """A stale-armed tab (the daemon restarting, or not holding the tab after a restart): the owner's click is never
    lost. The extension waits about a second for the daemon's answer (open_seen); without one, content.js opens the
    link natively, and a late answer can no longer make FERB open it too."""
    out = worker('''(async () => {
      const sent = [];
      port = {postMessage() {}}; send = async (type, body) => { sent.push(body); };
      chrome.tabs.get = async id => ({id, windowId: 9});
      tabState(5).armed = true;
      const answered = pageOpen(5, {url: 'https://s.example/a'});
      await sleep(20);
      const first = sent.filter(b => b.event === 'open')[0];
      const seen = await commands.open_seen({open_id: first.open_id});
      const a = await answered;
      const t0 = Date.now();
      const silent = await pageOpen(5, {url: 'https://s.example/b'});
      const waited = Date.now() - t0;
      const second = sent.filter(b => b.event === 'open')[1];
      const late = await commands.open_seen({open_id: second.open_id});
      const notArmed = await pageOpen(6, {url: 'https://s.example/c'});
      return {a, seen, silent, waited, late, notArmed, ids: [first.open_id, second.open_id], urls: [first.url, second.url]};
    })()''')
    assert out["a"] is True and out["seen"] == {"seen": True}
    assert out["silent"] is False and 900 <= out["waited"] <= 2000 and out["late"] == {"expired": True}
    assert out["notArmed"] is False and out["urls"] == ["https://s.example/a", "https://s.example/b"]
    assert len(set(out["ids"])) == 2 and all(isinstance(i, str) and 0 < len(i) <= 64 for i in out["ids"])


def test_relayed_tokens_are_bounded_per_opener_and_in_all() -> None:
    out = worker('''(async () => {
      port = {postMessage() {}}; send = async () => {};
      for (let id = 1; id <= 12; id++) tabState(id).armed = true;
      const tok = (n) => n.toString(16).padStart(32, '0');
      for (let i = 0; i < 50; i++) pageHookPopup(1, 0, {token: tok(i), url: 'https://s.example/' + i});
      const perOpener = [...relayedTokens.values()].filter(r => r.opener === 1).length;
      for (let t = 2; t <= 12; t++) for (let i = 0; i < 20; i++) pageHookPopup(t, 0, {token: tok(1000 * t + i), url: 'https://s.example/'});
      const t0 = Date.now();
      for (let i = 0; i < 20000; i++) pageHookPopup(2 + (i % 11), 0, {token: tok(100000 + i), url: 'https://s.example/'});
      return {perOpener, total: relayedTokens.size, ms: Date.now() - t0};
    })()''')
    assert out["perOpener"] == 20 and out["total"] == 100
    assert out["ms"] < 5000                                    # bounded work per relay, not a prune over 20k tokens


def test_a_popup_whose_opener_does_not_match_is_let_go_even_when_focus_emulation_cannot_be_undone() -> None:
    """Nothing was enabled on a tab whose opener the browser didn't confirm, so FERB detaches directly; a failing
    Emulation.setFocusEmulationEnabled (the tab never had it) can't keep the debugger attached."""
    out = worker('''(async () => {
      const detached = [];
      port = {postMessage() {}}; send = async () => {};
      cdp = async (tab, method) => method === 'Target.getTargetInfo'
        ? {targetInfo: {targetId: 'T' + tab, openerId: tab === 31 ? 'T200' : undefined}} : {};
      chrome.debugger = new Proxy({attach: async () => {}, detach: async ({tabId}) => { detached.push(tabId); },
        sendCommand: async () => { throw Error('Emulation is not enabled'); },
        onEvent: chrome.debugger.onEvent, onDetach: chrome.debugger.onDetach}, {get: (o, k) => o[k]});
      tabState(5).armed = true;
      pageHookPopup(5, 0, {token: 'abababababababababababababababab', url: 'https://s.example/p'});
      tabCreated({id: 31}); tabUpdated(31, 'about:blank#ferb-abababababababababababababababab');
      await sleep(30);
      return {detached, kept: TABS.has(31)};
    })()''')
    assert out["detached"] == [31] and out["kept"] is False


def test_every_page_save_dropped_over_the_hold_caps_is_reported_never_silently() -> None:
    """Step 12e (protocol 20): the first drop of a document is reported at once, the rest of that minute together at
    its end with their count; the counts add up to every save dropped."""
    out = worker('''(async () => {
      const sent = [], timers = []; let now = 1000000;
      Date.now = () => now;
      setTimeout = (fn, ms) => { if (ms !== DOWNLOAD_IDLE_MS) timers.push(fn); return timers.length + 1; };   // drop facts
      clearTimeout = () => {};
      port = {postMessage() {}}; send = async (type, body) => { sent.push(body); };
      tabState(3).armed = true;
      for (let i = 0; i < 25; i++) pageDownload(3, 0, {id: 'h' + i, name: 'x.csv', size: 1}, 'doc-A');   // 5 dropped
      pageDropped(3, 0, 'doc-A', 'from-content.csv', 3);         // content.js's own cap: 3 more, the same document
      const atOnce = sent.filter(b => b.uncaptured).length;
      pageDropped(3, 0, 'doc-B', 'y.csv');                      // another document in the tab: its own fact
      now += 60000;
      for (const fn of timers.splice(0)) fn();                  // the minute ends: doc-A's waiting count is told
      now += 1000;
      pageDownload(3, 0, {id: 'late', name: 'x.csv', size: 1}, 'doc-A');   // within a minute of that fact: waits
      now += 60000;
      for (const fn of timers.splice(0)) fn();
      tabState(4);                                              // a tab FERB doesn't drive: nobody to tell
      pageDropped(4, 0, 'doc-C', 'z.csv');
      return {atOnce, facts: sent.filter(b => b.uncaptured).map(b => [b.tab_id, b.detail, b.name, b.count])};
    })()''')
    assert out["atOnce"] == 1
    assert out["facts"] == [[3, "dropped", "x.csv", 1], [3, "dropped", "y.csv", 1], [3, "dropped", "x.csv", 7],
                            [3, "dropped", "x.csv", 1]]
    assert sum(f[3] for f in out["facts"] if f[0] == 3 and f[2] != "y.csv") == 5 + 3 + 1


def test_a_popup_past_the_tab_cap_is_closed_while_blank_and_only_a_claimed_one() -> None:
    out = worker('''(async () => {
      const removed = [];
      chrome.tabs.remove = async (id) => { removed.push(id); };
      const t = tabState(31); t.popupFor = {opener: 5, frameId: 0, token: 'x'};
      tabState(32);                                             // not a popup this worker claimed
      const refused = await commands.popup_refused({tab_id: 31});
      const other = await commands.popup_refused({tab_id: 32});
      return {refused, other, removed, gone: !TABS.has(31)};
    })()''')
    assert out["refused"] == {"closed": True, "tab_id": 31} and out["removed"] == [31] and out["gone"] is True
    assert out["other"]["closed"] is False


def test_a_detach_during_a_claim_the_daemon_never_heard_of_sends_nothing() -> None:
    out = worker('''(async () => {
      const sent = [];
      port = {postMessage() {}}; send = async (type, body) => { sent.push(body); };
      const fire = (tabId, reason) => chrome.debugger.onDetach.fire({tabId}, reason);
      chrome.debugger = new Proxy({attach: async () => {}, detach: async () => {}, sendCommand: async () => ({}),
        onEvent: chrome.debugger.onEvent, onDetach: chrome.debugger.onDetach}, {get: (o, k) => o[k]});
      let release; const asked = new Promise(r => { release = r; });
      cdp = async (tab, method) => { if (method === 'Target.getTargetInfo') { await asked; return {targetInfo: {targetId: 'T' + tab, openerId: 'T5'}}; } return {}; };
      tabState(5).armed = true;
      pageHookPopup(5, 0, {token: 'cdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcd', url: 'https://s.example/p'});
      tabCreated({id: 41}); tabUpdated(41, 'about:blank#ferb-cdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcd');
      await sleep(10);
      fire(41, 'canceled_by_user');                             // the owner cancelled the bar while FERB was asking
      const t = tabState(7); t.attached = true;                 // a tab the daemon drives
      fire(7, 'canceled_by_user');
      release(); await sleep(20);
      return sent.filter(b => b.event === 'detached').map(b => b.tab_id);
    })()''')
    assert out == [7]


@pytest.mark.parametrize("mode", ["orphaned", "invalidating"])
def test_an_orphaned_page_listener_never_eats_the_owners_click(mode: str) -> None:
    """After an extension reload (`ferb setup --update`) the old content.js in a still-armed tab can't reach the
    service worker. A click it would have taken goes to the browser exactly once: not taken at all when the runtime is
    already gone, or handed back (re-dispatched) when sending fails after it was taken; then it stops taking clicks."""
    out = _content(mode)
    taken = {k for k, v in out["clicks"].items() if v}
    passed = {p["href"]: p for p in out["passed"]}
    if mode == "orphaned":
        assert taken == set() and passed == {}                    # nothing prevented: the browser has every click
    else:
        assert taken == {"blob", "blobStopped", "data", "blank"}  # the first new-tab send failed: no more taken after it
        assert passed["https://s.example/next"]["target"] == "_blank"
        assert passed["data:text/csv;base64,YQ=="]["download"] == "d.csv"
        assert passed["blob:https://s.example/again"]["download"] in ("r.csv", "s.csv")
        assert len(out["passed"]) == 4                            # each taken click reached the browser once
    assert out["armedAnswer"] is False                            # the page hook behaves natively from then on


def test_the_extension_harness_returns_a_large_answer_whole():
    """Node writes to a pipe asynchronously on macOS: exiting right after console.log cut answers short (seen as
    JSONDecodeError on ~500-byte answers under load; deterministic past the pipe buffer)."""
    out = worker('(async () => ({x: "y".repeat(1 << 20)}))()')
    assert out == {"x": "y" * (1 << 20)}
