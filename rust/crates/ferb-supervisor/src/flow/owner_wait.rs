//! Waiting on the owner (supervisor.py:1892-2050, :6688-6812): "needs the owner" means one thing everywhere (ORCH-323)
//! — the step pauses in place and FERB asks. A paused tree's steps hold here (`_hold_pause`); a step's own question
//! (`_wait_for_owner`, `_ask_and_wait`) is asked through the gateway's questions; the task and its step show
//! `paused` / `needs_you` while it waits (`_mark_waiting`) and run again once no step of the tree waits (`_resumed`).
//! The check-in after five failed attempts (`_check_in`, ORCH-307) and the offer of another runtime after a usage
//! limit or a sign-in failure (`_service_down`, ORCH-315) are two such waits.
//!
//! Python polls the question every `gateway.hold_poll_s`; so does this (`Daemon::hold_poll_s`), on tokio's clock.
//! The questions themselves are the gateway's (`state.approvals`, ferb-gateway `questions`), reached through
//! [`Questions`].

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

use ferb_core::types::{Outcome, Task};
use ferb_orchestrator::attempts::{
    check_in_question, stopped_after, Attempts, CHECK_IN_AFTER, KEEP_GOING, STOP_HERE,
};
use ferb_sanitize::origin;
use serde_json::{json, Map, Value};

use super::{Inner, Tree};
use crate::words::*;

/// A question FERB asks the owner (`state.approvals.question(action, question, options)` and the fields the
/// supervisor sets on it before publishing it).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Ask {
    pub capability: String,
    pub task_id: String,
    pub request_id: String,
    pub question: String,
    pub options: Vec<String>,
    /// The card row it sits in (ORCH-319); "" for none.
    pub part: String,
    pub runtime_switch: Option<Value>,
    pub from_task: bool,
    pub options_only: bool,
    pub corrects: bool,
    /// GATE-566: the card's own words when code wrote them.
    pub summary: String,
    pub detail: String,
    pub exact: String,
    pub labels: Vec<String>,
}

/// A question as the waiting step reads it back (`Approval`). `Debug` never shows a typed answer.
#[derive(Clone, PartialEq, Default)]
pub struct Asked {
    pub id: String,
    pub capability: String,
    pub kind: String,
    pub request_id: String,
    pub task_id: String,
    /// "pending", "granted", "denied", "used", "superseded".
    pub state: String,
    pub options: Vec<String>,
    pub choice: Option<String>,
    pub answer: Option<String>,
    /// The kinds of secret FERB hid in a typed answer, by name.
    pub hidden: Vec<String>,
    pub options_only: bool,
    /// The card's box takes the owner's words for the task as a correction (GATE-510).
    pub corrects: bool,
}

impl std::fmt::Debug for Asked {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Asked")
            .field("id", &self.id)
            .field("capability", &self.capability)
            .field("state", &self.state)
            .field("choice", &self.choice)
            .field("answer", &self.answer.as_ref().map(|_| "<hidden>"))
            .finish_non_exhaustive()
    }
}

/// A gateway question as the waiting step reads it back.
pub fn asked_of(ap: &ferb_gateway::questions::Approval) -> Asked {
    Asked {
        id: ap.id.clone(),
        capability: ap.capability.clone(),
        kind: ap.kind.clone(),
        request_id: ap.request_id.clone(),
        task_id: ap.task_id.clone(),
        state: ap.state.as_str().into(),
        options: ap.options.clone(),
        choice: ap.choice.clone(),
        answer: ap.answer.clone(),
        hidden: ap.hidden.clone(),
        options_only: ap.options_only,
        corrects: ap.corrects,
    }
}

/// The gateway's questions for the owner (`state.approvals`), as the supervisor uses them.
pub trait Questions: Send + Sync {
    /// `approvals.question(...)` with the fields of `ask` set: the new question's id.
    fn ask(&self, ask: &Ask) -> String;
    fn get(&self, id: &str) -> Option<Asked>;
    /// `approvals.supersede(id)`: whether it was pending (and is now superseded).
    fn supersede(&self, id: &str) -> bool;
    fn pending(&self) -> Vec<Asked>;
    /// `approvals.resolve(id, True, text=words)`.
    fn answer(&self, id: &str, words: &str);
    /// `card_parts(ap) | card_flags(ap)`: what the card shows besides the question.
    fn card(&self, id: &str) -> Map<String, Value>;
}

/// supervisor.py:1005 `OwnerReply`: how a wait for the owner ended.
#[derive(Clone, PartialEq, Default)]
pub struct OwnerReply {
    /// "answer", "steer", "denied", "stopped", "declined".
    pub kind: String,
    pub words: String,
    pub choice: Option<String>,
    pub question: String,
    pub hidden: Vec<String>,
}

impl std::fmt::Debug for OwnerReply {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OwnerReply")
            .field("kind", &self.kind)
            .field("choice", &self.choice)
            .finish_non_exhaustive()
    }
}

