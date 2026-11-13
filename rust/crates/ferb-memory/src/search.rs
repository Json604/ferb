//! The one fused memory search (`search.py`, MEM-6, MEM-27): facts from `memory_items`, learned nodes and old
//! chat, each ranked by bm25 over the words that carry weight, the task's focus words alone, and the local
//! embedding model's nearest, fused by reciprocal rank plus a small recency nudge. The pure parts here build for
//! any target; the store-backed [`MemorySearch`] needs the `store` feature (Mac-side).

use crate::pystr;
use crate::pyval::{self, Crash};
use crate::terms;
use crate::times::{self, DateTime};

/// Learned node kinds search serves.
pub const NODE_KINDS: [&str; 4] = [
    "episode",
    "playbook",
    "activity_summary",
    "episode_annotation",
];
/// The conversation kinds the chat index holds.
pub const CHAT_KINDS: [&str; 5] = [
    "chat.message",
    "chat.reply",
    "chat.clarify",
    "task.answer",
    "chat.notice",
];
/// Cosine below this is not "about the same thing".
pub const MIN_SIMILARITY: f64 = 0.2;
/// How many nearest vectors a ranking takes per kind.
pub const NEAREST: usize = 100;
/// How many keyword matches a ranking takes.
pub const KEYWORD_MOST: usize = 1000;
pub const RECENCY_WEIGHT: f64 = 0.05;
pub const RECENCY_HALF_LIFE_DAYS: f64 = 30.0;
/// Vectors written per transaction by the background embedder.
pub const EMBED_BATCH: usize = 64;
/// Journal rows read per step of `index_chat`.
pub const CHAT_INDEX_BATCH: usize = 500;
/// As FERB's context counts memory text (do not replace with a real token count).
pub const CHARS_PER_TOKEN: usize = 4;

/// What one role is given of retrieved memory each time (MEM-27).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    pub memory_tokens: usize,
    pub memory_items: usize,
    pub activity_tokens: usize,
}

/// The background attention check's memory.
pub const ATTENTION: Limits = Limits {
    memory_tokens: 4000,
    memory_items: 20,
    activity_tokens: 2000,
};
/// Workers and plan steps.
pub const WORKER: Limits = Limits {
    memory_tokens: 2000,
    memory_items: 10,
    activity_tokens: 2000,
};

/// `tokens`: 4 characters a token, rounded up.
pub fn tokens(text: &str) -> usize {
    pystr::char_len(text).div_ceil(CHARS_PER_TOKEN)
}

/// `_tied`: rank per identifier, equal scores sharing a rank (dense, or the first position when `positional`).
pub fn tied<'a>(rows: impl IntoIterator<Item = (&'a str, f64)>, positional: bool) -> Ranking {
    let mut out = Ranking::default();
    let mut rank: i64 = -1;
    let mut last: Option<f64> = None;
    for (position, (ident, score)) in rows.into_iter().enumerate() {
        if last != Some(score) {
            rank = if positional {
                position as i64
            } else {
                rank + 1
            };
            last = Some(score);
        }
        out.set_default(ident, rank as usize);
    }
    out
}

/// An insertion-ordered map from identifier to rank (Python's dict).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Ranking(
    pub Vec<(String, usize)>,
    std::collections::HashMap<String, usize>,
);

impl Ranking {
    fn set_default(&mut self, ident: &str, rank: usize) {
        if !self.1.contains_key(ident) {
            self.1.insert(ident.to_owned(), self.0.len());
            self.0.push((ident.to_owned(), rank));
        }
    }

