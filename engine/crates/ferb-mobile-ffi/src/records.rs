//! What the apps read, as UniFFI records: the transcript (ferb-mobile `fold`), what is live (`live`), the outbox, the
//! conversations and the link's state, each a field-for-field copy of the core's type.
//!
//! Every conversion destructures the core's struct completely, so a field added to the core is a compile error here
//! until it is carried (the mapping is total). Each record also serializes exactly as its core type does (the same
//! serde attributes), which is how `tests/golden.rs` holds the transcript as the apps receive it to
//! `transcripts.json`. The one value that stays JSON text is a question's `body` (`body_json`): the engine's question
//! body has no fixed shape (ENG-8: kinds and fields are added, never renamed), so a typed copy could not be total.

use std::collections::{BTreeMap, HashMap};

use serde::Serialize;
use serde_json::{Map, Value};

use ferb_mobile::fold::model as fm;
use ferb_mobile::fold::{self as ff};
use ferb_mobile::live as fl;

/// A total conversion from a core type to its record (implemented by `mirror!` and for the containers).
pub(crate) trait Conv<U> {
    fn conv(self) -> U;
}

macro_rules! same {
    ($($t:ty),*) => {$(
        impl Conv<$t> for $t {
            fn conv(self) -> $t {
                self
            }
        }
    )*};
}
same!(String, bool, i64, i32, u32, u64, f64);

impl<T: Conv<U>, U> Conv<Option<U>> for Option<T> {
    fn conv(self) -> Option<U> {
        self.map(Conv::conv)
    }
}

impl<T: Conv<U>, U> Conv<Vec<U>> for Vec<T> {
    fn conv(self) -> Vec<U> {
        self.into_iter().map(Conv::conv).collect()
    }
}

impl Conv<BTreeMap<String, String>> for HashMap<String, String> {
    fn conv(self) -> BTreeMap<String, String> {
        self.into_iter().collect()
    }
}

impl Conv<HashMap<String, String>> for BTreeMap<String, String> {
    fn conv(self) -> HashMap<String, String> {
        self.into_iter().collect()
    }
}

