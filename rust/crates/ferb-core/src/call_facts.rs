//! The facts a tool's code measures while one call runs, kept on that call's tool span (ferb/core/call_facts.py).
//! Decides nothing: code adds facts, the gateway records them, nobody reads them back to act.
//!
//! The gateway opens a record around each tool run ([`recording`]); code under it adds a fact with [`note`]. With
//! no record open, `note` does nothing. Python's context variable follows the call into `asyncio.to_thread`; here
//! a blocking closure is given the record by hand: `let held = current();` then `held.enter(|| ...)` in the thread.

use std::future::Future;
use std::sync::{Arc, Mutex};

use serde_json::Value;

use crate::types::Dict;

/// One call's record: key to the list of values noted under it, in order.
#[derive(Debug, Clone, Default)]
pub struct Record(Arc<Mutex<Dict>>);

tokio::task_local! {
    static HELD: Record;
}

impl Record {
    /// What was noted so far.
    pub fn facts(&self) -> Dict {
        self.0.lock().expect("call facts lock").clone()
    }

    /// Runs `f` (not async) with this record open: for a worker thread doing part of the call.
    pub fn enter<T>(&self, f: impl FnOnce() -> T) -> T {
        HELD.sync_scope(self.clone(), f)
    }
}

/// call_facts.py:15: add one fact under `key` to the running call's record, in order.
pub fn note(key: &str, value: &str) {
    let _ = HELD.try_with(|held| {
        let mut facts = held.0.lock().expect("call facts lock");
        let entry = facts
            .entry(key.to_string())
            .or_insert_with(|| Value::Array(Vec::new()));
        if let Value::Array(values) = entry {
            values.push(Value::String(value.to_string()));
        }
    });
}

/// The record open here, to carry into a worker thread; None when no call is being recorded.
pub fn current() -> Option<Record> {
    HELD.try_with(Clone::clone).ok()
}

/// call_facts.py:22: a fresh record for one call: what `note` adds while `call` runs. A record opened inside
/// another takes the notes until it ends.
pub async fn recording<F: Future>(call: F) -> (F::Output, Dict) {
    let record = Record::default();
    let out = HELD.scope(record.clone(), call).await;
    (out, record.facts())
}
