//! The owner's message routed by the planner to one open question (supervisor.py:3155 `_answer`, L3, ORCH-536,
//! ORCH-537): the answer recorded is the owner's own words, never the planner's reading. FERB's own checks take only
//! one of their options; FERB's own question about whether a task goes on takes only one of its options; a task's
//! question with options takes the owner's exact words as an option and other words as their answer. When the words
//! could answer more than one open question, FERB asks which, once (`Focus`, kept for the chat's next message).
//! Also `asked_only_about` and `displaced_question` (:3242, :3253), which the daemon reads to hide a secret.

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

use ferb_gateway::questions::Approval;
use ferb_orchestrator::models::AnswerQuestion;
use serde_json::json;

use super::{Inner, Turn};
use crate::lines::{bare, plain_choice, takes_words, QuestionFacts};
use crate::words::{
    ANSWERED, FERB_CHECKS, NOT_WAITING, NO_QUESTION, PASTED_ANSWER, WHICH_ONE, WHICH_QUESTION,
};

/// supervisor.py:216 `ROUTABLE_QUESTIONS`.
pub fn routable(capability: &str) -> bool {
    capability == "owner.ask" || capability == "plan.ask" || FERB_CHECKS.contains(&capability)
}

/// supervisor.py:386 `_Focus`: what FERB itself just asked a chat about an answer, for that chat's next message only.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Focus {
    /// The owner's earlier words it asked "which one is it for?" about (None for "Yes or No?").
    pub words: Option<String>,
    pub named: BTreeSet<String>,
    pub seen: BTreeSet<String>,
    /// The planner's own question that FERB's asking displaced as the chat's pending question.
    pub planner_question: String,
}

fn facts(ap: &Approval) -> QuestionFacts {
    QuestionFacts {
        capability: ap.capability.clone(),
        options_only: ap.options_only,
        summary: ap.summary.clone(),
        reason: ap.reason.clone(),
        options: ap.options.clone(),
        labels: ap.labels.clone(),
    }
}

