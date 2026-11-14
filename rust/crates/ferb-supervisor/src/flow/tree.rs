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

/// supervisor.py `RESTORED_WAITS`: the waits a restart asks again as themselves.
pub const RESTORED_WAITS: [&str; 2] = ["check_in", "runtime_switch"];

/// `run_tree`'s keyword arguments.
#[derive(Clone, Debug, Default)]
pub struct RunOpts {
    pub conversation: Option<String>,
    pub prior_task: Option<String>,
    /// What the request's planning turn was given (S3); empty: unlabeled outside content.
    pub origins: BTreeSet<String>,
    /// The tree to resume in place (a restart, a continue or retry of an ended task).
    pub resume_id: Option<String>,
    /// Resumed after a restart: its steps carry on as saved, and nothing is said again.
    pub restored: bool,
    /// The standing scopes in force for a run nobody attends (None: the owner is present).
    pub unattended_scopes: Option<Vec<Value>>,
    /// D30: scopes recorded with the owner earlier (a schedule's) and their approval id.
    pub start_grant: Option<(Vec<Value>, String)>,
    /// A watch's check (fix wave 6c).
    pub watch: bool,
    /// The schedule run this is (kept with the plan).
    pub schedule: Option<Map<String, Value>>,
}

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

pub(crate) fn outcome(
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

    /// A suspension point of Python's (an `await` that yields to its event loop) where the order of a chat's frames
    /// depends on it: what the tree's line and the bus hold now reaches a chat stream as one burst, before anything
    /// told after it, however the stream's task is woken (a wake mark on the line: chat.rs `as_python_wakes`).
    pub(crate) fn python_wakes_here(&self, tree_id: &str) {
        let mut mark = Map::new();
        mark.insert(super::chat::WAKE.into(), Value::Bool(true));
        self.tell_tree(tree_id, mark);
    }

    /// A subtask event on the tree's line (Python: `queue.put` on the same queue).
    fn subtask(&self, tree_id: &str, event: Value) {
        let mut event = obj(event);
        let own = event.get("task_id").cloned().unwrap_or(Value::Null);
        event.insert(SUBTASK_ID.into(), own);
        self.tell_tree(tree_id, event);
    }

    /// supervisor.py:4465 `run_tree`: one task's run; its events on `emit`. `release`: the turn lets the chat go once
    /// the plan line is out. A shutdown suspends it (restart.rs): nothing records an end, and it resumes after the
    /// restart in place (`opts.resume_id`, `opts.restored`).
    #[allow(clippy::too_many_arguments)] // run_tree's own arguments
    pub async fn run_tree(
        &self,
        spec: TaskSpec,
        text: &str,
        request_id: &str,
        message: &str,
        opts: RunOpts,
        emit: &Emit,
        release: &(dyn Fn() + Send + Sync),
    ) -> Flow<()> {
        // L34 Phase G: an update's hold (the shutdown ends the wait)
        self.work_opened().await;
        let tree_id = opts.resume_id.clone().unwrap_or_else(|| self.new_id());
        let conversation = opts.conversation.as_deref();
        let prior_task = opts.prior_task.as_deref();
        let origins = &opts.origins;
        let restored = opts.restored;
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
        let schedule_id = opts
            .schedule
            .as_ref()
            .and_then(|s| s.get("id"))
            .map(ferb_py::value::py_str);
        let schedule_facts = opts
            .schedule
            .as_ref()
            .and_then(|s| s.get("facts"))
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        let tree = Arc::new(Tree {
            id: tree_id.clone(),
            worker: worker.clone(),
            step_id: WORKER.into(),
            asks_in_place: true,
            st: Mutex::new(TreeState {
                spec: Some(spec.clone()),
                prior_task: prior_task.map(str::to_string),
                accepting_updates: true,
                unattended_scopes: opts.unattended_scopes.clone(),
                watch: opts.watch,
                schedule_id,
                schedule_facts,
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
                if let Some(front) = t.front() {
                    // L12: the worker starts from the planner's picture (D26 clarified)
                    st.start_image = t.front_image().cloned();
                    let wrap = |text: &str| self.wrap(text);
                    let withheld = t.seen.as_ref().and_then(|s| s.withheld.clone());
                    st.start_block = Some(
                        crate::planner::front_window_block(
                            Some(front),
                            st.start_image.is_some(),
                            START_SCREENSHOT,
                            withheld.as_deref(),
                            None,
                            None,
                            &wrap,
                        )
                        .block,
                    );
                }
                (
                    t.fronts()
                        .iter()
                        .map(|f| f.origin.clone())
                        .collect::<Vec<_>>(),
                    // a scheduled run's request carries what the message that made it carried (L4)
                    if t.request_origins.is_empty() {
                        t.message_origin.clone()
                    } else {
                        t.request_origins.clone()
                    },
                )
            }
            None => (Vec::new(), BTreeSet::new()),
        };
        if let Some(t) = &turn {
            let mut t = t.lock().unwrap();
            if !t.said_first.is_empty() && !restored {
                // ORCH-638: said once, in this plan event
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
        let mut saved = obj(
            json!({"spec": dump_task_spec(&spec).to_value(), "request_id": request_id,
            "words": request.words, "payload": request.payload,
            "origins": origins.iter().collect::<Vec<_>>(), "message_origins": message_origins,
            "unattended": unattended}),
        );
        if let Some(schedule) = opts.schedule.as_ref().filter(|s| !s.is_empty()) {
            saved.insert("schedule".into(), Value::Object(schedule.clone()));
        }
        let saved = Value::Object(saved);
        match &spec.plan {
            Some(plan) => self.plans(|p| p.create(&tree_id, plan, &saved, conversation))?,
            None => self
                .plans(|p| p.create_single(&tree_id, &spec.goal, &saved, conversation, restored))?,
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
            // ORCH-319: several asks, one card; its events go on the task's line and the bus
            let card = self.new_card(&tree_id, request_id, spec.parts.clone());
            tree.lock().card = Some(card);
        }
        self.deps.daemon.task_folders(&tree_id, Some(&spec.folders));
        if spec.keep_tabs_open {
            self.deps.daemon.keep_tabs_open(&tree_id);
        }
        if !opts.watch {
            self.deps.daemon.publish(
                "task.created",
                json!({"task_id": tree_id, "goal": spec.goal,
                       "name": ferb_orchestrator::names::shown_name(Some(&spec.name), &spec.goal)}),
            );
        }
        if let Some((grant, approval)) = opts.start_grant.as_ref().filter(|g| !g.0.is_empty()) {
            // D30: scopes recorded with the owner earlier (a schedule's), checked again now
            if self.deps.daemon.grant_in_force(approval) {
                self.deps.daemon.standing_grant(&tree_id, grant, approval);
            } else {
                tree.lock().unattended_scopes = Some(Vec::new());
                self.journal(
                    "standing.withheld",
                    json!({"task_id": tree_id, "approval_id": approval,
                           "reason": "schedule no longer active"}),
                    None,
                );
            }
        }
        self.journal(
            "chat.plan",
            json!({"request_id": request_id, "task_id": tree_id}),
            None,
        );
        let mut stated = String::new();
        if let Some(plan) = &spec.plan {
            let approved = self
                .plans(|p| p.load(&tree_id))
                .map(|l| l.row.get("approved").is_some_and(ferb_py::value::truthy))
                .unwrap_or(false);
            if (plan.big_job || !plan.standing_sends.is_empty()) && !approved {
                // D24/D30 (L3): a big job's plan and any standing scope are stated here, never asked; work starts
                // and a reply corrects it (a steer)
                stated.push_str(&self.stated_line(&tree, plan));
            }
            if plan.steps.iter().any(Step::loops) {
                // how each loop ends (L31)
                let lines: Vec<String> = plan
                    .steps
                    .iter()
                    .filter(|s| s.loops())
                    .map(ferb_orchestrator::loops::loop_line)
                    .collect();
                stated.push('\n');
                stated.push_str(&self.redact_text(&lines.join("\n")));
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
        let plan_event = self.plan_event(&tree, text, &stated, restored);
        release();
        let _ = emit.send(Value::Object(plan_event));
        self.record_planner_reads(&tree, &root_span, request_id);
        self.browse_as(&tree).await; // APP-777: before any worker of the tree starts
        self.deps.daemon.on_task_start(&tree_id).await;
        let mut rx = self.runs.open_live(&tree_id);
        self.live().live_lines.insert(tree_id.clone());
        let forward = |mut ev: Map<String, Value>| {
            if let Some(own) = ev.remove(SUBTASK_ID) {
                ev.insert("task_id".into(), own);
            }
            let _ = emit.send(Value::Object(ev));
        };
        let mut run = Box::pin(self.execute(&tree, &root_span));
        // the shutdown's cancellation comes first: once it began, nothing more of this run is told
        let completed = loop {
            if self.shutting_down_now() {
                break false;
            }
            tokio::select! {
                biased;
                _ = self.shutting_down() => break false,
                Some(ev) = rx.recv() => forward(ev),
                done = &mut run => break done,
            }
        };
        drop(run);
        if completed {
            while let Ok(ev) = rx.try_recv() {
                forward(ev);
            }
        }
        self.runs.close_live(&tree_id);
        self.live().live_lines.remove(&tree_id);
        if !completed {
            // Python's `except BaseException`: the run was cancelled (FERB's shutdown)
            {
                let suspending = self.live().suspending;
                let mut st = tree.lock();
                if st.stop.is_empty() {
                    st.stop = if suspending {
                        SHUT_DOWN.into()
                    } else {
                        "FERB was stopped".into()
                    };
                }
            }
            self.journal(
                "task.interrupted",
                json!({"task_id": tree_id, "error": "CancelledError"}),
                None,
            );
            self.cancel_tree(&tree);
            self.release_resources(&tree, &root_span).await;
        } else {
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
        }
        // FERB's shutdown suspends the tree: it isn't over, and resumes after the restart, so nothing records an end
        // and nothing of the task-end clean-up runs (ORCH-510)
        let suspended = {
            let suspending = self.live().suspending;
            let st = tree.lock();
            suspending && (st.outcome.is_none() || st.stop == SHUT_DOWN)
        };
        let mut stored = None;
        if !suspended {
            self.deps.daemon.on_task_finish(&tree_id).await;
            let answer = ferb_py::value::py_str(
                self.redact(&Value::Object(self.final_(&tree)?))
                    .get("answer")
                    .unwrap_or(&Value::Null),
            );
            let made: Vec<String> = origin::split(Some(&self.task_made(&tree_id)))
                .into_iter()
                .collect();
            stored = Some(ferb_policy::carried::derive(
                &answer,
                &self.tree_shown(&tree_id),
                &made,
            ));
        }
        let final_ = self
            .finish_tree(&tree, &root_span, request_id, suspended, stored)
            .await?;
        if !suspended && completed {
            let _ = emit.send(Value::Object(final_));
        }
        if !completed {
            return Err(Failure::Cancelled);
        }
        Ok(())
    }

    /// supervisor.py:4637 `_plan_event`.
    fn plan_event(
        &self,
        tree: &Arc<Tree>,
        text: &str,
        stated: &str,
        restored: bool,
    ) -> Map<String, Value> {
        let spec = tree.spec();
        let card = tree.lock().card.clone();
        let mut text = text.to_string();
        if card.is_some() {
            for part in &spec.parts {
                if let Some(answer) = part
                    .direct
                    .as_ref()
                    .filter(|d| !ferb_orchestrator::py::strip(d).is_empty())
                {
                    text = crate::lines::without_whole(&text, answer);
                }
            }
        }
        let text = text.as_str();
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
        if restored {
            event.insert("resumed".into(), json!(true));
        }
        let Some(card) = card else {
            event.extend(self.checklist(tree, None));
            return self.said_first(tree, event);
        };
        let said: Vec<Value> = if restored {
            Vec::new()
        } else {
            self.with_card(&tree.id, &card, |c| c.say_direct())
        };
        let direct: Vec<Value> = said
            .into_iter()
            .map(|mut d| {
                let t = d["text"].as_str().unwrap_or("").to_string();
                d["text"] = json!(ferb_policy::confirm::from_task_lines(&self.redact_text(&t)));
                d
            })
            .collect();
        let (side_lines, side_cards) = {
            let mut st = tree.lock();
            (
                std::mem::take(&mut st.side_lines),
                std::mem::take(&mut st.side_cards),
            )
        };
        let receipts: Vec<String> = if restored {
            Vec::new()
        } else {
            side_lines
                .iter()
                .map(|l| ferb_policy::confirm::from_task_lines(&self.redact_text(l)))
                .collect()
        };
        let said_cards = if restored {
            Vec::new()
        } else {
            self.said_cards(&side_cards, true)
        };
        let before = event["text"].as_str().unwrap_or("").to_string();
        let joined: Vec<String> = [receipts.join("\n"), before]
            .into_iter()
            .filter(|p| !ferb_orchestrator::py::strip(p).is_empty())
            .collect();
        event.insert("text".into(), json!(joined.join("\n\n")));
        event.insert("direct".into(), Value::Array(direct));
        event.insert("receipts".into(), json!(receipts));
        event.insert("card".into(), Value::Object(self.card_payload(tree, &card)));
        if !said_cards.is_empty() {
            event.insert("cards".into(), Value::Array(said_cards));
        }
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
    pub(crate) fn plan_lines(&self, tree_id: &str) -> Vec<Value> {
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
    /// supervisor.py:4754 `checklist_state`: a running single ask's checklist now (chat.status).
    pub fn checklist_state(&self, tree_id: &str) -> Option<Value> {
        let tree = self.live().tree_of.get(tree_id).cloned()?;
        if tree.id != tree_id {
            return None;
        }
        self.checklist(&tree, None).get("checklist").cloned()
    }

    /// supervisor.py:2246 `tree_name`: a running tree's heading (its spec's short name, else its goal's first clause).
    pub fn tree_name(&self, key: &str) -> String {
        let tree = self.live().tree_of.get(key).cloned();
        match tree.and_then(|t| t.lock().spec.clone()) {
            Some(spec) => ferb_orchestrator::names::shown_name(Some(&spec.name), &spec.goal),
            None => String::new(),
        }
    }

    /// A watch check's tree (`tree.watch`): no client adopts it (I3).
    pub fn is_watch(&self, key: &str) -> bool {
        let tree = self.live().tree_of.get(key).cloned();
        tree.is_some_and(|t| t.lock().watch)
    }

    fn checklist(&self, tree: &Arc<Tree>, ended: Option<&str>) -> Map<String, Value> {
        if tree.lock().card.is_some() {
            return Map::new();
        }
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
    pub(crate) fn step_payload(&self, tree: &Arc<Tree>) -> Map<String, Value> {
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
        if self.live().suspending {
            return;
        }
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
        let row = self.part_of(tree_id, step_id);
        if let Some((_, part)) = &row {
            state.insert("part".into(), json!(part.id));
        }
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
        let request = match &row {
            Some((card, _)) => card.lock().request_id.clone(),
            None => match root.lock().card.clone() {
                Some(card) => card.lock().request_id.clone(),
                None => root.worker.request_id.clone(),
            },
        };
        let mut event = obj(
            json!({"type": "step", "task_id": tree_id, "request_id": request, "step": step_id,
            "step_task_id": step_task(tree_id, step_id)}),
        );
        event.extend(state);
        self.tell_tree(tree_id, event.clone());
        self.deps.daemon.publish("task.step", Value::Object(event));
        if status == "wont_run" && row.is_some() {
            // ORCH-532: a row whose step can never run is settled at once
            self.settle_wont_run(&root, step_id);
        }
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

    /// supervisor.py:6094 `_execute`. False: cancelled (a shutdown cut the step short; it runs again after the
    /// restart), nothing recorded.
    async fn execute(&self, tree: &Arc<Tree>, span: &str) -> bool {
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
                Err(Failure::Cancelled) => return false,
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
        true
    }

    /// supervisor.py:1784 `_stop_status`.
    pub fn stop_status(&self, tree: &Arc<Tree>) -> String {
        if tree.lock().stop == STOPPED_BY_YOU {
            "cancelled".into()
        } else {
            "failed".into()
        }
    }

    /// supervisor.py:1778 `_stopped`: the one stop check: the task was stopped (by the owner, or FERB was stopped),
    /// and FERB isn't merely shutting down (a shutdown suspends the tree; it resumes after the restart).
    pub fn stopped(&self, tree: &Arc<Tree>) -> bool {
        !tree.lock().stop.is_empty() && !self.live().suspending
    }

    /// `tree.stop`: any stop words, a shutdown's included (the attempt loop's own check).
    pub fn has_stop(&self, tree: &Arc<Tree>) -> bool {
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

    /// `_execute_plan`'s rounds: a correction that arrived after a step ended unfinished reaches the work still to do:
    /// the unfinished steps run again under it, once per correction.
    async fn run_plan_rounds(&self, tree: &Arc<Tree>, span: &str) -> Flow<Value> {
        loop {
            let result = self.run_plan(tree, span).await?;
            let steering = tree.steering.lock().await.messages.len();
            let states = self.plans(|p| p.steps(&tree.id))?;
            let unheard = states.iter().any(|(_, s)| {
                ["failed", "blocked", "needs_user"].contains(&s.status.as_str())
                    && (s
                        .output
                        .get("steers_seen")
                        .and_then(Value::as_i64)
                        .unwrap_or(steering as i64)
                        < steering as i64)
            });
            let status = result["status"].as_str().unwrap_or("").to_string();
            if status == "done"
                || status == "cancelled"
                || self.stopped(tree)
                || self.live().suspending
                || !unheard
            {
                return Ok(result);
            }
            self.plans(|p| p.retry_unfinished(&tree.id))?;
        }
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
            match self.wait_step(tree, &step).await {
                Ok(v) => v,
                Err(Failure::Cancelled) => return Err(Failure::Cancelled),
                Err(e) => {
                    json!({"status": "failed", "summary": format!("{}: {}", e.class(), e.message())})
                }
            }
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
            let card = self.ask_card(step, &prior);
            let options: Vec<&str> = step.options.iter().map(String::as_str).collect();
            return Ok(Value::Object(
                self.plan_question(tree, &step.question, &options, TASK_ASK, &step.id, card)
                    .await,
            ));
        }
        if step.kind == "converse" || step.loops() {
            // a loop or a conversation that answers: its end check also judges the task's conditions it answers
            let spec = tree.spec();
            let answering = answer_steps(spec.plan.as_ref(), &spec.parts);
            let answers = self.answer_conditions(&spec, &step.id, &answering);
            let result = if step.kind == "converse" {
                self.conversation_steps(tree, step, span, &prior, answers)
                    .await?
            } else {
                self.loop_step(tree, step, span, prior, answers).await?
            };
            let result = self.with_answer_check(&tree.id, &step.id, result);
            let status = result
                .get("status")
                .filter(|s| ferb_py::value::truthy(s))
                .map(ferb_py::value::py_str)
                .unwrap_or_else(|| "failed".into());
            self.post_part(tree, &step.id, None, &status, &result);
            return Ok(Value::Object(result));
        }
        let previous = prior
            .get("previous_attempt")
            .cloned()
            .unwrap_or(Value::Null);
        // a check-in or runtime switch it waited on when FERB restarted is asked again as itself (`_restored`)
        let paused = previous.get("status").and_then(Value::as_str) == Some("paused");
        let restored: Map<String, Value> = if paused
            && previous
                .get("question_kind")
                .and_then(Value::as_str)
                .is_some_and(|k| RESTORED_WAITS.contains(&k))
        {
            previous.as_object().cloned().unwrap_or_default()
        } else {
            Map::new()
        };
        let fixing: Map<String, Value> = previous
            .get("fixing")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        if paused && restored.is_empty() && !self.live().pauses.contains_key(&tree.id) {
            // it waited for the owner when FERB restarted: it pauses again and asks again (rule 9)
            let options: Vec<String> = previous
                .get("options")
                .and_then(Value::as_array)
                .map(|a| a.iter().map(ferb_py::value::py_str).collect())
                .unwrap_or_default();
            let options: Vec<&str> = options.iter().map(String::as_str).collect();
            let made = previous
                .get("origin")
                .filter(|o| ferb_py::value::truthy(o))
                .map(ferb_py::value::py_str)
                .unwrap_or_else(|| origin::FERB.to_string());
            let question = previous
                .get("question")
                .filter(|q| ferb_py::value::truthy(q))
                .map(ferb_py::value::py_str)
                .unwrap_or_default();
            let wait = super::owner_wait::Wait::new(&tree.id, &question, &options, &made, TASK_ASK);
            self.live().pauses.insert(tree.id.clone(), Arc::new(wait));
        }
        let spec = tree.spec();
        let single = spec.plan.is_none();
        if single {
            {
                let mut st = tree.lock();
                st.restored_wait = restored.clone();
                st.restored_fix = fixing.clone();
            }
            let out = self.worker_loop(tree, span).await?;
            if self.live().suspending {
                // a shutdown cut it short: it reports nothing and runs again after the restart
                return Err(Failure::Cancelled);
            }
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
            self.post_part(tree, &step.id, Some(tree), &status, &output);
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
        let mut child = tree.worker_task();
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
        {
            let mut st = member.lock();
            st.restored_wait = restored;
            st.restored_fix = fixing;
        }
        let ran = self.worker_loop(&member, span).await;
        let ran = match ran {
            // as for a single task's step: it runs again after the restart
            Ok(_) if self.live().suspending => Err(Failure::Cancelled),
            other => other,
        };
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
                self.post_part(tree, &step.id, Some(&member), &status, &output);
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

    /// plan_executor.py:47 `_wait`: a wait step, until its time (on the wall clock, looked at again at least every
    /// POLL_S) or its plan event after the step began (`notify_plan_event` wakes it); a stop ends it.
    async fn wait_step(&self, tree: &Arc<Tree>, step: &Step) -> Flow<Value> {
        let saved = self
            .plans(|p| p.steps(&tree.id))?
            .into_iter()
            .find(|(k, _)| *k == step.id)
            .map(|(_, s)| s.output)
            .unwrap_or(Value::Null);
        let cursor = match saved.get("event_cursor").and_then(Value::as_i64) {
            Some(c) => c,
            None => self.plans(|p| p.event_cursor())?,
        };
        self.plans(|p| {
            p.finish(
                &tree.id,
                &step.id,
                &json!({"status": "waiting", "event_cursor": cursor}),
            )
        })?;
        let mut facts = obj(json!({"loop": step.loops()}));
        match &step.event {
            Some(e) => facts.insert("event".into(), json!(e)),
            None => facts.insert("until".into(), json!(step.until)),
        };
        self.step_state(&tree.id, &step.id, "waiting", facts);
        let wake = self.plan_wake(&tree.id);
        while !self.stopped(tree) {
            if let Some(event) = &step.event {
                let found = self.plans(|p| p.event_after(event, cursor, &step.event_match))?;
                if let Some(payload) = found {
                    return Ok(json!({"status": "done", "summary": step.goal, "event": payload}));
                }
                wake.notified().await;
            } else {
                let until = step.until.clone().unwrap_or_default();
                let at = ferb_clock::py::parse(&until.replace('Z', "+00:00")).ok_or_else(|| {
                    Failure::raised(
                        "ValueError",
                        format!(
                            "Invalid isoformat string: {}",
                            ferb_py::text::str_repr(&until)
                        ),
                    )
                })?;
                let now = self.now();
                if at <= now {
                    return Ok(json!({"status": "done", "summary": step.goal}));
                }
                let left = (at - now).num_milliseconds().max(0) as f64 / 1000.0;
                let wait = left.min(ferb_clock::timers::POLL_S);
                tokio::select! {
                    _ = wake.notified() => {}
                    _ = tokio::time::sleep(std::time::Duration::from_secs_f64(wait)) => {}
                }
            }
        }
        Ok(json!({"status": "cancelled", "summary": "stopped by you"}))
    }

    /// The plan run's wake (plan_executor.py `PlanExecutor.wake`): a plan event, a steer or a stop for this tree.
    pub(crate) fn plan_wake(&self, tree_id: &str) -> Arc<tokio::sync::Notify> {
        self.live()
            .plan_wakes
            .entry(tree_id.to_string())
            .or_insert_with(|| Arc::new(tokio::sync::Notify::new()))
            .clone()
    }

    /// supervisor.py:2307 `notify_plan_event`: an event a running plan's wait step (or loop) waits for is stored
    /// (`plan_events`) and wakes that plan. `data` comes redacted.
    pub fn notify_plan_event(&self, topic: &str, data: &Value) {
        if topic == "browser.owner_stopped" {
            // HAND-201: the owner closed a tab FERB holds for a task, or its whole group
            let me = self.me();
            let data = data.clone();
            tokio::spawn(async move {
                me.owner_stopped_part(&data).await;
            });
        }
        // supervisor.py:2297 `_waits_for`: the runs whose plan has a step waiting for this topic (Python caches the
        // topics per plan revision; each running plan is read here)
        let runs: Vec<String> = self.live().plan_runs.iter().cloned().collect();
        let interested: Vec<String> = runs
            .into_iter()
            .filter(|key| {
                self.plans(|p| p.load(key))
                    .map(|l| l.plan.steps.iter().any(|s| s.wakes_on() == Some(topic)))
                    .unwrap_or(false)
            })
            .collect();
        if interested.is_empty() {
            return;
        }
        let _ = self.plans(|p| p.event(topic, data));
        for key in interested {
            self.plan_wake(&key).notify_waiters();
            self.wake_loops(&key); // a loop step waiting on this event looks now (L31)
        }
    }

    /// supervisor.py:2072 `owner_stopped_part` (HAND-201): the owner closed a tab FERB holds for a task, or its whole
    /// group: that counts as the owner stopping that part: the task pauses and FERB asks whether to continue (a loop:
    /// whether to stop it, Stop / Keep watching).
    pub async fn owner_stopped_part(&self, data: &Value) -> bool {
        let task_id = data
            .get("task_id")
            .filter(|v| ferb_py::value::truthy(v))
            .map(ferb_py::value::py_str)
            .unwrap_or_default();
        let tree = self.live().tree_of.get(&task_id).cloned();
        let Some(tree) = tree.filter(|t| t.lock().stop.is_empty()) else {
            return false;
        };
        let group = data.get("part").and_then(Value::as_str) == Some("group");
        let what = if group {
            "the task's tab group"
        } else {
            "a tab I was using"
        };
        let goal = ferb_orchestrator::names::short_label(&tree.spec().goal, 60);
        let (question, options, capability): (String, Vec<&str>, &str) = if task_id
            .contains(".loop.")
        {
            // a loop holding the owner's own tab names its site, as the loop's own closed-tab question does (L31)
            let wanted = data
                .get("tab")
                .map(ferb_py::value::py_str)
                .unwrap_or_default();
            let tab = self
                .deps
                .daemon
                .task_tabs(&task_id)
                .into_iter()
                .find(|t| t.get("tab").map(ferb_py::value::py_str) == Some(wanted.clone()))
                .unwrap_or(Value::Null);
            let taken = tab
                .get("taken_over_from")
                .filter(|v| ferb_py::value::truthy(v));
            let question = match taken {
                Some(from) => {
                    let page = from
                        .get("url")
                        .filter(|u| ferb_py::value::truthy(u))
                        .map(ferb_py::value::py_str)
                        .unwrap_or_default();
                    let host = if page.is_empty() {
                        String::new()
                    } else {
                        ferb_py::url::hostname(&page)
                            .ok()
                            .flatten()
                            .unwrap_or_default()
                    };
                    format!("The {host} tab you asked me to keep going was closed. Stop this task?")
                        .replace("The  tab", "The tab")
                }
                None => format!("You closed {what} for this: {goal}. Stop this task?"),
            };
            (question, vec![LOOP_STOP, LOOP_KEEP], LOOP_ASK)
        } else {
            (
                format!("You closed {what} for this: {goal}. Should I continue?"),
                vec![CONTINUE, STOP],
                TASK_ASK,
            )
        };
        self.journal(
            "task.owner_stopped",
            json!({"task_id": tree.id, "part": if group { "group" } else { "tab" },
                   "borrowed": data.get("borrowed").is_some_and(ferb_py::value::truthy)}),
            None,
        );
        self.pause_tree(&tree.id, &question, &options, origin::FERB, capability)
            .await
    }

    /// supervisor.py:5039 `_plan_question`: a plan's ask step (or a delegated conversation's scope approval), through
    /// the one wait for the owner; a run nobody attends can't be asked and reports it at once; a stop closes it as
    /// superseded, never a No.
    pub(crate) async fn plan_question(
        &self,
        tree: &Arc<Tree>,
        question: &str,
        options: &[&str],
        capability: &str,
        step_id: &str,
        card: Map<String, Value>,
    ) -> Map<String, Value> {
        let task = tree.worker_task();
        let (reply, wait) = loop {
            let mut wait = super::owner_wait::Wait::new(
                &tree.id,
                question,
                options,
                origin::INFERRED,
                capability,
            );
            wait.card = card.clone();
            wait.lock().extra.insert(
                "question_kind".into(),
                json!(
                    if capability == ferb_orchestrator::steering::CONVERSATION_ASK {
                        "conversation_scope"
                    } else {
                        "task"
                    }
                ),
            );
            let wait = Arc::new(wait);
            let reply = self
                .wait_for_owner(tree, &task, &wait, step_id, false, true)
                .await;
            // set aside by the owner's correction (ORCH-538): the step's question is asked again, never skipped
            if reply.kind != "steer" || !tree.lock().stop.is_empty() {
                break (reply, wait);
            }
        };
        if reply.kind == "declined" {
            return obj(json!({"status": "needs_user",
                "summary": format!("You need to answer this before I continue: {question}"), "question": question}));
        }
        let approval = wait.lock().approval.clone();
        match approval {
            Some(id) if reply.kind == "answer" => {
                obj(json!({"status": "done", "summary": reply.words,
                "answer": reply.words, "approval_id": id}))
            }
            _ => obj(
                json!({"status": "blocked", "summary": "I need your answer before I continue."}),
            ),
        }
    }

    /// supervisor.py:5061 `_ask_card` (ORCH-623): an ask step's card shows the checked answers of the steps it depends
    /// on, whole, in its depends_on order; {} when those answered nothing.
    fn ask_card(&self, step: &Step, prior: &Map<String, Value>) -> Map<String, Value> {
        let said: Vec<String> = step
            .depends_on
            .iter()
            .map(|d| {
                prior
                    .get(d)
                    .and_then(|p| p.get("answer"))
                    .filter(|a| ferb_py::value::truthy(a))
                    .map(ferb_py::value::py_str)
                    .map(|a| ferb_orchestrator::py::strip(&a).to_string())
                    .unwrap_or_default()
            })
            .collect();
        let shown = said
            .into_iter()
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join("\n\n");
        if shown.is_empty() {
            Map::new()
        } else {
            obj(json!({"detail": self.redact_text(&shown)}))
        }
    }

    /// supervisor.py:5359 `_adopt_saved`: the answer of answer steps that ended before this run (a restart), from
    /// what each saved: its answer, its "Couldn't confirm" line and its check.
    fn adopt_saved(&self, tree: &Arc<Tree>, outputs: &[Value]) -> Flow<()> {
        let said = |o: &Value| -> String {
            let pick = o
                .get("answer")
                .filter(|v| ferb_py::value::truthy(v))
                .or_else(|| o.get("summary").filter(|v| ferb_py::value::truthy(v)));
            ferb_orchestrator::py::strip(&pick.map(ferb_py::value::py_str).unwrap_or_default())
                .to_string()
        };
        let answer = outputs
            .iter()
            .map(said)
            .filter(|t| !t.is_empty())
            .collect::<Vec<_>>()
            .join("\n\n");
        let unconfirmed = outputs
            .iter()
            .filter_map(|o| o.get("note").filter(|n| ferb_py::value::truthy(n)))
            .map(ferb_py::value::py_str)
            .collect::<Vec<_>>()
            .join("; ");
        let planned = tree.spec().success_conditions.len() as i64;
        let one = outputs.len() == 1;
        let verification = if one
            && outputs[0]
                .get("verification")
                .is_some_and(ferb_py::value::truthy)
        {
            Some(super::attempt::saved_results(
                outputs[0].get("verification"),
                planned,
            )?)
        } else {
            None
        };
        let mut st = tree.lock();
        st.owner_answer = answer;
        st.unconfirmed = unconfirmed;
        if let Some(v) = verification {
            st.verification = v;
        }
        st.stands_unconfirmed = !one
            || !outputs[0]
                .get("verified")
                .is_some_and(ferb_py::value::truthy)
            || outputs[0].get("status").and_then(Value::as_str) != Some("done");
        Ok(())
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
        let part = spec
            .parts
            .iter()
            .find(|p| p.answer_step.as_deref() == Some(step.id.as_str()));
        let said = match part {
            Some(p) if tree.lock().card.is_some() => {
                format!(
                    "the answer to this part of their request: \u{201c}{}\u{201d}",
                    p.quote
                )
            }
            _ => "the task's answer".to_string(),
        };
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
            // D24/D30 (L3): the plan line stated the plan and its standing scope; work starts at once and the owner
            // corrects by replying (a steer). Nothing waits for a Yes. The grant names the recorded statement.
            self.state_plan(tree, &plan)?;
        }
        let spec = tree.spec();
        let single = spec.plan.is_none();
        let answering = answer_steps(spec.plan.as_ref(), &spec.parts);
        self.live().plan_runs.insert(tree.id.clone());
        let ran = self.run_plan_rounds(tree, span).await;
        self.live().plan_runs.remove(&tree.id);
        let result = ran?;
        let states = self.plans(|p| p.steps(&tree.id))?;
        let state = |id: &str| states.iter().find(|(k, _)| k == id).map(|(_, s)| s.clone());
        self.close_rows(tree, &states);
        let card = tree.lock().card.clone();

        if single && card.is_none() {
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
                self.adopt_saved(tree, std::slice::from_ref(&saved))?;
                status = "done".into();
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
        let outputs: Vec<Value> = if single {
            vec![state(SINGLE).map(|s| s.output).unwrap_or(json!({}))]
        } else {
            plan.steps
                .iter()
                .filter_map(|s| state(&s.id).map(|st| st.output))
                .collect()
        };
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
        if let Some(card) = card {
            // several asks: the rows are the answer (`_final_card`)
            return Ok(self.card_outcome(tree, &card, artifacts, evidence));
        }
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
            // ended before a restart: their saved answers
            let saved: Vec<Value> = answering
                .iter()
                .filter_map(|k| state(k).map(|s| s.output))
                .collect();
            self.adopt_saved(tree, &saved)?;
            let answer = tree.lock().owner_answer.clone();
            return Ok(outcome(
                &tree.worker.id,
                "done",
                &answer,
                artifacts,
                evidence,
            ));
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
        let mut ids = vec![tree.worker.id.clone()];
        ids.extend(self.helper_ids(&tree.worker.id));
        let mut observed = false;
        for tid in ids {
            let released = self.release_task(tree, &tid, parent_span).await;
            observed = observed || released;
        }
        observed
    }

    /// supervisor.py:6013 `_release_task`: one task id's resources released, recorded in `tree`'s resources and
    /// cleanup errors: whether any were observed.
    pub(crate) async fn release_task(
        &self,
        tree: &Arc<Tree>,
        tid: &str,
        parent_span: &str,
    ) -> bool {
        let span = self.span_start(
            "task.cleanup",
            Some(tid),
            Some(parent_span),
            None,
            json!({"operation": "release task resources"}),
        );
        let resources = self.deps.daemon.on_task_end(tid, &span).await;
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
        !resources.is_empty()
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

    /// supervisor.py:4917 `_finish_tree`: the one finish of a task tree, however it ended: its outcome recorded once,
    /// or, suspended by a shutdown, nothing but its plan marked to resume. Then its live state goes, and its episode is
    /// recorded.
    async fn finish_tree(
        &self,
        tree: &Arc<Tree>,
        root_span: &str,
        request_id: &str,
        suspended: bool,
        stored: Option<Value>,
    ) -> Flow<Map<String, Value>> {
        let tree_id = tree.id.clone();
        let spec = tree.spec();
        self.record_planner_reads(tree, root_span, request_id);
        let mut final_ = obj(self.redact(&Value::Object(self.final_(tree)?)));
        let status = final_["status"].as_str().unwrap_or("").to_string();
        self.plans(|p| p.set_status(&tree_id, if suspended { "suspended" } else { &status }))?;
        if !suspended {
            self.deps.daemon.standing_revoke(&tree_id);
        }
        final_.insert("goal".into(), json!(spec.goal));
        final_.insert(
            "name".into(),
            json!(ferb_orchestrator::names::shown_name(
                Some(&spec.name),
                &spec.goal
            )),
        );
        if suspended {
            self.span_finish(
                root_span,
                Some(json!({"status": "suspended", "summary": SHUT_DOWN})),
                Some(SHUT_DOWN),
                None,
                None,
            );
            // never left 'running': start-up would offer to pick it up again, though it resumes by itself
            self.set_status(
                &tree_id,
                "interrupted",
                Some(&json!({"status": "interrupted", "summary": SHUT_DOWN})),
            );
        } else {
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
        }
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
        {
            let mut live = self.live();
            live.pauses.remove(&tree_id);
            live.waiting.remove(&tree_id);
            live.questions.remove(&tree_id);
        }
        if suspended {
            return Ok(final_);
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
        self.capture_after_outcome(&tree_id);
        Ok(final_)
    }

    /// supervisor.py:4980 `_capture_after_outcome`: the task's episode card runs after the owner has the outcome (L28),
    /// as a tracked job, one capture at a time; a failure is journaled, never raised. At shutdown it is cancelled
    /// (`stop_episodes`) or refused (`Deferred`) and journaled as deferred: the hourly pass captures the card again.
    pub(crate) fn capture_after_outcome(&self, tree_id: &str) {
        let me = self.me();
        let id = tree_id.to_string();
        let job = tokio::spawn(async move {
            tokio::task::yield_now().await; // the outcome goes out first
            let lock = me.live().capture_lock.clone();
            let _one = lock.lock().await;
            match me.deps.daemon.capture_episode(&id).await {
                Ok(()) => {}
                Err(super::EpisodeFault::Deferred) => {
                    me.journal("memory.episode_deferred", json!({"task_id": id}), None);
                }
                Err(super::EpisodeFault::Failed(error)) => {
                    me.journal(
                        "memory.episode_hook_failed",
                        json!({"task_id": id, "error": error}),
                        None,
                    );
                }
            }
        });
        let mut live = self.live();
        live.episode_jobs.retain(|(_, j)| !j.is_finished());
        live.episode_jobs.push((tree_id.to_string(), job));
    }

    /// supervisor.py:5010 `stop_episodes`: the shutdown cancels every pending capture rather than wait for it; each
    /// is journaled as deferred.
    pub(crate) async fn stop_episodes(&self) {
        let jobs = std::mem::take(&mut self.live().episode_jobs);
        for (tree_id, job) in jobs {
            if job.is_finished() {
                continue;
            }
            job.abort();
            let cancelled = job.await.is_err_and(|e| e.is_cancelled());
            if cancelled {
                self.journal("memory.episode_deferred", json!({"task_id": tree_id}), None);
            }
        }
    }

    /// supervisor.py:5003 `finish_episodes`: wait for every pending capture (tests); none is cancelled.
    pub async fn finish_episodes(&self) {
        loop {
            let jobs = std::mem::take(&mut self.live().episode_jobs);
            if jobs.is_empty() {
                return;
            }
            for (_, job) in jobs {
                let _ = job.await;
            }
        }
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
        // supervisor.py:6671 `_told_not_done` (ORCH-609)
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
        let card = tree.lock().card.clone();
        if let Some(card) = card {
            return Ok(self.final_card(tree, &card));
        }
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
        // GATE-581, fixed against Python (control.rs): a stop's words other than the owner's ("FERB was stopped", a
        // shutdown, an interruption) are FERB's own and never marked "(from the task)"
        let code_stop = !stop.is_empty() && stop != STOPPED_BY_YOU;
        if !code_stop {
            summary = ferb_policy::confirm::from_task_lines(&summary);
        }
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
        final_.extend(self.told_in(tree)); // ORCH-579 (control.rs)
        final_.extend(self.checklist(tree, Some(&status)));
        Ok(final_)
    }

    /// supervisor.py:4704 `_result_cards`.
    pub(crate) fn result_cards(
        &self,
        tree: &Arc<Tree>,
        artifacts: &[String],
        sites: &[String],
    ) -> Vec<Value> {
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
    pub(crate) fn code_lines(&self, tree: &Arc<Tree>, sites: &[String]) -> Vec<String> {
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
