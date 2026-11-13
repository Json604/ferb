//! The activity trail (`ferb/sense/trail.py`): one path for every observation, in a fixed order (decision: SENSE-403).
//!
//! 1. paused: nothing; excluded: dropped completely; then redaction;
//! 2. id and actor (owner, ferb or unknown, from [`Attribution`]): the table refuses a row without an actor;
//! 3. persist to the append-only `trail` table, then publish `trail.event`;
//! 4. only then the tagging hook (Jev's; a no-op while Jev is off, SENSE-3).
//!
//! Raw key events and screen pixels are never recorded. Retention: a day's raw rows go only once the journal holds its
//! permanent summary (`purge`); accessibility rows go after 7 days whatever their day (`purge_source`, SENSE-1,
//! SENSE-4).

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use chrono::{NaiveDate, TimeDelta};
use ferb_clock::clock::ZonedClock;
use ferb_core::interfaces::IdSource;
use ferb_core::types::{Actor, TrailEvent};
use ferb_core::when::{isoformat, DateTime};
use ferb_sanitize::parity::casefold;
use ferb_store::retention::{delete_rows, BATCH};
use ferb_store::Db;
use rusqlite::types::Value as Sql;
use rusqlite::{params, params_from_iter};
use serde_json::{Map, Value};

use crate::attribution::Attribution;
use crate::batches::BROWSER_BUNDLES;
use crate::exclusions::Exclusions;
use crate::observation::{dumps, text_or_empty, truthy, Dict, Observation, Redact};

/// trail.py:33.
pub const ORDERING: &str =
    "newest first; observations are evidence, not a selected profile or target";
pub use crate::observation::{NARRATIVE_KIND, SUMMARY_KIND};

/// Why a trail call failed.
#[derive(Debug)]
pub enum Error {
    Store(rusqlite::Error),
    /// Python's `ValueError` (its text).
    Value(String),
}

impl From<rusqlite::Error> for Error {
    fn from(e: rusqlite::Error) -> Self {
        Error::Store(e)
    }
}

impl From<ferb_store::Error> for Error {
    fn from(e: ferb_store::Error) -> Self {
        Error::Value(e.to_string())
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Store(e) => write!(f, "{e}"),
            Error::Value(v) => write!(f, "{v}"),
        }
    }
}

impl std::error::Error for Error {}

pub type Result<T> = std::result::Result<T, Error>;

/// trail.py:105 `browser_key`: the browser name tab observations use for a browser app's bundle id ("" for none).
pub fn browser_key(bundle_id: &str) -> String {
    let b = casefold(bundle_id);
    BROWSER_BUNDLES
        .iter()
        .find(|(_, bundle)| casefold(bundle) == b && !b.is_empty())
        .map_or(String::new(), |(key, _)| key.to_string())
}

/// What the trail needs from other parts (seams).
pub struct Seams<'a> {
    /// `[privacy]` lists as they are now (re-read when config.toml changes).
    pub exclusions: Box<dyn Fn() -> Exclusions + 'a>,
    /// the daemon's `_publish` (the bus and its listeners).
    pub publish: Box<dyn Fn(&str, Value) + 'a>,
    pub device_id: Box<dyn Fn() -> String + 'a>,
    /// `ferb pause`.
    pub paused: Box<dyn Fn() -> bool + 'a>,
    /// GATE-550 (`orchestrator/test_runs.py` `TestRuns.task`): a test run's own activity isn't another run's context.
    pub test_task: Box<dyn Fn(Option<&str>) -> bool + 'a>,
    /// `act/mac/browsers.py` `is_browser`: a web browser by bundle id (never blocks).
    pub is_browser: Box<dyn Fn(&str) -> bool + 'a>,
}

