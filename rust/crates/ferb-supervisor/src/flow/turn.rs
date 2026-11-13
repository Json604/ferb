//! A chat message's way in: supervisor.py:2388 `send`, :2464 `_turn`, :2505 `_think`, :2515 `_failed`, :2537
//! `_intake`, :2575 `_take_in`, :1507 `_decide`, :1545 `_problems`, :3475 `_supervise`, :2703 `_route`, :2755 `_exit`,
//! :2790 `_record_reply`, :2819 `_start`, and the chat line that orders a chat's messages (ORCH-529).
//!
//! Python runs a turn's thinking as an asyncio task the chat line can cancel (a second message before the first
//! reply). Here the thinking is one future raced with the line's [`Cancel`]; every await point of it that Python's
//! cancellation would reach runs its `except` path itself before it gives up, so the records a cancelled turn leaves
//! (its `planner.prepare`, its `model.turn` with `CancelledError`) are Python's.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use ferb_context::assemble::ContextBlock;
use ferb_core::types::{Identity, Task, TaskRole};
use ferb_orchestrator::models::{SupervisorOutcome, TaskSpec};
use ferb_orchestrator::validate::supervisor_outcome_validate_json;
use ferb_policy::small::blocked;
use ferb_sanitize::origin;
use ferb_sanitize::untrusted::{mark, Marked};
use ferb_worker::{Call, CallHooks};
use serde_json::{json, Map, Value};
use tokio::sync::{oneshot, Notify};

use super::store::Entry;
use super::{Emit, Failure, Flow, Inner, Request, Start, Turn};
use crate::chat_line::{ChatLine, Decision, Role};
use crate::history::{
    conversation_blocks, latest_result, timeline, Sourced, Timeline, TimelineReads,
};
use crate::planner::{self, LatestReads, PlannerTurn};
use crate::seams::JournalRow;
use crate::words::*;
use crate::NotPorted;

/// A turn's cancellation (Python's `run.think.cancel()`): set once, seen by every await that checks it.
#[derive(Default)]
pub struct Cancel {
    flag: AtomicBool,
    notify: Notify,
}

impl Cancel {
    pub fn cancel(&self) {
        self.flag.store(true, Ordering::SeqCst);
        self.notify.notify_waiters();
    }

    pub fn is_set(&self) -> bool {
        self.flag.load(Ordering::SeqCst)
    }

    pub async fn wait(&self) {
        loop {
            let notified = self.notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self.is_set() {
                return;
            }
            notified.await;
        }
    }
}

/// One message on its way into a turn (`_Arrival`).
#[derive(Clone, Default)]
pub struct Arrival {
    pub request_id: String,
    pub text: String,
    pub input_paths: Vec<String>,
    pub hidden: Vec<String>,
    pub entry: Option<Entry>,
    pub marked: Option<Marked>,
    pub paths: Vec<String>,
    /// Its capture at send, once awaited (`_screen_at_send`); None while not awaited.
    pub seen: Option<super::Seen>,
    pub screen: bool,
}

/// One chat's messages in arrival order (`_ChatLine` with its arrivals and the running turn's state).
#[derive(Default)]
pub struct Line {
    pub chat: ChatLine,
    pub arrivals: HashMap<String, Arrival>,
    pub roles: HashMap<String, oneshot::Sender<Role>>,
    pub cancel: Option<Arc<Cancel>>,
    /// The running turn's `_Start`, read once however often it restarts.
    pub start: Option<Start>,
    pub answered: bool,
}

impl Line {
    fn decide(&mut self, decisions: Vec<Decision>) {
        for d in decisions {
            if let Some(tx) = self.roles.remove(&d.request_id) {
                let _ = tx.send(d.role);
            }
        }
    }
}

/// What a turn's planner call ended with.
struct Decided {
    out: SupervisorOutcome,
}

/// supervisor.py:437 `_Side`: what `_side_entries` did.
#[derive(Default)]
pub(crate) struct Side {
    pub(crate) events: Vec<Value>,
    pub(crate) lines: Vec<String>,
    /// The places in `lines` of lines FERB wrote whole (see [`own_tagged`]).
    pub(crate) own: Vec<usize>,
    pub(crate) refused: Vec<(String, String)>,
    pub(crate) cards: Vec<Value>,
}

/// Code's lines under a reply as the owner reads them: a line that reads like FERB's own check is marked as the
/// task's (`confirm.from_task_lines`, D33), except FERB's own heading of a line it wrote whole (`own`: a forget's
/// question, whose "FERB deletes: ..." is code's words, not the task's). `redacted` is `lines` joined and redacted;
/// when redaction changed how many lines there are, every line is tagged as before.
///
/// Fixed against Python (GATE-581's kind): supervisor.py `_exit` tagged every code line, so the forget question said
/// "(from the task) FERB deletes:", marking FERB's own words as the task's.
pub(crate) fn own_tagged(redacted: &str, lines: &[String], own: &[usize]) -> String {
    let shown: Vec<&str> = redacted.split('\n').collect();
    let counts: Vec<usize> = lines.iter().map(|l| l.split('\n').count()).collect();
    if counts.iter().sum::<usize>() != shown.len() {
        return ferb_policy::confirm::from_task_lines(redacted);
    }
    let mut out = Vec::with_capacity(shown.len());
    let mut at = 0;
    for (i, n) in counts.into_iter().enumerate() {
        for (k, line) in shown[at..at + n].iter().enumerate() {
            // the second line of a forget's question is code's heading of what goes ("FERB deletes: “…”"); the
            // memory's own words sit inside its quotes, never at the line's start
            if own.contains(&i) && k == 1 && line.starts_with("FERB deletes: ") {
                out.push(line.to_string());
            } else {
                out.push(ferb_policy::confirm::from_task(line));
            }
        }
        at += n;
    }
    out.join("\n")
}

/// supervisor.py:427 `_carries_more`: whether a planner turn carries anything besides an answer.
fn carries_more(out: &SupervisorOutcome) -> bool {
    out.control.is_some()
        || (out.kind == "task" && out.task.is_some())
        || !out.remember.is_empty()
        || !out.forget.is_empty()
        || !out.timers.is_empty()
        || !out.schedules.is_empty()
        || !out.schedule_changes.is_empty()
        || out.show_schedules
        || !out.reminder_cancels.is_empty()
        || !out.conversation_changes.is_empty()
}

fn obj(v: Value) -> Map<String, Value> {
    match v {
        Value::Object(m) => m,
        _ => Map::new(),
    }
}

fn unique(items: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for item in items {
        if !out.contains(&item) {
            out.push(item);
        }
    }
    out
}

fn labels(text: &str) -> BTreeSet<String> {
    origin::split(Some(text))
}

fn joined(set: &BTreeSet<String>) -> String {
    origin::join(set.iter().map(String::as_str))
}

fn kinds(set: &BTreeSet<String>) -> BTreeSet<String> {
    origin::kinds(set.iter().map(String::as_str))
}

/// carried.py's `Shown` from what a writer was shown (labels, text, stored values).
pub fn shown_records(
    shown: &[(BTreeSet<String>, String, Option<Value>)],
) -> Vec<ferb_policy::carried::Shown> {
    shown
        .iter()
        .map(|(l, text, values)| ferb_policy::carried::Shown {
            labels: l.clone(),
            text: text.clone(),
            stored: values.as_ref().and_then(Value::as_object).map(|m| {
                m.iter()
                    .map(|(k, v)| {
                        (
                            k.clone(),
                            v.as_array()
                                .map(|a| a.iter().map(ferb_py::value::py_str).collect())
                                .unwrap_or_default(),
                        )
                    })
                    .collect::<BTreeMap<String, Vec<String>>>()
            }),
            page: String::new(),
        })
        .collect()
}

pub(crate) struct Reads<'a>(pub(crate) &'a Inner);

impl TimelineReads for Reads<'_> {
    fn outcome_json(&self, task_id: &str) -> Option<String> {
        self.0
            .task_column(task_id, "outcome_json")
            .and_then(|v| v.as_str().map(str::to_string))
    }
    fn notice_sources(&self, notice_id: &str) -> Vec<String> {
        let db = self.0.db();
        ferb_orchestrator::attention::notice_sources(&db, notice_id).unwrap_or_default()
    }
}

struct Since<'a>(&'a Inner);

impl planner::SinceReads for Since<'_> {
    fn task_column(&self, task_id: &str, column: &str) -> Option<String> {
        self.0.task_column(task_id, column).map(|v| {
            v.as_str()
                .map(str::to_string)
                .unwrap_or_else(|| ferb_py::value::py_str(&v))
        })
    }
    fn tree_of(&self, task_id: &str) -> Option<String> {
        self.0.tree_of(task_id)
    }
    fn task_origins(&self, root: &str) -> BTreeSet<String> {
        self.0.task_origins(root)
    }
    fn schedule_label(&self, schedule_id: &str) -> String {
        self.0.deps.daemon.schedule_label(schedule_id)
    }
    fn question_asked(&self, approval_id: &str) -> Option<JournalRow> {
        self.0
            .of_kind("question.asked", "approval_id", approval_id, None)
            .first()
            .map(Entry::row)
    }
}

impl Inner {
    pub fn me(&self) -> Arc<Inner> {
        self.me.upgrade().expect("the supervisor")
    }

    /// `tasks.tree_of(task_id)`: the live record's tree, else the row's.
    pub fn tree_of(&self, task_id: &str) -> Option<String> {
        if let Some((tree, _)) = self.live().records.get(task_id) {
            return Some(tree.clone());
        }
        self.task_column(task_id, "tree_id")
            .and_then(|v| v.as_str().map(str::to_string))
    }

    /// supervisor.py:2388 `send`: one owner message in this chat; its events on `emit` (`merged` alone when another
    /// message's turn reads it). `screen`: a front window was captured at send (L12).
    #[allow(clippy::too_many_arguments)] // send's own arguments
    pub async fn send(
        &self,
        session_id: &str,
        text: &str,
        input_paths: Vec<String>,
        request_id: String,
        hidden: Vec<String>,
        screen: bool,
        emit: &Emit,
    ) -> Flow<()> {
        let (tx, rx) = oneshot::channel();
        {
            let mut live = self.live();
            let line = live.lines.entry(session_id.to_string()).or_default();
            line.arrivals.insert(
                request_id.clone(),
                Arrival {
                    request_id: request_id.clone(),
                    text: text.to_string(),
                    input_paths,
                    hidden,
                    screen,
                    ..Arrival::default()
                },
            );
            line.roles.insert(request_id.clone(), tx);
            let arrived = line.chat.arrive(&request_id);
            if arrived.cancel_think {
                if let Some(c) = &line.cancel {
                    c.cancel();
                }
            }
            line.decide(arrived.decisions);
        }
        let role = match rx.await {
            Ok(r) => r,
            Err(_) => {
                if let Some(l) = self.live().lines.get_mut(session_id) {
                    l.chat.withdraw(&request_id);
                }
                return Err(Failure::Cancelled);
            }
        };
        if let Role::Merged(into) = role {
            let _ = emit.send(json!({"type": "merged", "into": into}));
            return Ok(());
        }
        let out = self.turn(session_id, &request_id, emit).await;
        self.release(session_id, &request_id);
        out
    }

    /// The chat's line lets the next message in (`line.release(run)`, `turn.release()`).
    pub fn release(&self, session_id: &str, lead: &str) {
        let mut live = self.live();
        if let Some(line) = live.lines.get_mut(session_id) {
            let decisions = line.chat.release(lead);
            if line.chat.holder.is_none() {
                line.start = None;
                line.answered = false;
                line.cancel = None;
            }
            line.decide(decisions);
        }
    }

    /// supervisor.py:2464 `_turn`.
    async fn turn(&self, session_id: &str, lead: &str, emit: &Emit) -> Flow<()> {
        let decided: Flow<Decided> = loop {
            let cancel = Arc::new(Cancel::default());
            {
                let mut live = self.live();
                let line = live.lines.get_mut(session_id).expect("the chat's line");
                if let Some(run) = line.chat.holder.as_mut() {
                    run.restart = false;
                }
                line.cancel = Some(cancel.clone());
            }
            let r = self.think(session_id, lead, emit, &cancel).await;
            let (restart, arrivals) = {
                let live = self.live();
                let run = live
                    .lines
                    .get(session_id)
                    .and_then(|l| l.chat.holder.clone());
                (
                    run.as_ref().is_some_and(|r| r.restart),
                    run.map(|r| r.arrivals).unwrap_or_default(),
                )
            };
            if !restart {
                break r;
            }
            self.journal(
                "chat.restarted",
                json!({"session_id": session_id, "request_id": lead, "messages": arrivals}),
                None,
            );
        };
        {
            let mut live = self.live();
            if let Some(line) = live.lines.get_mut(session_id) {
                line.chat.replied();
            }
        }
        let turn = self.turn_of(lead).unwrap_or_else(|| {
            let t = Arc::new(Mutex::new(Turn {
                request_id: lead.to_string(),
                session: session_id.to_string(),
                ..Turn::default()
            }));
            self.live().turns.insert(lead.to_string(), t.clone());
            t
        });
        let result = match decided {
            Err(failure) => self.failed(&turn, &failure, emit).await,
            Ok(d) => {
                // the running summary's fold, in the background (fold.rs)
                self.start_keep_recent(session_id);
                self.route(&turn, d.out, emit).await
            }
        };
        self.live().turns.remove(lead);
        result
    }

