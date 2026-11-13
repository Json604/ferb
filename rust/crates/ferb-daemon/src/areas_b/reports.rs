//! The failure report the owner can send the developer (L35; DMN-8, DMN-9, DMN-28..32, DMN-503, DMN-505): one
//! plain-text file per turn FERB recorded as ending badly, under `~/.ferb/reports`, every part of it redacted; and the
//! diagnostic file. Port of `ferb/daemon/reports.py`.
//!
//! Must not judge why a task failed or send anything the owner didn't preview: FERB writes the file, the owner reads
//! it in FERB.app and presses Send, and only then the daemon sends that same text to the developer's report relay
//! (`relay/worker.mjs`, unchanged): `POST /file` with GitHub's create-a-file JSON, then `POST /report` with the
//! issue's fields and the relay's ticket. Raw stderr never goes in.
//!
//! Seams: the redaction (`walk`, the structural pass, and `text`, the text pass: the daemon's Redactor), FERB's and
//! macOS's versions ([`Versions`]), the Mac's local time (the caller passes `now` already in it: Python's
//! `now.astimezone()`), and the HTTP POST to the relay ([`RelayHttp`]; [`ReqwestRelay`] is the real one: reqwest
//! with rustls, the client ferb-runtimes already builds in, no FERB proxy, as Python's httpx).

