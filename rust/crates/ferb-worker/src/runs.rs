//! What the worker shares with the supervisor about the model calls running now (worker.py:297-306: `_active`,
//! `_worker_runs`, `_tree_of`, `_stalled`, `_retrying`, `_connection`, `_tools_seen`, `_tools_offered`, `_live`,
//! `_last_steps`), and one run's connection lines (worker.py:128 `ConnectionNotice`).
//!
//! The supervisor reads these to stop a tree (`_interrupt`: its runs cancelled, its workers' tokens revoked), to
//! stop everything (`cancel_all`), to deliver a correction into a live run (`steering.apply`) and to know whether
//! one is pending (`correction_pending`), and opens a tree's live line while the tree runs.

use std::collections::{BTreeSet, HashMap};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use ferb_core::types::RunHandle;
use indexmap::IndexMap;
use serde_json::{json, Map, Value};
use tokio::sync::mpsc;

use crate::errors::{connection_lost, Error};
use crate::ports::{RunControl, WorkerHost, WorkerTree};

/// A started run: its runtime's name, its handle and its control.
#[derive(Clone)]
pub struct ActiveRun {
    pub runtime: String,
    pub handle: RunHandle,
    pub control: Arc<dyn RunControl>,
}

/// Where a run's connection lines go: a tree's live line, or a chat turn's stream.
pub type Tell = Arc<dyn Fn(Map<String, Value>) + Send + Sync>;

/// worker.py:128 `ConnectionNotice`: the first retry of an outage says so at once, further retries stay quiet, and
/// the run's next real event (or tool call) says once that it reconnected (fix wave 7).
pub struct ConnectionNotice {
    runtime: String,
    tell: Tell,
    task: bool,
    lost: AtomicBool,
}

impl ConnectionNotice {
    pub fn new(runtime: &str, tell: Tell, task: bool) -> Self {
        ConnectionNotice {
            runtime: runtime.to_string(),
            tell,
            task,
            lost: AtomicBool::new(false),
        }
    }

    /// Mark it lost already (a test's, or a notice restored by its owner).
    pub fn set_lost(&self, lost: bool) {
        self.lost.store(lost, Ordering::SeqCst);
    }

    pub fn retrying(&self, host: &dyn WorkerHost, message: &str) -> Result<(), Error> {
        if !self.lost.swap(true, Ordering::SeqCst) {
            let text = connection_lost(&self.runtime, message, self.task, &|| host.has_route());
            self.say(host, "lost", &text)?;
        }
        Ok(())
    }

    pub fn back(&self, host: &dyn WorkerHost) -> Result<(), Error> {
        if self.lost.swap(false, Ordering::SeqCst) {
            let text = format!("Reconnected to the AI service I use ({}).", self.runtime);
            self.say(host, "back", &text)?;
        }
        Ok(())
    }

    fn say(&self, host: &dyn WorkerHost, state: &str, text: &str) -> Result<(), Error> {
        let text = host.redact(&Value::String(text.to_string()))?;
        let now = ferb_clock::py::isoformat(&host.clock().now(), ferb_clock::py::Spec::Auto);
        let event =
            json!({"type": "progress", "text": text, "started_at": now, "connection": state});
        if let Value::Object(map) = event {
            (self.tell)(map);
        }
        Ok(())
    }
}

#[derive(Default)]
struct Inner {
    active: IndexMap<String, ActiveRun>,
    worker_runs: IndexMap<String, ActiveRun>,
    tree_of: HashMap<String, Arc<dyn WorkerTree>>,
    stalled: BTreeSet<String>,
    retrying: HashMap<String, (String, String)>,
    connection: HashMap<String, Arc<ConnectionNotice>>,
    tools_seen: HashMap<String, Vec<String>>,
    tools_offered: HashMap<String, bool>,
    live: HashMap<String, mpsc::UnboundedSender<Map<String, Value>>>,
    last_steps: HashMap<String, Value>,
}

/// The running calls, shared by the worker and the supervisor.
#[derive(Default)]
pub struct Runs {
    inner: Mutex<Inner>,
}

