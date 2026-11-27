//! Background first in browser tasks, the extension's half (HAND-532..537; extension 0.19.11): the extension's own
//! JavaScript (`extension`) under node with fake Chrome APIs and a fake clock (tests/extension/harness.js),
//! never a real browser. The page hook refuses what a page does to come forward while FERB drives its tab; FERB's
//! minimized window and a task's claimed popup window go back down when a page brings them forward; a window the owner
//! opened is left alone; the owner's own windows only become facts; each fact reaches the tab's next reply once.
//!
//! Live 2026-11-15 (task 0821c105…): an IIT Madras sign-in (FirebaseUI, signInFlow "popup") opened its Google sign-in
//! as a popup window that started blank, so FERB never claimed it, and the window came up over the owner's full-screen
//! Brave.

use std::path::PathBuf;
use std::process::Command;

use serde_json::{json, Value};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

/// node, from PATH or where Homebrew and the installer put it; `None` when this Mac has none.
fn node() -> Option<PathBuf> {
    let mut places: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).map(|d| d.join("node")).collect())
        .unwrap_or_default();
    places.push("/opt/homebrew/bin/node".into());
    places.push("/usr/local/bin/node".into());
    places.into_iter().find(|p| p.is_file())
}

/// Runs a scenario in the harness (`worker` or `hook`) and returns what it returned.
fn run(mode: &str, scenario: &str) -> Option<Value> {
    let Some(node) = node() else {
        eprintln!("SKIPPED: no node on this Mac, so the extension's JavaScript wasn't run");
        return None;
    };
    let out = Command::new(node)
        .arg(root().join("engine/crates/ferb-browser/tests/extension/harness.js"))
        .arg(root().join("extension"))
        .arg(mode)
        .arg(scenario)
        .output()
        .expect("node runs");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    Some(serde_json::from_slice(&out.stdout).expect("the scenario's JSON"))
}

/// FERB's minimized window 9 (its blank placeholder tab 90 active) holds the task tab 8, armed; the owner's window 3
/// holds their own tab 30. `command` runs one daemon command and returns its result body.
const WORLD: &str = r#"
  gen = 1; port = {postMessage() {}, disconnect() {}};
  const sent = [];
  send = async (type, body) => { sent.push([type, body]); };
  fake.windows.set(9, {state: "minimized", type: "normal", focused: false});
  fake.windows.set(3, {state: "normal", type: "normal", focused: false});
  fake.tabs.set(90, {id: 90, windowId: 9, url: "about:blank", active: true});
  fake.tabs.set(8, {id: 8, windowId: 9, url: "https://app.example/login", active: false});
  fake.tabs.set(30, {id: 30, windowId: 3, url: "https://owner.example/", active: true});
  await chrome.storage.session.set({s: {seen: {}, order: [], windowId: 9}});
  await arm(8);
  let n = 0;
  const command = async (op, args) => {
    const id = "c" + (++n);
    await onCommand({gen: 1, id, op, args, timeout_ms: 30000});
    return sent.find(([t, b]) => t === "result" && b.id === id)[1];
  };
  const events = (name) => sent.filter(([t, b]) => t === "event" && b.event === name).map(([, b]) => b);
  // Chrome shows a window for what a page did (BrowserView::Show): restored, raised, focused
  const shown = (id) => Object.assign(fake.windows.get(id), {state: "normal", focused: true});
  const newTab = (tab) => { fake.tabs.set(tab.id, {...tab}); chrome.tabs.onCreated.fire({...tab}); };
"#;

fn worker(scenario: &str) -> Option<Value> {
    run("worker", &format!("{WORLD}\n{scenario}"))
}

