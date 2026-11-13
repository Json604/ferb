//! The Mac hands through the real binary. The default tests stay off the owner's screen: refusals before anything
//! reaches an app, and paths that need nothing of the Mac. `real_machine_checks` (ignored: run it by name, with
//! `--ignored`) reads this Mac for real and types into a TextEdit document it opens itself; see its text.

mod shim;
mod support;

use std::time::{Duration, Instant};

use serde_json::{json, Value};
use shim::{started, Shim};
use support::*;

#[test]
fn mac_tools_refuse_before_acting() {
    let tmp = root();
    let (_run, mut shim) = started(tmp.path(), "01");
    let both = shim.call(
        "mac.open",
        json!({"bundle_id": "com.example.none", "path": "~/a.txt", "url": "https://example.com"}),
    );
    assert_eq!(both["is_error"], true, "{both}");
    assert!(
        both.to_string().contains("pass path or url, not both"),
        "{both}"
    );
    let relative = shim.call(
        "mac.open",
        json!({"bundle_id": "com.example.none", "path": "a.txt"}),
    );
    assert!(
        relative.to_string().contains("path must be absolute"),
        "{relative}"
    );
    let shell = shim.call(
        "mac.applescript",
        json!({"app": "com.example.none", "script": "do shell script \"ls\""}),
    );
    assert_eq!(shell["is_error"], true, "{shell}");
    assert!(shell.to_string().contains("shell.run"), "{shell}");
    // a daemon started without the Mac's own parts never writes the owner's clipboard
    let clip = shim.call("mac.clipboard", json!({"text": "invented"}));
    assert_eq!(clip["is_error"], true, "{clip}");
    // audio.listen: exactly one of app or tab (the model's validator)
    let neither = shim.call("audio.listen", json!({"seconds": 1}));
    assert_eq!(neither["is_error"], true, "{neither}");
    // recordings: an id this task never started; a video that isn't there
    let stop = shim.call("recording.stop", json!({"id": "rec-none"}));
    assert_eq!(stop["is_error"], true, "{stop}");
    let inspect = shim.call("recording.inspect", json!({"path": "missing.mp4"}));
    assert_eq!(inspect["is_error"], true, "{inspect}");
}

fn frontmost(shim: &mut Shim) -> (usize, Option<String>) {
    let apps = shim.call("mac.apps", json!({}));
    let list = apps["content"]["apps"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let front = list
        .iter()
        .find(|a| a["frontmost"] == true)
        .and_then(|a| a["bundle_id"].as_str())
        .map(String::from);
    (list.len(), front)
}

fn running(shim: &mut Shim, bundle: &str) -> bool {
    let apps = shim.call("mac.apps", json!({}));
    apps["content"]["apps"]
        .as_array()
        .is_some_and(|l| l.iter().any(|a| a["bundle_id"] == bundle))
}

/// The element of a snapshot whose role is `role` (first), and the snapshot's elements' count.
fn element(snap: &Value, role: &str) -> Option<String> {
    snap["content"]["elements"]
        .as_array()?
        .iter()
        .find(|e| e["role"] == role)
        .and_then(|e| e["id"].as_str())
        .map(String::from)
}

const TEXTEDIT: &str = "com.apple.TextEdit";

/// The system's own answer (Launch Services' `lsappinfo`), independent of the daemon's lists: the bundle id of the
/// app in front.
fn system_front() -> Option<String> {
    let asn = std::process::Command::new("/usr/bin/lsappinfo")
        .arg("front")
        .output()
        .ok()?;
    let asn = String::from_utf8_lossy(&asn.stdout).trim().to_string();
    let info = std::process::Command::new("/usr/bin/lsappinfo")
        .args(["info", "-only", "bundleid", &asn])
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&info.stdout).to_string();
    text.split("bundleID=\"")
        .nth(1)?
        .split('"')
        .next()
        .map(String::from)
}

