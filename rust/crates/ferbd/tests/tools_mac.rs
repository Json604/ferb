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
        // 3. one window picture through the capture path, then the gateway's text-recognition masking
        // a window of that app by its window-server id (the snapshot's current one, else the first on the list): an app
        // with several windows has no unique capture target
        let window = snap["content"]["windows"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|w| w["current"] == true)
            .map(|w| ferb_py::value::py_str(&w["id"]))
            .or_else(|| {
                let apps = ferb_mac_hands::native::apps::running();
                let a = apps.iter().find(|a| &a.bundle_id == app)?;
                ferb_mac_hands::native::windows::NativeWindows
                    .list_all(false)?
                    .into_iter()
                    .find(|w| w.pid == a.pid && w.layer == 0)
                    .map(|w| w.id.to_string())
            });
        let mut args = json!({"app": app});
        if let Some(w) = window {
            args["window"] = json!(w);
        }
        let shot = shim.call("mac.screenshot", args);
        report.insert(
            "screenshot".into(),
            json!({"is_error": shot["is_error"], "image": shot.get("image").is_some(),
                   "image_withheld": shot["content"]["image_withheld"], "error": shot["content"]["error"],
                   "masking": shot["content"]["masking"]}),
        );
    }
    // 4. the pasteboard's change count (never its contents)
    report.insert(
        "pasteboard_change_count_read".into(),
        json!(ferbd::native::pasteboard_change_count() >= 0),
    );

    // 5. one harmless input test in a TextEdit document this test opens
    const TEXTEDIT: &str = "com.apple.TextEdit";
    if std::env::var("FERBD_REAL_INPUT").as_deref() == Ok("0") {
        report.insert("textedit".into(), json!("skipped: FERBD_REAL_INPUT=0"));
    } else if running(&mut shim, TEXTEDIT) {
        report.insert(
            "textedit".into(),
            json!("skipped: TextEdit was already running (the owner's documents)"),
        );
    } else {
        // if anything below fails, TextEdit (started by this test, holding only its own document) is ended
        struct Guard;
        impl Drop for Guard {
            fn drop(&mut self) {
                for a in ferb_mac_hands::native::apps::running() {
                    if a.bundle_id == "com.apple.TextEdit" {
                        // SAFETY: a pid read just now; SIGKILL to the app this test started
                        unsafe {
                            libc::kill(a.pid, libc::SIGKILL);
                        }
                        eprintln!(
                            "cleanup: TextEdit (pid {}) ended by the test's guard",
                            a.pid
                        );
                    }
                }
            }
        }
        let _guard = Guard;
        let doc = tmp.path().join("ferbd-check.txt");
        std::fs::write(&doc, "").unwrap();
        let before = frontmost(&mut shim).1;
        let opened = shim.call(
            "mac.open",
            json!({"bundle_id": TEXTEDIT, "path": doc.to_string_lossy()}),
        );
        let mut t = serde_json::Map::new();
        t.insert(
            "open".into(),
            json!({"is_error": opened["is_error"], "error": opened["content"]["error"]}),
        );
        // its window, through Accessibility
        let until = Instant::now() + Duration::from_secs(15);
        let mut area = None;
        while Instant::now() < until && area.is_none() {
            std::thread::sleep(Duration::from_millis(500));
            let snap = shim.call("mac.snapshot", json!({"app": TEXTEDIT}));
            area = element(&snap, "AXTextArea");
        }
        let after_open = frontmost(&mut shim).1;
        t.insert("front_changed_by_open".into(), json!(after_open != before));
        if after_open != before {
            t.insert(
                "typing".into(),
                json!("skipped: opening the document changed the front app"),
            );
        } else if let Some(el) = &area {
            const LINE: &str = "Invented line from the ferbd check";
            let typed = shim.call(
                "mac.act",
                json!({"app": TEXTEDIT, "action": "type", "element": el, "text": LINE}),
            );
            t.insert(
                "type".into(),
                json!({"is_error": typed["is_error"], "error": typed["content"]["error"],
                       "mechanism": typed["content"]["mechanism"], "delivered": typed["content"]["delivered"]}),
            );
            std::thread::sleep(Duration::from_millis(500));
            let snap = shim.call("mac.snapshot", json!({"app": TEXTEDIT}));
            let read_back = snap["content"]["elements"]
                .as_array()
                .into_iter()
                .flatten()
                .any(|e| {
                    e["role"] == "AXTextArea"
                        && e["value"].as_str().is_some_and(|v| v.contains(LINE))
                });
            t.insert("read_back".into(), json!(read_back));
            t.insert(
                "front_changed_by_typing".into(),
                json!(frontmost(&mut shim).1 != before),
            );
        } else {
            t.insert(
                "typing".into(),
                json!("skipped: no text area reachable through Accessibility"),
            );
        }
        // close the document without saving, then quit: File > Close, the sheet's Don't Save, then cmd+q (FERB
        // opened it for this task)
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
                e["role"] == "AXButton"
                    && e["label"]
                        .as_str()
                        .is_some_and(|l| l.starts_with("Don") || l == "Delete" || l == "Revert")
            });
        if let Some(b) = dont {
            let pressed = shim.call(
                "mac.act",
                json!({"app": TEXTEDIT, "action": "press", "element": b["id"]}),
            );
            t.insert(
                "dont_save".into(),
                json!({"is_error": pressed["is_error"], "label": b["label"]}),
            );
        }
        std::thread::sleep(Duration::from_millis(800));
        let quit = shim.call(
            "mac.act",
            json!({"app": TEXTEDIT, "action": "key", "text": "cmd+q"}),
        );
        t.insert(
            "quit".into(),
            json!({"is_error": quit["is_error"], "error": quit["content"]["error"]}),
        );
        let until = Instant::now() + Duration::from_secs(10);
        while Instant::now() < until && running(&mut shim, TEXTEDIT) {
            std::thread::sleep(Duration::from_millis(300));
        }
        t.insert("still_running".into(), json!(running(&mut shim, TEXTEDIT)));
        t.insert(
            "front_after".into(),
            json!(frontmost(&mut shim).1 == before),
        );
        report.insert("textedit".into(), Value::Object(t));
    }
    eprintln!("REAL-MACHINE REPORT {}", Value::Object(report));
    // what the evidence store kept (in the temp home only): its files are removed with the temp folder
    let kept = std::fs::read_dir(run.home.join("evidence"))
        .map(|d| d.count())
        .unwrap_or(0);
    eprintln!("evidence day folders in the temp home: {kept}");
}
