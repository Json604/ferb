//! The pieces of a turn as FERB.app holds them (`NotchCore`: ResultCard.swift, TurnCard.swift, StepChecklist.swift,
//! ConversationArchive.swift), each read two ways: from the engine's entry bodies (snake_case, `init?(_ value:)`) and
//! from FERB.app's own saved form (camelCase, `init(from: Decoder)`, for `legacy` entries). Serialized as FERB.app
//! encodes them with snake_case keys (an absent optional is left out), which is the phone transcript's shape
//! (engine/fixtures/chat-entries/README.md).

use std::collections::BTreeMap;

use serde::Serialize;
use serde_json::{Map, Value};

use super::text::{file_path, trim};
use crate::entry::int;

// ---- reading an entry's body (JSONSerialization + `as?`) ------------------------------------------------------------

/// `value[key] as? String`.
pub fn s(m: &Map<String, Value>, k: &str) -> Option<String> {
    m.get(k).and_then(Value::as_str).map(str::to_string)
}
/// `(value[key] as? String).flatMap { $0.isEmpty ? nil : $0 }`.
pub fn words(m: &Map<String, Value>, k: &str) -> Option<String> {
    s(m, k).filter(|w| !w.is_empty())
}
/// `value[key] as? Bool`.
pub fn b(m: &Map<String, Value>, k: &str) -> Option<bool> {
    m.get(k).and_then(Value::as_bool)
}
/// `value[key] as? [String]`: every element a text, else nil.
pub fn strs(m: &Map<String, Value>, k: &str) -> Option<Vec<String>> {
    m.get(k).and_then(str_list)
}
pub fn str_list(v: &Value) -> Option<Vec<String>> {
    v.as_array()?
        .iter()
        .map(|x| x.as_str().map(str::to_string))
        .collect()
}
/// `value[key] as? [[String: Any]]`.
pub fn objs<'a>(m: &'a Map<String, Value>, k: &str) -> Option<Vec<&'a Map<String, Value>>> {
    m.get(k)?.as_array()?.iter().map(Value::as_object).collect()
}

// ---- reading FERB.app's saved form (JSONDecoder: a wrong type fails the value) -------------------------------------

/// A value that could not be decoded (Swift's `DecodingError`): the element holding it is left out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Bad;
pub type Got<T> = Result<T, Bad>;

/// `decodeIfPresent`: absent or null is None; a wrong type fails.
pub fn opt<T>(m: &Map<String, Value>, k: &str, f: impl Fn(&Value) -> Option<T>) -> Got<Option<T>> {
    match m.get(k) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => f(v).map(Some).ok_or(Bad),
    }
}
/// `decode`: required.
pub fn req<T>(m: &Map<String, Value>, k: &str, f: impl Fn(&Value) -> Option<T>) -> Got<T> {
    opt(m, k, f)?.ok_or(Bad)
}
pub fn text(v: &Value) -> Option<String> {
    v.as_str().map(str::to_string)
}
pub fn boolean(v: &Value) -> Option<bool> {
    v.as_bool()
}
/// A Swift `Int` from JSON: a whole number only.
pub fn whole(v: &Value) -> Option<i64> {
    v.as_i64()
        .or_else(|| v.as_f64().filter(|f| f.fract() == 0.0).map(|f| f as i64))
}
/// A `Date` in the default strategy: seconds since the reference date.
pub fn date(v: &Value) -> Option<f64> {
    v.as_f64()
}
/// A `UUID`, as its upper-case text.
pub fn uuid(v: &Value) -> Option<String> {
    let t = v.as_str()?;
    let ok = t.len() == 36
        && t.char_indices().all(|(i, c)| {
            if [8, 13, 18, 23].contains(&i) {
                c == '-'
            } else {
                c.is_ascii_hexdigit()
            }
        });
    ok.then(|| t.to_ascii_uppercase())
}
/// A `URL` as JSONEncoder wrote it (its absolute string), shown by its path.
pub fn url_path(v: &Value) -> Option<String> {
    let t = v.as_str()?;
    let path = match t.strip_prefix("file://") {
        Some(rest) => {
            let at = rest.find('/').unwrap_or(rest.len());
            percent_decode(&rest[at..])
        }
        None => t.to_string(),
    };
    Some(file_path(&path))
}
fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            let hex = |c: u8| (c as char).to_digit(16);
            if let (Some(h), Some(l)) = (hex(b[i + 1]), hex(b[i + 2])) {
                out.push((h * 16 + l) as u8);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}
