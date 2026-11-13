//! What the worker reads of the supervisor (worker.py is the supervisor's mixin, `WorkerRuns`): ferb-worker's
//! `WorkerHost` and `WorkerTree`, answered from the supervisor's stores, its trees and its seams.

use std::collections::BTreeSet;
use std::sync::{Arc, Weak};

use ferb_clock::clock::ZonedClock;
use ferb_clock::py::Time;
use ferb_context::assemble::{Assembly, ContextBlock};
use ferb_core::types::{Identity, ModelImage, Task};
use ferb_orchestrator::ports::Policy;
use ferb_orchestrator::steering::Steering;
use ferb_worker::ports::{Grant, Liveness, ModelRuntime, TaskRecord, WorkerHost, WorkerTree};
use ferb_worker::Error;
use serde_json::{json, Map, Value};

use super::{Inner, Tree};

/// The worker's host: the supervisor, held weakly (the supervisor holds the worker).
pub struct Host {
    pub sup: Weak<Inner>,
    pub clock: Arc<dyn ZonedClock>,
    pub policy: Arc<dyn Policy>,
    /// The supervisor's liveness record, shared (the worker reads it through its host).
    pub activity: Arc<ferb_orchestrator::guards::Activity>,
}

impl Host {
    fn sup(&self) -> Arc<Inner> {
        self.sup.upgrade().expect("the supervisor")
    }
}

impl WorkerHost for Host {
    fn clock(&self) -> &dyn ZonedClock {
        self.clock.as_ref()
    }
    fn new_id(&self) -> String {
        self.sup().new_id()
    }
    fn device_id(&self) -> String {
        self.sup().deps.device_id.clone()
    }
    fn has_route(&self) -> Option<bool> {
        self.sup().deps.daemon.has_route()
    }
    fn policy(&self) -> &dyn Policy {
        self.policy.as_ref()
    }
    fn make_runtime(
        &self,
        role: &str,
        mode: &str,
        tool_names: &[String],
        schema: &Value,
        effort: Option<&str>,
    ) -> Result<Arc<dyn ModelRuntime>, Error> {
        self.sup()
            .deps
            .daemon
            .make_runtime(role, mode, tool_names, schema, effort)
    }
    fn tools_for(&self, grant: &Grant) -> Vec<Value> {
        self.sup().deps.daemon.tools_for(grant)
    }
    fn redact(&self, value: &Value) -> Result<Value, Error> {
        Ok(self.sup().redact(value))
    }
    fn initial_image_given(&self, task_id: &str) {
        self.sup().deps.daemon.initial_image_given(task_id)
    }
    fn forget_calls(&self, worker_id: &str) {
        self.sup().deps.daemon.forget_calls(worker_id)
    }
    fn calls_refused(&self, watch_id: &str) -> u64 {
        self.sup().deps.daemon.calls_refused(watch_id)
    }
    fn issue_token(
        &self,
        task_id: &str,
        worker_id: &str,
        scopes: &BTreeSet<String>,
        identity: Identity,
        ttl_s: f64,
        observe_only: bool,
    ) -> String {
        self.sup().deps.daemon.issue_token(
            task_id,
            worker_id,
            scopes,
            identity.as_str(),
            ttl_s,
            observe_only,
            None,
            None,
        )
    }
    fn validate_token(&self, token: &str) -> Result<Grant, Error> {
        self.sup()
            .deps
            .daemon
            .validate_token(token)
            .ok_or_else(|| Error::other("AuthError", "invalid or expired run token"))
    }
    fn revoke_worker(&self, worker_id: &str) {
        self.sup().deps.daemon.revoke_worker(worker_id)
    }
    fn span_start(
        &self,
        kind: &str,
        task_id: Option<&str>,
        parent_id: Option<&str>,
        device_id: Option<&str>,
        input: Map<String, Value>,
    ) -> String {
        self.sup()
            .span_start(kind, task_id, parent_id, device_id, Value::Object(input))
    }
    fn span_finish(
        &self,
        span_id: &str,
        output: Option<Map<String, Value>>,
        error: Option<&str>,
        cost_usd: Option<Value>,
    ) {
        self.sup().span_finish(
            span_id,
            output.map(Value::Object),
            error,
            None,
            cost_usd.and_then(|c| c.as_f64()),
        )
    }
    fn span_update_input(&self, span_id: &str, values: Map<String, Value>) {
        self.sup()
            .span_update_input(span_id, &Value::Object(values))
    }
    fn task_known(&self, task_id: &str) -> bool {
        self.sup().task_known(task_id)
    }
    fn task_add(&self, record: &TaskRecord, status: &str) {
        self.sup()
            .task_add(&record.task, &record.span_id, &record.tree_id, status)
    }
    fn task_set_span(&self, task_id: &str, span_id: &str) {
        self.sup().task_set_span(task_id, span_id)
    }
    fn set_status(&self, task_id: &str, status: &str, extra: Map<String, Value>) {
        self.sup()
            .set_status(task_id, status, Some(&Value::Object(extra)))
    }
    fn publish(&self, topic: &str, data: Map<String, Value>) {
        self.sup().deps.daemon.publish(topic, Value::Object(data))
    }
    fn journal(&self, kind: &str, data: Value) {
        self.sup().journal(kind, data, None);
    }
    fn activity(&self) -> &dyn Liveness {
        self.activity.as_ref()
    }
    fn signals(&self) -> (Option<Time>, Option<Time>) {
        self.sup().deps.daemon.signals()
    }
    fn browser_context(&self, task_id: &str) -> String {
        self.sup().deps.daemon.browser_context(task_id)
    }
    fn apps(&self) -> Result<Vec<(String, String)>, Error> {
        Ok(self.sup().deps.daemon.apps())
    }
    fn running_apps(&self) -> Vec<(String, String, i64)> {
        self.sup().deps.daemon.running_apps()
    }
    fn shell_process(&self, id: &str) -> Result<Option<Map<String, Value>>, Error> {
        Ok(self.sup().deps.daemon.shell_process(id))
    }
    fn on_violation(&self, runtime: &str, text: &str) {
        self.sup().deps.daemon.on_violation(runtime, text)
    }
    fn runtime_account(&self, runtime: &str, account: &str) {
        self.sup().deps.daemon.runtime_account(runtime, account)
    }
    fn system(&self, prompt: &str, roles: &[String]) -> String {
        let roles: Vec<&str> = roles.iter().map(String::as_str).collect();
        self.sup().system_for(prompt, true, &roles)
    }
    fn with_newly_always_on(&self, ctx: Vec<ContextBlock>) -> Vec<ContextBlock> {
        let pending = self.sup().deps.daemon.identity_pending();
        crate::planner::with_newly_always_on(ctx, &pending)
    }
    fn context_exposure(
        &self,
        task: &Task,
        ctx: &[ContextBlock],
        assembly: &Assembly,
        tree: Option<&dyn WorkerTree>,
    ) {
        self.sup()
            .context_exposure(task, ctx, assembly, tree.map(|t| t.id()))
    }
}