    pub fn turn_of(&self, request_id: &str) -> Option<Arc<Mutex<Turn>>> {
        self.live().turns.get(request_id).cloned()
    }

    /// supervisor.py:2505 `_think`.
    async fn think(
        &self,
        session_id: &str,
        lead: &str,
        emit: &Emit,
        cancel: &Cancel,
    ) -> Flow<Decided> {
        let turn = self.intake(session_id, lead, cancel).await?;
        let out = self.decide(&turn, emit, cancel).await?;
        self.kept_question(session_id, &turn);
        turn.lock().unwrap_or_else(|e| e.into_inner()).decided = Some(self.monotonic());
        Ok(Decided { out })
    }

    /// supervisor.py:2515 `_failed`.
    async fn failed(&self, turn: &Arc<Mutex<Turn>>, failure: &Failure, emit: &Emit) -> Flow<()> {
        let request_id = turn.lock().unwrap().request_id.clone();
        self.journal(
            "chat.failed",
            json!({"request_id": request_id, "why": failure.class(), "detail": failure.message()}),
            None,
        );
        let mut extra = Map::new();
        let text = match failure {
            Failure::Worker(ferb_worker::Error::Stalled { .. }) => SILENT.replace(
                "{minutes}",
                &ferb_py::value::py_str(&json!(self.deps.stall_s / 60.0)),
            ),
            Failure::Worker(e @ ferb_worker::Error::Service(s))
                if s.account == "usage_limit" || s.account == "signed_out" =>
            {
                let _ = e;
                let runtime = s.runtime.clone();
                let choices: Vec<String> = self
                    .deps
                    .daemon
                    .worker_types()
                    .into_iter()
                    .filter(|n| n != "default" && *n != runtime)
                    .collect();
                extra.insert(
                    "runtime_switch".into(),
                    json!({"runtime": runtime, "problem": s.account, "role": "orchestrator", "choices": choices}),
                );
                ferb_worker::errors::account_problem(&runtime, &s.account)
            }
            other => {
                let e = match other {
                    Failure::Worker(e) => e.clone(),
                    f => ferb_worker::Error::other(&f.class(), f.message()),
                };
                let route = || self.deps.daemon.has_route();
                ferb_worker::errors::couldnt(
                    "start that",
                    &e,
                    Some(&|t| self.redact_text(t)),
                    &route,
                )
            }
        };
        let text = self.redact_text(&text);
        self.record_reply(turn, "reply", &text, &origin_set(&[origin::FERB]), "", None)
            .await;
        let (session, lead) = {
            let t = turn.lock().unwrap();
            (t.session.clone(), t.request_id.clone())
        };
        self.release(&session, &lead);
        let mut ev = obj(json!({"type": "reply", "text": text, "status": "failed"}));
        ev.extend(extra);
        let _ = emit.send(Value::Object(ev));
        Ok(())
    }

    /// supervisor.py:2537 `_intake`.
    async fn intake(
        &self,
        session_id: &str,
        lead: &str,
        cancel: &Cancel,
    ) -> Flow<Arc<Mutex<Turn>>> {
        let has_start = self
            .live()
            .lines
            .get(session_id)
            .is_some_and(|l| l.start.is_some());
        if !has_start {
            tokio::select! {
                _ = self.deps.daemon.settle() => {}
                _ = cancel.wait() => return Err(Failure::Cancelled),
            }
            let (pending, asked) = self
                .continuity(|c| c.pending(session_id))
                .map_err(Failure::from)?;
            let text = |v: &Value| v.as_str().unwrap_or("").to_string();
            let (pending, asked) = (text(&pending), text(&asked));
            let payload = text(
                &self
                    .continuity(|c| c.pending_payload(session_id))
                    .map_err(Failure::from)?,
            );
            let start = Start {
                asked_outside: if asked.is_empty() {
                    BTreeSet::new()
                } else {
                    self.clarify_outside(session_id)
                },
                carried: if payload.is_empty() {
                    Vec::new()
                } else {
                    self.carried(session_id)
                },
                paths: if payload.is_empty() {
                    Vec::new()
                } else {
                    self.pending_input_paths(session_id)
                },
                pending,
                asked: asked.clone(),
                payload,
                focus: self.live().answer_focus.remove(session_id),
            };
            if let Some(line) = self.live().lines.get_mut(session_id) {
                line.start = Some(start);
            }
            if asked.is_empty() {
                self.continuity(|c| c.save_pending(session_id, "", "", ""))
                    .map_err(Failure::from)?;
            }
        }
        let arrivals: Vec<String> = self
            .live()
            .lines
            .get(session_id)
            .and_then(|l| l.chat.holder.as_ref().map(|h| h.arrivals.clone()))
            .unwrap_or_default();
        for rid in &arrivals {
            let pending = {
                let live = self.live();
                live.lines[session_id].arrivals.get(rid).cloned()
            };
            let Some(mut a) = pending else { continue };
            if a.entry.is_some() {
                continue;
            }
            let m = mark(&a.text);
            a.paths = unique(a.input_paths.iter().map(|p| self.normalize(p)));
            let mut data = obj(json!({
                "session_id": session_id, "request_id": a.request_id, "text": m.for_model,
                "untrusted_parts": m.untrusted.len() + usize::from(!a.paths.is_empty()), "input_paths": a.paths}));
            if *rid != lead {
                data.insert("read_with".into(), json!(lead));
            }
            a.entry = Some(self.journal("chat.message", Value::Object(data), None));
            a.marked = Some(m);
            self.live()
                .lines
                .get_mut(session_id)
                .unwrap()
                .arrivals
                .insert(rid.clone(), a);
        }
        let mut prep = Prepare::start(self, lead, false);
        match self
            .take_in(session_id, lead, &arrivals, &mut prep, cancel)
            .await
        {
            Ok(t) => {
                self.live().turns.insert(lead.to_string(), t.clone());
                t.lock().unwrap().prepare = Some(prep);
                Ok(t)
            }
            Err(e) => {
                prep.finish(self, Some(&e));
                Err(e)
            }
        }
    }

    /// supervisor.py:2575 `_take_in`.
    async fn take_in(
        &self,
        session_id: &str,
        lead: &str,
        arrivals: &[String],
        prep: &mut Prepare,
        cancel: &Cancel,
    ) -> Flow<Arc<Mutex<Turn>>> {
        let (start, list): (Start, Vec<Arrival>) = {
            let live = self.live();
            let line = &live.lines[session_id];
            (
                line.start.clone().unwrap_or_default(),
                arrivals
                    .iter()
                    .filter_map(|r| line.arrivals.get(r).cloned())
                    .collect(),
            )
        };
        let intake: Vec<crate::intake::Arrival> = list
            .iter()
            .map(|a| crate::intake::Arrival {
                marked: a.marked.clone().expect("a marked message"),
                paths: a.paths.clone(),
                attached: !a.input_paths.is_empty(),
                hidden: a.hidden.clone(),
            })
            .collect();
        let text = crate::intake::take_in(
            &intake,
            &crate::intake::Start {
                pending: start.pending.clone(),
                asked: start.asked.clone(),
                payload: start.payload.clone(),
                paths: start.paths.clone(),
            },
            &|t| self.wrap(t),
        );
        let joins = !start.pending.is_empty() && start.asked.is_empty();
        let mut made: BTreeSet<String> = list
            .iter()
            .filter_map(|a| a.entry.as_ref())
            .flat_map(|e| labels(&e.origin))
            .collect();
        if joins {
            made.extend(start.carried.iter().flat_map(|e| labels(&e.origin)));
        }
        prep.mark(self, "messages");
        let environment = tokio::select! {
            env = self.deps.daemon.environment(&text.words) => env,
            _ = cancel.wait() => return Err(Failure::Cancelled),
        };
        prep.mark(self, "environment");
        let waiting: Vec<String> = list
            .iter()
            .filter(|a| a.screen && a.seen.is_none())
            .map(|a| a.request_id.clone())
            .collect();
        if !waiting.is_empty() {
            // each message's own capture, waited for together (DMN-520)
            let seens = tokio::select! {
                s = futures_util::future::join_all(waiting.iter().map(|rid| self.screen_at_send(rid))) => s,
                _ = cancel.wait() => return Err(Failure::Cancelled),
            };
            for (rid, seen) in waiting.iter().zip(seens) {
                if let Some(line) = self.live().lines.get_mut(session_id) {
                    if let Some(a) = line.arrivals.get_mut(rid) {
                        a.seen = Some(seen);
                    }
                }
            }
            prep.mark(self, "front_window");
        }
        // (each arrival with its capture, in the order the messages came)
        let captured: Vec<(Arrival, super::Seen)> = {
            let live = self.live();
            let line = &live.lines[session_id];
            arrivals
                .iter()
                .filter_map(|r| line.arrivals.get(r))
                .filter_map(|a| a.seen.clone().map(|s| (a.clone(), s)))
                .collect()
        };
        let screens = if captured.len() > 1 {
            by_window(&captured)
        } else {
            Vec::new()
        };
        let seen = captured.last().map(|(_, s)| s.clone());
        let turn = Turn {
            request_id: lead.to_string(),
            session: session_id.to_string(),
            words: text.words,
            payload: text.payload,
            own_words: text.own_words,
            typed: text.typed,
            typed_lines: text.typed_lines,
            for_model: text.for_model,
            untrusted: text.untrusted,
            entries: list.iter().filter_map(|a| a.entry.clone()).collect(),
            carried: if joins {
                start.carried.clone()
            } else {
                Vec::new()
            },
            clarified: start.asked.clone(),
            focus: start.focus.clone(),
            open: if start.asked.is_empty() {
                None
            } else {
                Some(start.clone())
            },
            input_paths: text.input_paths,
            attached: text.attached,
            message_origin: made,
            environment,
            screen: seen.as_ref().and_then(super::Seen::front_seen),
            seen,
            screens,
            ..Turn::default()
        };
        Ok(Arc::new(Mutex::new(turn)))
    }

    pub(crate) fn normalize(&self, path: &str) -> String {
        ferb_fs::files::normalize_path(path, &self.deps.workspace, &self.deps.home)
            .unwrap_or_else(|_| path.to_string())
    }

