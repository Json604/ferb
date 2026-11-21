//! The chat as the engine keeps it (stage 2, ENG-8): one record per thing the owner sees in a conversation, written
//! by the engine where it emits today's `chat.send` frames and bus events, so every client (FERB.app, the phones)
//! reads the same conversation. Pure data: no I/O. This is a client contract: field names are snake_case and stable;
//! a field is added, never renamed or retyped.
//!
//! An entry has a creation order (`pos`, never changed) and a change number (`seq`, from one per-conversation counter,
//! bumped at creation and at every revision): "what changed after X" is `seq > X`. `rev` counts an entry's revisions
//! (1 at creation). The body is redacted JSON whose shape depends on the kind ([`MessageBody`], [`PlanBody`],
//! [`AnswerBody`], [`QuestionBody`], [`NoticeBody`], [`PostBody`], [`LegacyBody`]).

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// What an entry is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntryKind {
    /// The owner's message (a `chat.send` the engine accepted).
    Message,
    /// FERB's plan for a message: its plan line, the card of a multi-part request or the checklist of a single ask.
    Plan,
    /// FERB's answer: a reply, a clarifying question, a task's end, a result FERB reports with no message (a resumed
    /// or scheduled task).
    Answer,
    /// A question FERB asked the owner (an approval): its card, then how it was answered.
    Question,
    /// Something FERB noticed and tells the owner (`attention.notice`).
    Notice,
    /// A post FERB makes at its own time: a reminder or timer coming due.
    Post,
    /// FERB could not take the message (an `error` frame).
    Error,
    /// An entry imported from FERB.app's own saved chat (its old JSON, kept byte for byte in the body).
    Legacy,
}

impl EntryKind {
    pub fn as_str(self) -> &'static str {
        match self {
            EntryKind::Message => "message",
            EntryKind::Plan => "plan",
            EntryKind::Answer => "answer",
            EntryKind::Question => "question",
            EntryKind::Notice => "notice",
            EntryKind::Post => "post",
            EntryKind::Error => "error",
            EntryKind::Legacy => "legacy",
        }
    }
}

/// Where an entry stands. Per kind: a message is `open` until FERB answers it, then `done`, `failed`, `cancelled`,
/// `interrupted` (its stream ended with no answer and no task) or `merged` (read together with another message,
/// ORCH-529); a plan is `open` while its task runs, then its end; an answer is its task's end (`open` while it waits
/// on the owner); a question is `open`, then `answered`, `declined` or `expired`; notices and posts are `done`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntryState {
    Open,
    Done,
    Failed,
    Cancelled,
    Interrupted,
    Answered,
    Declined,
    Expired,
    Merged,
}

/// Where an answer shows, decided once by the engine when the answer is made (APP-580): under its message, or at the
/// bottom of the chat quoting its message (the message then reads "Answered below").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Placement {
    Inline,
    Below,
}

/// One entry, as the store keeps it and every client reads it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    pub id: String,
    pub conversation_id: String,
    /// Creation order in the conversation; assigned once.
    pub pos: u64,
    /// The conversation's change number at this revision.
    pub seq: u64,
    /// 1 at creation, +1 at each revision.
    pub rev: u64,
    pub kind: EntryKind,
    pub state: EntryState,
    /// Waiting on FERB or the owner.
    pub open: bool,
    /// The message this entry belongs to (`chat.send`'s request id).
    pub request_id: Option<String>,
    pub task_id: Option<String>,
    pub approval_id: Option<String>,
    /// The id FERB gave a notice or post (`reminder:<timer id>`, a notice's id), so it is posted once.
    #[serde(rename = "ref")]
    pub reference: Option<String>,
    /// An answer's message entry.
    pub answers: Option<String>,
    /// An answer's place (answers and errors only).
    pub placement: Option<Placement>,
    /// Who the words are from (`owner` for a message); none when the frame doesn't say.
    pub origin: Option<String>,
    /// The client that sent a message, and its own id for the submission (filled when submissions carry them).
    pub client_id: Option<String>,
    pub submission_id: Option<String>,
    /// The journal row the entry was made from, when known.
    pub journal_id: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    /// The Mac's local day of `created_at` (YYYY-MM-DD).
    pub day: String,
    pub body: Value,
    pub deleted: bool,
}

