//! The session driver's task (owner 2026-11-23, HAND-707; how it is built, HAND-709 and ORCH-705).
//!
//! Hand to FERB on one of the owner's sessions is not a chat turn: no planner, no checker. The daemon's sessions area
//! owns the loop (it waits in code for the agent's turn to end, with no model call and no time limit) and asks this
//! module for exactly one step per agent reply. A step is one worker run ([`Worker::run_worker`]) with the fresh switch
//! (ORCH-699's [`Inner::fresh_for`], read from the request id `driver-…`): no Tier 1 profile, memory, chat history,
//! schedules or screen are pushed to it, but its token keeps every usual tool and the lookups that read what FERB knows
//! (memory, the chat history, the trail), so a goal that names an earlier conversation or decision can be looked up.
//! Only typing into the session and answering its questions are left out of its token: code does those from the step's
//! decision, through the gateway like any call (GATE-585's question, one Yes per app for the drive's task).
//!
//! Recorded like any task: a root task row and its `task` span, the worker's row, one [`STEP_SPAN`] span per step with
//! its `worker` span (the run's tool calls) under it, and the outcome on both rows; never a chat entry, a task result
//! (the Tasks page) or an episode.

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

use ferb_context::assemble::ContextBlock;
use ferb_core::types::{Identity, Task, TaskRole};
use ferb_orchestrator::models::{TargetsSpec, TaskSpec};
use ferb_worker::{Attempt, RunOptions};
use serde_json::{json, Value};

use super::{Inner, Request, Supervisor, Tree, TreeState};

/// The step's own instructions (engine/runtime/orchestrator/prompts/session_driver.md, PRM-372).
pub const PROMPT: &str = "session_driver.md";
/// One step's span: its round, its decision, and the worker run under it.
pub const STEP_SPAN: &str = "session_driver.step";
/// The task's short name (its root span and row carry the owner's goal).
pub const NAME: &str = "Keep a session going";

/// What one step is given: the owner's goal, the session as code knows it, its recent exchanges and what the driver
/// decided before. Nothing else of FERB's context.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DriverStep {
    /// 1, 2, …: the step's round (one round is an agent reply and the driver's next message).
    pub round: u32,
    /// The owner's goal, his words.
    pub goal: String,
    /// The session as code knows it (its agent, folder, app, title, state, id).
    pub session: String,
    /// The session's recent exchanges, oldest first, the newest reply in full (the agent's words: data).
    pub exchanges: String,
    /// The agent's open question with its choices ("" when it asks nothing).
    pub asking: String,
    /// What the driver decided in earlier rounds, oldest first (short lines code wrote).
    pub decided: Vec<String>,
}

/// One step's decision: exactly one of three.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Decision {
    /// The next message to the session (or the label of the question's choice, when it asks one).
    Send(String),
    /// The goal is reached: a one-line summary.
    Done(String),
    /// Only the owner can settle what's next: his question.
    Ask(String),
}

impl Decision {
    /// "send" | "done" | "ask".
    pub fn word(&self) -> &'static str {
        match self {
            Decision::Send(_) => "send",
            Decision::Done(_) => "done",
            Decision::Ask(_) => "ask",
        }
    }

    pub fn text(&self) -> &str {
        match self {
            Decision::Send(t) | Decision::Done(t) | Decision::Ask(t) => t,
        }
    }
}

/// The step's answer form.
pub fn decision_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "decision": {"type": "string", "enum": ["send", "done", "ask"]},
            "text": {"type": "string"}
        },
        "required": ["decision", "text"]
    })
}

/// The step's report as a decision; Err: why it isn't one.
pub fn decision_of(report: &Value) -> Result<Decision, String> {
    let word = report.get("decision").and_then(Value::as_str).unwrap_or("");
    let text = report
        .get("text")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    if text.is_empty() {
        return Err("the step's decision had no text".into());
    }
    match word {
        "send" => Ok(Decision::Send(text)),
        "done" => Ok(Decision::Done(text)),
        "ask" => Ok(Decision::Ask(text)),
        other => Err(format!(
            "the step's decision {other:?} is not send, done or ask"
        )),
    }
}

fn block(text: String, reference: &str, origin: &str) -> ContextBlock {
    let mut b = ContextBlock::new(text, reference);
    b.protected = true;
    b.origin = origin.to_string();
    b
}

impl Supervisor {
    /// A session driver's task starts (HAND-709): its rows, its root span and its live tree (so the owner's Take it
    /// back, `task.stop`, stops it like any task). `app` is the session's app, the one target the owner's hand-over
    /// names. Answers its id.
    pub async fn drive_begin(&self, session: &str, goal: &str, app: &str) -> String {
        self.inner.drive_begin(session, goal, app).await
    }

