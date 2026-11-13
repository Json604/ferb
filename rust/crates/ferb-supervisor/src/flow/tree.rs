//! A task's run: supervisor.py:4465 `run_tree`, :4637 `_plan_event`, :4736 `_checklist`, :4786 `_step_state`,
//! :6094 `_execute`, :5116 `_execute_plan` with the plan runner (plan_executor.py), :4917 `_finish_tree`, :7406
//! `_final` and what they read.
//!
//! The run's events reach the turn's stream in the order Python's one queue (`_live[tree_id]`) carries them: the step
//! states and the worker's live lines through ferb-worker's live line, the subtask events through the same line (see
//! [`SUBTASK_ID`]), forwarded by [`Inner::run_tree`] while the run goes.

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

use ferb_core::types::{Identity, Outcome, OutcomeStatus, Task, TaskRole};
use ferb_orchestrator::models::{Check, Condition, Step, TaskSpec};
use ferb_orchestrator::plan_store::StepState;
use ferb_orchestrator::plans::{answer_steps, step_label, CHECKLIST_STEPS, DONE, ENDED, SINGLE};
use ferb_orchestrator::validate::{dump_step, dump_task_spec};
use ferb_orchestrator::verify::{ConditionResult, DECLINED, UNCHECKED};
use ferb_sanitize::origin;
use futures_util::stream::{FuturesUnordered, StreamExt};
use futures_util::FutureExt;
use serde_json::{json, Map, Value};

use super::{Emit, Failure, Flow, Inner, Tree, TreeState};
use crate::words::*;
use crate::NotPorted;

/// The step id of a single task's one step, its worker's (`WORKER`).
pub const WORKER: &str = SINGLE;

/// A subtask event rides the tree's live line (ferb-worker's `tell_tree` writes the tree's id over `task_id`); its own
/// task id travels under this key and is put back in place before the event is forwarded.
pub const SUBTASK_ID: &str = "\u{0}subtask_task_id";

fn obj(v: Value) -> Map<String, Value> {
    match v {
        Value::Object(m) => m,
        _ => Map::new(),
    }
}

fn outcome(
    task_id: &str,
    status: &str,
    summary: &str,
    artifacts: Vec<String>,
    evidence: Vec<String>,
) -> Outcome {
    Outcome {
        task_id: task_id.to_string(),
        status: status_of(status),
        summary: summary.to_string(),
        artifacts,
        evidence,
    }
}

pub fn status_of(s: &str) -> OutcomeStatus {
    OutcomeStatus::ALL
        .iter()
        .copied()
        .find(|k| k.as_str() == s)
        .unwrap_or(OutcomeStatus::Failed)
}

fn strings(v: Option<&Value>) -> Vec<String> {
    v.and_then(Value::as_array)
        .map(|a| a.iter().map(ferb_py::value::py_str).collect())
        .unwrap_or_default()
}

/// supervisor.py:4762 `_step_task`.
pub fn step_task(tree_id: &str, step_id: &str) -> String {
    if step_id == SINGLE {
        format!("{tree_id}.{WORKER}")
    } else {
        format!("{tree_id}.step.{step_id}")
    }
}

/// plan_executor.py:17 `_ENDED_AS`.
fn ended_as(status: &str) -> &'static str {
    match status {
        "done" | "done_unverified" => "done",
        "cancelled" | "needs_user" => "not_finished",
        _ => "failed",
    }
}

/// plan_executor.py:86 `_blocked`.
fn blocked(steps: &[Step], states: &[(String, StepState)]) -> Vec<(String, Vec<String>)> {
    let status = |id: &str| {
        states
            .iter()
            .find(|(k, _)| k == id)
            .map(|(_, s)| s.status.clone())
            .unwrap_or_default()
    };
    let ended: Vec<String> = states
        .iter()
        .filter(|(_, s)| ENDED.contains(&s.status.as_str()) && !DONE.contains(&s.status.as_str()))
        .map(|(k, _)| k.clone())
        .collect();
    let mut out: Vec<(String, Vec<String>)> = Vec::new();
    let mut changed = true;
    while changed {
        changed = false;
        for step in steps {
            if out.iter().any(|(k, _)| *k == step.id) || ENDED.contains(&status(&step.id).as_str())
            {
                continue;
            }
            let stops: Vec<String> = step
                .depends_on
                .iter()
                .filter(|d| ended.contains(d) || out.iter().any(|(k, _)| k == *d))
                .cloned()
                .collect();
            if !stops.is_empty() {
                out.push((step.id.clone(), stops));
                changed = true;
            }
        }
    }
    out
}

impl Inner {
    /// The root tree of a tree id (`_tree_of[tree_id]` whose id is it).
    pub fn root(&self, tree_id: &str) -> Option<Arc<Tree>> {
        self.live()
            .tree_of
            .get(tree_id)
            .filter(|t| t.id == tree_id)
            .cloned()
    }

    /// The tree's live line (`_tell_tree`), through ferb-worker's.
    pub fn tell_tree(&self, tree_id: &str, event: Map<String, Value>) {
        if let Some(w) = self.worker.get() {
            w.tell_tree(tree_id, event);
        }
    }

    /// A subtask event on the tree's line (Python: `queue.put` on the same queue).
    fn subtask(&self, tree_id: &str, event: Value) {
        let mut event = obj(event);
        let own = event.get("task_id").cloned().unwrap_or(Value::Null);
        event.insert(SUBTASK_ID.into(), own);
        self.tell_tree(tree_id, event);
    }

