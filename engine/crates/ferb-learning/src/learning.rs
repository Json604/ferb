//! `learning.py`'s store: learned nodes (episode cards, playbooks, day summaries, owner-confirmed identity), their
//! provenance and origins, the card bookkeeping (MEM-422), forget with tombstones (C1: nothing forgotten comes back),
//! the owner's Drop of a fact index, rollback of a batch, the one ranked search and the content re-index.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::rc::Rc;

use serde_json::{json, Map, Value};

use ferb_memory::embeddings::Embedder;
use ferb_memory::pystr;
use ferb_memory::search::{fused, match_expr, tied, Ranking, MIN_SIMILARITY, NEAREST, NODE_KINDS};
use ferb_memory::times::{self, LocalZone};
use ferb_sanitize::origin;

use crate::items::{prompt_ready, Memory};
use crate::lineage::{call_sources, direct, REQUEST_REFERENCE};
use crate::notes::{
    self, digest, encoded, search_text, CARD_MAKER, ID_KEYED, KEPT_FAMILIES, PARKED_BY_REDACTION,
    RECENCY_HALF_LIFE_DAYS, RECENCY_WEIGHT, SEARCH_TEXT_VERSION, SOURCE_REF_INDEX, SUGGESTIONS,
};
use crate::py;
use crate::sql;
use crate::store::{marks, to_json, Row, Sql, Store};
use crate::text::{query_terms, weighted};
use crate::{Clock, Error, Result};

fn refused(m: impl Into<String>) -> Error {
    Error::LearningRefused(m.into())
}

fn truthy(v: Option<&Value>) -> bool {
    v.is_some_and(ferb_py::value::truthy)
}

fn s(v: &Value, k: &str) -> String {
    v.get(k).map_or_else(String::new, py::py_str)
}

/// What learning.py takes from other parts.
pub trait Seams {
    /// `ferb.orchestrator.episodes.get(db, task_id)`: the episode as recorded (its JSON fields read, `how` added).
    fn episode(&self, store: &Store, task_id: &str) -> Result<Option<Value>>;
    /// `ferb.sense.tools.trail_sources({source})`: the read sources of a trail row's source.
    fn trail_sources(&self, source: &str) -> Vec<(String, String)>;
}

/// learning.py:68 `CardSpan`.
#[derive(Clone, Debug)]
pub struct CardSpan {
    pub id: String,
    pub kind: String,
    pub input: Value,
    pub result: Option<Value>,
    pub error: Option<String>,
}

/// learning.py:90 `redacted_json`: `value` redacted by structure (`redact_value`, the daemon's Redactor walk; else
/// each key, string and number by `redact`), refused only on a specific hit in the text to be stored.
pub fn redacted_json(
    redact: &dyn Fn(&str) -> String,
    value: &Value,
    secrets: &[String],
    redact_value: Option<&dyn Fn(&Value) -> Value>,
) -> Result<Value> {
    let out = match redact_value {
        Some(walk) => walk(value),
        None => each(redact, value),
    };
    let stored = encoded(&out);
    let known = ferb_sanitize::KnownValues::new(secrets.iter().cloned());
    let found = if ferb_sanitize::holds_sealed(&stored, &known) {
        Some("a saved secret")
    } else {
        ferb_sanitize::first_format(&stored)
    };
    if let Some(found) = found {
        return Err(refused(format!(
            "could not be stored safely: it still holds {found}"
        )));
    }
    Ok(out)
}

fn each(redact: &dyn Fn(&str) -> String, value: &Value) -> Value {
    match value {
        Value::Object(m) => {
            let mut out = Map::new();
            for (k, v) in m {
                let empty = match v {
                    Value::Null => true,
                    Value::String(t) => t.is_empty(),
                    Value::Array(a) => a.is_empty(),
                    Value::Object(o) => o.is_empty(),
                    _ => false,
                };
                let masked = ferb_sanitize::secret_key(k) && !empty && !v.is_boolean();
                out.insert(
                    redact(k),
                    if masked {
                        Value::String(ferb_sanitize::REDACTED.into())
                    } else {
                        each(redact, v)
                    },
                );
            }
            Value::Object(out)
        }
        Value::Array(a) => Value::Array(a.iter().map(|v| each(redact, v)).collect()),
        Value::String(t) => Value::String(redact(t)),
        Value::Number(_) => {
            let text = encoded(value);
            if redact(&text) == text {
                value.clone()
            } else {
                Value::String(ferb_sanitize::REDACTED.into())
            }
        }
        other => other.clone(),
    }
}

/// learning.py:241 `_tied` over SQL rows (id, score).
fn tied_rows(rows: &[Row], positional: bool) -> Ranking {
    let pairs: Vec<(String, f64)> = rows.iter().map(|r| (r.s_at(0), r.f_at(1))).collect();
    tied(pairs.iter().map(|(i, s)| (i.as_str(), *s)), positional)
}

trait RowF {
    fn f_at(&self, n: usize) -> f64;
}

impl RowF for Row {
    fn f_at(&self, n: usize) -> f64 {
        match self.at(n) {
            Sql::Integer(i) => *i as f64,
            Sql::Real(r) => *r,
            _ => 0.0,
        }
    }
}

/// learning.py:255 `_reindexed_through`.
fn reindexed_through(store: &Store) -> Result<Option<String>> {
    match store.one(
        "SELECT version, through FROM learning_search_version",
        sql![],
    )? {
        Some(r) if r.s_at(0) == SEARCH_TEXT_VERSION => Ok(r.opt_at(1)),
        _ => Ok(Some(String::new())),
    }
}

/// learning.py:265 `search_indexed`.
pub fn search_indexed(store: &Store) -> Result<bool> {
    Ok(reindexed_through(store)?.is_none())
}

/// learning.py:270 `reindex_search`: every node indexed by its content once per SEARCH_TEXT_VERSION, in batches of
/// about `batch_bytes` of stored bodies, each one write transaction that records how far it got.
pub fn reindex_search(
    store: &Store,
    batch_bytes: usize,
    stop: &mut dyn FnMut() -> bool,
) -> Result<bool> {
    let mut did = false;
    while !stop() {
        store.plain("BEGIN IMMEDIATE")?;
        let batch = (|| -> Result<Option<bool>> {
            let Some(after) = reindexed_through(store)? else {
                store.plain("COMMIT")?;
                return Ok(None);
            };
            let rows = store.query(
                "SELECT id, kind, key, body_json FROM learning_nodes WHERE id > ? ORDER BY id",
                sql![after],
            )?;
            let mut nodes: Vec<&Row> = Vec::new();
            let (mut size, mut finished) = (0usize, true);
            for r in &rows {
                if !nodes.is_empty() && size >= batch_bytes {
                    finished = false;
                    break;
                }
                nodes.push(r);
                size += pystr::char_len(&r.s_at(3));
            }
            if !nodes.is_empty() {
                store.exec(
                    &format!(
                        "DELETE FROM learning_fts WHERE id IN ({})",
                        marks(nodes.len())
                    ),
                    nodes.iter().map(|r| r.at(0).clone()).collect(),
                )?;
                let mut inserts = Vec::new();
                for r in &nodes {
                    let body = ferb_py::json::loads(&r.s_at(3)).map_err(Error::Json)?;
                    inserts.push(sql![r.s_at(0), search_text(&r.s_at(1), &r.s_at(2), &body)]);
                }
                store.exec_many("INSERT INTO learning_fts VALUES (?,?)", inserts)?;
            }
            if finished {
                store.exec(
                    "DELETE FROM learning_fts WHERE id NOT IN (SELECT id FROM learning_nodes)",
                    sql![],
                )?;
            }
            store.exec("DELETE FROM learning_search_version", sql![])?;
            let through = if finished {
                None
            } else {
                nodes.last().map(|r| r.s_at(0))
            };
            store.exec(
                "INSERT INTO learning_search_version VALUES (?,?)",
                sql![SEARCH_TEXT_VERSION, through],
            )?;
            store.plain("COMMIT")?;
            Ok(Some(!nodes.is_empty()))
        })();
        match batch {
            Ok(None) => break,
            Ok(Some(any)) => did = did || any,
            Err(e) => {
                if store.in_transaction() {
                    store.plain("ROLLBACK")?;
                }
                return Err(e);
            }
        }
    }
    Ok(did)
}

/// The search filters and options (`Learning.search`'s keywords).
#[derive(Clone, Debug, Default)]
pub struct Query<'a> {
    pub kind: Option<&'a str>,
    pub app: Option<&'a str>,
    pub person: Option<&'a str>,
    pub since: Option<&'a str>,
    pub until: Option<&'a str>,
    pub query_vector: Option<Vec<i8>>,
    pub no_embed: bool,
    pub limit: Option<usize>,
    pub leave_out: Vec<&'a str>,
    pub no_sources: bool,
    pub focus: &'a str,
}

