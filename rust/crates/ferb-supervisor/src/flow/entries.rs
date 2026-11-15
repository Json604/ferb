//! The chat as entries (stage 2, ENG-8): [`ChatProjector`] turns the frames a `chat.send` stream carries and the bus
//! events FERB.app reads into [`Entry`] records, so the conversation FERB.app's reducer computes today is computed once,
//! in the engine, and every client reads the same one. Pure: no I/O, no store, not wired into `chat.rs` yet; time, ids
//! and the conversation's change counter come in through [`Seams`] (TEST-508).
//!
//! What it reproduces is FERB.app's own reducer, read from its code (the app is the reference for what the owner
//! sees): `DaemonConnection.swift` `handleChatEvent`, `receiveOutcome`, `handleOutcome`, `receiveBusEvent`,
//! `receiveNotices`, `receiveReminder`, `finishSubmission`; `Conversation.swift` `recordAnswer`, `withWorkSession`;
//! `ConversationArchive.swift` `acknowledge`, `showPlan`, `place`, `applyPart`, `applyOutcome`, `merge`, `finish`,
//! `abortReply`, `recordRecovered`, `addNotice`; `TurnCard.swift` and `StepChecklist.swift` (a card's and a
//! checklist's saved state). One turn of the app is a `message` entry, a `plan` entry and its `answer` entries; a late
//! answer the app posts below its message (APP-580) is an answer entry with `placement: below`.
//!
//! Stored is only what the app saves (its `Codable` fields). What it keeps as live state comes back as
//! [`Projected::live`] and is never stored: `progress`, `step` (a card's and a checklist's step states are never saved:
//! TurnCard.swift `steps`, StepChecklist.swift `states`), `subtask`, `verification`, `memory`, `timer`, the streamed
//! plan and reply lines, `tool.called`, `action.*` (DaemonConnection.swift:361-464).
//!
//! What FERB says after a turn's answer (a reply while its task runs, then the task's end; several replies to one
//! message: the frames after the turn's session ended) is a later answer, as the app keeps it (APP-914,
//! `answerLater`): an answer entry that `follows` the message's answer, in its own words; the message's answer stays.

use std::collections::{BTreeSet, HashMap};

use ferb_core::chat_entry::{
    body, AnswerBody, Card, CardRow, Change, Checklist, Entry, EntryKind, EntryState, MessageBody,
    NoticeBody, NoticeQuote, Placement, PlanBody, PlanStep, PostBody, QuestionAnswer, QuestionBody,
    RowQuestion, SealedId,
};
use serde_json::{Map, Value};

/// What the projector is given from outside: the time, ids and the conversation's change counter.
pub trait Seams {
    /// Now, as ISO 8601 in the Mac's own offset (an entry's `day` is its date part).
    fn now(&mut self) -> String;
    /// A new entry id.
    fn new_id(&mut self) -> String;
    /// The conversation's next change number (strictly greater than every one before).
    fn next_seq(&mut self) -> u64;
}

/// One thing that happened in the conversation.
#[derive(Debug, Clone, Copy)]
pub enum Input<'a> {
    /// One frame of a `chat.send` stream (`chat.send.event`'s params as `chat.rs` sends them), of the message
    /// `request_id`. The engine may add `input_paths` (the message's files) to the `ack`.
    Frame {
        request_id: &'a str,
        frame: &'a Value,
    },
    /// The message's stream ended (its turn returned, or the daemon shut down).
    StreamEnded { request_id: &'a str },
    /// A bus event as FERB.app reads it (`task.result`, `task.part`, `approval.requested`, `approval.resolved`,
    /// `attention.notice`, …). A task's recorded result read back (`chat.status`'s `results`) is a `task.result`; an
    /// `approval.resolved` may carry the answer's facts the question record keeps (`answer`, `choice`, `said`,
    /// `hidden`).
    Bus { topic: &'a str, data: &'a Value },
    /// A banner as `notifications.subscribe` carries it; a reminder's (`target.kind: reminder`) is posted in the chat.
    Notification(&'a Value),
}

/// What one input made.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Projected {
    /// Entries created (rev 1) or revised, in the order they were made.
    pub changes: Vec<Change>,
    /// The input itself when it is live state only (never stored).
    pub live: Option<Value>,
}

/// questions.py `SECRET_KINDS`: a hidden input (compose.rs `SECRET_KINDS`).
const SECRET_KINDS: [&str; 3] = ["owner_login_code", "owner_admin_password", "owner_secret"];
/// The words FERB.app puts on a message whose stream ended with no answer (DaemonConnection.swift:314).
pub const ENDED_UNCONFIRMED: &str =
    "The response ended before FERB confirmed it. Check tasks before resubmitting.";
/// handleOutcome's words for an outcome with no answer (DaemonConnection.swift:883).
const NO_ANSWER: &str = "No answer was recorded.";
/// The error frame's default (DaemonConnection.swift:442).
const NOT_STARTED: &str = "FERB could not start this request.";
/// ResultCard.types: the typed cards FERB.app draws.
const CARD_TYPES: [&str; 9] = [
    "reminder",
    "event",
    "note",
    "payment",
    "send",
    "link",
    "file",
    "page",
    "schedules",
];
/// A checklist step's ends kept once its task ended (StepChecklist.end).
const ENDS: [&str; 5] = ["done", "failed", "not_finished", "wont_run", "stopped"];
/// The frames and bus topics FERB.app keeps as live state only.
const LIVE_FRAMES: [&str; 11] = [
    "progress",
    "step",
    "subtask",
    "verification",
    "memory",
    "timer",
    "plan_delta",
    "plan_abort",
    "plan_end",
    "reply_delta",
    "reply_abort",
];

/// A new answer entry to start at the next emit: where it shows, what a late one shows, and its id when it was
/// already given (a late answer is the chat's last entry from the moment it is placed).
#[derive(Debug, Clone)]
struct Pending {
    placement: Placement,
    part: Option<String>,
    id: Option<String>,
}

/// One turn as the app holds it (`ConversationTurn`'s saved fields) and the stream session that feeds it.
#[derive(Debug, Clone, Default)]
struct Turn {
    /// Its entries: the message, the plan, and the answer it shows now.
    message_entry: Option<String>,
    plan_entry: Option<String>,
    answer_entry: Option<String>,
    pending: Option<Pending>,
    request_id: Option<String>,
    /// The app's session for this turn: it takes the stream's frames while active (`activeTaskID`), and the task
    /// bound to it (`backendTaskID`).
    active: bool,
    session_task: Option<String>,
    // ConversationTurn
    request: String,
    hidden: Vec<String>,
    sealed: Vec<SealedId>,
    attachments: Vec<String>,
    task_id: Option<String>,
    status: String,
    answered: bool,
    text: String,
    display_text: Option<String>,
    verified: bool,
    artifacts: Vec<String>,
    response_notice: Option<String>,
    planned: bool,
    plan: PlanBody,
    note: Option<String>,
    code_lines: Option<Vec<String>>,
    runtime_switch: Option<Value>,
    cards: Option<Vec<Value>>,
    merged_into: Option<String>,
    question: Option<String>,
    asks: bool,
    /// Where the answer it shows now came from (`AnswerBody.from`); a task's end has been told.
    from: String,
    ended: bool,
    /// A turn with no message (recordRecovered): the task's name and goal.
    recovered: Option<(Option<String>, Option<String>)>,
    /// Its late answers posted below, oldest first (entry ids).
    replies: Vec<String>,
    /// Its later answers (APP-914), oldest first (entry ids).
    later: Vec<String>,
    /// ORCH-579: its next answer is told in another message's reply.
    told_elsewhere: bool,
    /// Its task was cut by a restart and did not resume: its plan closes as `interrupted` (the start-up pass).
    cut: bool,
    /// The client that sent the message and its id for the submission (the engine adds them to the ack it feeds).
    client_id: Option<String>,
    submission_id: Option<String>,
}

impl Turn {
    fn visible(&self) -> String {
        match &self.display_text {
            Some(d) if !d.is_empty() => d.clone(),
            _ => self.text.clone(),
        }
    }
}

/// The last thing shown at the end of the chat (`lastEntryID`): a turn, or a late answer or notice.
#[derive(Debug, Clone, PartialEq)]
enum Shown {
    Turn(usize),
    Entry(String),
}