    /// supervisor.py:4465 `run_tree`: one task's run; its events on `emit`. `release`: the turn lets the chat go once
    /// the plan line is out.
    #[allow(clippy::too_many_arguments)] // run_tree's own arguments
    pub async fn run_tree(
        &self,
        spec: TaskSpec,
        text: &str,
        request_id: &str,
        message: &str,
        conversation: Option<&str>,
        prior_task: Option<&str>,
        origins: &BTreeSet<String>,
        emit: &Emit,
        release: &(dyn Fn() + Send + Sync),
    ) -> Flow<()> {
        let tree_id = self.new_id();
        let scopes = self.deps.daemon.all_scopes();
        let root = Task {
            id: tree_id.clone(),
            parent_id: None,
            goal: spec.goal.clone(),
            device_id: self.deps.device_id.clone(),
            identity: Identity::Owner,
            scopes: BTreeSet::new(),
            role: TaskRole::Mixed,
            request_id: request_id.to_string(),
            success_conditions: Vec::new(),
            deadline: None,
        };
        let worker = Task {
            id: format!("{tree_id}.{WORKER}"),
            parent_id: Some(tree_id.clone()),
            goal: spec.goal.clone(),
            device_id: self.deps.device_id.clone(),
            identity: if spec.identity == "ferb" {
                Identity::Ferb
            } else {
                Identity::Owner
            },
            scopes,
            role: TaskRole::Worker,
            request_id: request_id.to_string(),
            success_conditions: spec
                .success_conditions
                .iter()
                .map(|c| c.text.clone())
                .collect(),
            deadline: None,
        };
        let turn = self.turn_of(request_id);
        let tree = Arc::new(Tree {
            id: tree_id.clone(),
            worker: worker.clone(),
            step_id: WORKER.into(),
            asks_in_place: true,
            st: Mutex::new(TreeState {
                spec: Some(spec.clone()),
                prior_task: prior_task.map(str::to_string),
                accepting_updates: true,
                ..TreeState::default()
            }),
            steering: Arc::default(),
            result_revision: Mutex::new(0),
        });
        let (fronts, asked) = match &turn {
            Some(t) => {
                let t = t.lock().unwrap();
                let mut st = tree.lock();
                st.intake_receipts = t.intake_receipts.clone();
                st.side_lines = t.side_lines.clone();
                st.side_cards = t.side_cards.clone();
                if t.screen.is_some() {
                    return Err(
                        NotPorted("Supervisor.run_tree (the start-of-task screenshot)").into(),
                    );
                }
                (
                    t.screen
                        .iter()
                        .map(|s| s.origin.clone())
                        .collect::<Vec<_>>(),
                    t.message_origin.clone(),
                )
            }
            None => (Vec::new(), BTreeSet::new()),
        };
        if let Some(t) = &turn {
            let mut t = t.lock().unwrap();
            if !t.said_first.is_empty() {
                let mut st = tree.lock();
                st.said_first = std::mem::take(&mut t.said_first);
                st.said_first_cards = std::mem::take(&mut t.said_first_cards);
            }
        }
        let mut request = self.deps.daemon.request(request_id);
        if request.payload.is_empty() {
            request.payload = if message.is_empty() {
                request.words.clone()
            } else {
                message.to_string()
            };
            self.deps.daemon.record_request(&request);
        }
        let asked = if asked.is_empty() {
            BTreeSet::from([origin::external("unlabeled", "")])
        } else {
            asked
        };
        let mut request_origins = asked;
        for o in fronts {
            request_origins.extend(
                origin::split(Some(&o))
                    .into_iter()
                    .filter(|x| origin::is_external(x)),
            );
        }
        if let Some(prior) = prior_task {
            request_origins.extend(self.request_origins_of(prior));
        }
        let receipts = tree.lock().intake_receipts.clone();
        let root_span = self.span_start(
            "task",
            Some(&tree_id),
            None,
            Some(&self.deps.device_id),
            json!({"goal": spec.goal, "identity": spec.identity,
                   "targets": {"apps": spec.targets.apps, "sites": spec.targets.sites, "paths": spec.targets.paths, "recipients": spec.targets.recipients}, "request_id": request_id,
                   "request": request.payload, "authority_words": request.words,
                   "request_origins": request_origins.iter().collect::<Vec<_>>(),
                   "asked_origins": request.asked_origins.iter().collect::<Vec<_>>(),
                   "task_spec": dump_task_spec(&spec).to_value(), "intake_receipts": receipts}),
        );
        self.deps.daemon.record_named(&request.words, request_id);
        let made = if origins.is_empty() {
            BTreeSet::from([origin::external("unlabeled", "")])
        } else {
            origins.clone()
        };
        let kinds = origin::kinds(made.iter().map(String::as_str));
        self.span_set_origin(&root_span, &origin::join(kinds.iter().map(String::as_str)));
        self.task_add(&root, &root_span, &tree_id, "running");
        self.task_add(&worker, &root_span, &tree_id, "planned");
        if let Some(conv) = conversation {
            self.continuity(|c| {
                c.save(
                    &tree_id,
                    conv,
                    &spec,
                    &request.words,
                    prior_task,
                    &request.payload,
                )
            })?;
        }
        let unattended = self.deps.daemon.unattended(request_id);
        let mut message_origins: Vec<&String> = request_origins.iter().collect();
        message_origins.sort();
        let saved = json!({"spec": dump_task_spec(&spec).to_value(), "request_id": request_id,
            "words": request.words, "payload": request.payload,
            "origins": origins.iter().collect::<Vec<_>>(), "message_origins": message_origins,
            "unattended": unattended});
        match &spec.plan {
            Some(plan) => self.plans(|p| p.create(&tree_id, plan, &saved, conversation))?,
            None => {
                self.plans(|p| p.create_single(&tree_id, &spec.goal, &saved, conversation, false))?
            }
        }
        if !receipts.is_empty() {
            let s = self.span_start(
                "task.intake",
                Some(&tree_id),
                Some(&root_span),
                None,
                json!({"request_id": request_id, "layer": "chat"}),
            );
            self.span_finish(
                &s,
                Some(json!({"memory_receipts": receipts})),
                None,
                None,
                None,
            );
        }
        {
            let mut live = self.live();
            live.tree_of.insert(tree_id.clone(), tree.clone());
            live.tree_of.insert(worker.id.clone(), tree.clone());
        }
        self.runs.set_tree(&tree_id, tree.clone());
        self.runs.set_tree(&worker.id, tree.clone());
        if spec.parts.len() > 1 {
            return Err(NotPorted("Supervisor.run_tree (several asks: the card)").into());
        }
        self.deps.daemon.task_folders(&tree_id, Some(&spec.folders));
        if spec.keep_tabs_open {
            self.deps.daemon.keep_tabs_open(&tree_id);
        }
        self.deps.daemon.publish(
            "task.created",
            json!({"task_id": tree_id, "goal": spec.goal,
                   "name": ferb_orchestrator::names::shown_name(Some(&spec.name), &spec.goal)}),
        );
        self.journal(
            "chat.plan",
            json!({"request_id": request_id, "task_id": tree_id}),
            None,
        );
        let mut stated = String::new();
        if let Some(plan) = &spec.plan {
            if plan.big_job || !plan.standing_sends.is_empty() {
                return Err(NotPorted("Supervisor._plan_statement").into());
            }
            if plan.steps.iter().any(Step::loops) {
                return Err(NotPorted("Supervisor.run_tree (loops)").into());
            }
        }
        if !spec.folders.is_empty() {
            stated.push('\n');
            stated.push_str(
                &self.redact_text(&crate::lines::work_in(&spec.folders, &self.deps.home)),
            );
        }
        if !spec.targets.sites.is_empty() {
            stated.push('\n');
            stated
                .push_str(&self.redact_text(&format!("Sites: {}.", spec.targets.sites.join(", "))));
        }
        let plan_event = self.plan_event(&tree, text, &stated);
        release();
        let _ = emit.send(Value::Object(plan_event));
        self.record_planner_reads(&tree, &root_span, request_id);
        if spec.identity == "ferb" {
            return Err(NotPorted("Supervisor._browse_as").into());
        }
        self.deps.daemon.on_task_start(&tree_id).await;
        let mut rx = self.runs.open_live(&tree_id);
        let forward = |mut ev: Map<String, Value>| {
            if let Some(own) = ev.remove(SUBTASK_ID) {
                ev.insert("task_id".into(), own);
            }
            let _ = emit.send(Value::Object(ev));
        };
        let run = self.execute(&tree, &root_span);
        tokio::pin!(run);
        loop {
            tokio::select! {
                biased;
                Some(ev) = rx.recv() => forward(ev),
                _ = &mut run => break,
            }
        }
        while let Ok(ev) = rx.try_recv() {
            forward(ev);
        }
        self.runs.close_live(&tree_id);
        let (verification, stopped) = {
            let st = tree.lock();
            (st.verification.clone(), !st.stop.is_empty())
        };
        if !verification.is_empty() && !stopped {
            let _ = emit.send(
                json!({"type": "verification", "id": WORKER, "task_id": worker.id,
                "results": verification.iter().map(ConditionResult::as_dict).collect::<Vec<_>>()}),
            );
        }
        self.release_resources(&tree, &root_span).await;
        self.deps.daemon.on_task_finish(&tree_id).await;
        let answer = ferb_py::value::py_str(
            self.redact(&Value::Object(self.final_(&tree)?))
                .get("answer")
                .unwrap_or(&Value::Null),
        );
        let made: Vec<String> = origin::split(Some(&self.task_made(&tree_id)))
            .into_iter()
            .collect();
        let stored = ferb_policy::carried::derive(&answer, &self.tree_shown(&tree_id), &made);
        let final_ = self
            .finish_tree(&tree, &root_span, request_id, Some(stored))
            .await?;
        let _ = emit.send(Value::Object(final_));
        Ok(())
    }

    /// supervisor.py:4637 `_plan_event`.
    fn plan_event(&self, tree: &Arc<Tree>, text: &str, stated: &str) -> Map<String, Value> {
        let spec = tree.spec();
        let plan_line = ferb_policy::confirm::from_task_lines(text);
        let lines: Vec<Value> = stated
            .split('\n')
            .filter(|l| !ferb_orchestrator::py::strip(l).is_empty())
            .map(|l| json!(ferb_policy::confirm::from_task_lines(l)))
            .collect();
        let mut event = obj(json!({
            "type": "plan", "text": ferb_policy::confirm::from_task_lines(&format!("{text}{stated}")),
            "task_id": tree.id, "goal": spec.goal,
            "name": ferb_orchestrator::names::shown_name(Some(&spec.name), &spec.goal),
            "identity": spec.identity, "plan_line": plan_line, "lines": lines,
            "subtasks": [{"id": WORKER, "task_id": tree.worker.id, "role": "worker", "goal": spec.goal,
                          "depends_on": []}]}));
        if !spec.folders.is_empty() {
            event.insert("folders".into(), json!(spec.folders));
            event.insert(
                "work_in".into(),
                json!(self.redact_text(&crate::lines::work_in(&spec.folders, &self.deps.home))),
            );
        }
        event.extend(self.checklist(tree, None));
        self.said_first(tree, event)
    }

    /// supervisor.py:4677 `_said_first` (ORCH-638).
    fn said_first(&self, tree: &Arc<Tree>, mut event: Map<String, Value>) -> Map<String, Value> {
        let (first, cards) = {
            let mut st = tree.lock();
            (
                std::mem::take(&mut st.said_first),
                std::mem::take(&mut st.said_first_cards),
            )
        };
        if first.is_empty() {
            return event;
        }
        let text = event
            .get("text")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let joined = if ferb_orchestrator::py::strip(&text).is_empty() {
            first.clone()
        } else {
            format!("{first}\n\n{text}")
        };
        event.insert("text".into(), json!(joined));
        let mut receipts = vec![json!(first)];
        receipts.extend(
            event
                .get("receipts")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default(),
        );
        event.insert("receipts".into(), Value::Array(receipts));
        if !cards.is_empty() {
            let mut all = cards;
            all.extend(
                event
                    .get("cards")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default(),
            );
            event.insert("cards".into(), Value::Array(all));
        }
        event
    }

    /// supervisor.py:4724 `_plan_lines`.
    fn plan_lines(&self, tree_id: &str) -> Vec<Value> {
        let Ok(loaded) = self.plans(|p| p.load(tree_id)) else {
            return Vec::new();
        };
        loaded
            .plan
            .steps
            .iter()
            .map(|step| {
                let mut line = obj(json!({"id": step.id, "label": self.redact_text(&step_label(step)),
                    "kind": step.kind, "depends_on": step.depends_on, "step_task_id": step_task(tree_id, &step.id)}));
                if step.loops() {
                    line.insert("loop".into(), json!(true));
                }
                Value::Object(line)
            })
            .collect()
    }