use std::fs;
use std::io::{Read, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::LazyLock;

use base64::Engine;
use ferb_core::when::DateTime;
use ferb_py::json::{dumps, dumps_utf8, dumps_with, Style};
use ferb_py::value::{py_str, strip, truthy};
use ferb_store::Db;
use ferb_trace::lines;
use regex::Regex;
use rusqlite::types::Value as Sql;
use serde_json::{json, Map, Value};

/// Where Send sends a report (owner, 2026-11-01): the developer's relay; config `[feedback] relay_url` overrides it.
pub const RELAY_URL: &str = "https://ferb-reports.ferb-reports.workers.dev";
pub const NOT_SET_UP: &str = "Sending reports to the developer isn't set up yet";
/// One sent report file at most (owner, 2026-11-01; relay/worker.mjs MAX_REPORT).
pub const MAX_REPORT_BYTES: usize = 25_000_000;
/// Its base64 in GitHub's JSON (relay/worker.mjs MAX_UPLOAD).
pub const UPLOAD_MAX_BYTES: usize = MAX_REPORT_BYTES.div_ceil(3) * 4 + 4096;
/// The issue's summary at most (relay/worker.mjs MAX_SUMMARY).
pub const SUMMARY_CHARS: usize = 30_000;
/// Each connect, read or write step of a request to the relay.
pub const SEND_TIMEOUT_S: f64 = 60.0;
/// One whole Send, inside the app's wait.
pub const SEND_DEADLINE_S: f64 = 540.0;
/// The file again when another report's commit landed at the same moment.
pub const CONFLICT_TRIES: usize = 3;
pub const TITLE_CHARS: usize = 200;
pub const VERSION_CHARS: usize = 50;
/// The only hosts a plain-http relay address may name (tests).
pub const LOCAL_HOSTS: [&str; 3] = ["127.0.0.1", "localhost", "::1"];
/// DMN-503: a failure report older than 30 days is deleted.
pub const KEEP_DAYS: i64 = 30;
pub const REPORTABLE: [&str; 2] = ["failed", "interrupted"];
pub const TURN_FAILED: &str = "turn failed";
pub const SECTIONS: [&str; 4] = ["Summary", "Request", "Steps", "FERB's recorded problems"];
pub const INTERNAL_ERROR: &str = "daemon.internal_error";
pub const SLOW_CALL: &str = "daemon.slow_call";
pub const PROBLEMS_LAST: i64 = 20;
pub const SUBJECT_CHARS: usize = 80;
pub const FOLDER: &str = "reports";
pub const SETUP_EVENTS: [&str; 5] = [
    "setup.%",
    "browsers.%",
    "browser.handoff%",
    "sense.watcher_failed",
    "schedule.retry_queued",
];
pub const SETUP_EVENTS_LAST: i64 = 40;
pub const RECENT_TASKS: i64 = 20;
/// `_redacted_text`'s piece size.
pub const PIECE_CHARS: usize = 256 * 1024;
const SPAN_CONTENT: [&str; 4] = ["input", "output", "error", "summary"];
const STEPS: &str = "\n## Steps\n";
const PROBLEMS: &str = "\n## FERB's recorded problems\n";
const QUESTION_KINDS: [&str; 1] = ["model.question"];
const QUESTION_TOOLS: [&str; 1] = ["owner.ask"];

fn rx(p: &str) -> Regex {
    Regex::new(p).expect("a valid pattern")
}

static NAME: LazyLock<Regex> =
    LazyLock::new(|| rx(r"\AFERB-report-\d{4}-\d{2}-\d{2}-[A-Za-z0-9._-]+\.txt\z"));
static DIAGNOSTIC: LazyLock<Regex> =
    LazyLock::new(|| rx(r"\AFERB-diagnostic-\d{4}-\d{2}-\d{2}-\d{6}\.txt\z"));
static SAFE_ID: LazyLock<Regex> = LazyLock::new(|| rx(r"\A[A-Za-z0-9][A-Za-z0-9._-]*\z"));
static SPAN_LABEL: LazyLock<Regex> = LazyLock::new(|| rx(r"\A *\d{2}:\d{2}:\d{2}  "));
static LABEL: LazyLock<Regex> =
    LazyLock::new(|| rx(r"\A *(\d{2}:\d{2}:\d{2})  (\S+)(?:  (.*?))?  \[[^\]]*\]$"));
static FIRST_ERROR: LazyLock<Regex> = LazyLock::new(|| rx(r#""error": ("(?:[^"\\]|\\.)*")"#));
static QUESTION: LazyLock<Regex> = LazyLock::new(|| rx(r#""question": ("(?:[^"\\]|\\.)*")"#));
static ANSWER: LazyLock<Regex> = LazyLock::new(|| rx(r#""answer": ("(?:[^"\\]|\\.)*")"#));

/// Which file names a purge or a Send takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Names {
    Reports,
    Diagnostics,
}

impl Names {
    fn matches(self, name: &str) -> bool {
        match self {
            Names::Reports => NAME.is_match(name),
            Names::Diagnostics => DIAGNOSTIC.is_match(name),
        }
    }
}

/// Why a report call failed.
#[derive(Clone, Debug, PartialEq)]
pub enum ReportError {
    /// FERB's own refusal (`FerbError`): the owner reads its words.
    Refused(String),
    /// Why a report wasn't sent (`Unsent`); the file stays and Send can be pressed again.
    Unsent(String),
    /// Anything else Python lets propagate (an OS error, a database error): the daemon's internal error.
    Fault(String),
}

impl std::fmt::Display for ReportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ReportError::Refused(t) | ReportError::Unsent(t) | ReportError::Fault(t) => {
                f.write_str(t)
            }
        }
    }
}

impl std::error::Error for ReportError {}

pub type Result<T> = std::result::Result<T, ReportError>;

fn fault(e: impl std::fmt::Display) -> ReportError {
    ReportError::Fault(e.to_string())
}

/// FERB's version (`ferb.__version__`) and macOS's (`platform.mac_ver()[0]`, "" when unknown).
#[derive(Clone, Debug)]
pub struct Versions {
    pub ferb: String,
    pub macos: String,
}

impl Versions {
    fn mac(&self) -> &str {
        if self.macos.is_empty() {
            "unknown"
        } else {
            &self.macos
        }
    }
}

/// The redaction passes and what a build needs from the daemon.
pub struct Env<'a> {
    pub home: &'a Path,
    /// The clock's now in the Mac's local time zone (Python's `now.astimezone()`).
    pub now: DateTime,
    /// The structural redaction (the daemon's `redact(value)`).
    pub walk: &'a dyn Fn(&Value) -> Value,
    /// The text pass (`Redactor.text`).
    pub text: &'a dyn Fn(&str) -> String,
    pub app_version: &'a str,
    pub versions: &'a Versions,
}

/// What failed, from FERB's own records.
#[derive(Clone, Debug, PartialEq)]
pub struct Failure {
    pub task_id: String,
    pub status: String,
    pub reason: String,
    pub request: String,
    pub answer: String,
    pub started: Option<String>,
    pub ended: Option<String>,
    pub task_status: String,
}

/// A written report or diagnostic file: `{path, subject, summary}` (a diagnostic's dict is `{path, summary,
/// subject}`).
#[derive(Clone, Debug, PartialEq)]
pub struct Built {
    pub path: String,
    pub subject: String,
    pub summary: String,
    pub diagnostic: bool,
}

impl Built {
    pub fn to_value(&self) -> Value {
        if self.diagnostic {
            json!({"path": self.path, "summary": self.summary, "subject": self.subject})
        } else {
            json!({"path": self.path, "subject": self.subject, "summary": self.summary})
        }
    }
}

// ---------- what failed ----------

fn sql_text(v: &Sql) -> Option<String> {
    match v {
        Sql::Null => None,
        Sql::Text(t) => Some(t.clone()),
        Sql::Integer(i) => Some(i.to_string()),
        Sql::Real(f) => Some(ferb_py::json::python_float(*f)),
        Sql::Blob(b) => Some(format!(
            "b{}",
            ferb_trace::py::repr_str(&String::from_utf8_lossy(b))
        )),
    }
}

/// `str(value)` of a column (None is "None").
fn col(row: &[(String, Sql)], name: &str) -> String {
    row.iter()
        .find(|(k, _)| k == name)
        .and_then(|(_, v)| sql_text(v))
        .unwrap_or_else(|| "None".into())
}

fn col_opt(row: &[(String, Sql)], name: &str) -> Option<String> {
    row.iter()
        .find(|(k, _)| k == name)
        .and_then(|(_, v)| sql_text(v))
}

fn query(db: &Db, sql: &str, params: &[&dyn rusqlite::ToSql]) -> Result<Vec<Vec<(String, Sql)>>> {
    let mut st = db.conn().prepare(sql).map_err(fault)?;
    let names: Vec<String> = st.column_names().map(str::to_owned).collect();
    let rows = st
        .query_map(params, |r| {
            let mut out = vec![];
            for (i, n) in names.iter().enumerate() {
                out.push((n.clone(), r.get::<_, Sql>(i)?));
            }
            Ok(out)
        })
        .map_err(fault)?;
    rows.collect::<std::result::Result<Vec<_>, _>>()
        .map_err(fault)
}

fn loads(text: &str) -> Result<Value> {
    ferb_py::json::loads(text).map_err(|_| ReportError::Fault("JSONDecodeError".into()))
}

/// `str(d.get(key) or "")`.
fn get_text(d: &Value, key: &str) -> String {
    match d.get(key) {
        Some(v) if truthy(v) => py_str(v),
        _ => String::new(),
    }
}

/// `failure(db, task_id, request_id)` (review 1, I2): a report is built exactly when FERB recorded that the turn
/// ended badly: its root task's row is failed or interrupted, or its request has a `chat.failed` row.
pub fn failure(db: &Db, task_id: &str, request_id: &str) -> Result<Failure> {
    let given: Vec<&str> = [task_id, request_id]
        .into_iter()
        .filter(|i| !i.is_empty())
        .collect();
    let shown = if task_id.is_empty() {
        request_id
    } else {
        task_id
    };
    let unknown = ReportError::Refused(format!(
        "FERB has no failed task or request {}",
        ferb_trace::py::repr_str(shown)
    ));
    if given.is_empty() || !given.iter().all(|i| SAFE_ID.is_match(i)) {
        return Err(unknown);
    }
    let row = if task_id.is_empty() {
        None
    } else {
        query(db, "SELECT * FROM tasks WHERE id = ?", &[&task_id])?
            .into_iter()
            .next()
    };
    if let Some(r) = &row {
        let root = col_opt(r, "parent_id").is_none();
        if root && REPORTABLE.contains(&col(r, "status").as_str()) {
            let outcome = loads(
                col_opt(r, "outcome_json")
                    .filter(|s| !s.is_empty())
                    .as_deref()
                    .unwrap_or("{}"),
            )?;
            let (told, _) = told(db, task_id)?;
            let mut answer = get_text(&outcome, "answer");
            if answer.is_empty() {
                answer = told;
            }
            if answer.is_empty() {
                answer = get_text(&outcome, "summary");
            }
            let mut reason = get_text(&outcome, "summary");
            if reason.is_empty() {
                reason = answer.clone();
            }
            let mut request = message(db, &col(r, "request_id"))?;
            if request.is_empty() {
                request = col(r, "goal");
            }
            return Ok(Failure {
                task_id: task_id.to_owned(),
                status: col(r, "status"),
                reason,
                request,
                answer,
                started: col_opt(r, "created_at"),
                ended: col_opt(r, "updated_at"),
                task_status: String::new(),
            });
        }
    }
    let request = if !request_id.is_empty() {
        request_id.to_owned()
    } else if let Some(r) = &row {
        col(r, "request_id")
    } else {
        task_id.to_owned()
    };
    let failed = query(
        db,
        "SELECT at, data_json FROM journal WHERE kind = 'chat.failed' AND \
         json_extract(data_json, '$.request_id') = ? ORDER BY seq DESC LIMIT 1",
        &[&request],
    )?
    .into_iter()
    .next();
    let Some(failed) = failed else {
        if let Some(r) = &row {
            if col_opt(r, "parent_id").is_some() {
                let tree = col(r, "tree_id");
                return Err(ReportError::Refused(format!(
                    "task {task_id} is part of {tree}; send the report for {tree}"
                )));
            }
            return Err(ReportError::Refused(format!(
                "Only a failed or interrupted task can be sent to the developer; this one is {}.",
                col(r, "status").replace('_', " ")
            )));
        }
        return Err(unknown);
    };
    let reply = query(
        db,
        "SELECT data_json FROM journal WHERE kind = 'chat.reply' AND \
         json_extract(data_json, '$.request_id') = ? ORDER BY seq DESC LIMIT 1",
        &[&request],
    )?
    .into_iter()
    .next();
    let (told_answer, goal) = told(db, &request)?; // a scheduled run's: no chat message, no chat reply
    let mut answer = match &reply {
        Some(r) => get_text(&loads(&col(r, "data_json"))?, "text"),
        None => String::new(),
    };
    if answer.is_empty() {
        answer = told_answer;
    }
    let why = get_text(&loads(&col(&failed, "data_json"))?, "why");
    let asked = query(
        db,
        "SELECT at FROM journal WHERE kind IN ('chat.ack', 'chat.message') AND \
         json_extract(data_json, '$.request_id') = ? ORDER BY seq LIMIT 1",
        &[&request],
    )?
    .into_iter()
    .next();
    let mut req = message(db, &request)?;
    if req.is_empty() {
        req = goal;
    }
    if req.is_empty() {
        req = row.as_ref().map(|r| col(r, "goal")).unwrap_or_default();
    }
    Ok(Failure {
        task_id: row.as_ref().map_or(request.clone(), |r| col(r, "tree_id")),
        status: TURN_FAILED.into(),
        reason: if answer.is_empty() {
            why
        } else {
            answer.clone()
        },
        request: req,
        answer,
        started: asked.and_then(|a| col_opt(&a, "at")),
        ended: col_opt(&failed, "at"),
        task_status: row.as_ref().map(|r| col(r, "status")).unwrap_or_default(),
    })
}

/// `_message`: the owner's message for this request, as the daemon recorded it (redacted at capture).
fn message(db: &Db, request_id: &str) -> Result<String> {
    let row = query(
        db,
        "SELECT data_json FROM journal WHERE kind = 'chat.message' AND \
         json_extract(data_json, '$.request_id') = ? ORDER BY seq LIMIT 1",
        &[&request_id],
    )?
    .into_iter()
    .next();
    match row {
        Some(r) => Ok(get_text(&loads(&col(&r, "data_json"))?, "text")),
        None => Ok(String::new()),
    }
}

/// `_told`: what FERB told the owner about this task's end, and its goal (`task_results`).
fn told(db: &Db, task_id: &str) -> Result<(String, String)> {
    let row = query(
        db,
        "SELECT answer, goal FROM task_results WHERE task_id = ?",
        &[&task_id],
    )?
    .into_iter()
    .next();
    Ok(match row {
        Some(r) => (
            col_opt(&r, "answer").unwrap_or_default(),
            col_opt(&r, "goal").unwrap_or_default(),
        ),
        None => (String::new(), String::new()),
    })
}

/// One recorded problem or event: (at, kind, data).
pub type Row = (String, String, Value);

fn rows_of(found: Vec<Vec<(String, Sql)>>) -> Result<Vec<Row>> {
    found
        .into_iter()
        .map(|r| {
            Ok((
                col(&r, "at"),
                col(&r, "kind"),
                loads(&col(&r, "data_json"))?,
            ))
        })
        .collect()
}

/// `problems(db, started, ended)` (DMN-505): FERB's recorded problems from the task's time, and the latest
/// PROBLEMS_LAST overall, oldest first.
pub fn problems(db: &Db, started: Option<&str>, ended: Option<&str>) -> Result<Vec<Row>> {
    let mut found: std::collections::BTreeMap<i64, Vec<(String, Sql)>> = Default::default();
    let seq = |r: &[(String, Sql)]| match r.iter().find(|(k, _)| k == "seq") {
        Some((_, Sql::Integer(i))) => *i,
        _ => 0,
    };
    if let (Some(s), Some(e)) = (
        started.filter(|s| !s.is_empty()),
        ended.filter(|s| !s.is_empty()),
    ) {
        for r in query(
            db,
            "SELECT seq, at, kind, data_json FROM journal WHERE kind IN (?, ?) AND at >= ? AND at <= ?",
            &[&INTERNAL_ERROR, &SLOW_CALL, &s, &e],
        )? {
            found.insert(seq(&r), r);
        }
    }
    for r in query(
        db,
        "SELECT seq, at, kind, data_json FROM journal WHERE kind IN (?, ?) ORDER BY seq DESC LIMIT ?",
        &[&INTERNAL_ERROR, &SLOW_CALL, &PROBLEMS_LAST],
    )? {
        found.insert(seq(&r), r);
    }
    rows_of(found.into_values().collect())
}

/// `setup_events(db)`: the latest SETUP_EVENTS_LAST setup, pairing and watcher journal entries, oldest first.
pub fn setup_events(db: &Db) -> Result<Vec<Row>> {
    let wh = vec!["kind LIKE ?"; SETUP_EVENTS.len()].join(" OR ");
    let sql =
        format!("SELECT at, kind, data_json FROM journal WHERE {wh} ORDER BY seq DESC LIMIT ?");
    let mut params: Vec<&dyn rusqlite::ToSql> = SETUP_EVENTS
        .iter()
        .map(|p| p as &dyn rusqlite::ToSql)
        .collect();
    params.push(&SETUP_EVENTS_LAST);
    let mut rows = rows_of(query(db, &sql, &params)?)?;
    rows.reverse();
    Ok(rows)
}

// ---------- the report text ----------

struct NoClock;

impl ferb_store::spans::Clock for NoClock {
    fn now(&self) -> ferb_py::time::DateTime {
        ferb_py::time::DateTime::utc_from_unix(0, 0)
    }
}

struct NoIds;

impl ferb_store::spans::IdSource for NoIds {
    fn new_id(&self) -> String {
        String::new()
    }
}

/// `report(...)`: what failed, its trace and its problems read on a connection of its own, then `build` writes the
/// file. (Python's clock and ids only build the span store, which this read never uses.)
pub fn report(
    db_path: &Path,
    task_id: &str,
    request_id: &str,
    facts: &Value,
    env: &Env<'_>,
) -> Result<(Failure, Built)> {
    let db = Db::open(db_path).map_err(fault)?;
    let found = failure(&db, task_id, request_id)?;
    let spans = ferb_store::spans::SpanStore::new(
        &db,
        Box::new(NoClock),
        Box::new(NoIds),
        Box::new(|v: &Value| v.clone()),
        Box::new(|_: &str| false),
    )
    .tree_json(&found.task_id)
    .map_err(fault)?;
    let rows = problems(&db, found.started.as_deref(), found.ended.as_deref())?;
    drop(db);
    let built = build(&found, &spans, &rows, facts, env)?;
    Ok((found, built))
}

/// `"; ".join(facts.get(key) or []) or "unknown"`.
fn joined(facts: &Value, key: &str) -> String {
    let parts: Vec<String> = match facts.get(key) {
        Some(Value::Array(a)) => a.iter().map(py_str).collect(),
        _ => vec![],
    };
    let s = parts.join("; ");
    if s.is_empty() {
        "unknown".into()
    } else {
        s
    }
}

/// `(fromisoformat(ended) - fromisoformat(started)).total_seconds()`, None when either can't be read or only one
/// has an offset (Python's TypeError).
fn seconds_between(started: Option<&str>, ended: Option<&str>) -> Option<f64> {
    use ferb_py::time::DateTime as PyTime;
    let (s, e) = (started.unwrap_or(""), ended.unwrap_or(""));
    let (a, b) = (
        PyTime::from_isoformat(s).ok()?,
        PyTime::from_isoformat(e).ok()?,
    );
    if a.offset.is_some() != b.offset.is_some() {
        return None;
    }
    ferb_trace::py::total_seconds(s, e)
}

/// `_redacted(node, walk)`: a span and its children with their recorded content through the structural redaction.
fn redacted_span(node: &Value, walk: &dyn Fn(&Value) -> Value) -> Value {
    let mut own = Map::new();
    if let Value::Object(m) = node {
        for (k, v) in m {
            if k == "children" {
                continue;
            }
            own.insert(
                k.clone(),
                if SPAN_CONTENT.contains(&k.as_str()) {
                    walk(v)
                } else {
                    v.clone()
                },
            );
        }
    }
    let children: Vec<Value> = match node.get("children") {
        Some(Value::Array(c)) => c.iter().map(|c| redacted_span(c, walk)).collect(),
        _ => vec![],
    };
    own.insert("children".into(), Value::Array(children));
    Value::Object(own)
}

/// `_blocks(rendered)`: one block per span (its label line and the lines under it).
fn blocks<S: AsRef<str>>(rendered: &[S]) -> Vec<Vec<String>> {
    let mut out = vec![];
    let mut block: Vec<String> = vec![];
    for line in rendered {
        let line = line.as_ref();
        if !block.is_empty() && SPAN_LABEL.is_match(line) {
            out.push(std::mem::take(&mut block));
        }
        block.push(line.to_owned());
    }
    if !block.is_empty() {
        out.push(block);
    }
    out
}

fn row_line(at: &str, kind: &str, data: &Value, env: &Env<'_>) -> String {
    (env.text)(&format!("{at}  {kind}  {}", dumps_utf8(&(env.walk)(data))))
}

/// `build(...)`: writes the report and returns {path, subject, summary}. Every part is redacted (rule 7): the trace
/// and the problem rows structurally before they are rendered, then each span's block and each section as text.
/// Reports older than KEEP_DAYS go first.
pub fn build(
    found: &Failure,
    spans: &[Value],
    rows: &[Row],
    facts: &Value,
    env: &Env<'_>,
) -> Result<Built> {
    let text = env.text;
    let seconds = seconds_between(found.started.as_deref(), found.ended.as_deref());
    let mut summary: Vec<(String, String)> = vec![
        ("FERB version".into(), env.versions.ferb.clone()),
        ("macOS version".into(), env.versions.mac().into()),
    ];
    if !env.app_version.is_empty() {
        summary.push(("Notch version".into(), lines::clip(env.app_version, 60)));
    }
    summary.push(("Extension version".into(), joined(facts, "extensions")));
    summary.push(("AI runtime and model".into(), joined(facts, "runtimes")));
    summary.push(("Task".into(), found.task_id.clone()));
    summary.push(("Status".into(), found.status.clone()));
    if !found.task_status.is_empty() {
        summary.push(("Task status".into(), found.task_status.clone()));
    }
    let reason = strip(&found.reason);
    summary.push((
        "Failure reason".into(),
        if reason.is_empty() {
            "(none recorded)".into()
        } else {
            reason.into()
        },
    ));
    summary.push((
        "Started".into(),
        found
            .started
            .clone()
            .filter(|s| !s.is_empty())
            .unwrap_or("unknown".into()),
    ));
    summary.push((
        "Ended".into(),
        found
            .ended
            .clone()
            .filter(|s| !s.is_empty())
            .unwrap_or("unknown".into()),
    ));
    summary.push((
        "Duration".into(),
        seconds.map_or("unknown".into(), |s| lines::duration(Some(s))),
    ));
    let trace = json!({"task_id": found.task_id,
                       "spans": spans.iter().map(|n| redacted_span(n, env.walk)).collect::<Vec<_>>()});
    let rendered = ferb_trace::render_trace(&trace, true);
    let steps: Vec<String> = blocks(&rendered)
        .iter()
        .map(|b| text(&b.join("\n")))
        .collect();
    let recorded: Vec<String> = rows
        .iter()
        .map(|(a, k, d)| row_line(a, k, d, env))
        .collect();
    let pairs = |items: &[(String, String)]| {
        items
            .iter()
            .map(|(k, v)| format!("{k}: {v}"))
            .collect::<Vec<_>>()
            .join("\n")
    };
    let or = |s: &str, d: &str| {
        if s.is_empty() {
            d.to_owned()
        } else {
            s.to_owned()
        }
    };
    let mut parts: Vec<String> = vec![
        text(&format!("FERB report for {}", found.task_id)),
        String::new(),
        "## Summary".into(),
        text(&pairs(&summary)),
        String::new(),
        "## Request".into(),
        text(
            &[
                "Your request:".to_owned(),
                or(strip(&found.request), "(not recorded)"),
                String::new(),
                "FERB's answer:".to_owned(),
                or(strip(&found.answer), "(none recorded)"),
            ]
            .join("\n"),
        ),
        String::new(),
        "## Steps".into(),
    ];
    parts.extend(steps);
    parts.push(String::new());
    parts.push("## FERB's recorded problems".into());
    parts.push(format!(
        "FERB's own records of requests that failed inside it or answered slowly: those during this task, and the \
         latest {PROBLEMS_LAST}."
    ));
    if recorded.is_empty() {
        parts.push("(none recorded)".into());
    } else {
        parts.extend(recorded);
    }
    parts.push(String::new());
    let reason = text(&found.reason);
    let short = ferb_trace::py::splitlines(&reason)
        .into_iter()
        .map(strip)
        .find(|l| !l.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| found.status.clone());
    let folder = env.home.join(FOLDER);
    purge(&folder, &env.now, KEEP_DAYS, Names::Reports)?;
    let name = format!(
        "FERB-report-{}-{}.txt",
        env.now.format("%Y-%m-%d"),
        found.task_id
    );
    let path = write(&folder, &name, &parts.join("\n"))?;
    let told = text(
        &[
            format!("What happened: {}", lines::clip(&short, 300)),
            format!("Task: {} ({})", found.task_id, found.status),
            format!("FERB {}, macOS {}", env.versions.ferb, env.versions.mac()),
        ]
        .join("\n"),
    );
    Ok(Built {
        path: path.to_string_lossy().into_owned(),
        subject: format!("FERB report: {}", lines::clip(&short, SUBJECT_CHARS)),
        summary: told,
        diagnostic: false,
    })
}

/// `diagnostic(...)` (L34 Phase E): versions, the daemon's health check, what it records, its latest tasks, its
/// recorded problems and its recent setup, pairing and watcher events, every part redacted. Nothing is sent here.
pub fn diagnostic(
    db_path: &Path,
    health: &Value,
    recording: &Value,
    facts: &Value,
    env: &Env<'_>,
) -> Result<Built> {
    let text = env.text;
    let db = Db::open(db_path).map_err(fault)?;
    let tasks = query(
        &db,
        "SELECT id, status, created_at FROM tasks WHERE parent_id IS NULL ORDER BY created_at DESC, rowid DESC \
         LIMIT ?",
        &[&RECENT_TASKS],
    )?;
    let rows = problems(&db, None, None)?;
    let events = setup_events(&db)?;
    drop(db);
    let mac = env.versions.mac().to_owned();
    let summary = [
        ("FERB version", env.versions.ferb.clone()),
        ("macOS version", mac.clone()),
        (
            "App version",
            if env.app_version.is_empty() {
                "unknown".into()
            } else {
                lines::clip(env.app_version, 60)
            },
        ),
        ("Extension version", joined(facts, "extensions")),
        ("AI runtime and model", joined(facts, "runtimes")),
        (
            "Written",
            env.now.format("%Y-%m-%dT%H:%M:%S%:z").to_string(),
        ),
    ];
    let shown = |v: &Value| {
        text(&dumps_with(
            &(env.walk)(v),
            Style {
                ascii: false,
                sort_keys: true,
                indent: Some(2),
            },
        ))
    };
    let or_none = |v: Vec<String>| {
        if v.is_empty() {
            vec!["(none recorded)".to_owned()]
        } else {
            v
        }
    };
    let mut parts: Vec<String> = vec![
        text("FERB diagnostic file"),
        String::new(),
        "## Summary".into(),
        text(
            &summary
                .iter()
                .map(|(k, v)| format!("{k}: {v}"))
                .collect::<Vec<_>>()
                .join("\n"),
        ),
        String::new(),
        "## Health".into(),
        shown(health),
        String::new(),
        "## Recording".into(),
        shown(recording),
        String::new(),
        "## Recent tasks".into(),
    ];
    parts.extend(or_none(
        tasks
            .iter()
            .map(|t| {
                text(&format!(
                    "{}  {}  {}",
                    col(t, "created_at"),
                    col(t, "id"),
                    col(t, "status")
                ))
            })
            .collect(),
    ));
    parts.push(String::new());
    parts.push("## FERB's recorded problems".into());
    parts.push(format!(
        "FERB's own records of requests that failed inside it or answered slowly: the latest {PROBLEMS_LAST}."
    ));
    parts.extend(or_none(
        rows.iter()
            .map(|(a, k, d)| row_line(a, k, d, env))
            .collect(),
    ));
    parts.push(String::new());
    parts.push("## Recent setup, pairing and watcher events".into());
    parts.extend(or_none(
        events
            .iter()
            .map(|(a, k, d)| row_line(a, k, d, env))
            .collect(),
    ));
    parts.push(String::new());
    let folder = env.home.join(FOLDER);
    purge(&folder, &env.now, KEEP_DAYS, Names::Diagnostics)?;
    let name = format!("FERB-diagnostic-{}.txt", env.now.format("%Y-%m-%d-%H%M%S"));
    let path = write(&folder, &name, &parts.join("\n"))?;
    let told = text(&format!("FERB {}, macOS {mac}", env.versions.ferb));
    Ok(Built {
        path: path.to_string_lossy().into_owned(),
        subject: format!("FERB diagnostics ({told})"),
        summary: told,
        diagnostic: true,
    })
}

// ---------- the files ----------

fn timestamp(dt: &DateTime) -> f64 {
    dt.timestamp() as f64 + f64::from(dt.timestamp_subsec_micros()) / 1e6
}

/// `purge(folder, now, days, names)` (DMN-503): removes FERB's own files of one kind last written more than `days`
/// ago; how many.
pub fn purge(folder: &Path, now: &DateTime, days: i64, names: Names) -> Result<usize> {
    if !folder.is_dir() {
        return Ok(0);
    }
    let cutoff = timestamp(now) - (days * 86400) as f64;
    let mut gone = 0;
    for entry in fs::read_dir(folder).map_err(fault)? {
        let entry = entry.map_err(fault)?;
        let p = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if !names.matches(&name) {
            continue;
        }
        let Ok(meta) = fs::symlink_metadata(&p) else {
            continue;
        };
        if meta.file_type().is_symlink() || !meta.is_file() {
            continue;
        }
        let mtime = meta.mtime() as f64 + meta.mtime_nsec() as f64 / 1e9;
        if mtime < cutoff {
            match fs::remove_file(&p) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(fault(e)),
            }
            gone += 1;
        }
    }
    Ok(gone)
}

static TMP_COUNTER: AtomicU64 = AtomicU64::new(0);

fn tmp_name() -> String {
    use std::hash::{BuildHasher, Hasher};
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u64(TMP_COUNTER.fetch_add(1, Ordering::Relaxed));
    h.write_u128(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos()),
    );
    format!(".report.{:016x}.txt", h.finish())
}

