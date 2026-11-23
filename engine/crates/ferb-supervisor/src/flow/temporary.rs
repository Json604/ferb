//! The temporary chat (owner 2026-11-23, ORCH-699; ended only by its icon, ORCH-700; how it is built, ORCH-701 and
//! ORCH-702).
//!
//! One conversation held only in the daemon's memory: its entries are made by the same projector the kept chat uses
//! ([`super::entries::ChatProjector`]) but never written to `chat_entries`, FERB.app's `chat_history` or a phone. It is
//! turned on and off only by the owner's icon (`chat.temporary.set`), or `chat.send` with `fresh` starts it; a
//! daemon restart ends it (nothing of it is kept to come back).
//!
//! **No prior context** ([`Fresh`]): every model role of one of its turns (planner, workers, checkers, the question
//! resolver, the conversation's fold) reads [`Inner::fresh_for`] by the request id, which carries the mark itself
//! (`temporary-…`, ferb-core `temporary`): no Tier 1 profile and nothing newly always on, no memory, no relevant
//! activity, no earlier chat or tasks, no lookups or tools that read memory, the activity trail or the chat history
//! ([`HIDDEN_TOOLS`], refused by the run token's tool list, so the gateway enforces it). Only the owner's words, the
//! code fact [`FRESH_FACT`] and the optional `extra_context` (owner's words, given through the switch) are added.
//! The safety path is unchanged: gateway, confirmations, sink rule, redaction, protection lists.
//!
//! Where each layer is cut, all by the same switch:
//! - the conversation: a temporary conversation is a reserved one (ferb-orchestrator `continuity::RESERVED_SESSIONS`,
//!   history.rs `reserved`), so it holds only its own messages and replies, never the kept chat's notices or FERB's
//!   own posts (a scheduled run's result, a reminder or timer that went off);
//! - the planner's live facts and every role's instructions (`Daemon::live_facts`' `owner_state`,
//!   `Daemon::system_inputs`' `fresh`): this Mac, its browsers and FERB's models stay; the owner's schedules,
//!   reminders and timers, the facts taken from memory and the Tier 1 profile go;
//! - the screen is not cut (ORCH-704): its message is captured at send like any message, the present and not memory;
//!   the front window and screenshot reach its planner and workers and are kept only where any capture is (the
//!   evidence store, the traces), never in its in-memory entries;
//! - open questions: only its own tasks' ([`Inner::question_in`]), for the planner and the question resolver.
//!
//! **Nothing remembered**: no chat entries, no `task_results` (so not in the Tasks page or chat.status' results, never
//! "since you last talked"), no `task.result` published, no memory side entries (remember/forget), no episodes, no
//! conversation index (ferb-memory `index_chat` skips its rows). The journal rows the supervisor works from while a
//! turn runs (its conversation for the planner, its tasks' context) are deleted by [`forget`] when the chat ends, when
//! a task of an ended chat finishes, and at every start. Kept, as for any task (rule 6 and the traces): the gateway's
//! ledger and confirmations, the journal's audit rows, `tasks` and their spans.

use std::collections::BTreeSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use ferb_core::temporary;
use rusqlite::Connection;
use serde_json::{json, Map, Value};

use super::entries::{ChatProjector, Input, Seams};
use super::Inner;

/// The bus topic every client hears when the temporary chat starts, changes or ends: `{on, id, rev}`.
pub const TOPIC: &str = "chat.temporary";

/// The planner's lookups a temporary chat keeps: the ones about FERB and this Mac, none that reads what FERB knows
/// (memory, the chat history, earlier tasks' evidence or traces, the activity trail).
pub const FRESH_PLANNER_TOOLS: [&str; 3] = ["about.read", "about.diagnostics", "device.facts"];

/// The tools a temporary chat's tasks are not given: they read FERB's memory, the chat history or the owner's activity
/// trail. Every other tool stays.
pub const HIDDEN_TOOLS: [&str; 5] = [
    "memory.read",
    "memory.learned",
    "chat.history",
    "trail.query",
    "trail.days",
];

