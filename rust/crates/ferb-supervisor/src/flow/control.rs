//! Control of running work (supervisor.py:2866 `_control`, :2979 `_then_start`, :2993 `_stop_told`, :2855
//! `_settled`, :1760 `stop_tree`, :1810 `pause_tree`, :6125 `cancel_tree`, :2057 `_interrupt`, :2279 `cancel_all`,
//! :3054 `_kept_unchanged`, :3069 `_steered_extras`, :1371 `_reopen_noes`; steering.py:221 `apply` and :92
//! `settle_questions`; areas/chat.py:168 `_task_stop`).
//!
//! A planner answer with a `control` stops, pauses, corrects (continue) or retries a task it was shown; a stop or a
//! pause whose answer also carries new work does both, and the `reply` still ends the turn (ORCH-638). A correction
//! goes into every live worker run of the tree under its steering lock (ferb-worker's `deliver`); a stop sets the
//! tree's stop words and cancels its runs (`Runs::active_of`); a pause ends the current attempt the same way but
//! keeps the task (ORCH-308).
//!
//! A continue or retry of a task that already ended (`_resume`) runs its tree again in place (`run_tree(resume_id=...)`).
//! Not here: a revised standing scope in a correction (`restate_scope`, D30) needs the gateway's standing grants:
//! `NotPorted`.

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

use ferb_core::types::Outcome;
use ferb_orchestrator::models::{StandingSends, SupervisorOutcome, TaskSpec};
use ferb_orchestrator::steering::{acknowledgement, FERB_CHECKS, JUST_ENDED, TASK_CHECKS};
use ferb_orchestrator::validate::{
    dump_json_task_spec_without_plan, dump_task_spec, task_spec_validate,
};
use ferb_sanitize::origin;
use serde_json::{json, Map, Value};

use super::owner_wait::Wait;
use super::turn::origin_set;
use super::{Emit, Failure, Flow, Inner, Request, Supervisor, Tree, Turn};
use crate::words::*;

fn obj(v: Value) -> Map<String, Value> {
    match v {
        Value::Object(m) => m,
        _ => Map::new(),
    }
}

/// An Outcome code writes (worker.py's `Outcome(task_id, status, summary, artifacts, [])`).
pub fn plain_outcome(
    task_id: &str,
    status: &str,
    summary: &str,
    artifacts: Vec<String>,
) -> Outcome {
    Outcome {
        task_id: task_id.to_string(),
        status: super::tree::status_of(status),
        summary: summary.to_string(),
        artifacts,
        evidence: Vec::new(),
    }
}

/// `_exit`'s keyword arguments (supervisor.py:2755).
#[derive(Clone, Debug)]
pub struct ExitAs {
    /// Who wrote the text: `origin::FERB` (code), else the planner (`origin::INFERRED`).
    pub by: String,
    /// The turn's side entries are applied here.
    pub side: bool,
    /// Already in its final form (a steer's acknowledgement, a stop's told end).
    pub ready: bool,
    /// What wrote the reply, when the caller knows (`made`).
    pub made: Option<BTreeSet<String>>,
    /// Sites the task named before a steer (for the side entries).
    pub named: BTreeSet<String>,
    /// More fields of the reply event.
    pub extra: Option<Map<String, Value>>,
    /// ORCH-596: (task id, its end) when the reply starts with a stopped task's end.
    pub told: Option<(String, String)>,
}

impl ExitAs {
    pub fn by(by: &str) -> ExitAs {
        ExitAs {
            by: by.to_string(),
            side: true,
            ready: false,
            made: None,
            named: BTreeSet::new(),
            extra: None,
            told: None,
        }
    }
}

impl Supervisor {
    /// The daemon's `task.stop` (areas/chat.py:168): the owner's stop of exactly these tasks, recorded as stopped by
    /// the owner. An empty list is refused: stopping everything is the global stop (DMN-1).
    pub fn task_stop(&self, params: &Map<String, Value>) -> Result<Value, String> {
        self.inner.task_stop(params)
    }

    /// supervisor.py:1760 `stop_tree` (the stub in mod.rs answers NotPorted; this is the port).
    pub fn stop_task(&self, tree_id: &str) -> bool {
        self.inner.stop_tree(tree_id)
    }

    /// supervisor.py:1810 `pause_tree` (FERB's own pause, no question).
    pub async fn pause_task(&self, tree_id: &str) -> bool {
        self.inner
            .pause_tree(tree_id, "", &[], origin::FERB, TASK_ASK)
            .await
    }

    /// supervisor.py:2279 `cancel_all`: the global stop's hook ("runtimes") and the shutdown's.
    pub fn cancel_all(&self) {
        self.inner.cancel_all()
    }

    /// supervisor.py:6745 `correct_from_question` (GATE-510): the owner's words typed in the box of a safety check's
    /// card, already redacted as a chat message is: their correction of exactly that check's task, through the one
    /// steering path. Err: the refusal's words. (The stub `correct_from_question` in mod.rs is this.)
    pub async fn correct_from_card(
        &self,
        approval_id: &str,
        words: &str,
        hidden: &[String],
    ) -> Result<Value, String> {
        self.inner
            .correct_from_question(approval_id, words, hidden)
            .await
    }

    /// supervisor.py:2240 `running_trees`: (tree id, goal) of every running tree.
    pub fn running_trees(&self) -> Vec<(String, String)> {
        self.inner.running_trees()
    }
}

impl Inner {
    /// areas/chat.py:168 `_task_stop`.
    pub fn task_stop(&self, params: &Map<String, Value>) -> Result<Value, String> {
        let ids: Vec<String> = match params.get("task_ids") {
            Some(Value::Array(a)) => a.iter().map(ferb_py::value::py_str).collect(),
            Some(v) if ferb_py::value::truthy(v) => match v {
                // `[str(t) for t in params.get("task_ids") or []]`: a string's characters, a mapping's keys
                Value::String(s) => s.chars().map(|c| c.to_string()).collect(),
                Value::Object(m) => m.keys().cloned().collect(),
                // Python raises TypeError (an internal error on the wire, not a FerbError)
                _ => return Err(format!("'{}' object is not iterable", py_type(v))),
            },
            _ => Vec::new(),
        };
        if ids.is_empty() {
            return Err(
                "task.stop needs the ids of the tasks to stop; stopping everything is stop.engage"
                    .into(),
            );
        }
        let stopped: Vec<String> = ids.into_iter().filter(|t| self.stop_tree(t)).collect();
        for t in &stopped {
            self.journal("task.stopped", json!({"task_id": t, "by": "owner"}), None);
        }
        Ok(json!({"stopped": stopped}))
    }