/// `write(folder, name, text)`: the folder owner-only (0700), the file 0600, written whole (a temporary file in the
/// same folder, then renamed).
pub fn write(folder: &Path, name: &str, text: &str) -> Result<PathBuf> {
    fs::create_dir_all(folder).map_err(fault)?;
    fs::set_permissions(folder, fs::Permissions::from_mode(0o700)).map_err(fault)?;
    let (tmp, mut f) = loop {
        let tmp = folder.join(tmp_name());
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&tmp)
        {
            Ok(f) => break (tmp, f),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(fault(e)),
        }
    };
    let done = (|| -> std::io::Result<()> {
        f.write_all(text.as_bytes())?;
        f.flush()?;
        f.sync_all()?;
        fs::set_permissions(&tmp, fs::Permissions::from_mode(0o600))?;
        fs::rename(&tmp, folder.join(name))
    })();
    if let Err(e) = done {
        let _ = fs::remove_file(&tmp);
        return Err(fault(e));
    }
    Ok(folder.join(name))
}

/// A path as Python's `PurePosixPath` reads it: its root ("", "/" or "//") and its parts ("" and "." dropped).
fn pure(path: &str) -> (&'static str, Vec<&str>) {
    let root = if path.starts_with("//") && !path.starts_with("///") {
        "//"
    } else if path.starts_with('/') {
        "/"
    } else {
        ""
    };
    let parts = path
        .split('/')
        .filter(|p| !p.is_empty() && *p != ".")
        .collect();
    (root, parts)
}

