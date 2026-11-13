//! Redacted span persistence, evidence reads and task exports: a port of `ferb/store/spans.py`.
//! It never invents a missing observation.
//!
//! Seams: the clock and id source (`ferb/core/interfaces.py:15-23`), the redactor
//! (`ferb/sanitize`), and which action kinds are reversible (`ferb/policy/rules.py:74`).
//! Decoded JSON is `serde_json::Value` with key order kept; a stored value that isn't the
//! object Python expects reads as one without keys where Python would raise.

use std::collections::{BTreeSet, HashMap, HashSet};

use rusqlite::{params, params_from_iter, types::Value as Sql, OptionalExtension};
use serde_json::{json, Map, Value};

use crate::{
    origin, pyjson,
    pytime::{DateTime, Timespec},
    pyval::{char_len, get, loads, marks, merged, prefix, py_eq, py_str, query_maps, truthy},
    swap::swap,
    Db, Error, Redact, Result,
};

/// Characters of each tool result matched for argument sources (`spans.py:15`; S3).
pub const OUTPUT_TEXT_CAP: usize = 20000;
/// `origin.recorded` in SQL (`spans.py:17`): a non-root span's output with no recorded origin
/// is unlabeled (the one parameter).
const RECORDED: &str =
    "CASE WHEN kind != 'task' AND origin IS NULL AND output_json IS NOT NULL THEN ? ELSE origin END";

/// `Clock.now` (`ferb/core/interfaces.py:15-16`): timezone-aware wall time.
pub trait Clock {
    fn now(&self) -> DateTime;
}

/// `IdSource` (`ferb/core/interfaces.py:21-23`).
pub trait IdSource {
    fn new_id(&self) -> String;
}

/// `_dumps` (`spans.py:20-23`): a span's stored JSON, text as written (UTF-8), so a search for
/// non-ASCII words finds it (step 12a).
fn dumps(value: &Value) -> String {
    pyjson::dumps_utf8(value)
}

/// `tree_where` (`spans.py:26-29`): SQL and parameters selecting a task tree's spans. A range,
/// not substr(): it can use the task_id index. "/" is the character right after ".".
pub fn tree_where(root_id: &str) -> (&'static str, [String; 3]) {
    (
        "(task_id = ? OR (task_id >= ? AND task_id < ?))",
        [
            root_id.to_owned(),
            format!("{root_id}."),
            format!("{root_id}/"),
        ],
    )
}

/// Python's `Span` (`ferb/core/types.py:69-82`).
#[derive(Clone, Debug, PartialEq)]
pub struct Span {
    pub id: String,
    pub parent_id: Option<String>,
    pub kind: String,
    pub task_id: Option<String>,
    pub device_id: Option<String>,
    pub started_at: DateTime,
    pub ended_at: Option<DateTime>,
    pub input: Value,
    pub output: Option<Value>,
    pub cost_usd: Option<f64>,
    pub error: Option<String>,
    pub origin: Option<String>,
    pub summary: Option<String>,
}

/// `SpanNode` (`spans.py:32-35`).
#[derive(Clone, Debug, PartialEq)]
pub struct SpanNode {
    pub span: Span,
    pub children: Vec<SpanNode>,
}

/// `PayloadOutput` (`spans.py:38-47`). `offset` and `next_offset` are the stored values as
/// they are (`0` and null when absent).
#[derive(Clone, Debug, PartialEq)]
pub struct PayloadOutput {
    pub rowid: i64,
    pub origins: BTreeSet<String>,
    pub text: String,
    pub path: String,
    pub offset: Value,
    pub next_offset: Value,
    /// A context record's entries, each with its stored values (GATE-519).
    pub segments: Option<Vec<Value>>,
    /// The page a browser result was read from (its `url`, GATE-528).
    pub page: String,
}

/// What `finish` records beside the end time (`spans.py:74-76`).
#[derive(Clone, Debug, Default)]
pub struct Finish<'a> {
    pub output: Option<&'a Value>,
    pub error: Option<&'a str>,
    pub cost_usd: Option<f64>,
    pub origin: Option<&'a str>,
    pub summary: Option<&'a str>,
}

/// Which records `task.evidence` may read (`spans.py:419-438`).
#[derive(Clone, Copy, Debug, Default)]
pub struct Evidence<'a> {
    pub root_id: Option<&'a str>,
    /// ORCH-517: every record of the tree `task_id` (an earlier task of the caller's chat).
    pub whole: bool,
}

/// Python's `SpanStore` (`spans.py:50`).
pub struct SpanStore<'a> {
    db: &'a Db,
    clock: Box<dyn Clock + 'a>,
    ids: Box<dyn IdSource + 'a>,
    redact: Redact,
    /// `kind in REVERSIBLE` (`ferb/policy/rules.py:74`): the policy's own table, given in.
    reversible: Box<dyn Fn(&str) -> bool + 'a>,
}

fn text(row: &Map<String, Value>, key: &str) -> Option<String> {
    row.get(key).and_then(Value::as_str).map(str::to_owned)
}

fn sql_params(values: &[Value]) -> Vec<Sql> {
    values
        .iter()
        .map(|v| match v {
            Value::Null => Sql::Null,
            Value::String(s) => Sql::Text(s.clone()),
            Value::Number(n) if n.is_f64() => Sql::Real(n.as_f64().unwrap_or(0.0)),
            Value::Number(n) => Sql::Integer(n.as_i64().unwrap_or(0)),
            Value::Bool(b) => Sql::Integer(i64::from(*b)),
            other => Sql::Text(pyjson::dumps_utf8(other)),
        })
        .collect()
}

fn parse_time(value: &str) -> Result<DateTime> {
    DateTime::from_isoformat(value)
        .map_err(|_| Error::Value(format!("Invalid isoformat string: '{value}'")))
}

/// `_row_to_span` (`spans.py:512-518`).
fn row_to_span(r: &Map<String, Value>) -> Result<Span> {
    let stored = |key: &str| text(r, key).filter(|t| !t.is_empty());
    Ok(Span {
        id: text(r, "id").unwrap_or_default(),
        parent_id: text(r, "parent_id"),
        kind: text(r, "kind").unwrap_or_default(),
        task_id: text(r, "task_id"),
        device_id: text(r, "device_id"),
        started_at: parse_time(&text(r, "started_at").unwrap_or_default())?,
        ended_at: stored("ended_at").map(|t| parse_time(&t)).transpose()?,
        input: loads(&text(r, "input_json").unwrap_or_default())?,
        output: stored("output_json").map(|t| loads(&t)).transpose()?,
        cost_usd: r.get("cost_usd").and_then(Value::as_f64),
        error: text(r, "error"),
        origin: text(r, "origin"),
        summary: text(r, "summary"),
    })
}

