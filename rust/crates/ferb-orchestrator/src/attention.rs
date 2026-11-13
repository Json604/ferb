//! ferb/orchestrator/attention.py: owner notices (L9): persisted, then posted to chat; their text keeps its outside
//! origin, and quoted outside text stays apart with its own (MP2). `expire_derived` clears a notice's text wherever
//! it was copied (the D14 purge, a forget).
//!
//! (L9, L10, D14 in the notes these rules came from.)

use std::collections::BTreeSet;

use ferb_sanitize::origin;
use ferb_store::spans::rewrite_recorded;
use ferb_store::Db;
use rusqlite::types::Value as Sql;
use serde_json::{json, Map, Value};

use crate::ports::Redact;
use crate::sql::{self, text, Result};

/// attention.py:15 `ORIGIN`.
pub fn notice_origin() -> String {
    origin::join([
        origin::external("observation", "").as_str(),
        origin::INFERRED,
    ])
}

pub const CONTEXT_REMOVED: &str =
    "Removed: this recorded context held text that has expired or that the owner forgot.";
/// lineage.py:12 `ENTRY`.
pub const ENTRY: &str = "entry:";

/// attention.py:18 `_quotes`: non-empty quotes, each labelled by code with `source`.
pub fn quotes(value: &Value, source: &str) -> Vec<Value> {
    let Value::Array(items) = value else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|q| {
            let t = q.get("text")?.as_str()?;
            if crate::py::strip(t).is_empty() {
                return None;
            }
            Some(json!({"text": t, "origin": source}))
        })
        .collect()
}

/// attention.py:27 `AttentionNotices`.
pub struct AttentionNotices<'a> {
    pub db: &'a Db,
    pub now: Box<dyn Fn() -> String + Send + Sync + 'a>,
    pub redact: Redact,
}

impl<'a> AttentionNotices<'a> {
    /// attention.py:31 `add`: the new notice, or None when that id was already made.
    #[allow(clippy::too_many_arguments)]
    pub fn add(
        &self,
        ident: &str,
        refs: &[String],
        text_: &str,
        quoted: &Value,
        considered: &[String],
        quote_origin: Option<&str>,
        made_by: Option<&str>,
        cards: &[Value],
    ) -> Result<Option<Map<String, Value>>> {
        let quote_origin = quote_origin
            .map(String::from)
            .unwrap_or_else(|| origin::external("observation", ""));
        let made_by = made_by.map(String::from).unwrap_or_else(notice_origin);
        let clean = (self.redact)(&json!({"text": text_, "quotes": quotes(quoted, &quote_origin)}));
        let all: BTreeSet<&String> = considered.iter().chain(refs.iter()).collect();
        let cards_clean = (self.redact)(&json!({"cards": cards}));
        let made = sql::exec(
            self.db.conn(),
            "INSERT OR IGNORE INTO attention_notices (id, source_refs_json, text, origin, created_at, delivered_at, \
             quotes_json, considered_refs_json, cards_json) VALUES (?,?,?,?,?,NULL,?,?,?)",
            &[
                text(ident),
                text(&ferb_py::json::dumps(&refs)),
                sql::to_sql(&clean["text"]),
                text(&made_by),
                text(&(self.now)()),
                text(&ferb_py::json::dumps(&clean["quotes"])),
                text(&ferb_py::json::dumps(&all.into_iter().collect::<Vec<_>>())),
                text(&ferb_py::json::dumps(&cards_clean["cards"])),
            ],
        )?;
        if made == 0 {
            return Ok(None);
        }
        sql::row(
            self.db.conn(),
            "SELECT * FROM attention_notices WHERE id=?",
            &[text(ident)],
        )
    }