impl Entry {
    /// The body read as its kind's shape.
    pub fn body_as<T: serde::de::DeserializeOwned>(&self) -> Option<T> {
        serde_json::from_value(self.body.clone()).ok()
    }

    /// The same entry apart from what a revision itself moves (`seq`, `rev`, `updated_at`): two revisions that are
    /// equal here changed nothing stored.
    pub fn same_content(&self, other: &Entry) -> bool {
        let strip = |e: &Entry| Entry {
            seq: 0,
            rev: 0,
            updated_at: String::new(),
            ..e.clone()
        };
        strip(self) == strip(other)
    }
}

/// One change: an entry created (`rev` 1) or revised, at `seq`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Change {
    pub seq: u64,
    pub entry: Entry,
}

/// Where a client has read up to: the store's epoch (a new store starts a new epoch, so an old cursor reads from the
/// start) and the last `seq` it has.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Cursor {
    pub epoch: String,
    pub seq: u64,
}

/// A message's body: the owner's words as the engine recorded and redacted them.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct MessageBody {
    /// The message as the ack carries it (redacted, untrusted-data marks removed).
    pub text: String,
    /// The kinds of secret FERB hid in it (a saved secret, a key format), never a value.
    pub hidden: Vec<String>,
    /// The files sent with it.
    pub attachments: Vec<String>,
    /// The request it was read together with (state `merged`).
    pub merged_into: Option<String>,
    /// Why an interrupted message has no answer (state `interrupted`).
    pub notice: Option<String>,
    /// The personal IDs FERB kept sealed from it (the ack's `sealed`, GATE-604): each one's kind and saved name, never
    /// a value. Added under ENG-8 as an optional field: absent when none, so every body written before reads the same.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sealed: Vec<SealedId>,
}

/// One personal ID kept sealed from a message (GATE-604): its kind ("PAN") and the name it is saved under ("PAN 2").
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct SealedId {
    pub kind: String,
    pub name: String,
}

/// One plan step as a checklist line or a card's plan names it.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct PlanStep {
    pub id: String,
    pub label: String,
    pub kind: String,
    pub depends_on: Vec<String>,
    pub step_task_id: String,
    #[serde(rename = "loop")]
    pub repeats: bool,
}

/// A question shown inside a card row.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct RowQuestion {
    pub approval_id: String,
    pub text: String,
    pub options: Vec<String>,
}

/// One row of a multi-part card (ORCH-319): one part of the owner's request.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct CardRow {
    pub task_id: String,
    pub part: String,
    pub label: String,
    /// working, done, failed, stopped, asks, paused or updated.
    pub state: String,
    pub text: Option<String>,
    pub note: Option<String>,
    pub why: Option<String>,
    pub chosen: Option<String>,
    pub verified: Option<bool>,
    pub direct: bool,
    pub step: Option<String>,
    pub step_task_id: Option<String>,
    /// The daemon's order of this row's changes (UTC time to the microsecond, ORCH-332).
    pub seq: Option<String>,
    pub question: Option<RowQuestion>,
    pub card_request_id: Option<String>,
}

/// A multi-part request's card as it stands: its rows, the counts and, once its task ended, the footer.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Card {
    /// The task whose card it is.
    pub task_id: String,
    /// The message whose card it is.
    pub request_id: String,
    pub rows: Vec<CardRow>,
    pub done: i64,
    pub total: i64,
    pub line: Option<String>,
    pub lines: Vec<String>,
    pub headline: Option<String>,
    pub plan: Vec<PlanStep>,
}

/// A single ask's checklist (APP-894): its steps and, once its task ended, what each step ended as.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Checklist {
    pub task_id: String,
    pub steps: Vec<PlanStep>,
    /// Step id → done, failed, not_finished, wont_run or stopped.
    pub ended: serde_json::Map<String, Value>,
    pub finished: bool,
}

/// A plan's body: the plan event's words and what code adds to them, and the card or checklist as it stands. Live
/// step states are never stored (they are live frames).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct PlanBody {
    /// The plan text (the plan line and code's lines in one).
    pub text: Option<String>,
    pub plan_line: Option<String>,
    pub lines: Option<Vec<String>>,
    /// The asks answered at once (ORCH-319).
    pub direct: Option<Vec<String>>,
    /// Code's receipts (Remembered, Scheduled) and their typed cards.
    pub receipts: Option<Vec<String>>,
    pub receipt_cards: Option<Vec<Value>>,
    /// The task's short name and its goal as the plan event gave them (a client makes the heading).
    pub name: Option<String>,
    pub goal: Option<String>,
    pub card: Option<Card>,
    pub checklist: Option<Checklist>,
}