fn opt(value: &Option<String>) -> Value {
    value.clone().map_or(Value::Null, Value::String)
}

fn cost(value: Option<f64>) -> Value {
    value
        .and_then(serde_json::Number::from_f64)
        .map_or(Value::Null, Value::Number)
}

/// `node_json` (`spans.py:521-526`): a node as plain JSON, times as ISO strings, children
/// nested; keys in the order of `Span`'s fields.
pub fn node_json(node: &SpanNode) -> Value {
    let s = &node.span;
    json!({
        "id": s.id,
        "parent_id": opt(&s.parent_id),
        "kind": s.kind,
        "task_id": opt(&s.task_id),
        "device_id": opt(&s.device_id),
        "started_at": s.started_at.isoformat(Timespec::Auto),
        "ended_at": s.ended_at.map_or(Value::Null, |t| json!(t.isoformat(Timespec::Auto))),
        "input": s.input,
        "output": s.output.clone().unwrap_or(Value::Null),
        "cost_usd": cost(s.cost_usd),
        "error": opt(&s.error),
        "origin": opt(&s.origin),
        "summary": opt(&s.summary),
        "children": node.children.iter().map(node_json).collect::<Vec<_>>(),
    })
}

/// `_span_record` (`spans.py:529-534`).
fn span_record(s: &Span) -> Value {
    json!({
        "id": s.id,
        "parent_id": opt(&s.parent_id),
        "task_id": opt(&s.task_id),
        "kind": s.kind,
        "started_at": s.started_at.isoformat(Timespec::Auto),
        "ended_at": s.ended_at.map_or(Value::Null, |t| json!(t.isoformat(Timespec::Auto))),
        "input": s.input,
        "output": s.output.clone().unwrap_or(Value::Null),
        "error": opt(&s.error),
        "cost_usd": cost(s.cost_usd),
        "origin": opt(&s.origin),
        "summary": opt(&s.summary),
    })
}

/// `_strings` (`spans.py:537-545`): every string (and number as a string) inside decoded JSON,
/// in order, as they are reached; `each` returns false to stop the walk.
fn strings(value: &Value, each: &mut dyn FnMut(&str) -> bool) -> bool {
    match value {
        Value::String(s) => each(s),
        Value::Number(n) => each(&pyjson::number(n)),
        Value::Array(items) => items.iter().all(|v| strings(v, each)),
        Value::Object(map) => map.values().all(|v| strings(v, each)),
        Value::Null | Value::Bool(_) => true,
    }
}

/// `_joined` (`spans.py:548-561`): the strings of decoded JSON joined by newlines, cut at
/// `cap` characters; the walk stops once the cap is reached.
fn joined(value: &Value, cap: usize) -> String {
    let mut parts: Vec<String> = Vec::new();
    let mut size = 0usize;
    strings(value, &mut |text| {
        if !parts.is_empty() {
            size += 1; // the newline before this string
        }
        let part = prefix(text, cap.saturating_sub(size));
        size += char_len(part);
        parts.push(part.to_owned());
        size < cap
    });
    prefix(&parts.join("\n"), cap).to_owned()
}

fn string_set(recorded: &[Value]) -> BTreeSet<String> {
    recorded.iter().filter(|o| truthy(o)).map(py_str).collect()
}

impl<'a> SpanStore<'a> {
    pub fn new(
        db: &'a Db,
        clock: Box<dyn Clock + 'a>,
        ids: Box<dyn IdSource + 'a>,
        redact: Redact,
        reversible: Box<dyn Fn(&str) -> bool + 'a>,
    ) -> Self {
        SpanStore {
            db,
            clock,
            ids,
            redact,
            reversible,
        }
    }

    /// The database the spans are kept in (`spans.py:54-57`).
    pub fn db(&self) -> &Db {
        self.db
    }

    /// One text redacted as the value of `key` (`self._redact({key: text})[key]`).
    fn redact_text(&self, key: &str, value: &str) -> Option<String> {
        let clean = (self.redact)(&json!({ key: value }));
        match get(&clean, key) {
            Some(Value::String(s)) => Some(s.clone()),
            Some(Value::Null) | None => None,
            Some(other) => Some(py_str(other)),
        }
    }

    /// `start` (`spans.py:59-68`): a new span, its input redacted, recorded as started now.
    pub fn start(
        &self,
        kind: &str,
        input: &Value,
        task_id: Option<&str>,
        parent_id: Option<&str>,
        device_id: Option<&str>,
    ) -> Result<Span> {
        let span = Span {
            id: self.ids.new_id(),
            parent_id: parent_id.map(str::to_owned),
            kind: kind.to_owned(),
            task_id: task_id.map(str::to_owned),
            device_id: device_id.map(str::to_owned),
            started_at: self.clock.now(),
            ended_at: None,
            input: (self.redact)(input),
            output: None,
            cost_usd: None,
            error: None,
            origin: None,
            summary: None,
        };
        self.db.conn().execute(
            "INSERT INTO spans (id, parent_id, task_id, kind, device_id, started_at, input_json) \
             VALUES (?, ?, ?, ?, ?, ?, ?)",
            params![
                span.id,
                parent_id,
                task_id,
                kind,
                device_id,
                span.started_at.isoformat(Timespec::Auto),
                dumps(&span.input)
            ],
        )?;
        Ok(span)
    }

    /// `finish` (`spans.py:74-90`): the span ended now, with its redacted output, error, origin
    /// and summary. No origin given keeps one recorded earlier (`set_origin`).
    pub fn finish(&self, span: &mut Span, with: Finish<'_>) -> Result<()> {
        let ended = self.clock.now();
        span.ended_at = Some(ended);
        span.output = with.output.map(|o| (self.redact)(o));
        span.error = with.error.and_then(|e| self.redact_text("error", e));
        span.cost_usd = with.cost_usd;
        if let Some(origin) = with.origin {
            span.origin = self.redact_text("origin", origin);
        }
        if let Some(summary) = with.summary {
            span.summary = self.redact_text("summary", summary);
        }
        self.db.conn().execute(
            "UPDATE spans SET ended_at = ?, output_json = ?, error = ?, cost_usd = ?, origin = COALESCE(?, origin), \
             summary = COALESCE(?, summary) WHERE id = ?",
            params![
                ended.isoformat(Timespec::Auto),
                span.output.as_ref().map(dumps),
                span.error,
                with.cost_usd,
                with.origin.and(span.origin.as_deref()),
                span.summary,
                span.id
            ],
        )?;
        Ok(())
    }