/// The conversation's entries as FERB.app's reducer would hold them, kept current input by input.
pub struct ChatProjector {
    conversation_id: String,
    next_pos: u64,
    entries: HashMap<String, Entry>,
    order: Vec<String>,
    turns: Vec<Turn>,
    by_request: HashMap<String, usize>,
    /// The app's foreground session: the newest message's turn.
    foreground: Option<usize>,
    last_shown: Option<Shown>,
    notices: HashMap<String, String>,
    questions: HashMap<String, String>,
    dirty: BTreeSet<usize>,
    made: Vec<Change>,
}

impl ChatProjector {
    /// A projector for one conversation (`chat.send`'s `session_id`), whose next entry takes `next_pos`.
    pub fn new(conversation_id: &str, next_pos: u64) -> ChatProjector {
        ChatProjector {
            conversation_id: conversation_id.to_string(),
            next_pos,
            entries: HashMap::new(),
            order: Vec::new(),
            turns: Vec::new(),
            by_request: HashMap::new(),
            foreground: None,
            last_shown: None,
            notices: HashMap::new(),
            questions: HashMap::new(),
            dirty: BTreeSet::new(),
            made: Vec::new(),
        }
    }

    /// Every entry as it stands now, in creation order.
    pub fn entries(&self) -> Vec<Entry> {
        self.order
            .iter()
            .filter_map(|id| self.entries.get(id).cloned())
            .collect()
    }

    /// A projector rebuilt from stored entries after a restart (ENG-9): `stored` are the entries the rebuild needs, in
    /// `pos` order (every open entry, the entries of their messages and of the last few turns: the store's
    /// `open_entries`, `of_requests` and `recent`), `refs` every notice and post already posted (each is posted once),
    /// `next_pos` the stream's next position. Each message's turn is filled back from its message, plan and answer
    /// entries as `emit` wrote them; no stream feeds a turn after a restart (`active` is false, as the app's sessions
    /// are gone), so a frame for one of them is a later frame (`receiveAfterAnswer`). The chat ends on the last entry.
    pub fn restore(
        conversation_id: &str,
        stored: Vec<Entry>,
        refs: Vec<String>,
        next_pos: u64,
    ) -> ChatProjector {
        let mut p = ChatProjector::new(conversation_id, next_pos);
        let mut seen = std::collections::HashSet::new();
        for e in stored {
            if e.conversation_id != conversation_id || !seen.insert(e.id.clone()) {
                continue;
            }
            p.order.push(e.id.clone());
            p.entries.insert(e.id.clone(), e);
        }
        p.order.sort_by_key(|id| p.entries[id].pos);
        for r in refs {
            p.notices.entry(r.clone()).or_insert(r);
        }
        let order = p.order.clone();
        for id in &order {
            let e = p.entries[id].clone();
            match e.kind {
                EntryKind::Question => {
                    if let Some(a) = &e.approval_id {
                        p.questions.insert(a.clone(), e.id.clone());
                    }
                }
                EntryKind::Notice | EntryKind::Post => {
                    if let Some(r) = &e.reference {
                        p.notices.insert(r.clone(), e.id.clone());
                    }
                }
                EntryKind::Message | EntryKind::Plan | EntryKind::Answer | EntryKind::Error => {
                    p.restore_part(&e)
                }
                EntryKind::Legacy => {}
            }
        }
        p.foreground = p.turns.iter().rposition(|t| t.message_entry.is_some());
        // the chat ends on its last entry a client shows at the end (a question never moves it: `question`)
        p.last_shown = order
            .iter()
            .rev()
            .find(|id| !matches!(p.entries[*id].kind, EntryKind::Question | EntryKind::Legacy))
            .map(|id| p.shown_as(id));
        p
    }

    /// One stored message, plan or answer put back on its turn.
    fn restore_part(&mut self, e: &Entry) {
        let follows = e
            .body_as::<AnswerBody>()
            .and_then(|b| b.follows)
            .filter(|_| matches!(e.kind, EntryKind::Answer | EntryKind::Error));
        let t = match (&e.request_id, e.kind) {
            (Some(r), _) => match self.by_request.get(r) {
                Some(&t) => t,
                None => {
                    let t = self.turns.len();
                    self.turns.push(Turn {
                        request_id: Some(r.clone()),
                        status: "working".into(),
                        ..Turn::default()
                    });
                    self.by_request.insert(r.clone(), t);
                    t
                }
            },
            // an answer with no message: a recovered task's turn (recordRecovered)
            (None, EntryKind::Answer) => {
                let t = self.turns.len();
                self.turns.push(Turn {
                    recovered: Some((None, None)),
                    ..Turn::default()
                });
                t
            }
            _ => return,
        };
        let turn = &mut self.turns[t];
        if e.task_id.is_some() {
            turn.task_id = e.task_id.clone();
        }
        match e.kind {
            EntryKind::Message => {
                let m: MessageBody = e.body_as().unwrap_or_default();
                turn.message_entry = Some(e.id.clone());
                turn.client_id = e.client_id.clone();
                turn.submission_id = e.submission_id.clone();
                turn.request = m.text;
                turn.hidden = m.hidden;
                turn.sealed = m.sealed;
                turn.attachments = m.attachments;
                turn.merged_into = m.merged_into;
                turn.response_notice = m.notice;
                if !turn.answered {
                    turn.status = match e.state {
                        EntryState::Open => "working",
                        EntryState::Interrupted => "interrupted",
                        EntryState::Merged => "merged",
                        EntryState::Failed => "failed",
                        EntryState::Cancelled => "cancelled",
                        _ => "done",
                    }
                    .into();
                }
            }
            EntryKind::Plan => {
                turn.plan_entry = Some(e.id.clone());
                turn.planned = true;
                turn.plan = e.body_as().unwrap_or_default();
                turn.ended = turn.ended || !e.open;
                turn.cut = e.state == EntryState::Interrupted;
            }
            _ if follows.is_some() => turn.later.push(e.id.clone()),
            _ => {
                let a: AnswerBody = e.body_as().unwrap_or_default();
                if e.placement == Some(Placement::Below) {
                    turn.replies.push(e.id.clone());
                }
                if turn.recovered.is_some() {
                    turn.recovered = Some((a.name.clone(), a.goal.clone()));
                }
                turn.answer_entry = Some(e.id.clone());
                turn.answered = true;
                turn.ended = turn.ended || a.from == "task";
                turn.status = a.status;
                turn.text = a.text;
                turn.display_text = a.display_text;
                turn.verified = a.verified;
                turn.artifacts = a.artifacts;
                turn.note = a.note;
                turn.code_lines = a.lines;
                turn.runtime_switch = a.runtime_switch;
                turn.cards = a.cards;
                turn.question = a.question;
                turn.asks = a.asks;
                turn.from = a.from;
            }
        }
    }

    /// How an entry stands at the end of the chat: as its turn (its message, plan or own answer in place) or itself (a
    /// late answer posted below, a later answer, a notice, a post, a question).
    fn shown_as(&self, id: &str) -> Shown {
        let e = &self.entries[id];
        let own = matches!(e.kind, EntryKind::Message | EntryKind::Plan)
            || (matches!(e.kind, EntryKind::Answer | EntryKind::Error)
                && e.placement != Some(Placement::Below)
                && e.body_as::<AnswerBody>()
                    .is_none_or(|b| b.follows.is_none()));
        let turn = self.turns.iter().position(|t| {
            t.message_entry.as_deref() == Some(id)
                || t.plan_entry.as_deref() == Some(id)
                || t.answer_entry.as_deref() == Some(id)
        });
        match turn {
            Some(t) if own => Shown::Turn(t),
            _ => Shown::Entry(id.to_string()),
        }
    }

