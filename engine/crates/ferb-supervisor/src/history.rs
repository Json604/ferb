//! The planner's conversation (supervisor.py:3778 `_conversation_blocks`, :3991 `_timeline`, :3818 `_self_post`,
//! :4088 `_told_in_reply`, :4174 `_recent_entries`; ORCH-303, ORCH-566, ORCH-588, ORCH-596, ORCH-619): the chat's
//! entries after FERB's running summary, oldest first, each complete with its display line, its own text (the S4
//! payload), its origins and its lineage handle.
//!
//! The rows come in as the journal returned them; what the timeline reads besides (a task's recorded outcome for
//! `told_in`, a notice's sources) comes in through [`TimelineReads`].

use std::collections::BTreeSet;

use ferb_clock::py::{fmt, Time};
use ferb_clock::zone::Zone;
use ferb_context::assemble::ContextBlock;
use ferb_context::tokens::Tokenizer;
use ferb_sanitize::origin;
use ferb_sanitize::untrusted::as_data;
use serde_json::{Map, Value};

use crate::lines::{
    entry_payload, first_content_line, py_str, result_reference, short_report, stored_values,
    within_tokens,
};
use crate::seams::JournalRow;
use crate::words::*;

/// One conversation entry as the planner reads it (supervisor.py:800 `_Entry`).
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub at: Time,
    /// The display line: tag, time, speaker, text, full-source reference.
    pub text: String,
    /// The entry's own text only (S4).
    pub payload: String,
    pub reference: String,
    pub chat: bool,
    pub origins: BTreeSet<String>,
    /// `entry:<journal id>`.
    pub handle: String,
    pub kind: String,
    pub refs: Vec<String>,
    /// An owner message that opens an exchange.
    pub opens: bool,
    pub values: Option<Value>,
    /// ORCH-619: a result's id, "" for any other entry.
    pub result: String,
}

/// What the timeline reads besides the rows.
pub trait TimelineReads {
    /// `tasks.column(task_id, "outcome_json")`.
    fn outcome_json(&self, task_id: &str) -> Option<String>;
    /// attention.py:175 `notice_sources(db, notice_id)`.
    fn notice_sources(&self, notice_id: &str) -> Vec<String>;
}

/// The inputs of one `_timeline` call.
pub struct Timeline<'a> {
    /// This turn's request id(s): their messages are the new message, not history.
    pub current: &'a BTreeSet<String>,
    pub conversation: Option<&'a str>,
    /// `journal.entries_of(*CONVERSATION_KINDS, *SELF_POSTS)`, oldest first.
    pub rows: &'a [JournalRow],
    /// `continuity.task_ids(conversation)`, None without a conversation.
    pub task_ids: Option<&'a BTreeSet<String>>,
    /// `_covered(summary)`: the handles FERB's running summaries folded in.
    pub covered: &'a BTreeSet<String>,
    pub reads: &'a dyn TimelineReads,
    pub zone: &'a Zone,
}

fn s(data: &Map<String, Value>, key: &str) -> Option<String> {
    data.get(key).filter(|v| v.is_string()).map(py_str)
}

/// `str(data.get(key))`: "None" when missing.
fn f(data: &Map<String, Value>, key: &str) -> String {
    data.get(key).map(py_str).unwrap_or_else(|| "None".into())
}

/// `str(data.get(key, ""))`.
fn text(data: &Map<String, Value>, key: &str) -> String {
    data.get(key).map(py_str).unwrap_or_default()
}

/// `data.get(key) in leave_out`.
fn held_in(data: &Map<String, Value>, key: &str, set: &BTreeSet<String>) -> bool {
    s(data, key).is_some_and(|v| set.contains(&v))
}

fn shown(held: &BTreeSet<String>) -> String {
    origin::shown(held.iter().map(String::as_str))
}

fn reserved(conversation: &str) -> bool {
    conversation.starts_with("schedule:") || conversation.starts_with("preview:")
}

/// continuity.py:22 `in_conversation`.
pub fn in_conversation(kind: &str, data: &Map<String, Value>, conversation: &str) -> bool {
    if kind == "chat.notice" && !data.contains_key("session_id") {
        return !reserved(conversation);
    }
    match data.get("session_id") {
        None => conversation == "owner",
        Some(v) => v.as_str() == Some(conversation),
    }
}