    /// supervisor.py:6745 `correct_from_question`.
    pub async fn correct_from_question(
        &self,
        approval_id: &str,
        words: &str,
        hidden: &[String],
    ) -> Result<Value, String> {
        // any open question of the task's: the gateway's (a confirmation, a retry) or the supervisor's own
        let ap = self.deps.daemon.approval(approval_id);
        let Some(ap) = ap.filter(|a| a.state.as_str() == "pending") else {
            return Err(CARD_NOT_WAITING.into());
        };
        let root = if ap.task_id.is_empty() {
            None
        } else {
            self.tree_of(&ap.task_id)
        };
        let Some(root) = root.filter(|_| ap.corrects) else {
            return Err(CARD_NO_TASK.into());
        };
        let tree = self.live().tree_of.get(&root).cloned();
        let session = self.continuity(|c| c.conversation_of(&root)).ok().flatten();
        let (Some(tree), Some(session)) = (tree, session) else {
            return Err(CARD_TASK_ENDED.into());
        };
        let ended = {
            let st = tree.lock();
            !st.stop.is_empty() || !st.accepting_updates
        };
        if tree.id != root || ended {
            return Err(CARD_TASK_ENDED.into());
        }
        let request_id = self.new_id();
        let marked = ferb_sanitize::untrusted::mark(words);
        let entry = self.journal(
            "chat.message",
            json!({"session_id": session, "request_id": request_id, "text": marked.for_model,
                   "untrusted_parts": 0, "input_paths": [], "correction_of": approval_id}),
            None,
        );
        let payload = if hidden.is_empty() {
            marked.for_model.clone()
        } else {
            let kinds: Vec<&str> = hidden.iter().map(String::as_str).collect();
            format!(
                "{}\n\n{}",
                marked.for_model,
                ferb_sanitize::formats::hidden_note(&kinds, "message")
            )
        };
        // no chat turn carries it (`_turns.get(request_id)` is None in Python): its words, payload and entry only
        let turn = Arc::new(Mutex::new(Turn {
            request_id: request_id.clone(),
            session: session.clone(),
            own_words: marked.own_words.clone(),
            payload,
            entries: vec![entry],
            ..Turn::default()
        }));
        let steered = self
            .apply_steering(
                &tree,
                &session,
                &turn,
                None,
                &StandingSends::Unchanged,
                &[],
                "",
            )
            .await;
        let steered = match steered {
            Ok(s) => s,
            Err(e) => {
                self.wake_deferred(&turn);
                return Err(e.message());
            }
        };
        let Some((text, _)) = steered else {
            self.wake_deferred(&turn);
            return Err(CARD_TASK_ENDED.into());
        };
        self.hand_over(&turn).await;
        self.journal(
            "chat.reply",
            json!({"session_id": session, "request_id": request_id, "text": text}),
            Some(origin::FERB),
        );
        self.wake_deferred(&turn);
        let state = self
            .deps
            .daemon
            .approval(approval_id)
            .map(|a| a.state.as_str().to_string())
            .unwrap_or_else(|| "gone".into());
        Ok(json!({"task_id": tree.id, "request_id": request_id, "text": text, "state": state}))
    }

    /// supervisor.py:2240 `running_trees`.
    pub fn running_trees(&self) -> Vec<(String, String)> {
        let live = self.live();
        let mut out: Vec<(String, String)> = live
            .tree_of
            .iter()
            .filter(|(k, t)| **k == t.id)
            .map(|(k, t)| {
                (
                    k.clone(),
                    t.lock()
                        .spec
                        .as_ref()
                        .map(|s| s.goal.clone())
                        .unwrap_or_default(),
                )
            })
            .collect();
        out.sort();
        out
    }

    /// `_tree_of.get(task_id).id`: the tree a task runs in. Python's `_tree_of` is one map the worker shares (every
    /// run registers its task, a check's `.verify` too); here the worker's half is `Runs::tree_of`.
    pub fn tree_id_of(&self, task_id: &str) -> Option<String> {
        if let Some(t) = self.live().tree_of.get(task_id) {
            return Some(t.id.clone());
        }
        self.runs.tree_of(task_id).map(|t| t.id())
    }

    /// supervisor.py:6121 `_tree_task_ids`.
    pub fn member_ids(&self, tree: &Tree) -> BTreeSet<String> {
        let mut ids: BTreeSet<String> = self
            .live()
            .tree_of
            .iter()
            .filter(|(_, t)| t.id == tree.id)
            .map(|(k, _)| k.clone())
            .collect();
        ids.extend(
            self.runs
                .trees()
                .into_iter()
                .filter(|(_, t)| t.id() == tree.id)
                .map(|(k, _)| k),
        );
        ids.insert(tree.worker.id.clone());
        ids.insert(format!("{}.verify", tree.worker.id));
        ids
    }

    /// supervisor.py:1760 `stop_tree`: the owner's stop of this task, recorded as stopped by the owner. A task whose
    /// work has already finished isn't stopped: its finished work is never recorded as cancelled.
    pub fn stop_tree(&self, tree_id: &str) -> bool {
        let Some(tree) = self.live().tree_of.get(tree_id).cloned() else {
            return false;
        };
        {
            let mut st = tree.lock();
            if (st.work_done || !st.accepting_updates) && st.stop.is_empty() {
                return false;
            }
            if st.stop.is_empty() {
                st.stop = STOPPED_BY_YOU.into();
            }
        }
        self.cancel_tree(&tree);
        true
    }

    /// supervisor.py:6125 `cancel_tree`.
    pub fn cancel_tree(&self, tree: &Arc<Tree>) {
        self.conversations().wake_all();
        let stop = {
            let s = tree.lock().stop.clone();
            if s.is_empty() {
                STOPPED_BY_YOU.to_string()
            } else {
                s
            }
        };
        let members: Vec<Arc<Tree>> = self
            .live()
            .tree_of
            .values()
            .filter(|m| m.id == tree.id)
            .cloned()
            .collect();
        for member in members {
            member.lock().stop = stop.clone();
        }
        // (the plan runner sees the stop words at its next look; the tree's loops look now)
        self.wake_loops(&tree.id);
        self.interrupt(tree, true);
    }

    /// supervisor.py:2057 `_interrupt`: end what the tree is doing now (its model runs, its tool calls in flight), for
    /// a stop (its tasks may call no tool again) or a pause (they may, once resumed).
    pub fn interrupt(&self, tree: &Tree, stop: bool) {
        let ids = self.member_ids(tree);
        if stop {
            self.deps.daemon.cancel_tasks(&ids);
        } else {
            self.deps.daemon.interrupt_tasks(&ids);
        }
        for (worker_id, run) in self.runs.worker_runs() {
            if ids.contains(&run.handle.task_id) {
                self.deps.daemon.revoke_worker(&worker_id);
            }
        }
        for run in self.runs.active_of(&ids) {
            tokio::spawn(async move { run.control.cancel().await });
        }
    }