pub fn list_of<T>(v: &Value, f: impl Fn(&Value) -> Option<T>) -> Option<Vec<T>> {
    v.as_array()?.iter().map(f).collect()
}
/// `lossyList`: elements that can't be read are left out; None when absent.
pub fn lossy_list<T>(
    m: &Map<String, Value>,
    k: &str,
    f: impl Fn(&Value) -> Got<T>,
) -> Got<Option<Vec<T>>> {
    match m.get(k) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Array(a)) => Ok(Some(a.iter().filter_map(|x| f(x).ok()).collect())),
        Some(_) => Err(Bad),
    }
}
/// `lossy`: a value that can't be read is None.
pub fn lossy<T>(m: &Map<String, Value>, k: &str, f: impl Fn(&Value) -> Got<T>) -> Option<T> {
    match m.get(k) {
        None | Some(Value::Null) => None,
        Some(v) => f(v).ok(),
    }
}
fn obj(v: &Value) -> Got<&Map<String, Value>> {
    v.as_object().ok_or(Bad)
}

// ---- ResultCard ----------------------------------------------------------------------------------------------------

/// One of the schedules on a `schedules` card (`ScheduleListRow`).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ScheduleRow {
    pub id: String,
    pub name: String,
    pub repeat_rule: String,
    pub state: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_run: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub until: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub watch: Option<String>,
}

impl ScheduleRow {
    fn read(v: &Value) -> Option<ScheduleRow> {
        let m = v.as_object()?;
        Some(ScheduleRow {
            id: s(m, "id")?,
            name: s(m, "name")?,
            repeat_rule: s(m, "repeat")?,
            state: s(m, "state").unwrap_or_else(|| "active".into()),
            next_run: s(m, "next_run"),
            until: s(m, "until"),
            watch: s(m, "watch"),
        })
    }
    fn saved(v: &Value) -> Got<ScheduleRow> {
        let m = obj(v)?;
        Ok(ScheduleRow {
            id: req(m, "id", text)?,
            name: req(m, "name", text)?,
            repeat_rule: req(m, "repeatRule", text)?,
            state: opt(m, "state", text)?.unwrap_or_else(|| "active".into()),
            next_run: opt(m, "nextRun", text)?,
            until: opt(m, "until", text)?,
            watch: opt(m, "watch", text)?,
        })
    }
}

/// A typed card (step 8e) built from FERB's own records: which card is the daemon's `type`.
#[derive(Debug, Clone, PartialEq, Serialize, Default)]
pub struct ResultCard {
    #[serde(rename = "type")]
    pub kind_of: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub due: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repeat_rule: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_run: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub schedule_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub until: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub what: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub when: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub watch: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub started: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timer_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repeat_of: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub item_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub amount: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub merchant: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub site: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub saved_card: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recipient: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub app: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub page: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub imported_from: Option<String>,
    pub changed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub was_repeat: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub was_request: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub count: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub schedules: Option<Vec<ScheduleRow>>,
}