    /// `set_origin` (`spans.py:92-95`): where a span's content came from, before it finishes.
    pub fn set_origin(&self, span_id: &str, origin: &str) -> Result<()> {
        self.db.conn().execute(
            "UPDATE spans SET origin = ? WHERE id = ?",
            params![self.redact_text("origin", origin), span_id],
        )?;
        Ok(())
    }

    /// `tree` (`spans.py:97-106`): all spans of a task and its steps as a forest (roots = spans
    /// whose parent isn't among them), in start order.
    pub fn tree(&self, task_id: &str) -> Result<Vec<SpanNode>> {
        let spans: Vec<Span> = self
            .task_rows(task_id)?
            .iter()
            .map(row_to_span)
            .collect::<Result<_>>()?;
        // Python keys its nodes by id: a later row with an id seen takes the first one's place.
        let mut index: HashMap<&str, usize> = HashMap::new();
        let mut order: Vec<usize> = Vec::new();
        for (i, span) in spans.iter().enumerate() {
            match index.get(span.id.as_str()) {
                Some(&slot) => order[slot] = i,
                None => {
                    index.insert(&span.id, order.len());
                    order.push(i);
                }
            }
        }
        let mut children: HashMap<usize, Vec<usize>> = HashMap::new();
        let mut roots = Vec::new();
        for &i in &order {
            let parent = spans[i]
                .parent_id
                .as_deref()
                .and_then(|p| index.get(p))
                .map(|&slot| order[slot]);
            match parent {
                Some(p) => children.entry(p).or_default().push(i),
                None => roots.push(i),
            }
        }
        fn build(i: usize, spans: &[Span], children: &HashMap<usize, Vec<usize>>) -> SpanNode {
            SpanNode {
                span: spans[i].clone(),
                children: children
                    .get(&i)
                    .map(|c| c.iter().map(|&j| build(j, spans, children)).collect())
                    .unwrap_or_default(),
            }
        }
        Ok(roots
            .into_iter()
            .map(|i| build(i, &spans, &children))
            .collect())
    }

    /// The origins of every row `sql` gives in its one column, split.
    fn origins_of(&self, sql: &str, args: &[&str]) -> Result<BTreeSet<String>> {
        let mut statement = self.db.conn().prepare(sql)?;
        let mut rows = statement.query(params_from_iter(args))?;
        let mut found = BTreeSet::new();
        while let Some(row) = rows.next()? {
            found.extend(origin::split(row.get::<_, Option<String>>(0)?.as_deref()));
        }
        Ok(found)
    }

    fn recorded_origins(&self, kinds: &str, root_id: &str) -> Result<BTreeSet<String>> {
        let (tree, p) = tree_where(root_id);
        self.origins_of(
            &format!("SELECT {RECORDED} AS origin FROM spans WHERE kind {kinds} AND {tree}"),
            &[&origin::external("unlabeled", ""), &p[0], &p[1], &p[2]],
        )
    }

    /// `tool_origins` (`spans.py:111-117`): every origin recorded on this task tree's tool
    /// results (a result with no recorded origin is unlabeled).
    pub fn tool_origins(&self, root_id: &str) -> Result<BTreeSet<String>> {
        self.recorded_origins("= 'tool.call'", root_id)
    }

    /// `request_origins` (`spans.py:119-131`): the origins of the request itself (S3), as the
    /// supervisor recorded them on the tree's root task span (`input.request_origins`). A root
    /// recorded without them gives the root's own origin: fail-closed.
    pub fn request_origins(&self, root_id: &str) -> Result<BTreeSet<String>> {
        let mut statement = self.db.conn().prepare(
            "SELECT input_json, origin FROM spans WHERE kind = 'task' AND task_id = ? ORDER BY rowid",
        )?;
        let mut rows = statement.query([root_id])?;
        let mut found = BTreeSet::new();
        while let Some(row) = rows.next()? {
            let input = loads(&row.get::<_, String>(0)?)?;
            match get(&input, "request_origins") {
                Some(Value::Array(recorded)) => found.extend(string_set(recorded)),
                _ => found.extend(origin::split(row.get::<_, Option<String>>(1)?.as_deref())),
            }
        }
        Ok(found)
    }