/// The code fact every role of a temporary chat's turn reads first, so none promises to remember or looks for what it
/// was told earlier.
pub const FRESH_FACT: &str = "This is a temporary chat: you are given nothing FERB knew before it (no profile, \
memories, earlier chats or earlier tasks), and nothing from it is remembered or kept afterwards. Work only from the \
owner's words here, FERB's tools and what you find now; don't say you will remember anything.";

/// What the owner gave the temporary chat through its switch (their own words, ORCH-699's `extra_context`).
pub const EXTRA_CONTEXT_LABEL: &str = "What the owner gave this temporary chat to start from:\n";

/// One turn's switch: no prior context, and what the owner added (empty for now: the field is for later).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Fresh {
    pub extra_context: Option<String>,
    /// decision: ORCH-705 — a session driver's step (HAND-707): nothing is pushed to it either, but it keeps the
    /// lookups that read what FERB knows (memory, the chat history, the trail), since its goal may name an earlier
    /// conversation or decision; its own prompt says what it is, so it gets no temporary chat's fact; and code, not the
    /// step, types into the session (its token has no `agent_session.send` or `.answer`: [`DRIVER_HIDDEN_TOOLS`]).
    pub driver: bool,
}

/// decision: ORCH-705 — what a session driver's step is not given: typing into the session and answering its
/// questions are code's, from the step's decision (one message per round), through the gateway like any tool call.
pub const DRIVER_HIDDEN_TOOLS: [&str; 2] = ["agent_session.send", "agent_session.answer"];

/// The temporary chat, as the daemon holds it.
struct Chat {
    id: String,
    started_at: String,
    extra_context: Option<String>,
    projector: ChatProjector,
    seq: u64,
    requests: BTreeSet<String>,
}

/// The daemon's one temporary chat (none, or the one on), and the change number clients compare.
#[derive(Default)]
pub struct TemporaryChats {
    chat: Mutex<Option<Chat>>,
    rev: AtomicU64,
}

impl TemporaryChats {
    fn lock(&self) -> std::sync::MutexGuard<'_, Option<Chat>> {
        self.chat.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn bump(&self) -> u64 {
        self.rev.fetch_add(1, Ordering::SeqCst) + 1
    }

    /// The change number now.
    pub fn rev(&self) -> u64 {
        self.rev.load(Ordering::SeqCst)
    }
}

/// The projector's seams in memory: the daemon's clock and ids, the chat's own change counter.
struct InMemory<'a> {
    inner: &'a Inner,
    seq: &'a mut u64,
}

impl Seams for InMemory<'_> {
    fn now(&mut self) -> String {
        self.inner.now_iso()
    }
    fn new_id(&mut self) -> String {
        self.inner.new_id()
    }
    fn next_seq(&mut self) -> u64 {
        *self.seq += 1;
        *self.seq
    }
}

fn clean(extra: Option<&str>) -> Option<String> {
    extra
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(str::to_string)
}

impl Inner {
    /// `chat.temporary.get`: `{on: false}`, or the chat with its entries in creation order.
    pub fn temporary_snapshot(&self) -> Value {
        let guard = self.temporary.lock();
        let rev = self.temporary.rev();
        match guard.as_ref() {
            None => json!({"on": false}),
            Some(c) => {
                let entries: Vec<Value> = c
                    .projector
                    .entries()
                    .iter()
                    .map(|e| super::entries_rpc::entry_json(e, None))
                    .collect();
                let mut out = Map::new();
                out.insert("on".into(), json!(true));
                out.insert("id".into(), json!(c.id));
                out.insert("conversation".into(), json!(temporary::session_of(&c.id)));
                out.insert("started_at".into(), json!(c.started_at));
                out.insert("rev".into(), json!(rev));
                if let Some(extra) = &c.extra_context {
                    out.insert("extra_context".into(), json!(extra));
                }
                out.insert("entries".into(), Value::Array(entries));
                Value::Object(out)
            }
        }
    }