    /// One step: one fresh worker run, its decision (Err: the run failed or was stopped, in words).
    pub async fn drive_step(&self, tree_id: &str, step: &DriverStep) -> Result<Decision, String> {
        self.inner.drive_step(tree_id, step).await
    }

    /// The owner stopped it (Take it back, the global stop) or it already ended.
    pub fn drive_stopped(&self, tree_id: &str) -> bool {
        self.inner.drive_stopped(tree_id)
    }

    /// The drive ended (`status`: done, needs_user, cancelled, failed): its resources released, its rows and root span
    /// closed with `summary`, its live tree gone.
    pub async fn drive_end(&self, tree_id: &str, status: &str, summary: &str) {
        self.inner.drive_end(tree_id, status, summary).await
    }
}

impl Inner {
    /// The task (any of a tree's) is a session driver's: by its live tree's request, else the stored task's.
    pub fn driver_task(&self, task_id: &str) -> bool {
        let live = self
            .live()
            .tree_of
            .get(task_id)
            .map(|t| t.worker.request_id.clone());
        let request = live.or_else(|| {
            self.task_column(task_id, "request_id")
                .filter(|v| !v.is_null())
                .map(|v| ferb_py::value::py_str(&v))
        });
        request.is_some_and(|r| ferb_core::driver::is_request(&r))
    }

    pub async fn drive_begin(&self, session: &str, goal: &str, app: &str) -> String {
        let tree_id = self.new_id();
        let request_id = ferb_core::driver::request_of(&tree_id);
        let goal = goal.trim().to_string();
        let apps: Vec<String> = if app.is_empty() {
            Vec::new()
        } else {
            vec![app.to_string()]
        };
        let spec = TaskSpec {
            goal: goal.clone(),
            name: NAME.into(),
            identity: "owner".into(),
            worker_type: "default".into(),
            targets: TargetsSpec {
                apps: apps.clone(),
                ..TargetsSpec::default()
            },
            success_conditions: Vec::new(),
            plan: None,
            folders: Vec::new(),
            keep_tabs_open: false,
            parts: Vec::new(),
            earlier_results: Vec::new(),
            profiles: Vec::new(),
        };
        let root = Task {
            id: tree_id.clone(),
            parent_id: None,
            goal: goal.clone(),
            device_id: self.deps.device_id.clone(),
            identity: Identity::Owner,
            scopes: BTreeSet::new(),
            role: TaskRole::Mixed,
            request_id: request_id.clone(),
            success_conditions: Vec::new(),
            deadline: None,
        };
        let worker = Task {
            id: super::tree::step_task(&tree_id, super::tree::WORKER),
            parent_id: Some(tree_id.clone()),
            goal: goal.clone(),
            device_id: self.deps.device_id.clone(),
            identity: Identity::Owner,
            scopes: self.deps.daemon.all_scopes(),
            role: TaskRole::Worker,
            request_id: request_id.clone(),
            success_conditions: Vec::new(),
            deadline: None,
        };
        // the owner's hand-over is his words: the gateway reads them (and the session's app as his named target) as
        // it reads any request's; no planner adds targets, so anything else asks as it would
        self.deps.daemon.record_request(&Request {
            request_id: request_id.clone(),
            words: goal.clone(),
            targets: [apps, Vec::new(), Vec::new(), Vec::new()],
            payload: goal.clone(),
            asked_origins: BTreeSet::new(),
        });
        let root_span = self.span_start(
            "task",
            Some(&tree_id),
            None,
            Some(&self.deps.device_id),
            json!({"goal": goal, "identity": "owner", "request_id": request_id, "request": goal,
                   "authority_words": goal, "session_driver": {"session": session}}),
        );
        self.span_set_origin(&root_span, ferb_sanitize::origin::OWNER);
        self.task_add(&root, &root_span, &tree_id, "running");
        self.task_add(&worker, &root_span, &tree_id, "planned");
        let tree = Arc::new(Tree {
            id: tree_id.clone(),
            worker: worker.clone(),
            step_id: super::tree::WORKER.into(),
            asks_in_place: true,
            st: Mutex::new(TreeState {
                spec: Some(spec),
                accepting_updates: true,
                ..TreeState::default()
            }),
            steering: Arc::default(),
            result_revision: Mutex::new(0),
        });
        {
            let mut live = self.live();
            live.tree_of.insert(tree_id.clone(), tree.clone());
            live.tree_of.insert(worker.id.clone(), tree.clone());
        }
        self.runs.set_tree(&tree_id, tree.clone());
        self.runs.set_tree(&worker.id, tree.clone());
        self.journal(
            "session_driver.started",
            json!({"task_id": tree_id, "session": session}),
            None,
        );
        self.deps.daemon.on_task_start(&tree_id).await;
        tree_id
    }

