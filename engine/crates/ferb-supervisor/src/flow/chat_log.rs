//! The chat as entries, kept (stage 2, slice 4; ENG-8, ENG-9, ENG-10): [`super::entries::ChatProjector`] wired where
//! the engine produces what a client sees, writing `chat_entries` (ferb-store's `chat_entries`) as it goes, and handing
//! each committed change and each live frame to the clients watching (`chat.entries.watch`).
//!
//! **One order.** Everything the projector reads goes through one queue, in the order it happened: a `chat.send`
//! stream's frames as `chat.rs` hands them to the stream (the same call that sends the frame queues it, so an entry's
//! `seq` order is the order of the frames a client of that stream saw: journal 70's bursts are already applied there),
//! the stream's end, and the bus events no stream carries (`task.result` of a task no open stream reads, `task.part`,
//! `approval.requested`, `approval.resolved`, `attention.notice`) as they are published, and a reminder's post. The
//! queue is drained under one lock, each drain in one write transaction that bumps the conversation's counter for every
//! change (order = commit order, never wall-clock). A stream's ack is drained synchronously (everything queued before
//! it first), in one transaction with its `chat.ack` journal row and, when the client named one, its submission row.
//! Everything else only queues, on the path that produced it, and a writer task drains the queue a moment later
//! ([`GRACE_MS`]): queuing takes no store lock, so the engine's own order of writes (ids, journal rows) is the one it
//! had before the entries existed, and a publisher that holds the store's lock never waits on it.
//!
//! **Bus events a stream carries.** `approval.requested` and `task.part` about a request whose stream is open ride
//! that stream (chat.rs `take`): their bus copies are held and applied at the stream's end, so a question a stream
//! did not carry is still posted (once: the projector asks once per approval) and nothing is fed twice in another
//! order; `task.result` of an outcome the stream carried is fed by `take` right after its outcome frame (chat.rs
//! publishes it there), so its bus copy is left out.
//!
//! **Routing.** A frame to its stream's conversation; a bus event by its request, then by its task (the task's
//! request, the conversation the task was given), then to the current conversation (`current_conversation`, else
//! `owner`); an answer to a question to the conversation that holds the question. Routing reads the store directly
//! inside the drain's transaction (never the supervisor's live state: its lock is taken elsewhere with the store's).
//!
//! **After a restart.** A conversation's projector is rebuilt from its stored entries the first time it is used
//! ([`ChatProjector::restore`]); [`ChatLog::after_restart`] runs once at start, before the daemon serves: the journal
//! backstop first (a `chat.ack` after the stage-2 watermark with no message entry gets one, with its `chat.reply`; a
//! message with no answer whose `chat.reply` was journaled gets that answer: a crash between the two writes), then each
//! conversation's start-up pass (open work whose task did not resume is `interrupted`, open questions expire).

