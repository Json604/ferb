//! The phone's fold: a conversation's stored entries → the transcript FERB.app shows for them (ENG-9, ENG-22).
//!
//! A port of FERB.app's own fold (`apps/mac/FERBNotch/Sources/NotchCore/EntryFold.swift` `EntryFold.archive`, with the
//! archive's `transcript()`, `followsItsTurn`, `rowPlacements` and the turn's derived lines from
//! ConversationArchive.swift), and of the platform-neutral shape `EntryFoldTranscriptTests.swift` writes to
//! `engine/fixtures/chat-entries/transcripts.json`. The Mac's behaviour is the golden: `tests/golden.rs` folds every
//! scenario of the three fixture files and compares with that file.
//!
//! The fold is a pure function of the conversation's newest copy of each entry (deleted ones left out), so the
//! phone folds the union of whatever it has fetched, in whatever order it arrived (`EntrySet`).

pub mod model;
pub mod text;

use std::collections::{BTreeMap, HashMap, HashSet};

use serde::Serialize;
use serde_json::{Map, Value};

use crate::entry::ChatEntry;
use crate::time;
use model::*;
use text::*;

// ---- the archive as FERB.app holds it -------------------------------------------------------------------------------

/// A turn (`ConversationTurn`). `id` is the entry id that made it, or for a carried row the app's own UUID.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Turn {
    pub id: String,
    pub created_at: f64,
    pub request: String,
    pub sources: Vec<String>,
    pub backend_task_id: Option<String>,
    pub answer: String,
    pub display_text: Option<String>,
    pub status: String,
    pub artifacts: Vec<String>,
    pub response_notice: Option<String>,
    pub request_id: Option<String>,
    pub plan: Option<String>,
    pub parts: Option<Vec<String>>,
    pub verified: Option<bool>,
    pub question: Option<String>,
    pub status_inferred: Option<bool>,
    pub task_goal: Option<String>,
    pub task_name: Option<String>,
    pub plan_line: Option<String>,
    pub plan_lines: Option<Vec<String>>,
    pub direct: Option<Vec<String>>,
    pub receipts: Option<Vec<String>>,
    pub card: Option<Card>,
    pub checklist: Option<Checklist>,
    pub note: Option<String>,
    pub code_lines: Option<Vec<String>>,
    pub runtime_switch: Option<RuntimeSwitch>,
    pub cards: Option<Vec<ResultCard>>,
    pub receipt_cards: Option<Vec<ResultCard>>,
    pub merged_into: Option<String>,
    pub hidden_kinds: Option<Vec<String>>,
    pub sealed: Option<Vec<Sealed>>,
}

impl Turn {
    fn new(id: &str, created_at: f64, request: String, sources: Vec<String>) -> Turn {
        Turn {
            id: id.to_string(),
            created_at,
            request,
            sources,
            status: "working".into(),
            ..Turn::default()
        }
    }

    /// FERB's own turn for a task no message here started (APP-585).
    pub fn is_recovered(&self) -> bool {
        self.task_goal.is_some()
    }

    fn shown_direct(&self) -> Vec<String> {
        if self.card.is_none() {
            self.direct.clone().unwrap_or_default()
        } else {
            Vec::new()
        }
    }

    fn without_direct(&self, text: &str) -> String {
        let Some(card) = &self.card else {
            return text.to_string();
        };
        let mut answers: Vec<String> = self.direct.clone().unwrap_or_default();
        answers.extend(
            card.rows
                .iter()
                .filter(|r| r.direct)
                .filter_map(|r| r.text.clone()),
        );
        let mut seen = HashSet::new();
        let mut shown = text.to_string();
        for a in answers {
            if seen.insert(a.clone()) {
                shown = without_whole(&shown, &a);
            }
        }
        shown
    }

    fn shown_plan_line(&self) -> Option<String> {
        self.plan_line.as_deref().map(|l| self.without_direct(l))
    }

    fn shown_plan(&self) -> Option<String> {
        self.plan.as_deref().map(|p| self.without_direct(p))
    }

    /// `firstReplySays`: whether the first reply's bubble has words once direct answers and carded receipts are out.
    fn first_reply_says(&self) -> bool {
        if let Some(line) = self.shown_plan_line() {
            let cards = self.receipt_cards.clone().unwrap_or_default();
            let drop = ResultCard::lines(&cards);
            let receipts = self
                .receipts
                .clone()
                .unwrap_or_default()
                .into_iter()
                .filter(|r| !drop.contains(&r.as_str()));
            return self
                .shown_direct()
                .into_iter()
                .chain(receipts)
                .chain(std::iter::once(line))
                .chain(self.plan_lines.clone().unwrap_or_default())
                .any(|t| says(&t));
        }
        if let Some(plan) = self.shown_plan() {
            return says(&plan);
        }
        false
    }