/// trail.py:112.
pub struct Trail<'a> {
    pub db: &'a Db,
    pub clock: &'a dyn ZonedClock,
    ids: &'a dyn IdSource,
    redact: &'a dyn Redact,
    seams: Seams<'a>,
    pub attribution: Arc<Attribution>,
    /// Jev tagging: a no-op unless Jev is on (SENSE-3).
    pub tag: RefCell<Box<dyn Fn(&TrailEvent) + 'a>>,
    pub last_event_at: Cell<Option<DateTime>>,
    /// Each browser's page as its last tab observation gave it (host, incognito), seen before the exclusions apply.
    pub pages: RefCell<BTreeMap<String, (String, bool)>>,
}

fn actor_of(s: &str) -> Result<Actor> {
    match s {
        "owner" => Ok(Actor::Owner),
        "ferb" => Ok(Actor::Ferb),
        "unknown" => Ok(Actor::Unknown),
        // Python's sqlite3.IntegrityError from the table's CHECK
        other => Err(Error::Value(format!(
            "CHECK constraint failed: actor IN ('owner', 'ferb', 'unknown') ({other})"
        ))),
    }
}

fn parse_at(at: &str) -> DateTime {
    ferb_core::when::parse(at).unwrap_or_default()
}

/// `" ".join(f'"{t}"' for t in terms)`: every word, each a literal phrase.
fn match_query(terms: &[String]) -> String {
    terms
        .iter()
        .map(|t| format!("\"{t}\""))
        .collect::<Vec<_>>()
        .join(" ")
}