// decision: HAND-533
#[test]
fn ferbs_window_brought_forward_by_a_new_tab_goes_back_down_and_the_tab_is_told_once() {
    let Some(out) = worker(
        r#"
        noteInput(8);                                 // FERB clicked in tab 8 just now
        shown(9);
        newTab({id: 50, windowId: 9, openerTabId: 90, url: "https://app.example/next"});
        chrome.windows.onFocusChanged.fire(9);       // Chrome reports the focus while the scheduled looks run too
        await advance(3000);
        const owners = ownerShown.has(9);
        const first = await command("rest", {tab_id: 8, resting: false});
        const second = await command("rest", {tab_id: 8, resting: false});
        return {updates: fake.updates, state: fake.windows.get(9).state, owners, first: first.value.surfaced,
                second: second.value.surfaced ?? null};
    "#,
    ) else {
        return;
    };
    assert_eq!(out["updates"], json!([[9, {"state": "minimized"}]]));
    assert_eq!(out["state"], "minimized");
    assert_eq!(
        out["owners"], false,
        "FERB's own put-back is never taken for the owner's"
    );
    assert_eq!(
        out["first"],
        json!([{"window": "ferb", "cause": "new_tab", "focused": true, "put_back": true, "new_tab": 50}])
    );
    assert_eq!(out["second"], Value::Null, "told once");
}

// decision: HAND-533
#[test]
fn a_window_the_owner_opened_is_never_minimized_behind_their_back() {
    let Some(out) = worker(
        r#"
        // the owner opens FERB's window from the Dock: nothing of FERB's just before it
        shown(9);
        chrome.windows.onFocusChanged.fire(9);
        await advance(100);
        // then a page in it opens a tab while the owner watches: still theirs
        noteInput(8);
        newTab({id: 51, windowId: 9, openerTabId: 90, url: "https://app.example/more"});
        await advance(3000);
        const kept = fake.updates.length;
        // the owner minimizes it again; the next time a page brings it forward, FERB puts it back down
        Object.assign(fake.windows.get(9), {state: "minimized", focused: false});
        await advance(5000);
        noteInput(8);
        newTab({id: 52, windowId: 9, openerTabId: 90, url: "https://app.example/again"});   // a look while minimized
        await advance(200);
        shown(9);
        newTab({id: 53, windowId: 9, openerTabId: 90, url: "https://app.example/third"});
        await advance(200);
        // show_tab (the owner clicked FERB's notification) is the owner's too
        const before = fake.updates.length;
        await command("show_tab", {tab_id: 8});
        noteInput(8);
        newTab({id: 54, windowId: 9, openerTabId: 90, url: "https://app.example/fourth"});
        await advance(3000);
        return {kept, after: fake.updates.slice(0, before), shown_tab: fake.updates.slice(before)};
    "#,
    ) else {
        return;
    };
    assert_eq!(out["kept"], 0, "{out}");
    assert_eq!(out["after"], json!([[9, {"state": "minimized"}]]), "{out}");
    assert_eq!(
        out["shown_tab"],
        json!([[9, {"state": "normal", "focused": true}]]),
        "{out}"
    );
}

// decision: HAND-532
// decision: HAND-536
#[test]
fn a_sign_in_popup_window_that_starts_blank_is_claimed_and_goes_down_at_once() {
    let Some(out) = worker(
        r#"
        // FirebaseUI's popup: window.open("", name, "width=..,height=..") then its address; Chrome made window 20 a
        // popup window, shown and focused, with Chrome's opener tab the active one of FERB's window
        fake.targets[8] = {targetId: "T8"};
        fake.targets[21] = {targetId: "T21", openerId: "T8"};
        noteInput(8);
        fake.windows.set(20, {state: "normal", type: "popup", focused: true});
        newTab({id: 21, windowId: 20, openerTabId: 90, url: "about:blank"});
        await advance(100);
        // a blank tab in a normal window is never touched (FERB's own open makes those)
        noteInput(8);
        newTab({id: 22, windowId: 9, url: "about:blank"});
        await advance(100);
        const reply = await command("rest", {tab_id: 8, resting: false});
        return {popups: events("popup"), updates: fake.updates, attached: [...fake.attached].sort(),
                surfaced: reply.value.surfaced};
    "#,
    ) else {
        return;
    };
    assert_eq!(
        out["popups"],
        json!([{"event": "popup", "tab_id": 21, "opener_tab_id": 8, "url": "about:blank"}])
    );
    assert_eq!(out["updates"], json!([[20, {"state": "minimized"}]]));
    assert_eq!(out["attached"], json!([21, 8]), "tab 22 was never attached");
    assert_eq!(
        out["surfaced"],
        json!([{"window": "popup", "cause": "popup_window", "focused": true, "put_back": true, "new_tab": 21}])
    );
}

