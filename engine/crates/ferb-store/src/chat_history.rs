//! The chat as FERB.app shows it (`chat_history`, decisions: APP-805, DMN-555, DMN-556). A port
//! of `ferb/store/chat_history.py`: each entry in the app's own shape, written by the app as it
//! changes, read back a page at a time. An entry's body is the app's, kept as it came
//! (redacted); only the columns the pages need are read.

use std::collections::BTreeSet;

use rusqlite::{params, params_from_iter, OptionalExtension};
use serde_json::{json, Map, Value};

use crate::{
    pyjson,
    pytime::{DateTime, Timespec},
    pyval::{get, loads, marks, merged, py_str, query_maps},
    unicode_nd, Db, Error, Redact, Result,
};

/// `KINDS` (`chat_history.py:14`).
pub const KINDS: [&str; 3] = ["turn", "reply", "notice"];
/// A day's first page and each earlier one (`chat_history.py:17`; decision: DMN-555).
pub const PAGE: i64 = 30;
pub const PAGE_MAX: i64 = 200;
/// Entries one call may write (`chat_history.py:20`; decision: DMN-555).
pub const WRITE_MAX: usize = 500;
pub const REF_MAX: usize = 200;

fn refused<T>(message: &str) -> Result<T> {
    Err(Error::Refused(message.to_owned()))
}

/// `_DAY.fullmatch(day)` (`chat_history.py:22`): `\d{4}-\d{2}-\d{2}`, where Python's `\d` is
/// any Unicode decimal digit.
fn is_day(day: &str) -> bool {
    let chars: Vec<char> = day.chars().collect();
    chars.len() == 10
        && chars.iter().enumerate().all(|(i, c)| match i {
            4 | 7 => *c == '-',
            _ => unicode_nd::is_decimal(*c),
        })
}

/// `_at` (`chat_history.py:25-33`): an entry's time as UTC ISO 8601 with microseconds, so text
/// order is time order. Refused without an offset.
fn at_utc(value: Option<&Value>) -> Result<String> {
    const WHY: &str = "at must be an ISO 8601 time with its offset";
    let text = py_str(value.unwrap_or(&Value::Null)).replace('Z', "+00:00");
    let Ok(when) = DateTime::from_isoformat(&text) else {
        return refused(WHY);
    };
    if when.offset.is_none() {
        return refused(WHY);
    }
    let utc = when.to_utc().map_err(|_| Error::Overflow)?;
    Ok(utc.isoformat(Timespec::Microseconds))
}

/// `str(uuid.UUID(text))` (Python's `uuid.py`): the canonical lowercase form, or `None` where
/// Python raises `ValueError`. Like Python it takes `urn:`/`uuid:` prefixes, braces, and
/// whatever `int(hex, 16)` takes in 32 characters (a `0x` prefix, a `+`, single underscores,
/// surrounding whitespace). Not mirrored: non-ASCII digits, which `int()` also reads.
fn uuid_text(text: &str) -> Option<String> {
    let hex = text.replace("urn:", "").replace("uuid:", "");
    let hex = hex.trim_matches(|c| c == '{' || c == '}').replace('-', "");
    if hex.chars().count() != 32 {
        return None;
    }
    let mut rest = hex.trim_matches(crate::pyval::is_py_space);
    rest = rest.strip_prefix('+').unwrap_or(rest);
    let prefixed = rest.strip_prefix("0x").or_else(|| rest.strip_prefix("0X"));
    let digits = match prefixed {
        Some(after) => after.strip_prefix('_').unwrap_or(after),
        None => rest,
    };
    if digits.is_empty() || digits.starts_with('_') || digits.ends_with('_') {
        return None;
    }
    let mut value: u128 = 0;
    let mut last_underscore = false;
    for c in digits.chars() {
        if c == '_' {
            if last_underscore {
                return None;
            }
            last_underscore = true;
            continue;
        }
        last_underscore = false;
        if !c.is_ascii() {
            return None;
        }
        value = (value << 4) | u128::from(c.to_digit(16)?);
    }
    let h = format!("{value:032x}");
    Some(format!(
        "{}-{}-{}-{}-{}",
        &h[..8],
        &h[8..12],
        &h[12..16],
        &h[16..20],
        &h[20..]
    ))
}

struct Checked {
    id: String,
    kind: String,
    at: String,
    day: String,
    open: i64,
    reference: Option<String>,
    body: Value,
}