/// The card types FERB.app draws (`ResultCard.types`).
pub const CARD_TYPES: [&str; 9] = [
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

impl ResultCard {
    /// `ResultCard.init?(_ value:)`: a card of a known type with something to show.
    pub fn read(v: &Value) -> Option<ResultCard> {
        let m = v.as_object()?;
        let kind_of = s(m, "type").filter(|t| CARD_TYPES.contains(&t.as_str()))?;
        let w = |k: &str| words(m, k);
        let c = ResultCard {
            kind_of,
            line: w("line"),
            title: w("title"),
            due: w("due"),
            repeat_rule: w("repeat"),
            next_run: w("next_run"),
            schedule_id: w("schedule_id"),
            until: w("until"),
            kind: w("kind"),
            id: w("id"),
            what: w("what"),
            when: w("when"),
            watch: w("watch"),
            started: w("started"),
            timer_id: w("timer_id").or_else(|| w("id")),
            repeat_of: w("repeat_of"),
            item_id: w("item_id"),
            text: w("text"),
            state: w("state"),
            amount: w("amount"),
            merchant: w("merchant"),
            site: w("site"),
            saved_card: w("card"),
            recipient: w("recipient"),
            app: w("app"),
            account: w("account"),
            host: w("host"),
            path: w("path"),
            name: w("name"),
            page: w("page"),
            changed: b(m, "changed").unwrap_or(false),
            was_repeat: w("was_repeat"),
            was_request: w("was_request"),
            request: w("request"),
            count: m.get("count").and_then(whole),
            schedules: m
                .get("schedules")
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(ScheduleRow::read).collect()),
            imported_from: m
                .get("filter")
                .and_then(Value::as_object)
                .and_then(|f| words(f, "imported_from")),
        };
        let has_nothing = match c.kind_of.as_str() {
            "note" => c.item_id.is_none() || c.text.is_none(),
            "link" => c.host.is_none(),
            "file" => c.path.is_none(),
            "send" => c.recipient.is_none() && c.site.is_none() && c.app.is_none(),
            "payment" => c.amount.is_none() && c.merchant.is_none(),
            "event" => c.what.is_none(),
            "page" => c.page.is_none(),
            "schedules" => c.schedules.is_none(),
            _ => false,
        };
        (!has_nothing).then_some(c)
    }

    /// `ResultCard.list`: the cards of an event's `cards`; None when it has none.
    pub fn list(v: Option<&Value>) -> Option<Vec<ResultCard>> {
        let cards: Vec<ResultCard> = v?.as_array()?.iter().filter_map(ResultCard::read).collect();
        (!cards.is_empty()).then_some(cards)
    }

    /// FERB.app's saved form.
    pub fn saved(v: &Value) -> Got<ResultCard> {
        let m = obj(v)?;
        let w = |k: &str| opt(m, k, text);
        Ok(ResultCard {
            kind_of: req(m, "type", text)?,
            line: w("line")?,
            title: w("title")?,
            due: w("due")?,
            repeat_rule: w("repeatRule")?,
            next_run: w("nextRun")?,
            schedule_id: w("scheduleID")?,
            until: w("until")?,
            kind: w("kind")?,
            id: w("id")?,
            what: w("what")?,
            when: w("when")?,
            watch: w("watch")?,
            started: w("started")?,
            timer_id: w("timerID")?,
            repeat_of: w("repeatOf")?,
            item_id: w("itemID")?,
            text: w("text")?,
            state: w("state")?,
            amount: w("amount")?,
            merchant: w("merchant")?,
            site: w("site")?,
            saved_card: w("savedCard")?,
            recipient: w("recipient")?,
            app: w("app")?,
            account: w("account")?,
            host: w("host")?,
            path: w("path")?,
            name: w("name")?,
            page: w("page")?,
            imported_from: w("importedFrom")?,
            changed: opt(m, "changed", boolean)?.unwrap_or(false),
            was_repeat: w("wasRepeat")?,
            was_request: w("wasRequest")?,
            request: w("request")?,
            count: opt(m, "count", whole)?,
            schedules: lossy_list(m, "schedules", ScheduleRow::saved)?,
        })
    }

    /// The lines the cards stand for (`CardLayout.lines`): only cards of a drawn type.
    pub fn lines(cards: &[ResultCard]) -> Vec<&str> {
        cards
            .iter()
            .filter(|c| CARD_TYPES.contains(&c.kind_of.as_str()))
            .filter_map(|c| c.line.as_deref())
            .collect()
    }
}

// ---- RuntimeSwitch -------------------------------------------------------------------------------------------------

/// The offer of another AI service when an account problem ended the work (ORCH-315).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RuntimeSwitch {
    pub runtime: String,
    pub problem: String,
    pub role: String,
    pub choices: Vec<String>,
}

