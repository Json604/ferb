//! HAND-675: when FERB's task first types into a desktop chat app (an app chat), the terminal question (GATE-585)
//! names the app's chat and its message box, never a terminal; a terminal's question is unchanged.

use ferb_core::types::TerminalInput;
use ferb_policy::confirm::{Apps, AppsUnread, RunningApp};
use ferb_policy::terminal;

struct NoApps;

impl Apps for NoApps {
    fn candidates(&self, _name: &str) -> Result<Vec<RunningApp>, AppsUnread> {
        Ok(Vec::new())
    }
    fn installed_bundle(&self, _name: &str) -> Result<Option<String>, AppsUnread> {
        Ok(None)
    }
    fn is_browser_app(&self, _app: &str) -> bool {
        false
    }
}

fn input(chat: bool) -> TerminalInput {
    TerminalInput {
        app: if chat { "Claude" } else { "Terminal" }.into(),
        bundle_id: if chat {
            "com.anthropic.claudefordesktop"
        } else {
            "com.apple.Terminal"
        }
        .into(),
        surface: if chat { "its message box" } else { "terminal" }.into(),
        text: Some("make it shorter".into()),
        enter: true,
        chat,
        ..TerminalInput::default()
    }
}

// decision: HAND-675
#[test]
fn a_chat_app_s_question_names_its_chat_never_a_terminal() {
    let need = terminal::need(&input(true));
    let card = ferb_policy::confirm::card(std::slice::from_ref(&need), "", &NoApps).unwrap();
    let all = format!(
        "{} {} {} {} {}",
        card.question, card.detail, card.tail, card.yes, card.no
    );
    assert_eq!(card.question, "Send this to Claude?");
    assert_eq!(card.exact, "make it shorter");
    assert!(all.contains("Claude's message box"), "{all}");
    assert!(!all.to_lowercase().contains("terminal"), "{all}");
    assert!(
        all.contains("for the rest of this task"),
        "one Yes per app per task: {all}"
    );
    let summary = ferb_policy::confirm::summary(&[need], "").unwrap();
    assert!(
        summary.contains("Claude's chat") && !summary.to_lowercase().contains("terminal"),
        "{summary}"
    );
    // still one Yes per app per task, keyed as GATE-586 keys it
    assert_eq!(
        terminal::need(&input(true)).grants,
        terminal::app_key("com.anthropic.claudefordesktop")
    );
    // a terminal's question is a terminal's, as before
    let t = ferb_policy::confirm::card(&[terminal::need(&input(false))], "", &NoApps).unwrap();
    assert!(t.question.contains("Terminal"), "{}", t.question);
    // a terminal's input without the field writes and reads the same bytes as before (Python's)
    let plain = serde_json::to_value(input(false)).unwrap();
    assert!(plain.get("chat").is_none());
    let back: TerminalInput = serde_json::from_value(plain).unwrap();
    assert!(!back.chat);
}
