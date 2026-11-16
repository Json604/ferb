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

/// HAND-586 (the owner, 2026-11-16, "yes it should if it can"): the screen question for a browser says why FERB
/// needs the window when the tool knows which case it is (`window`: a page in a profile the extension isn't connected
/// in, the browser's own controls, or a browser part not told apart); without it the card is as before.
// decision: HAND-586
#[test]
fn the_screen_question_for_a_browser_says_why_it_needs_the_window() {
    let apps = TwoApps;
    let card = |window: Option<&str>| {
        let mut need = confirm::foreground_need(
            "mac.click",
            &args(json!({"app": "Chat", "shot": "s", "x": 1, "y": 1})),
            &apps,
        )
        .expect("a foreground question");
        if let Some(w) = window {
            need.values.set("window", w);
        }
        confirm::card(&[need], "", &apps).unwrap()
    };
    assert_eq!(
        card(None).question,
        "Let FERB use your screen in Chat for this task?"
    );
    assert_eq!(
        card(None).detail,
        "Chat will come to the front while FERB works in it. One Yes covers Chat until this task ends."
    );
    assert_eq!(
        card(Some("page")).detail,
        "FERB's extension can't reach this part of Chat (a profile it isn't connected in), so FERB needs the \
         window. Chat will come to the front while FERB works in it. One Yes covers Chat until this task ends."
    );
    assert_eq!(
        card(Some("controls")).detail,
        "FERB's extension can't reach this part of Chat (the browser's own controls), so FERB needs the window. \
         Chat will come to the front while FERB works in it. One Yes covers Chat until this task ends."
    );
    assert_eq!(
        card(Some("browser")).detail,
        "FERB's extension can't reach this part of Chat (a profile it isn't connected in, or the browser's own \
         controls), so FERB needs the window. Chat will come to the front while FERB works in it. One Yes covers \
         Chat until this task ends."
    );
}