impl RuntimeSwitch {
    pub fn read(v: Option<&Value>) -> Option<RuntimeSwitch> {
        let m = v?.as_object()?;
        let choices = strs(m, "choices").filter(|c| !c.is_empty())?;
        Some(RuntimeSwitch {
            runtime: s(m, "runtime").unwrap_or_default(),
            problem: s(m, "problem").unwrap_or_default(),
            role: s(m, "role").unwrap_or_else(|| "worker".into()),
            choices,
        })
    }
    pub fn saved(v: &Value) -> Got<RuntimeSwitch> {
        let m = obj(v)?;
        Ok(RuntimeSwitch {
            runtime: opt(m, "runtime", text)?.unwrap_or_default(),
            problem: opt(m, "problem", text)?.unwrap_or_default(),
            role: opt(m, "role", text)?.unwrap_or_else(|| "worker".into()),
            choices: req(m, "choices", str_list)?,
        })
    }
}

// ---- the card of a multi-part request (ORCH-319) -------------------------------------------------------------------

/// A question FERB asks inside a row.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RowQuestion {
    pub approval_id: String,
    pub text: String,
    pub options: Vec<String>,
}

impl RowQuestion {
    fn read(v: Option<&Value>) -> Option<RowQuestion> {
        let m = v?.as_object()?;
        Some(RowQuestion {
            approval_id: words(m, "approval_id")?,
            text: s(m, "text").unwrap_or_default(),
            options: strs(m, "options").unwrap_or_default(),
        })
    }
    fn saved(v: &Value) -> Got<RowQuestion> {
        let m = obj(v)?;
        Ok(RowQuestion {
            approval_id: req(m, "approvalID", text)?,
            text: opt(m, "text", text)?.unwrap_or_default(),
            options: opt(m, "options", str_list)?.unwrap_or_default(),
        })
    }
}

/// One part of the owner's request in the card.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CardRow {
    pub task_id: String,
    pub part: String,
    pub label: String,
    pub state: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub why: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chosen: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verified: Option<bool>,
    pub direct: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub step: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub step_task_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seq: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub question: Option<RowQuestion>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub card_request_id: Option<String>,
}

impl CardRow {
    fn read(m: &Map<String, Value>, fallback: &str) -> Option<CardRow> {
        let part = words(m, "part")?;
        let state = s(m, "state").unwrap_or_else(|| "working".into());
        let question = if state == "asks" || state == "paused" {
            RowQuestion::read(m.get("question"))
        } else {
            None
        };
        Some(CardRow {
            task_id: words(m, "task_id").unwrap_or_else(|| fallback.to_string()),
            part,
            label: s(m, "label").unwrap_or_default(),
            state,
            text: words(m, "text"),
            note: words(m, "note"),
            why: words(m, "why"),
            chosen: words(m, "chosen"),
            verified: b(m, "verified"),
            direct: b(m, "direct").unwrap_or(false),
            step: s(m, "step"),
            step_task_id: s(m, "step_task_id"),
            seq: s(m, "seq"),
            question,
            card_request_id: s(m, "card_request_id"),
        })
    }
    fn saved(v: &Value) -> Got<CardRow> {
        let m = obj(v)?;
        Ok(CardRow {
            task_id: req(m, "taskID", text)?,
            part: req(m, "part", text)?,
            label: opt(m, "label", text)?.unwrap_or_default(),
            state: opt(m, "state", text)?.unwrap_or_else(|| "working".into()),
            text: opt(m, "text", text)?,
            note: opt(m, "note", text)?,
            why: opt(m, "why", text)?,
            chosen: opt(m, "chosen", text)?,
            verified: opt(m, "verified", boolean)?,
            direct: opt(m, "direct", boolean)?.unwrap_or(false),
            step: opt(m, "step", text)?,
            step_task_id: opt(m, "stepTaskID", text)?,
            seq: opt(m, "seq", text)?,
            question: lossy(m, "question", RowQuestion::saved),
            card_request_id: opt(m, "cardRequestID", text)?,
        })
    }

    /// Whether this open question belongs in this row (`CardRow.asks`).
    pub fn asks(&self, q: &Approval) -> bool {
        if self.question.as_ref().map(|x| x.approval_id.as_str()) == Some(q.id.as_str()) {
            return true;
        }
        if let Some(part) = &q.part {
            if *part == self.part && q.task_id.as_deref() == Some(self.task_id.as_str()) {
                return true;
            }
        }
        match (&self.step_task_id, &q.source_task_id) {
            (Some(step), Some(source)) if !step.is_empty() => {
                source == step || source.starts_with(&format!("{step}."))
            }
            _ => false,
        }
    }
}