/// An answer's body (and an error's).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct AnswerBody {
    /// `reply` (a planner turn's reply), `clarify` (FERB asks the owner something), `error`, or `task` (a task's end:
    /// its outcome or recorded result). A message's answer from another source is a new entry; a task's end that
    /// arrives again (its outcome, then its recorded result) revises it. Other words after the message's answer are a
    /// later answer that `follows` it (APP-914).
    pub from: String,
    pub text: String,
    pub display_text: Option<String>,
    /// The task's end as FERB reports it: done, done_unverified, done_with_gaps (ORCH-651), failed, cancelled, needs_user, …
    pub status: String,
    pub verified: bool,
    /// The files it produced (absolute paths).
    pub artifacts: Vec<String>,
    /// "Couldn't confirm: …" and code's lines of a one-ask answer. `note` is the outcome's own `note`, which only a
    /// result recorded before ORCH-606 carries; a newer outcome's `unconfirmed` stays in the record and is never shown
    /// (APP-915).
    pub note: Option<String>,
    pub lines: Option<Vec<String>>,
    /// The offer of another AI service (ORCH-315).
    pub runtime_switch: Option<Value>,
    /// Typed cards (a remembered fact, a reminder set, a file).
    pub cards: Option<Vec<Value>>,
    /// The question FERB had asked, kept where it showed once the final answer arrived.
    pub question: Option<String>,
    /// A late answer shown below: what it shows (`question` or `answer`), fixed when it was placed.
    pub part: Option<String>,
    /// FERB asks the owner something (a `clarify`).
    pub asks: bool,
    /// For an answer with no message (a resumed or scheduled task): the task's short name and goal.
    pub name: Option<String>,
    pub goal: Option<String>,
    /// Said after the message's answer `follows` (an answer entry id): another answer under the same message. The
    /// message's own answer is its latest answer entry without `follows`.
    pub follows: Option<String>,
}

/// How a question was answered.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct QuestionAnswer {
    /// granted, denied, used, superseded, expired.
    pub state: String,
    /// option, words or none.
    pub how: Option<String>,
    pub choice: Option<String>,
    /// A typed answer's first line; always null for a hidden input or an answer FERB hid a secret in (DMN-583).
    pub said: Option<String>,
}

/// A question's body: the approval as it was asked, and its answer once given.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct QuestionBody {
    pub kind: String,
    pub capability: String,
    pub title: Option<String>,
    pub reason: String,
    pub summary: Option<String>,
    pub options: Vec<String>,
    pub labels: Vec<String>,
    pub exact: Option<String>,
    pub part: Option<String>,
    /// A hidden input (a code, secret or password).
    pub secret: bool,
    pub answer: Option<QuestionAnswer>,
}

/// One quoted piece of outside text in a notice, with where it came from.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct NoticeQuote {
    pub text: String,
    pub origin: String,
}

/// A notice's body.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct NoticeBody {
    pub text: String,
    pub quotes: Vec<NoticeQuote>,
    pub cards: Option<Vec<Value>>,
    /// When FERB noticed it.
    pub at: Option<String>,
}

/// A post's body: a reminder or timer that came due.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct PostBody {
    pub text: String,
    pub timer_id: String,
    /// The reminder's card as the banner carries it (its short name, kind, due time).
    pub card: Option<Value>,
}

/// An imported entry: FERB.app's own saved form of a turn, late reply or notice, kept as it was.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct LegacyBody {
    /// turn, reply or notice.
    pub kind: String,
    pub body: Value,
}

/// A JSON value as a body (the typed shapes above always serialize).
pub fn body<T: Serialize>(shape: &T) -> Value {
    serde_json::to_value(shape).unwrap_or(Value::Null)
}

/// decision: APP-1105, APP-1106 — FERB's own alerts (the Mac getting hot, insomnia's end, a database set aside, a
/// phone unused for 30 days, a window that came forward, a page that refused a value) are shown for a while and never
/// kept in the chat. Code names an alert by its notice id: the notifier gives every notice of its `alert` category an
/// id under [`ALERT_PREFIX`]; the ids those notices had before (the families in [`EARLIER_ALERTS`]) still count, so what
/// was saved earlier is hidden too. These are FERB's own id formats, never words judged.
pub const ALERT_PREFIX: &str = "alert:";