use std::collections::{BTreeSet, HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use ferb_core::chat_entry::{Change, Entry};
use ferb_core::interfaces::IdSource;
use ferb_store::chat_entries as store;
use rusqlite::{Connection, OptionalExtension};
use serde_json::{json, Map, Value};
use tokio::sync::mpsc;

use super::entries::{ChatProjector, Input, Seams};
use super::Inner;

/// The entries a rebuilt projector reads beyond the open ones: the conversation's last ones (the chat's end, the
/// newest turns a late answer may still reach).
pub const RESTORE_RECENT: usize = 60;
/// How long the writer waits after a wake before it drains (milliseconds).
pub const GRACE_MS: u64 = 10;
/// How many changes a watcher may have waiting before it is told it is behind and its stream ends.
pub const WATCH_QUEUE: usize = 2048;
/// The bus topics the log reads (ENG-9's inputs; the rest of the bus is live state a stream carries).
pub const TOPICS: [&str; 5] = [
    "task.result",
    "task.part",
    "approval.requested",
    "approval.resolved",
    "attention.notice",
];

/// One thing queued for the projector.
#[derive(Debug, Clone)]
enum Queued {
    Frame {
        conversation: String,
        request_id: String,
        frame: Value,
    },
    Ended {
        conversation: String,
        request_id: String,
    },
    /// A bus event; `fed`: chat.rs fed it itself (a stream's own `task.result`).
    Bus {
        topic: String,
        data: Value,
        fed: bool,
    },
    Post(Value),
}

/// What a watcher is handed: a committed change, or a live frame of a conversation (never stored).
#[derive(Debug, Clone)]
pub enum Watched {
    Change(Box<Entry>),
    Live {
        request_id: Option<String>,
        frame: Value,
    },
}

struct Watcher {
    id: u64,
    conversation: String,
    tx: mpsc::Sender<Watched>,
    behind: Arc<AtomicBool>,
}

/// One watcher's end of the feed.
pub struct Watch {
    pub id: u64,
    pub rx: mpsc::Receiver<Watched>,
    /// Set when the watcher fell behind (its queue was full): the feed ended for it.
    pub behind: Arc<AtomicBool>,
}

#[derive(Default)]
struct State {
    projectors: HashMap<String, ChatProjector>,
    /// request -> conversation of every stream still open.
    open: HashMap<String, String>,
    /// request -> the bus events about it held while its stream is open.
    held: HashMap<String, Vec<(String, Value)>>,
}

/// The wired projector (one per daemon, inside the supervisor).
pub struct ChatLog {
    queue: Mutex<VecDeque<Queued>>,
    state: Mutex<State>,
    wake: Arc<tokio::sync::Notify>,
    writer: AtomicBool,
    ids: Mutex<Arc<dyn IdSource>>,
    watchers: Mutex<Vec<Watcher>>,
    next_watcher: AtomicU64,
    trace: Mutex<Option<Trace>>,
    /// The trace's items of the drain under way, handed to the trace once it is committed (never inside the store's
    /// transaction: the trace must not change how long the drain holds the store).
    traced: Mutex<Vec<Value>>,
}

/// Where a trace of the log's work goes (tests only: ferbd's stand-ins write it to a file).
pub type Trace = Arc<dyn Fn(&Value) + Send + Sync>;

impl Default for ChatLog {
    fn default() -> Self {
        ChatLog {
            queue: Mutex::new(VecDeque::new()),
            state: Mutex::new(State::default()),
            wake: Arc::new(tokio::sync::Notify::new()),
            writer: AtomicBool::new(false),
            ids: Mutex::new(Arc::new(ferb_core::ids::RandomIds)),
            watchers: Mutex::new(Vec::new()),
            next_watcher: AtomicU64::new(1),
            trace: Mutex::new(None),
            traced: Mutex::new(Vec::new()),
        }
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// What a drain made: per conversation, the changes and live frames to hand the watchers, in order.
type Made = Vec<(String, Watched)>;

/// What the ack's transaction gives back.
#[derive(Debug, Clone, PartialEq)]
pub struct Accepted {
    pub entry_id: String,
    pub seq: u64,
    pub epoch: String,
}

/// A submission already accepted, or refused (another message under the same id).
#[derive(Debug, Clone, PartialEq)]
pub enum Repeat {
    Same(store::Submission),
    Conflict,
}

impl ChatLog {
    /// Entry ids from `ids` (tests inject a counter; the daemon's own id source is never used, so recorded runs keep
    /// their ids).
    pub fn set_ids(&self, ids: Arc<dyn IdSource>) {
        *lock(&self.ids) = ids;
    }

    fn ids(&self) -> Arc<dyn IdSource> {
        lock(&self.ids).clone()
    }

    /// Every input the projectors are fed, in the order they are fed it, with the conversation it went to and the
    /// changes it made; a bus event held for its stream's end or left out; the start-up pass's changes. Tests only (the
    /// stand-ins' order check: the engine's real interleaving, `ferbd/tests/chat_replay.rs`); unset, nothing is kept.
    pub fn set_trace(&self, trace: Trace) {
        *lock(&self.trace) = Some(trace);
    }

    fn traced(&self, item: impl FnOnce() -> Value) {
        if lock(&self.trace).is_some() {
            lock(&self.traced).push(item());
        }
    }

    /// The drain's trace items, to the trace (after its transaction ended).
    fn hand_trace(&self) {
        let sink = lock(&self.trace).clone();
        if let Some(sink) = sink {
            for item in std::mem::take(&mut *lock(&self.traced)) {
                sink(&item);
            }
        }
    }

    // ---- inputs ----

    /// A frame of a stream (chat.rs `take`, just before the frame goes to the stream), queued in that order.
    pub fn frame(&self, inner: &Arc<Inner>, conversation: &str, request_id: &str, frame: &Value) {
        self.push(Queued::Frame {
            conversation: conversation.to_string(),
            request_id: request_id.to_string(),
            frame: frame.clone(),
        });
        self.kick(inner);
    }

    /// A stream's own `task.result`, right after its outcome frame.
    pub fn stream_result(&self, inner: &Arc<Inner>, data: &Value) {
        self.push(Queued::Bus {
            topic: "task.result".into(),
            data: data.clone(),
            fed: true,
        });
        self.kick(inner);
    }

    /// The stream's turn returned (or the daemon shut down).
    pub fn ended(&self, inner: &Arc<Inner>, conversation: &str, request_id: &str) {
        self.push(Queued::Ended {
            conversation: conversation.to_string(),
            request_id: request_id.to_string(),
        });
        self.kick(inner);
    }

    /// A bus event as published: queued, drained by the writer task (never here: the publisher may hold the store).
    pub fn bus(&self, inner: &Arc<Inner>, topic: &str, data: Value) {
        self.push(Queued::Bus {
            topic: topic.to_string(),
            data,
            fed: false,
        });
        self.kick(inner);
    }

    /// A reminder's banner as FERB.app's notifications stream carries it (posted in the chat once).
    pub fn post(&self, inner: &Arc<Inner>, item: Value) {
        self.push(Queued::Post(inner.redact(&item)));
        self.kick(inner);
    }

    fn push(&self, item: Queued) {
        lock(&self.queue).push_back(item);
    }

    /// Start the writer task on this (the daemon's) runtime; [`ChatLog::after_restart`] does it at start.
    pub fn start(&self, inner: &Arc<Inner>) {
        self.kick(inner);
    }

    /// Wake the writer task, starting it on the first call made inside the daemon's multi-thread runtime (never on a
    /// group's own loop thread).
    fn kick(&self, inner: &Arc<Inner>) {
        if !self.writer.load(Ordering::SeqCst) {
            let rt = tokio::runtime::Handle::try_current()
                .ok()
                .filter(|rt| rt.runtime_flavor() == tokio::runtime::RuntimeFlavor::MultiThread);
            if let Some(rt) = rt {
                if !self.writer.swap(true, Ordering::SeqCst) {
                    let me = Arc::downgrade(inner);
                    let wake = self.wake.clone();
                    rt.spawn(async move {
                        // a wake given while nobody waits is kept (Notify's permit): none is lost
                        loop {
                            wake.notified().await;
                            // a moment's grace: the work that queued it finishes its own writes first (the store's lock
                            // is never taken in the middle of a publish's consumers)
                            tokio::time::sleep(std::time::Duration::from_millis(GRACE_MS)).await;
                            let Some(inner) = me.upgrade() else { break };
                            inner.chat_log.drain(&inner);
                        }
                    });
                }
            }
        }
        self.wake.notify_one();
    }

    /// Everything queued, applied and written now.
    pub fn flush(&self, inner: &Inner) {
        self.drain(inner);
    }

    fn drain(&self, inner: &Inner) {
        if lock(&self.queue).is_empty() {
            return; // a drain under way takes whatever was queued
        }
        let _ = self.run(inner, |_, _| Ok(()));
    }

    /// Everything queued, in order, then `then`, all in one write transaction; the watchers get what it made once it
    /// is committed. A failed transaction drops every projector (each is rebuilt from the store when next used) and is
    /// journaled.
    fn run<T>(
        &self,
        inner: &Inner,
        then: impl FnOnce(&Connection, &mut Ctx<'_>) -> Result<T, ferb_store::Error>,
    ) -> Result<T, ferb_store::Error> {
        let mut st = lock(&self.state);
        let mut made = Made::new();
        let ids = self.ids();
        let result = {
            let db = inner.db();
            db.transaction(|conn| {
                let mut ctx = Ctx {
                    st: &mut st,
                    inner,
                    ids: ids.as_ref(),
                    made: &mut made,
                };
                loop {
                    let next = lock(&self.queue).pop_front();
                    let Some(item) = next else { break };
                    ctx.apply(conn, item)?;
                }
                then(conn, &mut ctx)
            })
        };
        self.hand_trace();
        match result {
            Ok(v) => {
                drop(st);
                self.hand_over(made);
                Ok(v)
            }
            Err(e) => {
                st.projectors.clear();
                drop(st);
                inner.journal("chat.entries_failed", json!({"error": e.to_string()}), None);
                Err(e)
            }
        }
    }

    // ---- the ack ----

    /// A message accepted (chat.rs's ack), in ONE transaction with its `chat.ack` journal row (`journal`, written by
    /// `write_journal` on the same connection) and, when the client named one, its submission: the message entry made
    /// from the ack frame (with the message's files added: the ack carries none). `Err(Repeat)` when the client's
    /// submission id was already accepted (nothing written).
    #[allow(clippy::too_many_arguments)]
    pub fn accept(
        &self,
        inner: &Inner,
        conversation: &str,
        request_id: &str,
        ack: &Value,
        input_paths: &[String],
        client: Option<&str>,
        submission: Option<(&str, &str, &str)>,
        write_journal: impl FnOnce(&Connection) -> Result<(), ferb_store::Error>,
    ) -> Result<Result<Accepted, Repeat>, ferb_store::Error> {
        // what the engine knows at its ack and the frame doesn't carry: the message's files, its client's ids
        let mut frame = ack.clone();
        frame["input_paths"] = json!(input_paths);
        if let Some(client) = client {
            frame["client_id"] = json!(client);
        }
        if let Some((_, sub, _)) = submission {
            frame["submission_id"] = json!(sub);
        }
        self.run(inner, |conn, ctx| {
            if let Some((client, sub, sha)) = submission {
                match store::check_submission(conn, client, sub, sha)? {
                    store::Accept::New => {}
                    store::Accept::Repeat(s) => return Ok(Err(Repeat::Same(s))),
                    store::Accept::Conflict(_) => return Ok(Err(Repeat::Conflict)),
                }
            }
            write_journal(conn)?;
            ctx.st
                .open
                .insert(request_id.to_string(), conversation.to_string());
            let changes = ctx.feed(
                conn,
                conversation,
                Input::Frame {
                    request_id,
                    frame: &frame,
                },
            )?;
            let message = changes
                .iter()
                .find(|c| {
                    c.entry.kind == ferb_core::chat_entry::EntryKind::Message
                        && c.entry.request_id.as_deref() == Some(request_id)
                })
                .map(|c| c.entry.clone());
            let stream = store::stream(conn, conversation)?;
            let (Some(message), Some(stream)) = (message, stream) else {
                return Err(ferb_store::Error::Value(
                    "the message was not recorded".into(),
                ));
            };
            if let Some(client) = client {
                store::touch_client(conn, client, &ctx.inner.now_iso())?;
            }
            if let Some((client, sub, sha)) = submission {
                store::record_submission(
                    conn,
                    &store::Submission {
                        client_id: client.to_string(),
                        submission_id: sub.to_string(),
                        conversation_id: conversation.to_string(),
                        request_id: request_id.to_string(),
                        entry_id: message.id.clone(),
                        seq: message.seq,
                        text_sha256: sha.to_string(),
                        accepted_at: ack
                            .get("at")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                    },
                )?;
            }
            Ok(Ok(Accepted {
                entry_id: message.id,
                seq: message.seq,
                epoch: stream.epoch,
            }))
        })
    }

    // ---- after a restart ----

    /// Once at start, before the daemon serves: the journal backstop, then every conversation's start-up pass
    /// (`resumed`: the trees that will run again).
    pub fn after_restart(&self, inner: &Inner, resumed: &BTreeSet<String>) {
        let _ = self.run(inner, |conn, ctx| {
            ctx.backstop(conn)?;
            for conversation in store::conversations(conn)? {
                ctx.projector(conn, &conversation)?;
                let p = ctx.st.projectors.get_mut(&conversation).expect("loaded");
                let mut seams = Stored::new(conn, &conversation, ctx.inner, ctx.ids);
                let changes = p.after_restart(&|t| resumed.contains(t), &mut seams);
                seams.ok()?;
                self.traced(|| {
                    json!({"conversation": conversation, "input": {"do": "restart"},
                           "changes": changes.iter().map(|c| &c.entry).collect::<Vec<_>>()})
                });
                ctx.write(conn, &conversation, changes)?;
            }
            Ok(())
        });
    }

    // ---- the owner's delete (STORE-424) ----

    /// Whether a stream is still open for this message (FERB is still answering it).
    pub fn is_open(&self, request_id: &str) -> bool {
        lock(&self.state).open.contains_key(request_id)
    }

    /// `work` in the log's write transaction, after everything queued: the entries it revised (tombstones) go to the
    /// watchers once committed, and their conversations' projectors are dropped (rebuilt from the store when next
    /// used), so nothing held in memory writes a deleted entry back.
    pub fn rewrite<T>(
        &self,
        inner: &Inner,
        work: impl FnOnce(&Connection) -> Result<(T, Vec<Entry>), ferb_store::Error>,
    ) -> Result<T, ferb_store::Error> {
        self.run(inner, |conn, ctx| {
            let (value, revised) = work(conn)?;
            for e in revised {
                ctx.st.projectors.remove(&e.conversation_id);
                ctx.made
                    .push((e.conversation_id.clone(), Watched::Change(Box::new(e))));
            }
            Ok(value)
        })
    }

    // ---- watchers ----

    /// A new watcher of one conversation: everything committed from now on, and the live frames.
    pub fn watch(&self, conversation: &str) -> Watch {
        let (tx, rx) = mpsc::channel(WATCH_QUEUE);
        let id = self.next_watcher.fetch_add(1, Ordering::SeqCst);
        let behind = Arc::new(AtomicBool::new(false));
        lock(&self.watchers).push(Watcher {
            id,
            conversation: conversation.to_string(),
            tx,
            behind: behind.clone(),
        });
        Watch { id, rx, behind }
    }

    /// The watcher left.
    pub fn unwatch(&self, id: u64) {
        lock(&self.watchers).retain(|w| w.id != id);
    }

    /// How many watchers are on now.
    pub fn watchers(&self) -> usize {
        lock(&self.watchers).len()
    }

    fn hand_over(&self, made: Made) {
        if made.is_empty() {
            return;
        }
        let mut watchers = lock(&self.watchers);
        watchers.retain(|w| {
            for (conversation, item) in &made {
                if *conversation != w.conversation {
                    continue;
                }
                match w.tx.try_send(item.clone()) {
                    Ok(()) => {}
                    Err(mpsc::error::TrySendError::Full(_)) => {
                        w.behind.store(true, Ordering::SeqCst);
                        return false;
                    }
                    Err(mpsc::error::TrySendError::Closed(_)) => return false,
                }
            }
            true
        });
    }
}

/// The projector's seams inside a drain's transaction: the clock, the log's ids, and the conversation's counter.
struct Stored<'a> {
    conn: &'a Connection,
    conversation: String,
    inner: &'a Inner,
    ids: &'a dyn IdSource,
    failed: Option<ferb_store::Error>,
}

impl<'a> Stored<'a> {
    fn new(
        conn: &'a Connection,
        conversation: &str,
        inner: &'a Inner,
        ids: &'a dyn IdSource,
    ) -> Self {
        Stored {
            conn,
            conversation: conversation.to_string(),
            inner,
            ids,
            failed: None,
        }
    }

    fn ok(self) -> Result<(), ferb_store::Error> {
        match self.failed {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }
}

impl Seams for Stored<'_> {
    fn now(&mut self) -> String {
        self.inner.now_iso()
    }
    fn new_id(&mut self) -> String {
        self.ids.new_id()
    }
    fn next_seq(&mut self) -> u64 {
        match store::next_seq(self.conn, &self.conversation) {
            Ok(s) => s,
            Err(e) => {
                self.failed.get_or_insert(e);
                0
            }
        }
    }
}

/// One drain's working state.
struct Ctx<'a> {
    st: &'a mut State,
    inner: &'a Inner,
    ids: &'a dyn IdSource,
    made: &'a mut Made,
}

impl Ctx<'_> {
    /// The conversation's projector, rebuilt from the store the first time.
    fn projector(
        &mut self,
        conn: &Connection,
        conversation: &str,
    ) -> Result<(), ferb_store::Error> {
        if self.st.projectors.contains_key(conversation) {
            return Ok(());
        }
        let stream = match store::stream(conn, conversation)? {
            Some(s) => s,
            None => store::open_stream(conn, conversation, &store::new_epoch())?,
        };
        let mut stored = store::open_entries(conn, conversation)?;
        let requests: Vec<String> = stored.iter().filter_map(|e| e.request_id.clone()).collect();
        let requests: Vec<&str> = requests.iter().map(String::as_str).collect();
        stored.extend(store::of_requests(conn, conversation, &requests)?);
        stored.extend(store::recent(conn, conversation, RESTORE_RECENT)?);
        let refs = store::refs(conn, conversation)?;
        let p = ChatProjector::restore(conversation, stored, refs, stream.last_pos + 1);
        self.st.projectors.insert(conversation.to_string(), p);
        Ok(())
    }

    /// One input to one conversation's projector, its changes written; the changes made.
    fn feed(
        &mut self,
        conn: &Connection,
        conversation: &str,
        input: Input<'_>,
    ) -> Result<Vec<Change>, ferb_store::Error> {
        self.projector(conn, conversation)?;
        let p = self
            .st
            .projectors
            .get_mut(conversation)
            .expect("the projector was loaded");
        let mut seams = Stored::new(conn, conversation, self.inner, self.ids);
        let out = p.apply(input, &mut seams);
        seams.ok()?;
        self.inner.chat_log.traced(|| {
            json!({"conversation": conversation, "input": traced_input(&input),
                   "changes": out.changes.iter().map(|c| &c.entry).collect::<Vec<_>>(),
                   "live": out.live.is_some()})
        });
        if let Some(live) = out.live {
            let request_id = match input {
                Input::Frame { request_id, .. } => Some(request_id.to_string()),
                _ => None,
            };
            self.made.push((
                conversation.to_string(),
                Watched::Live {
                    request_id,
                    frame: live,
                },
            ));
        }
        self.write(conn, conversation, out.changes.clone())?;
        Ok(out.changes)
    }

    fn write(
        &mut self,
        conn: &Connection,
        conversation: &str,
        changes: Vec<Change>,
    ) -> Result<(), ferb_store::Error> {
        for c in changes {
            store::put(conn, &c.entry)?;
            self.made
                .push((conversation.to_string(), Watched::Change(Box::new(c.entry))));
        }
        Ok(())
    }

    fn apply(&mut self, conn: &Connection, item: Queued) -> Result<(), ferb_store::Error> {
        match item {
            Queued::Frame {
                conversation,
                request_id,
                frame,
            } => {
                self.feed(
                    conn,
                    &conversation,
                    Input::Frame {
                        request_id: &request_id,
                        frame: &frame,
                    },
                )?;
            }
            Queued::Ended {
                conversation,
                request_id,
            } => {
                self.st.open.remove(&request_id);
                for (topic, data) in self.st.held.remove(&request_id).unwrap_or_default() {
                    let to = self.route(conn, &topic, &data)?;
                    if let Some(to) = to {
                        self.feed(
                            conn,
                            &to,
                            Input::Bus {
                                topic: &topic,
                                data: &data,
                            },
                        )?;
                    }
                }
                self.feed(
                    conn,
                    &conversation,
                    Input::StreamEnded {
                        request_id: &request_id,
                    },
                )?;
            }
            Queued::Bus { topic, data, fed } => {
                let request = data
                    .get("request_id")
                    .and_then(Value::as_str)
                    .map(str::to_string);
                let streamed = request.as_ref().filter(|r| self.st.open.contains_key(*r));
                if let (Some(r), false) = (streamed, fed) {
                    match topic.as_str() {
                        // the stream fed it itself, right after its outcome
                        "task.result" => {
                            self.inner
                                .chat_log
                                .traced(|| json!({"left_out": {"topic": topic, "data": data}}));
                            return Ok(());
                        }
                        // the stream carries it; held for its end
                        "task.part" | "approval.requested" => {
                            self.inner
                                .chat_log
                                .traced(|| json!({"held": {"topic": topic, "data": data}}));
                            self.st
                                .held
                                .entry(r.clone())
                                .or_default()
                                .push((topic, data));
                            return Ok(());
                        }
                        _ => {}
                    }
                }
                let data = if topic == "approval.resolved" {
                    answered(conn, &data)?
                } else {
                    data
                };
                if let Some(to) = self.route(conn, &topic, &data)? {
                    self.feed(
                        conn,
                        &to,
                        Input::Bus {
                            topic: &topic,
                            data: &data,
                        },
                    )?;
                } else {
                    self.inner
                        .chat_log
                        .traced(|| json!({"unrouted": {"topic": topic, "data": data}}));
                }
            }
            Queued::Post(item) => {
                let to = current(conn)?;
                self.feed(conn, &to, Input::Notification(&item))?;
            }
        }
        Ok(())
    }

    /// The conversation a bus event belongs to (None: an answer to a question no conversation holds).
    fn route(
        &mut self,
        conn: &Connection,
        topic: &str,
        data: &Value,
    ) -> Result<Option<String>, ferb_store::Error> {
        let text = |k: &str| {
            data.get(k)
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
        };
        if topic == "approval.resolved" {
            let Some(a) = text("approval_id") else {
                return Ok(None);
            };
            return Ok(conn
                .query_row(
                    "SELECT conversation_id FROM chat_entries WHERE approval_id = ? AND kind = 'question' \
                     ORDER BY pos DESC LIMIT 1",
                    [&a],
                    |r| r.get::<_, String>(0),
                )
                .optional()?);
        }
        if let Some(r) = text("request_id") {
            if let Some(c) = self.of_request(conn, &r)? {
                return Ctx::chat_of(conn, Some(c));
            }
        }
        if let Some(t) = text("task_id") {
            if let Some(c) = self.of_task(conn, &t)? {
                return Ctx::chat_of(conn, Some(c));
            }
        }
        Ok(Some(current(conn)?))
    }

    /// A conversation FERB's own work runs in (a schedule's run, the intro's preview) is not a chat: what is told
    /// there goes to the owner's chat.
    fn chat_of(
        conn: &Connection,
        found: Option<String>,
    ) -> Result<Option<String>, ferb_store::Error> {
        match found {
            Some(c) if c.starts_with("schedule:") || c.starts_with("preview:") => {
                Ok(Some(current(conn)?))
            }
            other => Ok(other),
        }
    }

    fn of_request(
        &self,
        conn: &Connection,
        request: &str,
    ) -> Result<Option<String>, ferb_store::Error> {
        if let Some(c) = self.st.open.get(request) {
            return Ok(Some(c.clone()));
        }
        if let Some((c, _)) = self
            .st
            .projectors
            .iter()
            .find(|(_, p)| p.knows_request(request))
        {
            return Ok(Some(c.clone()));
        }
        store::conversation_of_request(conn, request)
    }

    fn of_task(&self, conn: &Connection, task: &str) -> Result<Option<String>, ferb_store::Error> {
        if let Some((c, _)) = self.st.projectors.iter().find(|(_, p)| p.knows_task(task)) {
            return Ok(Some(c.clone()));
        }
        if let Some(c) = store::conversation_of_task(conn, task)? {
            return Ok(Some(c));
        }
        let row: Option<(String, String)> = conn
            .query_row(
                "SELECT tree_id, request_id FROM tasks WHERE id = ?",
                [task],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        if let Some((tree, request)) = row {
            if let Some(c) = self.of_request(conn, &request)? {
                return Ok(Some(c));
            }
            let given: Option<String> = conn
                .query_row(
                    "SELECT conversation_id FROM task_context WHERE task_id = ?",
                    [&tree],
                    |r| r.get(0),
                )
                .optional()?;
            if given.is_some() {
                return Ok(given);
            }
        }
        Ok(None)
    }

    /// The journal backstop (a crash between a chat journal row and its entry): every `chat.ack` after the stage-2
    /// watermark with no message entry gets one (its words from the request's `chat.message`, its answer from its
    /// `chat.reply`, then its stream's end); a message with no answer whose `chat.reply` was journaled gets that answer.
    fn backstop(&mut self, conn: &Connection) -> Result<(), ferb_store::Error> {
        let Some(from) = store::journal_from(conn)? else {
            return Ok(());
        };
        let rows: Vec<(String, String, String)> = {
            let mut q = conn.prepare(
                "SELECT kind, at, data_json FROM journal WHERE seq > ? AND kind IN ('chat.ack', 'chat.message', \
                 'chat.reply') ORDER BY seq",
            )?;
            let rows = q
                .query_map([from], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
                .collect::<Result<Vec<_>, _>>()?;
            rows
        };
        let data = |d: &str| -> Map<String, Value> {
            serde_json::from_str::<Value>(d)
                .ok()
                .and_then(|v| v.as_object().cloned())
                .unwrap_or_default()
        };
        let s = |m: &Map<String, Value>, k: &str| {
            m.get(k)
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        };
        let mut words: HashMap<String, String> = HashMap::new();
        let mut replies: HashMap<String, Vec<String>> = HashMap::new();
        let mut acks: Vec<(String, String, String)> = Vec::new(); // (request, session, at)
        for (kind, at, d) in &rows {
            let m = data(d);
            let request = s(&m, "request_id");
            if request.is_empty() {
                continue;
            }
            match kind.as_str() {
                "chat.ack" => acks.push((request, s(&m, "session_id"), at.clone())),
                "chat.message" => {
                    words.entry(request).or_insert_with(|| {
                        ferb_orchestrator::py::strip(&s(&m, "text")).to_string()
                    });
                }
                _ => replies.entry(request).or_default().push(s(&m, "text")),
            }
        }
        for (request, session, at) in acks {
            if store::conversation_of_request(conn, &request)?.is_some() {
                // a message entry exists: its journaled answer, if it has none
                let conversation =
                    store::conversation_of_request(conn, &request)?.unwrap_or_default();
                if let Some(said) = replies.get(&request).and_then(|r| r.first()) {
                    self.projector(conn, &conversation)?;
                    let p = self.st.projectors.get_mut(&conversation).expect("loaded");
                    let mut seams = Stored::new(conn, &conversation, self.inner, self.ids);
                    let changes = p.answer_from_journal(&request, said, &mut seams);
                    seams.ok()?;
                    self.write(conn, &conversation, changes)?;
                }
                continue;
            }
            let conversation = if session.is_empty() || session == "default" {
                current(conn)?
            } else {
                session
            };
            let ack = json!({"type": "ack", "request_id": request,
                             "message": words.get(&request).cloned().unwrap_or_default(), "at": at});
            self.feed(
                conn,
                &conversation,
                Input::Frame {
                    request_id: &request,
                    frame: &ack,
                },
            )?;
            for said in replies.get(&request).cloned().unwrap_or_default() {
                let reply = json!({"type": "reply", "text": said, "request_id": request});
                self.feed(
                    conn,
                    &conversation,
                    Input::Frame {
                        request_id: &request,
                        frame: &reply,
                    },
                )?;
            }
            self.feed(
                conn,
                &conversation,
                Input::StreamEnded {
                    request_id: &request,
                },
            )?;
        }
        Ok(())
    }
}

/// The owner's chat for what belongs to no message (a reminder, a notice, a result of a task no message started): the
/// current conversation (continuity.py `current`: the one FERB.app attached), else the one the owner last wrote in,
/// else `owner`; read on the drain's connection.
pub fn current(conn: &Connection) -> Result<String, ferb_store::Error> {
    let attached: Option<String> = conn
        .query_row(
            "SELECT conversation_id FROM current_conversation WHERE singleton = 1",
            [],
            |r| r.get(0),
        )
        .optional()?;
    if let Some(c) = attached {
        return Ok(c);
    }
    let wrote: Option<String> = conn
        .query_row(
            "SELECT conversation_id FROM chat_entries WHERE kind = 'message' ORDER BY created_at DESC, pos DESC LIMIT 1",
            [],
            |r| r.get(0),
        )
        .optional()?;
    Ok(wrote.unwrap_or_else(|| "owner".to_string()))
}

/// An input as the trace keeps it (`ChatLog::set_trace`): the timeline's words (`frame`, `end`, `bus`, `notification`).
fn traced_input(input: &Input<'_>) -> Value {
    match input {
        Input::Frame { request_id, frame } => {
            json!({"do": "frame", "request_id": request_id, "frame": frame})
        }
        Input::StreamEnded { request_id } => json!({"do": "end", "request_id": request_id}),
        Input::Bus { topic, data } => json!({"do": "bus", "topic": topic, "data": data}),
        Input::Notification(item) => json!({"do": "notification", "item": item}),
    }
}

/// An answer to a question with the facts its record keeps (compose.py `_question_record`'s `question.answered` row,
/// DMN-583: how it was answered, the choice, a typed answer's first line, the kinds of secret hidden in it), as
/// FERB.app's question card reads them; the bus event alone when there is no record.
fn answered(conn: &Connection, data: &Value) -> Result<Value, ferb_store::Error> {
    let Some(id) = data.get("approval_id").and_then(Value::as_str) else {
        return Ok(data.clone());
    };
    let row: Option<String> = conn
        .query_row(
            "SELECT data_json FROM journal WHERE kind = 'question.answered' \
             AND json_extract(data_json, '$.approval_id') = ? ORDER BY seq DESC LIMIT 1",
            [id],
            |r| r.get(0),
        )
        .optional()?;
    let Some(mut facts) = row.and_then(|r| serde_json::from_str::<Value>(&r).ok()) else {
        return Ok(data.clone());
    };
    if let (Some(f), Some(d)) = (facts.as_object_mut(), data.as_object()) {
        for (k, v) in d {
            f.entry(k.clone()).or_insert_with(|| v.clone());
        }
    }
    Ok(facts)
}