type Labels = (
    BTreeSet<String>,
    Vec<String>,
    Vec<String>,
    Vec<String>,
    Vec<String>,
);

/// learning.py:328 `Learning`.
pub struct Learning {
    pub store: Rc<Store>,
    pub clock: Rc<dyn Clock>,
    pub zone: Rc<dyn LocalZone>,
    pub publish: Box<dyn Fn(&str, Value)>,
    pub redact: Box<dyn Fn(&str) -> String>,
    pub redact_id: Option<Box<dyn Fn(&str) -> String>>,
    pub redact_value: Option<Box<dyn Fn(&Value) -> Value>>,
    pub embedder: Option<Rc<dyn Embedder>>,
    pub secret_values: Box<dyn Fn() -> Vec<String>>,
    pub seams: Box<dyn Seams>,
    pub generation: Cell<u64>,
    memo: RefCell<Option<HashMap<String, std::result::Result<BTreeSet<String>, Error>>>>,
    removing: RefCell<BTreeSet<String>>,
    /// The local model once loaded after the store was built (`learning.embedder = ...`, `_load_embedder`).
    late_embedder: RefCell<Option<Rc<dyn Embedder>>>,
}

impl Learning {
    /// memory.py:423 `self.learning.embedder = StaticEmbedder(...)`: the local model, loaded once the store runs
    /// (search ranks by keywords and recency until it is there).
    pub fn set_embedder(&self, embedder: Rc<dyn Embedder>) {
        *self.late_embedder.borrow_mut() = Some(embedder);
    }

    /// `self.embedder`: the model given when the store was built, else the one set since.
    pub fn embedder(&self) -> Option<Rc<dyn Embedder>> {
        self.embedder
            .clone()
            .or_else(|| self.late_embedder.borrow().clone())
    }

    pub fn new(
        store: Rc<Store>,
        clock: Rc<dyn Clock>,
        zone: Rc<dyn LocalZone>,
        publish: Box<dyn Fn(&str, Value)>,
        redact: Box<dyn Fn(&str) -> String>,
        seams: Box<dyn Seams>,
    ) -> Learning {
        Learning {
            store,
            clock,
            zone,
            publish,
            redact,
            redact_id: None,
            redact_value: None,
            embedder: None,
            secret_values: Box::new(Vec::new),
            seams,
            generation: Cell::new(0),
            memo: RefCell::new(None),
            removing: RefCell::new(BTreeSet::new()),
            late_embedder: RefCell::new(None),
        }
    }

    fn now(&self) -> String {
        py::iso(&self.clock.now())
    }

    fn bump(&self) {
        self.generation.set(self.generation.get() + 1);
    }

    /// learning.py:353 `get`.
    pub fn get(&self, node_id: &str, sources: bool) -> Result<Option<Value>> {
        let Some(r) = self
            .store
            .one("SELECT * FROM learning_nodes WHERE id=?", sql![node_id])?
        else {
            return Ok(None);
        };
        let mut out = Map::new();
        for (n, name) in r.names.iter().enumerate() {
            if name == "body_json" || name == "origins_json" {
                continue;
            }
            let v = to_json(r.at(n));
            out.insert(
                name.clone(),
                if name == "confirmed" {
                    json!(r.i_at(n) != 0)
                } else {
                    v
                },
            );
        }
        out.insert(
            "body".into(),
            ferb_py::json::loads(&r.s("body_json")).map_err(Error::Json)?,
        );
        out.insert(
            "origins".into(),
            ferb_py::json::loads(&r.s("origins_json")).map_err(Error::Json)?,
        );
        if sources {
            let refs: Vec<Value> = self
                .store
                .query(
                    "SELECT source_ref FROM learning_sources WHERE node_id=? ORDER BY source_ref",
                    sql![node_id],
                )?
                .iter()
                .map(|r| Value::String(r.s_at(0)))
                .collect();
            out.insert("sources".into(), Value::Array(refs));
        }
        Ok(Some(Value::Object(out)))
    }

    /// learning.py:368 `items`.
    pub fn items(
        &self,
        kind: Option<&str>,
        history: bool,
        sources: bool,
        kinds: &[&str],
    ) -> Result<Vec<Value>> {
        let mut q = "SELECT id FROM learning_nodes WHERE 1=1".to_owned();
        let mut args: Vec<Sql> = Vec::new();
        if !kinds.is_empty() {
            q.push_str(&format!(" AND kind IN ({})", marks(kinds.len())));
            args.extend(kinds.iter().map(|k| Sql::Text((*k).into())));
        }
        if let Some(k) = kind.filter(|k| !k.is_empty()) {
            q.push_str(" AND kind=?");
            args.push(Sql::Text(k.into()));
        }
        if !history {
            q.push_str(" AND valid_to IS NULL AND state NOT IN ('retired','rolled_back')");
        }
        let mut out = Vec::new();
        for r in self.store.query(&(q + " ORDER BY recorded_at,id"), args)? {
            let id = r.s_at(0);
            if let Some(mut node) = self.get(&id, sources)? {
                let n = self.store.count(
                    "SELECT COUNT(*) FROM learning_sources WHERE node_id=?",
                    sql![id],
                )?;
                node["source_count"] = json!(n);
                out.push(node);
            }
        }
        Ok(out)
    }

    /// learning.py:391 `versions`.
    pub fn versions(&self, kind: &str, key: &str) -> Result<Vec<Value>> {
        let mut out = Vec::new();
        for r in self.store.query(
            "SELECT id FROM learning_nodes WHERE kind=? AND key=? ORDER BY recorded_at,rowid",
            sql![kind, key],
        )? {
            if let Some(n) = self.get(&r.s_at(0), false)? {
                out.push(n);
            }
        }
        Ok(out)
    }

    fn source_origins(&self, r: &str) -> Result<BTreeSet<String>> {
        let cached = self.memo.borrow().as_ref().map(|m| m.get(r).cloned());
        match cached {
            None => self.read_origins(r),
            Some(Some(hit)) => hit,
            Some(None) => {
                let got = self.read_origins(r);
                let keep = match &got {
                    Ok(set) => Ok(set.clone()),
                    Err(e @ Error::LearningRefused(_)) => Err(e.clone()),
                    Err(e) => return Err(e.clone()),
                };
                if let Some(m) = self.memo.borrow_mut().as_mut() {
                    m.insert(r.to_owned(), keep);
                }
                got
            }
        }
    }

    fn read_origins(&self, r: &str) -> Result<BTreeSet<String>> {
        let node = if self.removing.borrow().contains(r) {
            None
        } else {
            self.get(r, false)?
        };
        if let Some(node) = node {
            return Ok(node["origins"]
                .as_array()
                .into_iter()
                .flatten()
                .map(py::py_str)
                .collect());
        }
        let Some((family, ident)) = r.split_once(':') else {
            return Err(refused(format!("unknown source {r}")));
        };
        match family {
            "memory" => {
                if let Some(row) = self
                    .store
                    .one("SELECT origin FROM memory_items WHERE id=?", sql![ident])?
                {
                    let mut labels = BTreeSet::from([notes::memory_origin(&row.s_at(0))]);
                    for src in self.store.query(
                        "SELECT kind,source_id FROM memory_sources WHERE item_id=?",
                        sql![ident],
                    )? {
                        match self.source_origins(&format!("{}:{}", src.s_at(0), src.s_at(1))) {
                            Ok(found) => labels.extend(found),
                            Err(Error::LearningRefused(_)) => {
                                labels.insert(origin::PRIVATE.into());
                                labels.insert(origin::external("unlabeled", ""));
                            }
                            Err(e) => return Err(e),
                        }
                    }
                    return Ok(labels);
                }
            }
            "episode" => {
                if let Some(row) = self
                    .store
                    .one("SELECT origins FROM episodes WHERE task_id=?", sql![ident])?
                {
                    let mut set = origin::split(row.opt_at(0).as_deref());
                    set.insert(origin::FERB.into());
                    return Ok(set);
                }
            }
            "trail" => {
                if let Some(row) = self
                    .store
                    .one("SELECT actor,source FROM trail WHERE id=?", sql![ident])?
                {
                    return Ok(self
                        .seams
                        .trail_sources(&row.s("source"))
                        .iter()
                        .map(|(k, v)| origin::from_read_source(k, v))
                        .collect());
                }
            }
            "journal" => {
                if let Some(row) = self
                    .store
                    .one("SELECT origin FROM journal WHERE id=?", sql![ident])?
                {
                    return Ok(origin::recorded(row.opt_at(0).as_deref(), true));
                }
            }
            _ => {}
        }
        let removing = self.removing.borrow().clone();
        let retained: Vec<Row> = self
            .store
            .query(
                "SELECT n.origins_json, n.id FROM learning_sources s JOIN learning_nodes n ON n.id=s.node_id \
                 WHERE s.source_ref=? ORDER BY n.recorded_at DESC",
                sql![r],
            )?
            .into_iter()
            .filter(|row| !removing.contains(&row.s_at(1)))
            .collect();
        if !retained.is_empty() {
            let mut out = BTreeSet::new();
            for row in retained {
                let labels = ferb_py::json::loads(&row.s_at(0)).map_err(Error::Json)?;
                out.extend(labels.as_array().into_iter().flatten().map(py::py_str));
            }
            return Ok(out);
        }
        Err(refused(format!("source is missing: {r}")))
    }