impl OwnerReply {
    pub fn of(kind: &str, question: &str) -> OwnerReply {
        OwnerReply {
            kind: kind.into(),
            question: question.into(),
            ..OwnerReply::default()
        }
    }
}

/// The mutable part of a wait (Python's `_Wait` fields set while it runs).
#[derive(Default)]
pub struct WaitState {
    pub approval: Option<String>,
    pub extra: Map<String, Value>,
    pub opened: bool,
    pub reply: Option<OwnerReply>,
    pub done: bool,
}

/// supervisor.py:1017 `_Wait`: one open wait for the owner (a step's question, or a pause of the whole tree, which its
/// steps share: one question, one reply).
#[derive(Default)]
pub struct Wait {
    pub tree_id: String,
    pub question: String,
    pub options: Vec<String>,
    pub origin: String,
    pub capability: String,
    /// GATE-566: the card's own words when code wrote them (summary, detail, exact, labels).
    pub card: Map<String, Value>,
    /// A check-in: the failed attempts it asks about (kept for a restart).
    pub attempts: u32,
    /// A check-in after a report-only retry (ORCH-541): kept for a restart.
    pub report: Map<String, Value>,
    /// The tree's corrections when it opened (a pause): one made after that resumes it.
    pub steered: Option<usize>,
    pub st: Mutex<WaitState>,
}

impl Wait {
    pub fn new(
        tree_id: &str,
        question: &str,
        options: &[&str],
        origin: &str,
        capability: &str,
    ) -> Wait {
        Wait {
            tree_id: tree_id.into(),
            question: question.into(),
            options: options.iter().map(|o| o.to_string()).collect(),
            origin: origin.into(),
            capability: capability.into(),
            ..Wait::default()
        }
    }

    pub fn lock(&self) -> std::sync::MutexGuard<'_, WaitState> {
        self.st.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn extra(&self, key: &str) -> Option<Value> {
        self.lock().extra.get(key).cloned()
    }
}

/// How the attempt loop goes on after its hold on a paused tree (`_worker`'s `if self._is_paused(tree)` branch).
pub enum Held {
    /// Not paused: the attempt runs.
    Go,
    /// Stopped while paused: the loop ends.
    Stop,
    /// Resumed: the next attempt is told this (`_owner_continuation`).
    Resume(String),
}

/// What `_check_in` decided (Python: `str | False | None`).
pub enum CheckIn {
    /// Keep going; the next attempt is told this ("" for a plain Yes).
    Keep(String),
    /// The task fails with what it found (its reason in `stop_reason`).
    Fail,
    /// Stopped while asking.
    Stopped,
}

/// What `_service_down` decided (Python: `bool | None`).
pub enum Switched {
    /// Carry on (on the owner's choice of runtime, or as they said in chat).
    Carry,
    /// The task fails with code's own words.
    Fail,
    Stopped,
}

fn obj(v: Value) -> Map<String, Value> {
    match v {
        Value::Object(m) => m,
        _ => Map::new(),
    }
}

impl Inner {
    /// supervisor.py:1778 `_stopped`: the task was stopped, and FERB isn't merely shutting down (a shutdown suspends
    /// the tree; it resumes after the restart).
    pub fn halted(&self, tree: &Tree) -> bool {
        !tree.lock().stop.is_empty() && !self.live().suspending
    }

    /// supervisor.py:1788 `_unattended`: `request_id in state.approvals.unattended`.
    pub fn unattended_task(&self, task: &Task) -> bool {
        self.deps.daemon.unattended(&task.request_id)
    }

    /// supervisor.py:1804 `task_paused`: whether this task's tree is paused (or a step of it waits for the owner).
    pub fn task_paused(&self, task_id: &str) -> bool {
        let root = self
            .tree_of(task_id)
            .unwrap_or_else(|| task_id.split('.').next().unwrap_or("").to_string());
        {
            let live = self.live();
            if live.pauses.contains_key(&root) || live.waiting.get(&root).copied().unwrap_or(0) > 0
            {
                return true;
            }
        }
        self.plans(|p| p.paused(&root)).unwrap_or(false)
    }

    /// The planner's task state for a running tree (`_task_state`): whether it is paused (the owner's pause, or a step
    /// waiting for them) and the question its latest open wait asked, else its pause's own question.
    pub fn paused_view(&self, tree_id: &str) -> (bool, String) {
        let live = self.live();
        let paused = live.pauses.contains_key(tree_id)
            || live.waiting.get(tree_id).copied().unwrap_or(0) > 0;
        if !paused {
            return (false, String::new());
        }
        let question = live
            .questions
            .get(tree_id)
            .filter(|q| !q.is_empty())
            .cloned()
            .or_else(|| live.pauses.get(tree_id).map(|w| w.question.clone()))
            .unwrap_or_default();
        (true, question)
    }

    /// supervisor.py:1837 `_is_paused`.
    pub fn is_paused(&self, tree: &Tree) -> bool {
        self.live().pauses.contains_key(&tree.id)
    }

    fn poll(&self) -> std::time::Duration {
        std::time::Duration::from_secs_f64(self.deps.daemon.hold_poll_s().max(0.0))
    }