    /// supervisor.py:2279 `cancel_all`: every running run is cancelled and every tree stops ("FERB was stopped"; a
    /// shutdown's stop is SHUT_DOWN and the tree resumes after the restart).
    pub fn cancel_all(&self) {
        self.conversations().wake_all();
        for run in self.runs.active() {
            tokio::spawn(async move { run.control.cancel().await });
        }
        let suspending = self.live().suspending;
        let trees: Vec<Arc<Tree>> = self.live().tree_of.values().cloned().collect();
        let mut seen: Vec<*const Tree> = Vec::new();
        for tree in trees {
            let p = Arc::as_ptr(&tree);
            if seen.contains(&p) {
                continue;
            }
            seen.push(p);
            let mut st = tree.lock();
            if st.stop.is_empty() {
                st.stop = if suspending {
                    SHUT_DOWN.into()
                } else {
                    "FERB was stopped".into()
                };
            }
        }
        let looping: Vec<String> = self.live().loop_wakes.keys().cloned().collect();
        for tree_id in looping {
            self.wake_loops(&tree_id);
        }
    }

    /// supervisor.py:1810 `pause_tree` (ORCH-308): the current attempt ends, nothing is released or reported; every
    /// step waits for the owner (`_hold_pause`). `question`: FERB asks it too, and its answer resumes or stops it.
    pub async fn pause_tree(
        &self,
        tree_id: &str,
        question: &str,
        options: &[&str],
        made: &str,
        capability: &str,
    ) -> bool {
        let Some(tree) = self.live().tree_of.get(tree_id).cloned() else {
            return false;
        };
        if !tree.lock().stop.is_empty() || tree.id != tree_id {
            return false;
        }
        let known = self.live().pauses.contains_key(tree_id);
        if !known {
            let steered = tree.steering.lock().await.messages.len();
            let mut wait = Wait::new(tree_id, question, options, made, capability);
            wait.steered = Some(steered);
            if !question.is_empty() {
                wait.lock()
                    .extra
                    .insert("question_kind".into(), json!("closed_tab"));
            }
            self.live()
                .pauses
                .insert(tree_id.to_string(), Arc::new(wait));
            self.journal(
                "task.paused",
                json!({"task_id": tree_id, "question": question}),
                Some(made),
            );
            self.set_status(tree_id, "paused", None);
            // ORCH-312: the owner's pause of a scheduled run pauses that schedule's next runs too (the daemon)
            let mut data = obj(json!({"task_id": tree_id}));
            if let Some(s) = tree
                .lock()
                .schedule_id
                .clone()
                .filter(|_| question.is_empty())
            {
                data.insert("schedule_id".into(), json!(s));
            }
            self.deps.daemon.publish("task.paused", Value::Object(data));
        }
        self.interrupt(&tree, false);
        self.wake_loops(tree_id);
        true
    }

    /// supervisor.py:2843 `_control_conversation`: the chat a task control may act in (any running task the planner
    /// is shown, whichever chat or schedule it belongs to, or one of this chat's own tasks).
    fn control_chat(&self, task_id: &str, session_id: &str) -> Option<String> {
        if self
            .continuity(|c| c.load(task_id, session_id))
            .ok()
            .flatten()
            .is_some()
        {
            return Some(session_id.to_string());
        }
        let tree = self.live().tree_of.get(task_id).cloned();
        if tree.is_some_and(|t| t.id == task_id) {
            let other = self
                .continuity(|c| c.conversation_of(task_id))
                .ok()
                .flatten()?;
            if self
                .continuity(|c| c.load(task_id, &other))
                .ok()
                .flatten()
                .is_some()
            {
                return Some(other);
            }
        }
        None
    }

    /// supervisor.py:2855 `_settled`: whether the task's run has ended and none of its calls is still in flight,
    /// waiting up to `wait_s` (None: until it has).
    async fn settled(&self, task_id: &str, ids: &BTreeSet<String>, wait_s: Option<f64>) -> bool {
        let deadline =
            wait_s.map(|s| tokio::time::Instant::now() + std::time::Duration::from_secs_f64(s));
        loop {
            let running =
                self.live().tree_of.contains_key(task_id) || self.deps.daemon.in_flight(ids);
            if !running {
                return true;
            }
            if deadline.is_some_and(|d| tokio::time::Instant::now() >= d) {
                return false;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    }

    /// supervisor.py:2993 `_stop_told` (ORCH-579): the end of a task the owner stopped from this request, when it was
    /// recorded with this request as the one telling it: its answer, its cards and what wrote it.
    fn stop_told(
        &self,
        task_id: &str,
        request_id: &str,
    ) -> Option<(String, Vec<Value>, BTreeSet<String>)> {
        if self.live().tree_of.contains_key(task_id) {
            return None;
        }
        let raw = self
            .task_column(task_id, "outcome_json")
            .and_then(|v| v.as_str().map(str::to_string))
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "{}".into());
        let recorded: Value = serde_json::from_str(&raw).unwrap_or(json!({}));
        let answer = recorded
            .get("answer")
            .map(ferb_py::value::py_str)
            .unwrap_or_default();
        let answer = if recorded
            .get("answer")
            .is_some_and(|a| !ferb_py::value::truthy(a))
        {
            String::new()
        } else {
            answer
        };
        if recorded.get("told_in").and_then(Value::as_str) != Some(request_id)
            || ferb_orchestrator::py::strip(&answer).is_empty()
        {
            return None;
        }
        let cards: Vec<Value> = recorded
            .get("cards")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter(|c| c.is_object()).cloned().collect())
            .unwrap_or_default();
        Some((answer, cards, origin::split(Some(&self.task_made(task_id)))))
    }

    /// supervisor.py:7539 `_told_in` (ORCH-579): the request whose reply tells the owner's stop of this task, so
    /// FERB.app files this end on the task's own turn instead of posting it again.
    pub fn told_in(&self, tree: &Tree) -> Map<String, Value> {
        let st = tree.lock();
        if st.stop == STOPPED_BY_YOU && !st.stop_told_in.is_empty() {
            return obj(json!({"told_in": st.stop_told_in}));
        }
        Map::new()
    }

