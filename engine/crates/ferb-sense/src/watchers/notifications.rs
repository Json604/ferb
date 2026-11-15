//! Notification Center, read through Full Disk Access (`ferb/sense/watchers/notifications.py`), including records
//! received under Do Not Disturb. Banner actions are never invoked. Records are read incrementally by rowid; the
//! request dictionary uses Notification Center's 4-letter keys (`titl`, `body`, `thre`). Nothing is read before
//! Full Disk Access is on (SENSE-405: reading without it makes macOS show "Data Access Blocked").

use std::sync::Arc;

use ferb_core::interfaces::BoxFuture;

use crate::manager::{Emit, Stop, Watcher};
use crate::observation::Observation;

/// What the watcher reads (the database reader, or a fake).
pub trait NotificationReader: Send + Sync {
    /// Records added since the previous read. `Err`: why the read failed (`"Type: message"`).
    fn read(&self) -> Result<Vec<Observation>, String>;
}

/// Notification Center's database, under the owner's home.
pub fn database(home: &str) -> String {
    format!(
        "{}/Library/Group Containers/group.com.apple.usernoted/db2/db",
        home.trim_end_matches('/')
    )
}

#[cfg(feature = "store")]
pub use db::NotificationDatabase;

#[cfg(feature = "store")]
mod db {
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex};

    use rusqlite::types::Value as Sql;
    use rusqlite::{params, Connection, OpenFlags};
    use serde_json::Value;
    use sha2::{Digest, Sha256};

    use super::NotificationReader;
    use crate::observation::{dict, Observation, Redact};
    use crate::software::{plist_loads, sqlite_error_name};

    /// notifications.py:18.
    pub struct NotificationDatabase {
        pub path: PathBuf,
        redact: Arc<dyn Redact>,
        excluded: Arc<dyn Fn(&str, &str) -> bool + Send + Sync>,
        /// the last record rowid read; the first read after a start reads them all
        pub after: Mutex<i64>,
    }

    /// Python's `str()` of a plist value (text as it is).
    fn py_str(v: &plist::Value) -> String {
        match v {
            plist::Value::String(s) => s.clone(),
            plist::Value::Boolean(true) => "True".into(),
            plist::Value::Boolean(false) => "False".into(),
            plist::Value::Integer(i) => i
                .as_signed()
                .map(|i| i.to_string())
                .or_else(|| i.as_unsigned().map(|u| u.to_string()))
                .unwrap_or_default(),
            plist::Value::Real(f) => ferb_py::json::python_float(*f),
            other => ferb_py::json::dumps_utf8(&crate::software::plist_json(other)),
        }
    }

    fn truthy(v: &plist::Value) -> bool {
        match v {
            plist::Value::String(s) => !s.is_empty(),
            plist::Value::Boolean(b) => *b,
            plist::Value::Integer(i) => i.as_signed() != Some(0),
            plist::Value::Real(f) => *f != 0.0,
            plist::Value::Array(a) => !a.is_empty(),
            plist::Value::Dictionary(d) => !d.is_empty(),
            plist::Value::Data(d) => !d.is_empty(),
            _ => true,
        }
    }

    fn uid_bytes(uid: &Sql) -> Vec<u8> {
        match uid {
            Sql::Blob(b) => b.clone(),
            Sql::Text(t) => t.as_bytes().to_vec(),
            Sql::Integer(i) => i.to_string().into_bytes(),
            Sql::Real(f) => ferb_py::json::python_float(*f).into_bytes(),
            Sql::Null => b"None".to_vec(),
        }
    }

    impl NotificationDatabase {
        pub fn new(
            path: PathBuf,
            redact: Arc<dyn Redact>,
            excluded: Arc<dyn Fn(&str, &str) -> bool + Send + Sync>,
        ) -> NotificationDatabase {
            NotificationDatabase {
                path,
                redact,
                excluded,
                after: Mutex::new(0),
            }
        }

        fn decoded(&self, external: &str, bundle: &str, blob: &Sql) -> Option<Observation> {
            let Sql::Blob(blob) = blob else {
                return None; // bytes(text or None): TypeError, a payload that can't be read
            };
            let content = plist_loads(blob).ok()?;
            let req = match &content {
                plist::Value::Dictionary(d) => match d.get("req") {
                    Some(r) => r.clone(),
                    None => content.clone(),
                },
                _ => return None, // `content.get` of a list: AttributeError
            };
            let plist::Value::Dictionary(req) = req else {
                return None;
            };
            let title = match req.get("titl").or_else(|| req.get("title")) {
                Some(v) => py_str(v),
                None => String::new(),
            };
            let body = req.get("body").map(py_str).unwrap_or_default();
            let thread = req.get("thre").filter(|t| truthy(t));
            let data = dict([
                ("external_id", Value::from(external)),
                ("bundle_id", Value::from(bundle)),
                ("title", Value::from(title.clone())),
                ("body", Value::from(body)),
                ("conversation", Value::from(thread.map_or(title, py_str))),
                (
                    "conversation_source",
                    Value::from(if thread.is_some() { "thread" } else { "title" }),
                ),
            ]);
            Some(Observation::by(
                "notification",
                self.redact.dict(&data),
                "unknown",
            ))
        }

        /// notifications.py:24 `read`: records added since the previous read. Rowids reused after Notification
        /// Center deleted its newest records are read again from the start; the seen-receipts drop what was
        /// already recorded.
        pub fn read_records(&self) -> Result<Vec<Observation>, String> {
            let why = |e: rusqlite::Error| format!("{}: {e}", sqlite_error_name(&e));
            // read-only, the normal WAL reader (immutable would miss notifications still in the WAL)
            let db = Connection::open_with_flags(
                &self.path,
                OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
            )
            .map_err(why)?;
            let names = |sql: &str, col: usize| -> rusqlite::Result<Vec<String>> {
                let mut st = db.prepare(sql)?;
                let r = st.query_map([], |r| r.get::<_, String>(col))?;
                r.collect()
            };
            let unsupported =
                || "RuntimeError: Notification Center schema is unsupported".to_string();
            let tables =
                names("SELECT name FROM sqlite_master WHERE type='table'", 0).map_err(why)?;
            if !["record", "app"]
                .iter()
                .all(|t| tables.iter().any(|x| x == t))
            {
                return Err(unsupported());
            }
            let records = names("PRAGMA table_info(record)", 1).map_err(why)?;
            let apps = names("PRAGMA table_info(app)", 1).map_err(why)?;
            if !["uuid", "app_id", "data"]
                .iter()
                .all(|c| records.iter().any(|x| x == c))
                || !["app_id", "identifier"]
                    .iter()
                    .all(|c| apps.iter().any(|x| x == c))
            {
                return Err(unsupported());
            }
            let mut after = *self
                .after
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let max: i64 = db
                .query_row("SELECT COALESCE(MAX(rowid), 0) FROM record", [], |r| {
                    r.get(0)
                })
                .map_err(why)?;
            if max < after {
                after = 0;
            }
            let mut last = after;
            let mut out = Vec::new();
            let mut st = db
                .prepare(
                    "SELECT r.rowid, r.uuid, a.identifier, r.data FROM record r JOIN app a \
                     ON r.app_id=a.app_id WHERE r.rowid > ? ORDER BY r.rowid",
                )
                .map_err(why)?;
            let mut q = st.query(params![after]).map_err(why)?;
            while let Some(r) = q.next().map_err(why)? {
                let rowid: i64 = r.get(0).map_err(why)?;
                last = last.max(rowid);
                let bundle = match r.get::<_, Sql>(2).map_err(why)? {
                    Sql::Text(t) => t,
                    other => crate::observation::py_str(Some(&match other {
                        Sql::Integer(i) => Value::from(i),
                        Sql::Null => Value::Null,
                        Sql::Real(f) => Value::from(f),
                        _ => Value::Null,
                    })),
                };
                if (self.excluded)(&bundle, &bundle) {
                    continue;
                }
                let external =
                    hex::encode(Sha256::digest(uid_bytes(&r.get::<_, Sql>(1).map_err(why)?)));
                let blob: Sql = r.get(3).map_err(why)?;
                out.push(self.decoded(&external, &bundle, &blob).unwrap_or_else(|| {
                    Observation::by(
                        "notification",
                        dict([
                            ("external_id", Value::from(external.clone())),
                            ("bundle_id", Value::from(bundle.clone())),
                            (
                                "decode_error",
                                Value::from("unsupported notification payload"),
                            ),
                        ]),
                        "unknown",
                    )
                }));
            }
            *self
                .after
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = last; // only once the read succeeded
            Ok(out)
        }
    }

    impl NotificationReader for NotificationDatabase {
        fn read(&self) -> Result<Vec<Observation>, String> {
            self.read_records()
        }
    }
}