    /// What `chat.status` says of it: `{id, rev, started_at}` while one is on.
    pub fn temporary_status(&self) -> Option<Value> {
        let guard = self.temporary.lock();
        guard
            .as_ref()
            .map(|c| json!({"id": c.id, "rev": self.temporary.rev(), "started_at": c.started_at}))
    }

    /// The owner's icon on (`chat.temporary.set {on: true}`): the chat on now is kept (its `extra_context` replaced
    /// only when given), else a new one starts. Its snapshot.
    pub fn temporary_start(&self, extra_context: Option<&str>) -> Value {
        self.temporary_open(extra_context);
        self.temporary_snapshot()
    }

    /// The chat on now (started if none), with `extra_context` set when given: its id.
    fn temporary_open(&self, extra_context: Option<&str>) -> String {
        let (id, started, rev) = {
            let mut guard = self.temporary.lock();
            match guard.as_mut() {
                Some(c) => {
                    if let Some(extra) = extra_context {
                        c.extra_context = clean(Some(extra));
                    }
                    (c.id.clone(), false, self.temporary.rev())
                }
                None => {
                    let id = self.new_id();
                    *guard = Some(Chat {
                        id: id.clone(),
                        started_at: self.now_iso(),
                        extra_context: clean(extra_context),
                        projector: ChatProjector::new(&temporary::session_of(&id), 1),
                        seq: 0,
                        requests: BTreeSet::new(),
                    });
                    (id, true, self.temporary.bump())
                }
            }
        };
        if started {
            self.deps
                .daemon
                .publish(TOPIC, json!({"on": true, "id": id, "rev": rev}));
        }
        id
    }

    /// The owner's icon off (`chat.temporary.set {on: false}`): its running tasks are stopped, its conversation is
    /// dropped from memory and its working rows forgotten ([`forget`]). `{on: false}`.
    pub fn temporary_end(&self) -> Value {
        let ended = self.temporary.lock().take();
        if let Some(chat) = ended {
            let rev = self.temporary.bump();
            self.deps
                .daemon
                .publish(TOPIC, json!({"on": false, "id": chat.id, "rev": rev}));
            let trees: BTreeSet<String> = self
                .live()
                .tree_of
                .values()
                .filter(|t| temporary::is_request(&t.worker.request_id))
                .map(|t| t.id.clone())
                .collect();
            for tree in trees {
                self.stop_tree(&tree);
            }
            self.temporary_forget();
        }
        json!({"on": false})
    }

    /// A message for the temporary chat (`chat.send` with `fresh`): the chat (started if off, `extra_context` set
    /// when given), its conversation and the message's request id, kept as one of its messages.
    pub(crate) fn temporary_accept(&self, extra_context: Option<&str>) -> (String, String) {
        let id = self.temporary_open(extra_context);
        let request_id = temporary::request_of(&self.new_id());
        if let Some(c) = self.temporary.lock().as_mut() {
            c.requests.insert(request_id.clone());
        }
        (temporary::session_of(&id), request_id)
    }

    /// One input for the temporary chat's entries (a frame of one of its streams, its end, a bus event about its
    /// tasks). Dropped when its chat is no longer on.
    pub(crate) fn temporary_feed(&self, input: Input<'_>) {
        let rev = {
            let mut guard = self.temporary.lock();
            let Some(c) = guard.as_mut() else {
                return;
            };
            let mut seams = InMemory {
                inner: self,
                seq: &mut c.seq,
            };
            let made = c.projector.apply(input, &mut seams);
            if made.changes.is_empty() {
                return;
            }
            (c.id.clone(), self.temporary.bump())
        };
        self.deps
            .daemon
            .publish(TOPIC, json!({"on": true, "id": rev.0, "rev": rev.1}));
    }

    /// The request is one of the temporary chat on now.
    pub(crate) fn temporary_holds(&self, request_id: &str) -> bool {
        self.temporary
            .lock()
            .as_ref()
            .is_some_and(|c| c.requests.contains(request_id))
    }