    /// supervisor.py:4116 `_clarify_outside`.
    fn clarify_outside(&self, session_id: &str) -> BTreeSet<String> {
        let asked = self.of_kind("chat.clarify", "session_id", session_id, None);
        asked
            .last()
            .map(|e| {
                labels(&e.origin)
                    .into_iter()
                    .filter(|o| origin::is_external(o))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// supervisor.py:4122 `_carried`.
    fn carried(&self, session_id: &str) -> Vec<Entry> {
        let asked = self.of_kind("chat.clarify", "session_id", session_id, None);
        let rows = self.of_kind("question.answered", "", "", None);
        let Some(last) = asked.last() else {
            return Vec::new();
        };
        let answered_ids: BTreeSet<String> = rows.iter().map(|e| e.text("request_id")).collect();
        if answered_ids.contains(&last.text("request_id")) {
            return Vec::new();
        }
        let messages = self.of_kind("chat.message", "session_id", session_id, Some("owner"));
        let answered: HashMap<String, String> = rows
            .iter()
            .filter(|e| e.get("by").is_some_and(ferb_py::value::truthy))
            .map(|e| (e.text("by"), e.text("request_id")))
            .collect();
        let kept: BTreeSet<String> = self
            .of_kind("question.kept", "", "", None)
            .iter()
            .map(|e| e.text("by"))
            .collect();
        let clarified: BTreeSet<String> = self
            .of_kind("chat.clarify", "", "", None)
            .iter()
            .map(|e| e.text("request_id"))
            .collect();
        let lead: Vec<String> = messages
            .iter()
            .map(|e| match e.get("read_with") {
                Some(v) if ferb_py::value::truthy(v) => ferb_py::value::py_str(v),
                _ => e.text("request_id"),
            })
            .collect();
        let mut carried: Vec<Entry> = Vec::new();
        let mut request = last.text("request_id");
        let mut seen = BTreeSet::new();
        while !request.is_empty() && !seen.contains(&request) {
            seen.insert(request.clone());
            let mine: Vec<Entry> = messages
                .iter()
                .zip(&lead)
                .filter(|(_, r)| **r == request)
                .map(|(e, _)| e.clone())
                .collect();
            let none = mine.is_empty();
            carried = mine.into_iter().chain(carried).collect();
            if let Some(next) = answered.get(&request) {
                request = next.clone();
            } else if kept.contains(&request) || none {
                break;
            } else {
                let at = lead.iter().position(|r| *r == request).unwrap_or(0);
                let before = if at > 0 {
                    lead[at - 1].clone()
                } else {
                    String::new()
                };
                request = if clarified.contains(&before) {
                    before
                } else {
                    String::new()
                };
            }
        }
        carried
    }

    /// supervisor.py:3264 `_pending_input_paths`.
    fn pending_input_paths(&self, session_id: &str) -> Vec<String> {
        let asked = self.of_kind("chat.clarify", "session_id", session_id, None);
        let Some(last) = asked.last() else {
            return Vec::new();
        };
        let rid = last.text("request_id");
        let paths = |e: &Entry| -> Vec<String> {
            e.get("input_paths")
                .and_then(Value::as_array)
                .map(|a| a.iter().map(ferb_py::value::py_str).collect())
                .unwrap_or_default()
        };
        let mut all: Vec<String> = self.carried(session_id).iter().flat_map(paths).collect();
        for kind in ["request_id", "read_with"] {
            for e in self.of_kind("chat.message", kind, &rid, None) {
                all.extend(paths(&e));
            }
        }
        unique(all)
    }

    /// supervisor.py:2624 `_question_answered`.
    fn question_answered(&self, session_id: &str, by: &str) {
        let rows = self.of_kind("chat.clarify", "session_id", session_id, None);
        let (kind, rid) = match rows.last() {
            Some(r) => (
                match r.get("question_kind") {
                    Some(v) if ferb_py::value::truthy(v) => ferb_py::value::py_str(v),
                    _ => "clarify".into(),
                },
                r.text("request_id"),
            ),
            None => ("clarify".into(), String::new()),
        };
        self.journal(
            "question.answered",
            json!({"kind": kind, "request_id": rid, "answer": "words", "by": by}),
            None,
        );
    }

    /// supervisor.py:2634 `_join_question` (ORCH-586).
    fn join_question(&self, turn: &Arc<Mutex<Turn>>, out: &SupervisorOutcome) {
        let (start, session, rid) = {
            let t = turn.lock().unwrap();
            (t.open.clone(), t.session.clone(), t.request_id.clone())
        };
        let Some(start) = start else { return };
        if !out.answers_question {
            return;
        }
        {
            let mut t = turn.lock().unwrap();
            t.open = None;
            t.joined = true;
        }
        let asked = self
            .continuity(|c| c.pending(&session))
            .map(|(_, a)| a.as_str().unwrap_or("").to_string())
            .unwrap_or_default();
        if !asked.is_empty() {
            let _ = self.continuity(|c| c.save_pending(&session, "", "", ""));
            self.question_answered(&session, &rid);
        }
        let mut t = turn.lock().unwrap();
        if !start.pending.is_empty() {
            t.typed = format!("{}\n{}", start.pending, t.typed);
            t.typed_lines = format!("{}\n{}", start.pending, t.typed_lines);
        }
        let before = if start.payload.is_empty() {
            String::new()
        } else {
            format!("{}\n\n", start.payload)
        };
        t.payload = format!(
            "{before}{}\n{}{}",
            t.payload,
            ferb_sanitize::untrusted::EARLIER_QUESTION_DATA,
            self.wrap(&start.asked)
        );
        t.carried = start.carried.clone();
        t.input_paths = unique(start.paths.iter().cloned().chain(t.input_paths.clone()));
        t.asked_outside = start.asked_outside.clone();
        let mut made = t.message_origin.clone();
        made.insert(origin::INFERRED.into());
        made.extend(start.carried.iter().flat_map(|e| labels(&e.origin)));
        t.message_origin = made;
    }

    /// supervisor.py:2660 `_kept_question`.
    fn kept_question(&self, session_id: &str, turn: &Arc<Mutex<Turn>>) {
        let open = turn.lock().unwrap().open.is_some();
        let answered = self
            .live()
            .lines
            .get(session_id)
            .is_some_and(|l| l.answered);
        if answered || !open {
            return;
        }
        if let Some(l) = self.live().lines.get_mut(session_id) {
            l.answered = true;
        }
        let rows = self.of_kind("chat.clarify", "session_id", session_id, None);
        let by = turn.lock().unwrap().request_id.clone();
        self.journal(
            "question.kept",
            json!({"request_id": rows.last().map(|r| r.text("request_id")).unwrap_or_default(), "by": by}),
            None,
        );
    }

    /// supervisor.py:1507 `_decide`: the planner's choice, repaired once.
    pub(crate) async fn decide(
        &self,
        turn: &Arc<Mutex<Turn>>,
        emit: &Emit,
        cancel: &Cancel,
    ) -> Flow<SupervisorOutcome> {
        let rid = turn.lock().unwrap().request_id.clone();
        let mut out = match self.supervise(turn, None, Some(emit), "", cancel).await {
            Ok(out) => out,
            Err(Repairable::Not(f)) => return Err(f),
            Err(Repairable::Problems(problems, _)) => {
                self.journal(
                    "task.repair",
                    json!({"request_id": rid, "problems": problems}),
                    None,
                );
                self.supervise(turn, Some(&problems), Some(emit), "", cancel)
                    .await
                    .map_err(Repairable::failure)?
            }
        };
        self.join_question(turn, &out);
        let mut problems = self.problems(turn, &mut out, Some(emit))?;
        problems.extend(self.side_quote_problems(turn, &out).await?);
        if problems.is_empty() {
            return Ok(out);
        }
        self.journal(
            "task.repair",
            json!({"request_id": rid, "problems": problems}),
            None,
        );
        if out.kind == "task" {
            let _ = emit.send(
                json!({"type": "plan_abort", "reason": "The provisional plan needs correction."}),
            );
        }
        let mut out = self
            .supervise(turn, Some(&problems), Some(emit), "", cancel)
            .await
            .map_err(Repairable::failure)?;
        self.join_question(turn, &out);
        let remaining = self.problems(turn, &mut out, Some(emit))?;
        if !remaining.is_empty() {
            self.journal(
                "chat.failed",
                json!({"request_id": rid, "why": "task still invalid after repair", "problems": remaining}),
                None,
            );
            return Err(Failure::PlanRejected(remaining));
        }
        Ok(out)
    }

    /// supervisor.py:1545 `_problems`: what code finds wrong with the planner's choice. `say`: the turn's live line
    /// (`_turn_line`) while the plan's inputs are walked.
    fn problems(
        &self,
        turn: &Arc<Mutex<Turn>>,
        candidate: &mut SupervisorOutcome,
        say: Option<&Emit>,
    ) -> Flow<Vec<String>> {
        let mut problems = Vec::new();
        let session = turn.lock().unwrap().session.clone();
        if let Some(control) = &candidate.control {
            if self
                .control_conversation(&control.task_id, &session)
                .is_none()
            {
                problems.push(
                    "Unknown task_id: it isn't a running task or one of this chat's tasks.".into(),
                );
            }
        }
        let has_new = candidate.new_task().is_some();
        let controlled = candidate.control.is_some();
        if let Some(task) = candidate.task.as_mut() {
            let mut named = vec![task.worker_type.clone()];
            if let Some(plan) = &task.plan {
                named.extend(plan.steps.iter().map(|s| s.worker_type.clone()));
            }
            let types = self.deps.daemon.worker_types();
            let unknown: BTreeSet<String> =
                named.into_iter().filter(|n| !types.contains(n)).collect();
            if !unknown.is_empty() {
                let shown: Vec<String> = unknown
                    .iter()
                    .map(|u| ferb_orchestrator::py::repr_str(u))
                    .collect();
                problems.push(format!(
                    "worker_type {} isn't one of the worker types available now: {}.",
                    shown.join(", "),
                    types.join(", ")
                ));
            }
            let (folders, broad) = self.folders(&task.folders);
            task.folders = folders;
            if !broad.is_empty() {
                problems.push(format!(
                    "folders must each name one specific folder, never the whole disk, the home folder, /Volumes or \
                     iCloud Drive itself: {}",
                    ferb_py::json::dumps(&broad)
                ));
            }
            if !controlled || has_new || task.plan.is_some() {
                problems.extend(ferb_orchestrator::plans::part_problems(
                    &task.parts,
                    task.plan.as_ref(),
                    task.success_conditions.len(),
                ));
                let said = self.said_in_chat(turn);
                for part in &task.parts {
                    if !said
                        .iter()
                        .any(|w| ferb_learning::text::quoted(w, &part.quote, false))
                    {
                        problems.push(format!(
                            "part {}: its quote must be the owner's own words from this message or their recent \
                             messages in this chat: {}",
                            part.id,
                            ferb_py::json::dumps_utf8(&part.quote)
                        ));
                    }
                }
            }
        }
        // ORCH-313: what every run uses is checked as a task's is
        let types = self.deps.daemon.worker_types();
        for sched in candidate.schedules.iter_mut() {
            if !sched.worker_type.is_empty() && !types.contains(&sched.worker_type) {
                problems.push(format!(
                    "schedule worker_type {} isn't one of the worker types available now: {}.",
                    ferb_orchestrator::py::repr_str(&sched.worker_type),
                    types.join(", ")
                ));
            }
            let (folders, broad) = self.folders(&sched.folders);
            sched.folders = folders;
            if !broad.is_empty() {
                problems.push(format!(
                    "schedule folders must each name one specific folder, never the whole disk, the home folder, \
                     /Volumes or iCloud Drive itself: {}",
                    ferb_py::json::dumps(&broad)
                ));
            }
        }
        problems.extend(self.unbacked_parts(turn, candidate));
        if let Some(task) = &candidate.task {
            // ORCH-619: the earlier results it names are shown here and fit its workers
            problems.extend(self.result_problems(turn, task));
        }
        let input_paths = turn.lock().unwrap().input_paths.clone();
        if let Some(plan) = candidate.task.as_mut().and_then(|t| t.plan.as_mut()) {
            let paths = unique(
                plan.input_paths
                    .iter()
                    .chain(input_paths.iter())
                    .map(|p| self.normalize(p)),
            );
            plan.input_paths = paths.clone();
            let normal = |p: &str| self.normalize(p);
            // ORCH-625: only what the input checks need is walked; a plan whose steps list no inputs walks nothing
            let walk = ferb_orchestrator::plans::walked(plan, &paths, &normal);
            let closed: Vec<String> = unique(paths.iter().chain(walk.iter()).cloned())
                .into_iter()
                .filter(|p| self.deps.daemon.read_denied(p))
                .collect();
            if !closed.is_empty() {
                problems.push(format!(
                    "Inventory paths are protected: {}",
                    ferb_py::json::dumps(&closed)
                ));
            } else if !walk.is_empty() {
                let task = candidate.task.as_ref().expect("a task");
                // the worker model's window (L28)
                let available = self.worker_room(turn, &task.identity, &task.worker_type);
                let plan = candidate
                    .task
                    .as_mut()
                    .and_then(|t| t.plan.as_mut())
                    .expect("a plan");
                let inputs: Vec<String> = plan
                    .steps
                    .iter()
                    .flat_map(|s| s.inputs.iter().map(|v| self.normalize(v)))
                    .collect();
                let steps = plan.steps.iter().filter(|s| !s.inputs.is_empty()).count();
                if let Some(emit) = say {
                    // ORCH-625 (`_turn_line`): the owner sees what FERB is doing while it walks, a `progress` line
                    // marked `planning` (no task exists yet)
                    let text = ferb_orchestrator::plans::looking_at(&walk, &self.deps.home);
                    let _ = emit.send(json!({"type": "progress", "text": self.redact_text(&text),
                        "planning": true, "started_at": self.now_iso()}));
                }
                let denied = |file: &str| self.deps.daemon.read_denied(file);
                let measured = ferb_orchestrator::plans::inventory(
                    &walk,
                    Some(&denied),
                    &inputs,
                    steps,
                    Some(available.max(0) as u64),
                    &self.deps.home,
                );
                // (Python walks it off the loop, L23; the turn's own line said so above)
                if measured.stopped.is_some() {
                    problems.push(ferb_orchestrator::plans::too_big(&measured, &|p| {
                        p.to_string()
                    }));
                    return Ok(problems);
                }
                // each step's inputs in the inventory's form (ORCH-580): checked, stored and handed to its worker
                ferb_orchestrator::plans::resolve_inputs(plan, &measured.files, &normal);
                problems.extend(ferb_orchestrator::plans::problems(
                    plan,
                    &measured.files,
                    available.max(0) as u64,
                ));
                if !problems.is_empty() {
                    let files: Vec<Value> = measured.files.iter().map(|f| f.to_json()).collect();
                    problems.push(format!(
                        "Measured inventory: {}",
                        ferb_py::json::dumps(&Value::Array(files))
                    ));
                }
            }
        }
        Ok(problems)
    }

    /// supervisor.py:1620 `_folders`.
    fn folders(&self, folders: &[String]) -> (Vec<String>, Vec<String>) {
        let home = self.deps.home.trim_end_matches('/').to_string();
        let whole = [
            "/".to_string(),
            home.clone(),
            "/Volumes".to_string(),
            format!("{home}/Library/Mobile Documents"),
        ];
        let normal = unique(
            folders
                .iter()
                .filter(|f| !ferb_orchestrator::py::strip(f).is_empty())
                .map(|f| self.normalize(f)),
        );
        let broad = normal
            .iter()
            .filter(|f| whole.contains(&f.trim_end_matches('/').to_string()) || f.as_str() == "/")
            .cloned()
            .collect();
        (normal, broad)
    }

    /// supervisor.py:1439 `_side_quote_problems`: the reminders and timers (side.rs); any other side entry present
    /// is refused by name.
    async fn side_quote_problems(
        &self,
        turn: &Arc<Mutex<Turn>>,
        out: &SupervisorOutcome,
    ) -> Flow<Vec<String>> {
        let mut problems = self.memory_quote_problems(turn, out);
        problems.extend(self.timer_quote_problems(turn, out));
        let tell = |what: &str, quote: &str| {
            format!(
                "{what}: its quote isn't the owner's exact words from this message or their recent messages in this \
                 chat, so code would refuse it: {}",
                ferb_py::json::dumps_utf8(&json!(quote))
            )
        };
        for (i, sched) in out.schedules.iter().enumerate() {
            let (_, refused) = self.schedule_words(turn, sched);
            if refused == "quote" {
                problems.push(tell(&format!("schedules[{i}]"), &sched.quote));
            } else if refused == "until" {
                if let Some(u) = &sched.until {
                    problems.push(tell(&format!("schedules[{i}] until"), &u.quote));
                }
            }
        }
        let repr = |id: &str| ferb_orchestrator::py::repr_str(id);
        let mut quotes: Vec<(String, String)> = out
            .schedule_changes
            .iter()
            .map(|ch| {
                (
                    format!("schedule_changes {}", repr(&ch.id)),
                    ch.quote.clone(),
                )
            })
            .collect();
        quotes.extend(out.schedule_changes.iter().filter_map(|ch| {
            ch.until.as_ref().map(|u| {
                (
                    format!("schedule_changes {} until", repr(&ch.id)),
                    u.quote.clone(),
                )
            })
        }));
        quotes.extend(out.reminder_cancels.iter().map(|rc| {
            (
                format!("reminder_cancels {}", repr(&rc.id)),
                rc.quote.clone(),
            )
        }));
        quotes.extend(out.conversation_changes.iter().map(|c| {
            (
                format!("conversation_changes {}", repr(&c.id)),
                c.quote.clone(),
            )
        }));
        for (what, quote) in quotes {
            if self.owner_said(turn, &quote).is_none() {
                problems.push(tell(&what, &quote));
            }
        }
        if !problems.is_empty() {
            problems.push(
                "Quote the owner's exact words for each of these, or leave it out; your text says only what will \
                 actually happen, never that something was set, saved, changed or restored."
                    .into(),
            );
        }
        // T-1066: each new or edited schedule's timing works and its request names only times it runs at
        let entries: Vec<super::schedules_chat::Entry> = out
            .schedules
            .iter()
            .cloned()
            .map(|e| super::schedules_chat::Entry::New(Box::new(e)))
            .chain(
                out.schedule_changes
                    .iter()
                    .filter(|ch| ch.action == "edit")
                    .cloned()
                    .map(|e| super::schedules_chat::Entry::Change(Box::new(e))),
            )
            .collect();
        if !entries.is_empty() {
            let Some(port) = self.deps.daemon.schedules_chat() else {
                return Err(
                    NotPorted("Supervisor._side_quote_problems (no schedules port)").into(),
                );
            };
            for entry in entries {
                problems.extend(port.problems(entry).await);
            }
        }
        Ok(problems)
    }

    /// supervisor.py:1399 `_said_in_chat`: this turn's words, then this chat's recent owner messages, newest first.
    fn said_in_chat(&self, turn: &Arc<Mutex<Turn>>) -> Vec<String> {
        let (first, session, current) = {
            let t = turn.lock().unwrap();
            (
                if t.typed_lines.is_empty() {
                    t.typed.clone()
                } else {
                    t.typed_lines.clone()
                },
                t.session.clone(),
                t.request_ids(),
            )
        };
        let mut said: Vec<Option<String>> = Vec::new();
        for entry in self.of_kind("chat.message", "session_id", &session, Some("owner")) {
            if current.contains(&entry.request_id) {
                continue;
            }
            let held = labels(&entry.origin);
            said.push(
                if !held.contains(origin::OWNER)
                    || held.contains(&origin::external("unlabeled", ""))
                {
                    None
                } else {
                    Some(ferb_sanitize::untrusted::typed_lines_of(
                        &entry.text("text"),
                    ))
                },
            );
        }
        let recent: Vec<String> = said
            .iter()
            .rev()
            .take(RECENT_EXCHANGES)
            .flatten()
            .cloned()
            .collect();
        std::iter::once(first).chain(recent).collect()
    }

    /// supervisor.py:2843 `_control_conversation`.
    fn control_conversation(&self, task_id: &str, session_id: &str) -> Option<String> {
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
                .flatten();
            if let Some(other) = other {
                if self
                    .continuity(|c| c.load(task_id, &other))
                    .ok()
                    .flatten()
                    .is_some()
                {
                    return Some(other);
                }
            }
        }
        None
    }

    /// supervisor.py:3475 `_supervise`: one planner call.
    /// `_supervise(turn, repair)` with no reply stream (the intro's preview).
    pub(crate) async fn supervise_turn(
        &self,
        turn: &Arc<Mutex<Turn>>,
        repair: Option<&[String]>,
        cancel: &Cancel,
    ) -> Result<SupervisorOutcome, Repairable> {
        self.supervise(turn, repair, None, "", cancel).await
    }

    async fn supervise(
        &self,
        turn: &Arc<Mutex<Turn>>,
        repair: Option<&[String]>,
        emit: Option<&Emit>,
        last_call: &str,
        cancel: &Cancel,
    ) -> Result<SupervisorOutcome, Repairable> {
        let rid = turn.lock().unwrap().request_id.clone();
        let held = turn.lock().unwrap().prepare.take();
        let mut prep = held.unwrap_or_else(|| Prepare::start(self, &rid, repair.is_some()));
        let built = self.planner_blocks(turn, repair, last_call, &mut prep);
        let (rt, ctx, sourced, lookups) = match built {
            Ok(b) => b,
            Err(e) => {
                prep.finish(self, Some(&e));
                return Err(Repairable::Not(e));
            }
        };
        prep.finish(self, None);
        let mut input =
            obj(json!({"role": "supervisor", "request_id": rid, "repair": repair.is_some()}));
        if !last_call.is_empty() {
            input.insert("last_call".into(), json!(last_call));
        }
        let span = self.span_start("model.turn", None, None, None, Value::Object(input));
        let draft = emit.map(|emit| {
            let emit = emit.clone();
            Arc::new(Mutex::new(
                ferb_orchestrator::reply_stream::ReplyDraft::new(
                    self.deps.redactor.clone(),
                    self.deps.policy.clone(),
                    Arc::new(move |v: Value| {
                        let _ = emit.send(v);
                    }),
                    None,
                ),
            ))
        });
        let usage: Arc<Mutex<Map<String, Value>>> = Arc::default();
        let call_timing: Arc<Mutex<Map<String, Value>>> = Arc::default();
        let mut clocked = {
            let clock = self.deps.clock.clone();
            ferb_core::timing::Steps::new(Box::new(move || clock.monotonic()))
        };
        let (token, looker) = self.lookup_grant(turn, &span, &lookups);
        let system = self.system_for("supervisor.md", false, &[]);
        clocked.mark("ferb_before");
        let task = Task {
            id: rid.clone(),
            parent_id: None,
            goal: "chat turn".into(),
            device_id: self.deps.device_id.clone(),
            identity: Identity::Owner,
            scopes: BTreeSet::new(),
            role: TaskRole::Mixed,
            request_id: rid.clone(),
            success_conditions: Vec::new(),
            deadline: None,
        };
        let images = turn.lock().unwrap().planner_images();
        let hooks = CallHooks {
            on_delta: draft.clone().map(|d| {
                Arc::new(move |t: &str| d.lock().unwrap().feed(t)) as ferb_worker::worker::OnText
            }),
            on_usage: Some({
                let usage = usage.clone();
                Arc::new(move |m: Map<String, Value>| usage.lock().unwrap().extend(m))
            }),
            notify: None,
            on_tool: draft.clone().map(|d| {
                Arc::new(move || d.lock().unwrap().restart()) as Arc<dyn Fn() + Send + Sync>
            }),
            on_timing: Some({
                let t = call_timing.clone();
                Arc::new(move |m: Map<String, Value>| t.lock().unwrap().extend(m))
            }),
        };
        let ctx = crate::planner::with_newly_always_on(ctx, &self.deps.daemon.identity_pending());
        let worker = self.worker.get().expect("the worker").clone();
        let call = Call {
            task: &task,
            system,
            context: ctx.clone(),
            token: &token,
            parent_span: &span,
            tree: None,
            worker_id: "",
            images,
            hooks,
            retry_silent: false,
        };
        let ran = tokio::select! {
            r = worker.run_call(&rt, call) => Some(r),
            _ = cancel.wait() => None,
        };
        clocked.mark("model_call");
        let timing = |clocked: &ferb_core::timing::Steps| -> Value {
            let mut t = clocked.facts();
            let call = call_timing.lock().unwrap().clone();
            if !call.is_empty() {
                t.insert("call".into(), Value::Object(call));
            }
            Value::Object(t)
        };
        let failed = |e: &Failure| {
            if let Some(d) = &draft {
                d.lock().unwrap().abort();
            }
            e.clone()
        };
        let result: Result<(), Failure> = match ran {
            None => {
                // a cancelled call (a newer message restarts the turn): its timing as far as it got
                let mut call = call_timing.lock().unwrap();
                if call.is_empty() {
                    let mut steps = Map::new();
                    for step in ["prepare", "start", "stream", "close"] {
                        steps.insert(step.into(), json!(0));
                    }
                    call.insert("steps_ms".into(), Value::Object(steps));
                    call.insert("total_ms".into(), json!(0));
                    call.insert("model_calls".into(), json!(0));
                }
                drop(call);
                Err(failed(&Failure::Cancelled))
            }
            Some(Err(e)) => Err(failed(&Failure::Worker(e))),
            Some(Ok(final_)) => match supervisor_outcome_validate_json(&final_) {
                Ok(out) => {
                    if let Some(d) = &draft {
                        d.lock().unwrap().finish(&final_);
                    }
                    self.after_decision(turn, &ctx, &sourced);
                    self.lookups_read(turn);
                    clocked.mark("ferb_after");
                    let held = usage.lock().unwrap().clone();
                    let mut kept = Map::new();
                    for k in [
                        "input_tokens",
                        "cached_input_tokens",
                        "cache_write_tokens",
                        "output_tokens",
                        "reasoning_tokens",
                    ] {
                        if let Some(v) = held.get(k) {
                            kept.insert(k.into(), v.clone());
                        }
                    }
                    self.span_finish(
                        &span,
                        Some(json!({"text": final_, "usage": kept, "timing": timing(&clocked)})),
                        None,
                        None,
                        None,
                    );
                    self.lookups_end(&rid, &looker);
                    return Ok(out);
                }
                Err(e) => {
                    let class = repair_class(&e);
                    if let Some(d) = &draft {
                        d.lock().unwrap().abort();
                    }
                    clocked.mark("ferb_after");
                    self.span_finish(
                        &span,
                        Some(json!({"text": final_, "timing": timing(&clocked)})),
                        Some(&format!("{class}: {e}")),
                        None,
                        None,
                    );
                    self.lookups_read(turn);
                    self.lookups_end(&rid, &looker);
                    return Err(Repairable::Problems(vec![e.to_string()], class.to_string()));
                }
            },
        };
        let e = result.unwrap_err();
        let text = "";
        self.span_finish(
            &span,
            Some(json!({"text": text, "timing": timing(&clocked)})),
            Some(&format!("{}: {}", e.class(), e.message())),
            None,
            None,
        );
        self.lookups_read(turn);
        self.lookups_end(&rid, &looker);
        if let Failure::Worker(we) = &e {
            if matches!(we, ferb_worker::Error::Invalid(_)) {
                return Err(Repairable::Problems(vec![we.to_string()], we.class()));
            }
            let spent =
                !looker.is_empty() && self.deps.daemon.lookups_made(&looker) >= PLANNER_LOOKUPS;
            let missing = matches!(we, ferb_worker::Error::AnswerMissing(_));
            if last_call.is_empty() && (spent || missing) {
                // ORCH-557/558: one last call without tools, given what the lookups found
                let why = if spent {
                    "spent"
                } else if matches!(we, ferb_worker::Error::AnswerMissing(m)
                    if matches!(m.kind, ferb_worker::errors::MissingKind::ToolLoop { .. }))
                {
                    "tool_loop"
                } else {
                    "empty"
                };
                if why == "spent" {
                    self.journal("planner.lookups_spent", json!({"request_id": rid}), None);
                } else {
                    self.journal(
                        "planner.last_call",
                        json!({"request_id": rid, "why": why}),
                        None,
                    );
                }
                return Box::pin(self.supervise(turn, repair, emit, why, cancel)).await;
            }
            if matches!(we, ferb_worker::Error::Service(s) if matches!(s.kind, ferb_worker::errors::ServiceKind::Image))
                && !turn.lock().unwrap().planner_images().is_empty()
            {
                // ORCH-524, L12: a model that won't take the screenshot must not fail the message: once more without
                // it, saying so, with the provider's own words (the journal redacts what it stores)
                self.journal(
                    "screen.image_refused",
                    json!({"request_id": rid, "error": we.to_string()}),
                    None,
                );
                {
                    let mut t = turn.lock().unwrap();
                    let withhold = |s: &mut super::Seen| {
                        s.image = None;
                        s.withheld = Some(IMAGE_REFUSED.to_string());
                    };
                    if let Some(seen) = t.seen.as_mut() {
                        withhold(seen);
                    }
                    for seen in t.screens.iter_mut() {
                        withhold(seen);
                    }
                    if let Some(screen) = t.screen.as_mut() {
                        screen.image = None;
                    }
                }
                return Box::pin(self.supervise(turn, repair, emit, last_call, cancel)).await;
            }
        }
        Err(Repairable::Not(e))
    }

    /// supervisor.py:3778 `_conversation_blocks`: the conversation as context blocks with their origins, the entries'
    /// ages, and the newest shown result's id. `since_last_notice` (the attention check, I2): only the entries after
    /// the conversation's last notice.
    #[allow(clippy::type_complexity)]
    pub(crate) fn conversation_view(
        &self,
        current: &BTreeSet<String>,
        session: &str,
        since_last_notice: bool,
    ) -> Flow<(Vec<Sourced>, Vec<(String, ferb_clock::py::Time)>, String)> {
        let summary = self
            .of_kind(SUMMARY_KIND, "session_id", session, None)
            .last()
            .cloned();
        let covered: BTreeSet<String> = match &summary {
            None => BTreeSet::new(),
            Some(s) => self
                .of_kind(SUMMARY_KIND, "session_id", &s.text("session_id"), None)
                .iter()
                .flat_map(|r| {
                    r.get("covered")
                        .and_then(Value::as_array)
                        .map(|a| a.iter().map(ferb_py::value::py_str).collect::<Vec<_>>())
                        .unwrap_or_default()
                })
                .collect(),
        };
        let mut kinds_read: Vec<&str> = CONVERSATION_KINDS.to_vec();
        kinds_read.extend(SELF_POSTS);
        let rows: Vec<JournalRow> = self
            .entries_of(&kinds_read)
            .iter()
            .map(Entry::row)
            .collect();
        let task_ids = self
            .continuity(|c| c.task_ids(session))
            .map_err(Failure::from)?;
        let zone = self.deps.clock.zone();
        let (entries, ages) = timeline(&Timeline {
            current,
            conversation: Some(session),
            rows: &rows,
            task_ids: Some(&task_ids),
            covered: &covered,
            reads: &Reads(self),
            zone: &zone,
        });
        let latest = latest_result(&entries);
        let summary_row = summary.as_ref().map(Entry::row);
        let conversation = conversation_blocks(
            entries,
            summary_row.as_ref(),
            since_last_notice,
            self.deps.tokenizer,
        );
        Ok((conversation, ages, latest))
    }

    /// `_supervise`'s blocks (supervisor.py:3497-3601): the runtime, the context and the blocks with their origins.
    #[allow(clippy::type_complexity)]
    fn planner_blocks(
        &self,
        turn: &Arc<Mutex<Turn>>,
        repair: Option<&[String]>,
        last_call: &str,
        prep: &mut Prepare,
    ) -> Flow<(
        Arc<dyn ferb_worker::ports::ModelRuntime>,
        Vec<ContextBlock>,
        Vec<Sourced>,
        Vec<String>,
    )> {
        let (session, current, preview) = {
            let t = turn.lock().unwrap();
            (t.session.clone(), t.request_ids(), t.preview)
        };
        // `_lookup_tools`: the planner's read-only lookups that are installed; none for the intro's preview
        let lookups = if !last_call.is_empty() || preview {
            Vec::new()
        } else {
            let names = self.deps.daemon.tool_names();
            let mut l: Vec<String> = PLANNER_TOOLS
                .iter()
                .filter(|n| names.iter().any(|x| x == *n))
                .map(|n| n.to_string())
                .collect();
            l.sort();
            l
        };
        let rt = self
            .deps
            .daemon
            .make_runtime(
                "orchestrator",
                "supervisor",
                &lookups,
                &ferb_context::schemas::supervisor_schema(),
                None,
            )
            .map_err(Failure::Worker)?;
        prep.mark(self, "runtime");
        // the conversation (`_conversation_blocks`)
        let (conversation, ages, latest) = self.conversation_view(&current, &session, false)?;
        let zone = self.deps.clock.zone();
        prep.mark(self, "conversation");
        let env = turn.lock().unwrap().environment.clone();
        let live_facts = if preview {
            self.deps.daemon.live_facts(false, false)
        } else {
            self.deps.daemon.live_facts(true, true)
        };
        prep.mark(self, "live_facts");
        let tasks = if preview {
            None
        } else {
            self.task_state(&session)
        };
        prep.mark(self, "tasks");
        let (owner_active, last_event) = self.deps.daemon.signals();
        let since = if preview || session.starts_with("schedule:") {
            None
        } else {
            self.since_block(&current)
        };
        let latest_blocks = if preview || session.contains(':') {
            Vec::new()
        } else {
            self.latest(&latest)
        };
        // L12: the window each message read together was sent from, or what the owner saw as they sent this one
        let windows = {
            let t = turn.lock().unwrap();
            let wrap = |text: &str| self.wrap(text);
            if !t.screens.is_empty() {
                let attached = t.planner_images();
                t.screens
                    .iter()
                    .map(|seen| {
                        let nth = seen.image.as_ref().map(|image| {
                            (
                                attached.iter().position(|a| a == image).unwrap_or(0) + 1,
                                attached.len(),
                            )
                        });
                        crate::planner::front_window_block(
                            seen.front.as_ref(),
                            seen.image.is_some(),
                            &crate::lines::sent_from(&seen.said),
                            seen.withheld.as_deref(),
                            seen.missing.as_deref(),
                            nth,
                            &wrap,
                        )
                    })
                    .collect()
            } else if let Some(seen) = &t.seen {
                vec![crate::planner::front_window_block(
                    seen.front.as_ref(),
                    seen.image.is_some(),
                    FRONT_WINDOW,
                    seen.withheld.as_deref(),
                    seen.missing.as_deref(),
                    None,
                    &wrap,
                )]
            } else {
                Vec::new()
            }
        };
        let questions = if preview {
            None
        } else {
            Some(self.questions_block(&session))
        };
        let repair_prompt = match repair {
            Some(_) => self
                .deps
                .prompts
                .load(&format!("{}/repair.md", crate::prompts::APPROACH), &[])
                .map_err(|e| Failure::raised("FileNotFoundError", e.to_string()))?,
            None => String::new(),
        };
        // ORCH-557/558: what this request's planner looked up, for its last call
        let found: Vec<crate::planner::Found> = if last_call.is_empty() {
            Vec::new()
        } else {
            let rid = turn.lock().unwrap().request_id.clone();
            self.tool_outputs(&format!("{rid}{LOOKUPS}"))
                .into_iter()
                .map(|(_, made, text)| (made, text))
                .collect()
        };
        let t = turn.lock().unwrap();
        let open = t.open.as_ref().map(|s| crate::planner::OpenAsk {
            asked: s.asked.clone(),
            payload: s.payload.clone(),
            carried: s
                .carried
                .iter()
                .map(|e| (e.id.clone(), e.origin.clone()))
                .collect(),
            asked_outside: s.asked_outside.clone(),
        });
        let entry_ids: Vec<String> = t.entries.iter().map(|e| e.id.clone()).collect();
        let now = self.now();
        let last_runs = if preview {
            BTreeSet::new()
        } else {
            self.deps.daemon.last_run_origins()
        };
        // a recurring run compares with its previous run
        let previous_run = if t.schedule_previous.is_some() || session.starts_with("schedule:") {
            let previous = t.schedule_previous.as_ref().map(|p| {
                let ran = ferb_clock::py::parse(&p.ran_at).unwrap_or(now);
                (
                    JournalRow {
                        id: p.entry_id.clone(),
                        at: ran,
                        kind: String::new(),
                        data: obj(json!({"text": p.text})),
                        origin: p.origin.clone(),
                    },
                    ran,
                    p.status.clone(),
                )
            });
            Some(crate::planner::previous_run_block(
                previous
                    .as_ref()
                    .map(|(row, ran, status)| (row, ran, status.as_str())),
                &zone,
            ))
        } else {
            None
        };
        let wrap = |text: &str| self.wrap(text);
        let (sourced, first, last) = crate::planner::planner_sourced(PlannerTurn {
            session: &session,
            preview,
            stable: &env.stable,
            live_env: &env.live,
            live_facts: &live_facts,
            last_run_origins: &last_runs,
            now: &now,
            owner_active: owner_active.as_ref(),
            last_event: last_event.as_ref(),
            conversation,
            ages: &ages,
            tasks,
            since,
            latest: latest_blocks,
            windows,
            previous_run,
            questions,
            repair: repair.map(|r| (repair_prompt.as_str(), r)),
            last_call: (!last_call.is_empty()).then_some((last_call, found.as_slice())),
            open: open.as_ref(),
            entries: &entry_ids,
            payload: &t.payload,
            message_origin: &t.message_origin,
            wrap: &wrap,
            zone: &zone,
        });
        drop(t);
        let ctx = crate::planner::planner_context(&sourced, first, last);
        let situation: Vec<Value> = sourced[first..last]
            .iter()
            .map(|s| {
                json!({"reference": s.block.reference, "text": s.block.text, "origin": joined(&s.origins),
                       "source_refs": crate::lines::stored_refs(&s.block.source_refs)})
            })
            .collect();
        turn.lock().unwrap().planner_sources = ferb_py::json::dumps_utf8(&situation);
        prep.mark(self, "blocks");
        Ok((rt, ctx, sourced, lookups))
    }

    /// The assembly and the lookups the planner saw (supervisor.py:3633-3640).
    fn after_decision(&self, turn: &Arc<Mutex<Turn>>, ctx: &[ContextBlock], sourced: &[Sourced]) {
        let mut t = turn.lock().unwrap();
        let Some(assembly) = t.assembly.take() else {
            return;
        };
        let given: Vec<(&ContextBlock, &BTreeSet<String>)> = ctx
            .iter()
            .zip(sourced)
            .filter(|(b, _)| !b.text.is_empty())
            .map(|(b, s)| (b, &s.origins))
            .collect();
        let mut origins = BTreeSet::new();
        let mut shown = Vec::new();
        for ((block, made), seen) in given.iter().zip(&assembly.shown) {
            if *seen || !block.history {
                origins.extend(made.iter().cloned());
                shown.push(((*made).clone(), block.text.clone(), block.values.clone()));
            }
        }
        t.turn_origins = origins;
        t.shown = shown;
    }

    /// supervisor.py:3690 `_lookup_grant`.
    fn lookup_grant(
        &self,
        turn: &Arc<Mutex<Turn>>,
        span_id: &str,
        tools: &[String],
    ) -> (String, String) {
        if tools.is_empty() {
            return (String::new(), String::new());
        }
        let (rid, session) = {
            let t = turn.lock().unwrap();
            (t.request_id.clone(), t.session.clone())
        };
        let task_id = format!("{rid}{LOOKUPS}");
        let holder = format!("call:{span_id}");
        self.task_hold(&task_id, span_id, &task_id);
        self.deps.daemon.task_record(&super::LiveTask {
            task_id: task_id.clone(),
            request_id: rid.clone(),
            started_at: self.now_iso(),
            deadline: None,
            span_id: span_id.to_string(),
            tree_id: Some(task_id.clone()),
        });
        self.lend(&task_id, &session);
        let scopes: BTreeSet<String> = PLANNER_SCOPES.iter().map(|s| s.to_string()).collect();
        let token = self.deps.daemon.issue_token(
            &task_id,
            &holder,
            &scopes,
            "owner",
            f64::INFINITY,
            true,
            Some(tools),
            Some(PLANNER_LOOKUPS),
        );
        (token, holder)
    }

    /// `continuity.lend` (kept beside the store: the gateway's lookups read it).
    fn lend(&self, task_id: &str, conversation: &str) {
        self.live()
            .lent
            .insert(task_id.to_string(), conversation.to_string());
    }

    /// `continuity.conversation_of` for a task lent a chat (the planner's lookups read it).
    pub fn lent_conversation(&self, task_id: &str) -> Option<String> {
        self.live().lent.get(task_id).cloned()
    }

    /// supervisor.py:3708 `_lookups_end`.
    fn lookups_end(&self, rid: &str, holder: &str) {
        if holder.is_empty() {
            return;
        }
        self.deps.daemon.revoke_worker(holder);
        self.deps.daemon.forget_calls(holder);
        let task_id = format!("{rid}{LOOKUPS}");
        self.task_remove(&task_id);
        self.live().lent.remove(&task_id);
    }

    /// supervisor.py:3717 `_lookups_read`.
    fn lookups_read(&self, turn: &Arc<Mutex<Turn>>) {
        let rid = turn.lock().unwrap().request_id.clone();
        let rows = self.tool_outputs(&format!("{rid}{LOOKUPS}"));
        let mut t = turn.lock().unwrap();
        for (_, made, _) in &rows {
            t.turn_origins.extend(made.iter().cloned());
        }
        for (rowid, made, text) in rows {
            if rowid > t.looked {
                t.exposures.push(obj(json!({"text": text, "origin": joined(&made), "source_refs": [],
                    "reference": "a lookup by this request's planner (its tool call in the trace)", "payload": text})));
                t.shown.push((made, text, None));
                t.looked = rowid;
            }
        }
    }

    /// supervisor.py:2099 `_task_state` (the running trees as `TreeView`s; this chat's tasks waiting on the owner).
    fn task_state(&self, session: &str) -> Option<Sourced> {
        let mine = self.continuity(|c| c.task_ids(session)).unwrap_or_default();
        let trees: Vec<Arc<super::Tree>> = {
            let live = self.live();
            let mut seen = Vec::new();
            for (key, tree) in &live.tree_of {
                if *key == tree.id && !seen.iter().any(|t: &Arc<super::Tree>| t.id == tree.id) {
                    seen.push(tree.clone());
                }
            }
            seen
        };
        let open_cards = self.deps.daemon.approvals();
        let zone = self.deps.clock.zone();
        let mut views = Vec::new();
        for tree in trees {
            // ORCH-595: every open card of it, whoever asked
            let cards: Vec<Map<String, Value>> = open_cards
                .iter()
                .filter(|ap| self.question_root(&ap.task_id) == tree.id)
                .map(|ap| {
                    let asked = self
                        .of_kind("question.asked", "approval_id", &ap.id, None)
                        .last()
                        .and_then(|e| ferb_clock::py::parse(&e.at));
                    crate::planner::card_row(
                        &ap.kind,
                        &ap.capability,
                        &ap.summary,
                        &ap.reason,
                        asked.as_ref(),
                        crate::planner::routable(&ap.kind, &ap.capability, &FERB_CHECKS),
                        &zone,
                    )
                })
                .collect();
            let (paused, question) = self.paused_view(&tree.id); // owner_wait.rs
            let loops = self.loop_facts(&tree.id);
            let asleep = self.live().sleeping.get(&tree.id).cloned();
            let preview = self.preview_status(&tree.id, "");
            let st = tree.lock();
            let spec = st.spec.clone().expect("a spec");
            let steps = if spec.plan.is_some() {
                self.plans(|p| p.steps(&tree.id))
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|(k, _)| !k.starts_with(ferb_orchestrator::plan_store::EARLIER))
                    .map(|(k, s)| (k, s.status))
                    .collect()
            } else {
                Vec::new()
            };
            views.push(crate::planner::TreeView {
                task_id: tree.id.clone(),
                mine: mine.contains(&tree.id),
                schedule: st.schedule_id.is_some(),
                goal: spec.goal.clone(),
                stopping: !st.stop.is_empty(),
                reports: st.reports.len(),
                last_report: st.reports.last().cloned(),
                steps,
                loops,
                paused,
                question,
                cards,
                last_step: self.runs.last_step(&tree.id),
                asleep,
                preview,
                goals: spec.plan.as_ref().map(|p| {
                    p.steps
                        .iter()
                        .map(|s| {
                            let g = if !s.goal.is_empty() {
                                s.goal.clone()
                            } else if !s.question.is_empty() {
                                s.question.clone()
                            } else {
                                s.kind.clone()
                            };
                            (s.id.clone(), json!(g))
                        })
                        .collect()
                }),
                runtime: if !st.runtime.is_empty() {
                    st.runtime.clone()
                } else if !spec.worker_type.is_empty() {
                    spec.worker_type.clone()
                } else {
                    "default".into()
                },
                standing: self.deps.daemon.standing_scopes(&tree.id),
                folders: spec.folders.clone(),
                origins: {
                    let mut o = kinds(&self.task_origins(&tree.id));
                    o.insert(origin::INFERRED.into());
                    o
                },
            });
        }
        let running: BTreeSet<String> = self.live().tree_of.keys().cloned().collect();
        let waiting: Vec<crate::planner::WaitingTask> = if session.starts_with("schedule:") {
            Vec::new()
        } else {
            self.continuity(|c| c.candidates(session))
                .unwrap_or_default()
                .into_iter()
                .filter(|r| {
                    let id = ferb_orchestrator::sql::col(r, "task_id");
                    !running.contains(&id)
                        && ferb_orchestrator::sql::col(r, "status") == "needs_user"
                })
                .map(|r| {
                    let id = ferb_orchestrator::sql::col(&r, "task_id");
                    let mut o = kinds(&self.task_origins(&id));
                    o.insert(origin::INFERRED.into());
                    crate::planner::WaitingTask {
                        task_id: id,
                        goal: ferb_orchestrator::sql::col(&r, "goal"),
                        status: ferb_orchestrator::sql::col(&r, "status"),
                        prior_task: r
                            .get("prior_task")
                            .and_then(Value::as_str)
                            .map(str::to_string),
                        origins: o,
                    }
                })
                .collect()
        };
        crate::planner::task_state(&views, &waiting, self.deps.tokenizer, &|t| self.wrap(t))
    }

    /// supervisor.py:4342 `_since_block`.
    fn since_block(&self, current: &BTreeSet<String>) -> Option<Sourced> {
        let previous: Vec<Entry> = self
            .of_kind("chat.message", "", "", None)
            .into_iter()
            .filter(|e| !current.contains(&e.request_id) && !current.contains(&e.text("read_with")))
            .collect();
        let last = previous.last()?;
        let marks = vec!["?"; SINCE_KINDS.len()].join(",");
        let after: Vec<JournalRow> = {
            let db = self.db();
            let mut params: Vec<rusqlite::types::Value> = SINCE_KINDS
                .iter()
                .map(|k| rusqlite::types::Value::Text((*k).into()))
                .collect();
            params.push(rusqlite::types::Value::Text(last.id.clone()));
            ferb_orchestrator::sql::rows(
                db.conn(),
                &format!(
                    "SELECT * FROM journal WHERE kind IN ({marks}) AND seq > (SELECT seq FROM journal WHERE id = ?) \
                     ORDER BY seq"
                ),
                &params,
            )
            .unwrap_or_default()
            .iter()
            .map(|r| {
                let s = |k: &str| r.get(k).and_then(Value::as_str).unwrap_or("").to_string();
                Entry {
                    id: s("id"),
                    at: s("at"),
                    kind: s("kind"),
                    data: ferb_py::json::loads(&s("data_json")).unwrap_or(Value::Null),
                    origin: s("origin"),
                    request_id: String::new(),
                }
                .row()
            })
            .collect()
        };
        let prev_rows: Vec<JournalRow> = previous.iter().map(Entry::row).collect();
        crate::planner::since_block(
            &prev_rows,
            &after,
            &Since(self),
            &self.deps.clock.zone(),
            &|t| self.wrap(t),
        )
    }

    /// supervisor.py:3887 `_latest_blocks`.
    fn latest(&self, latest: &str) -> Vec<Sourced> {
        if latest.is_empty() {
            return Vec::new();
        }
        let found = {
            let db = self.db();
            ferb_orchestrator::sql::row(
                db.conn(),
                "SELECT * FROM journal WHERE id = ?",
                &[rusqlite::types::Value::Text(latest.into())],
            )
            .ok()
            .flatten()
        };
        let Some(found) = found else {
            return Vec::new();
        };
        let s = |k: &str| {
            found
                .get(k)
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string()
        };
        let entry = Entry {
            id: s("id"),
            at: s("at"),
            kind: s("kind"),
            data: ferb_py::json::loads(&s("data_json")).unwrap_or(Value::Null),
            origin: s("origin"),
            request_id: String::new(),
        };
        let task_id = entry.text("task_id");
        let part_rows: Vec<JournalRow> = self
            .of_kind("chat.part", "task_id", &task_id, None)
            .iter()
            .map(Entry::row)
            .collect();
        let order = self
            .continuity(|c| c.part_order(&task_id))
            .unwrap_or_default();
        let row = entry.row();
        crate::planner::latest_blocks(
            &LatestReads {
                found: Some(&row),
                part_rows: &part_rows,
                part_order: &order,
            },
            self.deps.tokenizer,
            &self.deps.clock.zone(),
        )
    }

    /// supervisor.py:3757 `_questions_block`.
    fn questions_block(&self, session: &str) -> Sourced {
        let pending = self.deps.daemon.pending_questions();
        let mine = self.continuity(|c| c.task_ids(session)).unwrap_or_default();
        let conversations: Vec<Value> = {
            self.conversations()
                .list(None)
                .unwrap_or_default()
                .into_iter()
                .filter(|row| {
                    row.get("tree_id")
                        .and_then(Value::as_str)
                        .is_some_and(|t| mine.contains(t))
                })
                .map(Value::Object)
                .collect()
        };
        crate::planner::questions_block(&pending, &conversations, &FERB_CHECKS, &|t| self.wrap(t))
    }

    /// supervisor.py:4186 `_keep_recent`: starts the running summary's fold when the tail has gathered a chunk
    /// (fold.rs `start_keep_recent`; kept for callers of the earlier signature).
    pub fn keep_recent(&self, session: &str) -> Result<(), NotPorted> {
        self.start_keep_recent(session);
        Ok(())
    }

    /// supervisor.py:2703 `_route`.
    async fn route(
        &self,
        turn: &Arc<Mutex<Turn>>,
        mut out: SupervisorOutcome,
        emit: &Emit,
    ) -> Flow<()> {
        if let Some(routed) = out.answer_question.take() {
            // the answer is recorded first; the rest of the turn then runs as in any turn. When FERB has to ask which
            // question the answer is for, nothing else in the message runs now
            let (kind, mut said) = self.answer(turn, &routed);
            if kind == "clarify" {
                let made = origin_set(&[origin::FERB, origin::INFERRED]);
                return self
                    .exit_with(
                        turn,
                        &out,
                        "clarify",
                        &said,
                        origin::FERB,
                        false,
                        Some(&made),
                        emit,
                    )
                    .await;
            }
            let ends_in_reply =
                out.control.is_none() && !(out.kind == "task" && out.task.is_some());
            let alone = ends_in_reply
                && ferb_orchestrator::py::strip(&out.text).is_empty()
                && !carries_more(&out);
            let joined = turn.lock().unwrap().joined;
            if said == NO_QUESTION && joined {
                // ORCH-621: a routing that names no open question says nothing when the message answered FERB's own
                // question (its answer was taken); recorded, never shown
                let rid = turn.lock().unwrap().request_id.clone();
                self.journal(
                    "question.routing_unused",
                    json!({"request_id": rid, "approval_id": routed.approval_id, "joined": joined}),
                    None,
                );
                said = String::new();
            } else if alone {
                return self
                    .exit_with(turn, &out, "reply", &said, origin::FERB, false, None, emit)
                    .await;
            }
            if said.is_empty() {
            } else if ends_in_reply && !ferb_orchestrator::py::strip(&out.text).is_empty() {
                // one message: FERB's word on the answer opens the planner's reply
                turn.lock().unwrap().acknowledged = said;
            } else {
                // a stop, correction or task follows: its own line first
                self.record_reply(turn, "reply", &said, &origin_set(&[origin::FERB]), "", None)
                    .await;
                if out.new_task().is_some() {
                    turn.lock().unwrap().said_first = said;
                } else {
                    let _ = emit.send(json!({"type": "reply", "text": said}));
                }
            }
        }
        if out.control.is_some() {
            return self.control(turn, out, emit).await;
        }
        if out.kind == "reply" || out.kind == "clarify" || out.task.is_none() {
            let kind = if out.kind == "clarify" {
                "clarify"
            } else {
                "reply"
            };
            let text = out.text.clone();
            return self
                .exit(turn, &out, kind, &text, origin::INFERRED, emit)
                .await;
        }
        let spec = out.task.clone().expect("a task");
        self.start(turn, &out, spec, emit).await
    }

    /// supervisor.py:3344 `_side_entries`: the conversation changes (conversations.rs), memory and forgets
    /// (memory_side.rs), the reminder cancels, the reminders and timers (side.rs), the schedules and their changes and
    /// the schedules card (schedules_chat.rs, through the daemon's schedules port), each with the one quote check.
    async fn side_entries(&self, turn: &Arc<Mutex<Turn>>, out: &SupervisorOutcome) -> Flow<Side> {
        self.side_entries_named(turn, out, &BTreeSet::new()).await
    }

    /// `_side_entries(turn, out, named)`: `named`, the sites a steered task named before the steer restated it.
    pub(crate) async fn side_entries_named(
        &self,
        turn: &Arc<Mutex<Turn>>,
        out: &SupervisorOutcome,
        named: &BTreeSet<String>,
    ) -> Flow<Side> {
        let mut out = out.clone();
        let rid = turn.lock().unwrap().request_id.clone();
        if (!out.remember.is_empty() || !out.forget.is_empty())
            && ferb_orchestrator::test_runs::TestRuns::new(&self.db())
                .request(Some(&rid))
                .unwrap_or(false)
        {
            // GATE-550: a test run learns nothing and changes none of the owner's memory
            self.journal(
                "test_run.memory_skipped",
                json!({"request_id": rid, "remember": out.remember.len(), "forget": out.forget.len()}),
                None,
            );
            out.remember.clear();
            out.forget.clear();
        }
        let out = &out;
        // the conversation changes first, each with the one quote check (ORCH-520), then only this chat's own
        let mut events: Vec<Value> = Vec::new();
        for change in &out.conversation_changes {
            if self.owner_said(turn, &change.quote).is_none() {
                events.push(json!({"type": "reply", "text": blocked::blocked(
                    blocked::CONVERSATION,
                    blocked::NOT_IN_RECENT,
                    "that conversation didn't change",
                    "say what should change in it",
                )}));
                continue;
            }
            let session = turn.lock().unwrap().session.clone();
            let permitted = self
                .continuity(|c| c.task_ids(&session))
                .unwrap_or_default();
            let changed = match self.conversations().get(&change.id) {
                Err(e) => Err(e.to_string()),
                Ok(row) => {
                    let tree = row
                        .get("tree_id")
                        .map(ferb_py::value::py_str)
                        .unwrap_or_default();
                    if !permitted.contains(&tree) {
                        Err("That conversation belongs to another chat; use its chat or ferb conversations.".to_string())
                    } else {
                        self.change_conversation(&change.id, &change.action, &change.quote)
                    }
                }
            };
            let text = match changed {
                Ok(()) => format!("Conversation {}: {}", change.action, change.id),
                Err(words) => words,
            };
            events.push(json!({"type": "reply", "text": text}));
        }
        let memory = self.memory_side(turn, out, named).await?;
        let scheduled = !out.schedules.is_empty()
            || !out.schedule_changes.is_empty()
            || !out.reminder_cancels.is_empty()
            || out.show_schedules;
        let port = self.deps.daemon.schedules_chat();
        if scheduled && port.is_none() {
            return Err(NotPorted("Supervisor._side_entries (no schedules port)").into());
        }
        let recent = blocked::NOT_IN_RECENT;
        events.extend(memory.events);
        let mut changed: Vec<String> = Vec::new();
        let mut cards: Vec<Value> = memory.cards;
        if let Some(port) = &port {
            for rc in &out.reminder_cancels {
                let line = match self.owner_said(turn, &rc.quote) {
                    Some(words) => port.reminder_cancel(rc, &words).await,
                    None => blocked::blocked(
                        blocked::CHANGES,
                        recent,
                        "no reminder was cancelled",
                        "say which reminder to cancel and I'll cancel it",
                    ),
                };
                events.push(json!({"type": "timer", "text": line}));
                changed.push(line);
            }
        }
        let timers = self.timer_side(turn, out);
        events.extend(timers.events);
        cards.extend(timers.cards);
        if let Some(port) = &port {
            // what wrote a scheduled request
            let mut made: BTreeSet<String> =
                origin::kinds(turn.lock().unwrap().turn_origins.iter().map(String::as_str));
            made.insert(origin::INFERRED.into());
            for sched in &out.schedules {
                let (words, unquoted) = self.schedule_words(turn, sched);
                let (line, card) = if !unquoted.is_empty() {
                    self.journal(
                        "schedule.refused",
                        json!({"why": format!("{unquoted} words not in the owner's words")}),
                        None,
                    );
                    (
                        blocked::blocked(
                            if unquoted == "quote" {
                                blocked::REMINDERS
                            } else {
                                blocked::END_DATE
                            },
                            recent,
                            &format!(
                                "{} isn't scheduled",
                                crate::lines::quoted(&[&sched.request])
                            ),
                            "type what you want scheduled in this chat and I'll set it up",
                        ),
                        None,
                    )
                } else {
                    let sink = self.sink_origins_for_new_schedule(turn);
                    port.create(sched, &words, &rid, &made, &sink).await
                };
                events.push(json!({"type": "timer", "text": line}));
                changed.push(line);
                cards.extend(card);
            }
            for ch in &out.schedule_changes {
                let held = self.owner_said(turn, &ch.quote);
                let (line, card) = match held {
                    None => (
                        blocked::blocked(
                            blocked::CHANGES,
                            recent,
                            "no schedule was changed",
                            "say which schedule should change and how",
                        ),
                        None,
                    ),
                    Some(mut words) => {
                        if let Some(until) = &ch.until {
                            if !ferb_memory::terms::quoted(&words, &until.quote) {
                                if let Some(more) = self.owner_said(turn, &until.quote) {
                                    words = format!("{words}\n{more}");
                                }
                            }
                        }
                        if ch.action == "edit" {
                            let sink = self.sink_origins_for_new_schedule(turn);
                            port.edit(ch, &words, &rid, &made, &sink).await
                        } else {
                            (port.change(ch, &words).await, None)
                        }
                    }
                };
                events.push(json!({"type": "timer", "text": line}));
                changed.push(line);
                cards.extend(card);
            }
            if out.show_schedules {
                if let Some((line, card)) = port.card().await {
                    changed.push(line);
                    cards.push(card);
                }
            }
        }
        // code's lines: what was saved, what changed, the reminders set, then each refusal (the reminders' not set
        // and already set, the memory not saved, the quote check's)
        let mut refused: Vec<(String, String)> = timers.not_set;
        let not_saved = crate::lines::not_saved(&memory.unsaved);
        if !not_saved.is_empty() {
            refused.push(("memory".into(), not_saved));
        }
        refused.extend(memory.stopped);
        refused.extend(timers.stopped);
        let own = memory.own;
        let mut lines = memory.saved;
        lines.extend(changed);
        lines.extend(timers.set_lines);
        lines.extend(refused.iter().map(|(_, l)| l.clone()));
        Ok(Side {
            events,
            lines,
            own,
            refused,
            cards,
        })
    }

    /// supervisor.py:1414 `_owner_said`: the owner's words holding `quote` (the one quote check): this turn's first,
    /// else the newest recent earlier message of theirs in this chat; None: the quote check refuses.
    fn owner_said(&self, turn: &Arc<Mutex<Turn>>, quote: &str) -> Option<String> {
        self.said_in_chat(turn)
            .into_iter()
            .find(|w| ferb_learning::text::quoted(w, quote, false))
    }

    /// supervisor.py:1424 `_schedule_words`: the owner's words a new schedule rests on, and "" or which quote the check
    /// refused ("quote", "until").
    fn schedule_words(
        &self,
        turn: &Arc<Mutex<Turn>>,
        spec: &ferb_orchestrator::models::ScheduleRequest,
    ) -> (String, String) {
        let mut words = turn.lock().unwrap().typed.clone();
        let until = spec
            .until
            .as_ref()
            .map(|u| u.quote.clone())
            .unwrap_or_default();
        for (part, quote) in [("quote", spec.quote.as_str()), ("until", until.as_str())] {
            if quote.is_empty() || ferb_memory::terms::quoted(&words, quote) {
                continue;
            }
            match self.owner_said(turn, quote) {
                None => return (String::new(), part.to_string()),
                Some(held) => words = format!("{words}\n{held}"),
            }
        }
        (words, String::new())
    }

    /// supervisor.py:4321 `_sink_origins_for_new_schedule`: the sink rule's request origins a schedule made in this
    /// turn records for its runs (S4, L4).
    fn sink_origins_for_new_schedule(&self, turn: &Arc<Mutex<Turn>>) -> BTreeSet<String> {
        let t = turn.lock().unwrap();
        let mut out: BTreeSet<String> = BTreeSet::from([origin::INFERRED.to_string()]);
        if t.message_origin.is_empty() {
            out.insert(origin::external("unlabeled", ""));
        } else {
            out.extend(t.message_origin.iter().cloned());
        }
        for e in &t.exposures {
            let made = e.get("origin").and_then(Value::as_str).unwrap_or("");
            for o in origin::split(Some(made)) {
                if origin::is_external(&o) {
                    out.insert(origin::kind(&o));
                }
            }
        }
        out
    }

    /// supervisor.py:4691 `_said_cards`: cards as the owner is shown them, redacted like the text beside them, each
    /// card's `line` exactly as that text carries it.
    pub(crate) fn said_cards(&self, cards: &[Value], marked: bool) -> Vec<Value> {
        cards
            .iter()
            .map(|card| {
                let mut shown = self.redact(card);
                if marked {
                    if let Some(line) = shown.get("line").and_then(Value::as_str) {
                        let first = ferb_policy::confirm::from_task_lines(line)
                            .split('\n')
                            .next()
                            .unwrap_or("")
                            .to_string();
                        shown["line"] = json!(first);
                    }
                }
                shown
            })
            .collect()
    }

    /// supervisor.py:2755 `_exit`.
    async fn exit(
        &self,
        turn: &Arc<Mutex<Turn>>,
        out: &SupervisorOutcome,
        kind: &str,
        text: &str,
        by: &str,
        emit: &Emit,
    ) -> Flow<()> {
        self.exit_with(turn, out, kind, text, by, true, None, emit)
            .await
    }

    /// `_exit` with its `side` (apply the turn's side entries) and `made` (the reply's origins) arguments.
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn exit_with(
        &self,
        turn: &Arc<Mutex<Turn>>,
        out: &SupervisorOutcome,
        kind: &str,
        text: &str,
        by: &str,
        side: bool,
        made: Option<&BTreeSet<String>>,
        emit: &Emit,
    ) -> Flow<()> {
        let effects = if side {
            self.side_entries(turn, out).await?
        } else {
            Side::default()
        };
        for ev in &effects.events {
            let _ = emit.send(ev.clone());
        }
        let mut said = if by == origin::FERB {
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
            code_lines = own_tagged(
                &self.redact_text(&effects.lines.join("\n")),
                &effects.lines,
                &effects.own,
            );
            said = format!("{}\n{code_lines}", said.trim_end());
        }
        let written = if let Some(made) = made {
            made.clone()
        } else if by == origin::FERB && effects.lines.is_empty() {
            origin_set(&[origin::FERB])
        } else {
            let mut w = turn.lock().unwrap().turn_origins.clone();
            w.insert(origin::INFERRED.into());
            w
        };
        self.record_reply(turn, kind, &said, &written, &code_lines, None)
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
        let mut reply = json!({"type": kind, "text": said});
        if !effects.cards.is_empty() {
            // step 8e: the app draws these in place of their lines
            reply["cards"] = Value::Array(self.said_cards(&effects.cards, true));
        }
        let _ = emit.send(reply);
        let _ = effects.refused;
        Ok(())
    }

    /// supervisor.py:2790 `_record_reply`.
    pub(crate) async fn record_reply(
        &self,
        turn: &Arc<Mutex<Turn>>,
        kind: &str,
        text: &str,
        made: &BTreeSet<String>,
        lines: &str,
        told: Option<(&str, &str)>,
    ) {
        let (session, rid, shown, decided) = {
            let t = turn.lock().unwrap();
            (
                t.session.clone(),
                t.request_id.clone(),
                t.shown.clone(),
                t.decided,
            )
        };
        let mut data = obj(json!({"session_id": session, "request_id": rid, "text": text}));
        if !lines.is_empty() && text.ends_with(lines) {
            data.insert("lines".into(), json!(lines));
        }
        if let Some((task, end)) = told {
            if text.starts_with(end) {
                data.insert("told_task".into(), json!(task));
                data.insert("told_end".into(), json!(end));
            }
        }
        let question_kind = if made == &origin_set(&[origin::FERB, origin::INFERRED]) {
            "which"
        } else {
            "clarify"
        };
        if kind == "clarify" {
            data.insert("question_kind".into(), json!(question_kind));
        }
        let provenance = if made != &origin_set(&[origin::FERB]) {
            let made: Vec<String> = made.iter().cloned().collect();
            ferb_policy::carried::derive(text, &shown_records(&shown), &made)
        } else {
            json!({"kinds": [origin::FERB], "values": {}})
        };
        data.insert(ferb_policy::carried::PROVENANCE.into(), provenance);
        let kinds = kinds(made);
        self.journal(
            &format!("chat.{kind}"),
            Value::Object(data),
            Some(&joined(&kinds)),
        );
        if let Some(decided) = decided {
            let span = self.span_start(
                "planner.reply",
                None,
                None,
                None,
                json!({"role": "supervisor", "request_id": rid, "kind": kind}),
            );
            let after = ((self.monotonic() - decided) * 1000.0).round_ties_even() as i64;
            self.span_finish(
                &span,
                Some(json!({"after_decision_ms": after})),
                None,
                None,
                None,
            );
            turn.lock().unwrap().decided = None;
        }
        if kind == "clarify" {
            self.journal(
                "question.asked",
                json!({"kind": question_kind, "request_id": rid, "session_id": session}),
                None,
            );
        }
    }

    /// supervisor.py:2819 `_start`.
    pub(crate) async fn start(
        &self,
        turn: &Arc<Mutex<Turn>>,
        out: &SupervisorOutcome,
        spec: TaskSpec,
        emit: &Emit,
    ) -> Flow<()> {
        self.start_with(turn, out, spec, emit, None).await
    }

    /// `_start` with `resume=`, `prior_task=`, `message=`, `conversation=`: an ended task carried on in place.
    pub(crate) async fn start_with(
        &self,
        turn: &Arc<Mutex<Turn>>,
        out: &SupervisorOutcome,
        spec: TaskSpec,
        emit: &Emit,
        resumed: Option<Resumed>,
    ) -> Flow<()> {
        let effects = self.side_entries(turn, out).await?;
        for ev in &effects.events {
            let _ = emit.send(ev.clone());
        }
        let (rid, typed, payload, asked_outside, session, origins) = {
            let mut t = turn.lock().unwrap();
            t.side_lines = effects.lines.clone();
            t.side_cards = effects.cards.clone();
            (
                t.request_id.clone(),
                t.typed.clone(),
                t.payload.clone(),
                t.asked_outside.clone(),
                t.session.clone(),
                t.turn_origins.clone(),
            )
        };
        for (kind, text) in &effects.refused {
            let _ = emit.send(json!({"type": kind, "text": text}));
        }
        if resumed.is_none() {
            let targets = &spec.targets;
            let paths: Vec<String> = targets.paths.iter().map(|p| self.normalize(p)).collect();
            self.deps.daemon.record_request(&Request {
                request_id: rid.clone(),
                words: typed,
                targets: [
                    targets.apps.clone(),
                    targets.sites.clone(),
                    paths,
                    targets.recipients.clone(),
                ],
                payload: payload.clone(),
                asked_origins: asked_outside,
            });
        }
        let me = self.me();
        let release_session = session.clone();
        let release_rid = rid.clone();
        let release = move || me.release(&release_session, &release_rid);
        let r = resumed.unwrap_or_default();
        let message = r.message.unwrap_or(payload);
        self.run_tree(
            spec,
            &out.text,
            &rid,
            &message,
            super::tree::RunOpts {
                conversation: Some(r.conversation.unwrap_or(session)),
                origins,
                prior_task: r.prior_task,
                resume_id: r.resume_id,
                ..super::tree::RunOpts::default()
            },
            emit,
            &release,
        )
        .await
    }
}

/// `_start`'s keyword arguments for an ended task carried on in place (`_resume`).
#[derive(Clone, Debug, Default)]
pub struct Resumed {
    pub resume_id: Option<String>,
    pub prior_task: Option<String>,
    pub message: Option<String>,
    pub conversation: Option<String>,
}

/// supervisor.py:494 `_Prepare`: the planner's work before its model call (ORCH-525).
pub struct Prepare {
    span: String,
    steps: ferb_core::timing::Steps,
}

impl Prepare {
    fn start(sup: &Inner, request_id: &str, repair: bool) -> Prepare {
        let mut input = obj(json!({"role": "supervisor", "request_id": request_id}));
        if repair {
            input.insert("repair".into(), json!(true));
        }
        let span = sup.span_start("planner.prepare", None, None, None, Value::Object(input));
        let clock = sup.deps.clock.clone();
        Prepare {
            span,
            steps: ferb_core::timing::Steps::new(Box::new(move || clock.monotonic())),
        }
    }

