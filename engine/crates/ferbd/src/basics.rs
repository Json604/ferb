//! What every part of the composition shares: the store's connection, the journal written from any thread (Python's
//! `Journal.append` on the loop), the published events, and the global stop's state.

use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError};

use ferb_core::interfaces::{Clock, IdSource};
use ferb_learning::journal::journal_origin;
use ferb_sanitize::redact::Redactor;
use ferb_store::Db;
use serde_json::{Map, Value};

/// The daemon's own store connection (`self.db`), shared by the parts that take one (the gateway, notices, about).
pub type SharedDb = Arc<Mutex<Db>>;

pub fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Opens and migrates the store at `path`.
pub fn open_store(path: &Path) -> Result<Db, String> {
    let db = Db::open(path).map_err(|e| e.to_string())?;
    db.migrate().map_err(|e| e.to_string())?;
    Ok(db)
}

/// `Journal(db, clock, ids, redact)` written from any thread: the same rows `ferb_learning::journal::Journal.append`
/// writes (redacted data as `json.dumps`, the origin worked out when none is given).
pub struct Journal {
    pub db: SharedDb,
    pub clock: Arc<dyn Clock>,
    pub ids: Arc<dyn IdSource>,
    pub redactor: Arc<Redactor>,
}

impl Journal {
    pub fn append(&self, kind: &str, data: Value, origin: Option<&str>) -> Result<String, String> {
        let body = self.redactor.value(&data);
        let origin = match origin {
            Some(o) if !o.is_empty() => o.to_owned(),
            _ => journal_origin(kind, &body),
        };
        let id = self.ids.new_id();
        let at = ferb_core::when::isoformat(&self.clock.now());
        lock(&self.db)
            .conn()
            .execute(
                "INSERT INTO journal (id, at, kind, data_json, origin) VALUES (?, ?, ?, ?, ?)",
                rusqlite::params![id, at, kind, ferb_py::json::dumps_utf8(&body), origin],
            )
            .map_err(|e| e.to_string())?;
        Ok(id)
    }

    /// `append`, a failure only noted on stderr (what Python's `contextlib.suppress` around a journal write does).
    pub fn note(&self, kind: &str, data: Value) {
        if let Err(e) = self.append(kind, data, None) {
            eprintln!("FERB: couldn't journal {kind}: {}", self.redactor.text(&e));
        }
    }

    /// `of_kind(kind, field, equals)`: the rows' data, oldest first.
    pub fn of_kind(&self, kind: &str, field: &str, equals: &str) -> Vec<(String, Value)> {
        let db = lock(&self.db);
        let mut out = Vec::new();
        let sql = if field.is_empty() {
            "SELECT at, data_json FROM journal WHERE kind = ?1 ORDER BY seq"
        } else {
            "SELECT at, data_json FROM journal WHERE kind = ?1 AND json_extract(data_json, ?2) = ?3 ORDER BY seq"
        };
        let Ok(mut stmt) = db.conn().prepare(sql) else {
            return out;
        };
        let path = format!("$.{field}");
        let rows = if field.is_empty() {
            stmt.query_map(rusqlite::params![kind], row_pair)
        } else {
            stmt.query_map(rusqlite::params![kind, path, equals], row_pair)
        };
        if let Ok(rows) = rows {
            out.extend(rows.flatten());
        }
        out
    }

    /// `latest_kind(kinds, field, equals)`.
    pub fn latest_kind(&self, kinds: &[&str], field: &str, equals: &str) -> Option<String> {
        let marks = vec!["?"; kinds.len()].join(", ");
        let sql = format!(
            "SELECT kind FROM journal WHERE kind IN ({marks}) AND json_extract(data_json, ?) = ? ORDER BY seq DESC LIMIT 1"
        );
        let mut params: Vec<String> = kinds.iter().map(|k| k.to_string()).collect();
        params.push(format!("$.{field}"));
        params.push(equals.to_string());
        let db = lock(&self.db);
        db.conn()
            .query_row(&sql, rusqlite::params_from_iter(params), |r| r.get(0))
            .ok()
    }