    /// decision: ORCH-699 — the one switch every route reads: a temporary chat's turn gets no prior context. None for
    /// any other request.
    pub fn fresh_for(&self, request_id: &str) -> Option<Fresh> {
        // decision: ORCH-705 — a session driver's step: the same switch, keeping its lookups
        if ferb_core::driver::is_request(request_id) {
            return Some(Fresh {
                extra_context: None,
                driver: true,
            });
        }
        if !temporary::is_request(request_id) {
            return None;
        }
        let extra_context = self
            .temporary
            .lock()
            .as_ref()
            .filter(|c| c.requests.contains(request_id))
            .and_then(|c| c.extra_context.clone());
        Some(Fresh {
            extra_context,
            driver: false,
        })
    }

    /// decision: ORCH-699 — an open question (by its task) a conversation's turn may see and answer: any in the kept
    /// chat, as before; in a temporary chat only its own tasks' questions.
    pub(crate) fn question_in(&self, session: &str, task_id: &str) -> bool {
        if !temporary::is_session(session) {
            return true;
        }
        let mine = self.continuity(|c| c.task_ids(session)).unwrap_or_default();
        !task_id.is_empty() && mine.contains(&self.question_root(task_id))
    }

    /// The task (any of a tree's) is a temporary chat's: by its live tree's request, else the stored task's.
    pub(crate) fn temporary_task(&self, task_id: &str) -> bool {
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
        request.is_some_and(|r| temporary::is_request(&r))
    }

    /// A bus event (task.result, task.part, approval.*) about a temporary chat's message or task. `approval.resolved`
    /// names only its approval (`{approval_id, state}`): it is the temporary chat's when that chat asked it.
    pub(crate) fn temporary_event(&self, data: &Value) -> bool {
        let named = |k: &str| {
            data.get(k)
                .filter(|v| ferb_py::value::truthy(v))
                .map(ferb_py::value::py_str)
        };
        if named("request_id").is_some_and(|r| temporary::is_request(&r)) {
            return true;
        }
        if named("task_id").is_some_and(|t| self.temporary_task(&t)) {
            return true;
        }
        // decision: ORCH-702 — an answer to its own question closes that question in its memory (every client folds
        // the entry), never routed to the kept chat, which never held it
        named("approval_id").is_some_and(|a| {
            self.temporary
                .lock()
                .as_ref()
                .is_some_and(|c| c.projector.holds_question(&a))
        })
    }

    /// [`forget`] on the store, keeping the rows of the chat on now; a failure is journaled, never raised.
    pub fn temporary_forget(&self) {
        let keep: BTreeSet<String> = self
            .temporary
            .lock()
            .as_ref()
            .map(|c| c.requests.clone())
            .unwrap_or_default();
        let result = {
            let db = self.db();
            forget(db.conn(), &keep)
        };
        if let Err(e) = result {
            self.journal(
                "temporary.forget_failed",
                json!({"error": e.to_string()}),
                None,
            );
        }
    }

    /// The context block of a fresh turn: [`FRESH_FACT`], then the owner's `extra_context` when given.
    pub(crate) fn fresh_block(&self, fresh: &Fresh) -> ferb_context::assemble::ContextBlock {
        let mut text = FRESH_FACT.to_string();
        if let Some(extra) = &fresh.extra_context {
            text.push_str("\n\n");
            text.push_str(EXTRA_CONTEXT_LABEL);
            text.push_str(extra);
        }
        let mut block = ferb_context::assemble::ContextBlock::new(text, "temporary chat");
        block.protected = true;
        block.origin = ferb_sanitize::origin::OWNER.to_string();
        block
    }
}