impl<'a> Trail<'a> {
    pub fn new(
        db: &'a Db,
        clock: &'a dyn ZonedClock,
        ids: &'a dyn IdSource,
        redact: &'a dyn Redact,
        seams: Seams<'a>,
        attribution: Arc<Attribution>,
    ) -> Trail<'a> {
        Trail {
            db,
            clock,
            ids,
            redact,
            seams,
            attribution,
            tag: RefCell::new(Box::new(|_| {})),
            last_event_at: Cell::new(None),
            pages: RefCell::new(BTreeMap::new()),
        }
    }

    /// The publish seam, for the watcher manager's failure events.
    pub fn publish(&self, topic: &str, data: Value) {
        (self.seams.publish)(topic, data)
    }

    fn day(&self, t: &DateTime) -> NaiveDate {
        self.clock.zone().date(t)
    }

    /// trail.py:126 `_in_browser_page` (decision: SENSE-5).
    fn in_browser_page(&self, o: &Observation) -> Option<Observation> {
        if o.source == "browser_tab" {
            self.pages.borrow_mut().insert(
                text_or_empty(o.data.get("browser")),
                (
                    text_or_empty(o.data.get("host")),
                    truthy(o.data.get("incognito")),
                ),
            );
            return Some(o.clone());
        }
        if o.source != "typed_text" && o.source != "clipboard" {
            return Some(o.clone());
        }
        let bundle = text_or_empty(o.data.get("bundle_id"));
        if bundle.is_empty() || !(self.seams.is_browser)(&bundle) {
            return Some(o.clone());
        }
        let pages = self.pages.borrow();
        let (host, incognito) = pages.get(&browser_key(&bundle))?;
        let mut data = o.data.clone();
        data.insert("host".into(), Value::from(host.clone()));
        data.insert("incognito".into(), Value::Bool(*incognito));
        Some(Observation::with_hints(&o.source, data, o.hints.clone()))
    }

    /// trail.py:146 `record`.
    pub fn record(&self, o: &Observation) -> Result<Option<TrailEvent>> {
        if (self.seams.paused)() {
            return Ok(None);
        }
        let Some(o) = self.in_browser_page(o) else {
            return Ok(None);
        };
        if (self.seams.exclusions)().excluded(&o.source, &o.data) {
            return Ok(None); // 1. nothing excluded goes further
        }
        let data: Dict = self.redact.dict(&o.data);
        let (actor, task_id) = self.attribution.decide(&o); // 2. id + actor
        if (self.seams.test_task)(task_id.as_deref()) {
            return Ok(None); // GATE-550
        }
        let now = self.clock.now();
        let id = self.ids.new_id();
        let device = (self.seams.device_id)();
        let day = self.day(&now).format("%Y-%m-%d").to_string();
        self.db.conn().execute(
            "INSERT INTO trail (id, at, day, device_id, actor, task_id, source, data_json) VALUES (?,?,?,?,?,?,?,?)",
            params![
                id,
                isoformat(&now),
                day,
                device,
                actor,
                task_id,
                o.source,
                dumps(&Value::Object(data.clone()))
            ],
        )?; // 3. persist, then publish
        let e = TrailEvent {
            id: id.clone(),
            at: now,
            device_id: device,
            actor: actor_of(&actor)?,
            task_id: task_id.clone(),
            source: o.source.clone(),
            data: data.clone(),
        };
        self.last_event_at.set(Some(now));
        let mut published = Map::new();
        published.insert("id".into(), Value::from(id));
        published.insert("source".into(), Value::from(o.source.clone()));
        published.insert("actor".into(), Value::from(actor));
        published.insert("task_id".into(), task_id.map_or(Value::Null, Value::from));
        published.insert("data".into(), Value::Object(data));
        (self.seams.publish)("trail.event", Value::Object(published));
        (self.tag.borrow())(&e); // 4. only then tagging
        Ok(Some(e))
    }

    // ---------- reads ----------

    /// trail.py:174: one row as stored (column name to value), or None.
    pub fn row(&self, ident: &str) -> Result<Option<Map<String, Value>>> {
        let mut st = self.db.conn().prepare("SELECT * FROM trail WHERE id=?")?;
        let names: Vec<String> = st.column_names().map(|s| s.to_string()).collect();
        let mut q = st.query(params![ident])?;
        let Some(r) = q.next()? else {
            return Ok(None);
        };
        let mut out = Map::new();
        for (i, n) in names.iter().enumerate() {
            let v = match r.get::<_, Sql>(i)? {
                Sql::Null => Value::Null,
                Sql::Integer(i) => Value::from(i),
                Sql::Real(f) => Value::from(f),
                Sql::Text(t) => Value::from(t),
                Sql::Blob(b) => Value::from(hex::encode(b)),
            };
            out.insert(n.clone(), v);
        }
        Ok(Some(out))
    }

    /// trail.py:178.
    pub fn events(
        &self,
        source: Option<&str>,
        actor: Option<&str>,
        limit: i64,
    ) -> Result<Vec<TrailEvent>> {
        let mut q =
            "SELECT id, at, device_id, actor, task_id, source, data_json FROM trail WHERE 1=1"
                .to_string();
        let mut args: Vec<Sql> = Vec::new();
        if let Some(s) = source.filter(|s| !s.is_empty()) {
            q.push_str(" AND source = ?");
            args.push(Sql::Text(s.into()));
        }
        if let Some(a) = actor.filter(|s| !s.is_empty()) {
            q.push_str(" AND actor = ?");
            args.push(Sql::Text(a.into()));
        }
        q.push_str(" ORDER BY seq DESC LIMIT ?");
        args.push(Sql::Integer(limit));
        let mut st = self.db.conn().prepare(&q)?;
        let rows = st.query_map(params_from_iter(args), |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, Option<String>>(4)?,
                r.get::<_, String>(5)?,
                r.get::<_, String>(6)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (id, at, device_id, actor, task_id, source, data) = row?;
            out.push(TrailEvent {
                id,
                at: parse_at(&at),
                device_id,
                actor: actor_of(&actor)?,
                task_id,
                source,
                data: match ferb_py::json::loads(&data) {
                    Ok(Value::Object(m)) => m,
                    _ => Map::new(),
                },
            });
        }
        Ok(out)
    }

    /// trail.py:188 `query`: filter before paging; ordering is chronology, never an instruction to choose the
    /// newest candidate. `since` and `until` (exclusive) are ISO times in the form the rows were written.
    #[allow(clippy::too_many_arguments)]
    pub fn query(&self, q: &Query<'_>) -> Result<Value> {
        if q.limit < 1 {
            return Err(Error::Value("limit must be positive".into()));
        }
        let mut sql =
            "SELECT seq, id, at, source, actor, task_id, data_json FROM trail WHERE at >= ?"
                .to_string();
        let mut params: Vec<Sql> = vec![Sql::Text(q.since.into())];
        if let Some(until) = q.until {
            sql.push_str(" AND at < ?");
            params.push(Sql::Text(until.into()));
        }
        if let Some(id) = q.event_id {
            sql = "SELECT seq, id, at, source, actor, task_id, data_json FROM trail WHERE id = ?"
                .into();
            params = vec![Sql::Text(id.into())];
        }
        for (column, value) in [("source", q.source), ("actor", q.actor)] {
            if let Some(v) = value {
                sql.push_str(&format!(" AND {column} = ?"));
                params.push(Sql::Text(v.into()));
            }
        }
        if let Some(b) = q.before_seq {
            sql.push_str(" AND seq < ?");
            params.push(Sql::Integer(b));
        }
        if let Some(host) = q.host.filter(|h| !h.is_empty()) {
            // literal suffix comparison: '%' or '_' in the input are not SQL wildcards
            sql.push_str(
                " AND (lower(json_extract(data_json, '$.host')) = ? OR \
                 substr(lower(json_extract(data_json, '$.host')), -?) = ?)",
            );
            let h = casefold(host);
            let suffix = format!(".{h}");
            params.push(Sql::Text(h));
            params.push(Sql::Integer(suffix.chars().count() as i64));
            params.push(Sql::Text(suffix));
        }
        if let Some(text) = q.text {
            let terms = ferb_memory::terms::query_terms(text);
            if terms.is_empty() {
                return Ok(page(Vec::new(), None));
            }
            sql.push_str(" AND seq IN (SELECT rowid FROM trail_fts WHERE trail_fts MATCH ?)");
            params.push(Sql::Text(match_query(&terms)));
        }
        sql.push_str(" ORDER BY seq DESC LIMIT ?");
        params.push(Sql::Integer(q.limit + 1));
        let mut st = self.db.conn().prepare(&sql)?;
        let rows: Vec<(i64, Value)> = st
            .query_map(params_from_iter(params), |r| {
                let mut o = Map::new();
                o.insert("id".into(), Value::from(r.get::<_, String>(1)?));
                o.insert("at".into(), Value::from(r.get::<_, String>(2)?));
                o.insert("source".into(), Value::from(r.get::<_, String>(3)?));
                o.insert("actor".into(), Value::from(r.get::<_, String>(4)?));
                o.insert(
                    "task_id".into(),
                    r.get::<_, Option<String>>(5)?
                        .map_or(Value::Null, Value::from),
                );
                o.insert(
                    "data".into(),
                    ferb_py::json::loads(&r.get::<_, String>(6)?).unwrap_or(Value::Null),
                );
                Ok((r.get::<_, i64>(0)?, Value::Object(o)))
            })?
            .collect::<rusqlite::Result<_>>()?;
        let limit = q.limit as usize;
        let next = (rows.len() > limit).then(|| rows[limit - 1].0);
        Ok(page(
            rows.into_iter().take(limit).map(|(_, v)| v).collect(),
            next,
        ))
    }

    /// trail.py:227 `days`: daily summaries (code-built) and narratives (model-written), newest first.
    pub fn days(&self, text: Option<&str>, since_days: i64, limit: i64) -> Result<Vec<Value>> {
        let cutoff = (self.day(&self.clock.now()) - TimeDelta::days(since_days))
            .format("%Y-%m-%d")
            .to_string();
        let mut q = "SELECT id, kind, data_json FROM journal WHERE kind IN (?, ?) AND json_extract(data_json, '$.day') >= ?"
            .to_string();
        let mut params: Vec<Sql> = vec![
            Sql::Text(SUMMARY_KIND.into()),
            Sql::Text(NARRATIVE_KIND.into()),
            Sql::Text(cutoff),
        ];
        if let Some(text) = text {
            let terms = ferb_memory::terms::query_terms(text);
            if terms.is_empty() {
                return Ok(Vec::new());
            }
            q.push_str(" AND seq IN (SELECT rowid FROM summary_fts WHERE summary_fts MATCH ?)");
            params.push(Sql::Text(match_query(&terms)));
        }
        q.push_str(" ORDER BY json_extract(data_json, '$.day') DESC, seq DESC LIMIT ?");
        params.push(Sql::Integer(limit));
        let mut st = self.db.conn().prepare(&q)?;
        let rows = st.query_map(params_from_iter(params), |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (id, kind, data) = row?;
            let data = ferb_py::json::loads(&data).unwrap_or(Value::Null);
            let mut o = Map::new();
            o.insert("id".into(), Value::from(id));
            o.insert(
                "day".into(),
                data.as_object()
                    .and_then(|m| m.get("day"))
                    .cloned()
                    .unwrap_or(Value::Null),
            );
            o.insert(
                "kind".into(),
                Value::from(if kind == SUMMARY_KIND {
                    "summary"
                } else {
                    "narrative"
                }),
            );
            o.insert("data".into(), data);
            out.push(Value::Object(o));
        }
        Ok(out)
    }

    // ---------- retention ----------

    /// trail.py:249.
    pub fn summarized_days(&self) -> Result<BTreeSet<String>> {
        let mut st = self
            .db
            .conn()
            .prepare("SELECT data_json FROM journal WHERE kind = ?")?;
        let rows = st.query_map(params![SUMMARY_KIND], |r| r.get::<_, String>(0))?;
        let mut out = BTreeSet::new();
        for r in rows {
            let data = ferb_py::json::loads(&r?).unwrap_or(Value::Null);
            out.insert(crate::observation::py_str(
                data.as_object().and_then(|m| m.get("day")),
            ));
        }
        Ok(out)
    }

    /// trail.py:253 `purge`: (purged days, refused days); only days whose permanent summary is in the journal go.
    pub fn purge(
        &self,
        retention_days: i64,
        today: Option<NaiveDate>,
    ) -> Result<(Vec<String>, Vec<String>)> {
        let (purged, refused, steps) = self.purge_steps(retention_days, today)?;
        for step in steps {
            step?;
        }
        Ok((purged, refused))
    }

    /// trail.py:261 `purge_steps`: the deletes as steps of at most `BATCH` rows, each its own transaction.
    pub fn purge_steps(
        &self,
        retention_days: i64,
        today: Option<NaiveDate>,
    ) -> Result<(
        Vec<String>,
        Vec<String>,
        impl Iterator<Item = Result<usize>> + 'a,
    )> {
        let today = today.unwrap_or_else(|| self.day(&self.clock.now()));
        let cutoff = (today - TimeDelta::days(retention_days))
            .format("%Y-%m-%d")
            .to_string();
        let old: Vec<String> = {
            let mut st = self
                .db
                .conn()
                .prepare("SELECT DISTINCT day FROM trail WHERE day < ? ORDER BY day")?;
            let rows = st.query_map(params![cutoff], |r| r.get::<_, String>(0))?;
            rows.collect::<rusqlite::Result<_>>()?
        };
        let done = self.summarized_days()?;
        let purged: Vec<String> = old.iter().filter(|d| done.contains(*d)).cloned().collect();
        let refused: Vec<String> = old.iter().filter(|d| !done.contains(*d)).cloned().collect();
        let (days, db) = (purged.clone(), self.db);
        let steps = days
            .into_iter()
            .flat_map(move |d| DeleteWhere::new(db, "day = ?", vec![Sql::Text(d)], BATCH));
        Ok((purged, refused, steps))
    }

    /// trail.py:274 `purge_source` (SENSE-1, SENSE-4): one source's rows of days older than `days`, summarized
    /// or not; how many went.
    pub fn purge_source(
        &self,
        source: &str,
        days: i64,
        today: Option<NaiveDate>,
        chunk: usize,
    ) -> Result<usize> {
        let mut n = 0;
        for step in self.purge_source_steps(source, days, today, chunk) {
            n += step?;
        }
        Ok(n)
    }

    /// trail.py:279.
    pub fn purge_source_steps(
        &self,
        source: &str,
        days: i64,
        today: Option<NaiveDate>,
        chunk: usize,
    ) -> DeleteWhere<'a> {
        let today = today.unwrap_or_else(|| self.day(&self.clock.now()));
        let cutoff = (today - TimeDelta::days(days))
            .format("%Y-%m-%d")
            .to_string();
        DeleteWhere::new(
            self.db,
            "source = ? AND day < ?",
            vec![Sql::Text(source.into()), Sql::Text(cutoff)],
            chunk,
        )
    }
}

