//! ferb/orchestrator/guards.py: worker liveness. Owner and process waits suspend the stall clock.

use std::collections::HashMap;
use std::sync::Mutex;

pub const STALL_S: f64 = 180.0;

#[derive(Debug, Clone, Copy)]
struct Beat {
    last: f64,
    waits: u32,
}

/// guards.py:18 `Activity`: per-worker liveness on an injected monotonic clock.
pub struct Activity {
    now: Box<dyn Fn() -> f64 + Send + Sync>,
    beats: Mutex<HashMap<String, Beat>>,
}

impl Activity {
    pub fn new(now: impl Fn() -> f64 + Send + Sync + 'static) -> Activity {
        Activity {
            now: Box::new(now),
            beats: Mutex::new(HashMap::new()),
        }
    }

    fn beats(&self) -> std::sync::MutexGuard<'_, HashMap<String, Beat>> {
        self.beats.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn start(&self, worker: &str) {
        let now = (self.now)();
        self.beats().insert(
            worker.to_string(),
            Beat {
                last: now,
                waits: 0,
            },
        );
    }

    pub fn beat(&self, worker: &str) {
        let now = (self.now)();
        if let Some(b) = self.beats().get_mut(worker) {
            b.last = now;
        }
    }

    /// A wait on the owner opens (`on`) or closes; the stall clock starts over either way.
    pub fn waiting(&self, worker: &str, on: bool) {
        let now = (self.now)();
        if let Some(b) = self.beats().get_mut(worker) {
            b.waits = if on {
                b.waits + 1
            } else {
                b.waits.saturating_sub(1)
            };
            b.last = now;
        }
    }

    pub fn stalled(&self, worker: &str, stall_s: f64) -> bool {
        let now = (self.now)();
        self.beats()
            .get(worker)
            .is_some_and(|b| b.waits == 0 && now - b.last > stall_s)
    }

    pub fn end(&self, worker: &str) {
        self.beats().remove(worker);
    }
}