fn joined_path(root: &str, parts: &[&str]) -> String {
    let body = parts.join("/");
    if root.is_empty() && body.is_empty() {
        ".".into()
    } else {
        format!("{root}{body}")
    }
}

/// `saved(home, path)`: a report or diagnostic file FERB wrote, by the path `build`/`diagnostic` returned: only a
/// regular file directly in `~/.ferb/reports` with one of their names (DMN-28); (path, "failure" | "diagnostic").
pub fn saved(home: &Path, path: &str) -> Result<(PathBuf, &'static str)> {
    let only = || {
        ReportError::Refused(
            "FERB can send only a report or diagnostic file it wrote in ~/.ferb/reports".into(),
        )
    };
    let (root, parts) = pure(path);
    let name = parts.last().copied().unwrap_or("");
    let kind = if NAME.is_match(name) {
        "failure"
    } else if DIAGNOSTIC.is_match(name) {
        "diagnostic"
    } else {
        return Err(only());
    };
    let folder = home.join(FOLDER);
    let folder = folder.to_string_lossy();
    let (froot, fparts) = pure(&folder);
    let parent = &parts[..parts.len().saturating_sub(1)];
    if root != froot || parent != fparts.as_slice() || root.is_empty() {
        return Err(only());
    }
    let p = PathBuf::from(joined_path(root, &parts));
    match fs::symlink_metadata(&p) {
        Ok(m) if m.file_type().is_file() => Ok((p, kind)),
        Ok(_) => Err(only()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err(ReportError::Refused(
            "That report is no longer saved; send it again from where it was offered".into(),
        )),
        Err(e) => Err(fault(e)),
    }
}

// ---------- the relay address ----------

struct Split {
    scheme: String,
    netloc: String,
    path: String,
}

/// `urllib.parse.urlsplit` (Python 3.12) as far as `relay_url` reads it; Err where it raises ValueError.
fn urlsplit(url: &str) -> std::result::Result<Split, ()> {
    let url: String = url
        .trim_start_matches(|c: char| (c as u32) <= 0x20)
        .chars()
        .filter(|c| !matches!(c, '\t' | '\r' | '\n'))
        .collect();
    let mut rest = url.as_str();
    let mut scheme = String::new();
    if let Some(i) = rest.find(':') {
        let s = &rest[..i];
        if i > 0
            && s.starts_with(|c: char| c.is_ascii_alphabetic())
            && s.chars()
                .all(|c| c.is_ascii_alphanumeric() || "+-.".contains(c))
        {
            scheme = s.to_ascii_lowercase();
            rest = &rest[i + 1..];
        }
    }
    let mut netloc = String::new();
    if let Some(r) = rest.strip_prefix("//") {
        let end = r.find(['/', '?', '#']).unwrap_or(r.len());
        netloc = r[..end].to_owned();
        rest = &r[end..];
        if netloc.contains('[') != netloc.contains(']') {
            return Err(()); // Invalid IPv6 URL
        }
    }
    let rest = rest.split('#').next().unwrap_or("");
    let path = rest.split('?').next().unwrap_or("").to_owned();
    if let Some((_, after)) = netloc.split_once('[') {
        let host = after.split(']').next().unwrap_or("");
        let ok = if host.starts_with('v') {
            Regex::new(r"\Av[a-fA-F0-9]+\..+\z")
                .map(|r| r.is_match(host))
                .unwrap_or(false)
        } else {
            host.parse::<std::net::Ipv6Addr>().is_ok()
        };
        if !ok {
            return Err(());
        }
    }
    Ok(Split {
        scheme,
        netloc,
        path,
    })
}

impl Split {
    fn hostinfo(&self) -> &str {
        self.netloc
            .rsplit_once('@')
            .map_or(self.netloc.as_str(), |(_, h)| h)
    }
    /// `.hostname`: lowercased, brackets removed; "" for none.
    fn hostname(&self) -> String {
        let info = self.hostinfo();
        let host = match info.split_once('[') {
            Some((_, after)) => after.split(']').next().unwrap_or(""),
            None => info.split(':').next().unwrap_or(""),
        };
        let (h, zone) = match host.split_once('%') {
            Some((h, z)) => (h.to_lowercase(), format!("%{z}")),
            None => (host.to_lowercase(), String::new()),
        };
        format!("{h}{zone}")
    }
    /// (username, password) as the truthiness Python's checks see.
    fn userinfo(&self) -> (String, String) {
        match self.netloc.rsplit_once('@') {
            Some((info, _)) => match info.split_once(':') {
                Some((u, p)) => (u.to_owned(), p.to_owned()),
                None => (info.to_owned(), String::new()),
            },
            None => (String::new(), String::new()),
        }
    }
}

/// `relay_url(configured)` (DMN-31): config `[feedback] relay_url`, else RELAY_URL; the relay's own address
/// without a trailing `/report`. An https address, or plain http to this Mac only.
pub fn relay_url(configured: &Value) -> Result<String> {
    let raw = if truthy(configured) {
        py_str(configured)
    } else {
        RELAY_URL.to_owned()
    };
    let url = strip(&raw);
    if url.is_empty() {
        return Err(ReportError::Unsent(NOT_SET_UP.into()));
    }
    let bad = || {
        ReportError::Unsent(
            "The report relay address in config.toml ([feedback] relay_url) isn't a valid https address".into(),
        )
    };
    let parts = urlsplit(url).map_err(|_| bad())?;
    let host = parts.hostname();
    let (user, password) = parts.userinfo();
    let scheme_ok =
        parts.scheme == "https" || (parts.scheme == "http" && LOCAL_HOSTS.contains(&host.as_str()));
    if host.is_empty() || !user.is_empty() || !password.is_empty() || !scheme_ok {
        return Err(bad());
    }
    let path = parts.path.trim_end_matches('/');
    let path = path.strip_suffix("/report").unwrap_or(path);
    Ok(format!("{}://{}{path}", parts.scheme, parts.netloc))
}

// ---------- reading a report's Steps (DMN-29: structural facts only) ----------

/// `_steps(body)`: where the Steps section's blocks start and end (byte offsets), and its blocks.
fn steps_of(body: &str) -> Option<(usize, usize, Vec<String>)> {
    let at = body.find(STEPS)?;
    let start = at + STEPS.len();
    let mut end = body[start..]
        .find("\n## ")
        .map_or(body.len(), |e| start + e);
    while end > start && body.as_bytes()[end - 1] == b'\n' {
        end -= 1;
    }
    let lines: Vec<&str> = body[start..end].split('\n').collect();
    Some((
        start,
        end,
        blocks(&lines).into_iter().map(|b| b.join("\n")).collect(),
    ))
}

/// `_label(block)`: (time, span kind, tool or goal) of a Steps block's first line.
fn label(block: &str) -> (String, String, String) {
    let first = block.split('\n').next().unwrap_or("");
    match LABEL.captures(first) {
        Some(c) => (
            c[1].to_owned(),
            c[2].to_owned(),
            c.get(3).map_or(String::new(), |m| m.as_str().to_owned()),
        ),
        None => Default::default(),
    }
}

fn is_question(block: &str) -> bool {
    let (_, kind, name) = label(block);
    QUESTION_KINDS.contains(&kind.as_str())
        || (kind == "tool.call" && QUESTION_TOOLS.contains(&name.as_str()))
}

/// `HAS_ERROR`: `"error": ` not followed by `null` as a whole word.
fn has_error(block: &str) -> bool {
    const KEY: &str = "\"error\": ";
    let mut from = 0;
    while let Some(i) = block[from..].find(KEY) {
        let after = &block[from + i + KEY.len()..];
        let null = after.strip_prefix("null").is_some_and(|rest| {
            !rest
                .chars()
                .next()
                .is_some_and(ferb_sanitize::parity::word_char)
        });
        if !null {
            return true;
        }
        from += i + 1;
    }
    false
}

/// `_string(pattern, block)`: the first match's JSON string read, or the raw match when it can't be.
fn string_of(pattern: &Regex, block: &str) -> String {
    let Some(c) = pattern.captures(block) else {
        return String::new();
    };
    let raw = &c[1];
    match ferb_py::json::loads(raw) {
        Ok(v) => py_str(&v),
        Err(_) => raw.to_owned(),
    }
}

/// `f"{x:.1f}"`.
fn one_decimal(x: f64) -> String {
    format!("{x:.1}")
}

/// Python's `bytes[a:b]` with negative and out-of-range bounds.
fn py_slice(data: &[u8], start: i64, end: i64) -> &[u8] {
    let n = data.len() as i64;
    let fix = |i: i64| if i < 0 { (n + i).max(0) } else { i.min(n) };
    let (a, b) = (fix(start), fix(end));
    if a >= b {
        &[]
    } else {
        &data[a as usize..b as usize]
    }
}

/// `bytes.decode(errors="ignore")`.
fn decode_ignore(mut data: &[u8]) -> String {
    let mut out = String::new();
    loop {
        match std::str::from_utf8(data) {
            Ok(s) => {
                out.push_str(s);
                return out;
            }
            Err(e) => {
                let ok = e.valid_up_to();
                out.push_str(std::str::from_utf8(&data[..ok]).unwrap_or(""));
                let skip = e.error_len().unwrap_or(data.len() - ok);
                data = &data[ok + skip..];
            }
        }
    }
}

/// `shorten(body, limit, name)` (DMN-9, DMN-29): a report cut to at most `limit` bytes, and the note at its top
/// saying what was cut ("" when it fits whole). Kept: everything outside Steps, every step with an error and every
/// question, and as many of the last steps as fit; each run of cut steps leaves a line. When even that is too big,
/// the middle is cut and the beginning and the end kept.
pub fn shorten(body: &str, limit: usize, name: &str) -> (String, String) {
    let size = body.len();
    if size <= limit {
        return (body.to_owned(), String::new());
    }
    let mb = limit / 1_000_000;
    let kept_as = if name.is_empty() {
        String::new()
    } else {
        format!(" The full report is saved on the sender's Mac as {name}.")
    };
    if let Some((start, end, blocks)) = steps_of(body) {
        let mut keep: Vec<bool> = blocks
            .iter()
            .enumerate()
            .map(|(i, b)| i == 0 || has_error(b) || is_question(b))
            .collect();
        let sizes: Vec<i64> = blocks.iter().map(|b| b.len() as i64 + 1).collect();
        let kept = keep.iter().filter(|k| **k).count() as i64;
        let mut budget =
            limit as i64 - start as i64 - (body.len() - end) as i64 - 4096 - 80 * (kept + 2);
        budget -= sizes
            .iter()
            .zip(&keep)
            .filter(|(_, k)| **k)
            .map(|(n, _)| n)
            .sum::<i64>();
        if budget >= 0 {
            for i in (1..blocks.len()).rev() {
                if keep[i] {
                    continue;
                }
                if sizes[i] > budget {
                    break;
                }
                keep[i] = true;
                budget -= sizes[i];
            }
            let cut_line = |n: usize| format!("[FERB cut {n} steps here to fit {mb} MB]");
            let mut out: Vec<String> = vec![];
            let (mut run, mut cut, mut cut_bytes) = (0usize, 0usize, 0i64);
            for ((block, k), n) in blocks.iter().zip(&keep).zip(&sizes) {
                if *k {
                    if run > 0 {
                        out.push(cut_line(run));
                    }
                    out.push(block.clone());
                    run = 0;
                } else {
                    run += 1;
                    cut += 1;
                    cut_bytes += n;
                }
            }
            if run > 0 {
                out.push(cut_line(run));
            }
            let note = format!(
                "[Shortened to fit {mb} MB: {cut} of {} steps ({} MB) were cut from Steps. The summary, the request, \
                 every error and question, FERB's recorded problems and the last steps are kept.{kept_as}]",
                blocks.len() - 1,
                one_decimal(cut_bytes as f64 / 1_000_000.0)
            );
            let text = format!(
                "{note}\n\n{}{}{}",
                &body[..start],
                out.join("\n"),
                &body[end..]
            );
            if text.len() <= limit {
                return (text, note);
            }
        }
    }
    let data = body.as_bytes();
    let size = size as i64;
    let room = limit as i64 - 2048;
    let head = room.div_euclid(5);
    let gone = one_decimal((size - room) as f64 / 1_000_000.0);
    let note = format!(
        "[Shortened to fit {mb} MB: {gone} MB were cut from the middle; the beginning (the summary) and the end are \
         kept.{kept_as}]"
    );
    let cut_line = format!("\n[FERB cut {gone} MB here to fit {mb} MB]\n");
    let text = format!(
        "{note}\n\n{}{cut_line}{}",
        decode_ignore(py_slice(data, 0, head)),
        decode_ignore(py_slice(data, size - (room - head), i64::MAX))
    );
    (text, note)
}

fn chars(s: &str) -> usize {
    s.chars().count()
}

/// `summarize(body, note, limit)` (DMN-29): the issue's short summary, from the report's own redacted text.
pub fn summarize(body: &str, note: &str, limit: usize) -> String {
    let mut out: Vec<String> = if note.is_empty() {
        vec![]
    } else {
        vec![note.to_owned(), String::new()]
    };
    const SUMMARY: &str = "\n## Summary\n";
    if let Some(at) = body.find(SUMMARY) {
        let start = at + SUMMARY.len();
        let end = body[start..].find("\n\n").map_or(body.len(), |e| start + e);
        out.extend(body[start..end].split('\n').map(str::to_owned));
    }
    let mut groups: Vec<(String, Vec<String>)> = vec![];
    if let Some((_, _, blocks)) = steps_of(body) {
        let (mut errors, mut questions) = (vec![], vec![]);
        for block in blocks.iter().skip(1) {
            let (when, kind, name) = label(block);
            let what = format!("{when}  {kind}  {name}");
            let what = what.trim_end_matches(ferb_py::value::is_py_space);
            if has_error(block) {
                let e = string_of(&FIRST_ERROR, block);
                let e = if e.is_empty() {
                    "(error recorded)".to_owned()
                } else {
                    e
                };
                errors.push(format!("- {what}: {}", lines::clip(&e, 300)));
            }
            if is_question(block) {
                let q = string_of(&QUESTION, block);
                let q = if q.is_empty() {
                    "(question not recorded)".to_owned()
                } else {
                    q
                };
                let answer = string_of(&ANSWER, block);
                let tail = if answer.is_empty() {
                    String::new()
                } else {
                    format!(" → {}", lines::clip(&answer, 200))
                };
                questions.push(format!("- {what}: {}{tail}", lines::clip(&q, 300)));
            }
        }
        groups.push((format!("Errors ({})", errors.len()), errors));
        groups.push((format!("Questions ({})", questions.len()), questions));
    }
    if let Some(at) = body.find(PROBLEMS) {
        let start = at + PROBLEMS.len();
        let end = body[start..]
            .find("\n## ")
            .map_or(body.len(), |e| start + e);
        let rows: Vec<String> = body[start..end]
            .split('\n')
            .skip(1)
            .filter(|r| !strip(r).is_empty() && *r != "(none recorded)")
            .map(|r| format!("- {}", lines::clip(r, 300)))
            .collect();
        groups.push((format!("FERB's recorded problems ({})", rows.len()), rows));
    }
    let mut size: usize = out.iter().map(|l| chars(l) + 1).sum();
    for (title, items) in groups {
        out.push(String::new());
        size += chars(&title) + 2;
        out.push(title);
        for (n, item) in items.iter().enumerate() {
            if size + chars(item) + 1 > limit.saturating_sub(200) {
                out.push(format!("… {} more in the full report", items.len() - n));
                size += 40;
                break;
            }
            size += chars(item) + 1;
            out.push(item.clone());
        }
    }
    strip(&out.join("\n")).chars().take(limit).collect()
}

/// `_redacted_text(written, text)`: a saved file through the text pass in pieces cut only where `build` started a
/// new piece (a step's block or a section), so no piece `build` redacted whole is split.
pub fn redacted_text(written: &str, text: &dyn Fn(&str) -> String, piece_chars: usize) -> String {
    let mut pieces: Vec<String> = vec![];
    let mut piece: Vec<&str> = vec![];
    let mut size = 0usize;
    for line in written.split('\n') {
        if size > piece_chars && (SPAN_LABEL.is_match(line) || line.starts_with("## ")) {
            pieces.push(text(&piece.join("\n")));
            piece.clear();
            size = 0;
        }
        piece.push(line);
        size += chars(line) + 1;
    }
    pieces.push(text(&piece.join("\n")));
    pieces.join("\n")
}

// ---------- the Send ----------

/// One Send, ready (`Outgoing`): the file as GitHub's create-a-file JSON and the issue's fields but the ticket.
#[derive(Clone, Debug, PartialEq)]
pub struct Outgoing {
    pub kind: String,
    pub report_id: String,
    pub file: Vec<u8>,
    pub issue: Map<String, Value>,
    pub shortened: bool,
}

/// `str.isprintable()` for one character.
fn printable(c: char) -> bool {
    if c.is_ascii() {
        (' '..='~').contains(&c)
    } else {
        ferb_trace::py::repr_str(&c.to_string()) == format!("'{c}'")
    }
}

/// `prepare(path, kind, title, app_version, text)` (DMN-28): the saved file the owner previewed, through the text
/// pass once more (and the saved file rewritten to match), shortened past `max_report_bytes` (MAX_REPORT_BYTES; the
/// saved file stays whole), with the summary from the same redacted text.
pub fn prepare(
    path: &Path,
    kind: &str,
    title: &str,
    app_version: &str,
    text: &dyn Fn(&str) -> String,
    versions: &Versions,
    max_report_bytes: usize,
) -> Result<Outgoing> {
    let mut f = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
        .map_err(fault)?;
    let mut raw = vec![];
    f.read_to_end(&mut raw).map_err(fault)?;
    let written =
        String::from_utf8(raw).map_err(|_| ReportError::Fault("UnicodeDecodeError".into()))?;
    let body = redacted_text(&written, text, PIECE_CHARS);
    if body != written {
        let folder = path.parent().unwrap_or(Path::new("."));
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        write(folder, &name, &body)?;
    }
    let file_name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let stem = match file_name.rfind('.') {
        Some(i) if i > 0 => &file_name[..i],
        _ => file_name.as_str(),
    };
    let report_id: String = stem.chars().take(128).collect();
    let (sent, note) = shorten(&body, max_report_bytes, &file_name);
    let default = if kind == "failure" {
        "FERB report"
    } else {
        "FERB diagnostics"
    };
    let kept: String = app_version.chars().filter(|c| printable(*c)).collect();
    let mut version = kept
        .split(ferb_py::value::is_py_space)
        .filter(|w| !w.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    if version.is_empty() {
        version = versions.ferb.clone();
    }
    let summary: String = text(&summarize(&body, &note, SUMMARY_CHARS))
        .chars()
        .take(SUMMARY_CHARS)
        .collect();
    let summary = if summary.is_empty() {
        default.to_owned()
    } else {
        summary
    };
    let content = base64::engine::general_purpose::STANDARD.encode(sent.as_bytes());
    let file =
        dumps(&json!({"message": format!("FERB {kind} report {report_id}"), "content": content}))
            .into_bytes();
    if file.len() > UPLOAD_MAX_BYTES {
        // never: `shorten` keeps it within; the relay refuses it anyway
        return Err(ReportError::Unsent(format!(
            "FERB couldn't fit this report into {} MB. It stays saved at {}.",
            max_report_bytes / 1_000_000,
            path.display()
        )));
    }
    let title = lines::clip(&text(title), TITLE_CHARS);
    let mut issue = Map::new();
    issue.insert("version".into(), json!(2));
    issue.insert(
        "title".into(),
        json!(if title.is_empty() {
            default.to_owned()
        } else {
            title
        }),
    );
    issue.insert("summary".into(), json!(summary));
    issue.insert(
        "app_version".into(),
        json!(version.chars().take(VERSION_CHARS).collect::<String>()),
    );
    Ok(Outgoing {
        kind: kind.to_owned(),
        report_id,
        file,
        issue,
        shortened: !note.is_empty(),
    })
}

/// How a POST to the relay failed before an answer came.
#[derive(Clone, Debug, PartialEq)]
pub enum PostError {
    /// A connect, read or write step ran out of time (httpx's TimeoutException).
    Timeout,
    /// Any other transport failure, by httpx's class name (ConnectError, ReadError, ...).
    Failed(String),
}

pub type BoxFuture<'a, T> = std::pin::Pin<Box<dyn std::future::Future<Output = T> + Send + 'a>>;

/// The HTTP POST to the relay: the status and body, or why no answer came.
pub trait RelayHttp: Send + Sync {
    fn post<'a>(
        &'a self,
        url: &'a str,
        body: &'a [u8],
        headers: &'a [(&'a str, String)],
    ) -> BoxFuture<'a, std::result::Result<(u16, Vec<u8>), PostError>>;
}

