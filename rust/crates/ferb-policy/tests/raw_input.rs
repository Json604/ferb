//! HAND-549, HAND-554: raw input (pointer and key events) to an app seen coming to the front on FERB's raw input asks
//! first, with the foreground question and its key (once per app per task); its elements' Accessibility actions don't.

use ferb_policy::confirm::{self, Apps, AppsUnread, RunningApp, FOREGROUND};
use serde_json::{json, Map, Value};

/// Two running apps; the chat app's profile says it came to the front on FERB's last raw input.
struct TwoApps;

impl Apps for TwoApps {
    fn candidates(&self, name: &str) -> Result<Vec<RunningApp>, AppsUnread> {
        Ok(
            [("com.example.Chat", "Chat"), ("com.example.Notes", "Notes")]
                .into_iter()
                .filter(|(b, n)| *b == name || *n == name)
                .map(|(b, n)| RunningApp {
                    bundle_id: b.into(),
                    name: n.into(),
                })
                .collect(),
        )
    }
    fn installed_bundle(&self, _name: &str) -> Result<Option<String>, AppsUnread> {
        Ok(None)
    }
    fn is_browser_app(&self, _app: &str) -> bool {
        false
    }
    fn comes_forward_on_input(&self, app: &str) -> bool {
        app == "Chat"
    }
}

fn args(v: Value) -> Map<String, Value> {
    v.as_object().unwrap().clone()
}

// decision: HAND-549, HAND-554
#[test]
fn raw_input_to_an_app_that_raises_itself_asks_and_its_ax_actions_dont() {
    let apps = TwoApps;
    let need = |tool: &str, a: Value| confirm::foreground_need(tool, &args(a), &apps);
    for (tool, a) in [
        (
            "mac.pointer",
            json!({"app": "Chat", "action": "rightclick", "shot": "s", "x": 1, "y": 1}),
        ),
        (
            "mac.click",
            json!({"app": "Chat", "shot": "s", "x": 1, "y": 1}),
        ),
        (
            "mac.act",
            json!({"app": "Chat", "action": "type", "text": "hi", "element": "s1e1"}),
        ),
        (
            "mac.act",
            json!({"app": "Chat", "action": "key", "text": "return"}),
        ),
    ] {
        let n = need(tool, a.clone()).unwrap_or_else(|| panic!("{tool} {a}"));
        assert_eq!(n.rule, FOREGROUND);
        // the same key as any foreground question for that app: one Yes covers it for the task
        let flagged = need(
            "mac.act",
            json!({"app": "Chat", "action": "type", "text": "x", "mechanism": "foreground", "allow_foreground": true}),
        )
        .unwrap();
        assert_eq!(n.key, flagged.key, "{tool}");
    }
    // its elements' own actions, a hover, an AX click, and any app without the record: no question
    for (tool, a) in [
        (
            "mac.act",
            json!({"app": "Chat", "action": "menu", "element": "s1e1"}),
        ),
        (
            "mac.act",
            json!({"app": "Chat", "action": "perform", "element": "s1e1", "name": "Download"}),
        ),
        (
            "mac.act",
            json!({"app": "Chat", "action": "press", "element": "s1e1"}),
        ),
        (
            "mac.pointer",
            json!({"app": "Chat", "action": "hover", "shot": "s", "x": 1, "y": 1}),
        ),
        (
            "mac.click",
            json!({"app": "Chat", "mechanism": "ax", "shot": "s", "x": 1, "y": 1}),
        ),
        (
            "mac.pointer",
            json!({"app": "Notes", "action": "click", "shot": "s", "x": 1, "y": 1}),
        ),
        (
            "mac.act",
            json!({"app": "Notes", "action": "type", "text": "hi", "element": "s1e1"}),
        ),
    ] {
        assert!(need(tool, a.clone()).is_none(), "{tool} {a}");
    }
}