/// `_checked` (`chat_history.py:36-59`): one entry as sent, checked: its id a UUID, a known
/// kind, its time, its day, a body object.
fn checked(entry: &Value) -> Result<Checked> {
    let Some(map) = entry.as_object() else {
        return refused("each entry must be an object");
    };
    let Some(id) = uuid_text(&py_str(map.get("id").unwrap_or(&Value::Null))) else {
        return refused("an entry's id must be a UUID");
    };
    let kind = match map.get("kind").and_then(Value::as_str) {
        Some(kind) if KINDS.contains(&kind) => kind.to_owned(),
        _ => return refused("an entry's kind must be turn, reply or notice"),
    };
    let day = match map.get("day").and_then(Value::as_str) {
        Some(day) if is_day(day) => day.to_owned(),
        _ => return refused("an entry's day must be YYYY-MM-DD"),
    };
    let body = match map.get("body") {
        Some(body) if body.is_object() => body.clone(),
        _ => return refused("an entry's body must be an object"),
    };
    let reference = match map.get("ref") {
        None | Some(Value::Null) => None,
        Some(Value::String(text)) if text.chars().count() <= REF_MAX => {
            Some(text.clone()).filter(|t| !t.is_empty())
        }
        Some(_) => return refused("an entry's ref must be a short text"),
    };
    let open = match map.get("open") {
        None => false,
        Some(Value::Bool(open)) => *open,
        Some(_) => return refused("an entry's open must be true or false"),
    };
    Ok(Checked {
        id: id.to_uppercase(),
        kind,
        at: at_utc(map.get("at"))?,
        day,
        open: i64::from(open),
        reference,
        body,
    })
}

/// `cursor` (`chat_history.py:62-63`).
fn cursor(row: &Map<String, Value>) -> String {
    format!(
        "{}|{}",
        row["at"].as_str().unwrap_or(""),
        row["id"].as_str().unwrap_or("")
    )
}

/// `ChatHistory._out` (`chat_history.py:108-111`).
fn out(row: &Map<String, Value>) -> Result<Value> {
    let mut entry = Map::new();
    for key in ["id", "kind", "at", "day"] {
        entry.insert(key.to_owned(), row[key].clone());
    }
    entry.insert("open".to_owned(), json!(row["open"].as_i64() != Some(0)));
    if row["ref"].as_str().is_some_and(|r| !r.is_empty()) {
        entry.insert("ref".to_owned(), row["ref"].clone());
    }
    entry.insert(
        "body".to_owned(),
        loads(row["body_json"].as_str().unwrap_or(""))?,
    );
    Ok(Value::Object(entry))
}

/// Python's `ChatHistory` (`chat_history.py:66`).
pub struct ChatHistory<'a> {
    db: &'a Db,
    redact: Redact,
    /// The time a write is stamped with (`updated_at`), as the daemon's clock gives it.
    now: Box<dyn Fn() -> String>,
}

impl<'a> ChatHistory<'a> {
    pub fn new(db: &'a Db, redact: Redact, now: Box<dyn Fn() -> String>) -> Self {
        ChatHistory { db, redact, now }
    }

