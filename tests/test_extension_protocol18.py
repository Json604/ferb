"""Protocol 18 in the extension's own code (background.js, content.js, pointer.js, protocol.js), run under node: a
`paired` only while pairing, read errors known failures, masked tab URLs, tab groups and records, secret fields (cards
too) refusing text keys, form-submit facts, closed-shadow focus, the pointer's disabled check, no fixed page markers."""

import json
import subprocess
from typing import Any

from tests.chromelab import EXT, _node, worker
from tests.test_shown_password_fields import extension


# decision: HAND-222
def test_a_paired_message_is_taken_only_while_this_profiles_pairing_is_pending() -> None:
    out = worker('''(async () => {
      port = {postMessage() {}};
      const KEY = btoa(String.fromCharCode(...new Uint8Array(32).fill(9)));
      await onDaemon({type: "paired", key: KEY, gen: 1});            // nobody started a pairing here
      const ignored = (await install()).key;
      pairingPending = Date.now();
      await onDaemon({type: "paired", key: KEY, gen: 1});
      return {ignored, taken: (await install()).key === KEY, pending: pairingPending};
    })()''')
    assert out == {"ignored": None, "taken": True, "pending": None}


# decision: HAND-224
def test_a_failed_read_is_a_known_failure_and_a_failed_act_stays_unknown() -> None:
    out = worker('''(async () => {
      gen = 1; const sent = []; send = async (type, body) => { sent.push(body); };
      commands.snapshot = async () => { throw new Error("frame removed"); };
      commands.dialog = async () => { throw new Error("gone"); };
      commands.act = async () => { throw new Error("renderer went away"); };
      await onCommand({gen: 1, id: "r1", op: "snapshot", args: {tab_id: 1}});
      await onCommand({gen: 1, id: "r2", op: "dialog", args: {tab_id: 1}});
      await onCommand({gen: 1, id: "r3", op: "dialog", args: {tab_id: 1, decision: "accept"}});
      await onCommand({gen: 1, id: "a1", op: "act", args: {tab_id: 1}});
      return sent.map((b) => [b.id, b.outcome]);
    })()''')
    assert out == [["r1", "failed"], ["r2", "failed"], ["r3", "unknown"], ["a1", "unknown"]]


def test_list_tabs_and_the_pending_dialog_snapshot_mask_urls() -> None:
    out = worker('''(async () => {
      chrome.tabs.query = async () => [{id: 3, windowId: 5, url: "https://u:p@x.example/a?token=SECRET&q=1#k=v", title: "A"}];
      chrome.tabs.get = async () => ({id: 3, url: "https://x.example/cb?code=SECRET"});
      const listed = await commands.list_tabs();
      tabState(3).pendingDialog = {type: "alert"};
      const snap = await commands.snapshot({tab_id: 3, snap: 1});
      return {listed: listed.tabs[0].url, dialog: snap.frames[0].url};
    })()''')
    assert "SECRET" not in json.dumps(out) and "u:p@" not in out["listed"]
    assert out["listed"].startswith("https://x.example/a?token=") and "q=1" in out["listed"]


GROUPS = '''
      const calls = [], sent = [];
      port = {postMessage() {}}; gen = 1; send = async (type, body) => { sent.push([type, body]); };
      let nextGroup = 500; const groups = {}, tabs = {};
      chrome.windows = {get: async (id) => ({id, type: "normal", state: "normal"})};
      chrome.tabs.create = async ({windowId}) => { const id = 100 + Object.keys(tabs).length; tabs[id] = {id, windowId, groupId: -1}; return tabs[id]; };
      chrome.tabs.get = async (id) => { if (!tabs[id]) throw new Error("No tab with id: " + id); return {...tabs[id], url: "https://s.example/"}; };
      chrome.tabs.update = async () => {};
      chrome.tabs.remove = async (id) => { calls.push(["remove", id]); delete tabs[id]; };
      chrome.tabs.group = async ({groupId, tabIds, createProperties}) => {
        const gid = groupId ?? ++nextGroup;
        if (!groups[gid]) groups[gid] = {id: gid, windowId: createProperties.windowId, collapsed: false, title: ""};
        for (const t of tabIds) tabs[t].groupId = gid;
        calls.push(["group", gid, tabIds]); return gid; };
      chrome.tabs.ungroup = async (ids) => { for (const t of [].concat(ids)) tabs[t].groupId = -1; calls.push(["ungroup", ids]); };
      chrome.tabs.query = async ({groupId}) => Object.values(tabs).filter((t) => groupId === undefined || t.groupId === groupId);
      chrome.tabGroups = {get: async (gid) => { if (!groups[gid]) throw new Error("no group"); return groups[gid]; },
                          update: async (gid, p) => { Object.assign(groups[gid], p); calls.push(["update", gid, p]); }};
      arm = async () => {}; load = async () => {}; detach = async () => { calls.push(["detach"]); };
      const g = {root: "task-1", task: "task-1.w", title: "FERB · Book a table"};
'''