    /// supervisor.py:2051 `_plan_steps`.
    fn plan_step_ids(&self, tree_id: &str) -> BTreeSet<String> {
        self.plans(|p| p.steps(tree_id))
            .map(|s| s.into_iter().map(|(k, _)| k).collect())
            .unwrap_or_default()
    }

    /// supervisor.py:1892 `_hold_pause`: a step of a paused tree waits here for the owner (the tree's one wait). A
    /// No, or Stop, stops the task; an unattended run can't be asked, so a pause with a question stops it there.
    pub async fn hold_pause(&self, tree: &Arc<Tree>, task: &Task, step_id: &str) -> OwnerReply {
        let Some(wait) = self.live().pauses.get(&tree.id).cloned() else {
            return OwnerReply::of("steer", "");
        };
        let decline = !wait.question.is_empty();
        let reply = self
            .wait_for_owner(tree, task, &wait, step_id, true, decline)
            .await;
        let same = {
            let mut live = self.live();
            let same = live
                .pauses
                .get(&tree.id)
                .is_some_and(|w| Arc::ptr_eq(w, &wait));
            if same {
                live.pauses.remove(&tree.id);
            }
            same
        };
        if same {
            self.resumed(tree);
        }
        let stop_choice = reply
            .choice
            .as_deref()
            .is_some_and(|c| c == STOP || c == LOOP_STOP);
        if reply.kind == "denied"
            || reply.kind == "declined"
            || (reply.kind == "answer" && stop_choice)
        {
            self.stop_tree(&tree.id);
            return OwnerReply::of("stopped", "");
        }
        reply
    }

    /// The attempt loop's steering checkpoint (`_worker`: a correction the worker never saw, or a pause, supersedes
    /// this attempt): its done report is kept, marked, for the next attempt, which starts from the correction.
    pub fn superseded_attempt(&self, tree: &Tree, done: bool) {
        let mut st = tree.lock();
        st.work_done = false;
        if done {
            if let Some(report) = st.reports.last_mut() {
                report.insert(
                    "note".into(),
                    json!("Finished before the latest owner correction was delivered."),
                );
            }
        }
    }

    /// The attempt loop's hold (`_worker`: `if self._is_paused(tree): reply = await self._hold_pause(...)`).
    pub async fn hold_if_paused(&self, tree: &Arc<Tree>, task: &Task) -> Held {
        if !self.is_paused(tree) {
            return Held::Go;
        }
        let reply = self.hold_pause(tree, task, &tree.step_id).await;
        if reply.kind == "stopped" || !tree.lock().stop.is_empty() {
            return Held::Stop;
        }
        Held::Resume(owner_continuation(&reply))
    }

    /// supervisor.py:1907 `_wait_for_owner`: the step pauses in place and FERB asks; it ends with the owner's answer,
    /// their reply in chat made a correction of the task (`steer_resumes`), a No, or the task's stop. An unattended
    /// run can't be asked: `declined` at once (`decline`). Waiters that share `wait` get its one reply.
    pub async fn wait_for_owner(
        &self,
        tree: &Arc<Tree>,
        task: &Task,
        wait: &Arc<Wait>,
        step_id: &str,
        steer_resumes: bool,
        decline: bool,
    ) -> OwnerReply {
        if decline && self.unattended_task(task) {
            return OwnerReply::of("declined", &wait.question);
        }
        self.mark_waiting(tree, task, step_id, wait, true);
        let opened = std::mem::replace(&mut wait.lock().opened, true);
        let reply = if opened {
            loop {
                if wait.lock().done || self.halted(tree) {
                    break;
                }
                tokio::time::sleep(self.poll()).await;
            }
            wait.lock()
                .reply
                .clone()
                .unwrap_or_else(|| OwnerReply::of("stopped", ""))
        } else {
            self.ask_and_wait(tree, task, step_id, wait, steer_resumes)
                .await
        };
        self.mark_waiting(tree, task, step_id, wait, false);
        reply
    }