// decision: HAND-533
#[test]
fn the_owners_own_window_brought_forward_is_a_fact_only() {
    let Some(out) = worker(
        r#"
        // the task borrowed tab 30 in the owner's window; its page shows a dialog and Chrome focuses that window
        await arm(30);
        chrome.debugger.onEvent.fire({tabId: 30}, "Page.javascriptDialogOpening", {type: "alert", message: "m"});
        await advance(10);
        fake.windows.get(3).focused = true;
        chrome.windows.onFocusChanged.fire(3);
        await advance(100);
        const reply = await command("rest", {tab_id: 30, resting: false});
        return {updates: fake.updates, surfaced: reply.value.surfaced};
    "#,
    ) else {
        return;
    };
    assert_eq!(
        out["updates"],
        json!([]),
        "the owner's window is never minimized"
    );
    assert_eq!(
        out["surfaced"],
        json!([{"window": "owner", "cause": "dialog", "focused": true, "put_back": false}])
    );
}

// decision: HAND-533
#[test]
fn what_the_hook_refused_is_the_tabs_fact_and_a_tab_ferb_doesnt_drive_tells_nothing() {
    let Some(out) = worker(
        r#"
        const reply = () => {};
        chrome.runtime.onMessage.fire({ferbSurface: "focus"}, {tab: {id: 8}}, reply);
        chrome.runtime.onMessage.fire({ferbSurface: "fullscreen"}, {tab: {id: 8}}, reply);
        chrome.runtime.onMessage.fire({ferbSurface: "anything"}, {tab: {id: 8}}, reply);
        chrome.runtime.onMessage.fire({ferbSurface: "focus"}, {tab: {id: 30}}, reply);   // not armed: not FERB's
        const r = await command("rest", {tab_id: 8, resting: false});
        return {surfaced: r.value.surfaced, other: (TABS.get(30) || {}).surfaced ?? null};
    "#,
    ) else {
        return;
    };
    assert_eq!(
        out["surfaced"],
        json!([{"cause": "focus", "prevented": true}, {"cause": "fullscreen", "prevented": true}])
    );
    assert_eq!(out["other"], Value::Null);
}

// decision: HAND-533
#[test]
fn the_page_hook_refuses_coming_forward_only_while_ferb_drives_the_tab() {
    let Some(out) = run(
        "hook",
        r#"
        const el = new page.Element(), video = new page.Video();
        armed(true);
        window.focus(); self.focus();
        let full = "", pip = "";
        await el.requestFullscreen().catch((e) => { full = e.name; });
        const webkit = el.webkitRequestFullscreen();
        await video.requestPictureInPicture().catch((e) => { pip = e.name; });
        const whileDriven = {calls: [...page.calls], events: [...page.events], full, pip, webkit: webkit ?? null};
        page.calls.length = 0; page.events.length = 0;
        armed(false);
        window.focus();
        await el.requestFullscreen();
        el.webkitRequestFullscreen();
        await video.requestPictureInPicture();
        return {whileDriven, native: {calls: page.calls, events: page.events}};
    "#,
    ) else {
        return;
    };
    assert_eq!(
        out["whileDriven"],
        json!({"calls": [], "events": [["surface", "focus"], ["surface", "focus"], ["surface", "fullscreen"],
                                       ["surface", "fullscreen"], ["surface", "picture_in_picture"]],
               "full": "TypeError", "pip": "NotAllowedError", "webkit": null})
    );
    assert_eq!(
        out["native"],
        json!({"calls": ["focus", "requestFullscreen", "webkitRequestFullscreen", "requestPictureInPicture"],
               "events": []})
    );
}