    /// supervisor.py:2866 `_control`: a stop, retry, continue or pause of a task the planner was shown.
    pub async fn control(
        &self,
        turn: &Arc<Mutex<Turn>>,
        out: SupervisorOutcome,
        emit: &Emit,
    ) -> Flow<()> {
        let control = out.control.clone().expect("a control");
        let then = out.new_task().cloned();
        let session = turn.lock().unwrap().session.clone();
        let conversation = self.control_chat(&control.task_id, &session);
        let prior = conversation.as_ref().and_then(|c| {
            self.continuity(|s| s.load(&control.task_id, c))
                .ok()
                .flatten()
        });
        let ferb = origin_set(&[origin::FERB]);
        let (Some(conversation), Some(_prior)) = (conversation, prior) else {
            // a task that ended and went between the check and now
            if let Some(spec) = then {
                return self
                    .then_start(
                        turn,
                        &out,
                        spec,
                        UNKNOWN_TASK,
                        &ferb,
                        Vec::new(),
                        None,
                        emit,
                    )
                    .await;
            }
            return self
                .exit_as(
                    turn,
                    &out,
                    "reply",
                    UNKNOWN_TASK,
                    ExitAs::by(origin::FERB),
                    emit,
                )
                .await;
        };
        let active = self.live().tree_of.get(&control.task_id).cloned();
        let accepting = |t: &Arc<Tree>| {
            let st = t.lock();
            st.stop.is_empty() && st.accepting_updates
        };
        if control.action == "pause" {
            // ORCH-308: the running task pauses in place, losing nothing; the owner's next words resume it
            let paused = match &active {
                Some(a) if accepting(a) => {
                    self.pause_tree(&a.id, "", &[], origin::FERB, TASK_ASK)
                        .await
                }
                _ => false,
            };
            if let Some(spec) = then {
                let said = if paused { PAUSED_SAID } else { NOT_RUNNING };
                return self
                    .then_start(turn, &out, spec, said, &ferb, Vec::new(), None, emit)
                    .await;
            }
            let rest = ferb_orchestrator::py::strip(&out.text).to_string();
            let (text, by) = if paused && !rest.is_empty() {
                // ORCH-578: code's own words say it paused, the planner's follow
                (format!("{PAUSED_SAID}\n\n{rest}"), origin::INFERRED)
            } else if paused {
                (PAUSED_SAID.to_string(), origin::FERB)
            } else {
                (NOT_RUNNING.to_string(), origin::FERB)
            };
            return self
                .exit_as(turn, &out, "reply", &text, ExitAs::by(by), emit)
                .await;
        }
        if control.action == "continue" {
            if let Some(active) = active.clone().filter(|a| accepting(a)) {
                // the sites the task named before this steer (L36 §5)
                let named: BTreeSet<String> = active.spec().targets.sites.iter().cloned().collect();
                self.reopen_noes(&control.task_id, turn);
                let kept = self.kept_unchanged(&active, out.task.as_ref());
                self.steered_extras(&active, out.task.as_ref());
                let said = self.redact_text(&out.text);
                if let Some((text, restated)) = self
                    .apply_steering(
                        &active,
                        &conversation,
                        turn,
                        out.task.as_ref(),
                        &control.standing_sends,
                        &control.supersedes,
                        &said,
                    )
                    .await?
                {
                    // the woken loop's round starts before this reply is written, as on Python's one thread
                    self.hand_over(turn).await;
                    if text == JUST_ENDED {
                        // a correction that found its task ending applies none of the turn's entries
                        let mut opts = ExitAs::by(origin::FERB);
                        opts.side = false;
                        return self
                            .exit_as(turn, &out, "reply", JUST_ENDED, opts, emit)
                            .await;
                    }
                    let text = if kept.is_empty() {
                        text
                    } else {
                        format!("{text}\n{}", kept.join("\n"))
                    };
                    let mut opts = ExitAs::by(origin::INFERRED);
                    opts.ready = true;
                    opts.named = named;
                    opts.extra = Some(obj(json!({"task_id": active.id})));
                    self.exit_as(turn, &out, "reply", &text, opts, emit).await?;
                    if let Some(restated) = restated.filter(|r| r.widened) {
                        // a new or wider scope covers sends only once stated
                        self.grant_restated(&active, &restated);
                    }
                    return Ok(());
                }
            }
            if let Some(active) = active.clone().filter(|a| !a.lock().accepting_updates) {
                // its work is complete: carried on in place once it has finished
                while self
                    .live()
                    .tree_of
                    .get(&active.id)
                    .is_some_and(|t| Arc::ptr_eq(t, &active))
                {
                    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                }
            }
        }
        if control.action == "stop" || control.action == "retry" {
            let tree = self.live().tree_of.get(&control.task_id).cloned();
            let ids = tree
                .as_ref()
                .map(|t| self.member_ids(t))
                .unwrap_or_default();
            let finished = tree.as_ref().is_some_and(|t| {
                let st = t.lock();
                (st.work_done || !st.accepting_updates) && st.stop.is_empty()
            });
            let rid = turn.lock().unwrap().request_id.clone();
            if control.action == "stop" {
                if let Some(t) = &tree {
                    // ORCH-579: this reply tells the stop, if its end comes in time
                    t.lock().stop_told_in = rid.clone();
                }
            }
            let was_running = self.stop_tree(&control.task_id);
            let settled = self.settled(&control.task_id, &ids, Some(10.0)).await;
            if control.action == "stop" {
                let told = if was_running {
                    self.stop_told(&control.task_id, &rid)
                } else {
                    None
                };
                if told.is_none() {
                    if let Some(t) = &tree {
                        // still finishing: its own end, when it comes, tells it
                        t.lock().stop_told_in.clear();
                    }
                }
                if let (Some((answer, cards, made)), Some(spec)) = (&told, &then) {
                    // ORCH-638: the stop told, then the message's new task; the planner's text is its plan line
                    return self
                        .then_start(
                            turn,
                            &out,
                            spec.clone(),
                            answer,
                            made,
                            cards.clone(),
                            Some((control.task_id.clone(), answer.clone())),
                            emit,
                        )
                        .await;
                }
                if let Some((answer, cards, made)) = told {
                    // ORCH-579: the stop is told once, here, under the owner's message; ORCH-596: the planner's own
                    // answer to the rest of the message follows it, recorded as its own words
                    let rest = if ferb_orchestrator::py::strip(&out.text).is_empty() {
                        String::new()
                    } else {
                        ferb_orchestrator::py::strip(&ferb_policy::confirm::reply_lines(
                            &self.redact_text(&crate::lines::without_history_labels(&out.text)),
                        ))
                        .to_string()
                    };
                    let text = if !rest.is_empty() && rest != ferb_orchestrator::py::strip(&answer)
                    {
                        format!("{answer}\n\n{rest}")
                    } else {
                        answer.clone()
                    };
                    let mut written = made;
                    if !rest.is_empty() {
                        written.insert(origin::INFERRED.into());
                        written.extend(turn.lock().unwrap().turn_origins.clone());
                    }
                    let mut opts = ExitAs::by(origin::INFERRED);
                    opts.ready = true;
                    opts.made = Some(written);
                    if !cards.is_empty() {
                        opts.extra = Some(obj(json!({"cards": cards})));
                    }
                    opts.told = Some((control.task_id.clone(), answer));
                    return self.exit_as(turn, &out, "reply", &text, opts, emit).await;
                }
                let text = if !settled {
                    if then.is_none() {
                        STOP_SETTLING
                    } else {
                        STOP_SETTLING_THEN
                    }
                } else if was_running {
                    ferb_orchestrator::answer_marks::STOPPED
                } else if finished {
                    ALREADY_FINISHED
                } else {
                    ALREADY_ENDED
                };
                if let Some(spec) = then {
                    return self
                        .then_start(turn, &out, spec, text, &ferb, Vec::new(), None, emit)
                        .await;
                }
                return self
                    .exit_as(turn, &out, "reply", text, ExitAs::by(origin::FERB), emit)
                    .await;
            }
            if !settled {
                // a retry still waits for the stop it needs
                self.record_reply(turn, "reply", RETRY_SETTLING, &ferb, "", None)
                    .await;
                let (session, rid) = {
                    let t = turn.lock().unwrap();
                    (t.session.clone(), t.request_id.clone())
                };
                self.release(&session, &rid);
                let _ = emit.send(json!({"type": "reply", "text": RETRY_SETTLING}));
                self.settled(&control.task_id, &ids, None).await;
            }
        }
        self.resume_ended(turn, &out, &conversation, emit).await
    }