/// One plan step as the daemon names it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PlanStep {
    pub id: String,
    pub label: String,
    pub kind: String,
    pub depends_on: Vec<String>,
    pub step_task_id: String,
    #[serde(rename = "loop")]
    pub repeats: bool,
}

impl PlanStep {
    fn read(v: &Value) -> Option<PlanStep> {
        let m = v.as_object()?;
        let id = words(m, "id")?;
        Some(PlanStep {
            label: words(m, "label").unwrap_or_else(|| id.clone()),
            id,
            kind: s(m, "kind").unwrap_or_else(|| "do".into()),
            depends_on: strs(m, "depends_on").unwrap_or_default(),
            step_task_id: s(m, "step_task_id").unwrap_or_default(),
            repeats: b(m, "loop").unwrap_or(false),
        })
    }
    pub fn list(v: Option<&Value>) -> Vec<PlanStep> {
        v.and_then(Value::as_array)
            .map(|a| a.iter().filter_map(PlanStep::read).collect())
            .unwrap_or_default()
    }
    fn saved(v: &Value) -> Got<PlanStep> {
        let m = obj(v)?;
        let id = req(m, "id", text)?;
        Ok(PlanStep {
            label: opt(m, "label", text)?.unwrap_or_else(|| id.clone()),
            kind: opt(m, "kind", text)?.unwrap_or_else(|| "do".into()),
            depends_on: opt(m, "dependsOn", str_list)?.unwrap_or_default(),
            step_task_id: opt(m, "stepTaskID", text)?.unwrap_or_default(),
            repeats: opt(m, "loop", boolean)?.unwrap_or(false),
            id,
        })
    }
}

/// The card of one turn of a multi-part task (`TurnCard`).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Card {
    pub task_id: String,
    pub request_id: String,
    pub rows: Vec<CardRow>,
    pub done: i64,
    pub total: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<String>,
    pub lines: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub headline: Option<String>,
    pub plan: Vec<PlanStep>,
}

impl Card {
    /// `TurnCard.init?(_ payload:, taskID:)`.
    pub fn read(v: Option<&Value>, task_id: &str) -> Option<Card> {
        let m = v?.as_object()?;
        let rows: Vec<CardRow> = objs(m, "rows")?
            .into_iter()
            .filter_map(|r| CardRow::read(r, task_id))
            .collect();
        let total = m.get("total").and_then(int).unwrap_or(rows.len() as i64);
        Some(Card {
            task_id: task_id.to_string(),
            request_id: s(m, "request_id").unwrap_or_default(),
            done: m.get("done").and_then(int).unwrap_or(0),
            total,
            line: words(m, "line"),
            lines: strs(m, "lines").unwrap_or_default(),
            headline: words(m, "headline"),
            plan: PlanStep::list(m.get("plan")),
            rows,
        })
    }
    pub fn saved(v: &Value) -> Got<Card> {
        let m = obj(v)?;
        let rows = lossy_list(m, "rows", CardRow::saved)?.unwrap_or_default();
        Ok(Card {
            task_id: req(m, "taskID", text)?,
            request_id: opt(m, "requestID", text)?.unwrap_or_default(),
            done: opt(m, "done", whole)?.unwrap_or(0),
            total: opt(m, "total", whole)?.unwrap_or(rows.len() as i64),
            line: opt(m, "line", text)?,
            lines: opt(m, "lines", str_list)?.unwrap_or_default(),
            headline: opt(m, "headline", text)?,
            plan: lossy_list(m, "plan", PlanStep::saved)?.unwrap_or_default(),
            rows,
        })
    }
}

/// What one card row shows once read from history (`RowDisplay.make` with no live step states).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Shows {
    pub kind: String,
    pub words: Option<String>,
}