    /// `d[ident] = rank` (a key already there keeps its place).
    pub fn set(&mut self, ident: &str, rank: usize) {
        match self.1.get(ident) {
            Some(&at) => self.0[at].1 = rank,
            None => self.set_default(ident, rank),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, usize)> {
        self.0.iter().map(|(k, v)| (k.as_str(), *v))
    }
}

/// `_match`: each term quoted for FTS5, joined by OR.
pub fn match_expr(terms: &[String]) -> String {
    terms
        .iter()
        .map(|t| format!("\"{}\"", t.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(" OR ")
}

/// `_age_days`: days from `at` to `now` (0 when `at` is not a time, never negative); a naive `at` with an aware
/// `now` (or the other way) takes `now`'s zone.
pub fn age_days(at: &str, now: &DateTime) -> f64 {
    let Some(mut when) = times::parse_iso(at) else {
        return 0.0;
    };
    if when.offset.is_none() != now.offset.is_none() {
        when.offset = now.offset;
    }
    let micros = times::instant(now) - times::instant(&when);
    let days = times::total_seconds(micros) / 86_400.0;
    if days > 0.0 {
        days
    } else {
        0.0
    }
}

/// `_recency`: the small nudge for newer items (the owner's words don't age).
pub fn recency(at: &str, now: &DateTime, owner: bool) -> f64 {
    let age = if owner { 0.0 } else { age_days(at, now) };
    RECENCY_WEIGHT * 0.5f64.powf(age / RECENCY_HALF_LIFE_DAYS)
}

/// `fused`: reciprocal-rank fusion, insertion-ordered.
pub fn fused(rankings: &[Ranking]) -> Vec<(String, f64)> {
    let mut out: Vec<(String, f64)> = Vec::new();
    let mut at: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for ranking in rankings {
        for (ident, n) in ranking.iter() {
            let add = 1.0 / (n as f64 + 1.0);
            match at.get(ident) {
                Some(&i) => out[i].1 += add,
                None => {
                    at.insert(ident.to_owned(), out.len());
                    out.push((ident.to_owned(), 0.0 + add));
                }
            }
        }
    }
    out
}

/// `own_words`: a message without the parts the owner pasted (`<untrusted-data id="X">…</untrusted-data
/// id="X">`, each replaced by a space).
pub fn own_words(text: &str) -> String {
    const OPEN: &str = "<untrusted-data id=\"";
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < text.len() {
        let Some(found) = text[i..].find(OPEN) else {
            break;
        };
        let start = i + found;
        let id_start = start + OPEN.len();
        let block = text[id_start..].find('"').and_then(|q| {
            let id_end = id_start + q;
            if q == 0 || !text[id_end..].starts_with("\">") {
                return None;
            }
            let id = &text[id_start..id_end];
            let close = format!("</untrusted-data id=\"{id}\">");
            let body = id_end + 2;
            text[body..].find(&close).map(|c| body + c + close.len())
        });
        match block {
            Some(end) => {
                out.push_str(&text[i..start]);
                out.push(' ');
                i = end;
            }
            None => {
                let step = start + 1;
                out.push_str(&text[i..step]);
                i = step;
            }
        }
    }
    out.push_str(&text[i..]);
    out
}

/// `chat_text`: what the conversation index holds of an entry. `Crash` where Python would raise (a `data` that
/// isn't a dict, `quotes` that can't be iterated).
pub fn chat_text(kind: &str, data: &serde_json::Value) -> Result<String, Crash> {
    let text = pyval::str_or_empty(pyval::get(data, "text")?);
    if kind == "chat.message" {
        return Ok(pystr::squeeze(&own_words(&text)));
    }
    if kind == "chat.notice" {
        let mut parts = vec![text];
        for q in pyval::iter_or_empty(pyval::get(data, "quotes")?)? {
            parts.push(match &q {
                serde_json::Value::Object(m) => m.get("text").map_or(String::new(), pyval::to_str),
                other => pyval::to_str(other),
            });
        }
        return Ok(pystr::squeeze(&parts.join(" ")));
    }
    Ok(pystr::squeeze(&text))
}

/// `short`: the first paragraph, on one line, clipped to `chars` with an ellipsis.
pub fn short(text: &str, chars: usize) -> String {
    let stripped = pystr::strip(text);
    let first = stripped.split_once("\n\n").map_or(stripped, |(a, _)| a);
    let first = pystr::squeeze(pystr::strip(first));
    if pystr::char_len(&first) <= chars {
        first
    } else {
        format!(
            "{}…",
            pystr::rstrip(pystr::prefix(&first, chars.saturating_sub(1)))
        )
    }
}

/// `fact_text`: what a fact's embedding is made of.
pub fn fact_text(content: &str, about: &str, key: &str, subject: &str) -> String {
    let words = |slug: &str| -> String {
        let tail = slug.split_once(':').map_or(slug, |(_, rest)| rest);
        terms::query_terms(tail).join(" ")
    };
    let mut parts = vec![content.to_owned(), about.to_owned(), words(key)];
    if subject != "owner" {
        parts.push(words(subject));
    }
    parts
        .iter()
        .filter(|p| !p.is_empty() && !pystr::strip(p).is_empty())
        .map(|p| pystr::squeeze(p))
        .collect::<Vec<_>>()
        .join(" ")
}

/// `within`: the leading lines that fit `budget_tokens` and `most` (a line never cut; one that doesn't fit is
/// skipped and later shorter ones may still fit), and how many were left out.
pub fn within(lines: &[String], budget_tokens: usize, most: usize) -> (Vec<String>, usize) {
    let mut shown = Vec::new();
    let mut used = 0;
    for line in lines {
        let cost = tokens(line) + 1;
        if shown.len() >= most || used + cost > budget_tokens {
            continue;
        }
        shown.push(line.clone());
        used += cost;
    }
    let left = lines.len() - shown.len();
    (shown, left)
}

/// A search result.
#[derive(Clone, Debug, PartialEq)]
pub struct Hit<T> {
    /// "fact", a learned node kind, or "chat".
    pub kind: String,
    /// `memory:<id>`, a node id, or `journal:<id>`.
    pub doc: String,
    pub score: f64,
    pub at: String,
    /// Rests on the owner's own words.
    pub owner: bool,
    pub data: T,
}

/// A row of the conversation index.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChatRow {
    pub doc: String,
    pub kind: String,
    pub session: String,
    pub at: String,
    pub request: String,
    pub text: String,
}

#[cfg(feature = "store")]
fn round9(x: f64) -> f64 {
    crate::pymath::round(x, 9)
}

#[cfg(feature = "store")]
pub use store::*;

#[cfg(feature = "store")]
mod store {
    use std::collections::{HashMap, HashSet};

    use rusqlite::{params_from_iter, types::Value as Sql, Connection, OptionalExtension};
    use serde_json::Value;

    use super::*;
    use crate::embeddings::Embedder;
    use crate::pymath;
    use crate::terms::{Fact, SQL_SLACK_MICROS};
    use crate::vectors;
    use crate::Error;

    /// The clock search reads `now` from.
    pub trait Clock {
        fn now(&self) -> DateTime;
    }

    /// Where search reads a fact (the items part's `Memory.get`; [`StoreFacts`] reads the store directly).
    pub trait FactLookup {
        fn get(&self, id: &str) -> Result<Option<Fact>, Error>;
    }

    /// The fact view straight from `memory_items` and `memory_set_aside` (`Memory._item`'s fields search reads).
    pub struct StoreFacts<'a>(pub &'a Connection);

    impl FactLookup for StoreFacts<'_> {
        fn get(&self, id: &str) -> Result<Option<Fact>, Error> {
            let row = self
                .0
                .query_row(
                    "SELECT id, kind, subject, key, content, origin, status, valid_from, valid_to FROM memory_items \
                     WHERE id = ?",
                    [id],
                    |r| {
                        Ok((
                            r.get::<_, String>(0)?,
                            r.get::<_, String>(1)?,
                            r.get::<_, String>(2)?,
                            r.get::<_, String>(3)?,
                            r.get::<_, String>(4)?,
                            r.get::<_, String>(5)?,
                            r.get::<_, String>(6)?,
                            r.get::<_, String>(7)?,
                            r.get::<_, Option<String>>(8)?,
                        ))
                    },
                )
                .optional()?;
            let Some((id, kind, subject, key, content, origin, status, from, to)) = row else {
                return Ok(None);
            };
            let aside = self
                .0
                .query_row(
                    "SELECT 1 FROM memory_set_aside WHERE item_id = ?",
                    [&id],
                    |_| Ok(()),
                )
                .optional()?
                .is_some();
            let parse = |s: &str| times::parse_iso(s).ok_or(Crash("ValueError"));
            Ok(Some(Fact {
                id,
                kind,
                subject,
                key,
                content,
                origin,
                status,
                valid_from: parse(&from)?,
                valid_to: match to.as_deref() {
                    Some(t) if !t.is_empty() => Some(parse(t)?),
                    _ => None,
                },
                set_aside: aside,
            }))
        }
    }