/// The id families the `alert` notices had before they carried [`ALERT_PREFIX`] (frozen: a new alert needs nothing
/// here).
pub const EARLIER_ALERTS: [&str; 6] = [
    "insomnia-",
    "set-aside-",
    "window-shown-",
    "refused-",
    "phone-unused-",
    "screen-until-back-",
];

/// Whether a notice id names one of FERB's alerts ([`ALERT_PREFIX`], or an id of [`EARLIER_ALERTS`]).
pub fn is_alert(notice_id: &str) -> bool {
    notice_id.starts_with(ALERT_PREFIX) || EARLIER_ALERTS.iter().any(|p| notice_id.starts_with(p))
}

/// A notice entry's body marked as an alert (`"alert": true`): clients don't draw it in the transcript.
pub fn is_alert_body(body: &Value) -> bool {
    body.get("alert").and_then(Value::as_bool) == Some(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry() -> Entry {
        Entry {
            id: "e1".into(),
            conversation_id: "s1".into(),
            pos: 1,
            seq: 1,
            rev: 1,
            kind: EntryKind::Message,
            state: EntryState::Open,
            open: true,
            request_id: Some("r1".into()),
            task_id: None,
            approval_id: None,
            reference: None,
            answers: None,
            placement: None,
            origin: Some("owner".into()),
            client_id: None,
            submission_id: None,
            journal_id: None,
            created_at: "2026-01-01T09:00:00+00:00".into(),
            updated_at: "2026-01-01T09:00:00+00:00".into(),
            day: "2026-01-01".into(),
            body: body(&MessageBody {
                text: "hi".into(),
                ..MessageBody::default()
            }),
            deleted: false,
        }
    }

    // decision: ENG-8
    #[test]
    fn the_contract_is_snake_case_with_every_field_written() {
        let text = serde_json::to_string(&entry()).unwrap();
        assert_eq!(
            text,
            "{\"id\":\"e1\",\"conversation_id\":\"s1\",\"pos\":1,\"seq\":1,\"rev\":1,\"kind\":\"message\",\
             \"state\":\"open\",\"open\":true,\"request_id\":\"r1\",\"task_id\":null,\"approval_id\":null,\
             \"ref\":null,\"answers\":null,\"placement\":null,\"origin\":\"owner\",\"client_id\":null,\
             \"submission_id\":null,\"journal_id\":null,\"created_at\":\"2026-01-01T09:00:00+00:00\",\
             \"updated_at\":\"2026-01-01T09:00:00+00:00\",\"day\":\"2026-01-01\",\"body\":{\"text\":\"hi\",\
             \"hidden\":[],\"attachments\":[],\"merged_into\":null,\"notice\":null},\"deleted\":false}"
        );
        let back: Entry = serde_json::from_str(&text).unwrap();
        assert_eq!(back, entry());
        assert_eq!(back.body_as::<MessageBody>().unwrap().text, "hi");
    }

    // decision: ENG-8
    #[test]
    fn a_revision_that_moves_only_its_counters_changed_nothing() {
        let a = entry();
        let mut b = a.clone();
        b.seq = 9;
        b.rev = 2;
        b.updated_at = "later".into();
        assert!(a.same_content(&b));
        b.state = EntryState::Done;
        assert!(!a.same_content(&b));
    }

    // decision: APP-1105, APP-1106
    #[test]
    fn an_alert_is_named_by_its_id_or_marked_in_its_body() {
        assert!(is_alert("alert:insomnia-warning-w1"));
        for earlier in [
            "insomnia-ended-x",
            "set-aside-ferb.db",
            "window-shown-t-site",
            "refused-sp",
            "phone-unused-p-d",
            "screen-until-back-S-why",
        ] {
            assert!(is_alert(earlier), "{earlier}");
        }
        for kept in [
            "memory-always-on-1",
            "loop-s1-abc",
            "notice-1",
            "reminder:t1",
            "learned:2026-11-21",
        ] {
            assert!(!is_alert(kept), "{kept}");
        }
        assert!(is_alert_body(
            &serde_json::json!({"text": "t", "alert": true})
        ));
        assert!(!is_alert_body(&serde_json::json!({"text": "t"})));
    }
}