/// The real POST: reqwest (rustls), `timeout_s` for each connect and read step, no redirects followed and no FERB
/// proxy (as httpx's defaults); proxies from the environment as httpx reads them.
pub struct ReqwestRelay {
    client: reqwest::Client,
}

impl ReqwestRelay {
    pub fn new(timeout_s: f64) -> std::result::Result<ReqwestRelay, String> {
        let t = std::time::Duration::from_secs_f64(timeout_s.max(0.0));
        let client = reqwest::Client::builder()
            .connect_timeout(t)
            .read_timeout(t)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|e| e.to_string())?;
        Ok(ReqwestRelay { client })
    }

    fn class(e: &reqwest::Error) -> PostError {
        if e.is_timeout() {
            PostError::Timeout
        } else if e.is_connect() {
            PostError::Failed("ConnectError".into())
        } else if e.is_builder() {
            PostError::Failed("UnsupportedProtocol".into())
        } else if e.is_body() || e.is_decode() {
            PostError::Failed("ReadError".into())
        } else {
            PostError::Failed("RemoteProtocolError".into())
        }
    }
}

impl RelayHttp for ReqwestRelay {
    fn post<'a>(
        &'a self,
        url: &'a str,
        body: &'a [u8],
        headers: &'a [(&'a str, String)],
    ) -> BoxFuture<'a, std::result::Result<(u16, Vec<u8>), PostError>> {
        Box::pin(async move {
            let mut req = self.client.post(url).body(body.to_vec());
            for (k, v) in headers {
                req = req.header(*k, v);
            }
            let r = req.send().await.map_err(|e| Self::class(&e))?;
            let status = r.status().as_u16();
            let bytes = r.bytes().await.map_err(|e| Self::class(&e))?;
            Ok((status, bytes.to_vec()))
        })
    }
}

