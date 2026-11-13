//! Shared by the fixture replays: reading `rust/fixtures/clock/*.json` and comparing values.

#![allow(dead_code)]

use std::path::Path;

use ferb_clock::py::{self, Time};
use serde_json::{json, Value};

pub fn fixture(name: &str) -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/clock")
        .join(name);
    serde_json::from_str(&std::fs::read_to_string(path).expect("the fixture (run make.py)"))
        .expect("fixture JSON")
}

pub fn time(v: &Value) -> Time {
    py::parse(v.as_str().expect("a time")).expect("an aware ISO time")
}

pub fn opt_time(v: &Value) -> Option<Time> {
    v.as_str().map(|s| py::parse(s).expect("an aware ISO time"))
}

pub fn iso(t: &Option<Time>) -> Value {
    t.as_ref().map_or(Value::Null, |t| json!(py::iso(t)))
}

pub fn error(kind: &str, message: &str) -> Value {
    json!({"error": kind, "message": message})
}

/// Collects differences instead of stopping at the first, so one run shows them all.
#[derive(Default)]
pub struct Diffs {
    pub cases: usize,
    pub found: Vec<String>,
}

impl Diffs {
    pub fn check(&mut self, what: &str, want: &Value, got: &Value) {
        self.cases += 1;
        if !same(want, got) {
            self.found
                .push(format!("{what}\n  python: {want}\n  rust:   {got}"));
        }
    }

    pub fn finish(self, name: &str) {
        eprintln!(
            "{name}: {} comparisons, {} differ",
            self.cases,
            self.found.len()
        );
        if !self.found.is_empty() {
            let shown: Vec<&String> = self.found.iter().take(25).collect();
            panic!(
                "{name}: {} of {} differ:\n{}",
                self.found.len(),
                self.cases,
                shown
                    .iter()
                    .map(|s| s.as_str())
                    .collect::<Vec<_>>()
                    .join("\n")
            );
        }
    }
}

/// JSON equality with numbers compared as numbers (Python writes `3600.0`, Rust may hold an integer).
pub fn same(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => x.as_f64() == y.as_f64(),
        (Value::Array(x), Value::Array(y)) => {
            x.len() == y.len() && x.iter().zip(y).all(|(a, b)| same(a, b))
        }
        (Value::Object(x), Value::Object(y)) => {
            x.len() == y.len()
                && x.iter()
                    .zip(y)
                    .all(|((ka, va), (kb, vb))| ka == kb && same(va, vb))
        }
        _ => a == b,
    }
}

/// A tool call's context as the Python tests build it.
pub fn tool_ctx(
    identity: ferb_core::types::Identity,
    scopes: &[&str],
    workspace: &str,
    span: Option<&str>,
) -> ferb_core::types::ToolContext {
    ferb_core::types::ToolContext {
        task_id: "t1".into(),
        worker_id: "w1".into(),
        request_id: "r1".into(),
        identity,
        scopes: scopes.iter().map(|s| s.to_string()).collect(),
        device_id: "d".into(),
        span_id: span.map(str::to_string),
        workspace: workspace.into(),
        owner_roots: vec![],
        untrusted_roots: vec![],
        intent_id: None,
        observe_only: false,
        probe: false,
        path_closed: None,
        escape: None,
        terminal: None,
        folders: vec![],
    }
}

/// A future that is pending once (asking to be polled again), then ready: a rest that yields to the caller, as
/// Python's `await asyncio.sleep(0)` does.
pub struct YieldOnce(pub bool);

impl std::future::Future for YieldOnce {
    type Output = ();
    fn poll(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<()> {
        if self.0 {
            std::task::Poll::Ready(())
        } else {
            self.0 = true;
            cx.waker().wake_by_ref();
            std::task::Poll::Pending
        }
    }
}