    /// learning.py:454 `inputs_changed`.
    pub fn inputs_changed(&self) {
        self.bump();
    }

    /// learning.py:458 `forgotten`.
    pub fn forgotten(&self, r: &str) -> Result<bool> {
        self.store
            .exists("SELECT 1 FROM forgotten_refs WHERE ref=?", sql![direct(r)])
    }

    /// learning.py:465 `card_key`.
    pub fn card_key(task_id: &str) -> String {
        format!("key-{}", digest(&json!(["episode", task_id])))
    }

    /// learning.py:469 `labels`.
    pub fn labels(&self, sources: &[String]) -> Result<BTreeSet<String>> {
        let set: BTreeSet<&String> = sources.iter().collect();
        let mut out = BTreeSet::new();
        for s in set {
            out.extend(self.source_origins(s)?);
        }
        Ok(out)
    }

    /// learning.py:473 `put`.
    #[allow(clippy::too_many_arguments)]
    pub fn put(
        &self,
        kind: &str,
        key: &str,
        body: &Value,
        sources: &[String],
        origin_label: &str,
        made_by: &str,
        batch: Option<&str>,
        state: &str,
        owner: bool,
        labels: Option<BTreeSet<String>>,
        supersede: bool,
    ) -> Result<Value> {
        if kind == "identity" && !owner {
            return Err(refused("only the owner can write identity"));
        }
        if SUGGESTIONS.contains(&kind)
            && !owner
            && self.store.exists(
                "SELECT 1 FROM learning_nodes WHERE kind=? AND state='retired' AND (key=? OR \
                 (json_extract(body_json, '$.text') IS NOT NULL AND json_extract(body_json, '$.text')=?)) LIMIT 1",
                sql![kind, key, body.get("text").map_or_else(String::new, py::py_str)],
            )?
        {
            return Err(refused("the owner said this suggestion was not right"));
        }
        if origin_label == origin::OWNER && !owner {
            return Err(refused("a derived record cannot invent owner authority"));
        }
        let body = redacted_json(
            &*self.redact,
            body,
            &(self.secret_values)(),
            self.redact_value.as_deref(),
        )?;
        let key = if ID_KEYED.contains(&kind) {
            match &self.redact_id {
                Some(f) => f(key),
                None => (self.redact)(key),
            }
        } else {
            (self.redact)(key)
        };
        let refs: Vec<String> = sources
            .iter()
            .cloned()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        let mut labels = match labels {
            Some(l) => l,
            None => self.labels(&refs)?,
        };
        if owner {
            labels = BTreeSet::from([origin::OWNER.to_owned()]);
        } else {
            labels.extend(origin::split(Some(origin_label)));
        }
        let inherited = origin::least_trusted(labels.iter().map(String::as_str));
        let sorted_labels: Vec<&String> = labels.iter().collect();
        let node_id = format!(
            "m-{}",
            digest(&json!([kind, key, body, refs, sorted_labels, made_by]))
        );
        let logical = format!("key-{}", digest(&json!([kind, key])));
        if self.store.exists(
            "SELECT 1 FROM forgotten_refs WHERE ref IN (?,?)",
            sql![node_id, logical],
        )? {
            return Err(refused("this content was forgotten"));
        }
        if let Some(old) = self.get(&node_id, true)? {
            return Ok(old);
        }
        let now = self.now();
        let mut prev = if supersede {
            self.store
                .one(
                    "SELECT id FROM learning_nodes WHERE kind=? AND key=? AND valid_to IS NULL \
                     ORDER BY recorded_at DESC,rowid DESC LIMIT 1",
                    sql![kind, key],
                )?
                .map(|r| r.s_at(0))
        } else {
            None
        };
        if let Some(p) = &prev {
            let prev_origin = self
                .get(p, true)?
                .map(|n| s(&n, "origin"))
                .unwrap_or_default();
            if prev_origin == origin::OWNER && !owner {
                prev = None;
            }
        }
        if kind == "episode" {
            self.bump();
        }
        self.store.plain("SAVEPOINT learning_put")?;
        let written = (|| -> Result<()> {
            if let Some(p) = &prev {
                self.store.exec(
                    "UPDATE learning_nodes SET valid_to=? WHERE id=?",
                    sql![now, p],
                )?;
            }
            self.store.exec(
                "INSERT INTO learning_nodes \
                 (id,kind,key,body_json,origin,made_by,recorded_at,valid_from,supersedes,batch_id,state,confirmed,origins_json) \
                 VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?)",
                sql![
                    node_id,
                    kind,
                    key,
                    encoded(&body),
                    inherited,
                    made_by,
                    now,
                    now,
                    prev.clone(),
                    batch.map(str::to_owned),
                    state,
                    i64::from(owner),
                    encoded(&json!(sorted_labels))
                ],
            )?;
            self.store.exec_many(
                "INSERT INTO learning_sources VALUES (?,?)",
                refs.iter().map(|r| sql![node_id, r]).collect(),
            )?;
            self.store.exec(
                "INSERT INTO learning_fts VALUES (?,?)",
                sql![node_id, search_text(kind, &key, &body)],
            )?;
            self.store.exec(
                "INSERT INTO learning_titles VALUES (?,?)",
                sql![node_id, key],
            )?;
            self.store.exec(
                "INSERT INTO learning_changes(batch_id,node_id,action,prior_id,at) VALUES (?,?,?,?,?)",
                sql![batch.map_or_else(|| node_id.clone(), str::to_owned), node_id, "add", prev.clone(), now],
            )?;
            self.store.plain("RELEASE learning_put")?;
            Ok(())
        })();
        if let Err(e) = written {
            self.store.plain("ROLLBACK TO learning_put")?;
            self.store.plain("RELEASE learning_put")?;
            return Err(e);
        }
        (self.publish)(
            "memory.derived",
            json!({"id": node_id, "kind": kind, "origin": inherited}),
        );
        Ok(self.get(&node_id, true)?.unwrap_or(Value::Null))
    }

    /// learning.py:534 `capture_episode`.
    pub fn capture_episode(
        &self,
        task_id: &str,
        apps: Option<&Map<String, Value>>,
    ) -> Result<Option<Value>> {
        let prepared = self.prepare_episode(task_id, apps)?;
        self.write_episode(prepared.as_ref())
    }

    /// learning.py:537 `write_episode`: the only write of a capture; a card built on forgotten content is never
    /// written, and its tombstone stops every later capture of it.
    pub fn write_episode(&self, prepared: Option<&Value>) -> Result<Option<Value>> {
        let Some(p) = prepared else { return Ok(None) };
        let task_id = s(p, "task_id");
        if truthy(p.get("forgotten")) {
            self.store.exec(
                "INSERT OR IGNORE INTO forgotten_refs VALUES (?,?)",
                sql![Self::card_key(&task_id), self.now()],
            )?;
            return Ok(None);
        }
        let strings = |k: &str| -> Vec<String> {
            p.get(k)
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .map(py::py_str)
                .collect()
        };
        let card = self.put(
            "episode",
            &task_id,
            &p["body"],
            &strings("sources"),
            origin::FERB,
            CARD_MAKER,
            None,
            "active",
            false,
            Some(strings("labels").into_iter().collect()),
            true,
        )?;
        if truthy(p.get("inputs")) {
            self.card_read(&task_id, &s(p, "inputs"), None)?;
        }
        Ok(Some(card))
    }