fn page(events: Vec<Value>, next: Option<i64>) -> Value {
    let mut o = Map::new();
    o.insert("count".into(), Value::from(events.len()));
    o.insert("events".into(), Value::Array(events));
    let mut out = Map::new();
    out.insert("events".into(), o.remove("events").expect("events"));
    out.insert("count".into(), o.remove("count").expect("count"));
    out.insert(
        "next_before_seq".into(),
        next.map_or(Value::Null, Value::from),
    );
    out.insert("ordering".into(), Value::from(ORDERING));
    Value::Object(out)
}

/// trail.py:188's arguments.
#[derive(Clone, Debug, Default)]
pub struct Query<'q> {
    pub since: &'q str,
    pub until: Option<&'q str>,
    pub source: Option<&'q str>,
    pub actor: Option<&'q str>,
    pub host: Option<&'q str>,
    pub limit: i64,
    pub before_seq: Option<i64>,
    pub text: Option<&'q str>,
    pub event_id: Option<&'q str>,
}

/// trail.py:287 `_delete_where`: the rows `where` selects, deleted `chunk` at a time with what points at them (a
/// perception receipt), one transaction a step; yields each step's count.
pub struct DeleteWhere<'a> {
    db: &'a Db,
    sql: String,
    params: Vec<Sql>,
    chunk: usize,
    done: bool,
}