    fn hidden_notice(&self) -> Option<String> {
        let kept: Vec<(String, String)> = self
            .sealed
            .iter()
            .flatten()
            .map(|s| (s.kind.clone(), s.name.clone()))
            .collect();
        hidden_notice(self.hidden_kinds.as_deref().unwrap_or(&[]), &kept)
    }

    /// `ConversationTurn.saying`: the turn as one of its later answers shows it (unused by the transcript's fields,
    /// kept for the quote a later answer draws).
    pub fn quote(&self) -> String {
        squash(self.task_goal.as_deref().unwrap_or(&self.request))
    }
}

/// `ReplyPart`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplyPart {
    Question,
    Answer,
}

impl ReplyPart {
    fn named(s: &str) -> Option<ReplyPart> {
        match s {
            "question" => Some(ReplyPart::Question),
            "answer" => Some(ReplyPart::Answer),
            _ => None,
        }
    }
    fn as_str(self) -> &'static str {
        match self {
            ReplyPart::Question => "question",
            ReplyPart::Answer => "answer",
        }
    }
}

/// A late answer or a later answer (`ConversationReply`).
#[derive(Debug, Clone, PartialEq)]
pub struct Reply {
    pub id: String,
    pub turn_id: String,
    pub created_at: f64,
    pub after_turn_id: String,
    pub part: Option<ReplyPart>,
    pub said: Option<LaterAnswer>,
}

/// FERB's own line in the chat (`ConversationNotice`).
#[derive(Debug, Clone, PartialEq)]
pub struct Notice {
    pub id: String,
    pub entry_id: String,
    pub created_at: f64,
    pub text: String,
    pub quotes: Vec<Quote>,
    pub after_turn_id: Option<String>,
    pub cards: Option<Vec<ResultCard>>,
}

#[derive(Debug, Clone, Default)]
pub struct Archive {
    pub turns: Vec<Turn>,
    pub replies: Vec<Reply>,
    pub notices: Vec<Notice>,
}