    /// The start-up pass after a restart (journal 69's B3): what can no longer happen is said. A message still waiting
    /// with no task was cut with its stream; a turn whose task did not resume (`resumed` says which did) is
    /// `interrupted` (its plan closes; a message still waiting on its answer or on the owner says so); every open
    /// question expires (pending questions live in memory and die with the process, ferb/daemon/questions.py:121).
    pub fn after_restart(
        &mut self,
        resumed: &dyn Fn(&str) -> bool,
        seams: &mut dyn Seams,
    ) -> Vec<Change> {
        for t in 0..self.turns.len() {
            let turn = &mut self.turns[t];
            if turn.recovered.is_some() {
                continue;
            }
            let running = turn.task_id.as_deref().is_some_and(resumed);
            let waiting = turn.status == "working" || turn.status == "needs_user";
            if turn.task_id.is_none() {
                if turn.status == "working" && turn.message_entry.is_some() {
                    // the stream ended with the process (stream_ended)
                    turn.text.clear();
                    turn.response_notice = Some(
                        turn.response_notice
                            .clone()
                            .unwrap_or_else(|| ENDED_UNCONFIRMED.to_string()),
                    );
                    turn.status = "interrupted".into();
                    self.dirty.insert(t);
                }
                continue;
            }
            if running {
                continue;
            }
            if waiting {
                turn.response_notice = Some(
                    turn.response_notice
                        .clone()
                        .unwrap_or_else(|| ENDED_UNCONFIRMED.to_string()),
                );
                turn.status = "interrupted".into();
                self.dirty.insert(t);
            }
            if turn.planned && !turn.ended && !turn.cut {
                turn.cut = true;
                self.dirty.insert(t);
            }
        }
        let open: Vec<String> = self
            .questions
            .values()
            .filter(|id| self.entries.get(*id).is_some_and(|e| e.open))
            .cloned()
            .collect();
        let mut open: Vec<Entry> = open
            .iter()
            .filter_map(|id| self.entries.get(id).cloned())
            .collect();
        open.sort_by_key(|e| e.pos);
        self.emit(seams);
        for mut entry in open {
            let mut asked: QuestionBody = entry.body_as().unwrap_or_default();
            asked.answer = Some(QuestionAnswer {
                state: "expired".into(),
                ..QuestionAnswer::default()
            });
            entry.state = EntryState::Expired;
            entry.open = false;
            entry.body = body(&asked);
            self.revise(entry, seams);
        }
        std::mem::take(&mut self.made)
    }

    /// The journal backstop (a crash between a turn's `chat.reply` row and its frame): the reply recorded for a message
    /// that has no answer is its answer, as the reply frame would have made it. Nothing for a message already answered
    /// or unknown here.
    pub fn answer_from_journal(
        &mut self,
        request_id: &str,
        said: &str,
        seams: &mut dyn Seams,
    ) -> Vec<Change> {
        let Some(&t) = self.by_request.get(request_id) else {
            return Vec::new();
        };
        if self.turns[t].answered || self.turns[t].message_entry.is_none() {
            return Vec::new();
        }
        let frame = serde_json::json!({"type": "reply", "text": said, "request_id": request_id});
        self.turns[t].active = true;
        self.reply(t, "reply", &frame, seams);
        self.emit(seams);
        std::mem::take(&mut self.made)
    }

    /// The request ids this projector holds a turn for.
    pub fn knows_request(&self, request_id: &str) -> bool {
        self.by_request.contains_key(request_id)
    }

    /// Whether a turn here names this task.
    pub fn knows_task(&self, task_id: &str) -> bool {
        self.turns
            .iter()
            .any(|t| t.task_id.as_deref() == Some(task_id))
    }

    /// The position the next entry takes.
    pub fn next_pos(&self) -> u64 {
        self.next_pos
    }

    /// One input: the entries it created or revised, or the input back as live state.
    pub fn apply(&mut self, input: Input<'_>, seams: &mut dyn Seams) -> Projected {
        let live = match input {
            Input::Frame { request_id, frame } => self.frame(request_id, frame, seams),
            Input::StreamEnded { request_id } => {
                self.stream_ended(request_id);
                false
            }
            Input::Bus { topic, data } => self.bus(topic, data, seams),
            Input::Notification(item) => {
                self.notification(item, seams);
                false
            }
        };
        self.emit(seams);
        let changes = std::mem::take(&mut self.made);
        let live = match input {
            _ if !live || !changes.is_empty() => None,
            Input::Frame { frame, .. } => Some(frame.clone()),
            Input::Bus { topic, data } => {
                let mut ev = Map::new();
                ev.insert("type".into(), Value::from("event"));
                ev.insert("topic".into(), Value::from(topic));
                ev.insert("data".into(), data.clone());
                Some(Value::Object(ev))
            }
            _ => None,
        };
        Projected { changes, live }
    }

    // ---- inputs ----

    /// A stream frame (handleChatEvent). Returns whether it is live state only.
    fn frame(&mut self, request_id: &str, frame: &Value, seams: &mut dyn Seams) -> bool {
        let ty = text(frame, "type").unwrap_or_default();
        if ty == "ack" {
            self.ack(request_id, frame);
            return false;
        }
        let Some(&t) = self.by_request.get(request_id) else {
            // a frame of a message this projector never saw accepted: a task's end still reaches the chat (as its
            // task.result does), a row's change as task.part
            match ty.as_str() {
                "outcome" => self.receive_outcome(frame, None, seams),
                "part" => self.apply_part(frame),
                _ => return LIVE_FRAMES.contains(&ty.as_str()) || ty == "event",
            }
            return false;
        };
        if !self.turns[t].active {
            // the app has no session for this turn any more (receiveAfterAnswer): what FERB says is a later answer, a
            // task's end reaches it as its result does, a row's change as task.part; the rest comes again on the bus
            match ty.as_str() {
                "reply" | "clarify" | "error" => self.later_reply(t, &ty, frame, seams),
                "outcome" => self.receive_outcome(frame, None, seams),
                "part" => self.apply_part(frame),
                "event" => return self.stream_event(frame, seams),
                _ => return LIVE_FRAMES.contains(&ty.as_str()),
            }
            return false;
        }
        match ty.as_str() {
            "plan" => self.plan(t, frame),
            "part" => self.apply_part(frame),
            "reply" | "clarify" => self.reply(t, &ty, frame, seams),
            "merged" => self.merge(t, frame),
            "outcome" => self.receive_outcome(frame, Some(t), seams),
            "error" => {
                let said = text(frame, "text").unwrap_or_else(|| NOT_STARTED.to_string());
                let end = End {
                    said,
                    status: "failed".into(),
                    from: "error",
                    ..End::default()
                };
                self.finish(t, end, seams);
            }
            "event" => return self.stream_event(frame, seams),
            _ => return LIVE_FRAMES.contains(&ty.as_str()),
        }
        false
    }

    /// A bus event riding the stream (`{"type": "event", "topic", "data"}`): the app takes only a question from it.
    fn stream_event(&mut self, frame: &Value, seams: &mut dyn Seams) -> bool {
        if text(frame, "topic").as_deref() == Some("approval.requested") {
            self.question(&frame["data"], seams);
            return false;
        }
        true
    }

    /// `ack` (begin, acknowledge): the message, as the engine recorded and redacted it, becomes a turn.
    fn ack(&mut self, request_id: &str, frame: &Value) {
        if self.by_request.contains_key(request_id) {
            return; // one message per request
        }
        let turn = Turn {
            request_id: Some(request_id.to_string()),
            active: true,
            status: "working".into(),
            request: words(frame, "message").unwrap_or_default(),
            hidden: strings(frame, "hidden").unwrap_or_default(),
            sealed: frame
                .get("sealed")
                .cloned()
                .and_then(|v| serde_json::from_value(v).ok())
                .unwrap_or_default(),
            attachments: strings(frame, "input_paths").unwrap_or_default(),
            client_id: words(frame, "client_id"),
            submission_id: words(frame, "submission_id"),
            ..Turn::default()
        };
        let t = self.turns.len();
        self.turns.push(turn);
        self.by_request.insert(request_id.to_string(), t);
        self.foreground = Some(t);
        self.last_shown = Some(Shown::Turn(t));
        self.dirty.insert(t);
    }

    /// `plan`: the session takes the task (bindBackendTask), a provisional reply goes (clearProvisionalReply), the
    /// turn is bound to the task and shows the plan (showPlan).
    fn plan(&mut self, t: usize, frame: &Value) {
        let Some(task) = text(frame, "task_id") else {
            return;
        };
        let turn = &mut self.turns[t];
        turn.session_task = Some(task.clone());
        if turn.status == "working" || turn.status == "interrupted" {
            turn.text.clear();
            turn.response_notice = None;
            turn.status = "working".into();
        }
        turn.task_id = Some(task);
        self.show_plan(t, frame);
        self.dirty.insert(t);
    }