    /// supervisor.py:1927 `_ask_and_wait`.
    async fn ask_and_wait(
        &self,
        tree: &Arc<Tree>,
        task: &Task,
        step_id: &str,
        wait: &Arc<Wait>,
        steer_resumes: bool,
    ) -> OwnerReply {
        let seen = match wait.steered {
            Some(n) => n,
            None => tree.steering.lock().await.messages.len(),
        };
        let mut question = wait.question.clone();
        let loop_ask = wait.capability == LOOP_ASK;
        if !question.is_empty() {
            // the question's bus event reaches a chat stream after what is already on the tree's line (the waiting
            // step's paused and needs_you lines), as Python's queues deliver them; the question, its record and its
            // event then come together
            tokio::task::yield_now().await;
            match self.deps.daemon.questions() {
                None => {
                    // no question store here: nobody can answer, so the step waits as a stop would end it
                    self.journal(
                        "plan.question_unasked",
                        json!({"task_id": tree.id, "step": step_id}),
                        None,
                    );
                }
                Some(questions) => {
                    // a model-written question is tagged as the task's (D33, GATE-225); FERB's own isn't
                    let from_task = wait.origin != origin::FERB;
                    if from_task {
                        question = ferb_policy::confirm::task_question(
                            &ferb_sanitize::terminal::printable(&question),
                        );
                    }
                    let options: Vec<String> = wait
                        .options
                        .iter()
                        .map(|o| {
                            let clean = ferb_sanitize::terminal::printable(o);
                            ferb_policy::confirm::from_task(
                                &clean.split_whitespace().collect::<Vec<_>>().join(" "),
                            )
                        })
                        .collect();
                    let s = |k: &str| {
                        wait.card
                            .get(k)
                            .map(ferb_py::value::py_str)
                            .filter(|t| t != "None")
                            .unwrap_or_default()
                    };
                    let runtime_switch = wait.extra("runtime_switch").filter(|v| !v.is_null());
                    // ORCH-319: the question sits in its part's row (kept on the question, so chat.status lists it)
                    let row = self.part_of(&tree.id, step_id);
                    let ask = Ask {
                        capability: wait.capability.clone(),
                        task_id: task.id.clone(),
                        request_id: task.request_id.clone(),
                        question: question.clone(),
                        options: options.clone(),
                        part: row.as_ref().map(|(_, p)| p.id.clone()).unwrap_or_default(),
                        runtime_switch,
                        from_task,
                        options_only: !from_task && !options.is_empty(),
                        corrects: wait.capability == ferb_orchestrator::steering::CONVERSATION_ASK,
                        summary: s("summary"),
                        detail: s("detail"),
                        exact: s("exact"),
                        labels: wait
                            .card
                            .get("labels")
                            .and_then(Value::as_array)
                            .map(|a| a.iter().map(ferb_py::value::py_str).collect())
                            .unwrap_or_default(),
                    };
                    let id = questions.ask(&ask);
                    wait.lock().approval = Some(id.clone());
                    self.live()
                        .questions
                        .insert(tree.id.clone(), question.clone());
                    self.journal(
                        if loop_ask { "loop.question" } else { "plan.question" },
                        json!({"task_id": tree.id, "step": step_id, "approval_id": id, "question": question,
                               "options": options}),
                        Some(&wait.origin),
                    );
                    let mut data = obj(json!({"approval_id": id, "capability": wait.capability,
                        "kind": "owner_question", "reason": question, "options": options, "task_id": task.id,
                        "request_id": task.request_id}));
                    if let Some((_, part)) = &row {
                        data.insert("part".into(), json!(part.id));
                    }
                    if from_task {
                        data.insert("from_task".into(), json!(true));
                    }
                    if !ask.summary.is_empty() {
                        data.insert("summary".into(), json!(ask.summary));
                    }
                    data.extend(questions.card(&id));
                    data.extend(wait.lock().extra.clone());
                    self.deps
                        .daemon
                        .publish("approval.requested", Value::Object(data));
                    if let Some((card, part)) = &row {
                        // the other rows keep going; a check-in shows its row paused with why
                        let check_in = wait
                            .extra("question_kind")
                            .and_then(|v| v.as_str().map(str::to_string))
                            == Some("check_in".to_string());
                        let reason = if check_in {
                            let r = tree.lock().reason.clone();
                            self.said(&r)
                        } else {
                            String::new()
                        };
                        let shown =
                            json!({"approval_id": id, "text": question, "options": options});
                        self.with_card(&tree.id, card, |c| {
                            c.show(
                                part,
                                if check_in { "paused" } else { "asks" },
                                vec![("question", shown), ("text", json!(reason))],
                            )
                        });
                    }
                }
            }
        }
        let mut reply = OwnerReply::of("stopped", &question);
        while !self.halted(tree) {
            let approval = wait.lock().approval.clone();
            let ap = approval
                .as_ref()
                .and_then(|id| self.deps.daemon.questions().and_then(|q| q.get(id)));
            if let Some(ap) = ap.filter(|a| a.state != "pending") {
                let words = ap
                    .choice
                    .clone()
                    .or_else(|| ap.answer.clone())
                    .unwrap_or_default();
                if ap.state == "granted" && !words.is_empty() {
                    self.journal(
                        if loop_ask { "loop.answer" } else { "plan.answer" },
                        json!({"task_id": tree.id, "step": step_id, "approval_id": ap.id, "answer": words}),
                        Some(origin::OWNER),
                    );
                    reply = OwnerReply {
                        kind: "answer".into(),
                        words,
                        choice: ap.choice.clone(),
                        question: question.clone(),
                        hidden: ap.hidden.clone(),
                    };
                } else if ap.state == "denied" || ap.state == "granted" {
                    reply = OwnerReply::of("denied", &question);
                } else {
                    // set aside by the owner's correction (L3): it carries on from that
                    let last = tree
                        .steering
                        .lock()
                        .await
                        .messages
                        .last()
                        .cloned()
                        .unwrap_or_default();
                    reply = OwnerReply {
                        kind: "steer".into(),
                        words: last,
                        question: question.clone(),
                        ..OwnerReply::default()
                    };
                }
                break;
            }
            if steer_resumes {
                let steering = tree.steering.lock().await;
                if steering.messages.len() > seen {
                    reply = OwnerReply {
                        kind: "steer".into(),
                        words: steering.messages.last().cloned().unwrap_or_default(),
                        question: question.clone(),
                        ..OwnerReply::default()
                    };
                    break;
                }
            }
            tokio::time::sleep(self.poll()).await;
        }
        // an open question nobody can answer any more (resumed another way, or stopped) is closed, never a No; one
        // left open by FERB's shutdown is asked again after the restart (the step is saved paused)
        let suspending = self.live().suspending;
        let approval = wait.lock().approval.clone();
        if let Some(id) = approval {
            if !suspending
                && self
                    .deps
                    .daemon
                    .questions()
                    .is_some_and(|q| q.supersede(&id))
            {
                self.deps.daemon.publish(
                    "approval.resolved",
                    json!({"approval_id": id, "state": "superseded"}),
                );
            }
        }
        if !suspending {
            let mut st = wait.lock();
            st.reply = Some(reply.clone());
            st.done = true;
        }
        let mention = wait
            .extra("mention")
            .is_some_and(|v| ferb_py::value::truthy(&v));
        if (!question.is_empty() || mention)
            && !suspending
            && !self.stopped(tree)
            && reply.kind != "stopped"
        {
            if let Some((card, part)) = self.part_of(&tree.id, step_id) {
                let chosen = reply
                    .choice
                    .as_deref()
                    .filter(|c| !c.is_empty())
                    .map(ferb_policy::confirm::from_task)
                    .unwrap_or_default();
                self.with_card(&tree.id, &card, |c| {
                    c.show(&part, "working", vec![("chosen", json!(chosen))])
                });
            }
        }
        reply
    }