    /// supervisor.py:4736 `_checklist` (APP-894).
    fn checklist(&self, tree: &Arc<Tree>, ended: Option<&str>) -> Map<String, Value> {
        let lines = self.plan_lines(&tree.id);
        if lines.len() < CHECKLIST_STEPS {
            return Map::new();
        }
        let mut states = self.step_payload(tree);
        if ended == Some("cancelled") {
            let mut out = Map::new();
            for (key, state) in states {
                let mut state = obj(state);
                if state.get("status").and_then(Value::as_str) != Some("done") {
                    state.insert("status".into(), json!("stopped"));
                }
                out.insert(key, Value::Object(state));
            }
            for line in &lines {
                let id = line["id"].as_str().unwrap_or("").to_string();
                if !out.contains_key(&id) {
                    out.insert(
                        id,
                        json!({"status": "stopped", "waits_on": [], "attempt": 0}),
                    );
                }
            }
            states = out;
        }
        obj(json!({"checklist": {"steps": lines, "states": states}}))
    }

    /// supervisor.py:4829 `_step_payload`.
    fn step_payload(&self, tree: &Arc<Tree>) -> Map<String, Value> {
        let Ok(loaded) = self.plans(|p| p.load(&tree.id)) else {
            return Map::new();
        };
        let stored = self.plans(|p| p.steps(&tree.id)).unwrap_or_default();
        let status = |id: &str| -> Option<String> {
            stored
                .iter()
                .find(|(k, _)| k == id)
                .map(|(_, s)| s.status.clone())
        };
        let live = tree.lock().step_states.clone();
        let mut out = Map::new();
        for step in &loaded.plan.steps {
            if let Some(state) = live.get(&step.id) {
                out.insert(step.id.clone(), state.clone());
                continue;
            }
            let stored_status = status(&step.id).unwrap_or_else(|| "pending".into());
            let mut st: Option<&str> = match stored_status.as_str() {
                "done" | "done_unverified" => Some("done"),
                "failed" | "blocked" => Some("failed"),
                "cancelled" | "needs_user" => Some("not_finished"),
                "paused" => Some("needs_you"),
                "running" => Some("running"),
                "interrupted" => Some("starting"),
                "waiting" => Some("waiting"),
                _ => None,
            };
            let waits_on: Vec<String> = step
                .depends_on
                .iter()
                .filter(|d| !DONE.contains(&status(d).unwrap_or_default().as_str()))
                .cloned()
                .collect();
            if st.is_none() && !waits_on.is_empty() {
                st = Some("waiting");
            }
            if let Some(s) = st {
                let mut state = obj(json!({"status": s,
                    "waits_on": if s == "waiting" { waits_on } else { Vec::new() }, "attempt": 0}));
                if step.loops() {
                    state.insert("loop".into(), json!(true));
                }
                out.insert(step.id.clone(), Value::Object(state));
            }
        }
        out
    }

    /// supervisor.py:4786 `_step_state` (ORCH-531): the one step-state path.
    pub fn step_state(
        &self,
        tree_id: &str,
        step_id: &str,
        status: &str,
        facts: Map<String, Value>,
    ) {
        let Some(root) = self.root(tree_id) else {
            return;
        };
        let before = obj(root
            .lock()
            .step_states
            .get(step_id)
            .cloned()
            .unwrap_or(Value::Null));
        let given_waits = facts.get("waits_on").filter(|v| ferb_py::value::truthy(v));
        let waits_on: Vec<Value> = match given_waits {
            Some(w) => w.as_array().cloned().unwrap_or_default(),
            None => {
                if status == "waiting" || status == "wont_run" || !before.contains_key("waits_on") {
                    Vec::new()
                } else {
                    before["waits_on"].as_array().cloned().unwrap_or_default()
                }
            }
        };
        let attempt = facts
            .get("attempt")
            .filter(|v| ferb_py::value::truthy(v))
            .or_else(|| before.get("attempt").filter(|v| ferb_py::value::truthy(v)))
            .and_then(Value::as_i64)
            .unwrap_or(0);
        let same = before.get("status").and_then(Value::as_str) == Some(status)
            && before.get("waits_on").cloned().unwrap_or(json!([]))
                == Value::Array(waits_on.clone())
            && before.get("attempt").and_then(Value::as_i64).unwrap_or(0) == attempt;
        if same {
            return;
        }
        let mut now = ferb_clock::py::utc(&self.now());
        let iso = ferb_clock::py::iso(&now);
        let mut started = if status != "waiting" && status != "starting" {
            before.get("started_at").cloned().unwrap_or(Value::Null)
        } else {
            Value::Null
        };
        if status == "starting" || (started.is_null() && STEP_LIVE.contains(&status)) {
            started = json!(iso);
        }
        let ended = if STEP_ENDED.contains(&status) {
            json!(iso)
        } else {
            Value::Null
        };
        let floor = self.live().step_seq.clone();
        let micro = |t: &ferb_clock::py::Time| t.format("%Y-%m-%dT%H:%M:%S%.6f%:z").to_string();
        let mut stamp = micro(&now);
        while stamp <= floor {
            now = ferb_clock::py::plus_us(&now, 1).expect("a time");
            stamp = micro(&now);
        }
        self.live().step_seq = stamp.clone();
        let mut state = obj(
            json!({"status": status, "waits_on": waits_on, "attempt": attempt,
            "started_at": started, "ended_at": ended, "seq": stamp}),
        );
        if facts.get("loop").is_some_and(ferb_py::value::truthy)
            || before.get("loop").is_some_and(ferb_py::value::truthy)
        {
            state.insert("loop".into(), json!(true));
        }
        for k in ["until", "event"] {
            if let Some(v) = facts.get(k).filter(|v| ferb_py::value::truthy(v)) {
                state.insert(k.into(), v.clone());
            }
        }
        root.lock()
            .step_states
            .insert(step_id.to_string(), Value::Object(state.clone()));
        let request = root.worker.request_id.clone();
        let mut event = obj(
            json!({"type": "step", "task_id": tree_id, "request_id": request, "step": step_id,
            "step_task_id": step_task(tree_id, step_id)}),
        );
        event.extend(state);
        self.tell_tree(tree_id, event.clone());
        self.deps.daemon.publish("task.step", Value::Object(event));
    }

    /// supervisor.py:4780 `_attempt_state`.
    pub fn attempt_state(&self, tree: &Arc<Tree>, status: &str) {
        if tree.asks_in_place {
            let attempt = tree.lock().attempt;
            self.step_state(
                &tree.id,
                &tree.step_id,
                status,
                obj(json!({"attempt": attempt})),
            );
        }
    }

    /// supervisor.py:6094 `_execute`.
    async fn execute(&self, tree: &Arc<Tree>, span: &str) {
        let worker = &tree.worker;
        let stop = tree.lock().stop.clone();
        let out = if !stop.is_empty() {
            outcome(&worker.id, "cancelled", &stop, vec![], vec![])
        } else {
            self.subtask(
                &tree.id,
                json!({"type": "subtask", "id": WORKER, "task_id": worker.id, "status": "started", "role": "worker"}),
            );
            match self.execute_plan(tree, span).await {
                Ok(out) => out,
                Err(e) => {
                    self.journal(
                        "task.execution_failed",
                        json!({"task_id": worker.id, "error": format!("{}: {}", e.class(), e.message())}),
                        None,
                    );
                    let previous = tree.lock().reports.last().cloned().unwrap_or_default();
                    let err = ferb_worker::Error::other(&e.class(), e.message());
                    let route = || self.deps.daemon.has_route();
                    let said = ferb_worker::errors::couldnt(
                        "continue this task",
                        &err,
                        Some(&|t| self.redact_text(t)),
                        &route,
                    );
                    outcome(
                        &worker.id,
                        "failed",
                        &said,
                        strings(previous.get("artifacts")),
                        strings(previous.get("evidence")),
                    )
                }
            }
        };
        let stop = tree.lock().stop.clone();
        let out = if !stop.is_empty() && out.status != OutcomeStatus::Cancelled {
            outcome(
                &worker.id,
                &self.stop_status(tree),
                &stop,
                out.artifacts,
                out.evidence,
            )
        } else {
            out
        };
        {
            let mut st = tree.lock();
            st.outcome = Some(out.clone());
            st.accepting_updates = false;
        }
        let status = self.finish_step(tree, &out);
        self.subtask(
            &tree.id,
            json!({"type": "subtask", "id": WORKER, "task_id": worker.id, "status": status,
                   "summary": out.summary, "role": "worker"}),
        );
    }

    /// supervisor.py:1784 `_stop_status`.
    pub fn stop_status(&self, tree: &Arc<Tree>) -> String {
        if tree.lock().stop == STOPPED_BY_YOU {
            "cancelled".into()
        } else {
            "failed".into()
        }
    }

    /// supervisor.py:1778 `_stopped`.
    pub fn stopped(&self, tree: &Arc<Tree>) -> bool {
        !tree.lock().stop.is_empty()
    }