// decision: HAND-534
#[test]
fn a_new_tab_without_an_opener_goes_the_way_a_links_new_tab_goes() {
    let Some(out) = run(
        "hook",
        r#"
        armed(true);
        const unlinked = window.open("/docs", "_blank", "noopener");
        const routed = {ret: unlinked, calls: [...page.calls], events: [...page.events]};
        page.calls.length = 0; page.events.length = 0;
        // with an opener (a sign-in popup needs it) the browser still opens it, through the hook's popup path
        const linked = window.open("https://accounts.example/signin", "_blank", "width=500,height=600");
        const opened = page.calls[0];
        page.calls.length = 0; page.events.length = 0;
        armed(false);
        window.open("/docs", "_blank", "noopener");
        return {routed, linked: linked !== null, opened, native: page.calls};
    "#,
    ) else {
        return;
    };
    assert_eq!(
        out["routed"],
        json!({"ret": null, "calls": [], "events": [["open", "https://site.example/docs"]]})
    );
    assert_eq!(out["linked"], true);
    let opened = out["opened"].as_array().unwrap();
    assert_eq!(opened[0], "open");
    assert!(
        opened[1].as_str().unwrap().starts_with("about:blank#ferb-"),
        "{opened:?}"
    );
    assert_eq!(opened[3], "width=500,height=600");
    assert_eq!(
        out["native"],
        json!([["open", "/docs", "_blank", "noopener"]])
    );
}

// decision: HAND-537
#[test]
fn extension_0_19_11_and_later_keep_protocol_21_and_the_minimum() {
    let manifest: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(root().join("extension/manifest.json")).unwrap(),
    )
    .unwrap();
    // 0.19.11 shipped this half; 0.19.12 (HAND-566) keeps it
    let version = manifest["version"].as_str().unwrap();
    assert!(
        ferb_browser::protocol::version_ok(version, "0.19.11").unwrap(),
        "{version}"
    );
    let protocol = std::fs::read_to_string(root().join("extension/protocol.js")).unwrap();
    assert!(protocol.contains(&format!(
        "const FERB_PROTOCOL = {};",
        ferb_browser::protocol::PROTOCOL_VERSION
    )));
    assert_eq!(ferb_browser::protocol::MIN_EXTENSION_VERSION, "0.19.0");
}

/// FERB's window in a profile with none (HAND-649, extension 0.19.13): no FERB window yet; `open` runs one open
/// command (letting the extension's waits run on the fake clock) and returns its result body.
const NO_WINDOW: &str = r#"
  gen = 1; port = {postMessage() {}, disconnect() {}};
  const sent = [];
  send = async (type, body) => { sent.push([type, body]); };
  let n = 0;
  const open = async (args) => {
    const id = "c" + (++n);
    const done = onCommand({gen: 1, id, op: "open", args: {url: "about:blank", ...args}, timeout_ms: 30000});
    await advance(10000);
    await done;
    return sent.find(([t, b]) => t === "result" && b.id === id)[1].value;
  };
  const ferbId = async () => (await chrome.storage.session.get("s")).s.windowId;
"#;

fn no_window(scenario: &str) -> Option<Value> {
    run("worker", &format!("{NO_WINDOW}\n{scenario}"))
}

// Live 2026-11-21 07:50:37 and 07:57:22: the owner's own Chrome window was full screen, macOS opened FERB's new
// window full screen on a Space of its own, and the minimize only took it out of full screen (Dock: no minimize).
// decision: HAND-649
#[test]
fn ferbs_new_window_that_macos_made_full_screen_goes_back_to_normal_then_ends_minimized() {
    let Some(out) = no_window(
        r#"
        fake.createState = "fullscreen";
        // the browser can't minimize a full-screen window: asked to, it only leaves full screen
        fake.updateHook = (id, p, w) => {
          if (p.state === "minimized" && w.state === "fullscreen") w.state = "normal"; else Object.assign(w, p);
        };
        const r = await open({allow_foreground: true});
        return {made: fake.made, updates: fake.updates, state: fake.windows.get(500).state, window: r.window_id,
                window_made: r.window_made ?? null, ferb: await ferbId()};
    "#,
    ) else {
        return;
    };
    assert_eq!(
        out["made"],
        json!([{"url": "about:blank#ferb-window", "state": "minimized", "focused": false}])
    );
    assert_eq!(
        out["updates"],
        json!([[500, {"state": "normal"}], [500, {"state": "minimized"}]])
    );
    assert_eq!(out["state"], "minimized");
    assert_eq!(out["window"], 500);
    assert_eq!(out["ferb"], 500);
    assert_eq!(
        out["window_made"],
        json!({"fullscreen": true, "minimized": true, "tries": 1})
    );
}