/// A record with the core type's fields, serialized as the core serializes it, and its total conversion.
macro_rules! mirror {
    (
        $(#[$m:meta])*
        $name:ident from $core:path {
            $( $(#[$fm:meta])* $f:ident : $t:ty ),* $(,)?
        }
    ) => {
        $(#[$m])*
        #[derive(Debug, Clone, PartialEq, Serialize, uniffi::Record)]
        pub struct $name {
            $( $(#[$fm])* pub $f: $t ),*
        }

        impl Conv<$name> for $core {
            fn conv(self) -> $name {
                let $core { $($f),* } = self;
                $name { $($f: Conv::conv($f)),* }
            }
        }

        impl From<$core> for $name {
            fn from(c: $core) -> $name {
                c.conv()
            }
        }

        /// Back to the core's type (what the app hands in again, `shown`): as total as the way out.
        impl Conv<$core> for $name {
            fn conv(self) -> $core {
                let $name { $($f),* } = self;
                $core { $($f: Conv::conv($f)),* }
            }
        }
    };
}

// ---- the fold's types (engine/fixtures/chat-entries/README.md, "The phone's transcript") ---------------------------

mirror! {
    /// One of the schedules on a `schedules` card.
    ScheduleRow from fm::ScheduleRow {
        id: String,
        name: String,
        repeat_rule: String,
        state: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        next_run: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        until: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        watch: Option<String>,
    }
}

mirror! {
    /// A typed result card built from FERB's own records; `kind_of` is the daemon's `type` (reminder, schedule,
    /// payment, …), and which other fields are set depends on it.
    ResultCard from fm::ResultCard {
        #[serde(rename = "type")]
        kind_of: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        line: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        due: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        repeat_rule: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        next_run: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        schedule_id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        until: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        kind: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        what: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        when: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        watch: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        started: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        timer_id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        repeat_of: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        item_id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        text: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        state: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        amount: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        merchant: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        site: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        saved_card: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        recipient: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        app: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        account: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        host: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        path: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        page: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        imported_from: Option<String>,
        changed: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        was_repeat: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        was_request: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        request: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        count: Option<i64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        schedules: Option<Vec<ScheduleRow>>,
    }
}

mirror! {
    /// The offer of another AI service when an account problem ended the work.
    RuntimeSwitch from fm::RuntimeSwitch {
        runtime: String,
        problem: String,
        role: String,
        choices: Vec<String>,
    }
}

mirror! {
    /// A question FERB asks inside a card row.
    RowQuestion from fm::RowQuestion {
        approval_id: String,
        text: String,
        options: Vec<String>,
    }
}

mirror! {
    /// One part of the owner's request in a card.
    CardRow from fm::CardRow {
        task_id: String,
        part: String,
        label: String,
        state: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        text: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        note: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        why: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        chosen: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        verified: Option<bool>,
        direct: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        step: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        step_task_id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        seq: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        question: Option<RowQuestion>,
        #[serde(skip_serializing_if = "Option::is_none")]
        card_request_id: Option<String>,
    }
}

mirror! {
    /// One plan step as the daemon names it; `repeats` is the step's `loop`.
    PlanStep from fm::PlanStep {
        id: String,
        label: String,
        kind: String,
        depends_on: Vec<String>,
        step_task_id: String,
        #[serde(rename = "loop")]
        repeats: bool,
    }
}

mirror! {
    /// What one card row shows once read from history.
    Shows from fm::Shows {
        kind: String,
        words: Option<String>,
    }
}

mirror! {
    /// One checklist line as history shows it.
    ChecklistLine from fm::ChecklistLine {
        step: String,
        kind: String,
        words: Option<String>,
    }
}

mirror! {
    /// An identifier FERB kept sealed from a message: its kind and saved name, never a value.
    Sealed from fm::Sealed {
        kind: String,
        name: String,
    }
}

mirror! {
    /// Outside text a notice quotes, with where it came from (data, shown as a quote).
    Quote from fm::Quote {
        text: String,
        origin: String,
    }
}

mirror! {
    /// The owner's words of a turn (absent for FERB's own turn).
    OwnerWords from ff::OwnerWords {
        text: String,
        files: Vec<String>,
        hidden: Vec<String>,
        sealed: Vec<Sealed>,
        hidden_notice: Option<String>,
    }
}

mirror! {
    /// FERB's first bubble.
    FirstReply from ff::FirstReply {
        says: bool,
        direct: Vec<String>,
        receipts: Vec<String>,
        receipt_cards: Option<Vec<ResultCard>>,
        plan_line: Option<String>,
        plan_lines: Vec<String>,
        plan: Option<String>,
        /// `receipts` without those a receipt card stands for: what FERB.app's first bubble says. Not serialised.
        #[serde(skip_serializing)]
        #[uniffi(default = [])]
        receipts_shown: Vec<String>,
    }
}

mirror! {
    /// A turn's answer, or a later answer's own words (with its `status`).
    AnswerView from ff::AnswerView {
        text: String,
        display_text: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        status: Option<String>,
        verified: bool,
        artifacts: Vec<String>,
        note: Option<String>,
        lines: Option<Vec<String>>,
        cards: Option<Vec<ResultCard>>,
        runtime_switch: Option<RuntimeSwitch>,
    }
}

mirror! {
    /// A card row and what it shows.
    RowView from ff::RowView {
        #[serde(flatten)]
        row: CardRow,
        shows: Shows,
    }
}

mirror! {
    /// The card of a multi-part request.
    CardView from ff::CardView {
        task_id: String,
        request_id: String,
        rows: Vec<RowView>,
        done: i64,
        total: i64,
        #[serde(skip_serializing_if = "Option::is_none")]
        line: Option<String>,
        lines: Vec<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        headline: Option<String>,
        plan: Vec<PlanStep>,
        /// The footer's lines without those the turn's cards stand for. Not serialised.
        #[serde(skip_serializing)]
        #[uniffi(default = [])]
        lines_shown: Vec<String>,
    }
}

mirror! {
    /// A task's checklist; `ended` maps a step id to how it ended.
    ChecklistView from ff::ChecklistView {
        task_id: String,
        steps: Vec<PlanStep>,
        ended: HashMap<String, String>,
        finished: bool,
        lines: Vec<ChecklistLine>,
        collapsed: Option<String>,
    }
}

mirror! {
    /// A turn: the owner's words (or FERB's own goal), FERB's first bubble, the card or checklist, the answer.
    TurnItem from ff::TurnItem {
        entry: String,
        /// When the entry was made, seconds since 2001-01-01 (Foundation's reference date); not serialised (kb PHN-627).
        #[serde(skip_serializing)]
        at: f64,
        legacy: bool,
        client_id: Option<String>,
        request_id: Option<String>,
        task_id: Option<String>,
        from: String,
        task_name: Option<String>,
        status: String,
        status_inferred: bool,
        notice: Option<String>,
        merged_into: Option<String>,
        question: Option<String>,
        answered_below: bool,
        answer: Option<AnswerView>,
        card: Option<CardView>,
        checklist: Option<ChecklistView>,
        first_reply: Option<FirstReply>,
        parts: Vec<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        goal: Option<String>,
        #[serde(flatten)]
        words: Option<OwnerWords>,
        /// When its plan was stored (seconds since 2001), for the work row's time. Not serialised.
        #[serde(skip_serializing)]
        #[uniffi(default = None)]
        plan_at: Option<f64>,
        /// The words under its answer when it didn't end done ("Delivered, with parts not done", "Failed"). Not
        /// serialised.
        #[serde(skip_serializing)]
        #[uniffi(default = None)]
        status_words: Option<String>,
        /// The boxes (timers, reminders, schedules) a later item draws. Not serialised.
        #[serde(skip_serializing)]
        #[uniffi(default = [])]
        hidden: Vec<String>,
    }
}

mirror! {
    /// An answer drawn below, quoting the turn it answers.
    ReplyItem from ff::ReplyItem {
        entry: String,
        /// When the entry was made, seconds since 2001-01-01 (Foundation's reference date); not serialised (kb PHN-627).
        #[serde(skip_serializing)]
        at: f64,
        legacy: bool,
        answers: String,
        after: String,
        part: Option<String>,
        quotes_request: bool,
        said: Option<AnswerView>,
        /// The boxes a later item draws. Not serialised.
        #[serde(skip_serializing)]
        #[uniffi(default = [])]
        hidden: Vec<String>,
    }
}

mirror! {
    /// FERB's own line in the chat (a notice or a reminder); `reference` is the item it belongs to.
    NoticeItem from ff::NoticeItem {
        entry: String,
        /// When the entry was made, seconds since 2001-01-01 (Foundation's reference date); not serialised (kb PHN-627).
        #[serde(skip_serializing)]
        at: f64,
        legacy: bool,
        #[serde(rename = "ref")]
        reference: String,
        after: Option<String>,
        text: String,
        quotes: Vec<Quote>,
        cards: Option<Vec<ResultCard>>,
        /// The boxes a later item draws. Not serialised.
        #[serde(skip_serializing)]
        #[uniffi(default = [])]
        hidden: Vec<String>,
    }
}

/// One item of the chat, top to bottom.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, uniffi::Enum)]
pub enum Item {
    Turn { turn: TurnItem },
    Reply { reply: ReplyItem },
    Notice { notice: NoticeItem },
    Reminder { notice: NoticeItem },
}

impl Conv<Item> for ff::Item {
    fn conv(self) -> Item {
        match self {
            ff::Item::Turn(t) => Item::Turn { turn: t.conv() },
            ff::Item::Reply(r) => Item::Reply { reply: r.conv() },
            ff::Item::Notice(n) => Item::Notice { notice: n.conv() },
            ff::Item::Reminder(n) => Item::Reminder { notice: n.conv() },
        }
    }
}

impl Item {
    /// The item as the golden writes it (`{"item": kind, ...fields}`), the core `Item`'s serialization.
    pub fn to_json(&self) -> Value {
        let (kind, fields) = match self {
            Item::Turn { turn } => ("turn", serde_json::to_value(turn)),
            Item::Reply { reply } => ("reply", serde_json::to_value(reply)),
            Item::Notice { notice } => ("notice", serde_json::to_value(notice)),
            Item::Reminder { notice } => ("reminder", serde_json::to_value(notice)),
        };
        let mut out = Map::new();
        out.insert("item".into(), Value::String(kind.into()));
        if let Ok(Value::Object(m)) = fields {
            out.extend(m);
        }
        Value::Object(out)
    }
}

mirror! {
    /// A card row a question belongs to.
    RowRef from ff::RowRef {
        turn: String,
        part: String,
    }
}

/// A question entry and where the Mac draws it. `body_json` is the entry's body as JSON text (its shape has no fixed
/// set of fields); `shown` is `card_row`, `end_of_chat`, or none (no longer open).
#[derive(Debug, Clone, PartialEq, Serialize, uniffi::Record)]
pub struct QuestionItem {
    pub entry: String,
    pub approval_id: Option<String>,
    pub request_id: Option<String>,
    pub task_id: Option<String>,
    pub state: String,
    pub open: bool,
    #[serde(rename = "body", serialize_with = "as_json_value")]
    pub body_json: String,
    pub shown: Option<String>,
    pub row: Option<RowRef>,
    /// What FERB.app's card shows for it (kb PHN-739); none only in a record an app made itself.
    #[serde(skip_serializing)]
    #[uniffi(default = None)]
    pub words: Option<QuestionShown>,
}

fn as_json_value<S: serde::Serializer>(text: &str, s: S) -> Result<S::Ok, S::Error> {
    serde_json::from_str::<Value>(text)
        .unwrap_or(Value::Null)
        .serialize(s)
}

impl Conv<QuestionItem> for ff::QuestionItem {
    fn conv(self) -> QuestionItem {
        let ff::QuestionItem {
            entry,
            approval_id,
            request_id,
            task_id,
            state,
            open,
            body,
            shown,
            row,
            words,
        } = self;
        QuestionItem {
            entry,
            approval_id,
            request_id,
            task_id,
            state,
            open,
            body_json: body.to_string(),
            shown,
            row: row.conv(),
            words: words.conv(),
        }
    }
}

// ---- what is live ------------------------------------------------------------------------------------------------

mirror! {
    /// A live line: its words and when the step it describes started (ISO text).
    LiveLine from fl::LiveLine {
        text: String,
        started_at: Option<String>,
    }
}

mirror! {
    /// A card row's or checklist line's live look; `until` when it waits for a time (the app writes the time in the
    /// phone's own format).
    LiveRow from fl::LiveRow {
        id: String,
        kind: String,
        words: Option<String>,
        spinner: bool,
        until: Option<String>,
    }
}

mirror! {
    /// What a phone draws live for one turn: `state` is running, paused, sleeping or needs_you; `typing` the typing
    /// dots; `open_questions` the approval ids of its open questions.
    LiveView from fl::LiveView {
        entry: String,
        request_id: Option<String>,
        state: String,
        typing: bool,
        streaming_plan: Option<String>,
        streaming_reply: Option<String>,
        reply_notice: Option<String>,
        line: Option<LiveLine>,
        activity: Option<String>,
        rows: Vec<LiveRow>,
        checklist: Vec<LiveRow>,
        open_questions: Vec<String>,
        /// What the task is doing now (FERB.app's work row and short view); none for a turn that is no task.
        #[uniffi(default = None)]
        current: Option<String>,
        /// A running card's status line ("1 of 4 done · 2 running").
        #[uniffi(default = None)]
        card_line: Option<String>,
        /// The steps each waiting row waits on that no row answers, with their lines (APP-895).
        #[uniffi(default = [])]
        feeders: Vec<LiveFeeder>,
    }
}

mirror! {
    /// One step a waiting card row waits on that no row answers (APP-895): the row it shows under, the step, its line.
    LiveFeeder from fl::LiveFeeder {
        part: String,
        step: String,
        label: String,
        step_task_id: String,
        line: LiveRow,
    }
}

// ---- the core's own records ---------------------------------------------------------------------------------------

mirror! {
    /// A message in the outbox: `state` waiting (for the link, or the phone's unlock: `waiting_for`), sending, or
    /// failed (`reason`). `tried`: an earlier try's outcome was unknown, so the Mac may already have it.
    OutboxItem from ferb_mobile::OutboxItem {
        id: String,
        text: String,
        created_at: String,
        state: String,
        waiting_for: Option<String>,
        reason: Option<String>,
        tried: bool,
        attachments: Vec<Attachment>,
    }
}

mirror! {
    /// One of this phone's finished uploads on the Mac, sent with a message by its id (PHN-127).
    Attachment from ferb_mobile::store::Attachment {
        upload_id: String,
        name: String,
    }
}

impl From<Attachment> for ferb_mobile::store::Attachment {
    fn from(a: Attachment) -> Self {
        let Attachment { upload_id, name } = a;
        ferb_mobile::store::Attachment { upload_id, name }
    }
}

// ---- what FERB has waiting (`chat.status` as the phone gate shapes it) ---------------------------------------

mirror! {
    /// What answering one question needs from the phone: `can_answer` false: only on the Mac (`problem` says why);
    /// `needs_unlock` false (and not `hidden`): routine, answered from the notification with no prompt (PHN-417);
    /// else signed by the phone's key over `challenge`, the fingerprint-only key where `fingerprint`.
    QuestionNeeds from ferb_mobile::status::QuestionNeeds {
        can_answer: bool,
        needs_unlock: bool,
        hidden: bool,
        fingerprint: bool,
        challenge: Option<String>,
        problem: Option<String>,
        /// The screen-use question may be answered here "until I'm back at my Mac" (GATE-615): offer that button only
        /// when true, answered with `AnswerInput::UntilBack`.
        until_back: bool,
    }
}

mirror! {
    /// FERB using the Mac's screen on a phone's "until I'm back at my Mac" (GATE-615): `active` since `since`, said by
    /// the phone `phone` (its endpoint id) named `phone_name` on the Mac; `can_offer`: the Mac offers it on a phone now.
    ScreenUse from ferb_mobile::status::ScreenUse {
        active: bool,
        since: Option<String>,
        phone: Option<String>,
        phone_name: Option<String>,
        can_offer: bool,
    }
}

mirror! {
    /// What `Mobile.end_screen_use` did: `ended` (one was in force), `by` ("mac" or a phone's endpoint id), and what
    /// is in force after it.
    ScreenUseEnd from ferb_mobile::remote::ScreenUseEnd {
        ended: bool,
        by: String,
        screen_use: ScreenUse,
    }
}

mirror! {
    /// One open question; while `held` (a locked phone) its words are only which kind of question it is.
    StatusQuestion from ferb_mobile::status::StatusQuestion {
        approval_id: String,
        kind: String,
        capability: Option<String>,
        task_id: Option<String>,
        reason: String,
        options: Vec<String>,
        held: bool,
        needs: QuestionNeeds,
        /// What FERB.app's card shows for it (kb PHN-739); none only in a record an app made itself.
        #[uniffi(default = None)]
        words: Option<QuestionShown>,
        /// A question of the temporary chat's task: shown only in the temporary chat, never stored.
        #[uniffi(default = false)]
        temporary: bool,
    }
}

mirror! {
    /// A question as FERB.app shows it (ferb-mobile `question`, kb PHN-739): header, the one question, the why line,
    /// the exact thing, the task's reason, evidence, the buttons' words, the note, the typed answer's box and words,
    /// and the Mac's banner.
    QuestionShown from ferb_mobile::question::QuestionShown {
        header: String,
        secret: bool,
        from_task: bool,
        question: String,
        plain: bool,
        picture: bool,
        picture_problem: Option<String>,
        detail: Option<String>,
        exact: Option<String>,
        why: Option<String>,
        evidence: Vec<String>,
        buttons: Vec<String>,
        note: Option<String>,
        answer_box: String,
        field: Option<String>,
        decline: Option<String>,
        send: Option<String>,
        banner_title: Option<String>,
        banner_text: Option<String>,
        /// FERB's review of what an import brought in (MEM-460): drawn as the review's list (`Mobile.memory_review`).
        #[uniffi(default = false)]
        review: bool,
    }
}

mirror! {
    /// A notification's title and text in FERB.app's words (ferb-mobile `notify`, kb PHN-795).
    NotificationWords from ferb_mobile::notify::NotificationWords {
        title: String,
        text: String,
    }
}

mirror! {
    /// A task that runs now.
    StatusTask from ferb_mobile::status::StatusTask {
        task_id: String,
        name: String,
        goal: String,
        started_at: Option<String>,
        held: bool,
        /// A task of the temporary chat: shown only in the temporary chat, never stored.
        #[uniffi(default = false)]
        temporary: bool,
    }
}

mirror! {
    /// A task that finished since the Mac last showed it.
    StatusResult from ferb_mobile::status::StatusResult {
        task_id: String,
        request_id: Option<String>,
        at: Option<String>,
        status: String,
        name: String,
        answer: String,
        held: bool,
    }
}

mirror! {
    /// What FERB has waiting, as this phone may see it now (PHN-125, PHN-126): `locked` (no unlock window on the Mac),
    /// `words` (given: unlocked, or a locked phone that proved its device key), `stopped` (Stop everything is on).
    PhoneStatus from ferb_mobile::status::PhoneStatus {
        locked: bool,
        words: bool,
        stopped: bool,
        questions: Vec<StatusQuestion>,
        running: Vec<StatusTask>,
        results: Vec<StatusResult>,
        /// FERB using the screen "until I'm back" (none from a Mac that doesn't say).
        screen_use: Option<ScreenUse>,
        /// Insomnia mode (APP-980..APP-985); none from a Mac that doesn't say (an older FERB).
        #[uniffi(default = None)]
        insomnia: Option<Insomnia>,
        /// FERB's alerts not yet put away (APP-1106, PHN-791): shown above the chat, never in it; empty while held.
        #[uniffi(default = [])]
        alerts: Vec<StatusAlert>,
        /// The memory import's live card (MEM-456), drawn at the end of today's chat; none while nothing goes on, while
        /// the words are held back, or from an older Mac.
        #[uniffi(default = None)]
        memory_import: Option<ImportCard>,
        /// The temporary chat while one is on (its id, rev and start); none while none is on or from an older Mac.
        /// When it appears, goes or its rev moves, `Change.Temporary` says so: read `temporary_chat` again.
        #[uniffi(default = None)]
        temporary: Option<TemporaryState>,
    }
}

mirror! {
    /// The temporary chat as `chat.status` names it while one is on: its id, its change number and its start.
    TemporaryState from ferb_mobile::status::TemporaryState {
        id: String,
        rev: i64,
        started_at: Option<String>,
    }
}

mirror! {
    /// One source on the memory import's card: a bar from `done` of `total` (none while `total` is 0) with `count`
    /// beside it, `found_line` under it, `detail` quieter, `note` set apart. `key` is stable while the row lives.
    ImportRow from ferb_mobile::status::ImportRow {
        key: String,
        source: String,
        name: String,
        stage: String,
        line: String,
        detail: String,
        done: u32,
        total: u32,
        count: String,
        found: u32,
        found_line: String,
        note: String,
    }
}

mirror! {
    /// The memory import's live card (MEM-456), in the Mac's words; each row names its source only when there are
    /// more than one.
    ImportCard from ferb_mobile::status::ImportCard {
        title: String,
        rows: Vec<ImportRow>,
    }
}

mirror! {
    /// One of FERB's alerts (APP-1105, APP-1106): the Mac's words, shown above the chat until put away on any device
    /// (`Mobile::dismiss_alert`).
    StatusAlert from ferb_mobile::status::StatusAlert {
        id: String,
        text: String,
        at: Option<String>,
    }
}

// ---- insomnia mode (APP-980..APP-985) ----------------------------------------------------------------------------

mirror! {
    /// The Mac's power in insomnia mode (APP-981); any part may be unknown. `thermal`: nominal, fair, serious, critical.
    InsomniaPower from ferb_mobile::status::InsomniaPower {
        battery: Option<u32>,
        on_battery: Option<bool>,
        lid_closed: Option<bool>,
        thermal: Option<String>,
    }
}

mirror! {
    /// The camera in insomnia mode: on only while a phone pulls frames; `state` allowed, denied, not_asked, unknown.
    InsomniaCamera from ferb_mobile::status::InsomniaCamera {
        on: bool,
        state: String,
    }
}

mirror! {
    /// A warning while the mode is on (APP-981): `kind` battery_low, battery_urgent, hot, shield_down; `text` the Mac's.
    /// A notification of its own until it is gone from the status.
    InsomniaWarning from ferb_mobile::status::InsomniaWarning {
        id: String,
        kind: String,
        text: String,
        at: Option<String>,
    }
}

mirror! {
    /// Someone at the Mac (APP-983): `kind` touched or activation (`app` names it); `count` times in this episode.
    /// Answered by Show me (`watch_camera`), Lock my Mac (`lock_mac`) or It's fine (`dismiss_insomnia_notice`).
    InsomniaNotice from ferb_mobile::status::InsomniaNotice {
        id: String,
        kind: String,
        at: Option<String>,
        text: String,
        app: Option<String>,
        count: u32,
    }
}

mirror! {
    /// How the mode last ended (kept by the Mac 10 minutes): said once, quietly.
    InsomniaLastEnd from ferb_mobile::status::InsomniaLastEnd {
        id: String,
        why: String,
        text: String,
        at: Option<String>,
    }
}

mirror! {
    /// FERB's lid helper on the Mac: its `state`, a failed set-up's `error`, and `problem`: plain words for why it
    /// isn't on (shown under the lid line), none when nothing is wrong.
    InsomniaHelper from ferb_mobile::status::InsomniaHelper {
        state: String,
        error: Option<String>,
        problem: Option<String>,
    }
}

mirror! {
    /// Insomnia mode: FERB keeps the Mac working while the owner is away (APP-980). `line` is the Mac's own status line
    /// (empty while off); `notice` someone at the Mac; `warnings` battery, heat, the cover; `last_end` how it ended.
    Insomnia from ferb_mobile::status::Insomnia {
        on: bool,
        variant: Option<String>,
        since: Option<String>,
        by: Option<String>,
        screen_yes: bool,
        grace_minutes: Option<u32>,
        working: u32,
        ends_at: Option<String>,
        shield_up: bool,
        mac_locked: bool,
        camera: InsomniaCamera,
        power: InsomniaPower,
        warnings: Vec<InsomniaWarning>,
        notice: Option<InsomniaNotice>,
        line: String,
        task_awake: bool,
        last_end: Option<InsomniaLastEnd>,
        /// The Mac's own words for closing the lid on battery (true to its helper's state); none from an older Mac.
        #[uniffi(default = None)]
        lid_line: Option<String>,
        /// ready, absent or unknown: whether FERB can keep working with the lid closed on battery.
        #[uniffi(default = None)]
        closed_lid: Option<String>,
        /// FERB's lid helper on the Mac; none from an older Mac.
        #[uniffi(default = None)]
        helper: Option<InsomniaHelper>,
        /// The Mac is asleep and only briefly awake now (a Power Nap wake): replies are slow, screen work waits.
        #[uniffi(default = false)]
        asleep: bool,
        /// The Mac's own words for that while it is so; none otherwise or from an older Mac.
        #[uniffi(default = None)]
        asleep_line: Option<String>,
    }
}

mirror! {
    /// What `Mobile.start_insomnia` did: `already` (it was on: nothing changed) and the mode as it stands.
    InsomniaStarted from ferb_mobile::remote::InsomniaStarted {
        already: bool,
        insomnia: Insomnia,
    }
}

mirror! {
    /// What `Mobile.end_insomnia` did: `ended` (it was on), `by` (who ended it), and the mode after it.
    InsomniaEnded from ferb_mobile::remote::InsomniaEnded {
        ended: bool,
        by: Option<String>,
        insomnia: Insomnia,
    }
}

mirror! {
    /// What Lock my Mac did (`Mobile.lock_mac`): `locked`, the Mac's words, and the mode after it when the Mac gave it.
    MacLocked from ferb_mobile::remote::MacLocked {
        locked: bool,
        text: String,
        insomnia: Option<Insomnia>,
    }
}

/// One answer of the Mac's camera in insomnia mode (APP-983): the frame (JPEG, in memory only) when the Mac sent a
/// new one, its number, the state (starting, live, off, denied, failed) and the Mac's words for it.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct CameraFrame {
    pub jpeg: Option<Vec<u8>>,
    pub seq: Option<i64>,
    pub state: String,
    pub text: Option<String>,
}

impl Conv<CameraFrame> for ferb_mobile::remote::CameraFrame {
    fn conv(self) -> CameraFrame {
        let ferb_mobile::remote::CameraFrame {
            jpeg,
            seq,
            state,
            text,
        } = self;
        CameraFrame {
            jpeg,
            seq,
            state,
            text,
        }
    }
}

// ---- files, the preview, settings, tasks, the Mac ------------------------------------------------------------

mirror! {
    /// One entry of a folder on the Mac. `locked`: a key place: each access needs a fresh fingerprint (PHN-121).
    FileEntry from ferb_mobile::remote::FileEntry {
        name: String,
        path: String,
        dir: bool,
        link: bool,
        size: u64,
        modified: i64,
        locked: bool,
    }
}

mirror! {
    /// One page of a folder's listing (`total` entries; this page starts at index `from`). `cut`: more remain, and
    /// `next` is the marker `Mobile.files` takes as `after` for the next page ("load more", PHN-589).
    Folder from ferb_mobile::remote::Folder {
        path: String,
        parent: Option<String>,
        locked: bool,
        entries: Vec<FileEntry>,
        total: u64,
        cut: bool,
        from: u64,
        next: Option<String>,
    }
}

mirror! {
    TreeFile from ferb_mobile::remote::TreeFile {
        rel: String,
        size: u64,
    }
}

mirror! {
    /// Every file under a folder, to fetch it whole; `skipped` files (key places, FERB's own data) ask by themselves.
    Tree from ferb_mobile::remote::Tree {
        path: String,
        name: String,
        files: Vec<TreeFile>,
        bytes: u64,
        skipped: u64,
        cut: bool,
    }
}

mirror! {
    /// A file fetched into `saved_to` (or the part so far: `received` of `total` after its offset).
    Fetched from ferb_mobile::remote::Fetched {
        name: String,
        saved_to: String,
        received: u64,
        total: u64,
    }
}

mirror! {
    /// A file sent to the Mac: `upload_id` rides with a message (`send_with`); `saved_in` the folder it landed in,
    /// said plainly ("Downloads › FERB from phone", PHN-581).
    Uploaded from ferb_mobile::remote::Uploaded {
        upload_id: String,
        saved: String,
        size: u64,
        saved_in: Option<String>,
    }
}

mirror! {
    /// A folder sent from the phone, begun (`Mobile.begin_group`): `group` its files carry, `saved_in` where it lands.
    GroupStarted from ferb_mobile::remote::GroupStarted {
        group: String,
        saved_in: Option<String>,
    }
}

mirror! {
    /// Where files sent from the phone land on the Mac (PHN-580): `path`, `shown` (said plainly), `is_default` (FERB's
    /// own folder). Changed only in FERB.app on the Mac.
    UploadFolder from ferb_mobile::remote::UploadFolder {
        path: String,
        shown: String,
        is_default: bool,
    }
}

mirror! {
    /// One of FERB's settings: its key and value as JSON text.
    SettingValue from ferb_mobile::remote::SettingValue {
        key: String,
        value_json: String,
    }
}

mirror! {
    /// One of the Mac's tasks.
    TaskRow from ferb_mobile::remote::TaskRow {
        task_id: String,
        name: String,
        goal: String,
        status: String,
        created_at: Option<String>,
        updated_at: Option<String>,
        answer: String,
        steps_done: Option<u32>,
        steps_total: Option<u32>,
    }
}

mirror! {
    TaskPage from ferb_mobile::remote::TaskPage {
        tasks: Vec<TaskRow>,
        more: bool,
    }
}

mirror! {
    /// What the Mac says about itself and this phone: its name, this phone's pairing date (ISO 8601) and name there.
    MacInfo from ferb_mobile::remote::MacInfo {
        name: Option<String>,
        paired_at: Option<String>,
        phone_name: Option<String>,
        upload_folder: Option<UploadFolder>,
    }
}

/// One live preview answer: the frame (JPEG, in memory only, PHN-454) when the Mac sent a new one, its number, the
/// state and line FERB.app shows over it, and whether the last frame goes (`gone`).
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct PreviewFrame {
    pub jpeg: Option<Vec<u8>>,
    pub seq: Option<i64>,
    pub state: String,
    pub line: Option<String>,
    pub fresh: bool,
    pub busy: bool,
    pub gone: bool,
    pub empty_text: Option<String>,
}

impl Conv<PreviewFrame> for ferb_mobile::remote::PreviewFrame {
    fn conv(self) -> PreviewFrame {
        let ferb_mobile::remote::PreviewFrame {
            jpeg,
            seq,
            state,
            line,
            fresh,
            busy,
            gone,
            empty_text,
        } = self;
        PreviewFrame {
            jpeg,
            seq,
            state,
            line,
            fresh,
            busy,
            gone,
            empty_text,
        }
    }
}

mirror! {
    /// What cancelling did: an earlier try's outcome was unknown, so the Mac may have the message already.
    Cancelled from ferb_mobile::Cancelled {
        may_have_arrived: bool,
    }
}

/// A conversation held on the phone.
#[derive(Debug, Clone, PartialEq, Serialize, uniffi::Record)]
pub struct ConversationInfo {
    pub id: String,
    pub current: bool,
    pub entries: i64,
    pub updated_at: String,
}

impl Conv<ConversationInfo> for ferb_mobile::ConversationInfo {
    fn conv(self) -> ConversationInfo {
        let ferb_mobile::ConversationInfo {
            id,
            current,
            entries,
            updated_at,
        } = self;
        ConversationInfo {
            id,
            current,
            entries,
            updated_at,
        }
    }
}

/// Whether older entries can be fetched.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum OlderState {
    /// Nothing fetched yet.
    Unknown,
    /// Older entries exist: `load_older` fetches the next page.
    Available,
    /// Everything back to the conversation's start is here.
    Complete,
    /// The public relay refuses older pages: fetched by itself once the path carries them. `message` is the words.
    NeedsBetterPath { message: String },
}

impl Conv<OlderState> for ferb_mobile::OlderState {
    fn conv(self) -> OlderState {
        match self {
            ferb_mobile::OlderState::Unknown => OlderState::Unknown,
            ferb_mobile::OlderState::Available => OlderState::Available,
            ferb_mobile::OlderState::Complete => OlderState::Complete,
            ferb_mobile::OlderState::NeedsBetterPath { message } => {
                OlderState::NeedsBetterPath { message }
            }
        }
    }
}

/// A conversation as the app draws it: the folded transcript, its questions, what is live, the outbox (on the
/// current conversation) and whether older entries can be fetched.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct TranscriptView {
    pub conversation: String,
    pub items: Vec<Item>,
    pub questions: Vec<QuestionItem>,
    pub live: Vec<LiveView>,
    pub outbox: Vec<OutboxItem>,
    pub older: OlderState,
}

impl Conv<TranscriptView> for ferb_mobile::TranscriptView {
    fn conv(self) -> TranscriptView {
        let ferb_mobile::TranscriptView {
            conversation,
            items,
            questions,
            live,
            outbox,
            older,
        } = self;
        TranscriptView {
            conversation,
            items: items.conv(),
            questions: questions.conv(),
            live: live.conv(),
            outbox: outbox.conv(),
            older: older.conv(),
        }
    }
}

/// The temporary chat (memory only, never the phone's store): whether one is on, which, since when, its change
/// number, and its turns folded as any conversation (`transcript.conversation` is "temporary:<id>"; empty while off).
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct TemporaryChat {
    pub on: bool,
    pub id: Option<String>,
    pub started_at: Option<String>,
    pub rev: i64,
    pub transcript: TranscriptView,
}

impl Conv<TemporaryChat> for ferb_mobile::TemporaryChat {
    fn conv(self) -> TemporaryChat {
        let ferb_mobile::TemporaryChat {
            on,
            id,
            started_at,
            rev,
            transcript,
        } = self;
        TemporaryChat {
            on,
            id,
            started_at,
            rev,
            transcript: transcript.conv(),
        }
    }
}

/// The path the link is on now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum LinkPath {
    Offline,
    Direct,
    OwnRelay,
    PublicRelay,
}

impl Conv<LinkPath> for ferb_mobile::LinkPath {
    fn conv(self) -> LinkPath {
        match self {
            ferb_mobile::LinkPath::Offline => LinkPath::Offline,
            ferb_mobile::LinkPath::Direct => LinkPath::Direct,
            ferb_mobile::LinkPath::OwnRelay => LinkPath::OwnRelay,
            ferb_mobile::LinkPath::PublicRelay => LinkPath::PublicRelay,
        }
    }
}

/// The chat's view of the link: its path, whether pictures, files and older pages cross it (false on the public
/// relay), the owner's relay's fallback words, and whether the watch runs.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct LinkState {
    pub path: LinkPath,
    pub bulk_allowed: bool,
    pub relay_note: Option<String>,
    pub watching: bool,
    /// The app locked (`Mobile.lock`): the conversation is not read until the next `unlock`.
    pub locked: bool,
}

impl Conv<LinkState> for ferb_mobile::LinkState {
    fn conv(self) -> LinkState {
        let ferb_mobile::LinkState {
            path,
            bulk_allowed,
            relay_note,
            watching,
            locked,
        } = self;
        LinkState {
            path: path.conv(),
            bulk_allowed,
            relay_note,
            watching,
            locked,
        }
    }
}

/// What changed, so the app reads it again.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum Change {
    /// A conversation's stored entries changed: read its transcript again.
    Transcript {
        conversation: String,
    },
    /// Only what is live changed (typing, a streamed line, a step's state).
    Live {
        conversation: String,
    },
    /// The list of conversations or which one is current.
    Conversations,
    Outbox,
    Link,
    /// Whether older entries can be fetched changed.
    Older {
        conversation: String,
    },
    /// What FERB has waiting changed: read `last_status` again.
    Status,
    /// What the Mac told about itself changed: read `cached_mac_info` again.
    Mac,
    /// The Mac asks for one precise fix of this phone's location, after the owner said Yes to it for a task: read one
    /// (asking Android's permission then if needed) and answer with `answer_location`.
    LocationWanted {
        request_id: String,
    },
    /// The temporary chat moved: on or off (from any client), or its turns changed: read `temporary_chat` again; off,
    /// show the normal chat.
    Temporary,
    /// "Forget this Mac" ran or the core closed: every call now answers `closed`.
    Closed,
}