    /// supervisor.py:2006 `_mark_waiting`: the task, its step and its tree show `paused` while a step waits for the
    /// owner; `running` again once no step of the tree waits. The step's row keeps the question, so a restart asks it
    /// again.
    pub fn mark_waiting(
        &self,
        tree: &Arc<Tree>,
        task: &Task,
        step_id: &str,
        wait: &Arc<Wait>,
        on: bool,
    ) {
        {
            let mut live = self.live();
            if live.suspending {
                return;
            }
            let count =
                live.waiting.get(&tree.id).copied().unwrap_or(0) as i64 + if on { 1 } else { -1 };
            if count > 0 {
                live.waiting.insert(tree.id.clone(), count as usize);
            } else {
                live.waiting.remove(&tree.id);
                live.questions.remove(&tree.id);
            }
        }
        if on {
            self.set_status(&task.id, "paused", None);
            self.set_status(&tree.id, "paused", None);
            if self.plan_step_ids(&tree.id).contains(step_id) {
                // which wait it is (a check-in, a runtime switch), so a restart asks the same again, not a pause
                let kind = wait.extra("question_kind").unwrap_or(Value::Null);
                let is_pause = self
                    .live()
                    .pauses
                    .get(&tree.id)
                    .is_some_and(|w| Arc::ptr_eq(w, wait));
                let mut out = obj(json!({"status": "paused", "question": wait.question,
                                         "options": wait.options, "origin": wait.origin}));
                if kind.as_str().is_some_and(|k| RESTORED_WAITS.contains(&k)) && !is_pause {
                    out.insert("question_kind".into(), kind);
                    out.insert("attempts".into(), json!(wait.attempts));
                    out.insert(
                        "runtime_switch".into(),
                        wait.extra("runtime_switch").unwrap_or(Value::Null),
                    );
                    if !wait.report.is_empty() {
                        out.insert("report_only".into(), Value::Object(wait.report.clone()));
                    }
                }
                let _ = self.plans(|p| p.finish(&tree.id, step_id, &Value::Object(out)));
            }
            let summary = if wait.question.is_empty() {
                "Paused.".to_string()
            } else {
                wait.question.clone()
            };
            self.tell_tree(
                &tree.id,
                obj(
                    json!({"type": "subtask", "id": step_id, "status": "paused", "role": "worker",
                           "summary": summary}),
                ),
            );
            self.step_state(&tree.id, step_id, "needs_you", Map::new());
            return;
        }
        if !tree.lock().stop.is_empty() {
            return;
        }
        self.set_status(&task.id, "running", None);
        self.step_state(&tree.id, step_id, "running", Map::new());
        if self.plan_step_ids(&tree.id).contains(step_id) {
            let _ = self.plans(|p| p.finish(&tree.id, step_id, &json!({"status": "running"})));
        }
        self.resumed(tree);
    }