    /// supervisor.py:5463 `_finish_step`.
    pub fn finish_step(&self, member: &Arc<Tree>, out: &Outcome) -> String {
        let mut status = {
            let st = member.lock();
            if out.status == OutcomeStatus::Done && (st.stands_unconfirmed || st.kept_contradicted)
            {
                "done_unverified".to_string()
            } else {
                out.status.as_str().to_string()
            }
        };
        let declined = self.declined(member, out);
        member.lock().declined = declined;
        if declined {
            status = if member.step_id == WORKER {
                "cancelled"
            } else {
                "done"
            }
            .into();
        }
        self.set_status(
            &member.worker.id,
            &status,
            Some(&json!({"summary": out.summary})),
        );
        status
    }

    /// The plan runner (plan_executor.py `PlanExecutor.run`): steps whose dependencies are done run together; a step
    /// whose dependency ended unfinished won't run.
    async fn run_plan(&self, tree: &Arc<Tree>, span: &str) -> Flow<Value> {
        let tree_id = tree.id.clone();
        let plan = self.plans(|p| p.load(&tree_id))?.plan;
        self.plans(|p| p.set_status(&tree_id, "running"))?;
        let mut jobs = FuturesUnordered::new();
        let mut in_flight: Vec<String> = Vec::new();
        let steps_json = |states: &[(String, StepState)]| -> Value {
            Value::Object(
                states
                    .iter()
                    .map(|(k, s)| (k.clone(), json!({"status": s.status, "output": s.output})))
                    .collect(),
            )
        };
        while !self.stopped(tree) {
            let states = self.plans(|p| p.steps(&tree_id))?;
            let status = |id: &str| {
                states
                    .iter()
                    .find(|(k, _)| k == id)
                    .map(|(_, s)| s.status.clone())
                    .unwrap_or_default()
            };
            let unfinished: Vec<&Step> = plan
                .steps
                .iter()
                .filter(|s| !DONE.contains(&status(&s.id).as_str()))
                .collect();
            if unfinished.is_empty() {
                return Ok(json!({"status": "done", "steps": steps_json(&states)}));
            }
            let stuck = blocked(&plan.steps, &states);
            for step in &unfinished {
                if in_flight.contains(&step.id) || ENDED.contains(&status(&step.id).as_str()) {
                    continue;
                }
                let tell = |s: &str, facts: Vec<(&str, Value)>| {
                    let mut map = Map::new();
                    map.insert("loop".into(), json!(step.loops()));
                    for (k, v) in facts {
                        map.insert(k.into(), v);
                    }
                    self.step_state(&tree_id, &step.id, s, map);
                };
                if let Some((_, stops)) = stuck.iter().find(|(k, _)| *k == step.id) {
                    tell("wont_run", vec![("waits_on", json!(stops))]);
                } else if step
                    .depends_on
                    .iter()
                    .all(|d| DONE.contains(&status(d).as_str()))
                {
                    let mut prior = Map::new();
                    for d in &step.depends_on {
                        prior.insert(
                            d.clone(),
                            states
                                .iter()
                                .find(|(k, _)| k == d)
                                .map(|(_, s)| s.output.clone())
                                .unwrap_or(Value::Null),
                        );
                    }
                    prior.insert(
                        "previous_attempt".into(),
                        states
                            .iter()
                            .find(|(k, _)| *k == step.id)
                            .map(|(_, s)| s.output.clone())
                            .unwrap_or(Value::Null),
                    );
                    in_flight.push(step.id.clone());
                    let owned = (*step).clone();
                    let id = step.id.clone();
                    jobs.push(async move {
                        let r = self.plan_step(tree, owned, prior, span).await;
                        (id, r)
                    });
                } else {
                    let waits: Vec<String> = step
                        .depends_on
                        .iter()
                        .filter(|d| !DONE.contains(&status(d).as_str()))
                        .cloned()
                        .collect();
                    tell("waiting", vec![("waits_on", json!(waits))]);
                }
            }
            if jobs.is_empty() {
                let waiting = unfinished.iter().any(|s| status(&s.id) == "needs_user");
                return Ok(
                    json!({"status": if waiting { "needs_user" } else { "blocked" },
                                 "steps": steps_json(&states)}),
                );
            }
            let Some(first) = jobs.next().await else {
                continue;
            };
            let mut done = vec![first];
            while let Some(Some(more)) = jobs.next().now_or_never() {
                done.push(more);
            }
            for (id, result) in done {
                in_flight.retain(|s| *s != id);
                result?;
            }
        }
        drop(jobs);
        let states = self.plans(|p| p.steps(&tree_id))?;
        Ok(json!({"status": "cancelled", "steps": steps_json(&states)}))
    }

    /// plan_executor.py:72 `_step`.
    async fn plan_step(
        &self,
        tree: &Arc<Tree>,
        step: Step,
        prior: Map<String, Value>,
        span: &str,
    ) -> Flow<Value> {
        let tree_id = tree.id.clone();
        self.plans(|p| p.begin(&tree_id, &step.id, "running"))?;
        let tell = |s: &str| {
            let mut map = Map::new();
            map.insert("loop".into(), json!(step.loops()));
            self.step_state(&tree_id, &step.id, s, map);
        };
        tell("starting");
        let result = if step.kind == "wait" {
            return Err(NotPorted("Supervisor._execute_plan (a wait step)").into());
        } else {
            match self
                .execute_step(tree, &step, step.inputs.clone(), prior, span)
                .await
            {
                Ok(v) => v,
                Err(Failure::NotPorted(n)) => return Err(Failure::NotPorted(n)),
                Err(Failure::Cancelled) => return Err(Failure::Cancelled),
                Err(e) => {
                    json!({"status": "failed", "summary": format!("{}: {}", e.class(), e.message())})
                }
            }
        };
        self.plans(|p| p.finish(&tree_id, &step.id, &result))?;
        let status = result
            .get("status")
            .map(ferb_py::value::py_str)
            .unwrap_or_else(|| "None".into());
        tell(ended_as(&status));
        Ok(result)
    }

