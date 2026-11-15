//! The general pasteboard, read passively (`ferb/sense/watchers/clipboard.py`; decisions: SENSE-404, SENSE-403).
//! Private and transient markers are checked before any text is read; the contents are read only when macOS allows
//! it without asking (15.4+ `accessBehavior`); otherwise the access state is reported once and nothing is read. The
//! one deliberate request (`request_access`, in [`crate::mac`]) is setup's, with the owner there.

use std::sync::{Arc, Mutex};

use ferb_core::interfaces::BoxFuture;
use serde_json::Value;

use super::base::{poll, ReadFocused};
use crate::capture::{clipboard_observation, clipboard_write};
use crate::manager::{Emit, Stop, Watcher};
use crate::observation::{dict, Observation, Redact};

/// clipboard.py:17: read without macOS asking the owner.
pub const READABLE: [&str; 2] = ["allow", "unrestricted"];

/// clipboard.py:20 `access`: NSPasteboardAccessBehavior by value; `None` (before macOS 15.4) is `unrestricted`.
pub fn access(behavior: Option<i64>) -> &'static str {
    match behavior {
        None => "unrestricted",
        Some(0) => "default",
        Some(1) => "ask",
        Some(2) => "allow",
        Some(3) => "deny",
        Some(_) => "unknown",
    }
}

/// The general pasteboard as the watcher reads it. `change_count`, `access_behavior` and `types` never make macOS
/// ask; `read_string` does unless access is `allow`.
pub trait Pasteboard: Send + Sync {
    fn change_count(&self) -> i64;
    fn access_behavior(&self) -> Option<i64>;
    fn types(&self) -> Vec<String>;
    fn read_string(&self) -> Option<String>;
}

struct State {
    last: Option<i64>,
    state: Option<String>,
}

/// clipboard.py:45.
pub struct ClipboardWatcher {
    redact: Arc<dyn Redact>,
    excluded: Arc<dyn Fn(&str, &str) -> bool + Send + Sync>,
    pub interval_s: f64,
    report: Arc<dyn Fn(&str) + Send + Sync>,
    board: Arc<dyn Pasteboard>,
    focused: ReadFocused,
    held: Mutex<State>,
}

impl ClipboardWatcher {
    /// The count when the board is first known is the starting point: the old clipboard is never recaptured.
    pub fn new(
        redact: Arc<dyn Redact>,
        excluded: Arc<dyn Fn(&str, &str) -> bool + Send + Sync>,
        report: Arc<dyn Fn(&str) + Send + Sync>,
        board: Arc<dyn Pasteboard>,
        focused: ReadFocused,
    ) -> ClipboardWatcher {
        let last = Some(board.change_count());
        ClipboardWatcher {
            redact,
            excluded,
            interval_s: 0.5,
            report,
            board,
            focused,
            held: Mutex::new(State { last, state: None }),
        }
    }

    /// The change count last seen and the access state last reported.
    pub fn seen(&self) -> (Option<i64>, Option<String>) {
        let s = self
            .held
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        (s.last, s.state.clone())
    }

    /// clipboard.py:56 `step`.
    pub fn step(&self, emit: &dyn Fn(Observation), report: &dyn Fn(&str)) {
        let count = self.board.change_count();
        let state = {
            let mut s = self
                .held
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if s.last.is_none() {
                s.last = Some(count);
            }
            if s.last == Some(count) {
                return;
            }
            s.last = Some(count);
            let state = access(self.board.access_behavior());
            if s.state.as_deref() != Some(state) {
                s.state = Some(state.to_string());
                drop(s);
                report(state);
            }
            state
        };
        if !READABLE.contains(&state) {
            return; // reading would make macOS ask, or is denied: never read
        }
        let Some(app) = (self.focused)() else {
            return;
        };
        if (self.excluded)(&app.bundle_id, &app.name) {
            return;
        }
        let types = self.board.types();
        let marker = clipboard_write();
        let context = dict([
            ("bundle_id", Value::from(app.bundle_id.clone())),
            ("app", Value::from(app.name.clone())),
            ("pid", Value::from(app.pid)),
            ("app_context", Value::from("frontmost; not proof of writer")),
        ]);
        let board = self.board.clone();
        let event = clipboard_observation(
            count,
            &types,
            &move || board.read_string(),
            self.redact.as_ref(),
            marker.as_ref().map(|m| m.0),
            marker.as_ref().and_then(|m| m.1.as_deref()),
            Some(&context),
        );
        // frontmost is context, not proof of who copied it
        if let Some(event) = event {
            if self.board.change_count() == count {
                emit(event);
            }
        }
    }
}

impl Watcher for ClipboardWatcher {
    fn source(&self) -> &str {
        "clipboard"
    }

    fn run(self: Arc<Self>, emit: Emit, stop: Stop) -> BoxFuture<'static, Result<(), String>> {
        Box::pin(async move {
            self.held
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .last = Some(self.board.change_count());
            let me = self.clone();
            poll(
                self.interval_s,
                &stop,
                Arc::new(move || {
                    let report = me.report.clone();
                    me.step(&|o| emit(o), &|s| report(s));
                    Ok(())
                }),
            )
            .await
        })
    }
}