    /// `learning.search_text(kind, key, body)`: what a learned node is matched and embedded by (the learning
    /// part's; a seam until it is ported).
    pub type NodeText<'a> = &'a dyn Fn(&str, &str, &Value) -> String;

    /// What `embed_pending` did.
    #[derive(Clone, Debug, PartialEq)]
    pub struct Embedded {
        pub embedded: usize,
        pub batches: usize,
        pub encode_cpu_s: f64,
        pub removed: usize,
    }

    /// One thing waiting for a vector.
    struct Pending {
        doc: String,
        kind: String,
        at: String,
        text: String,
        chat_rowid: Option<i64>,
    }

    fn sql_text(v: Sql) -> String {
        match v {
            Sql::Null => "None".into(),
            Sql::Integer(i) => i.to_string(),
            Sql::Real(f) => ferb_py::json::python_float(f),
            Sql::Text(t) => t,
            Sql::Blob(b) => String::from_utf8_lossy(&b).into_owned(),
        }
    }

    fn sql_int(v: Sql) -> Result<i64, Error> {
        match v {
            Sql::Integer(i) => Ok(i),
            Sql::Text(t) => Ok(pyval::int(&Value::String(t))?),
            Sql::Real(f) => Ok(f.trunc() as i64),
            _ => Err(Crash("TypeError").into()),
        }
    }

    fn slack(now: &DateTime) -> Result<String, Error> {
        let at = times::plus(now, -SQL_SLACK_MICROS).ok_or(Crash("OverflowError"))?;
        Ok(times::iso(&at))
    }

    const CURRENT_FACTS: &str =
        "m.status IN ('active','proposed') AND (m.valid_to IS NULL OR m.valid_to > ?) \
                                 AND m.id NOT IN (SELECT item_id FROM memory_set_aside)";

