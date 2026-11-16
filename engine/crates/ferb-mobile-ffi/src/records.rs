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
same!(String, bool, i64, i32, u32, u64);

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
    }
}

mirror! {
    /// An answer drawn below, quoting the turn it answers.
    ReplyItem from ff::ReplyItem {
        entry: String,
        legacy: bool,
        answers: String,
        after: String,
        part: Option<String>,
        quotes_request: bool,
        said: Option<AnswerView>,
    }
}

mirror! {
    /// FERB's own line in the chat (a notice or a reminder); `reference` is the item it belongs to.
    NoticeItem from ff::NoticeItem {
        entry: String,
        legacy: bool,
        #[serde(rename = "ref")]
        reference: String,
        after: Option<String>,
        text: String,
        quotes: Vec<Quote>,
        cards: Option<Vec<ResultCard>>,
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
    /// One folder's listing (`total` entries, of which the first are here when `cut`).
    Folder from ferb_mobile::remote::Folder {
        path: String,
        parent: Option<String>,
        locked: bool,
        entries: Vec<FileEntry>,
        total: u64,
        cut: bool,
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
    /// A file sent to the Mac: `upload_id` rides with a message (`send_with`).
    Uploaded from ferb_mobile::remote::Uploaded {
        upload_id: String,
        saved: String,
        size: u64,
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
            ferb_mobile::Change::Closed => Change::Closed,
        }
    }
}