# decision: HAND-201
def test_a_tasks_tabs_join_one_collapsed_group_the_result_leaves_it_and_others_close() -> None:
    out = worker('''(async () => {''' + GROUPS + '''
      const a = await commands.open({url: "https://s.example/a", window_id: 7, group: g}, "c-a");
      const b = await commands.open({url: "https://s.example/b", group: g}, "c-b");   // joins the group's window
      const sameGroup = a.group_id === b.group_id && tabs[b.tab_id].windowId === 7;
      const made = groups[a.group_id];
      tabs[300] = {id: 300, windowId: 7, groupId: a.group_id};                    // the owner drags a tab in
      const kept = await commands.release_tab({tab_id: a.tab_id, keep_open: true}); // the result: handed over
      const closed = await commands.close({tab_id: b.tab_id});
      const refused = await commands.close({tab_id: 300});                         // never one FERB didn't open
      return {sameGroup, title: made.title, collapsed: made.collapsed, color: made.color, kept: kept.kept_open,
              resultGrouped: tabs[a.tab_id].groupId, closed: closed.closed, refused: refused.reason,
              ownerTabLeftGroup: tabs[300].groupId, ownerTabOpen: !!tabs[300], records: await records()};
    })()''')
    assert out["sameGroup"] and out["title"] == "FERB · Book a table" and out["collapsed"] and out["color"] == "grey"
    assert out["kept"] and out["resultGrouped"] == -1 and out["closed"] is True
    assert out["refused"] == "not_ferbs" and out["ownerTabOpen"] and out["ownerTabLeftGroup"] == -1
    assert out["records"] == {"tabs": {}, "groups": {}}


# decision: HAND-201
def test_a_borrowed_tab_is_recorded_never_grouped_and_a_tab_dragged_out_is_the_owners() -> None:
    out = worker('''(async () => {''' + GROUPS + '''
      tabs[41] = {id: 41, windowId: 7, groupId: -1};
      await commands.adopt_tab({tab_id: 41, task: "task-1.w", root: "task-1", borrowed: true});
      const a = await commands.open({url: "https://s.example/a", window_id: 7, group: g}, "c-a");
      await sleep(1100);                                                        // FERB's own grouping settled
      const hello = await liveRecords();
      tabs[a.tab_id].groupId = -1; await tabMoved(a.tab_id, -1);                  // the owner drags it out
      return {borrowedGrouped: tabs[41].groupId, hello, a: a.tab_id, taken: sent.filter(([t, b]) => b.event === "taken"),
              after: await liveRecords(), calls: calls.filter((c) => c[0] === "remove")};
    })()''')
    assert out["borrowedGrouped"] == -1
    assert sorted((r["tab_id"], r["borrowed"]) for r in out["hello"]) == [(41, True), (out["a"], False)]
    assert [b["tab_id"] for _, b in out["taken"]] == [out["a"]] and out["calls"] == []
    assert [r["tab_id"] for r in out["after"]] == [41]