impl WorkerTree for Tree {
    fn id(&self) -> String {
        self.id.clone()
    }
    fn worker_task_id(&self) -> String {
        self.worker.id.clone()
    }
    fn stop(&self) -> String {
        self.lock().stop.clone()
    }
    fn runtime(&self) -> String {
        self.lock().runtime.clone()
    }
    fn worker_type(&self) -> String {
        self.lock()
            .spec
            .as_ref()
            .map(|s| s.worker_type.clone())
            .unwrap_or_default()
    }
    fn start_block(&self) -> Option<ContextBlock> {
        self.lock().start_block.clone()
    }
    fn start_image(&self) -> Option<ModelImage> {
        self.lock().start_image.clone()
    }
    fn set_result_revision(&self, revision: usize) {
        *self
            .result_revision
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = revision;
    }
    fn steering(&self) -> &tokio::sync::Mutex<Steering> {
        &self.steering
    }
}

impl Inner {
    /// supervisor.py:1230 `_system`: a role's instructions, read from the seams at the moment it assembles.
    pub fn system_for(&self, name: &str, live: bool, roles: &[&str]) -> String {
        let inputs = self.deps.daemon.system_inputs(live);
        crate::prompts::system(&self.deps.prompts, Some(name), live, roles, &inputs)
            .unwrap_or_else(|e| panic!("the prompt package: {e}"))
    }