    /// learning.py:553 `prepare_episode`: every read of an episode card, without writing.
    pub fn prepare_episode(
        &self,
        task_id: &str,
        apps: Option<&Map<String, Value>>,
    ) -> Result<Option<Value>> {
        let Some(ep) = self.seams.episode(&self.store, task_id)? else {
            return Ok(None);
        };
        if self.store.exists(
            "SELECT 1 FROM forgotten_refs WHERE ref=?",
            sql![Self::card_key(task_id)],
        )? {
            return Ok(None);
        }
        let empty = Map::new();
        let apps_map = apps.unwrap_or(&empty);
        let mut actions: Vec<Value> = Vec::new();
        let mut source_refs: BTreeSet<String> = BTreeSet::from([format!("episode:{task_id}")]);
        let mut used_apps: BTreeSet<String> = BTreeSet::new();
        let mut traced = false;
        for span in self.card_trace(task_id)? {
            traced = true;
            source_refs.extend(self.span_refs(&span));
            if span.kind != "tool.call" {
                continue;
            }
            let inp = &span.input;
            let result = span.result.clone().unwrap_or_else(|| json!({}));
            let args = inp
                .get("args")
                .filter(|v| ferb_py::value::truthy(v))
                .cloned()
                .unwrap_or_else(|| json!({}));
            if let Some(Value::String(b)) = args.get("bundle_id") {
                used_apps.insert(b.clone());
            }
            let target = inp
                .get("target")
                .filter(|v| ferb_py::value::truthy(v))
                .cloned()
                .unwrap_or_else(|| json!({}));
            let tool = inp.get("tool").map_or_else(String::new, py::py_str);
            if tool.starts_with("mac.") {
                let site = target
                    .get("site")
                    .filter(|v| ferb_py::value::truthy(v))
                    .map(py::py_str);
                let app = args.get("app").and_then(Value::as_str);
                if let Some(site) = site.filter(|s| apps_map.contains_key(s)) {
                    used_apps.insert(site);
                } else if let Some(app) = app.filter(|a| apps_map.contains_key(*a)) {
                    used_apps.insert(app.to_owned());
                } else {
                    let bid = result
                        .get("bundle_id")
                        .filter(|v| ferb_py::value::truthy(v));
                    used_apps.insert(bid.map_or_else(|| "unresolved-mac-app".into(), py::py_str));
                }
            }
            if tool.starts_with("browser.") || tool.starts_with("ferb_browser.") {
                let browsers: Vec<String> = apps_map
                    .keys()
                    .filter(|k| {
                        matches!(
                            k.as_str(),
                            "com.google.Chrome"
                                | "com.brave.Browser"
                                | "com.apple.Safari"
                                | "com.microsoft.edgemac"
                        )
                    })
                    .cloned()
                    .collect();
                if browsers.is_empty() {
                    used_apps.insert("unresolved-browser".into());
                } else {
                    used_apps.extend(browsers);
                }
            }
            let mut facts = Map::new();
            if let Value::Object(r) = &result {
                for k in ["ok", "status", "intent_id", "action_id", "ledger_id"] {
                    if let Some(v) = r.get(k) {
                        facts.insert(k.into(), v.clone());
                    }
                }
            }
            let mut action = json!({"id": span.id, "tool": inp.get("tool").cloned().unwrap_or(Value::Null),
                                    "args": args, "kind": inp.get("kind").cloned().unwrap_or(Value::Null),
                                    "arg_sources": inp.get("arg_sources").cloned().unwrap_or(Value::Null),
                                    "error": span.error, "result": facts});
            if truthy(inp.get("declared")) {
                action["declared"] = inp["declared"].clone();
            }
            actions.push(action);
        }
        let feedback: Vec<Value> = self
            .store
            .query(
                "SELECT verdict,note,at FROM owner_verdicts WHERE task_id=? ORDER BY seq",
                sql![task_id],
            )?
            .iter()
            .map(|r| json!({"verdict": r.s("verdict"), "note": r.s("note"), "at": r.s("at")}))
            .collect();
        let valid = match feedback.last() {
            Some(f) => f["verdict"] == "pass",
            None => ep.get("verified") == Some(&Value::Bool(true)),
        };
        let (hints, hint_refs) = self.owner_hints(task_id)?;
        source_refs.extend(hint_refs);
        let field = |k: &str| ep.get(k).cloned().unwrap_or(Value::Null);
        let apps_body: Map<String, Value> = used_apps
            .iter()
            .map(|a| {
                (
                    a.clone(),
                    apps_map.get(a).cloned().unwrap_or_else(|| json!("unknown")),
                )
            })
            .collect();
        let how = ep
            .get("how")
            .filter(|v| ferb_py::value::truthy(v))
            .cloned()
            .unwrap_or_else(|| json!(""));
        let mut body = json!({"task_id": task_id, "request": field("request_text"), "intent": field("goal"),
            "slots": {}, "apps": apps_body, "app_versions_captured": apps.is_some(),
            "app_version_scope": "identified apps and browser inventory", "surfaces": field("tools"),
            "actions": actions, "status": field("status"), "verified": valid, "conditions": field("conditions"),
            "feedback": feedback, "narrative": field("answer"), "how": how, "problems": field("problems"),
            "started_at": field("started_at"), "ended_at": field("ended_at"), "usage": field("usage")});
        if !hints.is_empty() {
            body["owner_hints"] = json!(hints);
        }
        if let Some(old) = self.current_card(task_id, false)? {
            if apps.is_none() {
                let ob = &old["body"];
                body["apps"] = ob.get("apps").cloned().unwrap_or_else(|| json!({}));
                body["app_versions_captured"] = ob
                    .get("app_versions_captured")
                    .cloned()
                    .unwrap_or(json!(false));
                if body["actions"].as_array().is_none_or(Vec::is_empty) {
                    body["actions"] = ob.get("actions").cloned().unwrap_or_else(|| json!([]));
                }
                if !traced {
                    for r in self.store.query(
                        "SELECT source_ref FROM learning_sources WHERE node_id=?",
                        sql![s(&old, "id")],
                    )? {
                        source_refs.insert(r.s_at(0));
                    }
                }
            }
        }
        let (labels, damaged, expired, forgotten, kept) = self.card_labels(&source_refs)?;
        let inputs = self
            .card_inputs(Some(task_id))?
            .remove(task_id)
            .unwrap_or_default();
        if !forgotten.is_empty() {
            return Ok(Some(
                json!({"task_id": task_id, "forgotten": forgotten, "inputs": inputs}),
            ));
        }
        Ok(Some(
            json!({"task_id": task_id, "body": body, "sources": kept,
                       "labels": labels.into_iter().collect::<Vec<_>>(), "damaged": damaged,
                       "expired": expired, "inputs": inputs}),
        ))
    }

    /// learning.py:637 `card_trace`.
    pub fn card_trace(&self, task_id: &str) -> Result<Vec<CardSpan>> {
        let (wh, params) = ferb_store::spans::tree_where(task_id);
        let rows = self.store.query(
            &format!(
                "SELECT id,kind,input_json,output_json,error FROM spans WHERE kind IN \
                 ('tool.call','context.exposure','context.source') AND {wh} ORDER BY started_at,id"
            ),
            params.iter().map(|p| Sql::Text(p.clone())).collect(),
        )?;
        let mut out = Vec::new();
        for r in rows {
            let kind = r.s("kind");
            let input = ferb_py::json::loads(&r.s("input_json")).map_err(Error::Json)?;
            let result = if kind == "tool.call" {
                Some(match r.opt("output_json").filter(|o| !o.is_empty()) {
                    Some(o) => ferb_py::json::loads(&o).map_err(Error::Json)?,
                    None => json!({}),
                })
            } else {
                None
            };
            out.push(CardSpan {
                id: r.s("id"),
                kind,
                input,
                result,
                error: r.opt("error"),
            });
        }
        Ok(out)
    }

    fn span_refs(&self, span: &CardSpan) -> BTreeSet<String> {
        match span.kind.as_str() {
            "context.exposure" => {
                if span.input.get("reference") == Some(&json!(REQUEST_REFERENCE)) {
                    span.input
                        .get("source_refs")
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                        .map(|r| direct(&py::py_str(r)))
                        .collect()
                } else {
                    BTreeSet::new()
                }
            }
            "context.source" => BTreeSet::new(),
            _ => call_sources(&span.input, span.result.as_ref())
                .iter()
                .map(|r| direct(r))
                .collect(),
        }
    }

    /// learning.py:666 `card_source_refs`.
    pub fn card_source_refs(&self, task_id: &str) -> Result<Option<BTreeSet<String>>> {
        match self.trace_refs(task_id)? {
            None => Ok(None),
            Some(refs) => Ok(Some(self.bounded(&refs)?)),
        }
    }