    /// `_execute_plan`'s `execute`: one step's work.
    async fn execute_step(
        &self,
        tree: &Arc<Tree>,
        step: &Step,
        inputs: Vec<String>,
        prior: Map<String, Value>,
        span: &str,
    ) -> Flow<Value> {
        if step.kind == "ask" {
            return Err(NotPorted("Supervisor._plan_question").into());
        }
        if step.kind == "converse" || step.loops() {
            return Err(NotPorted("Supervisor._execute_plan (a conversation or a loop)").into());
        }
        let previous = prior
            .get("previous_attempt")
            .cloned()
            .unwrap_or(Value::Null);
        if previous.get("status").and_then(Value::as_str) == Some("paused")
            || previous.get("fixing").is_some()
        {
            return Err(NotPorted(
                "Supervisor._execute_plan (a step a restart left waiting or fixing)",
            )
            .into());
        }
        let spec = tree.spec();
        let single = spec.plan.is_none();
        if single {
            let out = self.worker_loop(tree, span).await?;
            tree.lock().single_ran = Some(out.clone());
            let status = self.finish_step(tree, &out);
            let (answer, declined, question, revision) = {
                let st = tree.lock();
                (
                    st.owner_answer.clone(),
                    st.declined,
                    st.owner_question.clone(),
                    *tree.result_revision.lock().unwrap(),
                )
            };
            let mut output = obj(
                json!({"status": status, "summary": out.summary, "answer": answer,
                "artifacts": out.artifacts, "evidence": out.evidence}),
            );
            output.extend(self.checked_record(tree));
            output.insert("steers_seen".into(), json!(revision));
            if declined {
                output.insert("declined".into(), json!(true));
            }
            if out.status == OutcomeStatus::NeedsUser {
                output.insert("question".into(), json!(question));
            }
            return Ok(Value::Object(output));
        }
        let answering = answer_steps(spec.plan.as_ref(), &spec.parts);
        let mut child_spec = spec.clone();
        child_spec.plan = None;
        child_spec.parts = Vec::new();
        child_spec.goal = step.goal.clone();
        child_spec.worker_type = step.worker_type.clone();
        child_spec.earlier_results = {
            let mut all = spec.earlier_results.clone();
            for r in &step.earlier_results {
                if !all.contains(r) {
                    all.push(r.clone());
                }
            }
            all
        };
        let mut conditions = self.answer_conditions(&spec, &step.id, &answering);
        conditions.extend(step.conditions.iter().map(|text| Condition {
            text: text.clone(),
            owner_text: text.clone(),
            check: Check::none(),
            layer: "outcome".into(),
        }));
        if conditions.is_empty() {
            conditions.push(Condition {
                text: step.goal.clone(),
                owner_text: step.goal.clone(),
                check: Check::none(),
                layer: "outcome".into(),
            });
        }
        child_spec.success_conditions = conditions;
        let mut child = tree.worker.clone();
        child.id = format!("{}.step.{}", tree.id, step.id);
        child.goal = step.goal.clone();
        child.success_conditions = child_spec
            .success_conditions
            .iter()
            .map(|c| c.text.clone())
            .collect();
        let earlier = self.plans(|p| p.earlier(&tree.id)).unwrap_or_default();
        let mut context =
            obj(json!({"step": dump_step(step).to_value(), "inputs": inputs, "prior": prior}));
        if !earlier.is_empty() {
            context.insert(
                "earlier_plan_results".into(),
                Value::Object(earlier.into_iter().collect()),
            );
        }
        context.extend(self.answer_context(&spec, tree, step, &answering, &prior));
        if !inputs.is_empty() {
            context.insert(
                "requirement".into(),
                json!("Write one input_notes entry per input with the complete note and evidence references."),
            );
        }
        let (prior_task, parent_goal) = (tree.lock().prior_task.clone(), spec.goal.clone());
        let member = Arc::new(Tree {
            id: tree.id.clone(),
            worker: child.clone(),
            step_id: step.id.clone(),
            asks_in_place: true,
            st: Mutex::new(TreeState {
                spec: Some(child_spec),
                prior_task,
                parent_goal,
                required_inputs: inputs.clone(),
                step_context: ferb_py::json::dumps(&Value::Object(context)),
                accepting_updates: true,
                ..TreeState::default()
            }),
            steering: tree.steering.clone(),
            result_revision: Mutex::new(0),
        });
        self.live().tree_of.insert(child.id.clone(), member.clone());
        self.runs.set_tree(&child.id, member.clone());
        self.task_add(&child, span, &tree.id, "planned");
        self.subtask(
            &tree.id,
            json!({"type": "subtask", "id": step.id, "task_id": child.id, "status": "started", "role": "worker"}),
        );
        let ran = self.worker_loop(&member, span).await;
        let result = match ran {
            Err(e) => Err(e),
            Ok(out) => {
                let status = self.finish_step(&member, &out);
                {
                    let mut st = tree.lock();
                    st.ran.push((step.id.clone(), member.clone()));
                    if answering.contains(&step.id) {
                        st.answers.push((step.id.clone(), member.clone()));
                    }
                    if let Some(sw) = member.lock().runtime_switch.clone() {
                        st.runtime_switch = Some(sw);
                    }
                }
                let (answer, notes, proposals, declined, question, revision) = {
                    let st = member.lock();
                    (
                        st.owner_answer.clone(),
                        st.input_notes.clone(),
                        st.conversation_proposals.clone(),
                        st.declined,
                        st.owner_question.clone(),
                        *member.result_revision.lock().unwrap(),
                    )
                };
                let mut output = obj(
                    json!({"status": status, "summary": out.summary, "answer": answer,
                    "artifacts": out.artifacts, "evidence": out.evidence, "input_notes": notes,
                    "conversation_proposals": proposals}),
                );
                output.extend(self.checked_record(&member));
                output.insert("steers_seen".into(), json!(revision));
                if declined {
                    output.insert("declined".into(), json!(true));
                }
                if out.status == OutcomeStatus::NeedsUser {
                    output.insert("question".into(), json!(question));
                }
                let digest = {
                    use sha2::Digest;
                    hex::encode(sha2::Sha256::digest(step.id.as_bytes()))
                };
                let path = format!(
                    "{}/task-notes/{}/{digest}.json",
                    self.deps.workspace.trim_end_matches('/'),
                    tree.id
                );
                let redacted = self.redact(&Value::Object(output.clone()));
                let text = ferb_py::json::dumps_with(
                    &redacted,
                    ferb_py::json::Style {
                        ascii: false,
                        sort_keys: false,
                        indent: Some(2),
                    },
                );
                self.deps
                    .daemon
                    .write_notes(&path, &text)
                    .map_err(|e| Failure::raised("OSError", e))?;
                output.insert("notes_file".into(), json!(path));
                let mut data = obj(json!({"task_id": tree.id, "step": step.id}));
                data.extend(output.clone());
                let made = self.task_made(&tree.id);
                self.journal("plan.step", Value::Object(data), Some(&made));
                self.subtask(
                    &tree.id,
                    json!({"type": "subtask", "id": step.id, "task_id": child.id, "status": status,
                           "summary": out.summary, "role": "worker"}),
                );
                Ok(Value::Object(output))
            }
        };
        self.release_resources(&member, span).await;
        let (resources, errors) = {
            let st = member.lock();
            (st.resources.clone(), st.cleanup_errors.clone())
        };
        let mut st = tree.lock();
        st.resources.extend(resources);
        st.cleanup_errors.extend(errors);
        result
    }

    /// supervisor.py:5315 `_answer_conditions`.
    fn answer_conditions(
        &self,
        spec: &TaskSpec,
        step_id: &str,
        answering: &[String],
    ) -> Vec<Condition> {
        if !answering.iter().any(|a| a == step_id) {
            return Vec::new();
        }
        if let Some(part) = spec
            .parts
            .iter()
            .find(|p| p.answer_step.as_deref() == Some(step_id))
        {
            return part
                .conditions
                .iter()
                .filter_map(|i| i.as_i64())
                .filter(|i| *i >= 0 && (*i as usize) < spec.success_conditions.len())
                .map(|i| spec.success_conditions[i as usize].clone())
                .collect();
        }
        if answering.len() == 1 {
            spec.success_conditions.clone()
        } else {
            Vec::new()
        }
    }

    /// supervisor.py:5327 `_answer_context`.
    fn answer_context(
        &self,
        spec: &TaskSpec,
        tree: &Arc<Tree>,
        step: &Step,
        answering: &[String],
        prior: &Map<String, Value>,
    ) -> Map<String, Value> {
        if !answering.contains(&step.id) {
            return Map::new();
        }
        // a part's own wording ("the answer to this part of their request: “…”") needs the card of several asks,
        // which run_tree refuses before any step runs (NotPorted)
        let _ = spec;
        let said = "the task's answer";
        let mut context = obj(json!({"answers": format!(
            "Your checked answer goes to the owner as it is, on its own, as {said}. Write it for them, answer first; \
             nothing sums it up afterwards.")}));
        let steps: Vec<Step> = self
            .plans(|p| p.load(&tree.id))
            .map(|l| l.plan.steps)
            .unwrap_or_default();
        let mut ancestors: BTreeSet<String> = BTreeSet::new();
        let mut todo = step.depends_on.clone();
        while let Some(key) = todo.pop() {
            if let Some(s) = steps.iter().find(|s| s.id == key) {
                if ancestors.insert(key.clone()) {
                    todo.extend(s.depends_on.clone());
                }
            }
        }
        let states = self.plans(|p| p.steps(&tree.id)).unwrap_or_default();
        let mut earlier = Map::new();
        for key in &ancestors {
            if prior.contains_key(key) {
                continue;
            }
            if let Some((_, s)) = states.iter().find(|(k, _)| k == key) {
                earlier.insert(key.clone(), s.output.clone());
            }
        }
        if !earlier.is_empty() {
            context.insert("earlier_steps".into(), Value::Object(earlier));
        }
        context
    }

    /// supervisor.py:5350 `_checked_record`.
    fn checked_record(&self, member: &Arc<Tree>) -> Map<String, Value> {
        let out = member.lock().outcome.clone();
        match out {
            Some(o) if o.status == OutcomeStatus::Done => {
                let (_, note, verified) = self.checked_answer(member, &o);
                let verification: Vec<Value> = member
                    .lock()
                    .verification
                    .iter()
                    .map(ConditionResult::as_dict)
                    .collect();
                obj(json!({"note": note, "verified": verified, "verification": verification}))
            }
            _ => Map::new(),
        }
    }