    /// supervisor.py:6983 `_context_exposure`: only bytes actually handed to a model become reads.
    pub fn context_exposure(
        &self,
        task: &Task,
        blocks: &[ContextBlock],
        assembly: &Assembly,
        tree: Option<String>,
    ) {
        let mut records: Vec<Option<Map<String, Value>>> = Vec::new();
        let mut history: Vec<&ContextBlock> = Vec::new();
        let given = blocks.iter().filter(|b| !b.text.is_empty());
        for (block, shown) in given.zip(&assembly.shown) {
            if !*shown || block.origin.is_empty() {
                continue;
            }
            if block.history {
                if history.is_empty() {
                    records.push(None);
                }
                history.push(block);
                continue;
            }
            let mut record = Map::new();
            record.insert("text".into(), json!(block.text));
            record.insert("origin".into(), json!(block.origin));
            record.insert("source_refs".into(), json!(block.source_refs));
            record.insert("reference".into(), json!(block.reference));
            record.insert(
                "payload".into(),
                json!(block.payload.clone().unwrap_or_else(|| block.text.clone())),
            );
            if block.values.is_some() {
                record.insert("segments".into(), json!([segment(block)]));
            }
            records.push(Some(record));
        }
        if !history.is_empty() {
            let mut labels: BTreeSet<String> = BTreeSet::new();
            let mut refs: BTreeSet<String> = BTreeSet::new();
            for b in &history {
                labels.extend(ferb_sanitize::origin::split(Some(&b.origin)));
                refs.extend(b.source_refs.iter().cloned());
            }
            let kinds = ferb_sanitize::origin::kinds(labels.iter().map(String::as_str));
            let payload: Vec<String> = history
                .iter()
                .map(|b| b.payload.clone().unwrap_or_else(|| b.text.clone()))
                .collect();
            let record = json!({"text": null,
                "origin": ferb_sanitize::origin::join(kinds.iter().map(String::as_str)),
                "source_refs": refs.into_iter().collect::<Vec<_>>(), "reference": "chat.history",
                "payload": payload.join("\n\n"),
                "segments": history.iter().map(|b| segment(b)).collect::<Vec<_>>()});
            if let Some(slot) = records.iter_mut().find(|r| r.is_none()) {
                *slot = record.as_object().cloned();
            }
        }
        let records: Vec<Map<String, Value>> = records.into_iter().flatten().collect();
        match tree {
            None => {
                let Some(turn) = self.turn_of(&task.request_id) else {
                    return;
                };
                let mut t = turn.lock().unwrap_or_else(|e| e.into_inner());
                t.exposures.extend(records);
                t.assembly = Some(assembly.clone());
            }
            Some(tree_id) => {
                for record in records {
                    let refs: Vec<String> = record
                        .get("source_refs")
                        .and_then(Value::as_array)
                        .map(|a| a.iter().map(ferb_py::value::py_str).collect())
                        .unwrap_or_default();
                    let span = self.span_start(
                        "context.exposure",
                        Some(&tree_id),
                        None,
                        None,
                        json!({"source_refs": crate::lines::stored_refs(&refs), "reference": record.get("reference")}),
                    );
                    let made = record
                        .get("origin")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string();
                    self.span_finish(
                        &span,
                        Some(Value::Object(crate::lines::exposure_output(&record))),
                        None,
                        Some(&made),
                        None,
                    );
                }
            }
        }
    }
}

/// supervisor.py:871 `_segment`: one shown record's place in a stored exposure (GATE-519).
fn segment(block: &ContextBlock) -> Value {
    let labels = ferb_sanitize::origin::split(Some(&block.origin));
    let kinds = ferb_sanitize::origin::kinds(labels.iter().map(String::as_str));
    let text = block.payload.clone().unwrap_or_else(|| block.text.clone());
    let reference = block
        .source_refs
        .first()
        .cloned()
        .unwrap_or_else(|| block.reference.clone());
    let mut seg = Map::new();
    seg.insert("ref".into(), json!(reference));
    seg.insert("kinds".into(), json!(kinds.into_iter().collect::<Vec<_>>()));
    seg.insert("len".into(), json!(text.chars().count()));
    match &block.values {
        None => {
            seg.insert("whole".into(), json!(true));
        }
        Some(v) => {
            seg.insert("values".into(), v.clone());
        }
    }
    Value::Object(seg)
}