    fn trace_refs(&self, task_id: &str) -> Result<Option<BTreeSet<String>>> {
        let mut refs = BTreeSet::from([format!("episode:{task_id}")]);
        let mut traced = false;
        for span in self.card_trace(task_id)? {
            traced = true;
            refs.extend(self.span_refs(&span));
        }
        Ok(traced.then_some(refs))
    }

    fn bounded(&self, refs: &BTreeSet<String>) -> Result<BTreeSet<String>> {
        let (_, _, _, forgotten, kept) = self.card_labels(refs)?;
        Ok(kept.into_iter().chain(forgotten).collect())
    }

    /// learning.py:687 `card_source_plan`: what the one-time clean-out removes, read without writing.
    pub fn card_source_plan(
        &self,
        stop: &mut dyn FnMut() -> bool,
        removing: &[String],
    ) -> Result<Value> {
        let mut cards: Vec<(String, Vec<String>)> = Vec::new();
        for r in self.store.query(
            "SELECT id, key FROM learning_nodes WHERE kind='episode' ORDER BY key, recorded_at, id",
            sql![],
        )? {
            let (id, key) = (r.s_at(0), r.s_at(1));
            match cards.iter_mut().find(|(k, _)| *k == key) {
                Some((_, v)) => v.push(id),
                None => cards.push((key, vec![id])),
            }
        }
        let nodes: usize = cards.iter().map(|(_, v)| v.len()).sum();
        *self.memo.borrow_mut() = Some(HashMap::new());
        *self.removing.borrow_mut() = removing.iter().cloned().collect();
        let result = (|| -> Result<Value> {
            let mut drop = BTreeMap::new();
            let (mut untraced, mut stopped) = (0, false);
            for (task_id, ids) in &cards {
                if stop() {
                    stopped = true;
                    break;
                }
                let trace = self.trace_refs(task_id)?;
                for node_id in ids {
                    let have: BTreeSet<String> = self
                        .store
                        .query(
                            "SELECT source_ref FROM learning_sources WHERE node_id=?",
                            sql![node_id],
                        )?
                        .iter()
                        .map(|r| r.s_at(0))
                        .collect();
                    if trace.is_none() {
                        untraced += 1;
                    }
                    let within: BTreeSet<String> = match &trace {
                        Some(t) => have.intersection(t).cloned().collect(),
                        None => have.clone(),
                    };
                    let keep = self.bounded(&within)?;
                    let gone: Vec<String> = have.difference(&keep).cloned().collect();
                    if !gone.is_empty() {
                        drop.insert(node_id.clone(), json!(gone));
                    }
                }
            }
            Ok(
                json!({"tasks": cards.len(), "nodes": nodes, "untraced": untraced, "stopped": stopped,
                      "drop": Value::Object(drop.into_iter().collect())}),
            )
        })();
        *self.memo.borrow_mut() = None;
        self.removing.borrow_mut().clear();
        result
    }

    /// learning.py:725 `prune_card_sources`: only the planned rows go, each node in a transaction of its own; the
    /// ref index is dropped for the deletes and made again on the way out, whatever happens.
    pub fn prune_card_sources(
        &self,
        stop: &mut dyn FnMut() -> bool,
        progress: &mut dyn FnMut(usize, usize),
        rebuilding: &mut dyn FnMut() -> Result<()>,
        plan: Option<Value>,
    ) -> Result<Value> {
        let plan = match plan {
            Some(p) => p,
            None => self.card_source_plan(&mut || false, &[])?,
        };
        let drop = plan["drop"].as_object().cloned().unwrap_or_default();
        let mut counts = json!({"tasks": plan["tasks"], "nodes": plan["nodes"], "cleaned": 0, "rows": 0,
                                "untraced": plan["untraced"], "stopped": false});
        let nodes: Vec<String> = self
            .store
            .query(
                "SELECT id FROM learning_nodes WHERE kind='episode' ORDER BY key, recorded_at, id",
                sql![],
            )?
            .iter()
            .map(|r| r.s_at(0))
            .collect();
        let total = nodes.len().max(1);
        let (mut done, mut dropped, mut finished) = (0usize, false, false);
        let (mut cleaned, mut rows) = (0i64, 0i64);
        let body = (|| -> Result<()> {
            self.store
                .plain("CREATE TEMP TABLE IF NOT EXISTS card_drop (ref TEXT PRIMARY KEY)")?;
            for node_id in &nodes {
                if let Some(gone) = drop
                    .get(node_id)
                    .and_then(Value::as_array)
                    .filter(|g| !g.is_empty())
                {
                    if stop() {
                        counts["stopped"] = json!(true);
                        return Ok(());
                    }
                    if !dropped {
                        self.store
                            .plain("DROP INDEX IF EXISTS learning_source_ref")?;
                        dropped = true;
                    }
                    self.store.exec("DELETE FROM temp.card_drop", sql![])?;
                    self.store.exec_many(
                        "INSERT INTO temp.card_drop VALUES (?)",
                        gone.iter().map(|r| sql![py::py_str(r)]).collect(),
                    )?;
                    self.store.plain("BEGIN IMMEDIATE")?;
                    let removed = match self.store.exec(
                        "DELETE FROM learning_sources WHERE node_id = ? AND source_ref IN \
                         (SELECT ref FROM temp.card_drop)",
                        sql![node_id],
                    ) {
                        Ok(n) => {
                            self.store.plain("COMMIT")?;
                            n as i64
                        }
                        Err(e) => {
                            if self.store.in_transaction() {
                                self.store.plain("ROLLBACK")?;
                            }
                            return Err(e);
                        }
                    };
                    cleaned += i64::from(removed > 0);
                    rows += removed;
                }
                done += 1;
                progress(done, total);
            }
            finished = true;
            Ok(())
        })();
        let told = if dropped && !finished {
            rebuilding()
        } else {
            Ok(())
        };
        let remade = self.store.plain(SOURCE_REF_INDEX);
        let cleared = self.store.plain("DROP TABLE IF EXISTS temp.card_drop");
        body?;
        told?;
        remade?;
        cleared?;
        counts["cleaned"] = json!(cleaned);
        counts["rows"] = json!(rows);
        Ok(counts)
    }

    /// learning.py:797 `card_inputs`.
    pub fn card_inputs(&self, task_id: Option<&str>) -> Result<BTreeMap<String, String>> {
        let mut q = "SELECT e.task_id, e.built_at, e.status, e.verified, (SELECT MAX(v.seq) FROM owner_verdicts v \
                     WHERE v.task_id = e.task_id) FROM episodes e"
            .to_owned();
        let mut params = Vec::new();
        if let Some(t) = task_id.filter(|t| !t.is_empty()) {
            q.push_str(" WHERE e.task_id = ?");
            params.push(Sql::Text(t.into()));
        }
        let mut out = BTreeMap::new();
        for r in self.store.query(&q, params)? {
            let d = digest(&json!([
                CARD_MAKER,
                to_json(r.at(1)),
                to_json(r.at(2)),
                to_json(r.at(3)),
                to_json(r.at(4))
            ]));
            out.insert(r.s_at(0), d);
        }
        Ok(out)
    }

    /// learning.py:805 `cards_to_read` (MEM-422).
    pub fn cards_to_read(&self) -> Result<Vec<String>> {
        let current = self.card_inputs(None)?;
        let mut recorded: HashMap<String, (String, String, String)> = HashMap::new();
        for r in self.store.query(
            "SELECT task_id, inputs, status, error FROM learning_card_inputs",
            sql![],
        )? {
            recorded.insert(
                r.s_at(0),
                (r.s_at(1), r.s_at(2), r.opt_at(3).unwrap_or_default()),
            );
        }
        let gone: BTreeSet<String> = self
            .store
            .query(
                "SELECT ref FROM forgotten_refs WHERE ref LIKE 'key-%'",
                sql![],
            )?
            .iter()
            .map(|r| r.s_at(0))
            .collect();
        let mut out = Vec::new();
        for r in self
            .store
            .query("SELECT task_id FROM episodes ORDER BY ended_at", sql![])?
        {
            let t = r.s_at(0);
            if gone.contains(&Self::card_key(&t)) {
                continue;
            }
            let read = match recorded.get(&t) {
                None => current.get(&t).is_none_or(|c| !c.is_empty()),
                Some((inputs, status, error)) => {
                    Some(inputs) != current.get(&t)
                        || status == "failed"
                        || (status == "parked" && error.contains(PARKED_BY_REDACTION))
                }
            };
            if read {
                out.push(t);
            }
        }
        Ok(out)
    }