impl Shows {
    pub fn of(row: &CardRow, live: bool) -> Shows {
        let (kind, words) = match row.state.as_str() {
            "preview" => (if live { "previewRunning" } else { "preview" }, None),
            "asks" => ("needsYou", None),
            "paused" => ("paused", None),
            "updated" => ("updated", None),
            "done" => ("done", None),
            "done_with_gaps" => ("gaps", Some("Delivered, with parts not done")),
            "stopped" => ("stopped", None),
            "failed" => ("failed", None),
            _ if !live => ("notFinished", Some("Not finished")),
            // no step states in history: a row of a turn still running is only known to be under way
            _ => ("working", Some("Working")),
        };
        Shows {
            kind: kind.into(),
            words: words.map(str::to_string),
        }
    }
}

// ---- a single ask's checklist (APP-894) ----------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Checklist {
    pub task_id: String,
    pub steps: Vec<PlanStep>,
    pub ended: BTreeMap<String, String>,
    pub finished: bool,
}

/// What a step may end as (`StepChecklist.end`).
const ENDED: [&str; 6] = [
    "done",
    "done_with_gaps",
    "failed",
    "not_finished",
    "wont_run",
    "stopped",
];

impl Checklist {
    /// The plan entry's `checklist` (`EntryFold.showPlan`): its steps, and once `finished` what each ended as.
    pub fn read(v: Option<&Value>) -> Option<Checklist> {
        let m = v?.as_object()?;
        let task_id = s(m, "task_id")?;
        let steps = PlanStep::list(m.get("steps"));
        if steps.is_empty() {
            return None;
        }
        let mut c = Checklist {
            task_id,
            steps,
            ended: BTreeMap::new(),
            finished: false,
        };
        if b(m, "finished") == Some(true) {
            // `as? [String: String]`: one value of another type and none is read
            let ended: BTreeMap<String, String> = m
                .get("ended")
                .and_then(Value::as_object)
                .and_then(|e| {
                    e.iter()
                        .map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                        .collect()
                })
                .unwrap_or_default();
            c.ended = ended
                .into_iter()
                .filter(|(_, v)| !v.is_empty() && ENDED.contains(&v.as_str()))
                .collect();
            c.finished = true;
        }
        Some(c)
    }
    pub fn saved(v: &Value) -> Got<Checklist> {
        let m = obj(v)?;
        let ended = match m.get("ended") {
            None | Some(Value::Null) => BTreeMap::new(),
            Some(Value::Object(e)) => e
                .iter()
                .map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                .collect::<Option<_>>()
                .ok_or(Bad)?,
            Some(_) => return Err(Bad),
        };
        Ok(Checklist {
            task_id: req(m, "taskID", text)?,
            steps: lossy_list(m, "steps", PlanStep::saved)?.unwrap_or_default(),
            ended,
            finished: opt(m, "finished", boolean)?.unwrap_or(false),
        })
    }

    /// Steps that ended done.
    pub fn done_count(&self) -> usize {
        self.steps
            .iter()
            .filter(|s| self.ended.get(&s.id).map(String::as_str) == Some("done"))
            .count()
    }
}

/// One checklist line as history shows it (`ChecklistLine.make` with no live state).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ChecklistLine {
    pub step: String,
    pub kind: String,
    pub words: Option<String>,
}

impl ChecklistLine {
    pub fn of(step: &PlanStep, ended: Option<&str>, live: bool) -> ChecklistLine {
        let (kind, words) = match ended {
            Some("done") => ("done", None),
            Some("done_with_gaps") => ("gaps", Some("Parts not done")),
            Some("failed") => ("failed", Some("Couldn't do")),
            Some("wont_run") => (
                "wontRun",
                Some("Won't run: an earlier step couldn't be done"),
            ),
            Some("stopped") => ("stopped", Some("Stopped by you")),
            Some("not_finished") => ("notFinished", Some("Not finished")),
            _ if !live => ("notFinished", Some("Not finished")),
            _ => ("upcoming", None),
        };
        ChecklistLine {
            step: step.id.clone(),
            kind: kind.into(),
            words: words.map(str::to_string),
        }
    }
}

/// `ChecklistSummary.collapsed`.
pub fn collapsed(done: usize, total: usize) -> String {
    if done == total {
        format!("{total} step{} done", if total == 1 { "" } else { "s" })
    } else {
        format!("{done} of {total} steps done")
    }
}

// ---- answers, sealed IDs, quotes -----------------------------------------------------------------------------------

