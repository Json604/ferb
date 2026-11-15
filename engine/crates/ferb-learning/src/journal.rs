//! `journal.py`: the append-only journal (writes pass the redaction seam) and the origins a row holds by its kind.

use serde_json::Value;

use ferb_sanitize::origin;

use crate::summary::{digest_origins, NARRATIVE_KIND, SUMMARY_KIND};

/// journal.py:29: the owner's own words and answers.
pub const OWNER_KINDS: [&str; 4] = [
    "owner.verdict",
    "memory.added",
    "memory.edited",
    "approval.resolved",
];

/// journal.py:32 `journal_origin`.
pub fn journal_origin(kind: &str, data: &Value) -> String {
    if kind == "chat.message" {
        let mut set = vec![origin::OWNER.to_owned()];
        match data.get("untrusted_parts") {
            None | Some(Value::Null) => set.push(origin::external("unlabeled", "")),
            Some(v) if ferb_py::value::truthy(v) => set.push(origin::external("paste", "")),
            _ => {}
        }
        return origin::join(set.iter().map(String::as_str));
    }
    if OWNER_KINDS.contains(&kind) {
        return origin::OWNER.into();
    }
    if kind == SUMMARY_KIND || kind == NARRATIVE_KIND {
        let found = digest_origins(kind, data);
        return origin::join(found.iter().map(String::as_str));
    }
    origin::FERB.into()
}

#[cfg(feature = "store")]
pub use stored::*;

#[cfg(feature = "store")]
mod stored {
    use std::rc::Rc;

    use serde_json::Value;

    use super::journal_origin;
    use crate::py;
    use crate::sql;
    use crate::store::{marks, to_json, Row, Sql, Store};
    use crate::{Clock, Ids, Result};

    /// journal.py:20 `JournalEntry`.
    #[derive(Clone, Debug, PartialEq)]
    pub struct Entry {
        pub id: String,
        pub at: String,
        pub kind: String,
        pub data: Value,
        pub origin: String,
    }

    impl Entry {
        pub fn json(&self) -> Value {
            serde_json::json!({"id": self.id, "at": self.at, "kind": self.kind, "data": self.data,
                               "origin": self.origin})
        }
    }

    pub type Redact = Box<dyn Fn(&Value) -> Value>;

    /// journal.py:47 `Journal`.
    pub struct Journal {
        pub store: Rc<Store>,
        clock: Rc<dyn Clock>,
        ids: Rc<dyn Ids>,
        redact: Redact,
    }

    fn entry(r: &Row) -> Result<Entry> {
        let data = ferb_py::json::loads(&r.s("data_json")).map_err(crate::Error::Json)?;
        let at = py::iso(&py::parse(&r.s("at"))?);
        let kind = r.s("kind");
        let origin = match r.opt("origin") {
            Some(o) if !o.is_empty() => o,
            _ => journal_origin(&kind, &data),
        };
        Ok(Entry {
            id: r.s("id"),
            at,
            kind,
            data,
            origin,
        })
    }

    fn entries(rows: Vec<Row>) -> Result<Vec<Entry>> {
        rows.iter().map(entry).collect()
    }

    impl Journal {
        pub fn new(
            store: Rc<Store>,
            clock: Rc<dyn Clock>,
            ids: Rc<dyn Ids>,
            redact: Redact,
        ) -> Journal {
            Journal {
                store,
                clock,
                ids,
                redact,
            }
        }

        /// journal.py:59 `append`: never dropped; a failed write is the caller's error.
        pub fn append(
            &self,
            kind: &str,
            data: Option<Value>,
            origin: Option<&str>,
        ) -> Result<Entry> {
            let raw = data.unwrap_or_else(|| Value::Object(Default::default()));
            let body = (self.redact)(&raw);
            let origin = match origin {
                Some(o) if !o.is_empty() => o.to_owned(),
                _ => journal_origin(kind, &body),
            };
            let e = Entry {
                id: self.ids.new_id(),
                at: py::iso(&self.clock.now()),
                kind: kind.into(),
                data: body,
                origin,
            };
            self.store.exec(
                "INSERT INTO journal (id, at, kind, data_json, origin) VALUES (?, ?, ?, ?, ?)",
                sql![
                    e.id,
                    e.at,
                    e.kind,
                    ferb_py::json::dumps_utf8(&e.data),
                    e.origin
                ],
            )?;
            Ok(e)
        }

