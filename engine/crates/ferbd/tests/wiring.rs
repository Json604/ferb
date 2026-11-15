//! What FERB.app reads of the daemon through seams the recorded scenarios never exercised, through the real program
//! (the audit of 2026-11-14, items 4, 5, 8 and 12): the menu bar's paused state, the banners a connected client shows
//! instead, the Mac scan started at `chat.attach`, and the owner told once about data set aside at start.

mod support;

use std::time::{Duration, Instant};

use serde_json::{json, Value};
use support::*;

fn call(run: &Run, method: &str, params: Value) -> Value {
    let mut c = Raw::connect(&run.socket()).unwrap();
    c.hello("cli").unwrap();
    c.send(&json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}).to_string());
    let v: Value = serde_json::from_str(&c.recv().expect("an answer")).unwrap();
    assert!(v.get("error").is_none(), "{method}: {v}");
    v["result"].clone()
}

fn banners(run: &Run) -> Vec<String> {
    run.control(json!({"do": "calls"}))
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|c| c["osascript"].as_str().map(str::to_string))
        .collect()
}

/// chat.py:116 (L34 D): `chat.status.paused` is sensing's pause, so the menu bar offers Resume after Pause FERB.
#[test]
fn chat_status_says_whether_sensing_is_paused() {
    let tmp = root();
    let mut run = Run::start(tmp.path(), "01", json!({}));
    assert_eq!(call(&run, "chat.status", json!({}))["paused"], false);
    call(&run, "sense.pause", json!({}));
    assert_eq!(call(&run, "chat.status", json!({}))["paused"], true);
    call(&run, "sense.resume", json!({}));
    assert_eq!(call(&run, "chat.status", json!({}))["paused"], false);
    run.stop();
}

/// notify.py:166-170, 249 (L9): a notice is a banner only while no connected client's `logs.subscribe` receives
/// `attention.notice`; an open stream (the notch's, no filter) shows it instead, and once it closes banners are back.
/// A stream filtered to other topics shows nothing.
#[test]
fn a_connected_client_shows_notices_instead_of_a_banner() {
    let tmp = root();
    let mut run = Run::start(tmp.path(), "02", json!({"record_banners": true}));
    banners(&run); // whatever the start posted
    run.control(json!({"do": "notice", "ident": "n1", "text": "first"}));
    let first = banners(&run);
    assert_eq!(first.len(), 1, "{first:?}");
    assert!(first[0].contains("FERB noticed"), "{first:?}");

    // the notch's stream: every topic
    let mut notch = Raw::connect(&run.socket()).unwrap();
    notch.hello("cli").unwrap();
    notch.send(r#"{"jsonrpc": "2.0", "id": 7, "method": "logs.subscribe", "params": {}}"#);
    std::thread::sleep(Duration::from_millis(300));
    run.control(json!({"do": "notice", "ident": "n2", "text": "second"}));
    assert!(
        banners(&run).is_empty(),
        "no banner while the notch shows it"
    );

    // a stream for timers only doesn't show notices
    drop(notch);
    std::thread::sleep(Duration::from_millis(300));
    let mut timers = Raw::connect(&run.socket()).unwrap();
    timers.hello("cli").unwrap();
    timers.send(
        r#"{"jsonrpc": "2.0", "id": 8, "method": "logs.subscribe", "params": {"filters": ["timers"]}}"#,
    );
    std::thread::sleep(Duration::from_millis(300));
    run.control(json!({"do": "notice", "ident": "n3", "text": "third"}));
    assert_eq!(
        banners(&run).len(),
        1,
        "a timers-only stream shows no notice"
    );
    drop(timers);
    run.stop();
}

/// chat.py:165 (MEM-13): `chat.attach` starts what FERB sees on this Mac, once, whatever beat of the tour was skipped;
/// `chat.status.persona` reports it.
#[test]
fn chat_attach_starts_the_mac_scan() {
    let tmp = root();
    let mut run = Run::start(tmp.path(), "03", json!({"persona_scan": true}));
    assert_eq!(
        call(&run, "chat.status", json!({}))["persona"]["state"],
        "not_started"
    );
    call(&run, "chat.attach", json!({}));
    let until = Instant::now() + Duration::from_secs(10);
    let state = loop {
        let s = call(&run, "chat.status", json!({}))["persona"]["state"].clone();
        if s != "not_started" || Instant::now() > until {
            break s;
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    assert!(
        ["scanning", "seen", "failed"].contains(&state.as_str().unwrap_or("")),
        "the scan started: {state}"
    );
    run.stop();
}

/// main.py:456 (DMN-10): a file FERB couldn't read is set aside at start and the owner is told once, as a notice.
// decision: DMN-10
#[test]
fn data_set_aside_at_start_is_told_once() {
    let tmp = root();
    let (home, _) = Run::paths(tmp.path(), "04");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::write(home.join("config.toml"), "this is = = not toml [").unwrap();
    let mut run = Run::start(tmp.path(), "04", json!({"record_banners": true}));
    let notices = call(&run, "chat.status", json!({}))["notices"].clone();
    assert_eq!(notices.as_array().map(Vec::len), Some(1), "{notices}");
    assert_eq!(
        banners(&run).len(),
        1,
        "told as a banner: no client was connected"
    );
    run.stop();
    // a restart doesn't tell it again
    run.spawn();
    let again = call(&run, "chat.status", json!({}))["notices"].clone();
    assert_eq!(again.as_array().map(Vec::len), Some(1), "{again}");
    run.stop();
}

/// HAND-545, HAND-546: a browser window that came up during a task, as the browser part publishes it, reaches the owner
/// through the daemon's own consumer: one notice (shown in chat, kept in `notices.list`), once per task and site, and
/// the task's record holds what was told (an `owner.notice` span).
// decision: HAND-546
#[test]
fn a_window_that_came_up_is_told_once_and_kept_in_the_tasks_record() {
    let tmp = root();
    let mut run = Run::start(tmp.path(), "05", json!({"record_banners": true}));
    banners(&run); // whatever the start posted
    let shown = json!({"task_id": "tree-9.1", "tab": "t3", "site": "app.example.edu",
                       "shown": [{"cause": "popup_window", "window": "popup", "focused": true, "put_back": true}]});
    for _ in 0..2 {
        run.control(json!({"do": "publish", "topic": "browser.window_shown", "data": shown}));
    }
    let line = "A pop-up window opened for a moment on app.example.edu and took your keyboard focus; I minimized it.";
    let deadline = Instant::now() + Duration::from_secs(10);
    let ours = |run: &Run| -> Vec<Value> {
        call(run, "notices.list", json!({}))["notices"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter(|n| n["id"] == "window-shown-tree-9-app.example.edu")
            .collect()
    };
    while ours(&run).is_empty() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
    std::thread::sleep(Duration::from_millis(300)); // the second publish's work, had it posted anything
    let notices = ours(&run);
    assert_eq!(notices.len(), 1, "{notices:?}");
    assert_eq!(notices[0]["text"], json!(line));
    let spans = run.control(json!({"do": "table", "name": "spans"}));
    let told: Vec<&Value> = spans
        .as_array()
        .unwrap()
        .iter()
        .filter(|r| r.to_string().contains("owner.notice"))
        .collect();
    assert_eq!(told.len(), 1, "{told:?}");
    let row = told[0].to_string();
    assert!(
        row.contains("tree-9") && row.contains("took your keyboard focus"),
        "{row}"
    );
    // no FERB client was connected: the fallback banner, once
    assert_eq!(banners(&run).len(), 1);
    run.stop();
}