/// The pids of running TextEdit processes, from the kernel's process table (never a cached app list).
fn textedit_pids() -> Vec<i32> {
    std::process::Command::new("/usr/bin/pgrep")
        .args(["-x", "TextEdit"])
        .output()
        .map(|o| {
            String::from_utf8_lossy(&o.stdout)
                .split_whitespace()
                .filter_map(|p| p.parse().ok())
                .collect()
        })
        .unwrap_or_default()
}

/// Where the owner's pointer is (a read: CGEventCreate's location).
fn cursor() -> (f64, f64) {
    let e = objc2_core_graphics::CGEvent::new(None);
    let p = objc2_core_graphics::CGEvent::location(e.as_deref());
    (p.x, p.y)
}

/// Asks the TextEdit processes this test started to quit, as ⌘Q does (NSRunningApplication terminate): never a
/// signal. An app with unsaved changes may stay, showing its own save sheet: it is then reported, never forced.
struct AskToQuit(Vec<i32>);

impl Drop for AskToQuit {
    fn drop(&mut self) {
        use ferb_mac_hands::launched::AppControl;
        for &pid in &self.0 {
            if textedit_pids().contains(&pid) {
                let asked = ferb_mac_hands::native::apps::NativeControl.quit(pid, TEXTEDIT);
                eprintln!(
                    "cleanup: asked TextEdit (pid {pid}, started by this test) to quit: {asked}"
                );
            }
        }
    }
}