    /// supervisor.py:5116 `_execute_plan`: the one execution path.
    async fn execute_plan(&self, tree: &Arc<Tree>, span: &str) -> Flow<Outcome> {
        let plan = self.plans(|p| p.load(&tree.id))?.plan;
        let approved = self
            .plans(|p| p.load(&tree.id))?
            .row
            .get("approved")
            .is_some_and(ferb_py::value::truthy);
        if (plan.big_job || !plan.standing_sends.is_empty()) && !approved {
            return Err(
                NotPorted("Supervisor._execute_plan (a stated plan's standing scope)").into(),
            );
        }
        let spec = tree.spec();
        let single = spec.plan.is_none();
        let answering = answer_steps(spec.plan.as_ref(), &spec.parts);
        let result = loop {
            let result = self.run_plan(tree, span).await?;
            let steering = tree.steering.lock().await.messages.len();
            let states = self.plans(|p| p.steps(&tree.id))?;
            let unheard: Vec<String> = states
                .iter()
                .filter(|(_, s)| {
                    ["failed", "blocked", "needs_user"].contains(&s.status.as_str())
                        && (s
                            .output
                            .get("steers_seen")
                            .and_then(Value::as_i64)
                            .unwrap_or(steering as i64)
                            < steering as i64)
                })
                .map(|(k, _)| k.clone())
                .collect();
            let status = result["status"].as_str().unwrap_or("").to_string();
            if status == "done" || status == "cancelled" || self.stopped(tree) || unheard.is_empty()
            {
                break result;
            }
            self.plans(|p| p.retry_unfinished(&tree.id))?;
        };
        let states = self.plans(|p| p.steps(&tree.id))?;
        let state = |id: &str| states.iter().find(|(k, _)| k == id).map(|(_, s)| s.clone());
        if single {
            let ran = tree.lock().single_ran.clone();
            if let Some(out) = ran {
                return Ok(out);
            }
            let saved = state(SINGLE).map(|s| s.output).unwrap_or(Value::Null);
            let mut status = saved
                .get("status")
                .map(ferb_py::value::py_str)
                .unwrap_or_else(|| "failed".into());
            if DONE.contains(&status.as_str()) {
                return Err(NotPorted("Supervisor._adopt_saved").into());
            }
            if self.stopped(tree)
                || !["done", "failed", "blocked", "needs_user", "cancelled"]
                    .contains(&status.as_str())
            {
                status = if self.stopped(tree) {
                    "cancelled".into()
                } else {
                    "failed".into()
                };
            }
            let stop = tree.lock().stop.clone();
            let summary = match saved.get("summary") {
                Some(v) if ferb_py::value::truthy(v) => ferb_py::value::py_str(v),
                _ => stop,
            };
            return Ok(outcome(
                &tree.worker.id,
                &status,
                &summary,
                strings(saved.get("artifacts")),
                strings(saved.get("evidence")),
            ));
        }
        let outputs: Vec<Value> = plan
            .steps
            .iter()
            .filter_map(|s| state(&s.id).map(|st| st.output))
            .collect();
        let mut artifacts: Vec<String> = Vec::new();
        for o in &outputs {
            for a in strings(o.get("artifacts")) {
                if !artifacts.contains(&a) {
                    artifacts.push(a);
                }
            }
        }
        let evidence: Vec<String> = outputs
            .iter()
            .flat_map(|o| strings(o.get("evidence")))
            .collect();
        let status = result["status"].as_str().unwrap_or("").to_string();
        if status == "done" {
            let members: Vec<Option<Arc<Tree>>> = answering
                .iter()
                .map(|k| {
                    tree.lock()
                        .answers
                        .iter()
                        .find(|(s, _)| s == k)
                        .map(|(_, t)| t.clone())
                })
                .collect();
            if members.len() == 1 {
                if let Some(member) = members[0].clone() {
                    if member.lock().outcome.is_some() {
                        let (answer, unconfirmed) = {
                            let st = member.lock();
                            (st.owner_answer.clone(), st.stands_unconfirmed)
                        };
                        let mut st = tree.lock();
                        st.answering = Some(member);
                        st.stands_unconfirmed = unconfirmed;
                        return Ok(outcome(
                            &tree.worker.id,
                            "done",
                            &answer,
                            artifacts,
                            evidence,
                        ));
                    }
                }
            }
            return Err(NotPorted("Supervisor._adopt_saved").into());
        }
        if status == "needs_user" {
            let question = plan
                .steps
                .iter()
                .filter_map(|s| state(&s.id))
                .find(|s| s.status == "needs_user")
                .map(|s| {
                    s.output
                        .get("question")
                        .map(ferb_py::value::py_str)
                        .unwrap_or_default()
                })
                .unwrap_or_default();
            tree.lock().owner_question = question;
        } else {
            let stopped: Vec<String> = plan
                .steps
                .iter()
                .filter(|s| {
                    state(&s.id).is_some_and(|st| st.status == "failed" || st.status == "blocked")
                })
                .map(|s| s.id.clone())
                .collect();
            let member = {
                let st = tree.lock();
                stopped
                    .iter()
                    .find_map(|k| st.ran.iter().find(|(s, _)| s == k).map(|(_, t)| t.clone()))
            };
            if let Some(m) = member {
                tree.lock().answering = Some(m);
            }
        }
        let status_of_step = |id: &str| state(id).map(|s| s.status);
        let summary = crate::lines::stopped_plan_summary(&plan.steps, &status_of_step);
        Ok(outcome(
            &tree.worker.id,
            if status == "blocked" {
                "failed"
            } else {
                &status
            },
            &summary,
            artifacts,
            evidence,
        ))
    }

    /// supervisor.py:6038 `_release_resources`.
    pub async fn release_resources(&self, tree: &Arc<Tree>, parent_span: &str) -> bool {
        let mut ids = vec![tree.worker.id.clone(), format!("{}.verify", tree.worker.id)];
        for extra in ["fix", "recheck"] {
            let tid = format!("{}.{extra}", tree.worker.id);
            if self.task_known(&tid) {
                ids.push(tid);
            }
        }
        let mut observed = false;
        for tid in ids {
            let span = self.span_start(
                "task.cleanup",
                Some(&tid),
                Some(parent_span),
                None,
                json!({"operation": "release task resources"}),
            );
            let resources = self.deps.daemon.on_task_end(&tid, &span).await;
            let errors: Vec<String> = resources
                .iter()
                .filter_map(|r| {
                    r.get("error")
                        .filter(|e| ferb_py::value::truthy(e))
                        .map(ferb_py::value::py_str)
                })
                .collect();
            {
                let mut st = tree.lock();
                st.resources.extend(resources.clone());
                st.cleanup_errors.extend(errors.clone());
            }
            let error = errors.join("; ");
            self.span_finish(
                &span,
                Some(json!({"resources": resources})),
                if error.is_empty() { None } else { Some(&error) },
                None,
                None,
            );
            observed = observed || !resources.is_empty();
        }
        observed
    }

    /// supervisor.py:5018 `_record_planner_reads`.
    fn record_planner_reads(&self, tree: &Arc<Tree>, root_span: &str, request_id: &str) {
        {
            let mut st = tree.lock();
            if st.planner_reads {
                return;
            }
            st.planner_reads = true;
        }
        let Some(turn) = self.turn_of(request_id) else {
            return;
        };
        let (sources, exposures) = {
            let t = turn.lock().unwrap();
            (t.planner_sources.clone(), t.exposures.clone())
        };
        if !sources.is_empty() {
            let items: Vec<Value> = serde_json::from_str(&sources).unwrap_or_default();
            let mut held = BTreeSet::new();
            let mut refs = BTreeSet::new();
            for item in &items {
                if item["text"].as_str().is_some_and(|t| !t.is_empty()) {
                    held.extend(origin::split(item["origin"].as_str()));
                }
                for r in strings(item.get("source_refs")) {
                    refs.insert(r);
                }
            }
            let refs: Vec<String> = refs.into_iter().collect();
            self.source_block(
                tree,
                "planner-context",
                &sources,
                &origin::join(held.iter().map(String::as_str)),
                &refs,
                None,
            );
        }
        for exposure in exposures {
            let refs = strings(exposure.get("source_refs"));
            let span = self.span_start(
                "context.exposure",
                Some(&tree.id),
                Some(root_span),
                None,
                json!({"source_refs": crate::lines::stored_refs(&refs), "reference": exposure.get("reference")}),
            );
            let made = exposure
                .get("origin")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            self.span_finish(
                &span,
                Some(Value::Object(crate::lines::exposure_output(&exposure))),
                None,
                Some(&made),
                None,
            );
        }
    }

    /// supervisor.py:4097 `_task_made`.
    pub fn task_made(&self, tree_id: &str) -> String {
        let mut made = origin::kinds(self.task_origins(tree_id).iter().map(String::as_str));
        made.insert(origin::INFERRED.into());
        origin::join(made.iter().map(String::as_str))
    }

    /// supervisor.py:4102 `_tree_shown`.
    fn tree_shown(&self, tree_id: &str) -> Vec<ferb_policy::carried::Shown> {
        ferb_policy::carried::shown_records(&self.payload_outputs(tree_id))
    }