    /// ConversationArchive.showPlan.
    fn show_plan(&mut self, t: usize, details: &Value) {
        let turn = &mut self.turns[t];
        turn.planned = true;
        let said = text(details, "text").unwrap_or_default();
        if !said.is_empty() {
            turn.plan.text = Some(said);
        }
        if details.get("name").is_some() || details.get("goal").is_some() {
            turn.plan.name = text(details, "name");
            turn.plan.goal = text(details, "goal");
        }
        if let Some(line) = text(details, "plan_line") {
            turn.plan.plan_line = Some(line);
            turn.plan.lines = Some(
                strings(details, "lines")
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|l| !l.is_empty())
                    .collect(),
            );
        }
        if let Some(direct) = objects(details, "direct") {
            turn.plan.direct = Some(
                direct
                    .iter()
                    .filter_map(|d| text(d, "text"))
                    .filter(|d| !d.trim().is_empty())
                    .collect(),
            );
        }
        if let Some(receipts) = strings(details, "receipts") {
            turn.plan.receipts = Some(receipts.into_iter().filter(|r| !r.is_empty()).collect());
        }
        if let Some(cards) = result_cards(details.get("cards")) {
            turn.plan.receipt_cards = Some(cards);
        }
        if let Some(task) = text(details, "task_id") {
            if let Some(card) = card_from(details.get("card"), &task) {
                place_card(&mut self.turns[t].plan, card);
            }
            if let Some((checklist, _)) = checklist_from(details.get("checklist"), &task) {
                place_checklist(&mut self.turns[t].plan, checklist);
            }
        }
    }

    /// ConversationArchive.applyPart: a row's change, in whichever card shows its part.
    fn apply_part(&mut self, event: &Value) {
        let (Some(task), Some(part)) = (text(event, "task_id"), text(event, "part")) else {
            return;
        };
        let request = text(event, "request_id");
        let first = text(event, "card_request_id");
        fn card_of<'a>(x: &'a Turn, task: &str) -> Option<&'a Card> {
            x.plan.card.as_ref().filter(|c| c.task_id == task)
        }
        let shows = |c: &Card| c.rows.iter().any(|r| r.part == part);
        let showing = |r: &Option<String>| -> Option<usize> {
            let r = r.as_ref()?;
            self.turns
                .iter()
                .rposition(|x| card_of(x, &task).is_some_and(|c| &c.request_id == r && shows(c)))
        };
        let index = showing(&request)
            .or_else(|| showing(&first))
            .or_else(|| {
                self.turns
                    .iter()
                    .rposition(|x| card_of(x, &task).is_some_and(shows))
            })
            .or_else(|| {
                let r = request.as_ref()?;
                self.turns
                    .iter()
                    .rposition(|x| card_of(x, &task).is_some_and(|c| &c.request_id == r))
            });
        let Some(t) = index else {
            return;
        };
        if let Some(card) = self.turns[t].plan.card.as_mut() {
            if apply_part_to(card, event) {
                self.dirty.insert(t);
            }
        }
    }

    /// ConversationArchive.applyOutcome: an outcome's own fields on the turn of its request, else its task's newest
    /// turn; a task's end after the turn's answer is a later answer with its own lines and cards (APP-914).
    fn apply_outcome(&mut self, event: &Value, seams: &mut dyn Seams) {
        let Some(task) = text(event, "task_id") else {
            return;
        };
        let request = text(event, "request_id");
        let index = request
            .as_ref()
            .and_then(|r| {
                self.turns
                    .iter()
                    .rposition(|x| x.request_id.as_ref() == Some(r))
            })
            .or_else(|| {
                self.turns
                    .iter()
                    .rposition(|x| x.task_id.as_deref() == Some(&task))
            });
        let Some(t) = index else {
            return;
        };
        let note = words(event, "note");
        let code_lines = strings(event, "lines").filter(|l| !l.is_empty());
        let offer = runtime_switch(event.get("runtime_switch"));
        let said = text(event, "answer").or_else(|| text(event, "summary"));
        if self.is_later(t, said.as_deref()) {
            let answer = AnswerBody {
                from: "task".into(),
                text: said.unwrap_or_default(),
                display_text: text(event, "display_text"),
                status: text(event, "status").unwrap_or_else(|| "failed".into()),
                verified: flag(event, "verified").unwrap_or(false),
                artifacts: paths(event),
                note,
                lines: code_lines,
                runtime_switch: offer,
                cards: result_cards(event.get("cards")),
                ..AnswerBody::default()
            };
            self.answer_later(t, answer, false, seams);
        } else {
            let turn = &mut self.turns[t];
            turn.note = note;
            turn.code_lines = code_lines;
            turn.runtime_switch = offer;
            if let Some(cards) = result_cards(event.get("cards")) {
                turn.cards = Some(cards);
            }
        }
        let turn = &mut self.turns[t];
        if let Some(card) = card_from(event.get("card"), &task) {
            place_card(&mut turn.plan, card);
        }
        if let Some((ended, states)) = checklist_from(event.get("checklist"), &task) {
            let mut checklist = match &turn.plan.checklist {
                Some(shown) if shown.task_id == task => shown.clone(),
                _ => ended.clone(),
            };
            checklist.steps = ended.steps;
            checklist.ended = states
                .into_iter()
                .filter(|(_, s)| ENDS.contains(&s.as_str()))
                .map(|(k, s)| (k, Value::from(s)))
                .collect();
            checklist.finished = true;
            turn.plan.checklist = Some(checklist);
        }
        if words(event, "told_in").is_some() && awaiting(&turn.status) {
            turn.told_elsewhere = true;
        }
        self.dirty.insert(t);
    }

    /// `reply` and `clarify`: the session ends (finishBackendWork) and the turn is answered (recordAnswer, finish);
    /// then the reply's offer of another AI service and its cards.
    fn reply(&mut self, t: usize, ty: &str, frame: &Value, seams: &mut dyn Seams) {
        self.turns[t].active = false;
        let end = End {
            said: text(frame, "text").unwrap_or_default(),
            status: text(frame, "status").unwrap_or_else(|| "done".into()),
            display: text(frame, "display_text"),
            from: if ty == "clarify" { "clarify" } else { "reply" },
            ..End::default()
        };
        self.finish(t, end, seams);
        let turn = &mut self.turns[t];
        turn.asks = ty == "clarify";
        if let Some(offer) = runtime_switch(frame.get("runtime_switch")) {
            turn.runtime_switch = Some(offer);
        }
        if let Some(cards) = result_cards(frame.get("cards")) {
            turn.cards = Some(cards);
        }
    }

    /// A reply, question or error after the turn's session ended with its answer (receiveAfterAnswer): a later answer.
    fn later_reply(&mut self, t: usize, ty: &str, frame: &Value, seams: &mut dyn Seams) {
        let error = ty == "error";
        let status = if error {
            "failed".to_string()
        } else {
            text(frame, "status").unwrap_or_else(|| "done".into())
        };
        let said = text(frame, "text").unwrap_or_else(|| {
            if error {
                NOT_STARTED.into()
            } else {
                String::new()
            }
        });
        let answer = AnswerBody {
            from: ty.to_string(),
            text: said,
            display_text: text(frame, "display_text"),
            status,
            asks: ty == "clarify",
            runtime_switch: runtime_switch(frame.get("runtime_switch")),
            cards: result_cards(frame.get("cards")),
            ..AnswerBody::default()
        };
        self.answer_later(t, answer, false, seams);
    }

    /// ConversationArchive.isLaterAnswer: the turn already shows FERB's own answer (not a question waiting on the
    /// owner, a merged message or a card, whose rows are its answer) and these are other words (APP-914).
    fn is_later(&self, t: usize, said: Option<&str>) -> bool {
        let turn = &self.turns[t];
        let Some(said) = said.filter(|s| !s.is_empty()) else {
            return false;
        };
        said != turn.text
            && !turn.text.is_empty()
            && turn.plan.card.is_none()
            && !awaiting(&turn.status)
            && turn.status != "needs_user"
            && turn.status != "merged"
    }

    /// ConversationArchive.answerLater (APP-914): a later answer of turn `t`, an answer entry that `follows` its
    /// answer, shown after the chat's last entry as it arrives; the same words again revise it (its status and words,
    /// and its lines and cards unless `words_only`). Nothing for words that aren't a later answer.
    fn answer_later(
        &mut self,
        t: usize,
        mut said: AnswerBody,
        words_only: bool,
        seams: &mut dyn Seams,
    ) {
        if !self.is_later(t, Some(&said.text)) {
            return;
        }
        let shown = self.turns[t]
            .later
            .iter()
            .rev()
            .find(|id| {
                self.entries
                    .get(*id)
                    .and_then(|e| e.body_as::<AnswerBody>())
                    .is_some_and(|b| b.text == said.text)
            })
            .cloned();
        if let Some(id) = shown {
            let Some(mut entry) = self.entries.get(&id).cloned() else {
                return;
            };
            let earlier: AnswerBody = entry.body_as().unwrap_or_default();
            if words_only {
                said.note = earlier.note.clone();
                said.lines = earlier.lines.clone();
                said.cards = earlier.cards.clone();
                said.runtime_switch = earlier.runtime_switch.clone();
            }
            let kept = AnswerBody {
                from: earlier.from,
                asks: earlier.asks,
                follows: earlier.follows,
                ..said
            };
            let (state, open) = category(&kept.status, kept.asks);
            entry.state = state;
            entry.open = open;
            entry.body = body(&kept);
            self.revise(entry, seams);
            return;
        }
        let (state, open) = category(&said.status, said.asks);
        let kind = if said.from == "error" {
            EntryKind::Error
        } else {
            EntryKind::Answer
        };
        let turn = &self.turns[t];
        said.follows = turn.answer_entry.clone();
        let (request, task, message) = (
            turn.request_id.clone(),
            turn.task_id.clone(),
            turn.message_entry.clone(),
        );
        let id = seams.new_id();
        let mut entry = self.new_entry(id.clone(), kind, state, open, request, seams);
        entry.task_id = task;
        entry.answers = message;
        entry.placement = Some(Placement::Inline);
        entry.body = body(&said);
        self.create(entry, seams);
        self.turns[t].later.push(id.clone());
        self.last_shown = Some(Shown::Entry(id));
    }

    /// `merged` (ORCH-529): the message was read together with another; its session ends (finishMerged).
    fn merge(&mut self, t: usize, frame: &Value) {
        let into = text(frame, "into").unwrap_or_default();
        let turn = &mut self.turns[t];
        if turn.status == "working" && turn.task_id.is_none() {
            turn.merged_into = Some(into);
            turn.status = "merged".into();
        }
        if turn.session_task.is_none() {
            turn.active = false;
        }
        self.dirty.insert(t);
    }

    /// receiveOutcome: the outcome's fields, then its end on the session that runs the task (handleOutcome), else on
    /// the turn of its request or task, else on a turn of its own (recordRecovered). `context`: the stream's turn,
    /// whose session is the one in hand; none for a bus event (the foreground session).
    fn receive_outcome(&mut self, event: &Value, context: Option<usize>, seams: &mut dyn Seams) {
        let Some(task) = text(event, "task_id") else {
            return;
        };
        self.apply_outcome(event, seams);
        let runs = |x: &Turn| x.active && x.session_task.as_deref() == Some(task.as_str());
        let session = context
            .or(self.foreground)
            .filter(|&s| runs(&self.turns[s]))
            .or_else(|| self.turns.iter().position(runs));
        if let Some(s) = session {
            self.handle_outcome(s, &task, event, seams);
        } else if let Some(status) = text(event, "status") {
            let found = text(event, "request_id")
                .and_then(|r| {
                    self.turns
                        .iter()
                        .position(|x| x.request_id.as_ref() == Some(&r) && x.recovered.is_none())
                })
                .or_else(|| {
                    self.turns
                        .iter()
                        .position(|x| x.task_id.as_deref() == Some(&task))
                });
            let mut end = End {
                said: String::new(),
                status,
                artifacts: paths(event),
                display: text(event, "display_text"),
                verified: flag(event, "verified").unwrap_or(false),
                from: "task",
            };
            if let Some(t) = found {
                self.turns[t].task_id = Some(task.clone());
                end.said = text(event, "answer")
                    .or_else(|| text(event, "summary"))
                    .unwrap_or_else(|| self.turns[t].text.clone());
                self.finish(t, end, seams);
            } else {
                end.said = text(event, "answer")
                    .or_else(|| text(event, "summary"))
                    .unwrap_or_default();
                self.recovered(&task, event, end);
            }
        }
        self.apply_outcome(event, seams);
    }

    /// recordRecovered: a finished task this chat has no turn for (started elsewhere, a schedule's run, before a
    /// restart), filed as FERB's message and answered in place.
    fn recovered(&mut self, task: &str, event: &Value, end: End) {
        let t = self.turns.len();
        self.turns.push(Turn {
            request_id: text(event, "request_id"),
            task_id: Some(task.to_string()),
            status: end.status,
            answered: true,
            text: end.said,
            display_text: end.display,
            verified: end.verified,
            artifacts: end.artifacts,
            from: "task".into(),
            ended: true,
            recovered: Some((text(event, "name"), text(event, "goal"))),
            pending: Some(Pending {
                placement: Placement::Inline,
                part: None,
                id: None,
            }),
            ..Turn::default()
        });
        self.last_shown = Some(Shown::Turn(t));
        self.dirty.insert(t);
    }

    /// handleOutcome on the session `s` that runs `task`: the session ends; the first turn of that session or task is
    /// answered (recordAnswer), then any other turn of the task still waiting.
    fn handle_outcome(&mut self, s: usize, task: &str, event: &Value, seams: &mut dyn Seams) {
        self.turns[s].active = false;
        let end = End {
            said: text(event, "answer")
                .or_else(|| text(event, "summary"))
                .unwrap_or_else(|| NO_ANSWER.into()),
            status: text(event, "status").unwrap_or_else(|| "failed".into()),
            artifacts: paths(event),
            display: text(event, "display_text"),
            verified: flag(event, "verified").unwrap_or(false),
            from: "task",
        };
        let first = self
            .turns
            .iter()
            .enumerate()
            .position(|(i, x)| i == s || x.task_id.as_deref() == Some(task))
            .unwrap_or(s);
        self.turns[first].task_id = Some(task.to_string());
        self.finish(first, end.clone(), seams);
        for t in 0..self.turns.len() {
            let x = &self.turns[t];
            if x.task_id.as_deref() == Some(task)
                && ["working", "interrupted", "needs_user"].contains(&x.status.as_str())
            {
                self.finish(t, end.clone(), seams);
            }
        }
    }

    /// The stream ended (the stream task's end and finishSubmission): a turn whose session still waits with no task
    /// was interrupted.
    fn stream_ended(&mut self, request_id: &str) {
        let Some(&t) = self.by_request.get(request_id) else {
            return;
        };
        let turn = &mut self.turns[t];
        if !turn.active || turn.session_task.is_some() {
            return;
        }
        if turn.status == "working" && turn.task_id.is_none() {
            let notice = turn
                .response_notice
                .clone()
                .unwrap_or_else(|| ENDED_UNCONFIRMED.to_string());
            turn.text.clear();
            turn.response_notice = Some(notice);
            turn.status = "interrupted".into();
        }
        turn.active = false;
        self.dirty.insert(t);
    }

    /// A bus event (receiveBusEvent). Returns whether it is live state only.
    fn bus(&mut self, topic: &str, data: &Value, seams: &mut dyn Seams) -> bool {
        match topic {
            "task.result" => self.receive_outcome(data, None, seams),
            "task.part" => self.apply_part(data),
            "approval.requested" => self.question(data, seams),
            "approval.resolved" => self.resolve(data, seams),
            "attention.notice" => self.notice(data, seams),
            _ => return true,
        }
        false
    }

    /// `approval.requested`: a question FERB asks, once per approval.
    fn question(&mut self, data: &Value, seams: &mut dyn Seams) {
        let Some(approval) = words(data, "approval_id") else {
            return;
        };
        if let Some(id) = self.questions.get(&approval).cloned() {
            // asked once; a later copy may name what an earlier one didn't (`chat.status` lists no request)
            if let Some(mut entry) = self.entries.get(&id).cloned() {
                entry.request_id = entry.request_id.or_else(|| text(data, "request_id"));
                entry.task_id = entry.task_id.or_else(|| text(data, "task_id"));
                self.revise(entry, seams);
            }
            return;
        }
        let kind = text(data, "kind").unwrap_or_default();
        let asked = QuestionBody {
            secret: SECRET_KINDS.contains(&kind.as_str()),
            kind,
            capability: text(data, "capability").unwrap_or_default(),
            title: text(data, "title"),
            reason: text(data, "reason").unwrap_or_default(),
            summary: text(data, "summary"),
            options: strings(data, "options").unwrap_or_default(),
            labels: strings(data, "labels").unwrap_or_default(),
            exact: text(data, "exact"),
            part: text(data, "part"),
            answer: None,
        };
        let id = seams.new_id();
        let mut entry = self.new_entry(
            id.clone(),
            EntryKind::Question,
            EntryState::Open,
            true,
            text(data, "request_id"),
            seams,
        );
        entry.task_id = text(data, "task_id");
        entry.approval_id = Some(approval.clone());
        entry.body = body(&asked);
        self.create(entry, seams);
        self.questions.insert(approval, id);
    }

    /// `approval.resolved`: how the question was answered. A hidden input's words, or words FERB hid a secret in, are
    /// never kept (DMN-583).
    fn resolve(&mut self, data: &Value, seams: &mut dyn Seams) {
        let Some(id) = words(data, "approval_id").and_then(|a| self.questions.get(&a).cloned())
        else {
            return;
        };
        let Some(mut entry) = self.entries.get(&id).cloned() else {
            return;
        };
        let mut asked: QuestionBody = entry.body_as().unwrap_or_default();
        let state = text(data, "state").unwrap_or_else(|| "granted".into());
        let hidden = strings(data, "hidden").is_some_and(|h| !h.is_empty());
        asked.answer = Some(QuestionAnswer {
            how: text(data, "answer"),
            choice: words(data, "choice"),
            said: if asked.secret || hidden {
                None
            } else {
                words(data, "said")
            },
            state: state.clone(),
        });
        entry.state = match state.as_str() {
            "denied" => EntryState::Declined,
            "superseded" | "expired" => EntryState::Expired,
            _ => EntryState::Answered,
        };
        entry.open = false;
        entry.body = body(&asked);
        self.revise(entry, seams);
    }

    /// `attention.notice` (receiveNotices, addNotice): posted once per id; a notice with nothing to say isn't shown.
    fn notice(&mut self, row: &Value, seams: &mut dyn Seams) {
        let (Some(reference), Some(said)) = (words(row, "id"), words(row, "text")) else {
            return;
        };
        if self.notices.contains_key(&reference) {
            return;
        }
        let quotes = objects(row, "quotes")
            .unwrap_or_default()
            .iter()
            .filter_map(|q| {
                words(q, "text").map(|t| NoticeQuote {
                    text: t,
                    origin: text(q, "origin").unwrap_or_default(),
                })
            })
            .collect();
        let told = NoticeBody {
            text: said,
            quotes,
            cards: result_cards(row.get("cards")),
            at: text(row, "at"),
        };
        let id = seams.new_id();
        let mut entry = self.new_entry(
            id.clone(),
            EntryKind::Notice,
            EntryState::Done,
            false,
            None,
            seams,
        );
        entry.task_id = text(row, "task_id");
        entry.reference = Some(reference.clone());
        entry.body = body(&told);
        self.create(entry, seams);
        self.notices.insert(reference, id.clone());
        self.last_shown = Some(Shown::Entry(id));
    }

    /// A reminder's banner (BannerItem, receiveReminder): posted once, as `reminder:<timer id>`.
    fn notification(&mut self, item: &Value, seams: &mut dyn Seams) {
        let (Some(_), Some(_), Some(said)) =
            (words(item, "id"), text(item, "title"), text(item, "body"))
        else {
            return;
        };
        let target = item.get("target").cloned().unwrap_or(Value::Null);
        let Some(timer) = words(&target, "timer_id")
            .filter(|_| text(&target, "kind").as_deref() == Some("reminder"))
        else {
            return;
        };
        if said.is_empty() {
            return;
        }
        let reference = format!("reminder:{timer}");
        if self.notices.contains_key(&reference) {
            return;
        }
        let card = item
            .get("card")
            .filter(|c| text(c, "type").is_some_and(|t| CARD_TYPES.contains(&t.as_str())))
            .cloned();
        let post = PostBody {
            text: said,
            timer_id: timer,
            card,
        };
        let id = seams.new_id();
        let mut entry = self.new_entry(
            id.clone(),
            EntryKind::Post,
            EntryState::Done,
            false,
            None,
            seams,
        );
        entry.reference = Some(reference.clone());
        entry.body = body(&post);
        self.create(entry, seams);
        self.notices.insert(reference, id.clone());
        self.last_shown = Some(Shown::Entry(id));
    }

    // ---- the answer and its place ----

    /// ConversationArchive.finish: the turn's answer, and where it shows (APP-580). A first answer, or the answer to
    /// a question FERB asked, to a turn the chat no longer ends on is posted below, quoting it; a card fills in place;
    /// a stop's end told in another message's reply is filed on its own turn (ORCH-579). The turn's answer entry is a
    /// new one when it is the first, posted below, or from another source than the one shown; otherwise it is revised.
    /// Other words after the turn's answer are a later answer (APP-914); the turn keeps its own.
    fn finish(&mut self, t: usize, end: End, seams: &mut dyn Seams) {
        if self.is_later(t, Some(&end.said)) {
            if end.from == "task" {
                self.turns[t].ended = true;
                self.dirty.insert(t);
            }
            let said = AnswerBody {
                from: end.from.to_string(),
                text: end.said,
                display_text: end.display,
                status: end.status,
                verified: end.verified,
                artifacts: end.artifacts,
                ..AnswerBody::default()
            };
            self.answer_later(t, said, true, seams);
            return;
        }
        let before = self.turns[t].status.clone();
        let first_answer = awaiting(&before) && !awaiting(&end.status);
        let after_question =
            before == "needs_user" && end.status != "needs_user" && !awaiting(&end.status);
        {
            let turn = &mut self.turns[t];
            if after_question {
                turn.question = Some(turn.visible());
            }
            turn.text = end.said;
            turn.display_text = end.display;
            turn.status = end.status;
            turn.artifacts = end.artifacts;
            turn.response_notice = None;
            turn.verified = end.verified;
            turn.answered = true;
        }
        self.dirty.insert(t);
        let below = self.placement(t, first_answer, after_question);
        let turn = &mut self.turns[t];
        if let Some(part) = below {
            let id = seams.new_id();
            turn.replies.push(id.clone());
            turn.pending = Some(Pending {
                placement: Placement::Below,
                part: Some(part),
                id: Some(id.clone()),
            });
            self.last_shown = Some(Shown::Entry(id));
        } else if turn.pending.is_none() && (turn.answer_entry.is_none() || turn.from != end.from) {
            turn.pending = Some(Pending {
                placement: Placement::Inline,
                part: None,
                id: None,
            });
        }
        turn.from = end.from.to_string();
        if end.from == "task" {
            turn.ended = true;
        }
    }

    /// finish's placement: the part a late answer below shows, or none (answered in place).
    fn placement(&mut self, t: usize, first_answer: bool, after_question: bool) -> Option<String> {
        let turn = &mut self.turns[t];
        if std::mem::take(&mut turn.told_elsewhere) {
            return None;
        }
        if !(first_answer || after_question) || turn.plan.card.is_some() {
            return None;
        }
        if first_answer && !turn.replies.is_empty() {
            return None;
        }
        let shown_at = match turn.replies.last() {
            Some(r) => Shown::Entry(r.clone()),
            None => Shown::Turn(t),
        };
        if Some(&shown_at) == self.last_shown.as_ref() {
            return None;
        }
        Some(
            if turn.status == "needs_user" {
                "question"
            } else {
                "answer"
            }
            .into(),
        )
    }

    // ---- entries ----

    /// Each changed turn's entries, created or revised.
    fn emit(&mut self, seams: &mut dyn Seams) {
        let dirty: Vec<usize> = std::mem::take(&mut self.dirty).into_iter().collect();
        for t in dirty {
            self.emit_message(t, seams);
            self.emit_plan(t, seams);
            self.emit_answer(t, seams);
        }
    }

    fn emit_message(&mut self, t: usize, seams: &mut dyn Seams) {
        let turn = &self.turns[t];
        if turn.recovered.is_some() {
            return;
        }
        let Some(request) = turn.request_id.clone() else {
            return;
        };
        let (state, open) = category(&turn.status, false);
        let said = MessageBody {
            text: turn.request.clone(),
            hidden: turn.hidden.clone(),
            sealed: turn.sealed.clone(),
            attachments: turn.attachments.clone(),
            merged_into: turn.merged_into.clone(),
            notice: turn
                .response_notice
                .clone()
                .filter(|_| turn.status == "interrupted"),
        };
        let task = turn.task_id.clone();
        let (client, submission) = (turn.client_id.clone(), turn.submission_id.clone());
        let mut entry = match self.stored(&self.turns[t].message_entry) {
            Some(e) => e,
            None => {
                let id = seams.new_id();
                self.turns[t].message_entry = Some(id.clone());
                self.new_entry(id, EntryKind::Message, state, open, Some(request), seams)
            }
        };
        entry.state = state;
        entry.open = open;
        entry.task_id = task;
        entry.origin = Some("owner".into());
        if client.is_some() {
            entry.client_id = client;
            entry.submission_id = submission;
        }
        entry.body = body(&said);
        self.put(entry, seams);
    }

    fn emit_plan(&mut self, t: usize, seams: &mut dyn Seams) {
        let turn = &self.turns[t];
        if !turn.planned {
            return;
        }
        let (state, open) = if turn.cut {
            (EntryState::Interrupted, false)
        } else if turn.ended {
            category(&turn.status, false)
        } else {
            (EntryState::Open, true)
        };
        let (request, task, plan) = (
            turn.request_id.clone(),
            turn.task_id.clone(),
            turn.plan.clone(),
        );
        let mut entry = match self.stored(&self.turns[t].plan_entry) {
            Some(e) => e,
            None => {
                let id = seams.new_id();
                self.turns[t].plan_entry = Some(id.clone());
                self.new_entry(id, EntryKind::Plan, state, open, request, seams)
            }
        };
        entry.state = state;
        entry.open = open;
        entry.task_id = task;
        entry.body = body(&plan);
        self.put(entry, seams);
    }

    fn emit_answer(&mut self, t: usize, seams: &mut dyn Seams) {
        let turn = &self.turns[t];
        if !turn.answered {
            return;
        }
        let (state, open) = category(&turn.status, turn.asks);
        let kind = if turn.from == "error" {
            EntryKind::Error
        } else {
            EntryKind::Answer
        };
        let mut answer = AnswerBody {
            from: turn.from.clone(),
            text: turn.text.clone(),
            display_text: turn.display_text.clone(),
            status: turn.status.clone(),
            verified: turn.verified,
            artifacts: turn.artifacts.clone(),
            note: turn.note.clone(),
            lines: turn.code_lines.clone(),
            runtime_switch: turn.runtime_switch.clone(),
            cards: turn.cards.clone(),
            question: turn.question.clone(),
            part: None,
            asks: turn.asks,
            name: turn.recovered.as_ref().and_then(|r| r.0.clone()),
            goal: turn.recovered.as_ref().and_then(|r| r.1.clone()),
            follows: None,
        };
        let (request, task, message) = (
            turn.request_id.clone(),
            turn.task_id.clone(),
            turn.message_entry.clone(),
        );
        let mut entry = match self.turns[t].pending.take() {
            Some(pending) => {
                let id = pending.id.unwrap_or_else(|| seams.new_id());
                self.turns[t].answer_entry = Some(id.clone());
                answer.part = pending.part;
                let mut e = self.new_entry(id, kind, state, open, request, seams);
                e.placement = Some(pending.placement);
                e
            }
            None => match self.stored(&self.turns[t].answer_entry) {
                Some(e) => {
                    answer.part = e.body_as::<AnswerBody>().and_then(|b| b.part);
                    e
                }
                None => return,
            },
        };
        entry.kind = kind;
        entry.state = state;
        entry.open = open;
        entry.task_id = task;
        entry.answers = message;
        entry.body = body(&answer);
        self.put(entry, seams);
    }

    fn stored(&self, id: &Option<String>) -> Option<Entry> {
        id.as_ref().and_then(|id| self.entries.get(id)).cloned()
    }

    fn new_entry(
        &mut self,
        id: String,
        kind: EntryKind,
        state: EntryState,
        open: bool,
        request_id: Option<String>,
        seams: &mut dyn Seams,
    ) -> Entry {
        let now = seams.now();
        let pos = self.next_pos;
        self.next_pos += 1;
        Entry {
            id,
            conversation_id: self.conversation_id.clone(),
            pos,
            seq: 0,
            rev: 0,
            kind,
            state,
            open,
            request_id,
            task_id: None,
            approval_id: None,
            reference: None,
            answers: None,
            placement: None,
            origin: None,
            client_id: None,
            submission_id: None,
            journal_id: None,
            day: now.chars().take(10).collect(),
            created_at: now.clone(),
            updated_at: now,
            body: Value::Null,
            deleted: false,
        }
    }

    /// An entry created (`rev` 0 so far) or revised.
    fn put(&mut self, entry: Entry, seams: &mut dyn Seams) {
        if entry.rev == 0 {
            self.create(entry, seams);
        } else {
            self.revise(entry, seams);
        }
    }

    /// Every entry of a message's turn names the client that sent the message (ENG-14: a client tells the turns it
    /// sent from the others by any of their entries, a revision read after a relaunch included).
    fn stamp(&self, entry: &mut Entry) {
        if entry.kind == EntryKind::Message || entry.client_id.is_some() {
            return;
        }
        if let Some(t) = entry
            .request_id
            .as_ref()
            .and_then(|r| self.by_request.get(r))
        {
            entry.client_id = self.turns[*t].client_id.clone();
        }
    }

    fn create(&mut self, mut entry: Entry, seams: &mut dyn Seams) {
        self.stamp(&mut entry);
        entry.rev = 1;
        entry.seq = seams.next_seq();
        self.order.push(entry.id.clone());
        self.entries.insert(entry.id.clone(), entry.clone());
        self.made.push(Change {
            seq: entry.seq,
            entry,
        });
    }

    /// A revision only when something stored changed.
    fn revise(&mut self, mut entry: Entry, seams: &mut dyn Seams) {
        self.stamp(&mut entry);
        let Some(stored) = self.entries.get(&entry.id) else {
            return;
        };
        if stored.same_content(&entry) {
            return;
        }
        entry.rev = stored.rev + 1;
        entry.seq = seams.next_seq();
        entry.updated_at = seams.now();
        self.entries.insert(entry.id.clone(), entry.clone());
        self.made.push(Change {
            seq: entry.seq,
            entry,
        });
    }
}