impl Conv<Change> for ferb_mobile::Change {
    fn conv(self) -> Change {
        match self {
            ferb_mobile::Change::Transcript { conversation } => Change::Transcript { conversation },
            ferb_mobile::Change::Live { conversation } => Change::Live { conversation },
            ferb_mobile::Change::Conversations => Change::Conversations,
            ferb_mobile::Change::Outbox => Change::Outbox,
            ferb_mobile::Change::Link => Change::Link,
            ferb_mobile::Change::Older { conversation } => Change::Older { conversation },
            ferb_mobile::Change::Status => Change::Status,
            ferb_mobile::Change::Mac => Change::Mac,
            ferb_mobile::Change::LocationWanted { request_id } => {
                Change::LocationWanted { request_id }
            }
            ferb_mobile::Change::Temporary => Change::Temporary,
            ferb_mobile::Change::Closed => Change::Closed,
        }
    }
}

// ---- the phone's own record and finding the Mac's files (T-1214, T-1211) ------------------------------------------

/// A coarse place as the phone's geocoder named it: city, region, country, ISO code. Never coordinates.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, uniffi::Record)]
pub struct DevicePlace {
    pub city: String,
    pub region: String,
    pub country: String,
    pub country_code: String,
}

/// What the phone tells FERB about itself (`Mobile.report_device`): what the app read from Android's own callbacks.
/// A field it could not read is empty or none. `location`: "on", "off", "denied" or "unavailable".
#[derive(Debug, Clone, PartialEq, Serialize, uniffi::Record)]
pub struct DeviceReport {
    pub name: String,
    pub model: String,
    pub os: String,
    pub time_zone: String,
    pub utc_offset_minutes: i32,
    pub location: String,
    pub place: Option<DevicePlace>,
    pub battery_percent: Option<u8>,
    pub charging: Option<bool>,
    pub power_saver: Option<bool>,
    pub network: String,
    pub locale: String,
    pub languages: Vec<String>,
    pub screen_on: Option<bool>,
    pub screen_locked: Option<bool>,
}