def test_a_new_browser_session_forgets_the_records_of_the_last() -> None:
    out = worker('''(async () => {
      await mutateRecords((r) => { r.tabs[5] = {task: "t", root: "t", cid: "c", group: 9};
                                   r.groups[9] = {root: "t", windowId: 1, title: "FERB · x"}; });
      chrome.tabs.get = async (id) => ({id, groupId: 9, url: "https://s.example/"});
      chrome.tabGroups = {get: async () => ({title: "renamed"})};               // ids reused by another session
      return await liveRecords();
    })()''')
    assert out == []


# decision: HAND-218
def test_a_text_key_into_a_secret_field_is_refused_by_trusted_input_and_tab_still_goes() -> None:
    out = worker('''(async () => {
      const pressed = [];
      attach = async () => tabState(1);
      topPoint = async () => ({x: 1, y: 1, secret: true});
      inFrame = async () => true;
      pressKey = async (tab, k) => { pressed.push(k); return true; };
      const r = [];
      for (const k of ["4", "shift+A", "Space", "cmd+v", "Tab", "Enter", "cmd+a"])
        r.push((await trusted(1, {kind: "press", text: k, frame_id: 0, snap: 1, el: 0})).detail);
      return {r, pressed};
    })()''')
    assert out["r"][:4] == ["secret_field"] * 4
    assert out["pressed"] == ["Tab", "Enter", "cmd+a"]


# decision: GATE-202
def test_an_act_that_can_submit_reports_the_pages_own_submit_events() -> None:
    out = worker('''(async () => {
      attach = async () => tabState(1);
      trusted = async () => ({ok: true, detail: "clicked (trusted)"});
      let round = 0;
      chrome.scripting = {executeScript: async () => (round++ === 0 ? [{frameId: 0, result: [{action_host: "old"}]}]
        : [{frameId: 0, result: [{action_host: "pay.example", method: "POST", kinds: ["card_number"]}]}])};
      return await commands.act({tab_id: 1, frame_id: 0, snap: 1, el: 0, kind: "click"});
    })()''')
    assert out["submitted"] == [{"action_host": "pay.example", "method": "POST", "kinds": ["card_number"], "frame_id": 0}]


# decision: GATE-206
def test_card_fields_are_secret_take_no_login_code_and_their_kinds_are_reported() -> None:
    shown = extension(r'''
class HTMLSelectElement {}
const sel = (ac) => Object.assign(new HTMLSelectElement(), {tagName: 'SELECT', value: '07', autocomplete: ac,
  selectedOptions: [{text: 'July'}], getAttribute: (k) => (k === 'autocomplete' ? ac : null), isContentEditable: false,
  getBoundingClientRect: () => ({x: 0, y: 0, width: 10, height: 10, left: 0, top: 0, right: 10, bottom: 10})});
const one = (attrs, text) => { const e = describe(input(Object.assign({value: '4111 1111 1111 1111'}, attrs), text), 1);
                               return {secret: e.secret, value: e.value, kind: e.field_kind || null, code: e.code_fill}; };
const month = describe(sel('cc-exp-month'), 1);
console.log(JSON.stringify({number: one({autocomplete: 'cc-number'}), csc: one({autocomplete: 'billing cc-csc'}),
  exp: one({autocomplete: 'cc-exp'}), name: one({autocomplete: 'cc-name'}, 'Name on card'),
  password: one({type: 'password'}), otp: one({autocomplete: 'one-time-code'}),
  month: {secret: month.secret, value: month.value, kind: month.field_kind}}));
''')
    assert shown["number"] == {"secret": True, "value": "filled", "kind": "card_number", "code": False}
    assert shown["csc"]["kind"] == "card_csc" and shown["exp"]["kind"] == "card_expiry" and not shown["csc"]["code"]
    assert shown["name"] == {"secret": False, "value": "4111 1111 1111 1111", "kind": None, "code": False}
    assert shown["password"]["kind"] == "password" and shown["otp"] == {"secret": True, "value": "filled",
                                                                         "kind": "one_time_code", "code": True}
    assert shown["month"] == {"secret": True, "value": "filled", "kind": "card_expiry"}
    assert "4111" not in json.dumps({k: v for k, v in shown.items() if k != "name"})


