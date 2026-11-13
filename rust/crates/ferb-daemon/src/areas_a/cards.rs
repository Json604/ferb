//! Episode cards (L28; `areas/memory.py` `_capture_learning_episode`, `_capture_card`, `_read_card`): a finished
//! task's card after its outcome, and the hourly re-capture of cards whose inputs changed, one at a time.
//!
//! The card reader. Python reads every card in a process of its own (`ferb.memory.card_reader`) because the lineage
//! resolution is CPU-bound and in a thread it would hold the interpreter's lock against the event loop; MEM-418 makes
//! that reader open only the database of the FERB home it finds itself. The Rust daemon has no such lock: the reader
//! runs in-process, on a blocking thread off the daemon's loop, on a connection of its own to this daemon's own
//! database with `PRAGMA query_only` (it can't write), never a path a caller names, which keeps MEM-418's boundary.
//! The write stays on the loop (`Learning.write_episode`). A card input that changed while the reader read (a forget,
//! a verdict, a card written: `Learning.generation`) makes it read again.

use std::rc::Rc;

use ferb_learning::learning::Learning;
use ferb_learning::store::Store;
use serde_json::{json, Map, Value};

use super::{LearnClock, LearningSeams, State, ZoneOf};

/// Why a card wasn't captured.
#[derive(Debug)]
pub enum CardError {
    /// `Deferred`: FERB is stopping; the card is captured again after the next start.
    Deferred,
    /// `LearningRefused`: the card can't be built (the reader's words).
    Refused(String),
    /// Anything else the store raised.
    Failed(ferb_learning::Error),
}

impl From<ferb_learning::Error> for CardError {
    fn from(e: ferb_learning::Error) -> CardError {
        match e {
            ferb_learning::Error::LearningRefused(m) => CardError::Refused(m),
            other => CardError::Failed(other),
        }
    }
}

impl std::fmt::Display for CardError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CardError::Deferred => {
                f.write_str("FERB is stopping; this card is captured again after the next start")
            }
            CardError::Refused(m) => f.write_str(m),
            CardError::Failed(e) => write!(f, "{e}"),
        }
    }
}

/// `_app_versions`: the device profile's apps, bundle id to version ({} without a profile).
pub fn app_versions(st: &State) -> Map<String, Value> {
    let id = match st.config_get("device", "id") {
        Some(v) if ferb_py::value::truthy(&v) => ferb_py::value::py_str(&v),
        _ => return Map::new(),
    };
    match ferb_device::profile::ProfileStore::new(&st.home).load(&id) {
        Ok(Some(p)) => p
            .apps
            .iter()
            .map(|a| (a.bundle_id.clone(), json!(a.version)))
            .collect(),
        _ => Map::new(),
    }
}

/// `_capture_learning_episode`: a finished task's card, after its outcome (`Supervisor.on_episode`). Failures are
/// journaled; a capture deferred by the shutdown is left to the next start's hourly pass.
pub async fn capture_learning_episode(st: &Rc<State>, task_id: &str) {
    let apps = app_versions(st);
    match capture_card(st, task_id, Some(apps), true).await {
        Ok(_) | Err(CardError::Deferred) => {}
        Err(e) => {
            let _ = st.journal(
                "memory.episode_failed",
                json!({"task_id": task_id, "error": st.redact_text(&e.to_string())}),
                None,
            );
        }
    }
}

/// `_capture_card`: capture (or refresh, `apps` None) a task's episode card. `report`: journal the refs skipped as
/// damaged by redaction or expired by retention, and a card skipped as built on forgotten content.
pub async fn capture_card(
    st: &Rc<State>,
    task_id: &str,
    apps: Option<Map<String, Value>>,
    report: bool,
) -> Result<Option<Value>, CardError> {
    // GATE-550: nothing is learned from a test run's task
    if ferb_orchestrator::test_runs::TestRuns::new(&st.store.db)
        .task(Some(task_id))
        .unwrap_or(false)
    {
        return Ok(None);
    }
    let lock = st.card_lock.clone();
    let _one = lock.lock().await;
    let prepared = loop {
        let generation = st.learning.generation.get();
        let prepared = read_card(st, task_id, apps.clone()).await?;
        if generation == st.learning.generation.get() {
            break prepared;
        }
    };
    if report {
        if let Some(p) = &prepared {
            for (kind, key) in [
                ("memory.episode_refs_damaged", "damaged"),
                ("memory.episode_refs_expired", "expired"),
            ] {
                if p.get(key).is_some_and(ferb_py::value::truthy) {
                    let _ = st.journal(kind, json!({"task_id": task_id, "refs": p[key]}), None);
                }
            }
            if p.get("forgotten").is_some_and(ferb_py::value::truthy) {
                let _ = st.journal(
                    "memory.episode_skipped",
                    json!({"task_id": task_id, "why": "built on forgotten content"}),
                    None,
                );
            }
        }
    }
    Ok(st.learning.write_episode(prepared.as_ref())?)
}

/// `_read_card`: every read of a card (`Learning.prepare_episode`), off the loop on a query-only connection to this
/// daemon's own database. Once the daemon is stopping, no reader starts.
async fn read_card(
    st: &Rc<State>,
    task_id: &str,
    apps: Option<Map<String, Value>>,
) -> Result<Option<Value>, CardError> {
    if st.shutting_down.get() {
        return Err(CardError::Deferred);
    }
    let path = st.db().path().to_path_buf();
    let clock = st.clock.clone();
    let task = task_id.to_string();
    let read = tokio::task::spawn_blocking(move || -> Result<Option<Value>, String> {
        let db = ferb_store::Db::open(&path).map_err(|e| format!("OperationalError: {e}"))?;
        db.conn()
            .execute_batch("PRAGMA query_only=ON")
            .map_err(|e| format!("OperationalError: {e}"))?;
        let reader = Learning::new(
            Store::new(Rc::new(db)),
            Rc::new(LearnClock(clock.clone())),
            Rc::new(ZoneOf(clock.zone())),
            Box::new(|_: &str, _: Value| {}),
            Box::new(|t: &str| t.to_string()),
            Box::new(LearningSeams),
        );
        reader
            .prepare_episode(&task, apps.as_ref())
            .map_err(|e| format!("{}: {e}", e.class()))
    })
    .await;
    match read {
        Ok(Ok(prepared)) => Ok(prepared),
        Ok(Err(why)) => Err(CardError::Refused(why)),
        Err(_) if st.shutting_down.get() => Err(CardError::Deferred),
        Err(_) => Err(CardError::Refused(
            "the card reader ended without an answer".into(),
        )),
    }
}