impl From<DeviceReport> for ferb_mobile::DeviceReport {
    fn from(r: DeviceReport) -> Self {
        let DeviceReport {
            name,
            model,
            os,
            time_zone,
            utc_offset_minutes,
            location,
            place,
            battery_percent,
            charging,
            power_saver,
            network,
            locale,
            languages,
            screen_on,
            screen_locked,
        } = r;
        ferb_mobile::DeviceReport {
            name,
            model,
            os,
            time_zone,
            utc_offset_minutes,
            location,
            place: place.map(|p| {
                let DevicePlace {
                    city,
                    region,
                    country,
                    country_code,
                } = p;
                ferb_mobile::Place {
                    city,
                    region,
                    country,
                    country_code,
                }
            }),
            battery_percent,
            charging,
            power_saver,
            network,
            locale,
            languages,
            screen_on,
            screen_locked,
        }
    }
}

/// One precise fix the app read for the Mac's request (`Mobile.answer_location`).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, uniffi::Record)]
pub struct LocationFix {
    pub latitude: f64,
    pub longitude: f64,
    pub accuracy_m: f64,
}

mirror! {
    /// One file the Mac found: `source` "made" (a task's result), "fetched" or "sent" (by this phone) for recent
    /// files, "" for a search; `at` when; the task that made it.
    FoundFile from ferb_mobile::FoundFile {
        name: String,
        path: String,
        dir: bool,
        size: u64,
        modified: i64,
        source: String,
        at: Option<String>,
        task_id: Option<String>,
        task_name: Option<String>,
    }
}