    /// supervisor.py:3085 `_resume`: a continue or retry of an ended task carries on in place (`resume_id`), so its
    /// completed work, uncertain effects (P19's repeat check is per tree), confirmations, history and sites stay the
    /// same records (L11): a continue with a new goal or a changed identity takes the planner's spec (the saved plan
    /// reopens only when unchanged, a new plan replaces it); any other continue, or a retry without a task, resumes the
    /// saved goal and plan; a retry with the planner's task is the fresh attempt with the earlier goal and result.
    async fn resume_ended(
        &self,
        turn: &Arc<Mutex<Turn>>,
        out: &SupervisorOutcome,
        conversation: &str,
        emit: &Emit,
    ) -> Flow<()> {
        let control = out.control.clone().expect("a control");
        // a forget's scrub queued as that task finished runs first (fix wave 6a)
        self.deps.daemon.settle().await;
        let prior = self.continuity(|c| c.load(&control.task_id, conversation))?;
        let Some(prior) = prior else {
            return self
                .exit_as(
                    turn,
                    out,
                    "reply",
                    UNKNOWN_TASK,
                    ExitAs::by(origin::FERB),
                    emit,
                )
                .await;
        };
        let prior_spec = prior.spec.clone();
        let tid = control.task_id.clone();
        let identity_changed = out
            .task
            .as_ref()
            .is_some_and(|t| t.identity != prior_spec.identity);
        let for_model = turn.lock().unwrap().for_model.clone();
        let mut resumed: TaskSpec;
        if ((control.action == "continue" && control.new_goal) || identity_changed)
            && out.task.is_some()
        {
            resumed = out.task.clone().expect("a task");
            resumed.goal.push_str(&format!(
                "\nEarlier goal of this task: {}\nLast recorded result (verify afresh): {}",
                prior_spec.goal, prior.state
            ));
            match (&prior_spec.plan, &resumed.plan) {
                (Some(old), Some(new)) if old == new => self.plans(|p| p.reopen(&tid))?,
                (Some(_), Some(new)) => self.plans(|p| p.replace(&tid, new))?,
                (Some(_), None) => self.plans(|p| p.set_status(&tid, "replaced"))?,
                _ => {}
            }
        } else if control.action == "continue"
            || out.task.is_none()
            || (prior_spec.plan.is_some()
                && out
                    .task
                    .as_ref()
                    .is_some_and(|t| t.plan.is_none() || t.plan == prior_spec.plan))
        {
            resumed = prior_spec.clone();
            if let Some(t) = &out.task {
                // the owner's folders and kept tabs for this go (HAND-201, 4)
                resumed.folders = t.folders.clone();
                resumed.keep_tabs_open = t.keep_tabs_open;
                // ORCH-619: earlier results the follow-up names join the ones the task already used
                let mut all = prior_spec.earlier_results.clone();
                for r in &t.earlier_results {
                    if !all.contains(r) {
                        all.push(r.clone());
                    }
                }
                resumed.earlier_results = all;
            }
            if prior_spec.plan.is_some() {
                self.plans(|p| p.reopen(&tid))?;
            } else {
                resumed.goal.push_str(&format!(
                    "\nLast recorded result (verify afresh): {}",
                    prior.state
                ));
            }
        } else {
            resumed = out.task.clone().expect("a task");
            resumed.goal.push_str(&format!(
                "\nOriginal request context: {}\nLast recorded result (verify afresh): {}\nOwner's follow-up: {for_model}",
                prior_spec.goal, prior.state
            ));
            if let (Some(_), Some(new)) = (&prior_spec.plan, &resumed.plan) {
                self.plans(|p| p.replace(&tid, new))?;
            }
        }
        // the earlier requests' answers to FERB's questions keep what those questions were written from (S4 b)
        let (rid, typed, payload, asked_outside) = {
            let t = turn.lock().unwrap();
            (
                t.request_id.clone(),
                t.typed.clone(),
                t.payload.clone(),
                t.asked_outside.clone(),
            )
        };
        let mut asked = self.deps.daemon.asked_origins(&tid);
        asked.extend(asked_outside);
        // supervisor.py:1503 `_targets`: the spec's targets, its paths normalized
        let targets = &resumed.targets;
        let paths: Vec<String> = targets.paths.iter().map(|p| self.normalize(p)).collect();
        let continued = Request {
            request_id: rid,
            words: format!("{}\n{typed}", prior.words),
            targets: [
                targets.apps.clone(),
                targets.sites.clone(),
                paths,
                targets.recipients.clone(),
            ],
            payload: format!("{}\n\nOwner follow-up:\n{payload}", prior.payload),
            asked_origins: asked,
        };
        self.deps.daemon.record_request(&continued);
        // the continuation carries the earlier request and what its attempts read (S3, MP2)
        let more = self.task_origins(&tid);
        let earlier = self.request_origins_of_request(&prior.request_id);
        let read = self.request_origins_of(&tid);
        {
            let mut t = turn.lock().unwrap();
            t.turn_origins.extend(more);
            t.turn_origins.extend(earlier);
            t.message_origin.extend(read);
        }
        self.reopen_noes(&tid, turn);
        let prior_task = self.continuity(|c| c.prior_of(&tid)).ok().flatten();
        self.start_with(
            turn,
            out,
            resumed,
            emit,
            Some(super::turn::Resumed {
                resume_id: Some(tid),
                prior_task,
                message: Some(continued.payload),
                conversation: Some(conversation.to_string()),
            }),
        )
        .await
    }