        /// journal.py:72 `entries`.
        pub fn entries(&self, since: Option<&str>, limit: Option<i64>) -> Result<Vec<Entry>> {
            let (wh, mut params) = match since {
                Some(s) => (
                    "WHERE julianday(at) >= julianday(?)",
                    sql![py::iso(&py::parse(s)?)],
                ),
                None => ("", Vec::new()),
            };
            match limit {
                None => entries(
                    self.store
                        .query(&format!("SELECT * FROM journal {wh} ORDER BY seq"), params)?,
                ),
                Some(n) => {
                    params.push(Sql::Integer(n));
                    let mut rows = self.store.query(
                        &format!("SELECT * FROM journal {wh} ORDER BY seq DESC LIMIT ?"),
                        params,
                    )?;
                    rows.reverse();
                    entries(rows)
                }
            }
        }

        /// journal.py:84 `entries_of`.
        pub fn entries_of(&self, kinds: &[&str]) -> Result<Vec<Entry>> {
            let sql_text = format!(
                "SELECT * FROM journal WHERE kind IN ({}) ORDER BY seq",
                marks(kinds.len())
            );
            entries(self.store.query(
                &sql_text,
                kinds.iter().map(|k| Sql::Text((*k).into())).collect(),
            )?)
        }

        /// journal.py:90 `kinds_after`.
        pub fn kinds_after(&self, kinds: &[&str], entry_id: &str) -> Result<Vec<Entry>> {
            let sql_text = format!(
                "SELECT * FROM journal WHERE kind IN ({}) AND seq > (SELECT seq FROM journal WHERE id = ?) ORDER BY seq",
                marks(kinds.len())
            );
            let mut params: Vec<Sql> = kinds.iter().map(|k| Sql::Text((*k).into())).collect();
            params.push(Sql::Text(entry_id.into()));
            entries(self.store.query(&sql_text, params)?)
        }

        /// journal.py:98 `of_kind`.
        pub fn of_kind(
            &self,
            kind: &str,
            field: &str,
            equals: &str,
            missing: Option<&str>,
        ) -> Result<Vec<Entry>> {
            let mut sql_text = "SELECT * FROM journal WHERE kind = ?".to_owned();
            let mut params = sql![kind];
            if !field.is_empty() {
                sql_text.push_str(" AND COALESCE(json_extract(data_json, ?), ?) = ?");
                params.extend(sql![
                    format!("$.{field}"),
                    missing.map(str::to_owned),
                    equals
                ]);
            }
            entries(self.store.query(&(sql_text + " ORDER BY seq"), params)?)
        }

        /// journal.py:109 `recent_values`.
        pub fn recent_values(&self, kind: &str, field: &str, limit: i64) -> Result<Vec<Value>> {
            Ok(self
                .store
                .query(
                    "SELECT json_extract(data_json, ?) FROM journal WHERE kind = ? ORDER BY seq DESC LIMIT ?",
                    sql![format!("$.{field}"), kind, limit],
                )?
                .iter()
                .map(|r| to_json(r.at(0)))
                .collect())
        }

        /// journal.py:115 `by_ids`.
        pub fn by_ids(&self, ids: &[String]) -> Result<Vec<Entry>> {
            if ids.is_empty() {
                return Ok(Vec::new());
            }
            let sql_text = format!(
                "SELECT * FROM journal WHERE id IN ({}) ORDER BY seq",
                marks(ids.len())
            );
            entries(self.store.query(
                &sql_text,
                ids.iter().map(|i| Sql::Text(i.clone())).collect(),
            )?)
        }

        /// journal.py:122 `latest_kind`.
        pub fn latest_kind(
            &self,
            kinds: &[&str],
            field: &str,
            equals: &str,
        ) -> Result<Option<String>> {
            let sql_text = format!(
                "SELECT kind FROM journal WHERE kind IN ({}) AND json_extract(data_json, ?) = ? ORDER BY seq DESC LIMIT 1",
                marks(kinds.len())
            );
            let mut params: Vec<Sql> = kinds.iter().map(|k| Sql::Text((*k).into())).collect();
            params.extend(sql![format!("$.{field}"), equals]);
            Ok(self.store.one(&sql_text, params)?.map(|r| r.s_at(0)))
        }

        /// journal.py:130 `of_running_tasks`.
        pub fn of_running_tasks(&self, kind: &str) -> Result<Vec<Entry>> {
            entries(self.store.query(
                "SELECT * FROM journal WHERE kind = ? AND json_extract(data_json, '$.task_id') IN \
                 (SELECT id FROM tasks WHERE status = 'running') ORDER BY seq",
                sql![kind],
            )?)
        }
    }
}