    /// learning.py:821 `stuck_cards`.
    pub fn stuck_cards(&self) -> Result<Vec<Value>> {
        Ok(self
            .store
            .query(
                "SELECT task_id, error FROM learning_card_inputs WHERE status = 'parked' ORDER BY task_id",
                sql![],
            )?
            .iter()
            .map(|r| json!({"task_id": r.s_at(0), "error": r.opt_at(1).unwrap_or_default()}))
            .collect())
    }

    /// learning.py:826 `card_read`.
    pub fn card_read(&self, task_id: &str, inputs: &str, error: Option<&str>) -> Result<String> {
        let mut status = "read";
        let error = error.filter(|e| !e.is_empty());
        if error.is_some() {
            let before = self.store.one(
                "SELECT inputs, status FROM learning_card_inputs WHERE task_id=?",
                sql![task_id],
            )?;
            status = match before {
                Some(b)
                    if b.s_at(0) == inputs && matches!(b.s_at(1).as_str(), "failed" | "parked") =>
                {
                    "parked"
                }
                _ => "failed",
            };
        }
        self.store.exec(
            "INSERT INTO learning_card_inputs VALUES (?,?,?,?) ON CONFLICT(task_id) DO UPDATE SET \
             inputs=excluded.inputs, status=excluded.status, error=excluded.error",
            sql![task_id, inputs, status, error.map(str::to_owned)],
        )?;
        Ok(status.into())
    }

    /// learning.py:843 `current_card`.
    pub fn current_card(&self, task_id: &str, sources: bool) -> Result<Option<Value>> {
        match self.store.one(
            "SELECT id FROM learning_nodes WHERE kind='episode' AND key=? AND valid_to IS NULL \
             AND state NOT IN ('retired','rolled_back') ORDER BY recorded_at DESC,rowid DESC LIMIT 1",
            sql![task_id],
        )? {
            Some(r) => self.get(&r.s_at(0), sources),
            None => Ok(None),
        }
    }

    fn owner_hints(&self, task_id: &str) -> Result<(Vec<String>, BTreeSet<String>)> {
        let (mut hints, mut refs) = (Vec::new(), BTreeSet::new());
        for row in self.store.query(
            "SELECT data_json FROM journal WHERE kind='task.steered' AND \
             json_extract(data_json,'$.task_id')=? ORDER BY seq",
            sql![task_id],
        )? {
            let data = ferb_py::json::loads(&row.s_at(0)).map_err(Error::Json)?;
            let source = data
                .get("source_ref")
                .filter(|v| ferb_py::value::truthy(v))
                .map_or_else(String::new, py::py_str);
            let r = direct(&source);
            let Some(id) = r.strip_prefix("journal:") else {
                continue;
            };
            let msg = self.store.one(
                "SELECT data_json FROM journal WHERE id=? AND kind='chat.message'",
                sql![id],
            )?;
            let words = match msg {
                Some(m) => {
                    let d = ferb_py::json::loads(&m.s_at(0)).map_err(Error::Json)?;
                    let text = d
                        .get("text")
                        .filter(|v| ferb_py::value::truthy(v))
                        .map_or_else(String::new, py::py_str);
                    pystr::split_ws(&ferb_memory::search::own_words(&text))
                        .collect::<Vec<_>>()
                        .join(" ")
                }
                None => String::new(),
            };
            if !words.is_empty() {
                hints.push(pystr::prefix(&words, notes::HINT_CHARS).to_owned());
                refs.insert(r.clone());
            }
        }
        Ok((hints, refs))
    }

    fn card_labels(&self, refs: &BTreeSet<String>) -> Result<Labels> {
        let mut labels = BTreeSet::new();
        let (mut damaged, mut expired, mut forgotten, mut kept) =
            (Vec::new(), Vec::new(), Vec::new(), Vec::new());
        let mut seen: BTreeSet<BTreeSet<String>> = BTreeSet::new();
        for r in refs {
            let family = r.split_once(':').map_or(r.as_str(), |(f, _)| f);
            match self.source_origins(r) {
                Ok(found) => {
                    labels.extend(found.iter().cloned());
                    if KEPT_FAMILIES.contains(&family) || seen.insert(found) {
                        kept.push(r.clone());
                    }
                    continue;
                }
                Err(Error::LearningRefused(_)) => {}
                Err(e) => return Err(e),
            }
            if r.contains(ferb_sanitize::REDACTED) {
                damaged.push(r.clone());
            } else if self.forgotten(r)? {
                forgotten.push(r.clone());
                continue;
            } else {
                expired.push(r.clone());
                if KEPT_FAMILIES.contains(&family)
                    || seen.insert(BTreeSet::from(["expired".to_owned()]))
                {
                    kept.push(r.clone());
                }
            }
            labels.insert(origin::PRIVATE.into());
            labels.insert(origin::external("unlabeled", ""));
        }
        Ok((labels, damaged, expired, forgotten, kept))
    }

    /// learning.py:915 `confirm`.
    pub fn confirm(&self, node_id: &str) -> Result<Value> {
        let p = match self.get(node_id, true)? {
            Some(p) if !truthy(p.get("valid_to")) && s(&p, "state") != "rolled_back" => p,
            _ => return Err(refused("no current proposal")),
        };
        if s(&p, "kind") == "identity_proposal" {
            self.put(
                "identity",
                &s(&p, "key"),
                &p["body"],
                &[s(&p, "id")],
                origin::OWNER,
                "owner:confirmed-digest/1",
                None,
                "active",
                true,
                None,
                true,
            )?;
        }
        self.store.exec(
            "UPDATE learning_nodes SET confirmed=1,state=CASE WHEN state='quarantined' \
             THEN 'candidate' ELSE state END WHERE id=?",
            sql![node_id],
        )?;
        (self.publish)("memory.confirmed_derived", json!({"id": node_id}));
        Ok(self.get(node_id, true)?.unwrap_or(Value::Null))
    }

    /// learning.py:931 `edit`.
    pub fn edit(&self, node_id: &str, text: &str) -> Result<Value> {
        let Some(p) = self.get(node_id, true)? else {
            return Err(refused("unknown item"));
        };
        let kind = s(&p, "kind");
        if kind == "episode" {
            return Err(refused(
                "episode evidence is immutable; retire it or edit a separate owner note",
            ));
        }
        let mut body = p["body"].as_object().cloned().unwrap_or_default();
        body.insert("text".into(), json!(text));
        self.put(
            if kind == "identity_proposal" {
                "identity"
            } else {
                &kind
            },
            &s(&p, "key"),
            &Value::Object(body),
            &[s(&p, "id")],
            origin::OWNER,
            "owner:edit/1",
            None,
            "active",
            true,
            None,
            true,
        )
    }

    /// learning.py:941 `identity`.
    pub fn identity(&self) -> Result<String> {
        Ok(self
            .items(Some("identity"), false, true, &[])?
            .iter()
            .filter(|p| s(p, "origin") == origin::OWNER)
            .map(|p| p["body"].get("text").map_or_else(String::new, py::py_str))
            .collect::<Vec<_>>()
            .join("\n"))
    }

    /// learning.py:944 `retire`: the owner's "Not right".
    pub fn retire(&self, node_id: &str) -> Result<Option<Value>> {
        self.store.exec(
            "UPDATE learning_nodes SET state='retired' WHERE id=?",
            sql![node_id],
        )?;
        self.get(node_id, true)
    }

    /// learning.py:949 `digest`.
    pub fn digest(&self) -> Result<Option<Value>> {
        let pending: Vec<Value> = self
            .items(Some("identity_proposal"), false, true, &[])?
            .into_iter()
            .filter(|p| p["confirmed"] != Value::Bool(true))
            .collect();
        let day = py::local_day(&self.clock.now(), self.zone.as_ref());
        if pending.is_empty()
            || self
                .store
                .exists("SELECT 1 FROM learning_digests WHERE day=?", sql![day])?
        {
            return Ok(None);
        }
        let ids: Vec<Value> = pending.iter().map(|p| p["id"].clone()).collect();
        self.store.exec(
            "INSERT INTO learning_digests VALUES (?,?,?)",
            sql![day, encoded(&json!(ids)), self.now()],
        )?;
        let proposals: Vec<Value> = pending
            .iter()
            .map(|p| json!({"id": p["id"], "text": p["body"].get("text").cloned().unwrap_or_else(|| json!(""))}))
            .collect();
        let out = json!({"day": day, "ids": ids, "proposals": proposals});
        (self.publish)("memory.digest", out.clone());
        Ok(Some(out))
    }