/// One end of a turn as finish takes it.
#[derive(Debug, Clone, Default)]
struct End {
    said: String,
    status: String,
    artifacts: Vec<String>,
    display: Option<String>,
    verified: bool,
    from: &'static str,
}

/// A turn that has no answer yet (`awaitingAnswer`).
fn awaiting(status: &str) -> bool {
    status == "working" || status == "interrupted"
}

/// An entry's state for a turn's status, and whether it waits on FERB or the owner.
fn category(status: &str, asks: bool) -> (EntryState, bool) {
    match status {
        "working" | "needs_user" => (EntryState::Open, true),
        "interrupted" => (EntryState::Interrupted, false),
        "merged" => (EntryState::Merged, false),
        "failed" | "blocked" => (EntryState::Failed, asks),
        "cancelled" | "stopped" => (EntryState::Cancelled, asks),
        _ => (EntryState::Done, asks),
    }
}

// ---- the card and the checklist (TurnCard.swift, StepChecklist.swift) ----

/// ConversationArchive.place(card): a newer picture of the same card merges in; another card replaces it.
fn place_card(plan: &mut PlanBody, card: Card) {
    match &mut plan.card {
        Some(shown) if shown.request_id == card.request_id && shown.task_id == card.task_id => {
            merge_card(shown, &card)
        }
        _ => plan.card = Some(card),
    }
}

