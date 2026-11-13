//! The daemon's side of memory import (kb area memory): the RPCs the app calls, the one background job, its resume
//! after a restart and its cancel. Port of `ferb/daemon/areas/memory_import.py`. Must not run an import on the owner's
//! turn, or keep what it read.

use std::rc::Rc;

use ferb_learning::distill::Notice;
use ferb_learning::import_job::{self, ImportJob};
use ferb_rpc::server::HandlerResult;
use ferb_rpc::wire::Params;
use serde_json::{json, Value};

use super::{learning_error, ok, Handler, State};
use crate::shared::{refusal, text};

/// `MemorySearch.facts` over the memory items, as `_memory_item_search` ranks them (no query vector: the local model
/// is the embedding loop's, not started here).
fn similar(st: &State, query: &str) -> Vec<String> {
    struct Now<'a>(&'a State);
    impl ferb_memory::search::Clock for Now<'_> {
        fn now(&self) -> ferb_memory::times::DateTime {
            super::to_py(&self.0.now())
        }
    }
    let conn = st.db().conn();
    let clock = Now(st);
    let search = ferb_memory::search::MemorySearch::new(conn, &clock, None);
    search
        .facts(
            &ferb_memory::search::StoreFacts(conn),
            query,
            None,
            "",
            Some(super::memory::MEMORY_LEARNED_MOST),
            &[],
        )
        .map(|hits| hits.into_iter().map(|h| h.data.id).collect())
        .unwrap_or_default()
}

struct Seams(std::rc::Weak<State>);

impl import_job::Seams for Seams {
    fn similar(&self, query: &str) -> Vec<String> {
        self.0
            .upgrade()
            .map(|st| similar(&st, query))
            .unwrap_or_default()
    }

    /// `_import_busy`: the owner's chat turn is under way (its planner call is the critical path).
    fn busy(&self) -> bool {
        self.0.upgrade().is_some_and(|st| st.tasks.turn_running())
    }

    fn sleep(&self, seconds: f64) -> ferb_core::interfaces::BoxFuture<'static, ()> {
        Box::pin(tokio::time::sleep(std::time::Duration::from_secs_f64(
            seconds.max(0.0),
        )))
    }
}

/// `_import_job`: built per use, late-bound (each reads the daemon's current runtime and state).
pub fn job(st: &Rc<State>) -> ImportJob<'static> {
    let make = st.make_runtime.clone();
    let (w1, w2, w3) = (Rc::downgrade(st), Rc::downgrade(st), Rc::downgrade(st));
    let redactor = st.redactor.clone();
    ImportJob {
        memory: st.memory.clone(),
        store: st.store.clone(),
        journal: st.journal.clone(),
        make_runtime: Box::new(move |role, mode, tools, schema| match &make {
            Some(m) => m(role, mode, tools, schema),
            None => Err(ferb_learning::Error::Runtime(
                "no model runtime is set up for the import".into(),
            )),
        }),
        prompts: st.prompts.clone(),
        redact: Box::new(move |t: &str| redactor.text(t)),
        publish: Box::new(move |topic: &str, data: Value| {
            if let Some(st) = w1.upgrade() {
                st.publish(topic, data);
            }
        }),
        tell: Box::new(move |n: Notice| {
            if let Some(st) = w2.upgrade() {
                let fut = st
                    .owner
                    .tell_owner(&n.id, n.refs, &n.text, &n.origin, n.cards);
                tokio::task::spawn_local(fut);
            }
        }),
        seams: Box::new(Seams(w3)),
        ids: Rc::new(super::LearnIds(st.ids.clone())),
        spans: None,
        usage: Box::new(|_, _| {}),
        device_id: st.device_id(),
        home: Some(st.user_home.clone()),
        unzstd: None,
        idle_poll: import_job::IDLE_POLL_S,
        chunk_chars: ferb_memory::import_extract::CHUNK_CHARS,
    }
}