    fn mark(&mut self, _sup: &Inner, step: &str) {
        self.steps.mark(step);
    }

    fn finish(self, sup: &Inner, error: Option<&Failure>) {
        let error = error.map(|e| format!("{}: {}", e.class(), e.message()));
        sup.span_finish(
            &self.span,
            Some(Value::Object(self.steps.facts())),
            error.as_deref(),
            None,
            None,
        );
    }
}

/// How a planner call failed: an answer to repair (its problems), or anything else.
pub enum Repairable {
    /// The answer to send back for repair: its problems and the exception's class.
    Problems(Vec<String>, String),
    Not(Failure),
}

impl Repairable {
    pub(crate) fn into_failure(self) -> Failure {
        self.failure()
    }

    fn failure(self) -> Failure {
        match self {
            Repairable::Problems(p, class) => Failure::raised(&class, p.join("\n")),
            Repairable::Not(f) => f,
        }
    }
}

impl From<Failure> for Repairable {
    fn from(f: Failure) -> Repairable {
        Repairable::Not(f)
    }
}

/// The class an invalid planner answer is repaired under. A malformed legacy answer whose validator raised TypeError
/// (a legacy `map` step whose `inputs` or `batches` isn't a list, or holds a list or an object; a legacy `schedule`
/// beside a `schedules` that isn't a list) went past Python's `except ValidationError` and failed the turn; here it is
/// sent back for repair like any invalid answer, with the exception's words (ENG-6, fixed against Python).
pub fn repair_class(e: &ferb_orchestrator::validate::ValidationError) -> &'static str {
    e.raised().map(|r| r.exception).unwrap_or("ValidationError")
}