    /// attention.py:47 `view`.
    pub fn view(row: &Map<String, Value>) -> Result<Value> {
        let cards_text = match row.get("cards_json") {
            Some(Value::String(s)) if !s.is_empty() => s.clone(),
            _ => "[]".into(),
        };
        let cards = sql::loads(&cards_text)?;
        let mut out = Map::new();
        out.insert("id".into(), row["id"].clone());
        out.insert("text".into(), row["text"].clone());
        out.insert("quotes".into(), sql::loads(&sql::col(row, "quotes_json"))?);
        out.insert("at".into(), row["created_at"].clone());
        if ferb_py::value::truthy(&cards) {
            out.insert("cards".into(), cards);
        }
        Ok(Value::Object(out))
    }

    pub fn unseen(&self) -> Result<Vec<Value>> {
        self.list(true)?.iter().map(Self::view).collect()
    }

    /// attention.py:57 `told`: every notice, or those about any of `apps` (by the rows each cited or considered).
    pub fn told(
        &self,
        apps: Option<&BTreeSet<String>>,
        app_of: &dyn Fn(&Value) -> String,
    ) -> Result<Vec<Value>> {
        let mut rows = sql::rows(
            self.db.conn(),
            "SELECT * FROM attention_notices ORDER BY created_at",
            &[],
        )?;
        if let Some(apps) = apps {
            let rows_of = |column: &str| {
                format!(
                    "SELECT n.id, t.data_json FROM attention_notices n, json_each({column}) ref \
                     JOIN trail t ON ref.value LIKE 'trail:%' AND t.id = substr(ref.value, 7)"
                )
            };
            let query = format!(
                "{} UNION {}",
                rows_of("n.source_refs_json"),
                rows_of("COALESCE(n.considered_refs_json, '[]')")
            );
            let mut about: std::collections::HashMap<String, BTreeSet<String>> =
                std::collections::HashMap::new();
            for r in sql::rows(self.db.conn(), &query, &[])? {
                let data = sql::loads(&sql::col(&r, "data_json"))?;
                about
                    .entry(sql::col(&r, "id"))
                    .or_default()
                    .insert(app_of(&data));
            }
            rows.retain(|r| {
                about
                    .get(&sql::col(r, "id"))
                    .is_some_and(|s| s.iter().any(|a| apps.contains(a)))
            });
        }
        rows.iter().map(Self::view).collect()
    }

    /// attention.py:71 `acknowledge`: unknown ids refuse the whole acknowledgement.
    pub fn acknowledge(&self, idents: &[String]) -> Result<Vec<Map<String, Value>>> {
        let mut rows = Vec::new();
        for ident in idents {
            match sql::row(
                self.db.conn(),
                "SELECT * FROM attention_notices WHERE id=?",
                &[text(ident)],
            )? {
                Some(r) => rows.push(r),
                None => return Err(crate::sql::Error::Value("No such owner notice".into())),
            }
        }
        for ident in idents {
            sql::exec(
                self.db.conn(),
                "UPDATE attention_notices SET delivered_at=COALESCE(delivered_at,?) WHERE id=?",
                &[text(&(self.now)()), text(ident)],
            )?;
        }
        Ok(rows)
    }

    /// attention.py:81 `list`.
    pub fn list(&self, pending: bool) -> Result<Vec<Map<String, Value>>> {
        let filter = if pending {
            " WHERE delivered_at IS NULL"
        } else {
            " WHERE delivered_at IS NOT NULL"
        };
        sql::rows(
            self.db.conn(),
            &format!("SELECT * FROM attention_notices{filter} ORDER BY created_at"),
            &[],
        )
    }
}

/// attention.py:156 `_forms`: a told string as a JSON "text" field, with its replacement, in each written form.
pub fn forms(text_: &str, replacement: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for ascii in [true, false] {
        let dump = |s: &str| {
            if ascii {
                ferb_py::json::dumps(&s)
            } else {
                ferb_py::json::dumps_utf8(&s)
            }
        };
        let field = (
            format!("\"text\": {}", dump(text_)),
            format!("\"text\": {}", dump(replacement)),
        );
        let inner = |s: &str| {
            let d = dump(s);
            d[1..d.len() - 1].to_string()
        };
        let quoted = (inner(&field.0), inner(&field.1));
        out.push(field);
        out.push(quoted);
    }
    out
}