/// ConversationArchive.place(checklist): a newer picture of the same running checklist takes its steps; an ended
/// one stays; another task's replaces it.
fn place_checklist(plan: &mut PlanBody, checklist: Checklist) {
    match &mut plan.checklist {
        Some(shown) if shown.task_id == checklist.task_id && !shown.finished => {
            shown.steps = checklist.steps
        }
        Some(shown) if shown.task_id == checklist.task_id => {}
        _ => plan.checklist = Some(checklist),
    }
}

/// TurnCard(payload, taskID:).
fn card_from(payload: Option<&Value>, task: &str) -> Option<Card> {
    let payload = payload?;
    let rows = objects(payload, "rows")?;
    let rows: Vec<CardRow> = rows.iter().filter_map(|r| row_from(r, task)).collect();
    Some(Card {
        task_id: task.to_string(),
        request_id: text(payload, "request_id").unwrap_or_default(),
        done: number(payload, "done").unwrap_or(0),
        total: number(payload, "total").unwrap_or(rows.len() as i64),
        rows,
        line: words(payload, "line"),
        lines: strings(payload, "lines").unwrap_or_default(),
        headline: words(payload, "headline"),
        plan: plan_steps(payload.get("plan")),
    })
}

/// CardRow(value, taskID:).
fn row_from(value: &Value, task: &str) -> Option<CardRow> {
    let part = words(value, "part")?;
    let mut row = CardRow {
        task_id: words(value, "task_id").unwrap_or_else(|| task.to_string()),
        part,
        label: text(value, "label").unwrap_or_default(),
        state: text(value, "state").unwrap_or_else(|| "working".into()),
        direct: flag(value, "direct").unwrap_or(false),
        step: text(value, "step"),
        step_task_id: text(value, "step_task_id"),
        card_request_id: text(value, "card_request_id"),
        seq: text(value, "seq"),
        ..CardRow::default()
    };
    take(&mut row, value);
    Some(row)
}