// decision: HAND-649
#[test]
fn a_minimize_that_doesnt_take_is_tried_once_more_then_reported() {
    let Some(out) = no_window(
        r#"
        fake.createState = "normal";                     // create() may report "normal" (probe E5)
        fake.updateHook = (id, p, w) => { if (p.state !== "minimized") Object.assign(w, p); };
        const r = await open({allow_foreground: true});
        return {updates: fake.updates, window_made: r.window_made ?? null, opened: r.tab_id !== undefined};
    "#,
    ) else {
        return;
    };
    assert_eq!(
        out["updates"],
        json!([[500, {"state": "minimized"}], [500, {"state": "minimized"}]])
    );
    assert_eq!(
        out["window_made"],
        json!({"fullscreen": false, "minimized": false, "tries": 2})
    );
    assert_eq!(out["opened"], true, "the tab still opens");
}

// decision: HAND-649
#[test]
fn a_window_made_minimized_says_nothing_more() {
    let Some(out) = no_window(
        r#"
        const r = await open({allow_foreground: true});
        return {updates: fake.updates, window_made: r.window_made ?? null, window: r.window_id};
    "#,
    ) else {
        return;
    };
    assert_eq!(out["updates"], json!([]));
    assert_eq!(out["window_made"], Value::Null);
    assert_eq!(out["window"], 500);
}

// After a browser restart its session storage (FERB's window id) is empty; a window it restored is FERB's by its
// placeholder's address, and is used as it is: no new window, nothing changed on it.
// decision: HAND-649
#[test]
fn ferbs_window_the_browser_restored_is_found_by_its_placeholder_and_reused() {
    let Some(out) = no_window(
        r#"
        fake.windows.set(7, {state: "minimized", type: "normal", focused: false});
        fake.tabs.set(70, {id: 70, windowId: 7, url: "about:blank#ferb-window", active: true});
        fake.windows.set(3, {state: "fullscreen", type: "normal", focused: true});   // the owner's
        fake.tabs.set(30, {id: 30, windowId: 3, url: "about:blank", active: true});
        const first = await open({});                   // as the daemon's first try: no foreground allowed
        const second = await open({});
        return {first: first.window_id, second: second.window_id, made: fake.made.length, updates: fake.updates,
                ferb: await ferbId(), window_made: first.window_made ?? null};
    "#,
    ) else {
        return;
    };
    assert_eq!(out["first"], 7, "{out}");
    assert_eq!(out["second"], 7);
    assert_eq!(out["made"], 0);
    assert_eq!(out["updates"], json!([]));
    assert_eq!(out["ferb"], 7);
    assert_eq!(out["window_made"], Value::Null);
}

/// HAND-724 (extension 0.19.15): `close` with `owner_asked` closes one of the owner's own tabs (borrowed, kept or not
/// recorded at all), the worker having said his words ask for it; without it the old refusal, unchanged. Leaving the
/// page and its dialogs go as for FERB's own tabs (HAND-446..448). `state(id)` is what the extension still holds for
/// the tab.
const OWNER_TAB: &str = r#"
  const state = async (id) => ({open: fake.tabs.has(id), record: (await records()).tabs[id] ?? null,
                                held: TABS.has(id), kept: (await keptTabs()).includes(id)});
"#;

fn owner_tab(scenario: &str) -> Option<Value> {
    worker(&format!("{OWNER_TAB}\n{scenario}"))
}