fn when(at: &Time, zone: &Zone) -> String {
    fmt(&zone.local(at), "%a %d %b %H:%M")
}

/// supervisor.py:3818 `_self_post` (ORCH-588).
pub fn self_post(
    entry: &JournalRow,
    conversation: Option<&str>,
    task_ids: Option<&BTreeSet<String>>,
    zone: &Zone,
) -> Option<Entry> {
    let data = &entry.data;
    let held = origin::split(Some(&entry.origin));
    if let Some(conversation) = conversation {
        let out = if entry.kind != "chat.part" {
            reserved(conversation)
        } else {
            task_ids.is_none_or(|ids| !held_in(data, "task_id", ids))
        };
        if out {
            return None;
        }
    }
    let body = text(data, "text");
    let (who, said) = if entry.kind == "timer.delivered" {
        let what = if data.get("kind").and_then(Value::as_str) == Some("timer") {
            "timer went off"
        } else {
            "reminder arrived"
        };
        (
            format!("FERB (code record: {what}; its words were written when it was set)"),
            text(data, "message"),
        )
    } else {
        let short = short_report(
            &if entry.kind == "chat.part" {
                first_content_line(&body)
            } else {
                body.clone()
            },
            false,
        );
        let cut = if short != body { "; short form" } else { "" };
        let who = if entry.kind == "schedule.posted" {
            format!(
                "FERB (posted by itself: scheduled run “{}” result, {}{cut}; model-written with code's lines; \
                 origin {})",
                text(data, "name"),
                f(data, "status"),
                shown(&held)
            )
        } else {
            format!(
                "FERB (posted by itself: part “{}” of task {}, {}{cut}; model-written; origin {})",
                text(data, "label"),
                f(data, "task_id"),
                f(data, "state"),
                shown(&held)
            )
        };
        (who, as_data(&short))
    };
    let result = if RESULT_KINDS.contains(&entry.kind.as_str()) {
        entry.id.clone()
    } else {
        String::new()
    };
    let reference = if result.is_empty() {
        format!("chat.history entry_id={}", entry.id)
    } else {
        result_reference(&entry.id, data.get("task_id").map(py_str).as_deref())
    };
    Some(Entry {
        at: entry.at,
        text: format!("({}) {who}: {said}", when(&entry.at, zone)),
        payload: entry_payload(&entry.kind, data),
        reference,
        chat: true,
        origins: held,
        handle: format!("{ENTRY}{}", entry.id),
        kind: entry.kind.clone(),
        refs: Vec::new(),
        opens: false,
        values: stored_values(&entry.kind, data),
        result,
    })
}

/// supervisor.py:4088 `_told_in_reply` (ORCH-596).
fn told_in_reply(
    data: &Map<String, Value>,
    replied: &BTreeSet<String>,
    reads: &dyn TimelineReads,
) -> bool {
    if !matches!(
        data.get("status").and_then(Value::as_str),
        Some("cancelled") | Some("failed")
    ) {
        return false;
    }
    let told = match data
        .get("told_in")
        .filter(|v| crate::lines::truthy(Some(v)))
    {
        Some(v) => Some(v.clone()),
        None => {
            let stored = reads
                .outcome_json(&f(data, "task_id"))
                .filter(|t| !t.is_empty())
                .unwrap_or_else(|| "{}".into());
            serde_json::from_str::<Value>(&stored)
                .ok()
                .and_then(|v| v.get("told_in").cloned())
        }
    };
    match told {
        Some(v) if crate::lines::truthy(Some(&v)) => replied.contains(&py_str(&v)),
        _ => false,
    }
}

