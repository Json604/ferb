//! HAND-677, HAND-678: the rules task a843aa54 ran into. A browser's menu item pressed through the Mac tools (its
//! Profiles menu, Window > Extensions) is the foreground question, one key per browser however the call names it, and
//! listing its menus is not; the pairing question for a profile whose FERB extension is off is its own: asked every
//! time and never answered by a "don't ask for 30 min" window, while the ordinary pairing question stays routine.

use ferb_policy::confirm::{self, Apps, AppsUnread, RunningApp, FOREGROUND, PAIR};
use serde_json::{json, Map, Value};

/// Chrome, running as pid 24033, and Notes.
struct ChromeAndNotes;

impl Apps for ChromeAndNotes {
    fn candidates(&self, name: &str) -> Result<Vec<RunningApp>, AppsUnread> {
        let chrome = RunningApp {
            bundle_id: "com.google.Chrome".into(),
            name: "Google Chrome".into(),
        };
        Ok(match name {
            "pid:24033" | "Google Chrome" | "com.google.Chrome" => vec![chrome],
            "Notes" => vec![RunningApp {
                bundle_id: "com.apple.Notes".into(),
                name: "Notes".into(),
            }],
            _ => vec![],
        })
    }
    fn installed_bundle(&self, _name: &str) -> Result<Option<String>, AppsUnread> {
        Ok(None)
    }
    fn is_browser_app(&self, app: &str) -> bool {
        matches!(app, "pid:24033" | "Google Chrome")
    }
}

fn args(v: Value) -> Map<String, Value> {
    v.as_object().unwrap().clone()
}

// decision: HAND-677, HAND-678
#[test]
fn a_browsers_menu_press_is_the_foreground_question_and_listing_its_menus_is_not() {
    let need = |a: Value| confirm::foreground_need("mac.menu", &args(a), &ChromeAndNotes);
    let profiles =
        need(json!({"app": "pid:24033", "path": "Profiles > 26F1001618 (ds.study.iitm.ac.in)"}))
            .expect("a browser's menu press asks");
    let extensions =
        need(json!({"app": "Google Chrome", "path": "Window > Extensions"})).expect("asks");
    assert_eq!(profiles.rule, FOREGROUND);
    assert_eq!(
        profiles.key, extensions.key,
        "one key for the browser however it is named"
    );
    assert!(
        need(json!({"app": "pid:24033", "path": ""})).is_none(),
        "listing its menus reads"
    );
    assert!(
        need(json!({"app": "Notes", "path": "File > New Note"})).is_none(),
        "another app's menu is background"
    );
}

// decision: HAND-677, HAND-678
#[test]
fn the_turned_off_pairing_question_is_asked_each_time_and_never_routine() {
    let plain = confirm::pair_need("IIT", "chrome", "Profile 30", "me@example.test");
    let off = confirm::pair_enable_need("IIT", "chrome", "Profile 30", "me@example.test");
    assert_eq!((plain.rule.as_str(), off.rule.as_str()), (PAIR, PAIR));
    assert_eq!(off.key, plain.key, "its Yes is also the pairing's");
    assert!(confirm::routine(&plain) && !plain.always);
    assert!(!confirm::routine(&off) && off.always);
    assert_eq!(off.values.get("off"), Some("yes"));
}

/// GATE-623 (owner, 2026-11-22: "No, always ask for screen input"): the foreground question (screen use, raw input
/// that brings an app forward, and a script for his browser, which shares its key, GATE-222) is never routine; a
/// browser's own pages, new recipients and pairing stay routine (GATE-207).
// decision: GATE-623, GATE-207
#[test]
fn a_dont_ask_window_never_covers_the_screen() {
    let screen = confirm::foreground_need(
        "mac.menu",
        &args(json!({"app": "pid:24033", "path": "Profiles > X"})),
        &ChromeAndNotes,
    )
    .unwrap();
    assert!(!confirm::routine(&screen));
    assert!(!confirm::ROUTINE.contains(&FOREGROUND));
    let page = confirm::browser_page_need("Google Chrome", "chrome://extensions", "Click");
    assert!(confirm::routine(&page));
}