    /// `_notice_text` (`chat_history.py:72-80`): a notice FERB recorded (attention_notices) says
    /// what that record says now, whatever copy the app still holds (rule 7).
    fn notice_text(&self, reference: Option<&str>, body: Value) -> Result<Value> {
        let Some(reference) = reference.filter(|r| !r.is_empty()) else {
            return Ok(body);
        };
        let row: Option<(Option<String>, Option<String>)> = self
            .db
            .conn()
            .query_row(
                "SELECT text, quotes_json FROM attention_notices WHERE id = ?",
                [reference],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        let Some((text, quotes)) = row else {
            return Ok(body);
        };
        let quotes = loads(quotes.as_deref().filter(|q| !q.is_empty()).unwrap_or("[]"))?;
        let mut now = Map::new();
        now.insert("text".to_owned(), text.map_or(Value::Null, Value::String));
        now.insert("quotes".to_owned(), quotes);
        let empty = Map::new();
        Ok(Value::Object(merged(
            body.as_object().unwrap_or(&empty),
            &now,
        )))
    }

    /// `write` (`chat_history.py:82-106`): entries the app sent, redacted. `replace`: a changed
    /// entry (`chat.history.put`) replaces the one kept; otherwise (`chat.history.import`) one
    /// already kept stays as it is. Returns `{"changed", "held"}`: how many rows changed and how
    /// many of the given ids are now kept. Refused whole when anything isn't an entry.
    pub fn write(&self, entries: &Value, replace: bool) -> Result<Value> {
        let list = match entries.as_array() {
            Some(list) if list.len() <= WRITE_MAX => list,
            _ => return refused(&format!("entries must be a list of at most {WRITE_MAX}")),
        };
        let checked: Vec<Checked> = list.iter().map(checked).collect::<Result<_>>()?;
        let verb = if replace {
            "INSERT OR REPLACE"
        } else {
            "INSERT OR IGNORE"
        };
        let now = (self.now)();
        let changed = self.db.transaction(|conn| -> Result<usize> {
            let mut changed = 0;
            for e in &checked {
                let mut body = (self.redact)(&e.body);
                if e.kind == "notice" {
                    body = self.notice_text(e.reference.as_deref(), body)?;
                }
                changed += conn.execute(
                    &format!(
                        "{verb} INTO chat_history (id, kind, at, day, open, ref, body_json, \
                         updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?)"
                    ),
                    params![
                        e.id,
                        e.kind,
                        e.at,
                        e.day,
                        e.open,
                        e.reference,
                        pyjson::dumps_utf8(&body),
                        now
                    ],
                )?;
            }
            Ok(changed)
        })?;
        let ids: Vec<&String> = checked.iter().map(|e| &e.id).collect();
        let mut held = 0i64;
        for chunk in ids.chunks(500) {
            held += self.db.conn().query_row(
                &format!(
                    "SELECT COUNT(*) FROM chat_history WHERE id IN ({})",
                    marks(chunk.len())
                ),
                params_from_iter(chunk),
                |r| r.get::<_, i64>(0),
            )?;
        }
        Ok(json!({"changed": changed, "held": held}))
    }

    /// `page` (`chat_history.py:113-136`): a day's newest `limit` entries before `before` (a
    /// page's `cursor`), oldest first; `more` when earlier ones exist; `context`: the turns a
    /// reply on the page answers that aren't on it.
    pub fn page(&self, day: &str, before: Option<&str>, limit: i64) -> Result<Value> {
        if !is_day(day) {
            return refused("day must be YYYY-MM-DD");
        }
        if !(1..=PAGE_MAX).contains(&limit) {
            return refused(&format!(
                "limit must be a whole number from 1 to {PAGE_MAX}"
            ));
        }
        let mut where_ = "day = ?".to_owned();
        let mut args: Vec<Value> = vec![json!(day)];
        if let Some(before) = before.filter(|b| !b.is_empty()) {
            let (at, ident) = before.split_once('|').unwrap_or((before, ""));
            where_ += " AND (at < ? OR (at = ? AND id < ?))";
            args.extend([json!(at), json!(at), json!(ident)]);
        }
        args.push(json!(limit + 1));
        let mut rows = query_maps(
            self.db.conn(),
            &format!("SELECT * FROM chat_history WHERE {where_} ORDER BY at DESC, id DESC LIMIT ?"),
            params_from_iter(args.iter().map(|v| match v {
                Value::String(s) => rusqlite::types::Value::Text(s.clone()),
                other => rusqlite::types::Value::Integer(other.as_i64().unwrap_or(0)),
            })),
        )?;
        let more = rows.len() as i64 > limit;
        rows.truncate(limit as usize);
        rows.reverse();
        let shown: BTreeSet<&str> = rows.iter().filter_map(|r| r["id"].as_str()).collect();
        let mut wanted = BTreeSet::new();
        for row in rows.iter().filter(|r| r["kind"] == "reply") {
            let body = loads(row["body_json"].as_str().unwrap_or(""))?;
            let turn = get(&body, "turnID")
                .map_or(String::new(), py_str)
                .to_uppercase();
            if !turn.is_empty() && !shown.contains(turn.as_str()) {
                wanted.insert(turn);
            }
        }
        let context = if wanted.is_empty() {
            Vec::new()
        } else {
            query_maps(
                self.db.conn(),
                &format!(
                    "SELECT * FROM chat_history WHERE kind = 'turn' AND id IN ({})",
                    marks(wanted.len())
                ),
                params_from_iter(wanted.iter()),
            )?
        };
        let cursor = match rows.first() {
            Some(first) if more => json!(cursor(first)),
            _ => Value::Null,
        };
        Ok(json!({
            "entries": rows.iter().map(out).collect::<Result<Vec<_>>>()?,
            "more": more,
            "cursor": cursor,
            "context": context.iter().map(out).collect::<Result<Vec<_>>>()?,
        }))
    }

    /// `open` (`chat_history.py:138-141`): every turn still waiting for FERB's final answer,
    /// any day.
    pub fn open(&self) -> Result<Vec<Value>> {
        query_maps(
            self.db.conn(),
            "SELECT * FROM chat_history WHERE open = 1 ORDER BY at, id",
            [],
        )?
        .iter()
        .map(out)
        .collect()
    }

    /// `days` (`chat_history.py:143-145`): each day that has an entry, newest first.
    pub fn days(&self) -> Result<Vec<String>> {
        let mut statement = self
            .db
            .conn()
            .prepare("SELECT DISTINCT day FROM chat_history ORDER BY day DESC")?;
        let days = statement.query_map([], |r| r.get(0))?;
        Ok(days.collect::<rusqlite::Result<_>>()?)
    }
}

/// `clear_notice` (`chat_history.py:149-155`): a notice whose text was cleared (a forget, the
/// typed-text purge) says `text` and quotes nothing in the chat history too. Returns the rows
/// changed.
pub fn clear_notice(db: &Db, reference: &str, text: &str) -> Result<usize> {
    Ok(db.conn().execute(
        "UPDATE chat_history SET body_json = json_set(body_json, '$.text', ?, '$.quotes', json('[]')) \
         WHERE kind = 'notice' AND ref = ?",
        params![text, reference],
    )?)
}