    /// supervisor.py:2042 `_resumed`: the tree runs again once no step of it waits for the owner and it isn't paused.
    pub fn resumed(&self, tree: &Arc<Tree>) {
        {
            let live = self.live();
            if !tree.lock().stop.is_empty()
                || live.suspending
                || live.waiting.get(&tree.id).copied().unwrap_or(0) > 0
                || live.pauses.contains_key(&tree.id)
            {
                return;
            }
        }
        let paused = self
            .task_column(&tree.id, "status")
            .is_some_and(|s| s.as_str() == Some("paused"));
        if paused {
            self.set_status(&tree.id, "running", None);
            let mut data = obj(json!({"task_id": tree.id}));
            if let Some(s) = tree.lock().schedule_id.clone() {
                data.insert("schedule_id".into(), json!(s));
            }
            self.deps
                .daemon
                .publish("task.resumed", Value::Object(data));
        }
    }

    /// supervisor.py:6790 `_check_in` (ORCH-307): after CHECK_IN_AFTER failed attempts, "keep going?". Yes allows
    /// CHECK_IN_AFTER more; No stops with what it found; a run nobody attends stops here and reports.
    pub async fn check_in(
        &self,
        tree: &Arc<Tree>,
        task: &Task,
        attempts: &mut Attempts,
        report: Option<Map<String, Value>>,
    ) -> CheckIn {
        let unattended = self.unattended_task(task);
        if !unattended {
            let goal = tree.spec().goal;
            let mut wait = Wait::new(
                &tree.id,
                &check_in_question(
                    attempts.failed,
                    &ferb_orchestrator::names::short_label(&goal, 60),
                ),
                &[KEEP_GOING, STOP_HERE],
                origin::FERB,
                TASK_ASK,
            );
            wait.attempts = attempts.failed;
            wait.report = report.unwrap_or_default();
            wait.lock()
                .extra
                .insert("question_kind".into(), json!("check_in"));
            let wait = Arc::new(wait);
            let reply = self
                .wait_for_owner(tree, task, &wait, &tree.step_id, true, true)
                .await;
            if reply.kind == "stopped" || !tree.lock().stop.is_empty() {
                return CheckIn::Stopped;
            }
            if reply.kind != "denied"
                && reply.kind != "declined"
                && reply.choice.as_deref() != Some(STOP_HERE)
            {
                attempts.allow_more();
                self.journal(
                    "task.check_in",
                    json!({"task_id": tree.id, "step": tree.step_id, "attempts": attempts.failed,
                           "keep_going": true}),
                    None,
                );
                return CheckIn::Keep(if reply.choice.as_deref() == Some(KEEP_GOING) {
                    String::new()
                } else {
                    owner_continuation(&reply)
                });
            }
        }
        self.journal(
            "task.check_in",
            json!({"task_id": tree.id, "step": tree.step_id, "attempts": attempts.failed,
                   "keep_going": false, "unattended": unattended}),
            None,
        );
        let mut st = tree.lock();
        st.stop_reason = stopped_after(attempts.failed, unattended);
        st.stop_by_code = true;
        CheckIn::Fail
    }

    /// The attempt loop's check-in (`_worker`: `if attempts.failed_one(): keep = await self._check_in(...)`): what the
    /// loop does next.
    pub async fn check_in_step(
        &self,
        tree: &Arc<Tree>,
        task: &Task,
        attempts: &mut Attempts,
        artifacts: Vec<String>,
    ) -> Result<String, Option<Outcome>> {
        match self.check_in(tree, task, attempts, None).await {
            CheckIn::Keep(more) => Ok(more),
            CheckIn::Stopped => Err(None),
            CheckIn::Fail => {
                let reason = tree.lock().stop_reason.clone();
                Err(Some(crate::flow::control::plain_outcome(
                    &task.id, "failed", &reason, artifacts,
                )))
            }
        }
    }