/// CardRow.take: the row's changeable fields (each event is the row's whole state).
fn take(row: &mut CardRow, value: &Value) {
    row.text = words(value, "text");
    row.note = words(value, "note");
    row.why = words(value, "why");
    row.chosen = words(value, "chosen");
    row.verified = flag(value, "verified");
    row.question = if row.state == "asks" || row.state == "paused" {
        value.get("question").and_then(|q| {
            Some(RowQuestion {
                approval_id: words(q, "approval_id")?,
                text: text(q, "text").unwrap_or_default(),
                options: strings(q, "options").unwrap_or_default(),
            })
        })
    } else {
        None
    };
}

/// CardRow.sortsAfter: `new` is a later change than `current` (a change without a seq applies only to a row without).
fn sorts_after(new: Option<&str>, current: Option<&str>) -> bool {
    match (
        new.filter(|n| !n.is_empty()),
        current.filter(|c| !c.is_empty()),
    ) {
        (None, current) => current.is_none(),
        (Some(_), None) => true,
        (Some(n), Some(c)) => n > c,
    }
}

/// CardRow.apply: a change newer than the row's.
fn apply_row(row: &mut CardRow, value: &Value) -> bool {
    let seq = text(value, "seq");
    if !sorts_after(seq.as_deref(), row.seq.as_deref()) {
        return false;
    }
    if let Some(label) = words(value, "label") {
        row.label = label;
    }
    if let Some(state) = text(value, "state") {
        row.state = state;
    }
    if let Some(step) = text(value, "step") {
        row.step = Some(step);
    }
    if let Some(step) = words(value, "step_task_id") {
        row.step_task_id = Some(step);
    }
    if let Some(card) = words(value, "card_request_id") {
        row.card_request_id = Some(card);
    }
    if seq.is_some() {
        row.seq = seq;
    }
    take(row, value);
    true
}