    /// The search over one database connection.
    pub struct MemorySearch<'a> {
        pub conn: &'a Connection,
        pub clock: &'a dyn Clock,
        /// The embedding model whose vectors to rank by (`None`: keywords and recency only).
        pub model: Option<String>,
    }

    impl<'a> MemorySearch<'a> {
        pub fn new(conn: &'a Connection, clock: &'a dyn Clock, model: Option<String>) -> Self {
            MemorySearch { conn, clock, model }
        }

        fn count(&self, sql: &str, params: &[&dyn rusqlite::ToSql]) -> Result<i64, Error> {
            Ok(self.conn.query_row(sql, params, |r| r.get::<_, i64>(0))?)
        }

        fn fact_terms(&self, words: &[String], now: &DateTime) -> Result<Vec<String>, Error> {
            let slack = slack(now)?;
            let total = self.count(
                &format!("SELECT COUNT(*) FROM memory_items m WHERE {CURRENT_FACTS}"),
                &[&slack],
            )?;
            terms::weighted(words, total, |term| {
                self.count(
                    &format!(
                        "SELECT COUNT(*) FROM memory_fts f JOIN memory_items m ON m.seq = f.rowid WHERE \
                         memory_fts MATCH ? AND {CURRENT_FACTS}"
                    ),
                    &[&match_expr(&[term.to_owned()]), &slack],
                )
            })
        }

        fn fact_bm25(
            &self,
            terms: &[String],
            now: &DateTime,
            positional: bool,
        ) -> Result<Ranking, Error> {
            let mut stmt = self.conn.prepare(&format!(
                "SELECT m.id, bm25(memory_fts) FROM memory_fts JOIN memory_items m ON m.seq = memory_fts.rowid \
                 WHERE memory_fts MATCH ? AND {CURRENT_FACTS} ORDER BY 2 LIMIT {KEYWORD_MOST}"
            ))?;
            let rows: Vec<(String, f64)> = stmt
                .query_map(rusqlite::params![match_expr(terms), slack(now)?], |r| {
                    Ok((sql_text(r.get(0)?), r.get::<_, f64>(1)?))
                })?
                .collect::<Result<_, _>>()?;
            Ok(tied(rows.iter().map(|(a, b)| (a.as_str(), *b)), positional))
        }

        fn vector_ranks(
            &self,
            vector: Option<&[i8]>,
            kinds: &[&str],
            prefix: &str,
        ) -> Result<Ranking, Error> {
            let (Some(vector), Some(model)) = (vector, self.model.as_deref()) else {
                return Ok(Ranking::default());
            };
            let mut out = Ranking::default();
            let near = vectors::nearest(self.conn, model, vector, kinds, NEAREST)?;
            for (n, (doc, _)) in near
                .into_iter()
                .filter(|(_, s)| *s >= MIN_SIMILARITY)
                .enumerate()
            {
                out.set(doc.strip_prefix(prefix).unwrap_or(&doc), n);
            }
            Ok(out)
        }

        /// `facts`: the facts that may shape prompts, best first. `leave_out`: ids not to return (dropped before
        /// `limit`).
        pub fn facts(
            &self,
            memory: &dyn FactLookup,
            query: &str,
            vector: Option<&[i8]>,
            focus: &str,
            limit: Option<usize>,
            leave_out: &[String],
        ) -> Result<Vec<Hit<Fact>>, Error> {
            let now = self.clock.now();
            let words = terms::query_terms(query);
            let terms = if words.is_empty() {
                Vec::new()
            } else {
                self.fact_terms(&words, &now)?
            };
            if !words.is_empty() && terms.is_empty() {
                return Ok(Vec::new());
            }
            let mut rankings = Vec::new();
            if !terms.is_empty() {
                rankings.push(self.fact_bm25(&terms, &now, false)?);
                let focused = if focus.is_empty() {
                    Vec::new()
                } else {
                    self.fact_terms(&terms::query_terms(focus), &now)?
                };
                if !focused.is_empty() {
                    rankings.push(self.fact_bm25(&focused, &now, true)?);
                }
                let near = self.vector_ranks(vector, &["fact"], "memory:")?;
                if !near.is_empty() {
                    rankings.push(near);
                }
            } else {
                let mut stmt = self.conn.prepare(&format!(
                    "SELECT m.id FROM memory_items m WHERE {CURRENT_FACTS} ORDER BY m.valid_from DESC, m.seq DESC"
                ))?;
                let ids: Vec<String> = stmt
                    .query_map([slack(&now)?], |r| Ok(sql_text(r.get(0)?)))?
                    .collect::<Result<_, _>>()?;
                let mut ranking = Ranking::default();
                for (n, id) in ids.iter().enumerate() {
                    ranking.set(id, n);
                }
                rankings.push(ranking);
            }
            let scores = fused(&rankings);
            let skip: HashSet<&str> = leave_out.iter().map(String::as_str).collect();
            let mut order = Vec::new();
            for (ident, score) in scores {
                if skip.contains(ident.as_str()) {
                    continue;
                }
                let row = self
                    .conn
                    .query_row(
                        "SELECT origin, valid_from FROM memory_items WHERE id = ?",
                        [&ident],
                        |r| Ok((sql_text(r.get(0)?), sql_text(r.get(1)?))),
                    )
                    .optional()?;
                let Some((origin, valid_from)) = row else {
                    continue;
                };
                let owner = origin == "owner_stated";
                let boost = if words.is_empty() {
                    0.0
                } else {
                    recency(&valid_from, &now, owner)
                };
                let total = score + boost;
                order.push((
                    round9(total),
                    !words.is_empty() && owner,
                    valid_from,
                    ident,
                    total,
                ));
            }
            order.sort_by(|a, b| {
                b.0.partial_cmp(&a.0)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then(b.1.cmp(&a.1))
                    .then_with(|| b.2.cmp(&a.2))
                    .then_with(|| b.3.cmp(&a.3))
                    .then_with(|| b.4.partial_cmp(&a.4).unwrap_or(std::cmp::Ordering::Equal))
            });
            let mut out = Vec::new();
            for (_, _, _, ident, score) in order {
                if limit.is_some_and(|l| out.len() >= l) {
                    break;
                }
                let Some(item) = memory.get(&ident)? else {
                    continue;
                };
                if !terms::prompt_ready(&item, &now)? || item.set_aside {
                    continue;
                }
                out.push(Hit {
                    kind: "fact".into(),
                    doc: format!("memory:{ident}"),
                    score,
                    at: times::iso(&item.valid_from),
                    owner: item.origin == "owner_stated",
                    data: item,
                });
            }
            Ok(out)
        }

        fn chat_marks() -> String {
            vec!["?"; CHAT_KINDS.len()].join(",")
        }

        /// `index_chat`: index the conversation entries journaled since the last call. Returns how many rows
        /// were read.
        pub fn index_chat(&self, stop: &dyn Fn() -> bool) -> Result<usize, Error> {
            let marks = Self::chat_marks();
            let mut done = 0;
            while !stop() {
                let kinds = params_from_iter(CHAT_KINDS.iter());
                let more = self
                    .conn
                    .query_row(
                        &format!(
                            "SELECT 1 FROM journal WHERE seq > COALESCE((SELECT CAST(value AS INTEGER) FROM \
                             recall_state WHERE name = 'chat_seq'), 0) AND kind IN ({marks}) LIMIT 1"
                        ),
                        kinds,
                        |_| Ok(()),
                    )
                    .optional()?;
                if more.is_none() {
                    break;
                }
                let own = self.conn.is_autocommit();
                if own {
                    self.conn.execute_batch("BEGIN IMMEDIATE")?;
                }
                let result = (|| -> Result<usize, Error> {
                    let after = match self
                        .conn
                        .query_row(
                            "SELECT value FROM recall_state WHERE name = 'chat_seq'",
                            [],
                            |r| r.get::<_, Sql>(0),
                        )
                        .optional()?
                    {
                        Some(v) => sql_int(v)?,
                        None => 0,
                    };
                    let mut args: Vec<Sql> = vec![Sql::Integer(after)];
                    args.extend(CHAT_KINDS.iter().map(|k| Sql::Text((*k).into())));
                    args.push(Sql::Integer(CHAT_INDEX_BATCH as i64));
                    let mut stmt = self.conn.prepare(&format!(
                        "SELECT seq, id, at, kind, data_json FROM journal WHERE seq > ? AND kind IN ({marks}) \
                         ORDER BY seq LIMIT ?"
                    ))?;
                    let rows: Vec<(i64, Sql, Sql, String, String)> = stmt
                        .query_map(params_from_iter(args), |r| {
                            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
                        })?
                        .collect::<Result<_, _>>()?;
                    for (_, id, at, kind, data_json) in &rows {
                        let data: Value = serde_json::from_str(data_json)
                            .map_err(|_| Crash("JSONDecodeError"))?;
                        let text = chat_text(kind, &data)?;
                        if text.is_empty() {
                            continue;
                        }
                        let session = if kind == "task.answer" {
                            String::new()
                        } else {
                            match pyval::get(&data, "session_id")? {
                                Some(v) if pyval::truthy(v) => pyval::to_str(v),
                                _ if kind == "chat.notice" => String::new(),
                                _ => "owner".into(),
                            }
                        };
                        let request = ["read_with", "request_id", "task_id"]
                            .iter()
                            .map(|k| pyval::get(&data, k))
                            .collect::<Result<Vec<_>, _>>()?
                            .into_iter()
                            .flatten()
                            .find(|v| pyval::truthy(v))
                            .map_or(String::new(), pyval::to_str);
                        self.conn.execute(
                            "INSERT INTO chat_fts (doc, kind, session, at, request, text) VALUES (?,?,?,?,?,?)",
                            rusqlite::params![
                                format!("journal:{}", sql_text(id.clone())),
                                kind,
                                session,
                                at,
                                request,
                                text
                            ],
                        )?;
                    }
                    if let Some(last) = rows.last() {
                        self.conn.execute(
                            "INSERT OR REPLACE INTO recall_state VALUES ('chat_seq', ?)",
                            [last.0.to_string()],
                        )?;
                    }
                    Ok(rows.len())
                })();
                match result {
                    Ok(n) => {
                        if own {
                            self.conn.execute_batch("COMMIT")?;
                        }
                        done += n;
                        if n < CHAT_INDEX_BATCH {
                            break;
                        }
                    }
                    Err(e) => {
                        if own && !self.conn.is_autocommit() {
                            let _ = self.conn.execute_batch("ROLLBACK");
                        }
                        return Err(e);
                    }
                }
            }
            Ok(done)
        }

        fn chat_terms(&self, words: &[String]) -> Result<Vec<String>, Error> {
            let total = self.count("SELECT COUNT(*) FROM chat_fts", &[])?;
            terms::weighted(words, total, |term| {
                self.count(
                    "SELECT COUNT(*) FROM chat_fts WHERE chat_fts MATCH ?",
                    &[&match_expr(&[term.to_owned()])],
                )
            })
        }

        /// `chat`: conversation entries matching `query`, best first. `accept`: which rows count; `exclude`:
        /// docs already shown; `since`/`until`: ISO bounds on `at` (compared as text).
        #[allow(clippy::too_many_arguments)]
        pub fn chat(
            &self,
            query: &str,
            vector: Option<&[i8]>,
            limit: usize,
            kinds: &[&str],
            accept: &dyn Fn(&ChatRow) -> bool,
            exclude: &[String],
            since: Option<&str>,
            until: Option<&str>,
        ) -> Result<Vec<Hit<ChatRow>>, Error> {
            self.index_chat(&|| false)?;
            let now = self.clock.now();
            let words = terms::query_terms(query);
            let terms = if words.is_empty() {
                Vec::new()
            } else {
                self.chat_terms(&words)?
            };
            if terms.is_empty() {
                return Ok(Vec::new());
            }
            let mut stmt = self.conn.prepare(&format!(
                "SELECT doc, bm25(chat_fts) FROM chat_fts WHERE chat_fts MATCH ? ORDER BY 2 LIMIT {KEYWORD_MOST}"
            ))?;
            let rows: Vec<(String, f64)> = stmt
                .query_map([match_expr(&terms)], |r| {
                    Ok((sql_text(r.get(0)?), r.get(1)?))
                })?
                .collect::<Result<_, _>>()?;
            let mut rankings = vec![tied(rows.iter().map(|(a, b)| (a.as_str(), *b)), false)];
            let near = self.vector_ranks(vector, &["chat"], "")?;
            if !near.is_empty() {
                rankings.push(near);
            }
            let scores = fused(&rankings);
            let wanted: HashSet<&str> = kinds.iter().copied().collect();
            let skip: HashSet<&str> = exclude.iter().map(String::as_str).collect();
            let mut ids: Vec<&str> = scores.iter().map(|(k, _)| k.as_str()).collect();
            ids.sort_unstable();
            let mut found: HashMap<String, ChatRow> = HashMap::new();
            for chunk in ids.chunks(500) {
                let mut stmt = self.conn.prepare(&format!(
                    "SELECT doc, kind, session, at, request, text FROM chat_fts WHERE doc IN ({})",
                    vec!["?"; chunk.len()].join(",")
                ))?;
                let rows = stmt.query_map(params_from_iter(chunk.iter()), |r| {
                    Ok(ChatRow {
                        doc: sql_text(r.get(0)?),
                        kind: sql_text(r.get(1)?),
                        session: sql_text(r.get(2)?),
                        at: sql_text(r.get(3)?),
                        request: sql_text(r.get(4)?),
                        text: sql_text(r.get(5)?),
                    })
                })?;
                for row in rows {
                    let row = row?;
                    found.insert(row.doc.clone(), row);
                }
            }
            let mut order = Vec::new();
            for (doc, score) in &scores {
                let Some(row) = found.get(doc) else { continue };
                if skip.contains(doc.as_str()) || !wanted.contains(row.kind.as_str()) {
                    continue;
                }
                let early = since.is_some_and(|s| !s.is_empty() && row.at.as_str() < s);
                let late = until.is_some_and(|u| !u.is_empty() && row.at.as_str() > u);
                if early || late || !accept(row) {
                    continue;
                }
                let owner = row.kind == "chat.message";
                let total = score + recency(&row.at, &now, false);
                order.push((
                    round9(total),
                    row.at.clone(),
                    doc.clone(),
                    total,
                    owner,
                    row,
                ));
            }
            order.sort_by(|a, b| {
                b.0.partial_cmp(&a.0)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then_with(|| b.1.cmp(&a.1))
                    .then_with(|| b.2.cmp(&a.2))
            });
            Ok(order
                .into_iter()
                .take(limit)
                .map(|(_, at, doc, total, owner, row)| Hit {
                    kind: "chat".into(),
                    doc,
                    score: total,
                    at,
                    owner,
                    data: row.clone(),
                })
                .collect())
        }

        fn pending_batch(&self, node_text: NodeText<'_>) -> Result<Vec<Pending>, Error> {
            let model = self.model.clone().ok_or(Crash("AssertionError"))?;
            let now = self.clock.now();
            let mut out = Vec::new();
            let mut stmt = self.conn.prepare(&format!(
                "SELECT m.id, m.content, m.subject, m.key, m.valid_from, COALESCE(c.about, '') AS about FROM \
                 memory_items m LEFT JOIN memory_context c ON c.item_id = m.id WHERE {CURRENT_FACTS} AND NOT EXISTS \
                 (SELECT 1 FROM recall_vectors v WHERE v.doc = 'memory:' || m.id AND v.model = ?) LIMIT ?"
            ))?;
            let facts: Vec<[String; 6]> = stmt
                .query_map(
                    rusqlite::params![slack(&now)?, model, EMBED_BATCH as i64],
                    |r| {
                        Ok([
                            sql_text(r.get(0)?),
                            sql_text(r.get(1)?),
                            sql_text(r.get(2)?),
                            sql_text(r.get(3)?),
                            sql_text(r.get(4)?),
                            sql_text(r.get(5)?),
                        ])
                    },
                )?
                .collect::<Result<_, _>>()?;
            for [id, content, subject, key, valid_from, about] in facts {
                out.push(Pending {
                    doc: format!("memory:{id}"),
                    kind: "fact".into(),
                    at: valid_from,
                    text: fact_text(&content, &about, &key, &subject),
                    chat_rowid: None,
                });
            }
            let marks = vec!["?"; NODE_KINDS.len()].join(",");
            let mut stmt = self.conn.prepare(&format!(
                "SELECT n.id, n.kind, n.key, n.body_json, n.recorded_at FROM learning_nodes n WHERE n.kind IN \
                 ({marks}) AND n.valid_to IS NULL AND n.state NOT IN ('retired','rolled_back') AND NOT EXISTS \
                 (SELECT 1 FROM recall_vectors v WHERE v.doc = n.id AND v.model = ?) LIMIT ?"
            ))?;
            let mut args: Vec<Sql> = NODE_KINDS.iter().map(|k| Sql::Text((*k).into())).collect();
            args.push(Sql::Text(model.clone()));
            args.push(Sql::Integer(EMBED_BATCH as i64));
            let nodes: Vec<[String; 5]> = stmt
                .query_map(params_from_iter(args), |r| {
                    Ok([
                        sql_text(r.get(0)?),
                        sql_text(r.get(1)?),
                        sql_text(r.get(2)?),
                        sql_text(r.get(3)?),
                        sql_text(r.get(4)?),
                    ])
                })?
                .collect::<Result<_, _>>()?;
            for [id, kind, key, body, recorded_at] in nodes {
                let body: Value =
                    serde_json::from_str(&body).map_err(|_| Crash("JSONDecodeError"))?;
                let text = node_text(&kind, &key, &body);
                out.push(Pending {
                    doc: id,
                    kind,
                    at: recorded_at,
                    text,
                    chat_rowid: None,
                });
            }
            let mark = self
                .conn
                .query_row(
                    "SELECT value FROM recall_state WHERE name = ?",
                    [format!("chat_vec:{model}")],
                    |r| r.get::<_, Sql>(0),
                )
                .optional()?;
            let mark = match mark {
                Some(v) => sql_int(v)?,
                None => 0,
            };
            let mut stmt = self.conn.prepare(
                "SELECT rowid, doc, at, text FROM chat_fts WHERE rowid > ? ORDER BY rowid LIMIT ?",
            )?;
            let chats: Vec<(i64, String, String, String)> = stmt
                .query_map(rusqlite::params![mark, EMBED_BATCH as i64], |r| {
                    Ok((
                        r.get(0)?,
                        sql_text(r.get(1)?),
                        sql_text(r.get(2)?),
                        sql_text(r.get(3)?),
                    ))
                })?
                .collect::<Result<_, _>>()?;
            for (rowid, doc, at, text) in chats {
                out.push(Pending {
                    doc,
                    kind: "chat".into(),
                    at,
                    text,
                    chat_rowid: Some(rowid),
                });
            }
            Ok(out)
        }

        /// Whether the thing a vector is made from still exists, read inside the write (the fix to Python's
        /// `embed_pending`, which stores a vector for a fact, node or chat entry deleted while its batch was
        /// being encoded, and no later pass removes it).
        fn still_there(&self, doc: &str, kind: &str) -> Result<bool, Error> {
            let (sql, key) = if kind == "fact" {
                (
                    "SELECT 1 FROM memory_items WHERE id = ?",
                    doc.strip_prefix("memory:").unwrap_or(doc),
                )
            } else if kind == "chat" {
                ("SELECT 1 FROM chat_fts WHERE doc = ?", doc)
            } else {
                ("SELECT 1 FROM learning_nodes WHERE id = ?", doc)
            };
            Ok(self
                .conn
                .query_row(sql, [key], |_| Ok(()))
                .optional()?
                .is_some())
        }

        /// `embed_pending`: embed what has no vector yet, a batch per transaction (encoding before the write lock
        /// is taken; `pause` between batches), then drop vectors whose fact or node no longer counts.
        ///
        /// Deliberate difference from Python: inside the write, a vector whose fact, node or chat entry was
        /// deleted while the batch was encoded is not stored (`still_there`). `embedded` counts what was encoded,
        /// as in Python.
        pub fn embed_pending(
            &mut self,
            embedder: &dyn Embedder,
            node_text: NodeText<'_>,
            stop: &dyn Fn() -> bool,
            pause: &mut dyn FnMut(),
        ) -> Result<Embedded, Error> {
            self.model = Some(embedder.name().to_owned());
            let (mut done, mut encode_s, mut batches) = (0, 0.0, 0);
            self.index_chat(stop)?;
            while !stop() {
                let batch = self.pending_batch(node_text)?;
                if batch.is_empty() {
                    break;
                }
                let mut chat_mark = None;
                let mut written = Vec::with_capacity(batch.len());
                for p in batch {
                    if let Some(rowid) = p.chat_rowid {
                        chat_mark = Some(rowid);
                    }
                    let started = cpu_seconds();
                    let vector = if pystr::strip(&p.text).is_empty() {
                        None
                    } else {
                        embedder.encode(&p.text)
                    };
                    encode_s += cpu_seconds() - started;
                    written.push((
                        p.doc,
                        p.kind,
                        p.at,
                        crate::embeddings::text_hash(&p.text),
                        vector,
                    ));
                }
                self.conn.execute_batch("BEGIN IMMEDIATE")?;
                let result = (|| -> Result<(), Error> {
                    for (doc, kind, at, digest, vector) in &written {
                        if !self.still_there(doc, kind)? {
                            continue;
                        }
                        let zero;
                        let vector = match vector {
                            Some(v) => v.as_slice(),
                            None => {
                                zero = vec![0i8; embedder.dimensions()];
                                zero.as_slice()
                            }
                        };
                        vectors::store(self.conn, doc, kind, at, embedder.name(), digest, vector)?;
                    }
                    if let Some(mark) = chat_mark {
                        self.conn.execute(
                            "INSERT OR REPLACE INTO recall_state VALUES (?, ?)",
                            [format!("chat_vec:{}", embedder.name()), mark.to_string()],
                        )?;
                    }
                    Ok(())
                })();
                match result {
                    Ok(()) => {
                        self.conn.execute_batch("COMMIT")?;
                        done += written.len();
                    }
                    Err(e) => {
                        if !self.conn.is_autocommit() {
                            let _ = self.conn.execute_batch("ROLLBACK");
                        }
                        return Err(e);
                    }
                }
                batches += 1;
                pause();
            }
            let removed = self.drop_stale(embedder.name())?;
            Ok(Embedded {
                embedded: done,
                batches,
                encode_cpu_s: pymath::round(encode_s, 4),
                removed,
            })
        }

        /// `drop_stale`: vectors of facts that no longer count and of closed nodes go.
        pub fn drop_stale(&self, model: &str) -> Result<usize, Error> {
            let cutoff = slack(&self.clock.now())?;
            let mut n = self.conn.execute(
                "DELETE FROM recall_vectors WHERE model = ? AND kind = 'fact' AND substr(doc, 8) IN (SELECT id FROM \
                 memory_items WHERE status NOT IN ('active','proposed') OR (valid_to IS NOT NULL AND valid_to < ?) \
                 UNION SELECT item_id FROM memory_set_aside)",
                rusqlite::params![model, cutoff],
            )?;
            let marks = vec!["?"; NODE_KINDS.len()].join(",");
            let mut args: Vec<Sql> = vec![Sql::Text(model.into())];
            args.extend(NODE_KINDS.iter().map(|k| Sql::Text((*k).into())));
            n += self.conn.execute(
                &format!(
                    "DELETE FROM recall_vectors WHERE model = ? AND kind IN ({marks}) AND doc IN (SELECT id FROM \
                     learning_nodes WHERE valid_to IS NOT NULL OR state IN ('retired','rolled_back'))"
                ),
                params_from_iter(args),
            )?;
            Ok(n)
        }

        /// `pending`: whether anything waits for a vector (sets this search's model).
        pub fn pending(&mut self, model: &str, node_text: NodeText<'_>) -> Result<bool, Error> {
            self.model = Some(model.to_owned());
            if !self.pending_batch(node_text)?.is_empty() {
                return Ok(true);
            }
            let marks = Self::chat_marks();
            Ok(self
                .conn
                .query_row(
                    &format!(
                        "SELECT 1 FROM journal WHERE seq > COALESCE((SELECT CAST(value AS INTEGER) FROM \
                         recall_state WHERE name = 'chat_seq'), 0) AND kind IN ({marks}) LIMIT 1"
                    ),
                    params_from_iter(CHAT_KINDS.iter()),
                    |_| Ok(()),
                )
                .optional()?
                .is_some())
        }
    }

    /// `time.process_time()`.
    fn cpu_seconds() -> f64 {
        #[cfg(unix)]
        {
            let mut ts = libc::timespec {
                tv_sec: 0,
                tv_nsec: 0,
            };
            // SAFETY: `ts` is a valid, writable timespec; the call only fills it.
            if unsafe { libc::clock_gettime(libc::CLOCK_PROCESS_CPUTIME_ID, &mut ts) } == 0 {
                return ts.tv_sec as f64 + ts.tv_nsec as f64 * 1e-9;
            }
        }
        0.0
    }

    /// `import_extract.imported_from`: which assistants each item was imported from.
    pub fn imported_from(
        conn: &Connection,
        item_ids: &[String],
    ) -> Result<Vec<(String, Vec<String>)>, Error> {
        let mut out: Vec<(String, Vec<String>)> = Vec::new();
        for batch in item_ids.chunks(500) {
            let mut stmt = conn.prepare(&format!(
                "SELECT item_id, source FROM memory_import_items WHERE item_id IN ({}) ORDER BY source",
                vec!["?"; batch.len()].join(",")
            ))?;
            let rows = stmt.query_map(params_from_iter(batch.iter()), |r| {
                Ok((sql_text(r.get(0)?), sql_text(r.get(1)?)))
            })?;
            for row in rows {
                let (id, source) = row?;
                match out.iter_mut().find(|(k, _)| *k == id) {
                    Some((_, v)) => v.push(source),
                    None => out.push((id, vec![source])),
                }
            }
        }
        Ok(out)
    }
}