/// `urlencode`'s `quote_plus` of one value.
fn quote_plus(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'_' | b'.' | b'-' | b'~' => {
                out.push(b as char)
            }
            b' ' => out.push('+'),
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

fn unsent(t: impl Into<String>) -> ReportError {
    ReportError::Unsent(t.into())
}

async fn call(
    http: &dyn RelayHttp,
    url: &str,
    content: &[u8],
    headers: &[(&str, String)],
) -> Result<(u16, Map<String, Value>)> {
    let (status, body) = http
        .post(url, content, headers)
        .await
        .map_err(|e| match e {
            PostError::Timeout => {
                unsent("FERB couldn't send the report: the report service didn't answer in time")
            }
            PostError::Failed(name) => {
                unsent(format!("FERB couldn't reach the report service ({name})"))
            }
        })?;
    let answer = std::str::from_utf8(&body)
        .ok()
        .and_then(|t| ferb_py::json::loads(t).ok());
    let answer = match answer {
        Some(Value::Object(m)) => m,
        _ => Map::new(),
    };
    if status == 429 {
        return Err(unsent(
            "Too many reports were sent from this network just now; try again in a few minutes",
        ));
    }
    Ok((status, answer))
}

fn refused(status: u16, answer: &Map<String, Value>) -> ReportError {
    let why = match answer.get("error") {
        Some(v) if truthy(v) => py_str(v),
        _ => "no reason given".into(),
    };
    unsent(format!(
        "The report service didn't take the report (HTTP {status}: {})",
        lines::clip(&why, 200)
    ))
}

/// `send(base, out)` (DMN-28, DMN-30, DMN-31): the file (again when the relay says another report's commit landed at
/// that moment, up to CONFLICT_TRIES, `pause_s` × the try between), then the issue with the relay's ticket; the
/// relay's report_id. Any failure is Unsent with a reason the owner can read; the whole Send within `deadline_s`.
pub async fn send(
    base: &str,
    out: &Outgoing,
    http: &dyn RelayHttp,
    ferb_version: &str,
    deadline_s: f64,
    pause_s: f64,
) -> Result<String> {
    let headers = [
        ("Content-Type", "application/json".to_owned()),
        ("User-Agent", format!("FERB/{ferb_version}")),
    ];
    let go = async {
        let query = format!(
            "kind={}&report_id={}",
            quote_plus(&out.kind),
            quote_plus(&out.report_id)
        );
        let mut got = (0u16, Map::new());
        for attempt in 0..CONFLICT_TRIES {
            got = call(http, &format!("{base}/file?{query}"), &out.file, &headers).await?;
            let retry = got.0 == 409
                && got.1.get("retry") == Some(&Value::Bool(true))
                && attempt + 1 < CONFLICT_TRIES;
            if !retry {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_secs_f64(
                (pause_s * (attempt + 1) as f64).max(0.0),
            ))
            .await;
        }
        let (status, answer) = got;
        let ticket = match answer.get("ticket") {
            Some(Value::String(t)) => t.clone(),
            _ => return Err(refused(status, &answer)),
        };
        if status != 200 || answer.get("ok") != Some(&Value::Bool(true)) {
            return Err(refused(status, &answer));
        }
        let mut issue = out.issue.clone();
        issue.insert("ticket".into(), json!(ticket));
        let body = dumps_utf8(&Value::Object(issue)).into_bytes();
        let (status, answer) = call(http, &format!("{base}/report"), &body, &headers).await?;
        if status != 200 || answer.get("ok") != Some(&Value::Bool(true)) {
            return Err(refused(status, &answer));
        }
        Ok(match answer.get("report_id") {
            Some(v) if truthy(v) => py_str(v),
            _ => out.report_id.clone(),
        })
    };
    match tokio::time::timeout(std::time::Duration::from_secs_f64(deadline_s.max(0.0)), go).await {
        Ok(r) => r,
        Err(_) => Err(unsent(
            "FERB couldn't send the report: it didn't finish in time",
        )),
    }
}