fn is_done(row: &CardRow) -> bool {
    row.state == "done" || row.state == "updated"
}

/// TurnCard.apply(part:): a row's change; a part this card doesn't list joins it when the event names its turn.
fn apply_part_to(card: &mut Card, event: &Value) -> bool {
    let Some(part) = text(event, "part") else {
        return false;
    };
    if let Some(i) = card.rows.iter().position(|r| r.part == part) {
        let was = is_done(&card.rows[i]);
        if !apply_row(&mut card.rows[i], event) {
            return false;
        }
        if !take_count(card, event) {
            let now = is_done(&card.rows[i]);
            card.done = (card.done + i64::from(now) - i64::from(was))
                .min(card.total)
                .max(0);
        }
        return true;
    }
    if text(event, "request_id").as_deref() != Some(card.request_id.as_str()) {
        return false;
    }
    let Some(row) = row_from(event, &card.task_id) else {
        return false;
    };
    let done = is_done(&row);
    card.rows.push(row);
    if !take_count(card, event) {
        card.total = card.total.max(card.rows.len() as i64);
        if done {
            card.done = (card.done + 1).min(card.total);
        }
    }
    true
}

/// TurnCard.takeCount: the daemon's count after the change, when the event carries it.
fn take_count(card: &mut Card, event: &Value) -> bool {
    let (Some(done), Some(total)) = (number(event, "done"), number(event, "total")) else {
        return false;
    };
    card.total = total.max(card.rows.len() as i64);
    card.done = done.min(card.total).max(0);
    true
}

/// TurnCard.merge: each row taken when newer than the one shown; the counts and the footer from the snapshot.
fn merge_card(shown: &mut Card, snapshot: &Card) {
    let mut merged = Vec::new();
    for incoming in &snapshot.rows {
        match shown.rows.iter().find(|r| r.part == incoming.part) {
            Some(r) if !sorts_after(incoming.seq.as_deref(), r.seq.as_deref()) => {
                merged.push(r.clone())
            }
            _ => merged.push(incoming.clone()),
        }
    }
    for r in &shown.rows {
        if !snapshot.rows.iter().any(|s| s.part == r.part) {
            merged.push(r.clone());
        }
    }
    shown.rows = merged;
    if !snapshot.plan.is_empty() {
        shown.plan = snapshot.plan.clone();
    }
    shown.done = snapshot.done;
    shown.total = snapshot.total.max(shown.rows.len() as i64);
    if snapshot.line.is_some() {
        shown.line = snapshot.line.clone();
        shown.lines = snapshot.lines.clone();
        shown.headline = snapshot.headline.clone();
    }
}

/// StepChecklist(payload, taskID:) and its step states' statuses (what an ended checklist keeps).
fn checklist_from(
    payload: Option<&Value>,
    task: &str,
) -> Option<(Checklist, Vec<(String, String)>)> {
    let payload = payload.filter(|p| p.is_object())?;
    let steps = plan_steps(payload.get("steps"));
    if steps.is_empty() {
        return None;
    }
    let states = payload
        .get("states")
        .and_then(Value::as_object)
        .map(|m| {
            m.iter()
                .filter_map(|(k, v)| words(v, "status").map(|s| (k.clone(), s)))
                .collect()
        })
        .unwrap_or_default();
    Some((
        Checklist {
            task_id: task.to_string(),
            steps,
            ended: Map::new(),
            finished: false,
        },
        states,
    ))
}

/// PlanStep.list.
fn plan_steps(value: Option<&Value>) -> Vec<PlanStep> {
    let Some(items) = value.and_then(Value::as_array) else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|v| {
            let id = words(v, "id")?;
            Some(PlanStep {
                label: words(v, "label").unwrap_or_else(|| id.clone()),
                kind: text(v, "kind").unwrap_or_else(|| "do".into()),
                depends_on: strings(v, "depends_on").unwrap_or_default(),
                step_task_id: text(v, "step_task_id").unwrap_or_default(),
                repeats: flag(v, "loop").unwrap_or(false),
                id,
            })
        })
        .collect()
}

// ---- reading a frame as Swift's `as?` reads it ----

/// `value[key] as? String`.
fn text(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(Value::as_str).map(str::to_string)
}

/// A nonempty `as? String`.
fn words(value: &Value, key: &str) -> Option<String> {
    text(value, key).filter(|s| !s.is_empty())
}

/// `as? [String]`: every element a string, or none.
fn strings(value: &Value, key: &str) -> Option<Vec<String>> {
    value
        .get(key)?
        .as_array()?
        .iter()
        .map(|v| v.as_str().map(str::to_string))
        .collect()
}

/// `as? [[String: Any]]`: every element an object, or none.
fn objects(value: &Value, key: &str) -> Option<Vec<Value>> {
    let items = value.get(key)?.as_array()?;
    items.iter().all(Value::is_object).then(|| items.clone())
}

/// `as? Bool`.
fn flag(value: &Value, key: &str) -> Option<bool> {
    value.get(key).and_then(Value::as_bool)
}

/// `(as? NSNumber)?.intValue`.
fn number(value: &Value, key: &str) -> Option<i64> {
    match value.get(key)? {
        Value::Number(n) => n.as_i64().or_else(|| n.as_f64().map(|f| f as i64)),
        Value::Bool(b) => Some(i64::from(*b)),
        _ => None,
    }
}

/// An outcome's files FERB.app keeps: absolute paths only.
fn paths(event: &Value) -> Vec<String> {
    strings(event, "artifacts")
        .unwrap_or_default()
        .into_iter()
        .filter(|p| p.starts_with('/'))
        .collect()
}

/// RuntimeSwitch(value): an offer needs its choices.
fn runtime_switch(value: Option<&Value>) -> Option<Value> {
    let value = value?;
    strings(value, "choices")
        .filter(|c| !c.is_empty())
        .map(|_| value.clone())
}

/// ResultCard.list: the typed cards FERB.app draws, or none when there are none.
fn result_cards(value: Option<&Value>) -> Option<Vec<Value>> {
    let cards: Vec<Value> = value?
        .as_array()?
        .iter()
        .filter(|c| text(c, "type").is_some_and(|t| CARD_TYPES.contains(&t.as_str())))
        .cloned()
        .collect();
    (!cards.is_empty()).then_some(cards)
}