/// One entry of the chat as shown (`TranscriptEntry`).
enum Shown<'a> {
    Turn(&'a Turn, bool),
    Reply(&'a Reply),
    Notice(&'a Notice),
}

impl Archive {
    fn turn(&self, id: &str) -> Option<&Turn> {
        self.turns.iter().find(|t| t.id == id)
    }

    /// `ConversationArchive.transcript()`: each turn, followed by the replies and notices that arrived while it was
    /// the newest, in arrival order (replies first at the same instant).
    fn transcript(&self) -> Vec<Shown<'_>> {
        let mut items: Vec<(f64, usize, Option<&str>, Shown<'_>)> = Vec::new();
        for r in &self.replies {
            if self.turn(&r.turn_id).is_none() {
                continue;
            }
            let n = items.len();
            items.push((
                r.created_at,
                n,
                Some(r.after_turn_id.as_str()),
                Shown::Reply(r),
            ));
        }
        for n in &self.notices {
            let i = items.len();
            items.push((
                n.created_at,
                i,
                n.after_turn_id.as_deref(),
                Shown::Notice(n),
            ));
        }
        items.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
        let mut following: HashMap<Option<&str>, Vec<Shown<'_>>> = HashMap::new();
        for (_, _, anchor, shown) in items {
            following.entry(anchor).or_default().push(shown);
        }
        let answered_below: HashSet<&str> = self
            .replies
            .iter()
            .filter(|r| r.said.is_none())
            .map(|r| r.turn_id.as_str())
            .collect();
        let mut out = following.remove(&None).unwrap_or_default();
        for t in &self.turns {
            out.push(Shown::Turn(t, answered_below.contains(t.id.as_str())));
            if let Some(after) = following.remove(&Some(t.id.as_str())) {
                out.extend(after);
            }
        }
        out
    }

    /// `followsItsTurn`: a later answer said while its turn was the newest, on its day, with no notice between.
    fn follows_its_turn(&self, r: &Reply, utc_offset_s: i32) -> bool {
        if r.said.is_none() || r.after_turn_id != r.turn_id {
            return false;
        }
        let Some(t) = self.turn(&r.turn_id) else {
            return false;
        };
        if time::day(t.created_at, utc_offset_s) != time::day(r.created_at, utc_offset_s) {
            return false;
        }
        !self.notices.iter().any(|n| {
            n.after_turn_id.as_deref() == Some(r.turn_id.as_str()) && n.created_at < r.created_at
        })
    }

    /// `rowPlacements`: the newest turn whose card has a row the question belongs to.
    fn row_place(&self, q: &Approval) -> Option<(&str, &str)> {
        self.turns
            .iter()
            .rev()
            .filter_map(|t| t.card.as_ref().map(|c| (t, c)))
            .find_map(|(t, c)| {
                c.rows
                    .iter()
                    .find(|r| r.asks(q))
                    .map(|r| (t.id.as_str(), r.part.as_str()))
            })
    }
}

// ---- one entry at a time (EntryFold's per-entry mapping) ------------------------------------------------------------

fn message_turn(e: &ChatEntry, created_at: f64) -> Turn {
    let body = &e.body;
    let mut t = Turn::new(
        &e.id,
        created_at,
        without_paste_markers(&s(body, "text").unwrap_or_default()),
        strs(body, "attachments")
            .unwrap_or_default()
            .iter()
            .map(|p| file_path(p))
            .collect(),
    );
    t.request_id = e.request_id.clone();
    t.backend_task_id = e.task_id.clone();
    if let Some(h) = strs(body, "hidden").filter(|h| !h.is_empty()) {
        t.hidden_kinds = Some(h);
    }
    t.sealed = Sealed::list(body);
    match e.state.as_str() {
        "merged" => {
            t.status = "merged".into();
            t.merged_into = s(body, "merged_into");
        }
        "interrupted" => {
            t.status = "interrupted".into();
            t.response_notice = s(body, "notice");
            t.status_inferred = Some(true);
        }
        _ => {}
    }
    t
}

fn recovered_turn(e: &ChatEntry, created_at: f64) -> Turn {
    let mut t = Turn::new(&e.id, created_at, String::new(), Vec::new());
    t.backend_task_id = e.task_id.clone();
    t.task_goal = Some(short_name(
        s(&e.body, "name").as_deref(),
        s(&e.body, "goal").as_deref(),
    ));
    t
}

fn show_plan(body: &Map<String, Value>, t: &mut Turn) {
    if let Some(text) = s(body, "text") {
        t.plan = Some(text);
    }
    if body.get("name").is_some_and(Value::is_string)
        || body.get("goal").is_some_and(Value::is_string)
    {
        let name = short_name(s(body, "name").as_deref(), s(body, "goal").as_deref());
        if !name.is_empty() {
            t.task_name = Some(name);
        }
    }
    if let Some(line) = s(body, "plan_line") {
        t.plan_line = Some(line);
        t.plan_lines = Some(strs(body, "lines").unwrap_or_default());
    }
    if let Some(d) = strs(body, "direct") {
        t.direct = Some(d);
    }
    if let Some(r) = strs(body, "receipts") {
        t.receipts = Some(r);
    }
    if let Some(c) = ResultCard::list(body.get("receipt_cards")) {
        t.receipt_cards = Some(c);
    }
    if let Some(card) = body.get("card").and_then(Value::as_object) {
        if let Some(task) = s(card, "task_id") {
            t.card = Card::read(body.get("card"), &task);
        }
    }
    if let Some(c) = Checklist::read(body.get("checklist")) {
        t.checklist = Some(c);
    }
}

fn show_answer(body: &Map<String, Value>, t: &mut Turn) {
    t.answer = s(body, "text").unwrap_or_default();
    t.display_text = s(body, "display_text");
    t.status = s(body, "status").unwrap_or_else(|| "done".into());
    t.artifacts = strs(body, "artifacts")
        .unwrap_or_default()
        .iter()
        .map(|p| file_path(p))
        .collect();
    t.verified = Some(b(body, "verified").unwrap_or(false));
    t.response_notice = None;
    t.status_inferred = None;
    t.note = s(body, "note");
    t.code_lines = strs(body, "lines");
    t.runtime_switch = RuntimeSwitch::read(body.get("runtime_switch"));
    t.cards = ResultCard::list(body.get("cards"));
    t.question = s(body, "question");
}

fn follows(e: &ChatEntry) -> bool {
    e.body.get("follows").is_some_and(Value::is_string)
}

fn notice(e: &ChatEntry, created_at: f64, after: Option<String>) -> Notice {
    let body = &e.body;
    let quotes = objs(body, "quotes")
        .unwrap_or_default()
        .into_iter()
        .filter_map(|q| {
            let text = s(q, "text").filter(|t| !t.is_empty())?;
            Some(Quote {
                text,
                origin: s(q, "origin").unwrap_or_default(),
            })
        })
        .collect();
    Notice {
        id: e.r#ref.clone().unwrap_or_else(|| e.id.clone()),
        entry_id: e.id.clone(),
        created_at,
        text: s(body, "text").unwrap_or_default(),
        quotes,
        after_turn_id: after,
        cards: ResultCard::list(body.get("cards")),
    }
}

/// A reminder as FERB.app's `receiveReminder` shows it: its card from the banner's.
fn post(e: &ChatEntry, created_at: f64, after: Option<String>) -> Notice {
    let body = &e.body;
    let text_ = s(body, "text").unwrap_or_default();
    let id = s(body, "timer_id").unwrap_or_default();
    let card = body.get("card").and_then(ResultCard::read);
    let mut shown = Map::new();
    shown.insert("type".into(), "reminder".into());
    shown.insert(
        "title".into(),
        card.as_ref()
            .and_then(|c| c.title.clone())
            .unwrap_or_else(|| clause(&text_, SHORT_NAME_CUT))
            .into(),
    );
    shown.insert(
        "kind".into(),
        card.as_ref()
            .and_then(|c| c.kind.clone())
            .unwrap_or_else(|| "reminder".into())
            .into(),
    );
    if let Some(due) = card.as_ref().and_then(|c| c.due.clone()) {
        shown.insert("due".into(), due.into());
    }
    if let Some(started) = card.as_ref().and_then(|c| c.started.clone()) {
        shown.insert("started".into(), started.into());
    }
    if let Some(first) = card.as_ref().and_then(|c| c.repeat_of.clone()) {
        shown.insert("repeat_of".into(), first.into());
    }
    shown.insert(
        "timer_id".into(),
        card.as_ref()
            .and_then(|c| c.timer_id.clone())
            .unwrap_or_else(|| id.clone())
            .into(),
    );
    Notice {
        id: e.r#ref.clone().unwrap_or_else(|| format!("reminder:{id}")),
        entry_id: e.id.clone(),
        created_at,
        text: text_,
        quotes: Vec::new(),
        after_turn_id: after,
        cards: ResultCard::list(Some(&Value::Array(vec![Value::Object(shown)]))),
    }
}

// ---- carried rows (`legacy`: FERB.app's own saved form, `HistoryEntry.read`) ----------------------------------------

#[allow(clippy::large_enum_variant)] // one value at a time, read and moved into the archive at once
enum Stored {
    Turn(Turn),
    Reply(Reply),
    Notice(Notice),
}

fn saved_turn(m: &Map<String, Value>) -> Got<Turn> {
    let urls = |v: &Value| list_of(v, url_path);
    let strings = str_list;
    Ok(Turn {
        id: req(m, "id", uuid)?,
        created_at: req(m, "createdAt", date)?,
        request: without_paste_markers(&opt(m, "request", text)?.unwrap_or_default()),
        sources: opt(m, "sources", urls)?.unwrap_or_default(),
        backend_task_id: opt(m, "backendTaskID", text)?,
        answer: opt(m, "answer", text)?.unwrap_or_default(),
        display_text: opt(m, "displayText", text)?,
        status: opt(m, "status", text)?.unwrap_or_else(|| "working".into()),
        artifacts: opt(m, "artifacts", urls)?.unwrap_or_default(),
        response_notice: opt(m, "responseNotice", text)?,
        request_id: {
            opt(m, "ack", text)?;
            opt(m, "requestID", text)?
        },
        plan: opt(m, "plan", text)?,
        parts: opt(m, "parts", strings)?,
        verified: opt(m, "verified", boolean)?,
        question: opt(m, "question", text)?,
        status_inferred: opt(m, "statusInferred", boolean)?,
        task_goal: opt(m, "taskGoal", text)?,
        task_name: opt(m, "taskName", text)?,
        plan_line: opt(m, "planLine", text)?,
        plan_lines: opt(m, "planLines", strings)?,
        direct: opt(m, "direct", strings)?,
        receipts: opt(m, "receipts", strings)?,
        card: lossy(m, "card", Card::saved),
        checklist: lossy(m, "checklist", Checklist::saved),
        note: opt(m, "note", text)?,
        code_lines: opt(m, "codeLines", strings)?,
        runtime_switch: lossy(m, "runtimeSwitch", RuntimeSwitch::saved),
        cards: lossy_list(m, "cards", ResultCard::saved)?,
        receipt_cards: lossy_list(m, "receiptCards", ResultCard::saved)?,
        merged_into: opt(m, "mergedInto", text)?,
        hidden_kinds: {
            opt(m, "sentElsewhere", boolean)?;
            opt(m, "hiddenKinds", strings)?
        },
        sealed: lossy_list(m, "sealed", Sealed::saved)?,
    })
}

fn saved_reply(m: &Map<String, Value>) -> Got<Reply> {
    Ok(Reply {
        id: req(m, "id", uuid)?,
        turn_id: req(m, "turnID", uuid)?,
        created_at: req(m, "createdAt", date)?,
        after_turn_id: req(m, "afterTurnID", uuid)?,
        part: lossy(m, "part", |v| {
            v.as_str().and_then(ReplyPart::named).ok_or(Bad)
        }),
        said: lossy(m, "said", LaterAnswer::saved),
    })
}

fn saved_notice(m: &Map<String, Value>) -> Got<Notice> {
    let n = Notice {
        id: req(m, "id", text)?,
        entry_id: req(m, "entryID", uuid)?,
        created_at: req(m, "createdAt", date)?,
        text: {
            opt(m, "noticedAt", model::date)?;
            req(m, "text", model::text)?
        },
        quotes: lossy_list(m, "quotes", Quote::saved)?.unwrap_or_default(),
        after_turn_id: {
            opt(m, "tag", model::text)?;
            opt(m, "afterTurnID", uuid)?
        },
        cards: lossy_list(m, "cards", ResultCard::saved)?,
    };
    // proposals: a list whose unreadable elements are left out; anything but a list fails the notice
    lossy_list(m, "proposals", |v| v.as_object().map(|_| ()).ok_or(Bad))?;
    Ok(n)
}

fn read_legacy(body: &Map<String, Value>) -> Option<Stored> {
    let kind = s(body, "kind")?;
    let saved = body.get("body")?.as_object()?;
    match kind.as_str() {
        "turn" => saved_turn(saved).ok().map(Stored::Turn),
        "reply" => saved_reply(saved).ok().map(Stored::Reply),
        "notice" => saved_notice(saved).ok().map(Stored::Notice),
        _ => None,
    }
}

// ---- the fold ---------------------------------------------------------------------------------------------------------

/// Each entry's newest copy, deleted ones left out, in `pos` order.
pub fn newest(entries: &[ChatEntry]) -> Vec<&ChatEntry> {
    let mut by_id: HashMap<&str, &ChatEntry> = HashMap::new();
    for e in entries {
        match by_id.get(e.id.as_str()) {
            Some(kept) if !e.newer_than(kept) => {}
            _ => {
                by_id.insert(&e.id, e);
            }
        }
    }
    let mut out: Vec<&ChatEntry> = by_id.into_values().collect();
    out.sort_by(|a, b| a.pos.cmp(&b.pos).then(a.id.cmp(&b.id)));
    out
}

/// `EntryFold.archive`.
pub fn archive(newest: &[&ChatEntry]) -> Archive {
    let ordered: Vec<&ChatEntry> = newest.iter().copied().filter(|e| !e.deleted).collect();
    let mut a = Archive::default();
    let mut turn_of: HashMap<String, usize> = HashMap::new();
    let mut by_request: HashMap<String, usize> = HashMap::new();
    let mut shown: BTreeMap<usize, &ChatEntry> = BTreeMap::new();
    // replies and notices are ordered by when they were made; two made in the same instant are a microsecond apart
    let mut last = time::DISTANT_PAST;
    let mut stamp = |text: &str| {
        let made = time::date(text).max(last + 0.000_001);
        last = made;
        made
    };
    for e in ordered {
        match e.kind.as_str() {
            "message" => {
                let t = message_turn(e, stamp(&e.created_at));
                turn_of.insert(e.id.clone(), a.turns.len());
                if let Some(r) = &e.request_id {
                    by_request.insert(r.clone(), a.turns.len());
                }
                a.turns.push(t);
            }
            "plan" => {
                if let Some(&i) = e.request_id.as_ref().and_then(|r| by_request.get(r)) {
                    show_plan(&e.body, &mut a.turns[i]);
                }
            }
            "answer" | "error" => {
                let index = if let Some(&i) = e.answers.as_ref().and_then(|m| turn_of.get(m)) {
                    i
                } else if let Some(&i) = turn_of.get(&e.id) {
                    i
                } else {
                    let i = a.turns.len();
                    turn_of.insert(e.id.clone(), i);
                    a.turns.push(recovered_turn(e, stamp(&e.created_at)));
                    i
                };
                let newest_id = a.turns.last().map(|t| t.id.clone());
                if follows(e) {
                    if let Some(after) = newest_id {
                        a.replies.push(Reply {
                            id: e.id.clone(),
                            turn_id: a.turns[index].id.clone(),
                            created_at: stamp(&e.created_at),
                            after_turn_id: after,
                            part: Some(ReplyPart::Answer),
                            said: Some(LaterAnswer::read(&e.body)),
                        });
                        continue;
                    }
                }
                if e.placement.as_deref() == Some("below") {
                    if let Some(after) = newest_id {
                        a.replies.push(Reply {
                            id: e.id.clone(),
                            turn_id: a.turns[index].id.clone(),
                            created_at: stamp(&e.created_at),
                            after_turn_id: after,
                            part: s(&e.body, "part").as_deref().and_then(ReplyPart::named),
                            said: None,
                        });
                    }
                }
                shown.insert(index, e);
            }
            "notice" => {
                let at = stamp(&e.created_at);
                let after = a.turns.last().map(|t| t.id.clone());
                a.notices.push(notice(e, at, after));
            }
            "post" => {
                let at = stamp(&e.created_at);
                let after = a.turns.last().map(|t| t.id.clone());
                a.notices.push(post(e, at, after));
            }
            "legacy" => match read_legacy(&e.body) {
                Some(Stored::Turn(t)) => {
                    turn_of.insert(e.id.clone(), a.turns.len());
                    if let Some(r) = &t.request_id {
                        by_request.insert(r.clone(), a.turns.len());
                    }
                    a.turns.push(t);
                }
                Some(Stored::Reply(r)) => a.replies.push(r),
                Some(Stored::Notice(n)) => a.notices.push(n),
                None => {}
            },
            // a question (drawn apart, `Transcript::questions`), or a kind this build doesn't know
            _ => {}
        }
    }
    for (i, e) in shown {
        show_answer(&e.body, &mut a.turns[i]);
    }
    a
}

// ---- the transcript a phone draws (engine/fixtures/chat-entries/README.md, "The phone's transcript") ----------------

/// The owner's words of a turn (absent for FERB's own turn).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct OwnerWords {
    pub text: String,
    pub files: Vec<String>,
    pub hidden: Vec<String>,
    pub sealed: Vec<Sealed>,
    pub hidden_notice: Option<String>,
}

/// FERB's first bubble.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FirstReply {
    pub says: bool,
    pub direct: Vec<String>,
    pub receipts: Vec<String>,
    pub receipt_cards: Option<Vec<ResultCard>>,
    pub plan_line: Option<String>,
    pub plan_lines: Vec<String>,
    pub plan: Option<String>,
}