    /// supervisor.py:6688 `_service_down` (ORCH-315): a usage limit or a sign-in failure is said once, by code, and
    /// FERB offers each other signed-in runtime with one tap; it never switches by itself (billing).
    pub async fn service_down(
        &self,
        tree: &Arc<Tree>,
        task: &Task,
        down_task_id: &str,
        runtime: &str,
        problem: &str,
        summary: &str,
    ) -> Switched {
        let role = if down_task_id.ends_with(".verify") {
            "checker"
        } else {
            "worker"
        };
        let choices: Vec<String> = self
            .deps
            .daemon
            .worker_types()
            .into_iter()
            .filter(|n| n != "default" && n != runtime)
            .collect();
        let offer =
            json!({"runtime": runtime, "problem": problem, "role": role, "choices": choices});
        let mut data = obj(json!({"task_id": tree.id}));
        data.extend(obj(offer.clone()));
        self.journal("runtime.account_problem", Value::Object(data), None);
        {
            let mut st = tree.lock();
            st.runtime_switch = Some(offer.clone());
            st.stop_reason = summary.to_string();
            st.stop_by_code = true;
            if choices.is_empty() {
                st.stop_reason =
                    format!("{summary} No other AI service is signed in for FERB (Settings → AI).");
                return Switched::Fail;
            }
        }
        if !tree.asks_in_place {
            return Switched::Fail;
        }
        let label = ferb_worker::errors::label;
        let mut options: Vec<String> = choices
            .iter()
            .map(|n| format!("Use {}", label(n)))
            .collect();
        options.push(STOP.into());
        let down_said = if problem == "usage_limit" {
            format!("{} hit its usage limit. Switch AI?", label(runtime))
        } else {
            format!("{} isn't signed in for FERB. Switch AI?", label(runtime))
        };
        let names: Vec<String> = choices.iter().map(|n| label(n)).collect();
        let option_refs: Vec<&str> = options.iter().map(String::as_str).collect();
        let mut wait = Wait::new(
            &tree.id,
            &format!("{summary} Carry on with {} instead?", names.join(" or ")),
            &option_refs,
            origin::FERB,
            RUNTIME_ASK,
        );
        wait.card = obj(json!({"summary": down_said, "detail": RUNTIME_SWITCH_DETAIL}));
        {
            let mut st = wait.lock();
            st.extra.insert("runtime_switch".into(), offer);
            st.extra
                .insert("question_kind".into(), json!("runtime_switch"));
        }
        let wait = Arc::new(wait);
        let reply = self
            .wait_for_owner(tree, task, &wait, &tree.step_id, true, true)
            .await;
        if reply.kind == "stopped" || !tree.lock().stop.is_empty() {
            return Switched::Stopped;
        }
        let clear = |st: &mut super::TreeState| {
            st.runtime_switch = None;
            st.stop_reason.clear();
            st.stop_by_code = false;
        };
        if reply.kind == "steer" {
            clear(&mut tree.lock());
            return Switched::Carry;
        }
        let asked: Vec<String> = wait
            .lock()
            .approval
            .clone()
            .and_then(|id| self.deps.daemon.questions().and_then(|q| q.get(&id)))
            .map(|a| a.options)
            .unwrap_or_default();
        if reply.kind == "answer" {
            if let Some(i) = reply
                .choice
                .as_ref()
                .and_then(|c| asked.iter().take(choices.len()).position(|o| o == c))
            {
                let chosen = choices[i].clone();
                {
                    let mut st = tree.lock();
                    st.runtime = chosen.clone();
                    clear(&mut st);
                }
                self.journal(
                    "runtime.switched",
                    json!({"task_id": tree.id, "runtime": chosen, "role": role}),
                    Some(origin::OWNER),
                );
                return Switched::Carry;
            }
        }
        Switched::Fail
    }

    /// The attempt loop's service-down branch (`_worker`: `if down is not None and not tree.stop`): Ok(instructions)
    /// to carry on, Err(None) to end the loop (stopped), Err(Some(outcome)) to end the step failed.
    pub async fn service_down_step(
        &self,
        tree: &Arc<Tree>,
        task: &Task,
        down: &Outcome,
        runtime: &str,
        problem: &str,
    ) -> Result<String, Option<Outcome>> {
        match self
            .service_down(tree, task, &down.task_id, runtime, problem, &down.summary)
            .await
        {
            Switched::Stopped => Err(None),
            Switched::Carry => {
                let chosen = tree.lock().runtime.clone();
                Ok(if chosen.is_empty() {
                    String::new()
                } else {
                    format!(
                        "\nThe owner chose to carry on this task with {}.",
                        ferb_worker::errors::label(&chosen)
                    )
                })
            }
            Switched::Fail => Err(Some(crate::flow::control::plain_outcome(
                &task.id,
                "failed",
                &down.summary,
                Vec::new(),
            ))),
        }
    }