    /// supervisor.py:4111 `_request_origins`: the stored origins of the owner's chat message(s) for a request (its
    /// lead's and those read with it).
    fn request_origins_of_request(&self, request_id: &str) -> BTreeSet<String> {
        let mut out = BTreeSet::new();
        for field in ["request_id", "read_with"] {
            for e in self.of_kind("chat.message", field, request_id, None) {
                out.extend(origin::split(Some(&e.origin)));
            }
        }
        out
    }

    /// supervisor.py:2979 `_then_start` (ORCH-638): a stop or pause, then the message's new task, in one turn; code's
    /// word on the stop or pause is recorded as this turn's reply and said first in the new task's plan event.
    #[allow(clippy::too_many_arguments)] // `_then_start`'s own arguments
    async fn then_start(
        &self,
        turn: &Arc<Mutex<Turn>>,
        out: &SupervisorOutcome,
        spec: TaskSpec,
        said: &str,
        made: &BTreeSet<String>,
        cards: Vec<Value>,
        told: Option<(String, String)>,
        emit: &Emit,
    ) -> Flow<()> {
        let told_ref = told.as_ref().map(|(a, b)| (a.as_str(), b.as_str()));
        self.record_reply(turn, "reply", said, made, "", told_ref)
            .await;
        {
            let mut t = turn.lock().unwrap();
            let first: Vec<String> = [t.said_first.clone(), said.to_string()]
                .into_iter()
                .filter(|p| !p.is_empty())
                .collect();
            t.said_first = first.join("\n\n");
            t.said_first_cards.extend(cards);
        }
        self.start(turn, out, spec, emit).await
    }

    /// supervisor.py:1371 `_reopen_noes`: the owner's own new message carrying a task on reopens each No recorded in
    /// its tree before it, once per turn and tree (decided from provenance, never wording).
    fn reopen_noes(&self, tree_id: &str, turn: &Arc<Mutex<Turn>>) {
        let mut t = turn.lock().unwrap();
        let entry = t
            .entries
            .iter()
            .find(|e| origin::split(Some(&e.origin)).contains(origin::OWNER))
            .cloned();
        let Some(entry) = entry else { return };
        if ferb_orchestrator::py::strip(&t.own_words).is_empty() || t.reopened.contains(tree_id) {
            return;
        }
        t.reopened.insert(tree_id.to_string());
        let rid = t.request_id.clone();
        drop(t);
        self.deps
            .daemon
            .reopen_noes(tree_id, &rid, &format!("journal:{}", entry.id));
    }

    /// supervisor.py:3054 `_kept_unchanged`: what a correction asked of a running task that code can't apply
    /// mid-task (whose account it uses, its plan's steps, the next attempt's worker), said in the reply.
    fn kept_unchanged(&self, tree: &Tree, spec: Option<&TaskSpec>) -> Vec<String> {
        let Some(spec) = spec else { return Vec::new() };
        let current = tree.spec();
        let mut lines = Vec::new();
        // APP-777: the planner's "ferb" for a task that browses as the owner (FERB has no profile) is no change
        let as_owner = tree.lock().as_owner;
        if spec.identity != current.identity && !(as_owner && spec.identity == "ferb") {
            let who = if current.identity == "owner" {
                "you"
            } else {
                "FERB"
            };
            lines.push(KEPT_IDENTITY.replace("{who}", who));
        }
        if let (Some(a), Some(b)) = (&spec.plan, &current.plan) {
            if a.steps != b.steps {
                lines.push(KEPT_PLAN.to_string());
            }
        }
        if spec.worker_type != current.worker_type
            && self.deps.daemon.worker_types().contains(&spec.worker_type)
        {
            lines.push(
                NEXT_WORKER.replace("{worker}", &ferb_worker::errors::label(&spec.worker_type)),
            );
        }
        lines
    }

    /// supervisor.py:3069 `_steered_extras`: a correction's changes code applies to a running task besides its goal,
    /// conditions and targets: the next attempt's worker type, its folders, earlier results, kept tabs.
    fn steered_extras(&self, tree: &Tree, spec: Option<&TaskSpec>) {
        let Some(spec) = spec else { return };
        let types = self.deps.daemon.worker_types();
        let keep_tabs = {
            let mut st = tree.lock();
            let Some(current) = st.spec.as_mut() else {
                return;
            };
            if types.contains(&spec.worker_type) {
                current.worker_type = spec.worker_type.clone();
            }
            current.folders = spec.folders.clone();
            for r in &spec.earlier_results {
                if !current.earlier_results.contains(r) {
                    current.earlier_results.push(r.clone());
                }
            }
            let turn_on = spec.keep_tabs_open && !current.keep_tabs_open;
            if turn_on {
                current.keep_tabs_open = true;
            }
            turn_on
        };
        self.deps.daemon.task_folders(&tree.id, Some(&spec.folders));
        if keep_tabs {
            self.deps.daemon.keep_tabs_open(&tree.id);
        }
    }