/// One input test in a TextEdit document this test opens (rule 10: background only; rule 16: it closes only what it
/// created). Skipped when TextEdit already runs: the owner may have documents open there. Stops before typing when
/// the open brought TextEdit to the front.
fn textedit_check(shim: &mut Shim, tmp: &std::path::Path) -> Value {
    if !textedit_pids().is_empty() || running(shim, TEXTEDIT) {
        return json!("skipped: TextEdit was already running (it may hold the owner's documents)");
    }
    let mut t = serde_json::Map::new();
    let doc = tmp.join("ferbd-check.txt");
    std::fs::write(&doc, "").unwrap();
    let (front_before, cursor_before) = (system_front(), cursor());
    t.insert("front_before".into(), json!(front_before));
    let opened = shim.call(
        "mac.open",
        json!({"bundle_id": TEXTEDIT, "path": doc.to_string_lossy()}),
    );
    // whatever happens next, the TextEdit this test started is asked to quit at the end
    let _quit = AskToQuit(textedit_pids());
    t.insert(
        "open".into(),
        json!({"is_error": opened["is_error"], "error": opened["content"]["error"],
               "launched": opened["content"]["launched"], "window": opened["content"]["window"]}),
    );
    let front_after_open = system_front();
    t.insert("front_after_open".into(), json!(front_after_open));
    let came_forward = front_after_open.as_deref() == Some(TEXTEDIT);
    t.insert("textedit_came_forward".into(), json!(came_forward));
    // the daemon's own list sees the app it just launched (the stale-list check)
    let until = Instant::now() + Duration::from_secs(5);
    let mut listed = running(shim, TEXTEDIT);
    while !listed && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(250));
        listed = running(shim, TEXTEDIT);
    }
    t.insert("daemon_lists_textedit".into(), json!(listed));
    // its window, through Accessibility
    let until = Instant::now() + Duration::from_secs(15);
    let (mut area, mut roles) = (None, std::collections::BTreeSet::new());
    while Instant::now() < until && area.is_none() {
        let snap = shim.call("mac.snapshot", json!({"app": TEXTEDIT}));
        area = element(&snap, "textbox");
        roles = snap["content"]["elements"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|e| e["role"].as_str().map(String::from))
            .collect();
        if area.is_none() {
            std::thread::sleep(Duration::from_millis(500));
        }
    }
    t.insert("text_area_found".into(), json!(area.is_some()));
    t.insert("snapshot_roles".into(), json!(roles));
    const LINE: &str = "Invented line from the ferbd check";
    if came_forward {
        t.insert(
            "typing".into(),
            json!("skipped: opening the document brought TextEdit to the front"),
        );
    } else if let Some(el) = &area {
        let cursor_before_type = cursor();
        let typed = shim.call(
            "mac.act",
            json!({"app": TEXTEDIT, "action": "type", "element": el, "text": LINE}),
        );
        t.insert(
            "cursor_moved_while_typing".into(),
            json!(cursor() != cursor_before_type),
        );
        let keys: Vec<String> = typed["content"]
            .as_object()
            .map(|m| m.keys().cloned().collect())
            .unwrap_or_default();
        t.insert(
            "type".into(),
            json!({"is_error": typed["is_error"], "error": typed["content"]["error"],
                   "code": typed["content"]["code"], "ok": typed["content"]["ok"],
                   "detail": typed["content"]["detail"], "result_keys": keys}),
        );
        // read back through Accessibility: the text area's value (or the window's text), up to 5 s
        let until = Instant::now() + Duration::from_secs(5);
        let mut read_back = json!(null);
        while Instant::now() < until {
            std::thread::sleep(Duration::from_millis(500));
            let snap = shim.call("mac.snapshot", json!({"app": TEXTEDIT}));
            let boxes: Vec<Value> = snap["content"]["elements"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|e| e["role"] == "textbox")
                .cloned()
                .collect();
            let in_value = boxes
                .iter()
                .any(|e| e["value"].as_str().is_some_and(|v| v.contains(LINE)));
            let in_text = snap["content"]["text"].to_string().contains(LINE);
            read_back = json!({"in_value": in_value, "in_text": in_text,
                               "value_lengths": boxes.iter().map(|e| e["value"].as_str().map(|v| v.chars().count())).collect::<Vec<_>>()});
            if in_value || in_text {
                break;
            }
        }
        t.insert("read_back".into(), read_back);
    } else {
        t.insert(
            "typing".into(),
            json!("skipped: no text area reachable through Accessibility"),
        );
    }
    t.insert("front_after_typing".into(), json!(system_front()));
    let cursor_after = cursor();
    t.insert("cursor_moved".into(), json!(cursor_after != cursor_before));
    // close the document: File > Close (TextEdit autosaves a document that has a file, so it closes without a
    // question; a sheet that asks anyway is answered Don't Save / Delete / Revert), then ⌘Q through FERB's safe quit
    // (HAND-200: an app this task launched, with no unsaved changes)
    let closed = shim.call("mac.menu", json!({"app": TEXTEDIT, "path": "File > Close"}));
    t.insert(
        "close".into(),
        json!({"is_error": closed["is_error"], "error": closed["content"]["error"]}),
    );
    std::thread::sleep(Duration::from_millis(800));
    let sheet = shim.call("mac.snapshot", json!({"app": TEXTEDIT}));
    let dont = sheet["content"]["elements"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|e| {
            e["role"] == "button"
                && e["label"]
                    .as_str()
                    .is_some_and(|l| l.starts_with("Don") || l == "Delete" || l == "Revert")
        })
        .cloned();
    if let Some(b) = dont {
        let pressed = shim.call(
            "mac.act",
            json!({"app": TEXTEDIT, "action": "press", "element": b["id"]}),
        );
        t.insert(
            "dont_save".into(),
            json!({"is_error": pressed["is_error"], "label": b["label"]}),
        );
        std::thread::sleep(Duration::from_millis(800));
    }
    let quit = shim.call(
        "mac.act",
        json!({"app": TEXTEDIT, "action": "key", "text": "cmd+q"}),
    );
    t.insert(
        "quit".into(),
        json!({"is_error": quit["is_error"], "error": quit["content"]["error"],
               "detail": quit["content"]["detail"]}),
    );
    let until = Instant::now() + Duration::from_secs(10);
    while Instant::now() < until && !textedit_pids().is_empty() {
        std::thread::sleep(Duration::from_millis(300));
    }
    t.insert(
        "still_running_after_quit".into(),
        json!(!textedit_pids().is_empty()),
    );
    t.insert("front_after".into(), json!(system_front()));
    t.insert(
        "front_unchanged".into(),
        json!(system_front() == front_before),
    );
    let _ =
        std::fs::read_to_string(&doc).map(|s| t.insert("doc_bytes_after".into(), json!(s.len())));
    Value::Object(t)
}

