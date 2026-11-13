//! Timing facts for a model call and the work around it (ferb/core/timing.py; RUN-419, ORCH-526): how long each
//! part took, in whole milliseconds, from an injected monotonic clock. Holds no content and never changes what a
//! call does. Every figure is a difference of rounded marks, so a set of parts adds up exactly to its total.

use serde_json::{json, Value};

use crate::types::Dict;

/// The injected monotonic clock, in seconds.
pub type Now = Box<dyn Fn() -> f64 + Send>;

/// timing.py:12: `int(round(seconds * 1000))`; Python rounds halves to even.
fn ms(seconds: f64) -> i64 {
    (seconds * 1000.0).round_ties_even() as i64
}

/// Named, consecutive steps of one piece of work (timing.py:16): each `mark(name)` ends the step that began at the
/// previous mark (or at the start). A name marked again adds to its step. The steps add up to `total_ms`.
pub struct Steps {
    now: Now,
    t0: f64,
    at: i64,
    /// Step name to milliseconds, in the order first marked.
    pub steps: Dict,
}

impl Steps {
    pub fn new(now: Now) -> Self {
        let t0 = now();
        Steps {
            now,
            t0,
            at: 0,
            steps: Dict::new(),
        }
    }

    pub fn mark(&mut self, name: &str) -> i64 {
        let at = ms((self.now)() - self.t0).max(self.at);
        let took = at - self.at;
        self.at = at;
        let before = self.steps.get(name).and_then(Value::as_i64).unwrap_or(0);
        self.steps.insert(name.to_string(), json!(before + took));
        took
    }

    pub fn total_ms(&self) -> i64 {
        self.at
    }

    /// `{"steps_ms": {...}, "total_ms": n}`.
    pub fn facts(&self) -> Dict {
        let mut out = Dict::new();
        out.insert("steps_ms".to_string(), Value::Object(self.steps.clone()));
        out.insert("total_ms".to_string(), json!(self.at));
        out
    }
}

/// One run's model calls, call by call, from the arrival times of its runtime's stream (timing.py:40-54). `done`
/// returns runtime_ms (only when `sent` was seen), ttft_ms, generate_ms, tail_ms and total_ms; `by` names whose
/// clock saw it: "runtime" (the adapter's own stream) or "arrival" (FERB's, at the events' arrival).
pub struct CallTimer {
    now: Now,
    pub by: String,
    begin: f64,
    sent: Option<f64>,
    first: Option<f64>,
    last: Option<f64>,
    next_begin: Option<f64>,
    next_sent: Option<f64>,
}

impl CallTimer {
    pub fn new(now: Now, by: &str) -> Self {
        let begin = now();
        CallTimer {
            now,
            by: by.to_string(),
            begin,
            sent: None,
            first: None,
            last: None,
            next_begin: None,
            next_sent: None,
        }
    }

    /// This call's request is handed over now (an API request, Codex's turn start).
    pub fn begin(&mut self) {
        self.begin = (self.now)();
        (
            self.sent,
            self.first,
            self.last,
            self.next_begin,
            self.next_sent,
        ) = (None, None, None, None, None);
    }

    /// The provider request went out now. One that goes out after this call's output belongs to the next call.
    pub fn sent(&mut self) {
        if self.first.is_none() {
            self.sent = Some((self.now)());
        } else {
            self.next_sent = Some((self.now)());
        }
    }

    pub fn output(&mut self) {
        let at = (self.now)();
        self.first.get_or_insert(at);
        self.last = Some(at);
    }

    /// A tool result went back to the model now: the next call begins here.
    pub fn handed_back(&mut self) {
        if self.first.is_none() {
            self.begin = (self.now)(); // this call is still waiting for its first output: it begins now
        } else {
            self.next_begin = Some((self.now)());
        }
    }

    /// The call's usage report arrived: its timing; the next call begins now unless a tool result already went
    /// back (timing.py:90-109).
    pub fn done(&mut self) -> Dict {
        let end = (self.now)();
        let begin = self.begin;
        let sent = self.sent.map_or(begin, |s| s.max(begin));
        let first = self.first.map_or(end, |f| f.max(sent));
        let last = self.last.unwrap_or(end).max(first);
        let marks = [sent, first, last, end.max(last)].map(|t| ms(t - begin));
        let mut facts = Dict::new();
        facts.insert("by".to_string(), json!(self.by));
        if self.sent.is_some() {
            facts.insert("runtime_ms".to_string(), json!(marks[0]));
        }
        facts.insert("ttft_ms".to_string(), json!(marks[1] - marks[0]));
        facts.insert("generate_ms".to_string(), json!(marks[2] - marks[1]));
        facts.insert("tail_ms".to_string(), json!(marks[3] - marks[2]));
        facts.insert("total_ms".to_string(), json!(marks[3]));
        self.begin = self.next_begin.unwrap_or(end);
        self.sent = self.next_sent;
        (self.first, self.last, self.next_begin, self.next_sent) = (None, None, None, None);
        facts
    }
}