pub fn origin_set(items: &[&str]) -> BTreeSet<String> {
    items.iter().map(|s| s.to_string()).collect()
}

/// `_carries_more`'s side outputs: anything besides an answer, a control or a task.
/// A side entry other than a reminder or timer (side.rs ports those).
pub fn has_unported_side_entries(out: &SupervisorOutcome) -> bool {
    !out.conversation_changes.is_empty()
}

pub fn has_side_entries(out: &SupervisorOutcome) -> bool {
    !out.remember.is_empty()
        || !out.forget.is_empty()
        || !out.timers.is_empty()
        || !out.schedules.is_empty()
        || !out.schedule_changes.is_empty()
        || out.show_schedules
        || !out.reminder_cancels.is_empty()
        || !out.conversation_changes.is_empty()
}

/// supervisor.py:759 `_by_window` (L12): the front window of each message read together, newest last; a window two
/// messages show is given once, as the newer message captured it, with both messages in `said`.
fn by_window(arrivals: &[(Arrival, super::Seen)]) -> Vec<super::Seen> {
    let mut windows: Vec<(Vec<String>, super::Seen)> = Vec::new();
    for (arrival, seen) in arrivals {
        let key = seen.window_key();
        let earlier = match windows.iter().position(|(k, _)| *k == key) {
            Some(i) => windows.remove(i).1.said,
            None => Vec::new(),
        };
        let text = match &arrival.marked {
            Some(m) => m.for_model.clone(),
            None => arrival.text.clone(),
        };
        let mut seen = seen.clone();
        seen.said = earlier;
        seen.said.push(crate::lines::said(&text));
        windows.push((key, seen));
    }
    windows.into_iter().map(|(_, s)| s).collect()
}