/// What a later answer says (APP-914).
#[derive(Debug, Clone, PartialEq)]
pub struct LaterAnswer {
    pub answer: String,
    pub display_text: Option<String>,
    pub status: String,
    pub verified: bool,
    pub artifacts: Vec<String>,
    pub note: Option<String>,
    pub code_lines: Option<Vec<String>>,
    pub cards: Option<Vec<ResultCard>>,
    pub runtime_switch: Option<RuntimeSwitch>,
}

impl LaterAnswer {
    /// `EntryFold.later`.
    pub fn read(body: &Map<String, Value>) -> LaterAnswer {
        LaterAnswer {
            answer: s(body, "text").unwrap_or_default(),
            display_text: s(body, "display_text"),
            status: s(body, "status").unwrap_or_else(|| "done".into()),
            verified: b(body, "verified").unwrap_or(false),
            artifacts: strs(body, "artifacts")
                .unwrap_or_default()
                .iter()
                .map(|p| file_path(p))
                .collect(),
            note: s(body, "note"),
            code_lines: strs(body, "lines"),
            cards: ResultCard::list(body.get("cards")),
            runtime_switch: RuntimeSwitch::read(body.get("runtime_switch")),
        }
    }
    pub fn saved(v: &Value) -> Got<LaterAnswer> {
        let m = obj(v)?;
        Ok(LaterAnswer {
            answer: req(m, "answer", text)?,
            display_text: opt(m, "displayText", text)?,
            status: opt(m, "status", text)?.unwrap_or_else(|| "done".into()),
            verified: opt(m, "verified", boolean)?.unwrap_or(false),
            artifacts: opt(m, "artifacts", |v| list_of(v, url_path))?.unwrap_or_default(),
            note: opt(m, "note", text)?,
            code_lines: opt(m, "codeLines", str_list)?,
            cards: lossy_list(m, "cards", ResultCard::saved)?,
            runtime_switch: lossy(m, "runtimeSwitch", RuntimeSwitch::saved),
        })
    }
}

/// An identifier FERB kept sealed from a message (GATE-604): its kind and saved name, never a value.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Sealed {
    pub kind: String,
    pub name: String,
}

impl Sealed {
    /// The ack's `sealed` kept on the message entry; None when none.
    pub fn list(body: &Map<String, Value>) -> Option<Vec<Sealed>> {
        let kept: Vec<Sealed> = objs(body, "sealed")
            .unwrap_or_default()
            .into_iter()
            .filter_map(|m| {
                Some(Sealed {
                    kind: s(m, "kind")?,
                    name: words(m, "name")?,
                })
            })
            .collect();
        (!kept.is_empty()).then_some(kept)
    }
    pub fn saved(v: &Value) -> Got<Sealed> {
        let m = obj(v)?;
        opt(m, "shown", boolean)?;
        Ok(Sealed {
            kind: req(m, "kind", text)?,
            name: req(m, "name", text)?,
        })
    }
}

/// Outside text a notice quotes, with where it came from (data, shown as a quote).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Quote {
    pub text: String,
    pub origin: String,
}

impl Quote {
    pub fn saved(v: &Value) -> Got<Quote> {
        let m = obj(v)?;
        Ok(Quote {
            text: req(m, "text", text)?,
            origin: opt(m, "origin", text)?.unwrap_or_default(),
        })
    }
}

/// The fields of a question the fold reads (`DaemonApproval`): where an open one is drawn.
#[derive(Debug, Clone, PartialEq)]
pub struct Approval {
    pub id: String,
    pub task_id: Option<String>,
    pub source_task_id: Option<String>,
    pub part: Option<String>,
}

impl Approval {
    /// From a question entry: its body with the entry's own `approval_id` and `task_id` over the body's.
    pub fn of(
        body: &Map<String, Value>,
        approval_id: Option<&str>,
        task_id: Option<&str>,
    ) -> Option<Approval> {
        let id = approval_id.filter(|i| !i.is_empty())?.to_string();
        let task_id = task_id.map(str::to_string);
        Some(Approval {
            id,
            source_task_id: s(body, "source_task_id").or_else(|| task_id.clone()),
            task_id,
            part: words(body, "part"),
        })
    }
}

/// `text` without surrounding whitespace is not empty.
pub fn says(text: &str) -> bool {
    !trim(text).is_empty()
}