    /// steering.py:221 `apply`: persist first, then steer every live member of the named tree. Ok(None): the tree no
    /// longer takes corrections; Ok(Some(text)): the reply (JUST_ENDED when it found its task ending).
    #[allow(clippy::too_many_arguments)] // steering.apply's own arguments
    async fn apply_steering(
        &self,
        tree: &Arc<Tree>,
        session_id: &str,
        turn: &Arc<Mutex<Turn>>,
        spec: Option<&TaskSpec>,
        standing: &StandingSends,
        supersedes: &[String],
        said: &str,
    ) -> Flow<Option<(String, Option<ferb_orchestrator::steering::Restated>)>> {
        let mut state = tree.steering.lock().await;
        if !tree.lock().accepting_updates {
            return Ok(None);
        }
        let current = self
            .live()
            .tree_of
            .get(&tree.id)
            .is_some_and(|t| Arc::ptr_eq(t, tree));
        if !tree.lock().stop.is_empty() || !current {
            return Ok(Some((JUST_ENDED.to_string(), None)));
        }
        let (request_id, words, payload, made, shown, turn_kinds, source_ref) = {
            let t = turn.lock().unwrap();
            let shown: BTreeSet<String> = t
                .fronts()
                .iter()
                .flat_map(|f| origin::split(Some(&f.origin)))
                .filter(|o| origin::is_external(o))
                .collect();
            (
                t.request_id.clone(),
                t.own_words.clone(),
                t.payload.clone(),
                t.message_origin.clone(),
                shown,
                origin::kinds(t.turn_origins.iter().map(String::as_str)),
                t.entries
                    .first()
                    .map(|e| format!("journal:{}", e.id))
                    .unwrap_or_default(),
            )
        };
        let clean = self.redact(&json!({"words": words, "payload": payload}));
        let clean_words = clean["words"].as_str().unwrap_or("").to_string();
        let clean_payload = clean["payload"].as_str().unwrap_or("").to_string();
        let prior = self.deps.daemon.request(&tree.worker.request_id);
        if let Some(spec) = spec {
            let dumped = dump_task_spec(spec).to_value();
            let redacted = self.redact(&dumped);
            let corrected = if redacted == dumped {
                spec.clone()
            } else {
                task_spec_validate(&ferb_context::Py::from_value(&redacted))
                    .map_err(|e| Failure::raised("ValidationError", e.to_string()))?
            };
            let mut st = tree.lock();
            if let Some(current) = st.spec.as_mut() {
                ferb_orchestrator::steering::apply_spec(current, &corrected);
            }
        }
        // `tree.worker.goal = tree.spec.goal`, `tree.worker.success_conditions = [...]`: the next attempt's worker,
        // its checker and every member made after this read them (`Tree::worker_task`)
        {
            let mut st = tree.lock();
            let spec = st.spec.clone().expect("a tree's spec");
            st.worker_goal = Some(spec.goal.clone());
            st.worker_conditions = Some(
                spec.success_conditions
                    .iter()
                    .map(|c| c.text.clone())
                    .collect(),
            );
        }
        let tree_spec = tree.spec();
        let (new_words, new_payload) = ferb_orchestrator::steering::corrected_request(
            &prior.words,
            &prior.payload,
            &clean_words,
            &clean_payload,
        );
        let mut asked = prior.asked_origins.clone();
        asked.extend(made.iter().filter(|o| origin::is_external(o)).cloned());
        let targets = &tree_spec.targets;
        let updated = Request {
            request_id: prior.request_id.clone(),
            words: new_words,
            // supervisor.py:1503 `_targets` (steering.py:254)
            targets: [
                targets.apps.clone(),
                targets.sites.clone(),
                targets
                    .paths
                    .iter()
                    .map(|p| {
                        ferb_fs::files::normalize_path(p, &self.deps.workspace, &self.deps.home)
                            .unwrap_or_else(|_| p.clone())
                    })
                    .collect(),
                targets.recipients.clone(),
            ],
            payload: new_payload,
            asked_origins: asked,
        };
        self.deps.daemon.record_request(&updated);
        // D30: the planner's decision about the tree's stated standing scope, made for every correction
        let restated = match standing {
            StandingSends::Unchanged => None,
            StandingSends::Scopes(revised) => self.restate(tree, revised),
        };
        let tree_spec = tree.spec();
        let prior_task = tree.lock().prior_task.clone();
        self.continuity(|c| {
            c.save(
                &tree.id,
                session_id,
                &tree_spec,
                &updated.words,
                prior_task.as_deref(),
                &updated.payload,
            )
        })?;
        let wrap = self.deps.wrap.clone();
        let wrap: ferb_orchestrator::ports::Wrap = Arc::new(move |t: &str| wrap(t));
        let message = ferb_orchestrator::steering::correction_message(
            &clean_payload,
            &dump_json_task_spec_without_plan(&tree_spec),
            &wrap,
        );
        let message = self.redact_text(&message);
        state.messages.push(message.clone());
        let revision = state.messages.len();
        // a sleeping loop reads the correction now (L31), once this turn's reply is recorded (`wake_deferred`)
        turn.lock().unwrap().wake_after.insert(tree.id.clone());
        if let Some(span) = self.task_span(&tree.id) {
            {
                let db = self.db();
                let _ = db.conn().execute(
                    "UPDATE tasks SET goal=? WHERE id IN (?,?)",
                    rusqlite::params![tree_spec.goal, tree.id, tree.worker.id],
                );
            }
            let mut origins = self.request_origins_of(&tree.id);
            origins.extend(made.iter().cloned());
            origins.extend(shown.iter().cloned());
            self.span_update_input(
                &span,
                &json!({"request": updated.payload, "authority_words": updated.words,
                        "request_origins": origins.iter().collect::<Vec<_>>(),
                        "asked_origins": updated.asked_origins.iter().collect::<Vec<_>>(),
                        "task_spec": dump_task_spec(&tree_spec).to_value(), "goal": tree_spec.goal}),
            );
        }
        // every task runs from its saved plan (a single task's is one step): a restart resumes the corrected task
        let saved = self.plans(|p| p.load(&tree.id))?;
        let message_origins: Vec<String> = self.request_origins_of(&tree.id).into_iter().collect();
        let request = ferb_orchestrator::steering::saved_request(
            saved.request.as_object().unwrap_or(&Map::new()),
            dump_task_spec(&tree_spec).to_value(),
            &updated.words,
            &updated.payload,
            message_origins,
        );
        match &tree_spec.plan {
            Some(plan) => self.plans(|p| p.create(&tree.id, plan, &request, Some(session_id)))?,
            None => self.plans(|p| {
                p.create_single(&tree.id, &tree_spec.goal, &request, Some(session_id), true)
            })?,
        }
        self.deps
            .daemon
            .record_named(&clean_words, &prior.request_id);
        let exposure = self.span_start(
            "context.exposure",
            Some(&tree.id),
            None,
            None,
            json!({"source_refs": [source_ref], "reference": "owner correction", "revision": revision}),
        );
        let mut written: BTreeSet<String> = made.clone();
        written.extend(shown);
        written.extend(turn_kinds);
        written.insert(origin::INFERRED.into());
        self.span_finish(
            &exposure,
            Some(json!({"text": message})),
            None,
            Some(&origin::join(written.iter().map(String::as_str))),
            None,
        );
        self.journal(
            "task.steered",
            json!({"task_id": tree.id, "session_id": session_id, "request_id": request_id,
                   "revision": revision, "source_ref": source_ref}),
            None,
        );
        let active: Vec<ferb_worker::runs::ActiveRun> = self
            .runs
            .worker_runs()
            .into_iter()
            .map(|(_, run)| run)
            .filter(|run| self.tree_id_of(&run.handle.task_id).as_deref() == Some(tree.id.as_str()))
            .collect();
        let worker = self.worker.get().expect("the worker").clone();
        let mut accepted = Vec::new();
        for run in &active {
            accepted.push(
                ferb_worker::deliver(
                    worker.host(),
                    &tree.id,
                    &mut state,
                    &run.handle.id,
                    &run.handle.task_id,
                    run.control.as_ref(),
                )
                .await,
            );
        }
        // ORCH-538: only now, with the correction in each live worker's input, are the task's open questions settled
        self.settle_questions(&prior.request_id, &clean_words, supersedes, Some(&tree.id));
        self.deps.daemon.publish(
            "task.steered",
            json!({"task_id": tree.id, "revision": revision, "goal": tree_spec.goal,
                   "name": ferb_orchestrator::names::shown_name(Some(&tree_spec.name), &tree_spec.goal)}),
        );
        drop(state);
        if self.live().loop_wakes.contains_key(&tree.id) {
            // the turn goes on after a suspension point of Python's (its side entries, the reply's record), so a
            // loop woken above reads the correction and starts its round first, as Python's event loop runs it
            tokio::task::yield_now().await;
        }
        let delivered = !active.is_empty() && accepted.iter().all(|a| *a);
        Ok(Some((
            acknowledgement(
                self.deps.policy.as_ref(),
                said,
                delivered,
                restated.as_ref().map(|r| r.text.as_str()),
            ),
            restated,
        )))
    }