/// supervisor.py:3260 `_listed`: open questions as the owner saw them, one per line.
fn listed<'a>(questions: impl IntoIterator<Item = &'a Approval>) -> String {
    questions
        .into_iter()
        .map(|q| {
            format!(
                "- {}",
                ferb_sanitize::terminal::printable(&q.reason)
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" ")
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

impl Inner {
    /// supervisor.py:3155 `_answer`: ("reply" or "clarify", FERB's words); the turn records them (`exit`).
    pub(crate) fn answer(
        &self,
        turn: &Arc<Mutex<Turn>>,
        routed: &AnswerQuestion,
    ) -> (String, String) {
        let (focus, clarified, session, own_words, untrusted) = {
            let t = turn.lock().unwrap();
            (
                t.focus.clone(),
                t.clarified.clone(),
                t.session.clone(),
                t.own_words.clone(),
                t.untrusted,
            )
        };
        let open_now: Vec<Approval> = self
            .deps
            .daemon
            .approvals()
            .into_iter()
            .filter(|q| q.kind == "owner_question")
            .collect();
        let seen: BTreeSet<String> = open_now.iter().map(|q| q.id.clone()).collect();
        let displaced = match &focus {
            None => clarified.clone(),
            Some(f) => f.planner_question.clone(),
        };
        let set_focus = |words: Option<String>, named: BTreeSet<String>| {
            self.live().answer_focus.insert(
                session.clone(),
                Focus {
                    words,
                    named,
                    seen: seen.clone(),
                    planner_question: displaced.clone(),
                },
            );
        };
        let ap = self
            .deps
            .daemon
            .approval(&routed.approval_id)
            .filter(|ap| ap.kind == "owner_question" && routable(&ap.capability));
        let Some(ap) = ap else {
            let routable_now: Vec<&Approval> = open_now
                .iter()
                .filter(|q| routable(&q.capability))
                .collect();
            if routable_now.is_empty() {
                return ("reply".into(), NO_QUESTION.into());
            }
            set_focus(None, BTreeSet::new());
            return (
                "clarify".into(),
                format!("{WHICH_QUESTION}\n{}", listed(routable_now)),
            );
        };
        if ap.state.as_str() != "pending" {
            return ("reply".into(), NOT_WAITING.into());
        }
        let words = ferb_orchestrator::py::strip(&own_words).to_string();
        if untrusted || words.is_empty() {
            set_focus(None, BTreeSet::new());
            return ("clarify".into(), PASTED_ANSWER.into());
        }
        // the owner's reply to FERB's "which one is it for?": the earlier words answer the question they now name
        let carried = focus.as_ref().is_some_and(|f| {
            f.words.is_some() && f.named.contains(&ap.id) && ap.option_for(&bare(&words)).is_none()
        });
        let mut text = if carried {
            focus
                .as_ref()
                .and_then(|f| f.words.clone())
                .unwrap_or_default()
        } else {
            words.clone()
        };
        let mut choice: Option<usize> = None;
        let checks = FERB_CHECKS;
        if checks.contains(&ap.capability.as_str()) {
            choice = ap.option_for(&bare(&text));
            if choice.is_none() || ap.option_for(&bare(&routed.answer)) != choice {
                set_focus(None, BTreeSet::from([ap.id.clone()]));
                return ("clarify".into(), plain_choice(&facts(&ap)));
            }
            if !carried {
                // FERB just asked "Yes or No?" about exactly this check: questions open then don't compete
                let asked_this = focus
                    .as_ref()
                    .is_some_and(|f| f.named == BTreeSet::from([ap.id.clone()]));
                let others: Vec<&Approval> = open_now
                    .iter()
                    .filter(|q| {
                        q.id != ap.id
                            && (q.options.is_empty() || q.option_for(&bare(&text)).is_some())
                            && !(asked_this
                                && focus.as_ref().is_some_and(|f| f.seen.contains(&q.id)))
                    })
                    .collect();
                let planner_asked = if focus.is_none() {
                    clarified.clone()
                } else {
                    String::new()
                };
                if !others.is_empty() || !ferb_orchestrator::py::strip(&planner_asked).is_empty() {
                    let mut named = BTreeSet::from([ap.id.clone()]);
                    named.extend(others.iter().map(|q| q.id.clone()));
                    set_focus(Some(text.clone()), named);
                    let mut all = vec![&ap];
                    all.extend(others);
                    let mut lines = vec![listed(all)];
                    if !ferb_orchestrator::py::strip(&planner_asked).is_empty() {
                        lines.push(format!(
                            "- {}",
                            ferb_policy::confirm::from_task(
                                &ferb_sanitize::terminal::printable(&planner_asked)
                                    .split_whitespace()
                                    .collect::<Vec<_>>()
                                    .join(" ")
                            )
                        ));
                    }
                    return (
                        "clarify".into(),
                        format!(
                            "{}\n{}",
                            WHICH_ONE
                                .replace("{words}", &ferb_sanitize::terminal::printable(&text)),
                            lines.join("\n")
                        ),
                    );
                }
            }
            text = ap.options[choice.unwrap() - 1].clone();
        } else if !ap.options.is_empty() && ap.options_only {
            // FERB's own question about whether the task goes on (ORCH-536): exactly an option, or the planner's reading
            choice = ap.option_for(&bare(&text));
            if choice.is_none() {
                choice = ap.option_for(&bare(&routed.answer));
            }
            if choice.is_none() {
                set_focus(None, BTreeSet::from([ap.id.clone()]));
                return ("clarify".into(), plain_choice(&facts(&ap)));
            }
        } else if !ap.options.is_empty() {
            // a task's own question with options (ORCH-537)
            choice = ap.option_for(&bare(&text));
            if choice.is_none() && !carried {
                let asked_this = focus
                    .as_ref()
                    .is_some_and(|f| f.named == BTreeSet::from([ap.id.clone()]));
                let others: Vec<&Approval> = open_now
                    .iter()
                    .filter(|q| {
                        q.id != ap.id
                            && routable(&q.capability)
                            && (takes_words(&facts(q), &checks)
                                || q.option_for(&bare(&text)).is_some())
                            && !(asked_this
                                && focus.as_ref().is_some_and(|f| f.seen.contains(&q.id)))
                    })
                    .collect();
                if !others.is_empty() {
                    let mut named = BTreeSet::from([ap.id.clone()]);
                    named.extend(others.iter().map(|q| q.id.clone()));
                    set_focus(Some(text.clone()), named);
                    let mut all = vec![&ap];
                    all.extend(others);
                    return (
                        "clarify".into(),
                        format!(
                            "{}\n{}",
                            WHICH_ONE
                                .replace("{words}", &ferb_sanitize::terminal::printable(&text)),
                            listed(all)
                        ),
                    );
                }
            }
        }
        let resolved = self
            .deps
            .daemon
            .resolve_question(&ap.id, true, choice, Some(&text));
        let state = resolved
            .as_ref()
            .map(|a| a.state.as_str().to_string())
            .unwrap_or_else(|| "granted".into());
        self.journal(
            "approval.resolved",
            json!({"approval_id": ap.id, "state": state, "capability": ap.capability}),
            None,
        );
        self.deps.daemon.publish(
            "approval.resolved",
            json!({"approval_id": ap.id, "state": state}),
        );
        // its sites the owner typed count for a login name given in the answer (L36 §5)
        let answered_tree = if ap.task_id.is_empty() {
            None
        } else {
            Some(self.tree_of_task(&ap.task_id))
        };
        turn.lock().unwrap().answered_tree = answered_tree;
        let followed = resolved
            .map(|a| self.deps.daemon.question_answered(&a))
            .unwrap_or_default();
        let said = if followed.is_empty() {
            ANSWERED.to_string()
        } else {
            format!("{ANSWERED}\n{followed}")
        };
        ("reply".into(), said)
    }

    /// supervisor.py:3242 `asked_only_about`: whether this chat's pending question is FERB's own "which is it?" about
    /// an answer naming only questions of these capabilities.
    pub fn asked_only_about(&self, session_id: &str, capabilities: &[&str]) -> bool {
        let focus = self.live().answer_focus.get(session_id).cloned();
        let Some(focus) = focus.filter(|f| !f.named.is_empty()) else {
            return false;
        };
        focus.named.iter().all(|id| {
            self.deps
                .daemon
                .approval(id)
                .is_some_and(|ap| capabilities.contains(&ap.capability.as_str()))
        })
    }

    /// supervisor.py:3253 `displaced_question`: the planner's own question this chat's next message may still answer.
    pub fn displaced_question(&self, session_id: &str) -> String {
        self.live()
            .answer_focus
            .get(session_id)
            .map(|f| f.planner_question.clone())
            .unwrap_or_default()
    }

    /// supervisor.py:3738 `_question_root`: the task tree an open question belongs to ("" for one outside any task).
    pub fn question_root(&self, task_id: &str) -> String {
        if task_id.is_empty() {
            return String::new();
        }
        self.task_column(task_id, "tree_id")
            .and_then(|v| v.as_str().map(str::to_string))
            .unwrap_or_else(|| task_id.split('.').next().unwrap_or(task_id).to_string())
    }

    /// supervisor.py:3731 `_question_task` (ORCH-539): the short label of the running task an open question belongs
    /// to; "" for one outside any running task.
    pub fn question_task(&self, task_id: &str) -> String {
        if task_id.is_empty() {
            return String::new();
        }
        let root = self.question_root(task_id);
        let tree = self.live().tree_of.get(&root).cloned();
        tree.and_then(|t| t.lock().spec.clone())
            .map(|spec| ferb_orchestrator::names::short_label(&spec.goal, 60))
            .unwrap_or_default()
    }

    /// `tasks.tree_of(task_id)`: the tree a task belongs to (its root's id).
    pub(crate) fn tree_of_task(&self, task_id: &str) -> String {
        let live = self.live();
        if let Some(t) = live.tree_of.get(task_id) {
            return t.id.clone();
        }
        drop(live);
        self.task_column(task_id, "tree_id")
            .and_then(|v| v.as_str().map(str::to_string))
            .unwrap_or_else(|| task_id.split('.').next().unwrap_or(task_id).to_string())
    }
}

impl Inner {
    /// The open questions as the planner's questions block reads them (`state.approvals.pending()` with
    /// `_question_task` and `_card_row`): what a daemon's `Daemon::pending_questions` may answer with.
    pub fn open_questions(&self) -> Vec<crate::planner::OpenQuestion> {
        let zone = self.deps.clock.zone();
        self.deps
            .daemon
            .approvals()
            .into_iter()
            .map(|ap| {
                let asked = self
                    .of_kind("question.asked", "approval_id", &ap.id, None)
                    .last()
                    .and_then(|e| ferb_clock::py::parse(&e.at));
                let mut row = crate::planner::card_row(
                    &ap.kind,
                    &ap.capability,
                    &ap.summary,
                    &ap.reason,
                    asked.as_ref(),
                    crate::planner::routable(&ap.kind, &ap.capability, &FERB_CHECKS),
                    &zone,
                );
                row.remove("answered_in");
                crate::planner::OpenQuestion {
                    task_label: self.question_task(&ap.task_id),
                    id: ap.id,
                    kind: ap.kind,
                    capability: ap.capability,
                    reason: ap.reason,
                    options: ap.options,
                    card_row: row,
                }
            })
            .collect()
    }
}

impl Inner {
    /// supervisor.py:7054 `route_question`: a worker's question (`owner.ask`), read by the orchestrator with the task's
    /// context (`question.md`, QUESTION_SCHEMA): `{"ask_owner", "answer", "question"}`. A task not running here goes
    /// to the owner as asked.
    pub async fn route_worker_question(
        &self,
        task_id: &str,
        question: &str,
        options: &[String],
    ) -> Result<serde_json::Map<String, serde_json::Value>, String> {
        let tree = self.live().tree_of.get(task_id).cloned();
        let Some(tree) = tree else {
            let mut out = serde_json::Map::new();
            out.insert("ask_owner".into(), json!(true));
            out.insert("question".into(), json!(question));
            out.insert("answer".into(), json!(""));
            return Ok(out);
        };
        let report = format!(
            "\nWorker question (data): {}\nOptions (data): {}",
            self.wrap(question),
            self.wrap(&ferb_py::json::dumps(&json!(options)))
        );
        let mut ctx = self
            .context(&tree, false)
            .await
            .map_err(|e| format!("{}: {}", e.class(), e.message()))?;
        ctx.push(ferb_context::assemble::ContextBlock {
            protected: true,
            ..ferb_context::assemble::ContextBlock::new(report, "current worker question")
        });
        let schema = ferb_context::schemas::question_schema();
        let rt = self
            .deps
            .daemon
            .make_runtime("orchestrator", "supervisor", &[], &schema, None)
            .map_err(|e| format!("{}: {e}", e.class()))?;
        let decision = ferb_core::types::Task {
            id: format!("{task_id}.question"),
            parent_id: Some(task_id.to_string()),
            goal: "resolve worker question".into(),
            device_id: self.deps.device_id.clone(),
            identity: tree.worker_task().identity,
            scopes: BTreeSet::new(),
            role: ferb_core::types::TaskRole::Mixed,
            request_id: tree.worker.request_id.clone(),
            success_conditions: Vec::new(),
            deadline: None,
        };
        self.live()
            .tree_of
            .insert(decision.id.clone(), tree.clone());
        let span = self.span_start(
            "model.question",
            Some(task_id),
            None,
            None,
            json!({"question": question}),
        );
        let system = self.system_for("question.md", true, &[]);
        let ctx = crate::planner::with_newly_always_on(ctx, &self.deps.daemon.identity_pending());
        let wtree: Arc<dyn ferb_worker::ports::WorkerTree> = tree.clone();
        let worker = self.worker.get().expect("the worker").clone();
        let call = ferb_worker::worker::Call {
            task: &decision,
            system,
            context: ctx,
            token: "",
            parent_span: &span,
            tree: Some(&wtree),
            worker_id: "",
            images: Vec::new(),
            hooks: Default::default(),
            retry_silent: true,
        };
        let final_ = match worker.run_call(&rt, call).await {
            Ok(f) => f,
            Err(e) => {
                let error = format!("{}: {e}", e.class());
                self.span_finish(
                    &span,
                    Some(json!({"decision": ""})),
                    Some(&error),
                    None,
                    None,
                );
                return Err(error);
            }
        };
        match question_outcome(&final_) {
            Ok(out) => {
                self.span_finish(&span, Some(json!({"decision": final_})), None, None, None);
                Ok(out)
            }
            Err(error) => {
                self.span_finish(
                    &span,
                    Some(json!({"decision": final_})),
                    Some(&error),
                    None,
                    None,
                );
                Err(error)
            }
        }
    }
}

/// `QuestionOutcome.model_validate_json(final).model_dump()`: `{ask_owner, answer, question}`.
fn question_outcome(final_: &str) -> Result<serde_json::Map<String, serde_json::Value>, String> {
    let v: serde_json::Value = serde_json::from_str(final_).map_err(|e| {
        format!("ValidationError: 1 validation error for QuestionOutcome\n  Invalid JSON: {e}")
    })?;
    let ask = v.get("ask_owner").and_then(serde_json::Value::as_bool);
    let answer = v.get("answer").and_then(serde_json::Value::as_str);
    let question = v.get("question").and_then(serde_json::Value::as_str);
    match (ask, answer, question) {
        (Some(ask), Some(answer), Some(question)) => {
            let mut out = serde_json::Map::new();
            out.insert("ask_owner".into(), json!(ask));
            out.insert("answer".into(), json!(answer));
            out.insert("question".into(), json!(question));
            Ok(out)
        }
        _ => Err("ValidationError: 1 validation error for QuestionOutcome".into()),
    }
}

/// The gateway's question router (`QuestionRouter`, gateway.py's `route_question`) over a supervisor.
impl ferb_gateway::seams::QuestionRouter for super::Supervisor {
    fn route<'a>(
        &'a self,
        task_id: &'a str,
        question: &'a str,
        options: &'a [String],
    ) -> ferb_orchestrator::ports::BoxFuture<
        'a,
        Result<serde_json::Map<String, serde_json::Value>, String>,
    > {
        Box::pin(async move {
            self.inner
                .route_worker_question(task_id, question, options)
                .await
        })
    }
}

impl Inner {
    /// supervisor.py:6727 `runtime_offer` (ORCH-315): the offer of another runtime this running task waits on.
    pub fn runtime_offer(&self, task_id: &str) -> Option<serde_json::Value> {
        let root = self.question_root(task_id);
        let trees: Vec<Arc<super::Tree>> = self.live().tree_of.values().cloned().collect();
        trees
            .iter()
            .filter(|t| t.id == root)
            .find_map(|t| t.lock().runtime_switch.clone())
    }

    /// supervisor.py:6733 `answer_runtime_switch`: the app's one tap (`runtimes.switch_task`) answers this task's
    /// waiting runtime question with `runtime`; the offer it answered. Err: FerbError's words.
    pub fn answer_runtime_switch(
        &self,
        task_id: &str,
        runtime: &str,
    ) -> Result<serde_json::Value, String> {
        let root = self.question_root(task_id);
        let offer = self.runtime_offer(task_id);
        let ap = self.deps.daemon.approvals().into_iter().find(|a| {
            a.capability == crate::words::RUNTIME_ASK && self.question_root(&a.task_id) == root
        });
        let choices: Vec<String> = offer
            .as_ref()
            .and_then(|o| o.get("choices"))
            .and_then(serde_json::Value::as_array)
            .map(|a| a.iter().map(ferb_py::value::py_str).collect())
            .unwrap_or_default();
        let at = choices.iter().position(|c| c == runtime);
        let (Some(offer), Some(ap), Some(at)) = (offer, ap, at) else {
            return Err(format!(
                "no question is waiting to switch task {task_id} to {runtime}"
            ));
        };
        // options: the choices, then Stop
        self.deps
            .daemon
            .resolve_question(&ap.id, true, Some(at + 1), None);
        Ok(offer)
    }
}
