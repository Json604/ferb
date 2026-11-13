//! Owner idle and active transitions (`ferb/sense/watchers/idle.py`), from the owner-input tap (FERB's own marked
//! input never counts). Only the transitions; nothing about what was typed or clicked. Idle is never authorization.

use std::sync::{Arc, Mutex};

use ferb_core::interfaces::BoxFuture;
use serde_json::Value;

use crate::manager::{Emit, Stop, Watcher};
use crate::observation::{dict, Observation};

/// idle.py:14.
pub struct IdleWatcher {
    last_owner_wall: Arc<dyn Fn() -> Option<f64> + Send + Sync>,
    tap_running: Arc<dyn Fn() -> bool + Send + Sync>,
    pub idle_after_s: f64,
    pub interval_s: f64,
    now: Arc<dyn Fn() -> f64 + Send + Sync>,
    state: Mutex<Option<String>>,
}

impl IdleWatcher {
    /// `now`: the wall clock as a POSIX time.
    pub fn new(
        last_owner_wall: Arc<dyn Fn() -> Option<f64> + Send + Sync>,
        tap_running: Arc<dyn Fn() -> bool + Send + Sync>,
        idle_after_s: f64,
        now: Arc<dyn Fn() -> f64 + Send + Sync>,
    ) -> IdleWatcher {
        IdleWatcher {
            last_owner_wall,
            tap_running,
            idle_after_s,
            interval_s: 2.0,
            now,
            state: Mutex::new(None),
        }
    }

    /// idle.py:24: without the tap FERB can't tell, and records nothing.
    pub fn check(&self, emit: &Emit) {
        if !(self.tap_running)() {
            return;
        }
        let last = (self.last_owner_wall)();
        let state = match last {
            Some(t) if (self.now)() - t < self.idle_after_s => "active",
            _ => "idle",
        };
        let mut held = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if held.as_deref() != Some(state) {
            *held = Some(state.to_string());
            drop(held);
            emit(Observation::by(
                "idle",
                dict([("state", Value::from(state))]),
                "owner",
            ));
        }
    }
}

impl Watcher for IdleWatcher {
    fn source(&self) -> &str {
        "idle"
    }

    fn run(self: Arc<Self>, emit: Emit, stop: Stop) -> BoxFuture<'static, Result<(), String>> {
        Box::pin(async move {
            *self
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
            while !stop.is_set() {
                self.check(&emit);
                stop.wait_for(self.interval_s).await;
            }
            Ok(())
        })
    }
}