impl Runs {
    pub fn new() -> Self {
        Runs::default()
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    // --- for the supervisor ---

    /// `_active`: every started model call (worker or not), in start order.
    pub fn active(&self) -> Vec<ActiveRun> {
        self.lock().active.values().cloned().collect()
    }

    /// The started calls of these tasks (`_interrupt`: each is cancelled).
    pub fn active_of(&self, task_ids: &BTreeSet<String>) -> Vec<ActiveRun> {
        self.lock()
            .active
            .values()
            .filter(|r| task_ids.contains(&r.handle.task_id))
            .cloned()
            .collect()
    }

    /// `_worker_runs`: (worker id, run) of every worker run started now, in start order.
    pub fn worker_runs(&self) -> Vec<(String, ActiveRun)> {
        self.lock()
            .worker_runs
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    }

    /// `_worker_runs.get(worker_id)`.
    pub fn worker_run(&self, worker_id: &str) -> Option<ActiveRun> {
        self.lock().worker_runs.get(worker_id).cloned()
    }

    /// `_tree_of.get(task_id)`.
    pub fn tree_of(&self, task_id: &str) -> Option<Arc<dyn WorkerTree>> {
        self.lock().tree_of.get(task_id).cloned()
    }

    /// `_tree_of[task_id] = tree` (the supervisor also records trees it runs without a worker).
    pub fn set_tree(&self, task_id: &str, tree: Arc<dyn WorkerTree>) {
        self.lock().tree_of.insert(task_id.to_string(), tree);
    }

    /// `_tree_of.pop(task_id)`.
    pub fn forget_tree(&self, task_id: &str) {
        self.lock().tree_of.remove(task_id);
    }

    /// Every (task id, tree) known (`_tree_of.items()`).
    pub fn trees(&self) -> Vec<(String, Arc<dyn WorkerTree>)> {
        self.lock()
            .tree_of
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    }

    /// `_live[tree_id] = queue`: the tree's live line while it runs.
    pub fn open_live(&self, tree_id: &str) -> mpsc::UnboundedReceiver<Map<String, Value>> {
        let (tx, rx) = mpsc::unbounded_channel();
        self.lock().live.insert(tree_id.to_string(), tx);
        rx
    }

    pub fn close_live(&self, tree_id: &str) {
        self.lock().live.remove(tree_id);
    }

    /// `_last_steps[tree_id]`: the tree's last tool step, as its live line said it (ORCH-595).
    pub fn last_step(&self, tree_id: &str) -> Option<Value> {
        self.lock().last_steps.get(tree_id).cloned()
    }

    pub fn forget_last_step(&self, tree_id: &str) {
        self.lock().last_steps.remove(tree_id);
    }

    /// `_connection.get(worker_id)`.
    pub fn connection(&self, worker_id: &str) -> Option<Arc<ConnectionNotice>> {
        self.lock().connection.get(worker_id).cloned()
    }

    /// `_connection[worker_id] = notice`.
    pub fn set_connection(&self, worker_id: &str, notice: Arc<ConnectionNotice>) {
        self.lock().connection.insert(worker_id.to_string(), notice);
    }

    /// What is left of the bookkeeping (for tests: after a run ends, only `tree_of` and `last_steps` stay).
    pub fn left(&self) -> Value {
        let i = self.lock();
        let mut tree_of: Vec<&String> = i.tree_of.keys().collect();
        tree_of.sort();
        let mut active: Vec<&String> = i.active.keys().collect();
        active.sort();
        let mut worker_runs: Vec<&String> = i.worker_runs.keys().collect();
        worker_runs.sort();
        let mut connection: Vec<&String> = i.connection.keys().collect();
        connection.sort();
        json!({
            "active": active,
            "worker_runs": worker_runs,
            "connection": connection,
            "tools_seen": i.tools_seen.iter().collect::<std::collections::BTreeMap<_, _>>(),
            "tools_offered": i.tools_offered.iter().collect::<std::collections::BTreeMap<_, _>>(),
            "stalled": i.stalled.iter().collect::<Vec<_>>(),
            "retrying": i.retrying.iter().map(|(k, (a, b))| (k.clone(), json!([a, b]))).collect::<std::collections::BTreeMap<_, _>>(),
            "tree_of": tree_of,
            "last_steps": i.last_steps.iter().collect::<std::collections::BTreeMap<_, _>>(),
        })
    }

    // --- the worker's own bookkeeping ---

    pub(crate) fn add_active(&self, run: ActiveRun) {
        self.lock().active.insert(run.handle.id.clone(), run);
    }

    pub(crate) fn remove_active(&self, run_id: &str) {
        self.lock().active.shift_remove(run_id);
    }

    pub(crate) fn add_worker_run(&self, worker_id: &str, run: ActiveRun) {
        self.lock().worker_runs.insert(worker_id.to_string(), run);
    }

    pub(crate) fn remove_worker_run(&self, worker_id: &str) {
        self.lock().worker_runs.shift_remove(worker_id);
    }

    pub(crate) fn remove_connection(&self, worker_id: &str) {
        self.lock().connection.remove(worker_id);
    }

    pub(crate) fn mark_stalled(&self, watch_id: &str) {
        self.lock().stalled.insert(watch_id.to_string());
    }

    pub(crate) fn is_stalled(&self, watch_id: &str) -> bool {
        self.lock().stalled.contains(watch_id)
    }

    pub(crate) fn unmark_stalled(&self, watch_id: &str) {
        self.lock().stalled.remove(watch_id);
    }

    pub(crate) fn set_retrying(&self, worker_id: &str, runtime: &str, message: &str) {
        self.lock().retrying.insert(
            worker_id.to_string(),
            (runtime.to_string(), message.to_string()),
        );
    }

    pub(crate) fn take_retrying(&self, worker_id: &str) -> Option<(String, String)> {
        self.lock().retrying.remove(worker_id)
    }

    pub(crate) fn retrying_of(&self, worker_id: &str) -> Option<(String, String)> {
        self.lock().retrying.get(worker_id).cloned()
    }

    pub(crate) fn set_tools_seen(&self, worker_id: &str, seen: Vec<String>) {
        self.lock().tools_seen.insert(worker_id.to_string(), seen);
    }

    pub(crate) fn take_tools_seen(&self, worker_id: &str) -> Option<Vec<String>> {
        self.lock().tools_seen.remove(worker_id)
    }

    pub(crate) fn set_tools_offered(&self, worker_id: &str, offered: bool) {
        self.lock()
            .tools_offered
            .insert(worker_id.to_string(), offered);
    }

    pub(crate) fn take_tools_offered(&self, worker_id: &str) -> bool {
        self.lock().tools_offered.remove(worker_id).unwrap_or(false)
    }

    pub(crate) fn live_sender(
        &self,
        tree_id: &str,
    ) -> Option<mpsc::UnboundedSender<Map<String, Value>>> {
        self.lock().live.get(tree_id).cloned()
    }

    pub(crate) fn set_last_step(&self, tree_id: &str, step: Value) {
        self.lock().last_steps.insert(tree_id.to_string(), step);
    }
}