mirror! {
    /// The Mac's recent files for this phone; `skipped`: key places and files gone, left out.
    RecentFiles from ferb_mobile::RecentFiles {
        files: Vec<FoundFile>,
        skipped: u64,
    }
}

mirror! {
    /// One page of a search of names under `path`: `looked` names looked at, `skipped` key places not looked into,
    /// `incomplete` the search stopped before it looked at every name; this page starts at match `from`, and `cut`:
    /// more remain, `next` the marker `Mobile.search_files` takes as `after` ("load more", PHN-589).
    FoundFiles from ferb_mobile::FoundFiles {
        path: String,
        query: String,
        files: Vec<FoundFile>,
        looked: u64,
        skipped: u64,
        cut: bool,
        incomplete: bool,
        from: u64,
        next: Option<String>,
    }
}

// ---- the owner's coding-agent sessions (kb HAND-651..666) ----------------------------------------------------------------

mirror! {
    /// One of the owner's coding sessions on the Mac (Claude Code or Codex, whatever app shows it).
    AgentSession from ferb_mobile::sessions::AgentSession {
        id: String,
        agent: String,
        agent_name: String,
        title: String,
        folder: String,
        folder_name: String,
        status: String,
        waiting: String,
        host: String,
        can_send: bool,
        no_send: String,
        foreground: bool,
        last_at: String,
        last_text: String,
        controlled_here: bool,
        control: String,
        with_ferb: bool,
        goal: String,
        terminals: Vec<SessionChoice>,
        program: String,
        window: String,
        no_text: String,
        ferb_task: String,
        ferb_note: String,
        /// HAND-710: FERB reads none of its words: its screen is its window, live, and "Type here" types into it.
        #[uniffi(default = false)]
        watch: bool,
    }
}