/// supervisor.py:3991 `_timeline`: the entries after the running summary, each shown with its `[cN]` tag, and the
/// tags' times (`ages`, in order).
pub fn timeline(t: &Timeline<'_>) -> (Vec<Entry>, Vec<(String, Time)>) {
    let leave_out = t.current;
    let replied: BTreeSet<String> = t
        .rows
        .iter()
        .filter(|e| e.kind == "chat.reply" && !held_in(&e.data, "request_id", leave_out))
        .map(|e| f(&e.data, "request_id"))
        .collect();
    let mut answers: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    for e in t.rows.iter().filter(|e| e.kind == "task.answer") {
        answers.insert(f(&e.data, "task_id"), e.id.clone());
    }
    let mut items: Vec<Entry> = Vec::new();
    for entry in t.rows {
        let data = &entry.data;
        if held_in(data, "request_id", leave_out) || held_in(data, "read_with", leave_out) {
            continue;
        }
        if SELF_POSTS.contains(&entry.kind.as_str()) {
            if let Some(made) = self_post(entry, t.conversation, t.task_ids, t.zone) {
                items.push(made);
            }
            continue;
        }
        if let Some(conversation) = t.conversation {
            if entry.kind == "task.answer" {
                if t.task_ids.is_none_or(|ids| !held_in(data, "task_id", ids)) {
                    continue;
                }
            } else if !in_conversation(&entry.kind, data, conversation) {
                continue;
            }
        }
        let mut who = if entry.kind == "chat.message" {
            "Owner".to_string()
        } else {
            "FERB".to_string()
        };
        let mut said = text(data, "text");
        let held = origin::split(Some(&entry.origin));
        let outside = held.iter().any(|o| origin::is_external(o));
        let mut result = if entry.kind == "task.answer" {
            entry.id.clone()
        } else {
            String::new()
        };
        if entry.kind == "task.answer" {
            if told_in_reply(data, &replied, t.reads) {
                continue;
            }
            let short = short_report(&said, crate::lines::truthy(data.get("index")));
            who.push_str(&format!(
                " (task {} result, {}{}; origin {})",
                f(data, "task_id"),
                f(data, "status"),
                if short != said { "; short form" } else { "" },
                shown(&held)
            ));
            said = as_data(&short);
        } else if entry.kind == "chat.reply" {
            let code = data
                .get("lines")
                .filter(|v| crate::lines::truthy(Some(v)))
                .map(py_str)
                .unwrap_or_default();
            let prose = if !code.is_empty() && said.ends_with(&code) {
                ferb_orchestrator::py::rstrip(&said[..said.len() - code.len()]).to_string()
            } else {
                said.clone()
            };
            let end = data
                .get("told_end")
                .filter(|v| crate::lines::truthy(Some(v)))
                .map(py_str)
                .unwrap_or_default();
            let code_line = if code.is_empty() {
                String::new()
            } else {
                format!("\nFERB (code record): {code}")
            };
            if !end.is_empty() && prose.starts_with(&end) {
                let short = short_report(&end, false);
                let rest = ferb_orchestrator::py::strip(&prose[end.len()..]).to_string();
                result = answers
                    .get(&f(data, "told_task"))
                    .cloned()
                    .unwrap_or_default();
                who.push_str(&format!(
                    " (task {} result, stopped, as code recorded it{}; origin {})",
                    f(data, "told_task"),
                    if short != end { "; short form" } else { "" },
                    shown(&held)
                ));
                let mut text = as_data(&short);
                if !rest.is_empty() {
                    text.push_str("\nFERB (reply, model-written, unchecked): ");
                    text.push_str(&if outside { as_data(&rest) } else { rest });
                }
                text.push_str(&code_line);
                said = text;
            } else if held.len() == 1 && held.contains(origin::FERB) {
                who.push_str(" (code record)");
            } else {
                who.push_str(" (reply, model-written, unchecked");
                if outside {
                    who.push_str(&format!("; origin {}", shown(&held)));
                }
                who.push(')');
                said = (if outside { as_data(&prose) } else { prose }) + &code_line;
            }
        } else if entry.kind == "chat.clarify" && outside {
            who.push_str(&format!(" (origin {})", shown(&held)));
            said = as_data(&said);
        }
        let mut refs = Vec::new();
        if entry.kind == "chat.notice" {
            who.push_str(&format!(" (noticed; origin {})", shown(&held)));
            let mut notice = Map::new();
            notice.insert("text".into(), Value::String(said.clone()));
            notice.insert(
                "quotes".into(),
                data.get("quotes")
                    .cloned()
                    .unwrap_or(Value::Array(Vec::new())),
            );
            said = ferb_py::json::dumps(&Value::Object(notice));
            refs = t.reads.notice_sources(&text(data, "id"));
        }
        let task_of = if entry.kind == "chat.reply" {
            data.get("told_task")
        } else {
            data.get("task_id")
        };
        let reference = if result.is_empty() {
            format!("chat.history entry_id={}", entry.id)
        } else {
            result_reference(
                &result,
                task_of.filter(|v| !v.is_null()).map(py_str).as_deref(),
            )
        };
        items.push(Entry {
            at: entry.at,
            text: format!("({}) {who}: {said}", when(&entry.at, t.zone)),
            payload: entry_payload(&entry.kind, data),
            reference,
            chat: true,
            origins: held,
            handle: format!("{ENTRY}{}", entry.id),
            kind: entry.kind.clone(),
            refs,
            opens: entry.kind == "chat.message" && !crate::lines::truthy(data.get("read_with")),
            values: stored_values(&entry.kind, data),
            result,
        });
    }
    let mut ages = Vec::new();
    let mut shown_entries = Vec::new();
    for (n, item) in items.into_iter().enumerate() {
        let tag = format!("[c{}] ", n + 1);
        if t.covered.contains(&item.handle) {
            continue;
        }
        ages.push((format!("c{}", n + 1), item.at));
        let tail = if item.result.is_empty() {
            format!("Full source: {}", item.reference)
        } else {
            item.reference.clone()
        };
        shown_entries.push(Entry {
            text: format!("{tag}{}\n[{tail}]", item.text),
            ..item
        });
    }
    (shown_entries, ages)
}