    /// learning.py:960 `descendants`.
    pub fn descendants(&self, refs: &BTreeSet<String>) -> Result<BTreeSet<String>> {
        let (mut frontier, mut found) = (refs.clone(), BTreeSet::new());
        while !frontier.is_empty() {
            let now = std::mem::take(&mut frontier);
            for r in &now {
                if self.get(r, true)?.is_some() {
                    found.insert(r.clone());
                }
                for c in self.store.query(
                    "SELECT node_id FROM learning_sources WHERE source_ref=?",
                    sql![r],
                )? {
                    let c = c.s_at(0);
                    if !found.contains(&c) {
                        frontier.insert(c);
                    }
                }
            }
        }
        Ok(found)
    }

    /// learning.py:972 `forget`: every version of a requested node and every node derived from it, each leaving a
    /// tombstone (its id, never its content).
    pub fn forget(&self, refs: &[String]) -> Result<Vec<String>> {
        self.bump();
        let mut versions: BTreeSet<String> = refs.iter().cloned().collect();
        let mut logical: BTreeSet<String> = BTreeSet::new();
        for r in refs {
            if let Some(node) = self.get(r, true)? {
                logical.insert(format!(
                    "key-{}",
                    digest(&json!([node["kind"], node["key"]]))
                ));
                for v in self.store.query(
                    "SELECT id FROM learning_nodes WHERE kind=? AND key=?",
                    sql![s(&node, "kind"), s(&node, "key")],
                )? {
                    versions.insert(v.s_at(0));
                }
            }
        }
        let doomed = self.descendants(&versions)?;
        if !doomed.is_empty() {
            for row in self.store.query(
                &format!(
                    "SELECT kind,key FROM learning_nodes WHERE id IN ({})",
                    marks(doomed.len())
                ),
                doomed.iter().map(|d| Sql::Text(d.clone())).collect(),
            )? {
                if ID_KEYED.contains(&row.s("kind").as_str()) {
                    logical.insert(format!(
                        "key-{}",
                        digest(&json!([row.s("kind"), row.s("key")]))
                    ));
                }
            }
        }
        let now = self.now();
        let mut tombs: BTreeSet<String> = logical;
        tombs.extend(refs.iter().map(|r| direct(r)));
        tombs.extend(doomed.iter().cloned());
        self.store.plain("SAVEPOINT learning_forget")?;
        let done = (|| -> Result<()> {
            self.store.exec_many(
                "INSERT OR IGNORE INTO forgotten_refs VALUES (?,?)",
                tombs.iter().map(|t| sql![t, now]).collect(),
            )?;
            for ident in &doomed {
                self.store
                    .exec("DELETE FROM learning_nodes WHERE id=?", sql![ident])?;
            }
            self.store.plain("RELEASE learning_forget")
        })();
        if let Err(e) = done {
            self.store.plain("ROLLBACK TO learning_forget")?;
            self.store.plain("RELEASE learning_forget")?;
            return Err(e);
        }
        self.store.truncate_wal("memory.forget")?;
        let sorted: Vec<String> = doomed.into_iter().collect();
        (self.publish)("memory.forgot_derived", json!({"ids": sorted}));
        Ok(sorted)
    }

    /// learning.py:1011 `drop_fact_index`: the owner's Drop on one of FERB's guesses deletes the fact nodes that index
    /// that memory item and nothing else; no tombstone.
    pub fn drop_fact_index(&self, r: &str, memory: &Memory) -> Result<Vec<String>> {
        let rows = self.store.query(
            "SELECT n.id, n.supersedes FROM learning_nodes n JOIN learning_sources s ON s.node_id = n.id \
             WHERE s.source_ref = ? AND n.kind = 'fact'",
            sql![r],
        )?;
        let doomed: Vec<String> = rows
            .iter()
            .map(|r| r.s_at(0))
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        if doomed.is_empty() {
            return Ok(Vec::new());
        }
        let closed: BTreeSet<String> = rows
            .iter()
            .filter_map(|r| r.opt_at(1))
            .filter(|c| !c.is_empty() && !doomed.contains(c))
            .collect();
        let mut reopen = Vec::new();
        for c in closed {
            let Some(node) = self.get(&c, true)? else {
                continue;
            };
            let item_refs: Vec<String> = node["sources"]
                .as_array()
                .into_iter()
                .flatten()
                .map(py::py_str)
                .filter(|s| s.starts_with("memory:"))
                .collect();
            if s(&node, "kind") != "fact" || node["valid_to"].is_null() || item_refs.len() != 1 {
                continue;
            }
            let item = memory.get(item_refs[0].trim_start_matches("memory:"))?;
            let ready = match &item {
                Some(it) => prompt_ready(it, &self.clock.now())?,
                None => false,
            };
            if !ready
                || self.store.exists(
                    "SELECT 1 FROM learning_nodes n JOIN learning_sources s ON s.node_id = n.id WHERE s.source_ref = ? \
                     AND n.kind = 'fact' AND n.valid_to IS NULL",
                    sql![item_refs[0]],
                )?
            {
                continue;
            }
            reopen.push(c);
        }
        self.bump();
        self.store.plain("SAVEPOINT learning_drop")?;
        let done = (|| -> Result<()> {
            self.store.exec_many(
                "DELETE FROM learning_nodes WHERE id=?",
                doomed.iter().map(|i| sql![i]).collect(),
            )?;
            self.store.exec_many(
                "UPDATE learning_nodes SET valid_to=NULL WHERE id=?",
                reopen.iter().map(|i| sql![i]).collect(),
            )?;
            self.store.plain("RELEASE learning_drop")
        })();
        if let Err(e) = done {
            self.store.plain("ROLLBACK TO learning_drop")?;
            self.store.plain("RELEASE learning_drop")?;
            return Err(e);
        }
        (self.publish)("memory.forgot_derived", json!({"ids": doomed}));
        Ok(doomed)
    }

    /// learning.py:1052 `rollback`: a batch's nodes and what was built on them are rolled back (a later owner
    /// statement is independent authority and stays); the immediate predecessor reopens when nothing newer holds.
    pub fn rollback(&self, batch: &str) -> Result<Vec<String>> {
        let rows = self.store.query(
            "SELECT * FROM learning_changes WHERE batch_id=? AND rolled_back=0 ORDER BY seq DESC",
            sql![batch],
        )?;
        let mut changed = Vec::new();
        for r in rows {
            let node_id = r.s("node_id");
            if self.get(&node_id, true)?.is_some() {
                for ident in self.descendants(&BTreeSet::from([node_id.clone()]))? {
                    if ident != node_id {
                        let o = self
                            .get(&ident, true)?
                            .map(|n| s(&n, "origin"))
                            .unwrap_or_default();
                        if o == origin::OWNER {
                            continue;
                        }
                    }
                    self.store.exec(
                        "UPDATE learning_nodes SET valid_to=?,state='rolled_back' WHERE id=?",
                        sql![self.now(), ident],
                    )?;
                }
                if let Some(prior_id) = r.opt("prior_id").filter(|p| !p.is_empty()) {
                    if let Some(prior) = self.get(&prior_id, true)? {
                        let newer = self.store.exists(
                            "SELECT 1 FROM learning_nodes WHERE kind=? AND key=? AND valid_to IS NULL",
                            sql![s(&prior, "kind"), s(&prior, "key")],
                        )?;
                        if !newer {
                            self.store.exec(
                                "UPDATE learning_nodes SET valid_to=NULL WHERE id=?",
                                sql![prior_id],
                            )?;
                        }
                    }
                }
                changed.push(node_id);
            }
            self.store.exec(
                "UPDATE learning_changes SET rolled_back=1 WHERE seq=?",
                sql![r.i("seq")],
            )?;
        }
        (self.publish)("memory.rollback", json!({"batch": batch, "ids": changed}));
        Ok(changed)
    }

    /// learning.py:1075 `weighted_terms`.
    pub fn weighted_terms(&self, words: &[String]) -> Result<Vec<String>> {
        let total = self
            .store
            .count("SELECT COUNT(*) FROM learning_fts WHERE text != ''", sql![])?;
        weighted(words, total, |term| {
            self.store.count(
                "SELECT COUNT(*) FROM learning_fts WHERE learning_fts MATCH ?",
                sql![format!("\"{}\"", term.replace('"', "\"\""))],
            )
        })
    }

    fn bm25(&self, terms: &[String], positional: bool) -> Result<Ranking> {
        let rows = self.store.query(
            "SELECT id, bm25(learning_fts) FROM learning_fts WHERE learning_fts MATCH ? ORDER BY 2",
            sql![match_expr(terms)],
        )?;
        Ok(tied_rows(&rows, positional))
    }