# decision: GATE-202
def test_a_submit_controls_facts_come_from_the_forms_own_markup() -> None:
    out = extension(r'''
const store = new WeakMap();
for (const k of ['action', 'method', 'elements'])
  Object.defineProperty(HTMLFormElement.prototype, k, {get() { return store.get(this)[k]; }});
const form = new HTMLFormElement();
form.action = 'clobbered';                                       // a field named "action" can't stand in
const btn = input({type: 'submit', tagName: 'INPUT', form, labelText: 'Pay ₹99'}, 'Pay ₹99');
btn.hasAttribute = () => false;
const card = input({autocomplete: 'cc-number', form}); const box = input({form, name: 'q'});
store.set(form, {action: 'https://pay.example/charge', method: 'post', elements: [card, box, btn]});
const button = Object.assign(input({form}), {tagName: 'BUTTON', getAttribute: (k) => (k === 'type' ? 'button' : null)});
console.log(JSON.stringify({click: submitInfo(btn), enter: submitInfo(box), none: submitInfo(button),
                            alone: submitInfo(input({}))}));
''')
    assert out["click"] == {"click": True, "enter": False, "action_host": "pay.example", "method": "POST",
                            "kinds": ["card_number"], "label": "Pay ₹99"}
    assert out["enter"]["enter"] is True and out["enter"]["click"] is False and out["enter"]["label"] == "Pay ₹99"
    assert out["none"] is None and out["alone"] is None


def _content_fn(*names: str) -> str:
    src = (EXT / "content.js").read_text()
    parts = [src[src.index("  const shadowOf = "):src.index("  function eachElement(")]]
    for name in names:
        start = src.index(f"  function {name}(")
        parts.append(src[start:src.index("\n  }\n", start) + 4])
    return "\n".join(parts)


def test_focus_inside_a_closed_shadow_root_is_found() -> None:
    script = r'''
const field = {tagName: 'INPUT'}; const host = {tagName: 'DIV'};
const closed = {activeElement: field};
const chrome = {dom: {openOrClosedShadowRoot: (el) => (el === host ? closed : null)}};
const document = {activeElement: host, body: {}};
const isSecret = (el) => el === field;
''' + _content_fn("deepActive", "focusedSecret") + r'''
console.log(JSON.stringify([deepActive() === field, focusedSecret()]));
'''
    ran = subprocess.run([_node(), "-e", script], capture_output=True, text=True, timeout=20)
    assert json.loads(ran.stdout) == [True, True], ran.stderr


def test_the_pointer_refuses_a_disabled_target_by_the_field_describe_emits() -> None:
    script = (EXT / "pointer.js").read_text() + r'''
async function resolveVisualPoint() { return {x: 1, y: 1, element: {disabled: true}}; }
checkPoint(1, {x: 1, y: 1, element: {disabled: true}}, {snap: 1}).then(() => console.log('"pressed"'),
  (e) => console.log(JSON.stringify(e.message)));
'''
    ran = subprocess.run([_node(), "-e", script], capture_output=True, text=True, timeout=20)
    assert json.loads(ran.stdout) == "pointer_target_unavailable", ran.stderr


def test_no_fixed_ferb_marker_reaches_a_page() -> None:
    script = f"""
const p = require({json.dumps(str(EXT / "protocol.js"))});
console.log(JSON.stringify({{hook: p.PAGE_HOOK, token: p.FERB_TOKEN, bind: p.BIND_ATTR, stamp: p.STAMP_ATTR,
  enters: ["a", "Enter", "cmd+v", "Tab"].map(p.entersText), reads: [...p.READ_OPS].sort()}}));
"""
    out: dict[str, Any] = json.loads(subprocess.run([_node(), "-e", script], capture_output=True, text=True,
                                                    timeout=20).stdout)
    assert "ferb-armed" not in out["hook"] and "ferb-popup" not in out["hook"] and out["token"] in out["hook"]
    assert out["bind"] == f"data-{out['token']}" and out["stamp"] == f"data-{out['token']}s"
    content = (EXT / "content.js").read_text()
    assert "data-ferb" not in content and "ferb-armed" not in content
    assert out["enters"] == [True, False, True, False]
    from ferb.act.browser.ext_protocol import READS
    assert out["reads"] == sorted(READS)