type Floor = Arc<dyn Fn(Vec<Observation>) -> Vec<Observation> + Send + Sync>;

/// notifications.py:68.
pub struct NotificationWatcher {
    reader: Arc<dyn NotificationReader>,
    seen: Arc<dyn Fn(&str) -> bool + Send + Sync>,
    record: Arc<dyn Fn(Observation) + Send + Sync>,
    pub interval_s: f64,
    /// I1: what was there at install is marked seen, never recorded
    floor: Floor,
    /// Full Disk Access is on
    allowed: Arc<dyn Fn() -> bool + Send + Sync>,
    pub recheck_s: f64,
}

impl NotificationWatcher {
    pub fn new(
        reader: Arc<dyn NotificationReader>,
        seen: Arc<dyn Fn(&str) -> bool + Send + Sync>,
        record: Arc<dyn Fn(Observation) + Send + Sync>,
        floor: Floor,
        allowed: Arc<dyn Fn() -> bool + Send + Sync>,
    ) -> NotificationWatcher {
        NotificationWatcher {
            reader,
            seen,
            record,
            interval_s: 1.0,
            floor,
            allowed,
            recheck_s: 30.0,
        }
    }
}

impl Watcher for NotificationWatcher {
    fn source(&self) -> &str {
        "notification"
    }

    fn run(self: Arc<Self>, _emit: Emit, stop: Stop) -> BoxFuture<'static, Result<(), String>> {
        Box::pin(async move {
            while !stop.is_set() {
                let allowed = self.allowed.clone();
                if !tokio::task::spawn_blocking(move || allowed())
                    .await
                    .unwrap_or(false)
                {
                    stop.wait_for(self.recheck_s).await;
                    continue;
                }
                let reader = self.reader.clone();
                let read = tokio::task::spawn_blocking(move || reader.read())
                    .await
                    .map_err(|e| format!("RuntimeError: {e}"))??;
                for o in (self.floor)(read) {
                    if stop.is_set() {
                        return Ok(());
                    }
                    let id = crate::observation::py_str(o.data.get("external_id"));
                    if !(self.seen)(&id) {
                        (self.record)(o); // the event and its external id recorded together on the daemon's loop
                    }
                }
                stop.wait_for(self.interval_s).await;
            }
            Ok(())
        })
    }
}