    /// `request_began` (`spans.py:133-145`): when this tree's run for `request_id` began (its
    /// first root task span) and how many runs of its worker came before it, but only when an
    /// earlier message's run of the same tree did; `None` for a tree with one message. Counted
    /// in record order, not by time.
    pub fn request_began(
        &self,
        root_id: &str,
        request_id: &str,
        worker_id: &str,
    ) -> Result<Option<(String, i64)>> {
        let conn = self.db.conn();
        let mut statement = conn.prepare(
            "SELECT rowid, started_at, input_json FROM spans WHERE kind = 'task' \
             AND task_id = ? AND parent_id IS NULL ORDER BY rowid",
        )?;
        let rows: Vec<(i64, String, String)> = statement
            .query_map([root_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
            .collect::<rusqlite::Result<_>>()?;
        let mut mine = Vec::new();
        for (rowid, started, input) in &rows {
            let input = loads(input)?;
            if get(&input, "request_id").is_some_and(|r| r.as_str() == Some(request_id)) {
                mine.push((*rowid, started.clone()));
            }
        }
        if mine.is_empty() || mine.len() == rows.len() {
            return Ok(None);
        }
        let earlier: i64 = conn.query_row(
            "SELECT COUNT(*) FROM spans WHERE kind = 'worker' AND task_id = ? AND rowid < ?",
            params![worker_id, mine[0].0],
            |r| r.get(0),
        )?;
        Ok(Some((mine[0].1.clone(), earlier)))
    }

    /// `asked_origins` (`spans.py:147-156`): the outside origins of FERB's earlier questions
    /// this tree's requests answered (S4 b), as recorded on each attempt's root task span.
    pub fn asked_origins(&self, root_id: &str) -> Result<BTreeSet<String>> {
        let mut statement = self
            .db
            .conn()
            .prepare("SELECT input_json FROM spans WHERE kind = 'task' AND task_id = ?")?;
        let mut rows = statement.query([root_id])?;
        let mut found = BTreeSet::new();
        while let Some(row) = rows.next()? {
            let input = loads(&row.get::<_, String>(0)?)?;
            if let Some(Value::Array(recorded)) = get(&input, "asked_origins") {
                found.extend(string_set(recorded));
            }
        }
        Ok(found)
    }

    /// `reopened_answers` (`spans.py:158-163`): the ids of the owner's No answers in this tree
    /// that the owner's own later message carrying the task on reopened.
    pub fn reopened_answers(&self, root_id: &str) -> Result<BTreeSet<String>> {
        let mut statement = self.db.conn().prepare(
            "SELECT input_json FROM spans WHERE kind = 'confirmations.reopened' AND task_id = ?",
        )?;
        let mut rows = statement.query([root_id])?;
        let mut found = BTreeSet::new();
        while let Some(row) = rows.next()? {
            let input = loads(&row.get::<_, String>(0)?)?;
            if let Some(Value::Array(ids)) = get(&input, "reopened") {
                found.extend(ids.iter().map(py_str));
            }
        }
        Ok(found)
    }

    /// `exposure_origins` (`spans.py:165-170`): actual retrieved content given to models.
    pub fn exposure_origins(&self, root_id: &str) -> Result<BTreeSet<String>> {
        self.recorded_origins("='context.exposure'", root_id)
    }

    /// `outside_exposure_origins` (`spans.py:172-180`; decision: GATE-525): the outside labels
    /// of the context given to this tree's models.
    pub fn outside_exposure_origins(&self, root_id: &str) -> Result<BTreeSet<String>> {
        let mut found = self.exposure_origins(root_id)?;
        found.retain(|o| origin::is_external(o));
        Ok(found)
    }

    /// `task_origins` (`spans.py:182-194`): every origin recorded on this task tree's request,
    /// tool results and context sources, with its episode's (retention deletes the spans, never
    /// the episode: its origins keep the labels fail-closed, MP2).
    pub fn task_origins(&self, root_id: &str) -> Result<BTreeSet<String>> {
        let mut found = self.recorded_origins(
            "IN ('task', 'tool.call', 'context.source', 'context.exposure')",
            root_id,
        )?;
        let episode: Option<Option<String>> = self
            .db
            .conn()
            .query_row(
                "SELECT origins FROM episodes WHERE task_id = ?",
                [root_id],
                |r| r.get(0),
            )
            .optional()?;
        found.extend(origin::split(episode.flatten().as_deref()));
        Ok(found)
    }

    /// `tool_outputs` (`spans.py:196-213`): (rowid, origins, text) of this task tree's finished
    /// tool calls after `after_rowid`, in call order; the text is every string value of the
    /// decoded output joined by newlines, capped at 20000 characters. `include_context` adds
    /// the context given to its models whose stored origin is outside content (L4).
    pub fn tool_outputs(
        &self,
        root_id: &str,
        after_rowid: i64,
        include_context: bool,
    ) -> Result<Vec<(i64, BTreeSet<String>, String)>> {
        let (tree, p) = tree_where(root_id);
        let kinds = if include_context {
            "('tool.call','context.exposure')"
        } else {
            "('tool.call')"
        };
        let mut statement = self.db.conn().prepare(&format!(
            "SELECT rowid, kind, origin, output_json FROM spans WHERE kind IN {kinds} AND \
             {tree} AND rowid > ? AND output_json IS NOT NULL ORDER BY rowid"
        ))?;
        let mut rows = statement.query(params![p[0], p[1], p[2], after_rowid])?;
        let mut out = Vec::new();
        while let Some(row) = rows.next()? {
            let labels = origin::split(row.get::<_, Option<String>>(2)?.as_deref());
            if row.get::<_, String>(1)? == "context.exposure"
                && !origin::stored_outside(labels.iter().map(String::as_str))
            {
                continue;
            }
            let output = loads(&row.get::<_, String>(3)?)?;
            out.push((row.get(0)?, labels, joined(&output, OUTPUT_TEXT_CAP)));
        }
        Ok(out)
    }

    /// `payload_outputs` (`spans.py:215-252`): new actual source data, by capability: a file
    /// read's text with its chunk bounds, a shell call's output, nothing for calls that return
    /// no content, a context record's payload. Even empty results are returned: they advance
    /// the caller's cursor.
    pub fn payload_outputs(
        &self,
        root_id: &str,
        after_rowid: i64,
        seen: Option<&HashSet<i64>>,
    ) -> Result<Vec<PayloadOutput>> {
        const NO_CONTENT: [&str; 12] = [
            "fs.list",
            "fs.write",
            "fs.patch",
            "fs.mkdir",
            "fs.trash",
            "fs.delete",
            "browser.open",
            "browser.navigate",
            "browser.close",
            "ferb_browser.open",
            "ferb_browser.navigate",
            "ferb_browser.close",
        ];
        let (tree, p) = tree_where(root_id);
        let mut statement = self.db.conn().prepare(&format!(
            "SELECT rowid,kind,origin,input_json,output_json FROM spans WHERE kind IN ('tool.call','context.exposure') AND {tree} \
             AND rowid > ? AND output_json IS NOT NULL ORDER BY rowid"
        ))?;
        let mut rows = statement.query(params![p[0], p[1], p[2], after_rowid])?;
        let mut out = Vec::new();
        let empty = json!("");
        while let Some(row) = rows.next()? {
            let rowid: i64 = row.get(0)?;
            if seen.is_some_and(|s| s.contains(&rowid)) {
                continue;
            }
            let kind: String = row.get(1)?;
            let input = loads(&row.get::<_, String>(3)?)?;
            let body = loads(&row.get::<_, String>(4)?)?;
            let mut result = PayloadOutput {
                rowid,
                origins: origin::split(row.get::<_, Option<String>>(2)?.as_deref()),
                text: String::new(),
                path: String::new(),
                offset: json!(0),
                next_offset: Value::Null,
                segments: None,
                page: String::new(),
            };
            let tool = get(&input, "tool").cloned().unwrap_or(json!(""));
            let tool = tool.as_str().unwrap_or("");
            let mut payload = &body;
            if body.is_object() {
                if kind == "tool.call" {
                    if let Some(Value::String(url)) = get(&body, "url") {
                        result.page = url.clone();
                    }
                }
                if kind == "context.exposure" {
                    payload = get(&body, "payload")
                        .or_else(|| get(&body, "text"))
                        .unwrap_or(&empty);
                    if let Some(Value::Array(segments)) = get(&body, "segments") {
                        result.segments = Some(segments.clone());
                    }
                } else if tool == "fs.read" {
                    payload = get(&body, "text").unwrap_or(&empty);
                    let path = get(&body, "path")
                        .filter(|v| truthy(v))
                        .or_else(|| get(&input, "args").and_then(|a| get(a, "path")))
                        .filter(|v| truthy(v));
                    result.path = path.map(py_str).unwrap_or_default();
                    result.offset = get(&body, "offset").cloned().unwrap_or(json!(0));
                    result.next_offset = get(&body, "next_offset").cloned().unwrap_or(Value::Null);
                } else if matches!(tool, "shell.run" | "shell.read" | "shell.wait") {
                    payload = get(&body, "output").unwrap_or(&empty);
                } else if NO_CONTENT.contains(&tool) {
                    payload = &empty;
                }
            }
            let mut parts: Vec<String> = Vec::new();
            strings(payload, &mut |text| {
                parts.push(text.to_owned());
                true
            });
            result.text = parts.join("\n");
            out.push(result);
        }
        Ok(out)
    }

    /// `latest_acting_call` (`spans.py:254-265`): the latest tool.call in this task tree: of
    /// `tool` when named, else any consequential (non-reading) one. Failed calls count. Never a
    /// model's claim: the recorded span. `{"tool", "output", "error"}`.
    pub fn latest_acting_call(&self, root_id: &str, tool: Option<&str>) -> Result<Option<Value>> {
        let (tree, p) = tree_where(root_id);
        let mut statement = self.db.conn().prepare(&format!(
            "SELECT input_json, output_json, error FROM spans WHERE kind = 'tool.call' AND \
             {tree} ORDER BY rowid DESC"
        ))?;
        let mut rows = statement.query(params_from_iter(p.iter()))?;
        while let Some(row) = rows.next()? {
            let input = loads(&row.get::<_, String>(0)?)?;
            let hit = match tool.filter(|t| !t.is_empty()) {
                Some(tool) => get(&input, "tool").and_then(Value::as_str) == Some(tool),
                None => match get(&input, "kind") {
                    None | Some(Value::Null) => false,
                    Some(Value::String(kind)) => !(self.reversible)(kind),
                    Some(_) => true,
                },
            };
            if hit {
                let output: Option<String> = row.get(1)?;
                let output = match output.filter(|o| !o.is_empty()) {
                    Some(text) => loads(&text)?,
                    None => Value::Null,
                };
                return Ok(Some(json!({
                    "tool": get(&input, "tool").cloned().unwrap_or(Value::Null),
                    "output": output,
                    "error": row.get::<_, Option<String>>(2)?,
                })));
            }
        }
        Ok(None)
    }

    /// `first_open_tool_call` (`spans.py:267-274`): the rowid of this task tree's earliest tool
    /// call after `after_rowid` that hasn't finished, if any.
    pub fn first_open_tool_call(
        &self,
        root_id: &str,
        after_rowid: i64,
        include_context: bool,
    ) -> Result<Option<i64>> {
        let (tree, p) = tree_where(root_id);
        let kinds = if include_context {
            "('tool.call','context.exposure')"
        } else {
            "('tool.call')"
        };
        Ok(self.db.conn().query_row(
            &format!(
                "SELECT min(rowid) FROM spans WHERE kind IN {kinds} AND {tree} \
                 AND rowid > ? AND ended_at IS NULL"
            ),
            params![p[0], p[1], p[2], after_rowid],
            |r| r.get(0),
        )?)
    }

    /// `owner_answers` (`spans.py:276-284`): the owner's answers to owner.ask in this task
    /// tree: only spans whose input names that tool and whose origin is exactly the owner.
    pub fn owner_answers(&self, root_id: &str) -> Result<Vec<String>> {
        let (tree, p) = tree_where(root_id);
        let mut statement = self.db.conn().prepare(&format!(
            "SELECT output_json FROM spans WHERE kind = 'tool.call' AND origin = ? AND output_json IS NOT NULL \
             AND json_extract(input_json, '$.tool') = 'owner.ask' AND {tree} ORDER BY rowid"
        ))?;
        let mut rows = statement.query(params![origin::OWNER, p[0], p[1], p[2]])?;
        let mut answers = Vec::new();
        while let Some(row) = rows.next()? {
            let output = loads(&row.get::<_, String>(0)?)?;
            if let Some(Value::String(answer)) = get(&output, "answer") {
                answers.push(answer.clone());
            }
        }
        Ok(answers)
    }

    /// `owner_questions` (`spans.py:286-305`): the task's questions to the owner (owner.ask,
    /// not FERB's own checks or retry questions) that the owner answered, each with the answer,
    /// in order, each pair once: what carries across the task's retries (rule 9).
    pub fn owner_questions(&self, root_id: &str) -> Result<Vec<(String, String)>> {
        let (tree, p) = tree_where(root_id);
        let mut statement = self.db.conn().prepare(&format!(
            "SELECT input_json, output_json FROM spans WHERE kind = 'tool.call' AND origin = ? \
             AND output_json IS NOT NULL AND json_extract(input_json, '$.tool') = 'owner.ask' \
             AND json_extract(input_json, '$.ferb_question') IS NULL \
             AND json_extract(input_json, '$.args.retry_action') IS NULL \
             AND json_extract(input_json, '$.args.question') IS NOT NULL AND {tree} ORDER BY rowid"
        ))?;
        let mut rows = statement.query(params![origin::OWNER, p[0], p[1], p[2]])?;
        let mut found: Vec<(String, String)> = Vec::new();
        while let Some(row) = rows.next()? {
            let asked = loads(&row.get::<_, String>(0)?)?;
            let given = loads(&row.get::<_, String>(1)?)?;
            if let Some(Value::String(answer)) = get(&given, "answer") {
                let question = get(&asked, "args")
                    .and_then(|a| get(a, "question"))
                    .map(py_str)
                    .unwrap_or_default();
                let item = (question, answer.clone());
                if !found.contains(&item) {
                    found.push(item);
                }
            }
        }
        Ok(found)
    }

    /// `last_typed` (`spans.py:307-315`): the text this task's worker last typed with `tool`
    /// into the page tab `tab` (a type action that didn't fail), as recorded; "" when none.
    pub fn last_typed(&self, task_id: &str, tool: &str, tab: &str) -> Result<String> {
        let row: Option<Sql> = self
            .db
            .conn()
            .query_row(
                "SELECT json_extract(input_json, '$.args.text') AS text FROM spans WHERE kind = 'tool.call' AND \
                 task_id = ? AND json_extract(input_json, '$.tool') = ? AND json_extract(input_json, '$.args.tab') = ? \
                 AND json_extract(input_json, '$.args.action') = 'type' AND error IS NULL ORDER BY rowid DESC LIMIT 1",
                params![task_id, tool, tab],
                |r| r.get(0),
            )
            .optional()?;
        Ok(row.and_then(typed_text).unwrap_or_default())
    }

    /// `typed_in_tab` (`spans.py:317-324`; decision: GATE-215): every text this task typed or
    /// chose into the page tab `tab` (a fill that didn't fail), as recorded, oldest first: what
    /// a form submitted there can carry (the sink rule).
    pub fn typed_in_tab(&self, task_id: &str, tab: &str) -> Result<Vec<String>> {
        let mut statement = self.db.conn().prepare(
            "SELECT json_extract(input_json, '$.args.text') AS text FROM spans WHERE kind = 'tool.call' AND \
             task_id = ? AND json_extract(input_json, '$.args.tab') = ? AND json_extract(input_json, '$.kind') = \
             'fill_form' AND error IS NULL ORDER BY rowid",
        )?;
        let rows = statement.query_map(params![task_id, tab], |r| r.get::<_, Sql>(0))?;
        let mut out = Vec::new();
        for row in rows {
            out.extend(typed_text(row?));
        }
        Ok(out)
    }

    /// `_task_rows` (`spans.py:326-351`): the tree's spans, the intake planner's by exact
    /// request id (it ran before the task id existed), then every descendant of a selected
    /// span, in start order. An id that is no task's may be a learning model call's, or a
    /// request's whose planning failed before any task existed.
    fn task_rows(&self, task_id: &str) -> Result<Vec<Map<String, Value>>> {
        let conn = self.db.conn();
        let root: Option<String> = conn
            .query_row(
                "SELECT request_id FROM tasks WHERE id = ?",
                [task_id],
                |r| r.get(0),
            )
            .optional()?;
        let call = if root.is_some() { None } else { Some(task_id) };
        let request_id = root.as_deref().unwrap_or(task_id);
        let (tree, p) = tree_where(task_id);
        let mut rows = query_maps(
            conn,
            &format!(
                "SELECT rowid AS row_order, * FROM spans WHERE {tree} \
                 OR (task_id IS NULL AND json_extract(input_json, '$.request_id') = ?) \
                 OR (task_id IS NULL AND kind = 'model.learning' AND json_extract(input_json, '$.call') = ?)"
            ),
            params![p[0], p[1], p[2], request_id, call],
        )?;
        let id_of = |r: &Map<String, Value>| text(r, "id").unwrap_or_default();
        let mut seen: HashSet<String> = rows.iter().map(id_of).collect();
        // Python walks `list(seen)`, a set's order; the rows found are sorted below, so only
        // rows with the same start time and rowid could tell, and none have the same rowid.
        let mut frontier: Vec<String> = seen.iter().cloned().collect();
        while !frontier.is_empty() {
            let mut found = Vec::new();
            for chunk in frontier.chunks(500) {
                let children = query_maps(
                    conn,
                    &format!(
                        "SELECT rowid AS row_order, * FROM spans WHERE parent_id IN ({})",
                        marks(chunk.len())
                    ),
                    params_from_iter(chunk),
                )?;
                found.extend(children.into_iter().filter(|r| !seen.contains(&id_of(r))));
            }
            seen.extend(found.iter().map(id_of));
            frontier = found.iter().map(id_of).collect();
            rows.extend(found);
        }
        rows.sort_by(|a, b| {
            let key = |r: &Map<String, Value>| {
                (
                    text(r, "started_at").unwrap_or_default(),
                    r.get("row_order").and_then(Value::as_i64).unwrap_or(0),
                )
            };
            key(a).cmp(&key(b))
        });
        Ok(rows)
    }

    /// `tree_json` (`spans.py:353-355`): `tree` as plain JSON: what trace.get and episodes read.
    pub fn tree_json(&self, task_id: &str) -> Result<Vec<Value>> {
        Ok(self.tree(task_id)?.iter().map(node_json).collect())
    }

    /// `outcome` (`spans.py:357-405`; decision: STORE-413): one redacted evidence object.
    /// Missing historical facts stay explicit, never reconstructed guesses. A continued tree
    /// has one root span per run: the request is the first run's, the result the last run's.
    pub fn outcome(&self, task_id: &str) -> Result<Value> {
        let conn = self.db.conn();
        let events: Vec<Value> = self
            .task_rows(task_id)?
            .iter()
            .map(|r| row_to_span(r).map(|s| span_record(&s)))
            .collect::<Result<_>>()?;
        let kind = |e: &Value| e["kind"].as_str().unwrap_or("").to_owned();
        let roots: Vec<&Value> = events
            .iter()
            .filter(|e| kind(e) == "task" && e["task_id"].as_str() == Some(task_id))
            .collect();
        let empty = json!({});
        let root = roots.last().copied().unwrap_or(&empty);
        let source = roots.first().map_or(&empty, |r| &r["input"]);
        let source = if source.is_object() { source } else { &empty };
        let mut tasks = Vec::new();
        for mut task in query_maps(
            conn,
            "SELECT * FROM tasks WHERE tree_id = ? ORDER BY created_at, id",
            [task_id],
        )? {
            for name in ["scopes", "outcome"] {
                let stored = task.shift_remove(&format!("{name}_json"));
                let stored = stored.as_ref().and_then(Value::as_str).unwrap_or("");
                task.insert(name.to_owned(), loads(stored)?);
            }
            tasks.push(Value::Object(task));
        }
        let request_id = match get(source, "request_id").filter(|v| truthy(v)) {
            Some(id) => id.clone(),
            None => tasks
                .first()
                .map_or(Value::Null, |t| t["request_id"].clone()),
        };
        let mut messages = Vec::new();
        if truthy(&request_id) {
            for r in query_maps(
                conn,
                "SELECT * FROM journal WHERE kind = 'chat.message' \
                 AND json_extract(data_json, '$.request_id') = ? ORDER BY seq",
                params_from_iter(sql_params(std::slice::from_ref(&request_id))),
            )? {
                messages.push(json!({
                    "id": r["id"],
                    "at": r["at"],
                    "data": loads(r["data_json"].as_str().unwrap_or(""))?,
                }));
            }
        }
        let mut outcomes = Vec::new();
        {
            let mut statement = conn.prepare(
                "SELECT data_json FROM journal WHERE kind IN ('task.outcome', 'task.answer') \
                 AND json_extract(data_json, '$.task_id') = ? ORDER BY seq",
            )?;
            let mut rows = statement.query([task_id])?;
            while let Some(row) = rows.next()? {
                outcomes.push(loads(&row.get::<_, String>(0)?)?);
            }
        }
        let verdict = query_maps(
            conn,
            "SELECT verdict, note, at FROM owner_verdicts WHERE task_id = ? ORDER BY seq DESC LIMIT 1",
            [task_id],
        )?
        .into_iter()
        .next()
        .map_or(Value::Null, Value::Object);
        let pick = |keep: &dyn Fn(&Value) -> bool| -> Vec<Value> {
            events.iter().filter(|e| keep(e)).cloned().collect()
        };
        let field = |key: &str| get(source, key).cloned().unwrap_or(Value::Null);
        Ok(json!({
            "schema_version": 1,
            "task_id": task_id,
            "request": {
                "id": request_id,
                "text": field("request"),
                "authority_words": field("authority_words"),
                "messages": messages,
            },
            "task_spec": field("task_spec"),
            "tasks": tasks,
            "planner_decisions": pick(&|e| kind(e) == "model.turn"
                && get(&e["input"], "role").and_then(Value::as_str) == Some("supervisor")),
            "approach_decisions": pick(&|e| kind(e).starts_with("model.")),
            "context_sources": pick(&|e| kind(e) == "context.source"),
            "worker_runs": pick(&|e| kind(e) == "worker"),
            "runtime_tools": pick(&|e| kind(e) == "runtime.tools"),
            "intake": pick(&|e| kind(e) == "task.intake"),
            "actions": pick(&|e| kind(e) == "tool.call"),
            "verification": pick(&|e| matches!(kind(e).as_str(), "verification" | "verify")
                || e["task_id"].as_str().unwrap_or("").ends_with(".verify")),
            "disturbances": pick(&|e| kind(e).starts_with("disturbance.")),
            "cleanup": pick(&|e| kind(e) == "task.cleanup"),
            "result": get(root, "output").cloned().unwrap_or(Value::Null),
            "outcome_records": outcomes,
            "owner_verdict": verdict,
            "coverage": {
                "full_request_recorded": get(source, "request").is_some(),
                "sentinel_recorded": events.iter().any(|e| kind(e) == "disturbance.summary"),
                "owner_verdict": if verdict.is_null() {
                    "no owner verdict collected; null is not acceptance"
                } else {
                    "recorded"
                },
            },
        }))
    }

    /// `update_input` (`spans.py:407-416`): measured runtime input added after tokens, tools
    /// and time are assembled, redacted before it is kept. Written only while the span still
    /// holds the input read (`swap`): a forget's scrub rewriting it meanwhile is never undone.
    pub fn update_input(&self, span_id: &str, values: &Value) -> Result<()> {
        let clean = (self.redact)(values);
        let conn = self.db.conn();
        let held: Option<Sql> = conn
            .query_row(
                "SELECT input_json FROM spans WHERE id = ?",
                [span_id],
                |r| r.get(0),
            )
            .optional()?;
        let Some(held) = held else {
            return Err(Error::Value("unknown span".to_owned()));
        };
        let empty = Map::new();
        swap(
            conn,
            "spans",
            "id",
            &Sql::Text(span_id.to_owned()),
            &["input_json"],
            |row| {
                let Sql::Text(stored) = &row[0] else {
                    return Err(Error::Json("a span's input isn't text".to_owned()));
                };
                let before = loads(stored)?;
                let after = merged(
                    before.as_object().unwrap_or(&empty),
                    clean.as_object().unwrap_or(&empty),
                );
                Ok(vec![Sql::Text(dumps(&Value::Object(after)))])
            },
            Some(vec![held]),
        )?;
        Ok(())
    }

    /// `_evidence_where` (`spans.py:418-438`).
    fn evidence_where(task_id: &str, scope: Evidence<'_>) -> (String, Vec<Sql>) {
        let t = |s: &str| Sql::Text(s.to_owned());
        if scope.whole {
            let (tree, p) = tree_where(task_id);
            return (
                format!(
                    "({tree} AND ((kind = 'tool.call' AND json_extract(input_json, '$.tool') NOT IN \
                     ('task.evidence','chat.history','memory.read')) OR kind IN ('task','task.intake','context.source',\
                     'context.exposure','disturbance.baseline','disturbance.change','disturbance.error',\
                     'disturbance.summary','disturbance.paused','disturbance.resumed','worker','task.cleanup')))"
                ),
                p.iter().map(|s| t(s)).collect(),
            );
        }
        // Root identity is resolved from the registered live task, never inferred from a name.
        let mut where_ =
            "(task_id = ? AND kind = 'tool.call' AND json_extract(input_json, '$.tool') NOT IN \
                          ('task.evidence','chat.history','memory.read'))"
                .to_owned();
        let mut args = vec![t(task_id)];
        if let Some(root_id) = scope.root_id.filter(|r| !r.is_empty()) {
            where_ += " OR (task_id = ? AND kind IN ('task','task.intake','context.source','context.exposure','disturbance.baseline',\
                       'disturbance.change','disturbance.error','disturbance.summary','disturbance.paused',\
                       'disturbance.resumed'))";
            args.push(t(root_id));
            where_ += " OR (task_id IN (?, ?) AND kind IN ('worker','task.cleanup'))";
            args.extend([t(task_id), t(&format!("{task_id}.verify"))]);
        }
        (format!("({where_})"), args)
    }

    /// `tool_call` (`spans.py:440-453`): an authorized recorded fact/report; never provider
    /// reasoning or recursive evidence copies.
    pub fn tool_call(
        &self,
        task_id: &str,
        span_id: &str,
        scope: Evidence<'_>,
    ) -> Result<Option<Value>> {
        let (where_, mut args) = Self::evidence_where(task_id, scope);
        args.push(Sql::Text(span_id.to_owned()));
        let row = query_maps(
            self.db.conn(),
            &format!("SELECT * FROM spans WHERE {where_} AND id = ?"),
            params_from_iter(args),
        )?
        .into_iter()
        .next();
        let Some(row) = row else {
            return Ok(None);
        };
        let output = match text(&row, "output_json").filter(|o| !o.is_empty()) {
            Some(stored) => loads(&stored)?,
            None => Value::Null,
        };
        Ok(Some(json!({
            "id": row["id"],
            "kind": row["kind"],
            "task_id": row["task_id"],
            "started_at": row["started_at"],
            "ended_at": row["ended_at"],
            "input": loads(row["input_json"].as_str().unwrap_or(""))?,
            "output": output,
            "error": row["error"],
            "origin": row["origin"],
        })))
    }

    /// `tool_calls` (`spans.py:455-479`): full redacted observations searched by literal text,
    /// a page of references at a time (`{"records", "next_after"}`). The query is matched as
    /// written now (UTF-8) and as rows written before step 12a hold it (`\uXXXX` escapes).
    pub fn tool_calls(
        &self,
        task_id: &str,
        query: &str,
        after: i64,
        limit: i64,
        kind: &str,
        scope: Evidence<'_>,
    ) -> Result<Value> {
        if limit < 1 || after < 0 {
            return Err(Error::Value(
                "limit must be positive and after nonnegative".to_owned(),
            ));
        }
        let (mut where_, mut args) = Self::evidence_where(task_id, scope);
        if !query.is_empty() {
            let escaped = pyjson::dumps(&json!(query));
            let mut forms = vec![query.to_owned()];
            let escaped = &escaped[1..escaped.len() - 1];
            if escaped != query {
                forms.push(escaped.to_owned());
            }
            let each = "instr(lower(kind || input_json || coalesce(output_json, '') || coalesce(error, '')), lower(?)) > 0";
            where_ += &format!(" AND ({})", vec![each; forms.len()].join(" OR "));
            args.extend(forms.into_iter().map(Sql::Text));
        }
        if !kind.is_empty() {
            where_ += " AND kind = ?";
            args.push(Sql::Text(kind.to_owned()));
        }
        args.extend([Sql::Integer(after), Sql::Integer(limit + 1)]);
        let rows = query_maps(
            self.db.conn(),
            &format!(
                "SELECT rowid AS cursor, * FROM spans WHERE {where_} AND rowid > ? ORDER BY rowid LIMIT ?"
            ),
            params_from_iter(args),
        )?;
        let limit = limit as usize;
        let mut records = Vec::new();
        for row in rows.iter().take(limit) {
            let input = row["input_json"].as_str().unwrap_or("");
            records.push(json!({
                "id": row["id"],
                "kind": row["kind"],
                "task_id": row["task_id"],
                "tool": get(&loads(input)?, "tool").cloned().unwrap_or(Value::Null),
                "started_at": row["started_at"],
                "ended_at": row["ended_at"],
                "error": row["error"],
                "input_preview": prefix(input, 500),
                "input_truncated": char_len(input) > 500,
                "read_full": {"tool": "task.evidence", "span_id": row["id"]},
                "has_output": !row["output_json"].is_null(),
                "origin": row["origin"],
            }));
        }
        let next_after = if rows.len() > limit {
            rows[limit - 1]["cursor"].clone()
        } else {
            Value::Null
        };
        Ok(json!({"records": records, "next_after": next_after}))
    }

    /// `evidence_kinds` (`spans.py:481-485`; decision: ORCH-617): how many records of each kind
    /// task.evidence can read for this task, most first.
    pub fn evidence_kinds(&self, task_id: &str, scope: Evidence<'_>) -> Result<Vec<(String, i64)>> {
        let (where_, args) = Self::evidence_where(task_id, scope);
        let mut statement = self.db.conn().prepare(&format!(
            "SELECT kind, COUNT(*) FROM spans WHERE {where_} GROUP BY kind ORDER BY COUNT(*) DESC, kind"
        ))?;
        let rows = statement.query_map(params_from_iter(args), |r| Ok((r.get(0)?, r.get(1)?)))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }
}

/// A typed text as `last_typed` and `typed_in_tab` read it: `str(text)` when it is truthy.
fn typed_text(value: Sql) -> Option<String> {
    match value {
        Sql::Text(t) if !t.is_empty() => Some(t),
        Sql::Integer(n) if n != 0 => Some(n.to_string()),
        Sql::Real(f) if f != 0.0 => Some(pyjson::python_float(f)),
        _ => None,
    }
}

/// `rewrite_recorded` (`spans.py:491-509`): one span's recorded input and output (decoded,
/// output null when none) rewritten through `change(input, output)`, written only while the
/// span still holds what was read (`swap`): a live write landing in between makes it read and
/// change the span again. `held`: the span's stored (input, output) as the caller already read
/// them. `needles`: a span whose stored text (lowercased) holds none of them can't change, and
/// isn't decoded. A side `change` leaves alone keeps its stored text. Returns whether it wrote.
pub fn rewrite_recorded(
    db: &Db,
    ident: &str,
    mut change: impl FnMut(&Value, &Value) -> (Value, Value),
    needles: Option<&[String]>,
    held: Option<(String, Option<String>)>,
) -> Result<bool> {
    let step = |stored: &Vec<Sql>| -> Result<Vec<Sql>> {
        let Sql::Text(inp) = &stored[0] else {
            return Err(Error::Json("a span's input isn't text".to_owned()));
        };
        let out = match &stored[1] {
            Sql::Text(out) => Some(out.as_str()),
            _ => None,
        };
        let out_text = out.filter(|o| !o.is_empty());
        if let Some(needles) = needles {
            let hay = format!("{inp}{}", out_text.unwrap_or("")).to_lowercase();
            if !needles.iter().any(|n| hay.contains(n.as_str())) {
                return Ok(stored.clone());
            }
        }
        let before_in = loads(inp)?;
        let before_out = loads(out_text.unwrap_or("null"))?;
        let (new_in, new_out) = change(&before_in, &before_out);
        let (same_in, same_out) = (py_eq(&new_in, &before_in), py_eq(&new_out, &before_out));
        if same_in && same_out {
            return Ok(stored.clone());
        }
        Ok(vec![
            if same_in {
                stored[0].clone()
            } else {
                Sql::Text(dumps(&new_in))
            },
            if same_out {
                stored[1].clone()
            } else if new_out.is_null() {
                Sql::Null
            } else {
                Sql::Text(dumps(&new_out))
            },
        ])
    };
    let held = held.map(|(inp, out)| vec![Sql::Text(inp), out.map_or(Sql::Null, Sql::Text)]);
    let written = swap(
        db.conn(),
        "spans",
        "id",
        &Sql::Text(ident.to_owned()),
        &["input_json", "output_json"],
        step,
        held,
    )?;
    Ok(written.is_some())
}