    /// learning.py:1087 `search`: the one ranked search over learned nodes.
    pub fn search(&self, query: &str, q: &Query<'_>) -> Result<Vec<Value>> {
        let words = query_terms(query);
        let terms = if words.is_empty() {
            Vec::new()
        } else {
            self.weighted_terms(&words)?
        };
        if !words.is_empty() && terms.is_empty() {
            return Ok(Vec::new());
        }
        let mut rankings: Vec<Ranking> = Vec::new();
        if !terms.is_empty() {
            rankings.push(self.bm25(&terms, false)?);
            let focused = if q.focus.is_empty() {
                Vec::new()
            } else {
                self.weighted_terms(&query_terms(q.focus))?
            };
            if !focused.is_empty() {
                rankings.push(self.bm25(&focused, true)?);
            }
            if pystr::char_len(query) >= 3 {
                let rows = self.store.query(
                    "SELECT id, bm25(learning_titles) FROM learning_titles WHERE learning_titles MATCH ? ORDER BY 2",
                    sql![format!("\"{}\"", query.replace('"', "\"\""))],
                )?;
                rankings.push(tied_rows(&rows, false));
            }
        } else {
            let kind = q.kind.filter(|k| !k.is_empty()).map(str::to_owned);
            let listed = self.store.query(
                "SELECT id FROM learning_nodes WHERE valid_to IS NULL AND state NOT IN \
                 ('retired','rolled_back') AND (? IS NULL OR kind = ?) ORDER BY recorded_at DESC, id DESC",
                sql![kind.clone(), kind],
            )?;
            let mut ranking = Ranking::default();
            for (n, r) in listed.iter().enumerate() {
                ranking.set(&r.s_at(0), n);
            }
            rankings.push(ranking);
        }
        if let Some(embedder) = self.embedder().filter(|_| !terms.is_empty() && !q.no_embed) {
            let vector = q.query_vector.clone().or_else(|| embedder.encode(query));
            if let Some(vector) = vector {
                match ferb_memory::vectors::nearest(
                    self.store.conn(),
                    embedder.name(),
                    &vector,
                    &NODE_KINDS,
                    NEAREST,
                ) {
                    Ok(near) => {
                        let mut ranking = Ranking::default();
                        for (n, (doc, _)) in near
                            .iter()
                            .filter(|(_, s)| *s >= MIN_SIMILARITY)
                            .enumerate()
                        {
                            ranking.set(doc, n);
                        }
                        rankings.push(ranking);
                    }
                    Err(e) => (self.publish)(
                        "memory.embedding_unavailable",
                        json!({"error": (self.redact)(&e.to_string())}),
                    ),
                }
            }
        }
        let scores = fused(&rankings);
        let now = self.clock.now();
        let mut order: Vec<(f64, bool, String, String, f64)> = Vec::new();
        let score_of: HashMap<&str, f64> = scores.iter().map(|(k, v)| (k.as_str(), *v)).collect();
        for row in self.columns(scores.iter().map(|(k, _)| k.clone()).collect())? {
            let kind = row.s("kind");
            let state = row.s("state");
            if row.opt("valid_to").is_some_and(|v| !v.is_empty())
                || matches!(state.as_str(), "retired" | "rolled_back" | "proposed")
                || q.kind.is_some_and(|k| !k.is_empty() && kind != k)
                || q.leave_out.contains(&kind.as_str())
                || kind == "episode_intent"
            {
                continue;
            }
            let vf = row.s("valid_from");
            if q.since.is_some_and(|s| !s.is_empty() && vf.as_str() < s)
                || q.until.is_some_and(|u| !u.is_empty() && vf.as_str() > u)
            {
                continue;
            }
            let recorded = row.s("recorded_at");
            let mut at = py::parse(&recorded)?;
            if at.offset.is_none() != now.offset.is_none() {
                at.offset = now.offset;
            }
            let owner = row.s("origin") == origin::OWNER;
            let age = if owner {
                0.0
            } else {
                (times::total_seconds(times::minus(&now, &at)?) / 86400.0).max(0.0)
            };
            let boost = if words.is_empty() {
                0.0
            } else {
                RECENCY_WEIGHT * 0.5f64.powf(age / RECENCY_HALF_LIFE_DAYS)
            };
            let id = row.s("id");
            let score = score_of[id.as_str()] + boost;
            order.push((
                ferb_memory::pymath::round(score, 9),
                !words.is_empty() && owner,
                recorded,
                id,
                score,
            ));
        }
        order.sort_by(|a, b| {
            b.0.partial_cmp(&a.0)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(b.1.cmp(&a.1))
                .then(b.2.cmp(&a.2))
                .then(b.3.cmp(&a.3))
                .then(b.4.partial_cmp(&a.4).unwrap_or(std::cmp::Ordering::Equal))
        });
        let mut out = Vec::new();
        for (_, _, _, ident, score) in order {
            if q.limit.is_some_and(|l| out.len() >= l) {
                break;
            }
            let Some(mut p) = self.get(&ident, !q.no_sources)? else {
                continue;
            };
            let body = p["body"].clone();
            let intent = body
                .get("intent")
                .filter(|v| ferb_py::value::truthy(v))
                .map_or_else(String::new, py::py_str);
            if s(&p, "kind") == "episode" && pystr::strip(&intent).is_empty() {
                continue;
            }
            if let Some(app) = q.app {
                let in_apps = body
                    .get("apps")
                    .and_then(Value::as_object)
                    .is_some_and(|m| m.contains_key(app));
                let scope = body
                    .get("scope")
                    .and_then(|s| s.get("app"))
                    .and_then(Value::as_str)
                    == Some(app);
                let pre = body
                    .get("preconditions")
                    .and_then(|p| p.get("apps"))
                    .and_then(Value::as_object)
                    .is_some_and(|m| m.contains_key(app));
                if !in_apps && !scope && !pre {
                    continue;
                }
            }
            if let Some(person) = q.person {
                if !pystr::casefold(&encoded(&body)).contains(&pystr::casefold(person)) {
                    continue;
                }
            }
            p["score"] = json!(score);
            out.push(p);
        }
        Ok(out)
    }

    fn columns(&self, ids: Vec<String>) -> Result<Vec<Row>> {
        let mut rows = Vec::new();
        for chunk in ids.chunks(500) {
            rows.extend(self.store.query(
                &format!(
                    "SELECT id, kind, origin, state, valid_to, valid_from, recorded_at FROM learning_nodes WHERE id IN ({})",
                    marks(chunk.len())
                ),
                chunk.iter().map(|i| Sql::Text(i.clone())).collect(),
            )?);
        }
        Ok(rows)
    }
}

/// learning.py:1198 `index_summaries`: each day summary and narrative as an `activity_summary` node, again only
/// when its journal row changed.
pub fn index_summaries(
    learning: &Learning,
    seams: &dyn crate::summary::Seams,
    stop: &mut dyn FnMut() -> bool,
) -> Result<()> {
    use crate::summary::{build, NARRATIVE_KIND, SUMMARY_KIND};
    let store = learning.store.clone();
    for row in store.query(
        "SELECT id,kind,data_json FROM journal WHERE kind IN (?,?)",
        sql![SUMMARY_KIND, NARRATIVE_KIND],
    )? {
        let data = ferb_py::json::loads(&row.s("data_json")).map_err(Error::Json)?;
        let Some(day) = data
            .get("day")
            .filter(|v| ferb_py::value::truthy(v))
            .map(py::py_str)
        else {
            continue;
        };
        let (kind, id) = (row.s("kind"), row.s("id"));
        if store.exists(
            "SELECT 1 FROM learning_nodes n JOIN learning_sources s ON s.node_id = n.id WHERE n.kind = \
             'activity_summary' AND n.key = ? AND n.valid_to IS NULL AND s.source_ref = ?",
            sql![format!("{kind}:{day}"), format!("journal:{id}")],
        )? {
            continue;
        }
        if stop() {
            return Ok(());
        }
        let ev = build(&store, seams, learning.zone.as_ref(), &day)?;
        let raw: BTreeSet<String> = ev
            .handles
            .iter()
            .flat_map(|(_, refs)| refs.iter().map(|(k, i)| format!("{k}:{i}")))
            .collect();
        let mut sources = vec![format!("journal:{id}")];
        sources.extend(raw);
        let made_by = data
            .get("made_by")
            .filter(|v| ferb_py::value::truthy(v))
            .map_or_else(|| "code:summary-index/1".into(), py::py_str);
        match learning.put(
            "activity_summary",
            &format!("{kind}:{day}"),
            &data,
            &sources,
            origin::INFERRED,
            &made_by,
            None,
            "active",
            false,
            None,
            true,
        ) {
            Ok(_) | Err(Error::LearningRefused(_)) => {}
            Err(e) => return Err(e),
        }
    }
    Ok(())
}