impl Turn {
    /// The images the planner is given: each window's screenshot (messages read together) or this message's.
    pub fn planner_images(&self) -> Vec<ferb_core::types::ModelImage> {
        if !self.screens.is_empty() {
            self.screens
                .iter()
                .filter_map(|s| s.image.clone())
                .collect()
        } else {
            self.front_image().cloned().into_iter().collect()
        }
    }

    /// `turn.front_window`.
    pub fn front(&self) -> Option<&crate::planner::FrontWindow> {
        self.seen.as_ref().and_then(|s| s.front.as_ref())
    }

    /// `turn.front_image`.
    pub fn front_image(&self) -> Option<&ferb_core::types::ModelImage> {
        self.seen.as_ref().and_then(|s| s.image.as_ref())
    }

    /// `turn.fronts`: every front window this turn's planner was given (one per message's window).
    pub fn fronts(&self) -> Vec<&crate::planner::FrontWindow> {
        if self.screens.is_empty() {
            self.front().into_iter().collect()
        } else {
            self.screens
                .iter()
                .filter_map(|s| s.front.as_ref())
                .collect()
        }
    }
}

impl Inner {
    /// supervisor.py:2671 `_screen_at_send` (L12): the front window the daemon captured as the owner sent this message,
    /// and its redacted screenshot. A failed capture is journaled by type and gives "not available"; a capture that
    /// takes longer than FRONT_WINDOW_WAIT_S (DMN-520) is given up on and journaled: it never blocks the message.
    async fn screen_at_send(&self, request_id: &str) -> super::Seen {
        let wait = std::time::Duration::from_secs_f64(FRONT_WINDOW_WAIT_S);
        let got =
            tokio::time::timeout(wait, self.deps.daemon.front_window_capture(request_id)).await;
        let capture = match got {
            Err(_) => {
                self.journal(
                    "screen.capture_timeout",
                    json!({"request_id": request_id, "timeout_s": FRONT_WINDOW_WAIT_S}),
                    None,
                );
                return super::Seen {
                    missing: Some(CAPTURE_LATE.to_string()),
                    ..super::Seen::default()
                };
            }
            Ok(Err(class)) => {
                self.journal(
                    "screen.capture_failed",
                    json!({"request_id": request_id, "error": class}),
                    None,
                );
                None
            }
            Ok(Ok(capture)) => capture,
        };
        match capture {
            None => super::Seen::default(),
            Some(c) => {
                let image = c
                    .image
                    .filter(|i| !i.data.is_empty() && !c.window.image_ref.is_empty());
                super::Seen {
                    front: Some(c.window),
                    image,
                    ..super::Seen::default()
                }
            }
        }
    }
}