    /// supervisor.py:4917 `_finish_tree`.
    async fn finish_tree(
        &self,
        tree: &Arc<Tree>,
        root_span: &str,
        request_id: &str,
        stored: Option<Value>,
    ) -> Flow<Map<String, Value>> {
        let tree_id = tree.id.clone();
        let spec = tree.spec();
        self.record_planner_reads(tree, root_span, request_id);
        let mut final_ = obj(self.redact(&Value::Object(self.final_(tree)?)));
        let status = final_["status"].as_str().unwrap_or("").to_string();
        self.plans(|p| p.set_status(&tree_id, &status))?;
        self.deps.daemon.standing_revoke(&tree_id);
        final_.insert("goal".into(), json!(spec.goal));
        final_.insert(
            "name".into(),
            json!(ferb_orchestrator::names::shown_name(
                Some(&spec.name),
                &spec.goal
            )),
        );
        let answer = ferb_py::value::py_str(&final_["answer"]);
        let terms = ferb_orchestrator::language::internal_terms(&answer);
        if !terms.is_empty() {
            self.journal(
                "language.audit",
                json!({"task_id": tree_id, "terms": terms}),
                None,
            );
        }
        let made = self.task_made(&tree_id);
        let provenance = stored.unwrap_or_else(|| {
            let made: Vec<String> = origin::split(Some(&made)).into_iter().collect();
            ferb_policy::carried::derive(&answer, &self.tree_shown(&tree_id), &made)
        });
        let mut data = obj(
            json!({"task_id": tree_id, "request_id": request_id, "status": status,
            "text": answer, ferb_policy::carried::PROVENANCE: provenance}),
        );
        if let Some(w) = final_.get("watch_met") {
            data.insert("watch_met".into(), w.clone());
        }
        if let Some(t) = final_.get("told_in").filter(|t| ferb_py::value::truthy(t)) {
            data.insert("told_in".into(), t.clone());
        }
        if final_.contains_key("card") {
            data.insert("index".into(), json!(true));
        }
        self.journal("task.answer", Value::Object(data), Some(&made));
        self.span_finish(
            root_span,
            Some(Value::Object(final_.clone())),
            None,
            None,
            None,
        );
        self.set_status(&tree_id, &status, Some(&Value::Object(final_.clone())));
        let resources = tree.lock().resources.clone();
        self.journal(
            "task.outcome",
            json!({"task_id": tree_id, "status": status, "summary": final_["summary"], "verified": final_["verified"],
                   "verification": final_["verification"], "subtasks": final_["subtasks"], "resources": resources}),
            Some(&made),
        );
        self.deps.daemon.publish(
            "task.finished",
            json!({"task_id": tree_id, "status": status}),
        );
        let ids = self.tree_task_ids(tree);
        self.deps.daemon.forget_tasks(&ids);
        self.deps.daemon.task_folders(&tree_id, None);
        self.deps.daemon.forget_request(request_id);
        let members: Vec<String> = {
            let live = self.live();
            live.tree_of
                .iter()
                .filter(|(_, t)| t.id == tree_id)
                .map(|(k, _)| k.clone())
                .collect()
        };
        for tid in members {
            self.live().tree_of.remove(&tid);
            self.runs.forget_tree(&tid);
            self.task_remove(&tid);
        }
        let stop = tree.lock().stop.clone();
        for row in self.task_rows(&tree_id) {
            let s = row.get("status").and_then(Value::as_str).unwrap_or("");
            if ["running", "planned", "paused"].contains(&s) {
                let id = row
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                let status = if stop.is_empty() {
                    "cancelled".to_string()
                } else {
                    self.stop_status(tree)
                };
                let summary = if stop.is_empty() {
                    "Task ended before this step completed".to_string()
                } else {
                    stop.clone()
                };
                self.set_status(&id, &status, Some(&json!({"summary": summary})));
            }
        }
        self.record_episode(&tree_id);
        let me = self.me();
        let id = tree_id.clone();
        tokio::spawn(async move {
            tokio::task::yield_now().await;
            me.deps.daemon.on_episode(&id).await;
        });
        Ok(final_)
    }

    /// episodes.py `record`: the task's permanent record, composed from its trace.
    fn record_episode(&self, tree_id: &str) {
        let trace = json!({"task_id": tree_id, "spans": self.tree_json(tree_id), "outcome": self.span_outcome(tree_id)});
        let composed = ferb_trace::compose::compose(&trace);
        let now = self.now_iso();
        let failed = {
            let db = self.db();
            let mut failure = None;
            ferb_orchestrator::episodes::record(
                &db,
                || Ok(composed),
                &now,
                tree_id,
                |kind, data| {
                    failure = Some((kind.to_string(), data));
                    Ok(())
                },
            );
            failure
        };
        if let Some((kind, data)) = failed {
            self.journal(&kind, data, None);
        }
    }

    /// supervisor.py:6121 `_tree_task_ids`.
    fn tree_task_ids(&self, tree: &Arc<Tree>) -> BTreeSet<String> {
        let mut ids: BTreeSet<String> = self
            .live()
            .tree_of
            .iter()
            .filter(|(_, t)| t.id == tree.id)
            .map(|(k, _)| k.clone())
            .collect();
        ids.insert(tree.worker.id.clone());
        ids.insert(format!("{}.verify", tree.worker.id));
        ids
    }

    /// supervisor.py:7376 `_results`.
    #[allow(clippy::type_complexity)]
    pub fn results(
        &self,
        src: &Arc<Tree>,
    ) -> (
        Vec<ConditionResult>,
        Vec<ConditionResult>,
        bool,
        Vec<Condition>,
    ) {
        let st = src.lock();
        let spec = st.spec.clone().expect("a spec");
        let attempted = !st.verification.is_empty();
        let conditions = if attempted {
            crate::lines::checked_conditions(&spec.success_conditions, st.watch, st.watch_met)
        } else {
            spec.success_conditions.clone()
        };
        let results: Vec<ConditionResult> = conditions
            .iter()
            .enumerate()
            .map(|(i, c)| {
                st.verification
                    .iter()
                    .find(|r| r.index == i as i64)
                    .cloned()
                    .unwrap_or_else(|| {
                        ConditionResult::new(i as i64, &c.text, None, UNCHECKED, NOT_CHECKED)
                    })
            })
            .collect();
        let planned = spec.success_conditions.len() as i64 + i64::from(st.watch);
        let outcomes = results
            .iter()
            .filter(|r| r.index < planned)
            .cloned()
            .collect();
        (results, outcomes, attempted, conditions)
    }

    /// supervisor.py:6682 `_owner_text`.
    pub fn owner_text(&self, tree: &Arc<Tree>, result: &ConditionResult) -> String {
        let st = tree.lock();
        let conditions = &st.spec.as_ref().expect("a spec").success_conditions;
        if (result.index as usize) < conditions.len() && result.index >= 0 {
            let c = &conditions[result.index as usize];
            return if c.owner_text.is_empty() {
                result.text.clone()
            } else {
                c.owner_text.clone()
            };
        }
        if st.watch && result.index as usize == conditions.len() {
            return WATCH_TEXT.into();
        }
        result.text.clone()
    }

    /// supervisor.py:7388 `_checked_answer`.
    pub fn checked_answer(&self, src: &Arc<Tree>, out: &Outcome) -> (String, String, bool) {
        let (_, outcomes, attempted, conditions) = self.results(src);
        let declined = self.declined(src, out);
        let st = src.lock();
        let verified = (out.status == OutcomeStatus::Done || declined)
            && attempted
            && !st.stands_unconfirmed
            && !st.kept_contradicted
            && !outcomes.is_empty()
            && outcomes
                .iter()
                .all(|r| r.met == Some(true) || r.path == DECLINED);
        let unconfirmed = st.unconfirmed.clone();
        let (answer_unmarked, owner_answer) = (st.answer_unmarked.clone(), st.owner_answer.clone());
        drop(st);
        let note = if declined && verified {
            String::new()
        } else if !unconfirmed.is_empty() {
            unconfirmed
        } else if verified {
            String::new()
        } else if conditions.is_empty() {
            NOTHING_TO_CHECK.into()
        } else {
            outcomes
                .iter()
                .filter(|r| r.met != Some(true) && r.path != DECLINED)
                .map(|r| self.owner_text(src, r))
                .collect::<Vec<_>>()
                .join("; ")
        };
        let answer = if !note.is_empty() && !verified && !answer_unmarked.is_empty() {
            answer_unmarked
        } else {
            owner_answer
        };
        let answer = if answer.is_empty() {
            out.summary.clone()
        } else {
            answer
        };
        (
            ferb_orchestrator::py::strip(&answer).to_string(),
            if note.is_empty() {
                String::new()
            } else {
                crate::lines::clipped(&note)
            },
            verified,
        )
    }

    /// supervisor.py:7482 `_declined`.
    pub fn declined(&self, src: &Arc<Tree>, out: &Outcome) -> bool {
        {
            let st = src.lock();
            if !st.stop.is_empty()
                || st.stop_by_code
                || matches!(
                    out.status,
                    OutcomeStatus::NeedsUser | OutcomeStatus::Cancelled
                )
            {
                return false;
            }
        }
        let outcomes = self.results(src).1;
        outcomes.iter().any(|r| r.path == DECLINED)
            && !outcomes
                .iter()
                .any(|r| r.met == Some(false) && r.path != DECLINED)
    }

    /// supervisor.py:6651 `_told_failure`.
    pub fn told_failure(
        &self,
        src: &Arc<Tree>,
        outcomes: &[ConditionResult],
        why: &str,
    ) -> (String, String) {
        let st = src.lock();
        let not_done = !st
            .reports
            .last()
            .is_some_and(|r| r.get("status").and_then(Value::as_str) == Some("done"));
        let own = if not_done {
            ferb_orchestrator::py::strip(&st.owner_answer).to_string()
        } else {
            String::new()
        };
        if st.stop_by_code || st.reports.is_empty() {
            return (ferb_orchestrator::py::strip(why).to_string(), own);
        }
        let reason = ferb_orchestrator::py::strip(&st.reason).to_string();
        if not_done && (!own.is_empty() || !reason.is_empty()) {
            return (String::new(), if own.is_empty() { reason } else { own });
        }
        let conditions = st.spec.as_ref().expect("a spec").success_conditions.clone();
        let check_told = st.check_told.clone();
        drop(st);
        let mut told: Vec<String> = Vec::new();
        for r in outcomes {
            if r.met == Some(false) && r.path == "code" && (r.index as usize) < conditions.len() {
                let t = ferb_orchestrator::verify::not_done_said(&conditions[r.index as usize]);
                if !t.is_empty() && !told.contains(&t) {
                    told.push(t);
                }
            }
        }
        let lead = told.join(" ");
        let lead = if lead.is_empty() { check_told } else { lead };
        (
            ferb_orchestrator::py::strip(&lead).to_string(),
            String::new(),
        )
    }