/// attention.py:167 `notice_payload`.
pub fn notice_payload(data: &Value) -> String {
    let mut lines = vec![data.get("text").map(crate::py::str_of).unwrap_or_default()];
    if let Some(Value::Array(qs)) = data.get("quotes") {
        for q in qs {
            if let Value::Object(q) = q {
                lines.push(q.get("text").map(crate::py::str_of).unwrap_or_default());
            }
        }
    }
    lines.join("\n")
}

/// attention.py:175 `notice_sources`.
pub fn notice_sources(db: &Db, notice_id: &str) -> Result<Vec<String>> {
    let Some(row) = sql::row(
        db.conn(),
        "SELECT source_refs_json, considered_refs_json FROM attention_notices WHERE id=?",
        &[text(notice_id)],
    )?
    else {
        return Ok(Vec::new());
    };
    let mut all: BTreeSet<String> = BTreeSet::new();
    for (key, default) in [("source_refs_json", ""), ("considered_refs_json", "[]")] {
        let raw = match row.get(key) {
            Some(Value::String(s)) if !s.is_empty() => s.clone(),
            _ => default.into(),
        };
        if let Value::Array(items) = sql::loads(&raw)? {
            all.extend(items.iter().map(crate::py::str_of));
        }
    }
    Ok(all.into_iter().collect())
}

/// attention.py:232 `_without`: every told string replaced, in every string a value holds.
pub fn without(value: &Value, told: &[(String, String)]) -> Value {
    match value {
        Value::Object(m) => Value::Object(
            m.iter()
                .map(|(k, v)| (k.clone(), without(v, told)))
                .collect(),
        ),
        Value::Array(a) => Value::Array(a.iter().map(|v| without(v, told)).collect()),
        Value::String(s) => {
            let mut s = s.clone();
            for (old, new) in told {
                s = s.replace(old.as_str(), new);
            }
            Value::String(s)
        }
        other => other.clone(),
    }
}

fn held_of(row: &Map<String, Value>) -> Option<(String, Option<String>)> {
    Some((
        sql::col(row, "input_json"),
        row.get("output_json")
            .and_then(|v| v.as_str())
            .map(String::from),
    ))
}

/// attention.py:182 `scrub_spans`.
pub fn scrub_spans(db: &Db, told: &[(String, String)], every_kind: bool) -> Result<()> {
    let c = db.conn();
    c.execute_batch("CREATE TEMP TABLE IF NOT EXISTS expiring_text (held TEXT PRIMARY KEY)")?;
    let found = (|| -> Result<Vec<Map<String, Value>>> {
        for (old, _) in told {
            let a = ferb_py::json::dumps(&old);
            let b = ferb_py::json::dumps_utf8(&old);
            for form in [&a[1..a.len() - 1], &b[1..b.len() - 1]] {
                sql::exec(
                    c,
                    "INSERT OR IGNORE INTO expiring_text VALUES (?)",
                    &[text(form)],
                )?;
            }
        }
        sql::rows(
            c,
            "SELECT id,input_json,output_json FROM spans s WHERE (? OR kind!='attention.consider') AND EXISTS (SELECT 1 \
             FROM expiring_text e WHERE instr(s.input_json, e.held) > 0 OR instr(COALESCE(s.output_json, ''), e.held) > 0)",
            &[Sql::Integer(i64::from(every_kind))],
        )
    })();
    c.execute_batch("DELETE FROM expiring_text")?;
    for span in found? {
        rewrite_recorded(
            db,
            &sql::col(&span, "id"),
            |i, o| (without(i, told), without(o, told)),
            None,
            held_of(&span),
        )?;
    }
    Ok(())
}