    /// The step's context: the goal (the owner's words), the session (code's facts), its exchanges and open question
    /// (the agent's words: data), what was decided before (code's lines). Nothing pushed from FERB's memory.
    pub fn driver_context(&self, step: &DriverStep) -> Vec<ContextBlock> {
        use ferb_sanitize::origin;
        let mut ctx = vec![
            block(
                format!(
                    "The owner's goal for this session, in their words:\n{}",
                    step.goal
                ),
                "the owner's goal",
                origin::OWNER,
            ),
            block(
                format!("The session (round {}):\n{}", step.round, step.session),
                "the session",
                origin::FERB,
            ),
        ];
        let exchanges = if step.exchanges.trim().is_empty() {
            "(nothing in it yet)".to_string()
        } else {
            self.wrap(&step.exchanges)
        };
        ctx.push(block(
            format!("The session's recent exchanges, oldest first:\n{exchanges}"),
            "the session's exchanges",
            &origin::external("agent_session", ""),
        ));
        if !step.asking.trim().is_empty() {
            ctx.push(block(
                format!(
                    "The agent is asking this now; to answer, send exactly one choice's label:\n{}",
                    self.wrap(&step.asking)
                ),
                "the agent's question",
                &origin::external("agent_session", ""),
            ));
        }
        if !step.decided.is_empty() {
            ctx.push(block(
                format!(
                    "What you decided in earlier rounds, oldest first:\n{}",
                    step.decided.join("\n")
                ),
                "earlier decisions",
                origin::FERB,
            ));
        }
        ctx
    }

    pub async fn drive_step(&self, tree_id: &str, step: &DriverStep) -> Result<Decision, String> {
        let Some(tree) = self.live().tree_of.get(tree_id).cloned() else {
            return Err("FERB's drive of this session has ended.".into());
        };
        if !tree.lock().stop.is_empty() {
            return Err(tree.lock().stop.clone());
        }
        let root_span = self.task_span(tree_id).unwrap_or_default();
        let span = self.span_start(
            STEP_SPAN,
            Some(tree_id),
            Some(&root_span),
            None,
            json!({"goal": format!("round {}", step.round), "round": step.round}),
        );
        let task = tree.worker_task();
        self.set_status(&task.id, "running", None);
        let worker = self.worker.get().expect("the worker").clone();
        let wtree: Arc<dyn ferb_worker::ports::WorkerTree> = tree.clone();
        let ran = worker
            .run_worker(
                &wtree,
                &task,
                PROMPT,
                self.driver_context(step),
                &decision_schema(),
                &span,
                &RunOptions::default(),
            )
            .await;
        let decided = match ran {
            Ok((Attempt::Report(report), _)) => decision_of(&report.to_value()),
            Ok((Attempt::Outcome(o), _)) => Err(o.outcome.summary),
            Err(e) => Err(worker.reason(&e)),
        };
        match &decided {
            Ok(d) => self.span_finish(
                &span,
                Some(json!({"decision": d.word(),
                            "result": format!("{}: {}", d.word(), self.redact_text(d.text()))})),
                None,
                None,
                None,
            ),
            Err(e) => self.span_finish(&span, None, Some(&self.redact_text(e)), None, None),
        }
        decided
    }

    pub fn drive_stopped(&self, tree_id: &str) -> bool {
        match self.live().tree_of.get(tree_id).cloned() {
            Some(tree) => !tree.lock().stop.is_empty(),
            None => true,
        }
    }

    pub async fn drive_end(&self, tree_id: &str, status: &str, summary: &str) {
        let Some(tree) = self.live().tree_of.get(tree_id).cloned() else {
            return;
        };
        {
            let mut st = tree.lock();
            st.work_done = true;
            st.accepting_updates = false;
        }
        let root_span = self.task_span(tree_id).unwrap_or_default();
        self.release_resources(&tree, &root_span).await;
        self.deps.daemon.on_task_finish(tree_id).await;
        let summary = self.redact_text(summary);
        let outcome = json!({"status": status, "summary": summary});
        self.set_status(&tree.worker.id, status, Some(&outcome));
        self.set_status(tree_id, status, Some(&outcome));
        self.span_finish(
            &root_span,
            Some(json!({"status": status, "summary": summary,
                        "result": ferb_py::json::dumps(&outcome)})),
            None,
            None,
            None,
        );
        self.journal(
            "session_driver.ended",
            json!({"task_id": tree_id, "status": status}),
            None,
        );
        let ids: BTreeSet<String> = [tree_id.to_string(), tree.worker.id.clone()].into();
        self.deps.daemon.forget_tasks(&ids);
        self.deps.daemon.forget_request(&tree.worker.request_id);
        for tid in &ids {
            self.live().tree_of.remove(tid);
            self.runs.forget_tree(tid);
            self.task_remove(tid);
        }
    }
}