/// Real-machine checks (rust/crates/ferbd, point 4 of its brief). Passive reads of this Mac through the composed Mac
/// layer, then one input test in a TextEdit document this test opens itself (only when TextEdit isn't running, so
/// none of the owner's documents is near). Nothing is printed of what the screen shows: counts, statuses and facts
/// only. The screenshot exists only in the temp home's evidence folder, removed with it.
#[test]
#[ignore]
fn real_machine_checks() {
    let tmp = root();
    let run = Run::start(tmp.path(), "01", json!({"ids": "id", "mac": true}));
    let token = run.control(
        json!({"do": "issue", "task_id": "task-1", "worker_id": "w-1", "identity": "owner",
        "scopes": ["mac:read", "mac:act", "screen", "fs:read"]}),
    );
    run.control(json!({"do": "hold_task", "task_id": "task-1"}));
    std::fs::create_dir_all(run.home.join("workspace")).unwrap();
    // the first text recognition loads Vision's recognizer: up to minutes on this Mac (DMN-520 warms it at start)
    let mut shim = Shim::open_waiting(&run, token.as_str().unwrap(), 300);
    let mut report = serde_json::Map::new();

    // 1. the running apps
    let (count, front) = frontmost(&mut shim);
    report.insert("apps_listed".into(), json!(count));
    report.insert("frontmost_known".into(), json!(front.is_some()));

    // 2. the frontmost window's Accessibility tree
    if let Some(app) = &front {
        let snap = shim.call("mac.snapshot", json!({"app": app}));
        report.insert(
            "snapshot".into(),
            json!({"is_error": snap["is_error"], "elements": snap["content"]["elements"].as_array().map(Vec::len),
                   "error": snap["content"]["error"], "code": snap["content"]["code"]}),
        );
        // 3. one window picture through the capture path, then the gateway's text-recognition masking. With no window
        // id, an app with more than one reachable window has no unique capture target (driver.py:913-923, as Python):
        // the call says so; then the snapshot's current window by its window-server id.
        let windows = snap["content"]["windows"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let started = Instant::now();
        let bare = shim.call("mac.screenshot", json!({"app": app}));
        report.insert(
            "screenshot_without_window".into(),
            json!({"windows_listed": windows.len(), "is_error": bare["is_error"],
                   "no_unique_target": bare.to_string().contains("no unique capture target"),
                   "image": bare.get("image").is_some(), "seconds": started.elapsed().as_secs()}),
        );
        // the current window, else the first listed (the worker's choice of an id from the list)
        if let Some(w) = windows
            .iter()
            .find(|w| w["current"] == true)
            .or(windows.first())
        {
            let started = Instant::now();
            let shot = shim.call(
                "mac.screenshot",
                json!({"app": app, "window": ferb_py::value::py_str(&w["id"])}),
            );
            report.insert(
                "screenshot".into(),
                json!({"is_error": shot["is_error"], "image": shot.get("image").is_some(),
                       "image_withheld": shot["content"]["image_withheld"], "error": shot["content"]["error"],
                       "masking": shot["content"]["masking"], "seconds": started.elapsed().as_secs()}),
            );
        }
    }
    // 4. the pasteboard's change count (never its contents)
    report.insert(
        "pasteboard_change_count_read".into(),
        json!(ferbd::native::pasteboard_change_count() >= 0),
    );

    // 5. one harmless input test in a TextEdit document this test opens (only when TextEdit isn't running)
    if std::env::var("FERBD_REAL_INPUT").as_deref() == Ok("0") {
        report.insert("textedit".into(), json!("skipped: FERBD_REAL_INPUT=0"));
    } else {
        report.insert("textedit".into(), textedit_check(&mut shim, tmp.path()));
    }
    eprintln!("REAL-MACHINE REPORT {}", Value::Object(report));
    // what the evidence store kept (in the temp home only): its files are removed with the temp folder
    let kept = std::fs::read_dir(run.home.join("evidence"))
        .map(|d| d.count())
        .unwrap_or(0);
    eprintln!("evidence day folders in the temp home: {kept}");
}
