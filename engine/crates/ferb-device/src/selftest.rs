//! The daemon's passive permission self-test (ferb/device/selftest.py): reading the Dock's Accessibility tree and
//! creating a listen-only event tap. Never posts input, activates an app or moves the cursor.
//!
//! The two probes call AppKit, Accessibility and Quartz; they belong with the Mac hands (`ferb-mac-hands`) and come in
//! through [`Probes`]. This module keeps the result type and the words.

use serde::Serialize;

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SelfTestResult {
    pub name: String,
    pub ok: bool,
    pub detail: String,
}

/// What the Mac answered: the Dock's top-level element count (Err: the AX error code, or None when the Dock isn't
/// running), and whether a listen-only tap could be created.
pub trait Probes {
    fn dock_children(&mut self) -> Result<usize, Option<i32>>;
    fn listen_only_tap(&mut self) -> bool;
}

/// selftest.py:468.
pub fn ax_read(p: &mut dyn Probes) -> SelfTestResult {
    let (ok, detail) = match p.dock_children() {
        Ok(n) => (
            true,
            format!("Dock accessibility tree readable ({n} top-level elements)"),
        ),
        Err(None) => (false, "Dock not running".to_string()),
        Err(Some(code)) => (false, format!("AX error {code} reading the Dock")),
    };
    SelfTestResult {
        name: "ax_read".into(),
        ok,
        detail,
    }
}

/// selftest.py:519.
pub fn event_tap(p: &mut dyn Probes) -> SelfTestResult {
    let ok = p.listen_only_tap();
    SelfTestResult {
        name: "event_tap".into(),
        ok,
        detail: if ok {
            "listen-only tap created; no input posted".into()
        } else {
            "couldn't create a listen-only event tap (Accessibility/Input Monitoring unavailable)"
                .into()
        },
    }
}

pub fn run_all(p: &mut dyn Probes) -> Vec<SelfTestResult> {
    vec![ax_read(p), event_tap(p)]
}
