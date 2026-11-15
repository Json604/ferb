//! Focused window titles (`ferb/sense/watchers/windows.py`), when `[sense] window_titles` is on. Browsers are
//! skipped: their pages are recorded by the tab watcher, where the URL lets the domain exclusions apply. Never
//! raises, moves or resizes a window.

use std::sync::{Arc, Mutex};

use ferb_core::interfaces::BoxFuture;
use serde_json::Value;

use super::base::{poll, Focused};
use crate::manager::{Emit, Stop, Watcher};
use crate::observation::{dict, Observation};

/// The frontmost app and its focused window's title (`_read`).
pub type ReadTitle = Arc<dyn Fn() -> Option<(Focused, Option<String>)> + Send + Sync>;

/// windows.py:15.
pub struct WindowTitleWatcher {
    pub interval_s: f64,
    enabled: Arc<dyn Fn() -> bool + Send + Sync>,
    read: ReadTitle,
    is_browser: Arc<dyn Fn(&str) -> bool + Send + Sync>,
    last: Mutex<Option<(String, String)>>,
}

impl WindowTitleWatcher {
    pub fn new(
        enabled: Arc<dyn Fn() -> bool + Send + Sync>,
        read: ReadTitle,
        is_browser: Arc<dyn Fn(&str) -> bool + Send + Sync>,
    ) -> WindowTitleWatcher {
        WindowTitleWatcher {
            interval_s: 1.0,
            enabled,
            read,
            is_browser,
            last: Mutex::new(None),
        }
    }

    pub fn step(&self, emit: &Emit) {
        if !(self.enabled)() {
            return;
        }
        let Some((f, Some(title))) = (self.read)() else {
            return;
        };
        if (self.is_browser)(&f.bundle_id) {
            return;
        }
        let key = (f.bundle_id.clone(), title.clone());
        let mut last = self
            .last
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if last.as_ref() == Some(&key) {
            return;
        }
        *last = Some(key);
        drop(last);
        emit(Observation::new(
            "window",
            dict([
                ("app", Value::from(f.name)),
                ("bundle_id", Value::from(f.bundle_id)),
                ("title", Value::from(title)),
            ]),
        ));
    }
}

impl Watcher for WindowTitleWatcher {
    fn source(&self) -> &str {
        "window"
    }

    fn run(self: Arc<Self>, emit: Emit, stop: Stop) -> BoxFuture<'static, Result<(), String>> {
        Box::pin(async move {
            *self
                .last
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
            let me = self.clone();
            poll(
                self.interval_s,
                &stop,
                Arc::new(move || {
                    me.step(&emit);
                    Ok(())
                }),
            )
            .await
        })
    }
}