    /// supervisor.py:6277 `_restored`: the check-in or runtime switch this step waited on when FERB restarted, asked
    /// again as itself. Ok(instructions) for the first attempt; Err(outcome) for the step's end.
    pub async fn restored_wait(
        &self,
        tree: &Arc<Tree>,
        task: &Task,
        attempts: &mut Attempts,
        span: &str,
    ) -> Result<String, Outcome> {
        let saved = std::mem::take(&mut tree.lock().restored_wait);
        let stopped = |me: &Inner| {
            let stop = tree.lock().stop.clone();
            let _ = me;
            crate::flow::control::plain_outcome(
                &task.id,
                "cancelled",
                if stop.is_empty() {
                    STOPPED_BY_YOU
                } else {
                    &stop
                },
                Vec::new(),
            )
        };
        if saved.get("question_kind").and_then(Value::as_str) == Some("check_in") {
            let n = saved
                .get("attempts")
                .and_then(Value::as_u64)
                .filter(|n| *n > 0)
                .map(|n| n as u32)
                .unwrap_or(CHECK_IN_AFTER)
                .max(1);
            attempts.failed = n;
            attempts.allowed = n;
            let report = saved
                .get("report_only")
                .and_then(Value::as_object)
                .cloned()
                .unwrap_or_default();
            let given = (!report.is_empty()).then(|| report.clone());
            return match self.check_in(tree, task, attempts, given).await {
                CheckIn::Stopped => Err(stopped(self)),
                CheckIn::Keep(more) => Ok(more),
                CheckIn::Fail => {
                    let answer = report
                        .get("answer")
                        .map(ferb_py::value::py_str)
                        .unwrap_or_default();
                    if !ferb_orchestrator::py::strip(&answer).is_empty() {
                        // STORE-403: a check-in an older build saved after a report-only retry (ORCH-541) keeps that
                        // answer: a No ends done-unconfirmed with it (`_report_unconfirmed`)
                        let artifacts: Vec<String> = report
                            .get("artifacts")
                            .and_then(Value::as_array)
                            .map(|a| a.iter().map(ferb_py::value::py_str).collect())
                            .unwrap_or_default();
                        Err(self
                            .report_unconfirmed(tree, &answer, artifacts, span)
                            .await)
                    } else {
                        let reason = tree.lock().stop_reason.clone();
                        Err(crate::flow::control::plain_outcome(
                            &task.id,
                            "failed",
                            &reason,
                            Vec::new(),
                        ))
                    }
                }
            };
        }
        let Some(offer) = saved
            .get("runtime_switch")
            .and_then(Value::as_object)
            .cloned()
        else {
            return Ok(String::new());
        };
        let runtime = offer
            .get("runtime")
            .map(ferb_py::value::py_str)
            .unwrap_or_default();
        if runtime.is_empty() || runtime == "None" {
            return Ok(String::new());
        }
        let problem = offer
            .get("problem")
            .map(ferb_py::value::py_str)
            .unwrap_or_default();
        let down_id = if offer.get("role").and_then(Value::as_str) == Some("checker") {
            format!("{}.verify", task.id)
        } else {
            task.id.clone()
        };
        let summary = ferb_worker::errors::account_problem(&runtime, &problem);
        match self
            .service_down(tree, task, &down_id, &runtime, &problem, &summary)
            .await
        {
            Switched::Stopped => Err(stopped(self)),
            Switched::Fail => Err(crate::flow::control::plain_outcome(
                &task.id,
                "failed",
                &summary,
                Vec::new(),
            )),
            Switched::Carry => {
                let chosen = tree.lock().runtime.clone();
                Ok(if chosen.is_empty() {
                    String::new()
                } else {
                    format!(
                        "\nThe owner chose to carry on this task with {}.",
                        ferb_worker::errors::label(&chosen)
                    )
                })
            }
        }
    }

    /// supervisor.py:6263 `_report_unconfirmed` (STORE-403 only): the work's own outcome stands, done-unconfirmed,
    /// with the saved answer and code's one "Couldn't confirm" line.
    async fn report_unconfirmed(
        &self,
        tree: &Arc<Tree>,
        answer: &str,
        artifacts: Vec<String>,
        span: &str,
    ) -> Outcome {
        {
            let mut st = tree.lock();
            st.owner_answer = answer.to_string();
            st.unconfirmed = REPORT_UNCONFIRMED.into();
            st.stands_unconfirmed = true;
            st.stop_reason.clear();
            st.stop_by_code = false;
        }
        let mine = self
            .live()
            .tree_of
            .get(&tree.id)
            .is_some_and(|t| Arc::ptr_eq(t, tree));
        if mine {
            tree.lock().work_done = true;
        }
        self.release_resources(tree, span).await;
        let done = crate::flow::control::plain_outcome(&tree.worker.id, "done", answer, artifacts);
        tree.lock().outcome = Some(done.clone());
        done
    }

    /// The attempt loop's "needs the owner" (`_worker`'s `choice.action == "ask"` with `asks_in_place`): Ok(what the
    /// next attempt is told) when the owner answered or replied; Err(true) when stopped; Err(false) when it ends as
    /// needs_user (a No, nobody there).
    pub async fn ask_in_place(
        &self,
        tree: &Arc<Tree>,
        task: &Task,
        question: &str,
    ) -> Result<String, bool> {
        let made = self.task_made(&tree.id);
        let wait = Arc::new(Wait::new(&tree.id, question, &[], &made, TASK_ASK));
        let reply = self
            .wait_for_owner(tree, task, &wait, &tree.step_id, true, true)
            .await;
        if reply.kind == "answer" || reply.kind == "steer" {
            tree.lock().owner_question.clear();
            return Ok(owner_continuation(&reply));
        }
        let stopped = reply.kind == "stopped" || !tree.lock().stop.is_empty();
        Err(stopped)
    }
}

/// supervisor.py:6781 `_owner_continuation`: what the next attempt is told after the owner answered or replied.
pub fn owner_continuation(reply: &OwnerReply) -> String {
    if reply.kind == "answer" && !reply.question.is_empty() {
        let note = if reply.hidden.is_empty() {
            String::new()
        } else {
            let kinds: Vec<&str> = reply.hidden.iter().map(String::as_str).collect();
            format!(
                "\n{}",
                ferb_sanitize::formats::hidden_note(&kinds, "answer")
            )
        };
        return format!(
            "\nThe owner's answer to the question “{}”: {}{note}",
            reply.question, reply.words
        );
    }
    "\nThe owner replied: their words are in the latest owner corrections. Carry on from there."
        .into()
}