    /// supervisor.py:7406 `_final`.
    pub fn final_(&self, tree: &Arc<Tree>) -> Flow<Map<String, Value>> {
        let src = tree
            .lock()
            .answering
            .clone()
            .unwrap_or_else(|| tree.clone());
        let out = tree.lock().outcome.clone().unwrap_or_else(|| {
            outcome(
                &tree.worker.id,
                "failed",
                "No report was received.",
                vec![],
                vec![],
            )
        });
        let (results, outcomes, _, _) = self.results(&src);
        let mut note = String::new();
        let mut verified = false;
        let stop = tree.lock().stop.clone();
        let (status, mut summary, mut answer);
        if stop == STOPPED_BY_YOU {
            status = self.stop_status(tree);
            summary = STOPPED_BY_YOU.to_string();
            answer = ferb_orchestrator::answer_marks::STOPPED.to_string();
            let found = src
                .lock()
                .reports
                .iter()
                .rev()
                .map(|r| {
                    ferb_orchestrator::py::strip(
                        &r.get("answer")
                            .map(ferb_py::value::py_str)
                            .unwrap_or_default(),
                    )
                    .to_string()
                })
                .find(|t| !t.is_empty());
            if let Some(found) = found {
                answer.push_str(&format!(
                    " {}\n\n{found}",
                    ferb_orchestrator::answer_marks::WHERE_STOOD
                ));
            }
        } else if !stop.is_empty() {
            status = self.stop_status(tree);
            summary = stop.clone();
            answer = format!("I couldn't finish that: {stop}");
        } else if self.declined(&src, &out) {
            status = "cancelled".into();
            let own = ferb_orchestrator::py::strip(&src.lock().owner_answer).to_string();
            summary = if own.is_empty() {
                ferb_orchestrator::answer_marks::DECLINED_LINE.to_string()
            } else {
                own
            };
            answer = summary.clone();
        } else if out.status == OutcomeStatus::Done
            && !outcomes
                .iter()
                .any(|r| r.met == Some(false) && r.path == "code")
        {
            let (text, n, v) = self.checked_answer(&src, &out);
            note = n;
            verified = v;
            status = if verified {
                "done".into()
            } else {
                "done_unverified".into()
            };
            answer = if note.is_empty() {
                text
            } else {
                format!("{text}\n\n{UNCONFIRMED_LINE}")
            };
            summary = answer.clone();
        } else if out.status == OutcomeStatus::NeedsUser {
            status = "needs_user".into();
            let question = {
                let s = src.lock().owner_question.clone();
                let t = tree.lock().owner_question.clone();
                if !s.is_empty() {
                    s
                } else if !t.is_empty() {
                    t
                } else {
                    out.summary.clone()
                }
            };
            answer =
                ferb_orchestrator::py::strip(&format!("{NEEDS_ANSWER} {question}")).to_string();
            summary = answer.clone();
        } else {
            status = "failed".into();
            let why = {
                let st = src.lock();
                if !st.stop_reason.is_empty() {
                    st.stop_reason.clone()
                } else if !st.reason.is_empty() {
                    st.reason.clone()
                } else if out.status != OutcomeStatus::Done {
                    out.summary.clone()
                } else {
                    NOT_ACCEPTED.into()
                }
            };
            let not_finished = ferb_orchestrator::answer_marks::NOT_FINISHED;
            summary = ferb_orchestrator::py::strip(&format!(
                "{not_finished} {}",
                crate::lines::sentence(&why)
            ))
            .to_string();
            let (lead, own) = self.told_failure(&src, &outcomes, &why);
            answer = ferb_orchestrator::py::strip(&format!(
                "{not_finished} {}",
                crate::lines::sentence(&lead)
            ))
            .to_string();
            if !own.is_empty() {
                answer.push_str(&format!("\n\n{own}"));
            }
        }
        summary = ferb_policy::confirm::from_task_lines(&summary);
        answer = ferb_policy::confirm::from_task_lines(&answer);
        let sites = self.deps.daemon.sites_contacted(&tree.id);
        let lines = self.code_lines(tree, &sites);
        if !lines.is_empty() {
            answer = format!("{answer}\n\n{}", lines.join("\n"));
        }
        let (switch, how, stop_reason) = {
            let (s_switch, s_how, s_reason) = {
                let s = src.lock();
                (
                    s.runtime_switch.clone(),
                    s.how.clone(),
                    s.stop_reason.clone(),
                )
            };
            let (t_switch, t_reason) = {
                let t = tree.lock();
                (t.runtime_switch.clone(), t.stop_reason.clone())
            };
            (
                s_switch.or(t_switch),
                s_how,
                if !s_reason.is_empty() {
                    s_reason
                } else {
                    t_reason
                },
            )
        };
        let mut artifacts = out.artifacts.clone();
        artifacts.sort();
        let cards = self.result_cards(tree, &artifacts, &sites);
        let resources = tree.lock().resources.clone();
        let mut final_ = obj(
            json!({"type": "outcome", "task_id": tree.id, "status": status, "summary": summary,
            "answer": answer, "verified": verified,
            "verification": results.iter().map(ConditionResult::as_dict).collect::<Vec<_>>(),
            "subtasks": {WORKER: {"status": out.status.as_str(), "summary": out.summary}},
            "artifacts": artifacts, "resources": resources, "stop_reason": stop_reason}),
        );
        if !note.is_empty() {
            final_.insert(
                "unconfirmed".into(),
                json!(ferb_policy::confirm::from_task_lines(&note)),
            );
        }
        if !lines.is_empty() {
            final_.insert("lines".into(), json!(lines));
        }
        if (status == "done" || status == "done_unverified")
            && !ferb_orchestrator::py::strip(&how).is_empty()
        {
            final_.insert("how".into(), json!(how));
        }
        if let Some(sw) = switch.filter(|_| status == "failed") {
            final_.insert("runtime_switch".into(), sw);
        }
        let (watch, watch_met) = {
            let st = tree.lock();
            (st.watch, st.watch_met)
        };
        if watch {
            if let Some(m) = watch_met {
                final_.insert("watch_met".into(), json!(m));
            }
        }
        if !cards.is_empty() {
            final_.insert("cards".into(), Value::Array(cards));
        }
        final_.extend(self.checklist(tree, Some(&status)));
        Ok(final_)
    }

    /// supervisor.py:4704 `_result_cards`.
    fn result_cards(&self, tree: &Arc<Tree>, artifacts: &[String], sites: &[String]) -> Vec<Value> {
        let side = tree.lock().side_cards.clone();
        let mut out: Vec<Value> = side.iter().map(|c| self.redact(c)).collect();
        out.extend(self.deps.daemon.ran_cards(&tree.id));
        if !sites.is_empty() {
            out.extend(ferb_orchestrator::cards::links(
                sites,
                &crate::lines::sites_line(sites),
            ));
        }
        out.extend(ferb_orchestrator::cards::files(artifacts, &self.deps.home));
        out
    }

    /// supervisor.py:7544 `_code_lines`.
    fn code_lines(&self, tree: &Arc<Tree>, sites: &[String]) -> Vec<String> {
        let (mut lines, resources, errors) = {
            let st = tree.lock();
            (
                st.side_lines.clone(),
                st.resources.clone(),
                st.cleanup_errors.clone(),
            )
        };
        if !sites.is_empty() {
            lines.push(crate::lines::sites_line(sites));
        }
        lines.extend(self.apps_left_running(&tree.id));
        if !errors.is_empty() {
            lines.extend(ferb_orchestrator::leftovers::leftover_lines(
                &resources, &errors,
            ));
        }
        lines
    }

    /// supervisor.py:7367 `_apps_left_running`.
    fn apps_left_running(&self, tree_id: &str) -> Vec<String> {
        let rows = {
            let db = self.db();
            ferb_orchestrator::sql::rows(
                db.conn(),
                "SELECT output_json FROM spans WHERE kind = 'task.cleanup' AND (task_id = ? OR substr(task_id, 1, ?) = ?) \
                 ORDER BY rowid",
                &[
                    rusqlite::types::Value::Text(tree_id.into()),
                    rusqlite::types::Value::Integer(tree_id.len() as i64 + 1),
                    rusqlite::types::Value::Text(format!("{tree_id}.")),
                ],
            )
            .unwrap_or_default()
        };
        let records: Vec<Value> = rows
            .iter()
            .flat_map(|r| {
                let out = r.get("output_json").and_then(Value::as_str).unwrap_or("{}");
                let v = ferb_py::json::loads(out).unwrap_or(Value::Null);
                v.get("resources")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default()
            })
            .collect();
        let mut out: Vec<String> = Vec::new();
        for line in self.deps.daemon.left_running(&records) {
            if !out.contains(&line) {
                out.push(line);
            }
        }
        out
    }
}