/// The conversation rows a temporary chat's messages left in the journal (its messages, FERB's replies and questions,
/// its tasks' answers, plans and parts, its running summaries), its tasks' context, results and episodes, and its
/// conversation's continuity rows, except those of the requests in `keep` (the chat on now) and of its conversation.
/// How many rows went. The journal's delete trigger takes any index copy or vector with them. Kept: `tasks`, spans,
/// and the journal's other rows (audit, rule 6).
pub fn forget(conn: &Connection, keep: &BTreeSet<String>) -> rusqlite::Result<usize> {
    let request_like = format!("{}%", temporary::REQUEST_PREFIX);
    let session_like = format!("{}%", temporary::SESSION_PREFIX);
    let text = |v: &Value, k: &str| v.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    // the temporary chats' tasks, by their request
    let mut tasks: Vec<String> = Vec::new();
    {
        let mut stmt = conn.prepare("SELECT id, request_id FROM tasks WHERE request_id LIKE ?")?;
        let rows = stmt.query_map([&request_like], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })?;
        for row in rows {
            let (id, request) = row?;
            if !keep.contains(&request) {
                tasks.push(id);
            }
        }
    }
    // the conversation the chat on now writes in (from one of its messages' acks)
    let mut live_sessions: BTreeSet<String> = BTreeSet::new();
    for r in keep {
        let s: Option<String> = conn
            .query_row(
                "SELECT json_extract(data_json, '$.session_id') FROM journal WHERE kind = 'chat.ack' \
                 AND json_valid(data_json) AND json_extract(data_json, '$.request_id') = ? LIMIT 1",
                [r],
                |row| row.get(0),
            )
            .unwrap_or(None);
        live_sessions.extend(s);
    }
    let kinds = ferb_store::data_view::CONTENT_KINDS
        .iter()
        .map(|k| format!("'{k}'"))
        .collect::<Vec<_>>()
        .join(",");
    let mut doomed: Vec<String> = Vec::new();
    {
        let by_task = if tasks.is_empty() {
            String::new()
        } else {
            format!(
                " OR json_extract(data_json, '$.task_id') IN ({})",
                vec!["?"; tasks.len()].join(",")
            )
        };
        let mut stmt = conn.prepare(&format!(
            "SELECT id, data_json FROM journal WHERE kind IN ({kinds}) AND json_valid(data_json) AND \
             (json_extract(data_json, '$.request_id') LIKE ? OR json_extract(data_json, '$.session_id') LIKE ?{by_task})"
        ))?;
        let mut args = vec![request_like.clone(), session_like.clone()];
        args.extend(tasks.iter().cloned());
        let rows = stmt.query_map(rusqlite::params_from_iter(args.iter()), |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })?;
        for row in rows {
            let (id, data) = row?;
            let data: Value = serde_json::from_str(&data).unwrap_or(Value::Null);
            let (request, session, task) = (
                text(&data, "request_id"),
                text(&data, "session_id"),
                text(&data, "task_id"),
            );
            let ours = temporary::is_request(&request)
                || temporary::is_session(&session)
                || tasks.contains(&task);
            let kept = keep.contains(&request) || live_sessions.contains(&session);
            if ours && !kept {
                doomed.push(id);
            }
        }
    }
    let mut gone = 0;
    for id in &doomed {
        gone += conn.execute("DELETE FROM journal WHERE id = ?", [id])?;
    }
    for task in &tasks {
        for sql in [
            "DELETE FROM task_result_cards WHERE task_id = ?",
            "DELETE FROM task_result_names WHERE task_id = ?",
            "DELETE FROM task_results WHERE task_id = ?",
            "DELETE FROM task_context WHERE task_id = ?",
            "DELETE FROM episodes WHERE task_id = ?",
        ] {
            gone += conn.execute(sql, [task])?;
        }
    }
    // the ended chats' conversations (the chat on now keeps its own)
    let ended: Vec<String> = {
        let mut stmt = conn.prepare("SELECT id FROM conversations WHERE id LIKE ?")?;
        let rows = stmt.query_map([&session_like], |r| r.get::<_, String>(0))?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
            .into_iter()
            .filter(|c| !live_sessions.contains(c))
            .collect()
    };
    for conversation in &ended {
        gone += conn.execute(
            "DELETE FROM task_context WHERE conversation_id = ?",
            [conversation],
        )?;
        gone += conn.execute("DELETE FROM conversations WHERE id = ?", [conversation])?;
    }
    Ok(gone)
}