// decision: HAND-724
#[test]
fn the_owners_unrecorded_tab_closes_only_when_his_words_asked() {
    let Some(out) = owner_tab(
        r#"
        const refused = (await command("close", {tab_id: 30})).value;
        const after = await state(30);
        const closed = (await command("close", {tab_id: 30, owner_asked: true})).value;
        return {refused, after, closed, now: await state(30)};
    "#,
    ) else {
        return;
    };
    assert_eq!(out["refused"]["reason"], "not_ferbs", "{out}");
    assert_eq!(out["refused"]["closed"], false);
    assert_eq!(out["after"]["open"], true);
    assert_eq!(out["closed"], json!({"closed": true, "owner_tab": true}));
    assert_eq!(out["now"]["open"], false);
}

// decision: HAND-724
#[test]
fn a_borrowed_tab_ferb_drives_is_let_go_of_and_closed() {
    let Some(out) = owner_tab(
        r#"
        await command("adopt_tab", {tab_id: 30, task: "task-a", root: "task-a", borrowed: true});
        await arm(30);
        const before = await state(30);
        const refused = (await command("close", {tab_id: 30})).value;
        const closed = (await command("close", {tab_id: 30, owner_asked: true, leave_page: "leave"})).value;
        return {before, refused, closed, now: await state(30),
                detached: sent.filter(([t, b]) => t === "event" && b.event === "detached").length};
    "#,
    ) else {
        return;
    };
    assert_eq!(out["before"]["held"], true, "{out}");
    assert_eq!(out["before"]["record"]["borrowed"], true);
    assert_eq!(out["refused"]["reason"], "not_ferbs");
    assert_eq!(out["closed"], json!({"closed": true, "owner_tab": true}));
    assert_eq!(
        out["now"],
        json!({"open": false, "record": null, "held": false, "kept": false})
    );
    assert_eq!(
        out["detached"], 0,
        "FERB's own close is never reported as a lost session"
    );
}

// decision: HAND-724
// decision: HAND-446
#[test]
fn his_tab_whose_page_asks_stays_open_when_the_worker_said_stay() {
    let Some(out) = owner_tab(
        r#"
        await command("adopt_tab", {tab_id: 30, task: "task-a", root: "task-a", borrowed: true});
        await arm(30);
        chrome.tabs.remove = () => new Promise(() => {});     // the page's "Leave site?" holds the close
        const done = onCommand({gen: 1, id: "s1", op: "close", args: {tab_id: 30, owner_asked: true, leave_page: "stay"},
                                timeout_ms: 30000});
        await settle();
        chrome.debugger.onEvent.fire({tabId: 30}, "Page.javascriptDialogOpening", {type: "beforeunload", message: ""});
        await settle();
        await done;
        const reply = sent.find(([t, b]) => t === "result" && b.id === "s1")[1].value;
        return {reply, now: await state(30),
                answered: fake.cdp.filter(([id, m]) => id === 30 && m === "Page.handleJavaScriptDialog").length};
    "#,
    ) else {
        return;
    };
    assert_eq!(out["reply"]["reason"], "stayed", "{out}");
    assert_eq!(out["reply"]["closed"], false);
    assert_eq!(out["answered"], 1, "dismissed by the debugger at once");
    assert_eq!(out["now"]["open"], true);
    assert_eq!(
        out["now"]["record"]["borrowed"], true,
        "still the task's, held as before"
    );
}

// decision: HAND-724
#[test]
fn a_kept_tab_is_closed_and_no_longer_kept_when_his_words_asked() {
    let Some(out) = owner_tab(
        r#"
        await rememberKeptTab(30);
        const released = (await command("close", {tab_id: 30})).value;
        const after = await state(30);
        const closed = (await command("close", {tab_id: 30, owner_asked: true})).value;
        return {released, after, closed, now: await state(30)};
    "#,
    ) else {
        return;
    };
    assert_eq!(out["released"]["closed"], false, "{out}");
    assert_eq!(out["released"]["kept_open"], true);
    assert_eq!(out["after"]["open"], true);
    assert_eq!(out["closed"], json!({"closed": true, "owner_tab": true}));
    assert_eq!(out["now"]["open"], false);
    assert_eq!(out["now"]["kept"], false);
}