/// A turn's answer, or a later answer's own words (`said`, with its `status`).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AnswerView {
    pub text: String,
    pub display_text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    pub verified: bool,
    pub artifacts: Vec<String>,
    pub note: Option<String>,
    pub lines: Option<Vec<String>>,
    pub cards: Option<Vec<ResultCard>>,
    pub runtime_switch: Option<RuntimeSwitch>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RowView {
    #[serde(flatten)]
    pub row: CardRow,
    pub shows: Shows,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CardView {
    pub task_id: String,
    pub request_id: String,
    pub rows: Vec<RowView>,
    pub done: i64,
    pub total: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<String>,
    pub lines: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub headline: Option<String>,
    pub plan: Vec<PlanStep>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ChecklistView {
    pub task_id: String,
    pub steps: Vec<PlanStep>,
    pub ended: BTreeMap<String, String>,
    pub finished: bool,
    pub lines: Vec<ChecklistLine>,
    pub collapsed: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TurnItem {
    pub entry: String,
    pub legacy: bool,
    pub client_id: Option<String>,
    pub request_id: Option<String>,
    pub task_id: Option<String>,
    pub from: String,
    pub task_name: Option<String>,
    pub status: String,
    pub status_inferred: bool,
    pub notice: Option<String>,
    pub merged_into: Option<String>,
    pub question: Option<String>,
    pub answered_below: bool,
    pub answer: Option<AnswerView>,
    pub card: Option<CardView>,
    pub checklist: Option<ChecklistView>,
    pub first_reply: Option<FirstReply>,
    pub parts: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub goal: Option<String>,
    #[serde(flatten)]
    pub words: Option<OwnerWords>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ReplyItem {
    pub entry: String,
    pub legacy: bool,
    pub answers: String,
    pub after: String,
    pub part: Option<String>,
    pub quotes_request: bool,
    pub said: Option<AnswerView>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct NoticeItem {
    pub entry: String,
    pub legacy: bool,
    #[serde(rename = "ref")]
    pub reference: String,
    pub after: Option<String>,
    pub text: String,
    pub quotes: Vec<Quote>,
    pub cards: Option<Vec<ResultCard>>,
}

/// One item of the chat, top to bottom. (Owned values handed to the app once per read: a turn is larger than the
/// others, and boxing it would buy nothing.)
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "item", rename_all = "snake_case")]
pub enum Item {
    Turn(TurnItem),
    Reply(ReplyItem),
    Notice(NoticeItem),
    Reminder(NoticeItem),
}

impl Item {
    /// The entry that made it.
    pub fn entry(&self) -> &str {
        match self {
            Item::Turn(t) => &t.entry,
            Item::Reply(r) => &r.entry,
            Item::Notice(n) | Item::Reminder(n) => &n.entry,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RowRef {
    pub turn: String,
    pub part: String,
}

/// A question entry and where the Mac draws it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct QuestionItem {
    pub entry: String,
    pub approval_id: Option<String>,
    pub request_id: Option<String>,
    pub task_id: Option<String>,
    pub state: String,
    pub open: bool,
    pub body: Value,
    /// `card_row`, `end_of_chat`, or None (no longer open).
    pub shown: Option<String>,
    pub row: Option<RowRef>,
}

/// A conversation as a phone draws it.
#[derive(Debug, Clone, PartialEq, Serialize, Default)]
pub struct Transcript {
    pub items: Vec<Item>,
    pub questions: Vec<QuestionItem>,
}

fn answer_view(t: &Turn) -> AnswerView {
    AnswerView {
        text: t.answer.clone(),
        display_text: t.display_text.clone(),
        status: None,
        verified: t.verified == Some(true),
        artifacts: t.artifacts.clone(),
        note: t.note.clone(),
        lines: t.code_lines.clone(),
        cards: t.cards.clone(),
        runtime_switch: t.runtime_switch.clone(),
    }
}

fn said_view(l: &LaterAnswer) -> AnswerView {
    AnswerView {
        text: l.answer.clone(),
        display_text: l.display_text.clone(),
        status: Some(l.status.clone()),
        verified: l.verified,
        artifacts: l.artifacts.clone(),
        note: l.note.clone(),
        lines: l.code_lines.clone(),
        cards: l.cards.clone(),
        runtime_switch: l.runtime_switch.clone(),
    }
}

fn card_view(c: &Card, live: bool) -> CardView {
    CardView {
        task_id: c.task_id.clone(),
        request_id: c.request_id.clone(),
        rows: c
            .rows
            .iter()
            .map(|r| RowView {
                row: r.clone(),
                shows: Shows::of(r, live),
            })
            .collect(),
        done: c.done,
        total: c.total,
        line: c.line.clone(),
        lines: c.lines.clone(),
        headline: c.headline.clone(),
        plan: c.plan.clone(),
    }
}

fn checklist_view(c: &Checklist, live: bool) -> ChecklistView {
    ChecklistView {
        task_id: c.task_id.clone(),
        steps: c.steps.clone(),
        ended: c.ended.clone(),
        finished: c.finished,
        lines: c
            .steps
            .iter()
            .map(|s| {
                ChecklistLine::of(
                    s,
                    c.ended.get(&s.id).map(String::as_str),
                    live && !c.finished,
                )
            })
            .collect(),
        collapsed: c.finished.then(|| collapsed(c.done_count(), c.steps.len())),
    }
}

fn first_reply(t: &Turn) -> Option<FirstReply> {
    if t.plan.is_none() && t.plan_line.is_none() && t.direct.is_none() && t.receipts.is_none() {
        return None;
    }
    Some(FirstReply {
        says: t.first_reply_says(),
        direct: t.shown_direct(),
        receipts: t.receipts.clone().unwrap_or_default(),
        receipt_cards: t.receipt_cards.clone(),
        plan_line: t.shown_plan_line(),
        plan_lines: t.plan_lines.clone().unwrap_or_default(),
        plan: if t.plan_line.is_none() {
            t.shown_plan()
        } else {
            None
        },
    })
}

/// The conversation's transcript from its entries (any revisions, any order): `utc_offset_s` is the phone's zone,
/// which decides whether a later answer was said on its turn's day.
pub fn transcript(entries: &[ChatEntry], utc_offset_s: i32) -> Transcript {
    let newest = newest(entries);
    transcript_of(&newest, utc_offset_s)
}

/// The same from each entry's newest copy, already chosen.
pub fn transcript_of(newest: &[&ChatEntry], utc_offset_s: i32) -> Transcript {
    let a = archive(newest);
    // where each item came from: the entry that made it, or the legacy entry holding the app's saved form
    let mut made: HashMap<String, &ChatEntry> = HashMap::new();
    for e in newest.iter().filter(|e| !e.deleted) {
        made.insert(e.id.clone(), e);
        if e.kind == "legacy" {
            if let Some(body) = e.body.get("body").and_then(Value::as_object) {
                for key in ["id", "entryID"] {
                    if let Some(id) = body.get(key).and_then(uuid) {
                        made.insert(id, e);
                    }
                }
            }
        }
    }
    let r = |id: &str| {
        made.get(id)
            .map(|e| e.id.clone())
            .unwrap_or_else(|| id.to_string())
    };
    let is_legacy = |id: &str| made.get(id).is_some_and(|e| e.kind == "legacy");
    let items = a
        .transcript()
        .into_iter()
        .map(|shown| match shown {
            Shown::Turn(t, below) => {
                let live = t.status == "working" || t.status == "needs_user";
                let words = (!t.is_recovered()).then(|| OwnerWords {
                    text: t.request.clone(),
                    files: t.sources.clone(),
                    hidden: t.hidden_kinds.clone().unwrap_or_default(),
                    sealed: t.sealed.clone().unwrap_or_default(),
                    hidden_notice: t.hidden_notice(),
                });
                Item::Turn(TurnItem {
                    entry: r(&t.id),
                    legacy: is_legacy(&t.id),
                    client_id: made.get(&t.id).and_then(|e| e.client_id.clone()),
                    request_id: t.request_id.clone(),
                    task_id: t.backend_task_id.clone(),
                    from: if t.is_recovered() { "ferb" } else { "owner" }.into(),
                    task_name: t.task_name.clone(),
                    status: t.status.clone(),
                    status_inferred: t.status_inferred == Some(true),
                    notice: t.response_notice.clone(),
                    merged_into: t.merged_into.clone(),
                    question: t.question.clone(),
                    answered_below: below,
                    answer: (!(t.answer.is_empty()
                        && t.display_text.as_deref().unwrap_or("").is_empty()))
                    .then(|| answer_view(t)),
                    card: t.card.as_ref().map(|c| card_view(c, live)),
                    checklist: t.checklist.as_ref().map(|c| checklist_view(c, live)),
                    first_reply: first_reply(t),
                    parts: t.parts.clone().unwrap_or_default(),
                    goal: t
                        .is_recovered()
                        .then(|| t.task_goal.clone().unwrap_or_default()),
                    words,
                })
            }
            Shown::Reply(rep) => Item::Reply(ReplyItem {
                entry: r(&rep.id),
                legacy: is_legacy(&rep.id),
                answers: r(&rep.turn_id),
                after: r(&rep.after_turn_id),
                part: rep.part.map(|p| p.as_str().to_string()),
                quotes_request: !a.follows_its_turn(rep, utc_offset_s),
                said: rep.said.as_ref().map(said_view),
            }),
            Shown::Notice(n) => {
                let view = NoticeItem {
                    entry: r(&n.entry_id),
                    legacy: is_legacy(&n.entry_id),
                    reference: n.id.clone(),
                    after: n.after_turn_id.as_deref().map(r),
                    text: n.text.clone(),
                    quotes: n.quotes.clone(),
                    cards: n.cards.clone(),
                };
                if made.get(&n.entry_id).is_some_and(|e| e.kind == "post") {
                    Item::Reminder(view)
                } else {
                    Item::Notice(view)
                }
            }
        })
        .collect();
    let questions = newest
        .iter()
        .filter(|e| e.kind == "question" && !e.deleted)
        .map(|e| {
            let placed = Approval::of(&e.body, e.approval_id.as_deref(), e.task_id.as_deref())
                .and_then(|q| a.row_place(&q));
            QuestionItem {
                entry: e.id.clone(),
                approval_id: e.approval_id.clone(),
                request_id: e.request_id.clone(),
                task_id: e.task_id.clone(),
                state: e.state.clone(),
                open: e.open,
                body: Value::Object(e.body.clone()),
                shown: e.open.then(|| {
                    if placed.is_some() {
                        "card_row"
                    } else {
                        "end_of_chat"
                    }
                    .to_string()
                }),
                row: if e.open {
                    placed.map(|(turn, part)| RowRef {
                        turn: made.get(turn).map(|e| e.id.clone()).unwrap_or_default(),
                        part: part.to_string(),
                    })
                } else {
                    None
                },
            }
        })
        .collect();
    Transcript { items, questions }
}