    /// steering.py:92 `settle_questions` (L3, ORCH-538): every open safety check of the task is set aside; of its
    /// other questions only those the planner named are touched.
    pub fn settle_questions(
        &self,
        request_id: &str,
        words: &str,
        supersedes: &[String],
        tree_id: Option<&str>,
    ) {
        // every open question, the gateway's (a confirmation, a P19 retry) and the supervisor's own
        let daemon = &self.deps.daemon;
        for q in daemon.approvals() {
            let belongs = q.request_id == request_id
                || (tree_id.is_some()
                    && !q.task_id.is_empty()
                    && self.tree_of(&q.task_id).as_deref() == tree_id);
            if !belongs {
                continue;
            }
            if TASK_CHECKS.contains(&q.capability.as_str()) && q.kind == "owner_question" {
                daemon.supersede_question(&q.id);
            } else if !supersedes.contains(&q.id) {
                continue;
            } else if q.kind == "owner_question"
                && !FERB_CHECKS.contains(&q.capability.as_str())
                && !q.options_only
                && !ferb_orchestrator::py::strip(words).is_empty()
            {
                if daemon
                    .resolve_question(&q.id, true, None, Some(words))
                    .is_none()
                {
                    if let Some(own) = daemon.questions() {
                        own.answer(&q.id, words);
                    }
                }
            } else {
                daemon.supersede_question(&q.id);
            }
            let state = daemon
                .approval(&q.id)
                .map(|a| a.state.as_str().to_string())
                .unwrap_or_default();
            let event = json!({"approval_id": q.id, "state": state, "capability": q.capability});
            self.journal("approval.resolved", event.clone(), None);
            self.deps.daemon.publish("approval.resolved", event);
        }
    }

    /// supervisor.py:2755 `_exit` with all its keyword arguments: the one end of a turn that answers in chat (the
    /// turn's side entries applied once, the reply with code's lines recorded once with its origin, a question kept
    /// so the next message continues it, and the chat let go).
    pub async fn exit_as(
        &self,
        turn: &Arc<Mutex<Turn>>,
        out: &SupervisorOutcome,
        kind: &str,
        text: &str,
        opts: ExitAs,
        emit: &Emit,
    ) -> Flow<()> {
        let effects = if opts.side {
            self.side_entries_named(turn, out, &opts.named).await?
        } else {
            super::turn::Side::default()
        };
        for ev in &effects.events {
            let _ = emit.send(ev.clone());
        }
        let mut said = if opts.ready || opts.by == origin::FERB {
            text.to_string()
        } else {
            let redacted = self.redact_text(&crate::lines::without_history_labels(text));
            ferb_policy::confirm::reply_lines(&redacted)
        };
        {
            let mut t = turn.lock().unwrap();
            if !t.acknowledged.is_empty() {
                let sep = if t.acknowledged.contains('\n') {
                    "\n"
                } else {
                    " "
                };
                said = format!("{}{sep}{said}", t.acknowledged);
                t.acknowledged.clear();
            }
        }
        let mut code_lines = String::new();
        if !effects.lines.is_empty() {
            code_lines = super::turn::own_tagged(
                &self.redact_text(&effects.lines.join("\n")),
                &effects.lines,
                &effects.own,
            );
            said = format!("{}\n{code_lines}", said.trim_end());
        }
        let mut extra = opts.extra.clone();
        if !effects.cards.is_empty() {
            // step 8e: the app draws these in place of their lines
            let mut e = extra.unwrap_or_default();
            let mut cards: Vec<Value> = e
                .get("cards")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            cards.extend(self.said_cards(&effects.cards, true));
            e.insert("cards".into(), Value::Array(cards));
            extra = Some(e);
        }
        let written = match opts.made {
            Some(m) if !m.is_empty() => m,
            _ => {
                if opts.by == origin::FERB && effects.lines.is_empty() {
                    origin_set(&[origin::FERB])
                } else {
                    let mut w = turn.lock().unwrap().turn_origins.clone();
                    w.insert(origin::INFERRED.into());
                    w
                }
            }
        };
        let told = opts.told.as_ref().map(|(a, b)| (a.as_str(), b.as_str()));
        self.record_reply(turn, kind, &said, &written, &code_lines, told)
            .await;
        let (session, rid, typed, payload) = {
            let t = turn.lock().unwrap();
            (
                t.session.clone(),
                t.request_id.clone(),
                t.typed.clone(),
                t.payload.clone(),
            )
        };
        if kind == "clarify" {
            self.continuity(|c| c.save_pending(&session, &typed, text, &payload))
                .map_err(Failure::from)?;
        }
        self.release(&session, &rid);
        let mut event = obj(json!({"type": kind, "text": said}));
        if let Some(extra) = extra {
            event.extend(extra);
        }
        let _ = emit.send(Value::Object(event));
        self.wake_deferred(turn);
        Ok(())
    }

    /// This turn's reply is recorded: the rounds it handed over go on, and a correction's loops not handed over (a
    /// path that ended early) are woken now.
    pub(crate) fn wake_deferred(&self, turn: &Arc<Mutex<Turn>>) {
        let (trees, held) = {
            let mut t = turn.lock().unwrap();
            (
                std::mem::take(&mut t.wake_after),
                std::mem::take(&mut t.held),
            )
        };
        for (tree_id, hold) in held {
            self.release_hold(&tree_id, &hold);
        }
        for tree_id in trees {
            self.wake_loops(&tree_id);
        }
    }
}

fn py_type(v: &Value) -> &'static str {
    match v {
        Value::Bool(_) => "bool",
        Value::Number(n) if n.is_f64() => "float",
        Value::Number(_) => "int",
        Value::Object(_) => "dict",
        Value::Array(_) => "list",
        Value::String(_) => "str",
        Value::Null => "NoneType",
    }
}