fn scrub_payloads(db: &Db, entries: &[(String, String)], replacement: &str) -> Result<()> {
    let c = db.conn();
    c.execute_batch("CREATE TEMP TABLE IF NOT EXISTS expiring_entries (ref TEXT PRIMARY KEY)")?;
    let found = (|| -> Result<Vec<Map<String, Value>>> {
        for (r, _) in entries {
            sql::exec(
                c,
                "INSERT OR IGNORE INTO expiring_entries VALUES (?)",
                &[text(r)],
            )?;
        }
        sql::rows(
            c,
            "SELECT DISTINCT s.id, s.input_json, s.output_json FROM spans s, json_each(s.input_json, '$.source_refs') r \
             JOIN expiring_entries e ON r.value = e.ref WHERE s.kind='context.exposure' AND s.output_json IS NOT NULL",
            &[],
        )
    })();
    c.execute_batch("DELETE FROM expiring_entries")?;
    for span in found? {
        rewrite_recorded(
            db,
            &sql::col(&span, "id"),
            |inp, out| {
                let Some(payload) = out.get("payload").and_then(|p| p.as_str()) else {
                    return (inp.clone(), out.clone());
                };
                let shown: BTreeSet<String> = inp
                    .get("source_refs")
                    .and_then(|v| v.as_array())
                    .map(|a| a.iter().map(crate::py::str_of).collect())
                    .unwrap_or_default();
                let mut new = payload.to_string();
                for (r, t) in entries {
                    if shown.contains(r) && !t.is_empty() {
                        new = new.replace(t.as_str(), replacement);
                    }
                }
                if new != payload {
                    let mut o = out.as_object().cloned().unwrap_or_default();
                    o.insert("payload".into(), Value::String(new));
                    (inp.clone(), Value::Object(o))
                } else {
                    (inp.clone(), out.clone())
                }
            },
            None,
            held_of(&span),
        )?;
    }
    Ok(())
}