mirror! {
    /// The sessions, and whether the owner turned them on.
    SessionsList from ferb_mobile::sessions::SessionsList {
        enabled: bool,
        sessions: Vec<AgentSession>,
        no_chats: Vec<String>,
    }
}

mirror! {
    /// One item of a session's conversation (user, assistant, tool, interrupted, error).
    SessionItem from ferb_mobile::sessions::SessionItem {
        seq: u64,
        at: String,
        kind: String,
        text: String,
        tool: String,
        parts: Vec<SessionPart>,
        /// HAND-709: a message FERB sent while it kept the session going.
        #[uniffi(default = false)]
        ferb: bool,
    }
}

mirror! {
    /// One part of a reply as the app shows it (HAND-703): text (Markdown), thinking, tool or meta.
    SessionPart from ferb_mobile::sessions::SessionPart {
        kind: String,
        text: String,
        label: String,
        detail: String,
    }
}

mirror! {
    /// A new terminal the Mac opened: its session (to open; "" when it hadn't shown yet), its app, the command typed.
    SessionStarted from ferb_mobile::sessions::SessionStarted {
        session: String,
        host: String,
        command: String,
    }
}

mirror! {
    /// One answer a session's card offers.
    SessionChoice from ferb_mobile::sessions::SessionChoice {
        id: String,
        label: String,
    }
}