    /// `recent_values(kind, field, limit)`.
    pub fn recent_values(&self, kind: &str, field: &str, limit: i64) -> Vec<Value> {
        let db = lock(&self.db);
        let Ok(mut stmt) = db.conn().prepare(
            "SELECT json_extract(data_json, ?) FROM journal WHERE kind = ? ORDER BY seq DESC LIMIT ?",
        ) else {
            return Vec::new();
        };
        let rows = stmt.query_map(rusqlite::params![format!("$.{field}"), kind, limit], |r| {
            Ok(match r.get_ref(0)? {
                rusqlite::types::ValueRef::Null => Value::Null,
                rusqlite::types::ValueRef::Integer(i) => Value::from(i),
                rusqlite::types::ValueRef::Real(f) => Value::from(f),
                rusqlite::types::ValueRef::Text(t) => {
                    Value::String(String::from_utf8_lossy(t).into_owned())
                }
                rusqlite::types::ValueRef::Blob(_) => Value::Null,
            })
        });
        rows.map(|r| r.flatten().collect()).unwrap_or_default()
    }
}

fn row_pair(r: &rusqlite::Row<'_>) -> rusqlite::Result<(String, Value)> {
    let at: String = r.get(0)?;
    let data: String = r.get(1)?;
    Ok((at, serde_json::from_str(&data).unwrap_or(Value::Null)))
}

impl ferb_daemon::areas_b::core::Journal for Journal {
    fn append(&self, kind: &str, data: Value, origin: Option<&str>) -> Result<(), String> {
        Journal::append(self, kind, data, origin).map(|_| ())
    }
}

/// The daemon's events (`events.Events`): each redacted once, then handed to its consumers; also the in-process bus
/// `logs.subscribe` and FERB.app's streams read.
pub struct Events {
    pub events: ferb_daemon::areas_b::events::Events,
}

impl Events {
    pub fn new(redactor: Arc<Redactor>) -> Arc<Events> {
        let r = redactor.clone();
        Arc::new(Events {
            events: ferb_daemon::areas_b::events::Events::new(move || {
                let r = r.clone();
                Arc::new(move |v: &Value| r.value(v)) as ferb_daemon::areas_b::events::Redact
            }),
        })
    }

    pub fn publish(&self, topic: &str, data: Map<String, Value>) {
        self.events.publish(topic, &data);
    }
}

impl ferb_daemon::areas_b::core::Bus for Events {
    fn publish(&self, topic: &str, data: Value) {
        ferb_daemon::areas_b::core::Bus::publish(&self.events, topic, data)
    }
}

/// `state.stopped` and the stop marker (`_set_blocked`): the global stop outlives a restart until the owner lifts
/// it.
pub struct Stop {
    pub marker: std::path::PathBuf,
    on: std::sync::atomic::AtomicBool,
}

impl Stop {
    pub fn new(home: &Path) -> Arc<Stop> {
        Arc::new(Stop {
            marker: home.join("STOPPED"),
            on: std::sync::atomic::AtomicBool::new(false),
        })
    }

    pub fn engaged(&self) -> bool {
        self.on.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// `state.stopped = True` without touching the marker (the start reading it back).
    pub fn mark_engaged(&self) {
        self.on.store(true, std::sync::atomic::Ordering::SeqCst);
    }

    /// `_set_blocked(blocked)`.
    pub fn set(&self, blocked: bool) -> std::io::Result<()> {
        self.on.store(blocked, std::sync::atomic::Ordering::SeqCst);
        if blocked {
            use std::os::unix::fs::OpenOptionsExt;
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .mode(0o600)
                .open(&self.marker)
                .map(|_| ())
        } else {
            match std::fs::remove_file(&self.marker) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
                _ => Ok(()),
            }
        }
    }
}

/// A JSON object's map; anything else is `{}`.
pub fn object(v: Value) -> Map<String, Value> {
    match v {
        Value::Object(m) => m,
        _ => Map::new(),
    }
}
