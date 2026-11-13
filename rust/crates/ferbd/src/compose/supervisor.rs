//! The supervisor's place in the composition, kept apart until it lands.
//!
//! `rust/crates/ferb-supervisor` is being finished by two other agents (its `flow::Daemon` seam is on disk; the
//! daemon-side `chat_rpc::register` is not yet). Until it is wired here, the daemon builds and runs without it:
//! every method that needs it answers one plain refusal, [`NOT_IN_THIS_BUILD`], as one of FERB's own errors (a
//! refusal, never an internal error), and the seams of other areas that would call it ([`NoSupervisor`]) answer
//! with that refusal or the inert value Python's daemon has before its supervisor is built.
//!
//! What wiring it will take (the coordinator's list): implement `ferb_supervisor::flow::Daemon` over this
//! composition's gateway, run tokens, environment, desktop, browser, memory and schedules; give it
//! `runtimes::ModelFactory` (compose/runtimes.rs) as its `make_runtime`; register its chat RPCs in place of
//! [`register`]; and replace [`NoSupervisor`] in group A's `Tasks` and group B's `runtimes::Supervisor`.

use std::collections::BTreeSet;

use ferb_clock::schedule_store::ClaimedRun;
use ferb_clock::schedules::Schedule;
use ferb_daemon::areas_a::seams::RunFailed;
use ferb_daemon::areas_a::{LocalFuture, Tasks};
use ferb_rpc::{HandlerError, Server};
use serde_json::{Map, Value};

use crate::basics::SharedDb;

/// The refusal every supervisor method answers in this build.
pub const NOT_IN_THIS_BUILD: &str =
    "not available in this build: FERB's planner (the supervisor) isn't part of \
this Rust daemon yet";

/// The methods that need the supervisor (`areas/chat.py`, `areas/rpc.py`'s task handlers).
pub const UNARY: &[&str] = &[
    "chat.status",
    "chat.attach",
    "chat.answer",
    "chat.approve",
    "chat.correct",
    "results.ack",
    "task.stop",
    "tasks.list",
];
pub const STREAMS: &[&str] = &["chat.send"];

pub fn refusal() -> HandlerError {
    HandlerError::refusal("FerbError", NOT_IN_THIS_BUILD)
}

/// The refusals in place of the supervisor's RPCs.
pub fn register(server: &Server) {
    for m in UNARY {
        server.register_unary(m, |_s, _p: Map<String, Value>| async { Err(refusal()) });
    }
    for m in STREAMS {
        server.register_stream(m, |_s, _p: Map<String, Value>| {
            let (tx, rx) = tokio::sync::mpsc::channel(1);
            let _ = tx.try_send(Err(refusal()));
            Box::new(rx) as Box<dyn ferb_rpc::server::EventStream>
        });
    }
}

/// Group A's `Tasks` seam without a supervisor: no run is under way, scheduled runs refuse (the run is redelivered
/// once a build has the supervisor: the timer stays unhandled), and the task store is read and written directly.
pub struct NoSupervisor {
    pub db: SharedDb,
    pub now: std::sync::Arc<dyn Fn() -> String + Send + Sync>,
}

impl Tasks for NoSupervisor {
    fn schedule_running(&self, _schedule_id: &str) -> bool {
        false
    }

    fn create_scheduled(
        &self,
        _conn: &rusqlite::Connection,
        _run: &ClaimedRun,
        _late: bool,
    ) -> Result<Value, String> {
        Err(NOT_IN_THIS_BUILD.into())
    }

    fn run_scheduled(
        &self,
        _schedule: Schedule,
        _created: Value,
        _late: bool,
        _overdue_s: f64,
        _retry: bool,
    ) -> LocalFuture<Result<Map<String, Value>, RunFailed>> {
        Box::pin(async {
            Err(RunFailed {
                described: format!("FerbError: {NOT_IN_THIS_BUILD}"),
                reason: NOT_IN_THIS_BUILD.into(),
            })
        })
    }

    fn turn_running(&self) -> bool {
        false
    }

    fn running(&self) -> bool {
        false
    }

    fn mention_paused(&self) -> LocalFuture<()> {
        Box::pin(async {})
    }

    fn task_column(&self, task_id: &str, column: &str) -> Option<String> {
        let db = crate::basics::lock(&self.db);
        let now = self.now.clone();
        let tree = ferb_orchestrator::tree::TaskTree::new(Some(&db), Box::new(move || now()));
        match tree.column(task_id, column).ok().flatten()? {
            Value::Null => None,
            Value::String(s) => Some(s),
            other => Some(ferb_py::value::py_str(&other)),
        }
    }

    fn set_task_status(&self, task_id: &str, status: &str, outcome: &Value) -> Result<(), String> {
        let db = crate::basics::lock(&self.db);
        let now = self.now.clone();
        let tree = ferb_orchestrator::tree::TaskTree::new(Some(&db), Box::new(move || now()));
        tree.set_status(task_id, status, Some(outcome), "")
            .map_err(|e| e.to_string())
    }

    fn forget_pending(&self, _texts: &[String]) {}

    fn forget_trail_copies(&self, _trail: &BTreeSet<String>) {}
}

/// Group B's runtimes seam without a supervisor: no task waits on a runtime switch.
pub struct NoSwitch;

impl ferb_daemon::areas_b::runtimes::Supervisor for NoSwitch {
    fn runtime_offer(&self, _task_id: &str) -> Option<Value> {
        None
    }

    fn answer_runtime_switch(&self, _task_id: &str, _runtime: &str) -> Result<Value, HandlerError> {
        Err(refusal())
    }
}