impl<'a> DeleteWhere<'a> {
    fn new(db: &'a Db, where_: &str, params: Vec<Sql>, chunk: usize) -> DeleteWhere<'a> {
        DeleteWhere {
            db,
            sql: format!("SELECT rowid FROM trail WHERE {where_} LIMIT ?"),
            params,
            chunk,
            done: false,
        }
    }

    fn step(&mut self) -> Result<Option<usize>> {
        let mut params = self.params.clone();
        params.push(Sql::Integer(self.chunk as i64));
        let ids: Vec<Sql> = {
            let mut st = self.db.conn().prepare(&self.sql)?;
            let rows = st.query_map(params_from_iter(params), |r| r.get::<_, i64>(0))?;
            rows.map(|r| r.map(Sql::Integer))
                .collect::<rusqlite::Result<_>>()?
        };
        if ids.is_empty() {
            self.done = true;
            return Ok(None);
        }
        let mut n = 0;
        for step in delete_rows(self.db, "trail", "rowid", &ids, self.chunk) {
            n += step?;
        }
        if ids.len() < self.chunk {
            self.done = true;
        }
        Ok(Some(n))
    }
}

impl Iterator for DeleteWhere<'_> {
    type Item = Result<usize>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.done {
            return None;
        }
        match self.step() {
            Ok(Some(n)) => Some(Ok(n)),
            Ok(None) => None,
            Err(e) => {
                self.done = true;
                Some(Err(e))
            }
        }
    }
}