mirror! {
    /// The agent waits for the owner: a permission prompt or a question (no choices: answered on the Mac); or FERB's
    /// own question about its drive of the session (kind "ferb", HAND-709).
    SessionCard from ferb_mobile::sessions::SessionCard {
        id: String,
        kind: String,
        tool: String,
        detail: String,
        question: String,
        choices: Vec<SessionChoice>,
    }
}

mirror! {
    /// One page of a session's conversation, with its open cards.
    SessionTimeline from ferb_mobile::sessions::SessionTimeline {
        session: Option<AgentSession>,
        items: Vec<SessionItem>,
        more: bool,
        cards: Vec<SessionCard>,
    }
}

mirror! {
    /// What the owner is asked before FERB types for him: the exact text and what the Yes covers.
    SessionYes from ferb_mobile::sessions::SessionYes {
        question: String,
        detail: String,
        covers: String,
    }
}

mirror! {
    /// What a send did (`needs_yes`: nothing was sent; ask, then `session_control`).
    SessionSent from ferb_mobile::sessions::SessionSent {
        needs_yes: Option<SessionYes>,
        sent: bool,
        delivered: bool,
        path: String,
    }
}

mirror! {
    /// The folders a new session may start in; with one chosen, the exact command and its question.
    SessionStartPlan from ferb_mobile::sessions::SessionStartPlan {
        folders: Vec<String>,
        needs_yes: Option<SessionYes>,
    }
}

