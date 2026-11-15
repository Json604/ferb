//! Frontmost-app changes (`ferb/sense/watchers/app_focus.py`): app name and bundle id only. Never activates an app.

use std::sync::{Arc, Mutex};

use ferb_core::interfaces::BoxFuture;
use serde_json::Value;

use super::base::{poll, Focused, ReadFocused};
use crate::manager::{Emit, Stop, Watcher};
use crate::observation::{dict, Observation};

/// app_focus.py:12.
pub struct AppFocusWatcher {
    pub interval_s: f64,
    read: ReadFocused,
    on_change: Arc<dyn Fn(&Focused) + Send + Sync>,
    last: Mutex<Option<String>>,
}

impl AppFocusWatcher {
    pub fn new(
        read: ReadFocused,
        on_change: Arc<dyn Fn(&Focused) + Send + Sync>,
    ) -> AppFocusWatcher {
        AppFocusWatcher {
            interval_s: 0.5,
            read,
            on_change,
            last: Mutex::new(None),
        }
    }

    /// One poll: an observation when the frontmost app changed.
    pub fn step(&self, emit: &Emit) {
        let Some(f) = (self.read)() else {
            return;
        };
        let mut last = self
            .last
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if last.as_deref() == Some(f.bundle_id.as_str()) {
            return;
        }
        *last = Some(f.bundle_id.clone());
        drop(last);
        emit(Observation::new(
            "app_focus",
            dict([
                ("app", Value::from(f.name.clone())),
                ("bundle_id", Value::from(f.bundle_id.clone())),
            ]),
        ));
        (self.on_change)(&f);
    }
}

impl Watcher for AppFocusWatcher {
    fn source(&self) -> &str {
        "app_focus"
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