/// supervisor.py:4174 `_recent_entries`: the last RECENT_EXCHANGES exchanges, oldest first.
pub fn recent_entries(entries: &[Entry]) -> Vec<Entry> {
    let mut groups: Vec<Vec<&Entry>> = Vec::new();
    for e in entries {
        if e.opens || groups.is_empty() {
            groups.push(vec![e]);
        } else if let Some(last) = groups.last_mut() {
            last.push(e);
        }
    }
    let from = groups.len().saturating_sub(RECENT_EXCHANGES);
    groups[from..]
        .iter()
        .flat_map(|g| g.iter().map(|e| (*e).clone()))
        .collect()
}

/// A context block with the origins of what it shows (S3), as `_supervise` carries each.
#[derive(Clone, Debug, PartialEq)]
pub struct Sourced {
    pub block: ContextBlock,
    pub origins: BTreeSet<String>,
}

/// supervisor.py:3778 `_conversation_blocks`: the header (with FERB's running summary) and one cacheable history
/// block per entry; the tag ages; the newest shown result's id ("" when none). `summary`: the chat's latest
/// running summary row. `since_last_notice` (the attention check): only the entries after the last notice.
pub fn conversation_blocks(
    entries: Vec<Entry>,
    summary: Option<&JournalRow>,
    since_last_notice: bool,
    tokens: Tokenizer,
) -> Vec<Sourced> {
    let entries = if since_last_notice {
        match entries.iter().rposition(|e| e.kind == "chat.notice") {
            Some(last) => entries[last + 1..].to_vec(),
            None => entries,
        }
    } else {
        entries
    };
    let mut header = CONVERSATION_HEADER.to_string();
    let mut refs = Vec::new();
    let mut made = BTreeSet::new();
    let mut payload = String::new();
    let mut values = None;
    if let Some(summary) = summary {
        let said = text(&summary.data, "text");
        header.push_str(SUMMARY_HEADER);
        header.push_str(&within_tokens(&said, SUMMARY_TOKENS, tokens));
        refs.push(format!("{ENTRY}{}", summary.id));
        made = origin::split(Some(&summary.origin));
        payload = said;
        values = stored_values(SUMMARY_KIND, &summary.data);
    }
    let mut out = vec![Sourced {
        block: ContextBlock {
            protected: true,
            cache: true,
            source_refs: refs,
            payload: Some(payload),
            values,
            ..ContextBlock::new(format!("Layer 5 — conversation:\n{header}"), "chat.history")
        },
        origins: made,
    }];
    for e in entries {
        let mut source_refs = vec![e.handle.clone()];
        source_refs.extend(e.refs.iter().cloned());
        out.push(Sourced {
            block: ContextBlock {
                cache: true,
                history: true,
                source_refs,
                payload: Some(e.payload.clone()),
                values: e.values.clone(),
                ..ContextBlock::new(e.text.clone(), e.reference.clone())
            },
            origins: e.origins.clone(),
        });
    }
    out
}

/// The newest shown result's id among `entries` (`_conversation_blocks`' third value).
pub fn latest_result(entries: &[Entry]) -> String {
    entries
        .iter()
        .rev()
        .find(|e| !e.result.is_empty())
        .map(|e| e.result.clone())
        .unwrap_or_default()
}