/// `_resume_imports`: at start, a run a restart interrupted goes on from its last finished chunk; one job at a time.
pub fn resume(st: &Rc<State>) {
    let j = job(st);
    for run_id in j.running().unwrap_or_default() {
        let busy = st
            .import_task
            .borrow()
            .as_ref()
            .is_some_and(|t| !t.is_finished());
        if busy {
            let _ = j.end(&run_id, "failed", "another import was running");
        } else {
            spawn_run(st, run_id);
        }
    }
}

fn spawn_run(st: &Rc<State>, run_id: String) {
    let me = st.clone();
    let task = tokio::task::spawn_local(async move { run_import(&me, &run_id).await });
    *st.import_task.borrow_mut() = Some(task);
}

/// `_run_import`: FERB's own failure ends the run (never resumed in a loop), told once.
async fn run_import(st: &Rc<State>, run_id: &str) {
    let j = job(st);
    if let Err(e) = j.run(run_id).await {
        let why: String = st.redact_text(&e.described()).chars().take(300).collect();
        let _ = j.end(run_id, "failed", &why);
    }
    let _ = j.tell_done(run_id);
}

fn import_params(params: &Params) -> Result<(String, String), ferb_rpc::server::HandlerError> {
    let (source, path) = (text(params, "source"), text(params, "path"));
    import_job::check(&source, &path).map_err(|e| refusal(e.to_string()))?;
    Ok((source, path))
}

pub(crate) fn handlers() -> Vec<(&'static str, Handler)> {
    vec![
        ("memory.import.estimate", |s, p| Box::pin(estimate(s, p))),
        ("memory.import.start", |s, p| Box::pin(start(s, p))),
        ("memory.import.status", |s, p| Box::pin(status(s, p))),
        ("memory.import.cancel", |s, p| Box::pin(cancel(s, p))),
    ]
}

fn unreadable(e: ferb_learning::Error) -> ferb_rpc::server::HandlerError {
    use ferb_learning::Error as E;
    match e {
        E::ImportUnreadable(m) | E::ImportBusy(m) => refusal(m),
        E::Crash("OSError", m) => refusal(m),
        other => learning_error(other),
    }
}

/// `memory.import.estimate {source, path?}`: the dry run (nothing is sent anywhere).
async fn estimate(st: Rc<State>, params: Params) -> HandlerResult {
    let (source, path) = import_params(&params)?;
    ok(job(&st).estimate(&source, &path).map_err(unreadable)?)
}

/// `memory.import.start {source, path?}`: the import in the background; the answer carries the estimate.
async fn start(st: Rc<State>, params: Params) -> HandlerResult {
    let (source, path) = import_params(&params)?;
    let j = job(&st);
    let estimate = j.estimate(&source, &path).map_err(unreadable)?;
    let run_id = j.create(&source, &path).map_err(unreadable)?;
    st.import_cancel.borrow_mut().clear();
    spawn_run(&st, run_id.clone());
    ok(json!({"run": run_id, "estimate": estimate}))
}

/// `memory.import.status {}`.
async fn status(st: Rc<State>, _params: Params) -> HandlerResult {
    ok(job(&st).status().map_err(learning_error)?)
}

/// `memory.import.cancel {}`: stops the running import after its current call; what it saved stays.
async fn cancel(st: Rc<State>, _params: Params) -> HandlerResult {
    let j = job(&st);
    let running = j.running().map_err(learning_error)?;
    let Some(first) = running.first().cloned() else {
        return ok(json!({"cancelled": null}));
    };
    *st.import_cancel.borrow_mut() = first.clone();
    let task = st.import_task.borrow_mut().take();
    match task {
        Some(t) if !t.is_finished() => {
            t.abort();
            let _ = t.await;
            // the owner's cancel (a shutdown leaves it running, to resume)
            j.end(&first, "cancelled", "").map_err(learning_error)?;
        }
        _ => j.end(&first, "cancelled", "").map_err(learning_error)?,
    }
    ok(json!({"cancelled": first, "status": j.row(&first).map_err(learning_error)?}))
}