/// attention.py:90 `expire_derived`: clear the text of every notice derived from these rows, wherever it was
/// copied. Runs inside the caller's transaction when there is one. Returns how many notices it cleared.
pub fn expire_derived(db: &Db, refs: &[String], replacement: &str, now: &str) -> Result<usize> {
    let c = db.conn();
    let refs: BTreeSet<&str> = refs.iter().map(String::as_str).collect();
    let mut cleared = 0;
    let mut said: Vec<String> = Vec::new();
    let mut entries: Vec<(String, String)> = Vec::new();
    for row in sql::rows(c, "SELECT * FROM attention_notices", &[])? {
        let cited: Vec<String> = match sql::loads(&sql::col(&row, "source_refs_json"))? {
            Value::Array(a) => a.iter().map(crate::py::str_of).collect(),
            _ => Vec::new(),
        };
        let considered: Vec<String> = match sql::loads(&sql::col(&row, "considered_refs_json"))? {
            Value::Array(a) => a.iter().map(crate::py::str_of).collect(),
            _ => Vec::new(),
        };
        if !cited
            .iter()
            .chain(considered.iter())
            .any(|r| refs.contains(r.as_str()))
        {
            continue;
        }
        let held = vec![
            sql::to_sql(&row["text"]),
            sql::to_sql(&row["quotes_json"]),
            sql::to_sql(&row["delivered_at"]),
        ];
        let mut replaced: Option<Vec<Sql>> = None;
        let written = ferb_store::swap::swap(
            c,
            "attention_notices",
            "id",
            &sql::to_sql(&row["id"]),
            &["text", "quotes_json", "delivered_at"],
            |h| {
                if matches!(&h[0], Sql::Text(t) if t == replacement) {
                    return Ok(h.clone());
                }
                replaced = Some(h.clone());
                let delivered = match &h[2] {
                    Sql::Null => text(now),
                    Sql::Text(t) if t.is_empty() => text(now),
                    other => other.clone(),
                };
                Ok(vec![text(replacement), text("[]"), delivered])
            },
            Some(held),
        )?;
        if written.is_none() {
            continue;
        }
        let replaced = replaced.expect("a write replaced what it held");
        let old_text = match &replaced[0] {
            Sql::Text(t) => t.clone(),
            _ => String::new(),
        };
        let old_quotes = match &replaced[1] {
            Sql::Text(t) => sql::loads(t)?,
            _ => json!([]),
        };
        said.push(old_text.clone());
        if let Value::Array(qs) = &old_quotes {
            said.extend(qs.iter().map(|q| {
                q.get("text")
                    .map(crate::py::str_of)
                    .unwrap_or_else(|| "None".into())
            }));
        }
        let id = sql::col(&row, "id");
        for entry in sql::rows(
            c,
            "SELECT id,data_json FROM journal WHERE kind='chat.notice' AND json_extract(data_json,'$.id')=?",
            &[text(&id)],
        )? {
            let key = format!("{ENTRY}{}", sql::col(&entry, "id"));
            let payload = notice_payload(&sql::loads(&sql::col(&entry, "data_json"))?);
            match entries.iter_mut().find(|(k, _)| *k == key) {
                Some(slot) => slot.1 = payload,
                None => entries.push((key, payload)),
            }
        }
        sql::exec(
            c,
            "DELETE FROM journal WHERE kind='chat.notice' AND json_extract(data_json,'$.id')=?",
            &[text(&id)],
        )?;
        ferb_store::chat_history::clear_notice(db, &id, replacement)?;
        for r in &cited {
            sql::exec(
                c,
                "UPDATE notification_told SET text=? WHERE event_id=? AND text=?",
                &[
                    text(replacement),
                    text(r.strip_prefix("trail:").unwrap_or(r)),
                    text(&old_text),
                ],
            )?;
        }
        cleared += 1;
    }
    // {form: (form, new) ...}: the first place of each form, its last pair
    let mut told: Vec<(String, String)> = Vec::new();
    for t in said.iter().filter(|t| !t.is_empty()) {
        for (form, new) in forms(t, replacement) {
            match told.iter_mut().find(|(f, _)| *f == form) {
                Some(slot) => slot.1 = new,
                None => told.push((form, new)),
            }
        }
    }
    if !told.is_empty() {
        scrub_spans(db, &told, false)?;
    }
    if !entries.is_empty() {
        scrub_payloads(db, &entries, replacement)?;
    }
    for span in sql::rows(
        c,
        "SELECT id,input_json,output_json FROM spans WHERE kind='attention.consider'",
        &[],
    )? {
        rewrite_recorded(
            db,
            &sql::col(&span, "id"),
            |inp, out| {
                let sources: Vec<String> = inp
                    .get("source_refs")
                    .and_then(|v| v.as_array())
                    .map(|a| a.iter().map(crate::py::str_of).collect())
                    .unwrap_or_default();
                if sources.iter().any(|s| refs.contains(s.as_str())) {
                    let mut new_in = inp.as_object().cloned().unwrap_or_default();
                    if new_in.contains_key("context") {
                        new_in.insert("context".into(), Value::String(CONTEXT_REMOVED.into()));
                    }
                    let new_out = match out {
                        Value::Object(o)
                            if o.get("text").is_some_and(ferb_py::value::truthy)
                                || o.get("quotes").is_some_and(ferb_py::value::truthy) =>
                        {
                            let mut o = o.clone();
                            o.insert("text".into(), Value::String(replacement.into()));
                            o.insert("quotes".into(), json!([]));
                            Value::Object(o)
                        }
                        other => other.clone(),
                    };
                    return (Value::Object(new_in), new_out);
                }
                let new_in = match inp {
                    Value::Object(m) => Value::Object(
                        m.iter()
                            .map(|(k, v)| (k.clone(), without(v, &told)))
                            .collect(),
                    ),
                    other => other.clone(),
                };
                (new_in, out.clone())
            },
            None,
            held_of(&span),
        )?;
    }
    Ok(cleared)
}