// ---- the owner's review of what an import brought in (kb MEM-460..457) --------------------------------------------------

mirror! {
    /// One answer a button of the review gives: what is sent and its words.
    ReviewButton from ferb_mobile::review::ReviewButton {
        answer: String,
        label: String,
    }
}

mirror! {
    /// One item of the review: the memory's id, the line in plain words, a small note under it, and its own buttons
    /// when they differ from its part's (already always on: Keep / Remove); empty: the part's `answers`.
    ReviewItem from ferb_mobile::review::ReviewItem {
        id: String,
        text: String,
        note: String,
        answers: Vec<ReviewButton>,
    }
}

mirror! {
    /// A group of the review's items under its title ("" for none), and its own buttons for all of them at once
    /// (Keep all / Remove all when all are already always on and the part has others); usually empty.
    ReviewGroup from ferb_mobile::review::ReviewGroup {
        title: String,
        items: Vec<ReviewItem>,
        all: Vec<ReviewButton>,
    }
}

mirror! {
    /// One part of the review (always_on, still_true, offers): its question, the line under it, its groups, the
    /// answers each item takes and those for all at once.
    ReviewPart from ferb_mobile::review::ReviewPart {
        part: String,
        title: String,
        hint: String,
        groups: Vec<ReviewGroup>,
        answers: Vec<ReviewButton>,
        all: Vec<ReviewButton>,
    }
}

mirror! {
    /// The review as it stands: its parts and how many items are left.
    MemoryReview from ferb_mobile::review::MemoryReview {
        parts: Vec<ReviewPart>,
        count: u64,
    }
}
