//! The records the supervisor writes and reads through FERB's store, as Python's `Journal`, `SpanStore` and `TaskTree`
//! write them (ferb/memory/journal.py, ferb/store/spans.py, ferb/orchestrator/tree.py), each write under the store's
//! lock and never across an await.

use std::collections::BTreeSet;
use std::sync::MutexGuard;

use ferb_core::types::Task;
use ferb_orchestrator::continuity::Continuity;
use ferb_orchestrator::plan_store::PlanStore;
use ferb_orchestrator::tree::{TaskRecord, TaskTree};
use ferb_sanitize::origin;
use ferb_store::spans::{Finish, Span, SpanStore};
use ferb_store::Db;
use rusqlite::types::Value as Sql;
use serde_json::{json, Map, Value};

use super::Inner;

/// One journal row (journal.py `JournalEntry`), its time as Python's `isoformat()`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Entry {
    pub id: String,
    pub at: String,
    pub kind: String,
    pub data: Value,
    pub origin: String,
    /// `data["request_id"]` as text ("" when absent).
    pub request_id: String,
}

impl Entry {
    pub fn get(&self, key: &str) -> Option<&Value> {
        self.data.get(key)
    }

    /// `str(data.get(key, ""))`.
    pub fn text(&self, key: &str) -> String {
        match self.data.get(key) {
            None | Some(Value::Null) if key.is_empty() => String::new(),
            None => String::new(),
            Some(v) => ferb_py::value::py_str(v),
        }
    }

    pub fn row(&self) -> crate::seams::JournalRow {
        crate::seams::JournalRow {
            id: self.id.clone(),
            at: ferb_clock::py::parse(&self.at).expect("a journal time"),
            kind: self.kind.clone(),
            data: self.data.as_object().cloned().unwrap_or_default(),
            origin: self.origin.clone(),
        }
    }
}

/// journal.py:30 `journal_origin`.
pub fn journal_origin(kind: &str, data: &Value) -> String {
    if kind == "chat.message" {
        let extra = match data.get("untrusted_parts") {
            None => vec![origin::external("unlabeled", "")],
            Some(p) if ferb_py::value::truthy(p) => vec![origin::external("paste", "")],
            _ => vec![],
        };
        let mut all = vec![origin::OWNER.to_string()];
        all.extend(extra);
        return origin::join(all.iter().map(String::as_str));
    }
    if matches!(
        kind,
        "owner.verdict" | "memory.added" | "memory.edited" | "approval.resolved"
    ) {
        return origin::OWNER.into();
    }
    origin::FERB.into()
}

fn entry_of(row: &Map<String, Value>) -> Entry {
    let s = |k: &str| row.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    let data = ferb_py::json::loads(&s("data_json")).unwrap_or(Value::Null);
    let kind = s("kind");
    let origin = match row.get("origin") {
        Some(Value::String(o)) if !o.is_empty() => o.clone(),
        _ => journal_origin(&kind, &data),
    };
    let request_id = match data.get("request_id") {
        None | Some(Value::Null) => String::new(),
        Some(v) => ferb_py::value::py_str(v),
    };
    Entry {
        id: s("id"),
        at: s("at"),
        kind,
        data,
        origin,
        request_id,
    }
}

struct Clock<'a>(&'a Inner);

impl ferb_store::spans::Clock for Clock<'_> {
    fn now(&self) -> ferb_py::time::DateTime {
        ferb_core::when::to_py(&self.0.now()).expect("the clock's time")
    }
}

struct Ids<'a>(&'a Inner);

impl ferb_store::spans::IdSource for Ids<'_> {
    fn new_id(&self) -> String {
        self.0.deps.ids.new_id()
    }
}

impl Inner {
    pub fn db(&self) -> MutexGuard<'_, Db> {
        self.deps.db.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn now(&self) -> ferb_clock::py::Time {
        self.deps.clock.now()
    }

    /// `clock.now().isoformat()`.
    pub fn now_iso(&self) -> String {
        ferb_clock::py::iso(&self.deps.clock.now())
    }

    pub fn monotonic(&self) -> f64 {
        self.deps.clock.monotonic()
    }

    pub fn new_id(&self) -> String {
        self.deps.ids.new_id()
    }

    /// `gateway.redact` over JSON.
    pub fn redact(&self, v: &Value) -> Value {
        self.deps.redactor.value(v)
    }

    /// `gateway.redact` over a text.
    pub fn redact_text(&self, text: &str) -> String {
        self.deps.redactor.text(text)
    }

    pub fn wrap(&self, text: &str) -> String {
        (self.deps.wrap)(text)
    }

    /// journal.py:54 `append`: redacted, never dropped.
    pub fn journal(&self, kind: &str, data: Value, made: Option<&str>) -> Entry {
        let body = self.redact(&data);
        let origin = match made {
            Some(o) if !o.is_empty() => o.to_string(),
            _ => journal_origin(kind, &body),
        };
        let id = self.new_id();
        let at = self.now_iso();
        let db = self.db();
        db.conn()
            .execute(
                "INSERT INTO journal (id, at, kind, data_json, origin) VALUES (?, ?, ?, ?, ?)",
                rusqlite::params![id, at, kind, ferb_py::json::dumps_utf8(&body), origin],
            )
            .expect("a journal write");
        let request_id = match body.get("request_id") {
            None | Some(Value::Null) => String::new(),
            Some(v) => ferb_py::value::py_str(v),
        };
        Entry {
            id,
            at,
            kind: kind.into(),
            data: body,
            origin,
            request_id,
        }
    }

    /// journal.py:90 `of_kind(kind, field, equals, missing)`: oldest first.
    pub fn of_kind(
        &self,
        kind: &str,
        field: &str,
        equals: &str,
        missing: Option<&str>,
    ) -> Vec<Entry> {
        let db = self.db();
        let rows = if field.is_empty() {
            ferb_orchestrator::sql::rows(
                db.conn(),
                "SELECT * FROM journal WHERE kind = ? ORDER BY seq",
                &[Sql::Text(kind.into())],
            )
        } else {
            ferb_orchestrator::sql::rows(
                db.conn(),
                "SELECT * FROM journal WHERE kind = ? AND COALESCE(json_extract(data_json, ?), ?) = ? ORDER BY seq",
                &[
                    Sql::Text(kind.into()),
                    Sql::Text(format!("$.{field}")),
                    missing.map(|m| Sql::Text(m.into())).unwrap_or(Sql::Null),
                    Sql::Text(equals.into()),
                ],
            )
        }
        .expect("a journal read");
        rows.iter().map(entry_of).collect()
    }

    /// journal.py `entries_of(*kinds)`.
    pub fn entries_of(&self, kinds: &[&str]) -> Vec<Entry> {
        let db = self.db();
        let marks = vec!["?"; kinds.len()].join(",");
        let params: Vec<Sql> = kinds.iter().map(|k| Sql::Text((*k).into())).collect();
        ferb_orchestrator::sql::rows(
            db.conn(),
            &format!("SELECT * FROM journal WHERE kind IN ({marks}) ORDER BY seq"),
            &params,
        )
        .expect("a journal read")
        .iter()
        .map(entry_of)
        .collect()
    }

    pub(crate) fn with_spans<T>(&self, f: impl FnOnce(&SpanStore<'_>) -> T) -> T {
        let db = self.db();
        let redactor = self.deps.redactor.clone();
        let store = SpanStore::new(
            &db,
            Box::new(Clock(self)),
            Box::new(Ids(self)),
            Box::new(move |v: &Value| redactor.value(v)),
            Box::new(|kind: &str| ferb_policy::rules::REVERSIBLE.contains(&kind)),
        );
        f(&store)
    }

    /// spans.py:59 `start`: the new span's id.
    pub fn span_start(
        &self,
        kind: &str,
        task_id: Option<&str>,
        parent_id: Option<&str>,
        device_id: Option<&str>,
        input: Value,
    ) -> String {
        self.with_spans(|s| {
            s.start(kind, &input, task_id, parent_id, device_id)
                .expect("a span write")
                .id
        })
    }

    /// spans.py:74 `finish`.
    pub fn span_finish(
        &self,
        span_id: &str,
        output: Option<Value>,
        error: Option<&str>,
        made: Option<&str>,
        cost_usd: Option<f64>,
    ) {
        let started = ferb_core::when::to_py(&self.now()).expect("a time");
        let mut span = Span {
            id: span_id.to_string(),
            parent_id: None,
            kind: String::new(),
            task_id: None,
            device_id: None,
            started_at: started,
            ended_at: None,
            input: Value::Null,
            output: None,
            cost_usd: None,
            error: None,
            origin: None,
            summary: None,
        };
        self.with_spans(|s| {
            s.finish(
                &mut span,
                Finish {
                    output: output.as_ref(),
                    error,
                    cost_usd,
                    origin: made,
                    summary: None,
                },
            )
            .expect("a span write")
        })
    }

    pub fn span_set_origin(&self, span_id: &str, made: &str) {
        self.with_spans(|s| s.set_origin(span_id, made).expect("a span write"))
    }

    pub fn span_update_input(&self, span_id: &str, values: &Value) {
        self.with_spans(|s| s.update_input(span_id, values).expect("a span write"))
    }

    pub fn task_origins(&self, root: &str) -> BTreeSet<String> {
        self.with_spans(|s| s.task_origins(root).unwrap_or_default())
    }

    pub fn request_origins_of(&self, root: &str) -> BTreeSet<String> {
        self.with_spans(|s| s.request_origins(root).unwrap_or_default())
    }

    pub fn tool_outputs(&self, task_id: &str) -> Vec<(i64, BTreeSet<String>, String)> {
        self.with_spans(|s| s.tool_outputs(task_id, 0, false).unwrap_or_default())
    }

    /// `spans.payload_outputs(root)` as carried.py reads them.
    pub fn payload_outputs(&self, root: &str) -> Vec<ferb_policy::carried::Output> {
        self.with_spans(|s| s.payload_outputs(root, 0, None).unwrap_or_default())
            .into_iter()
            .map(|o| ferb_policy::carried::Output {
                origins: o.origins,
                text: o.text,
                segments: o
                    .segments
                    .unwrap_or_default()
                    .iter()
                    .map(ferb_policy::carried::Segment::from_json)
                    .collect(),
                page: o.page,
            })
            .collect()
    }

    pub fn owner_questions(&self, root: &str) -> Vec<Value> {
        self.with_spans(|s| {
            s.owner_questions(root)
                .unwrap_or_default()
                .into_iter()
                .map(|(q, a)| json!({"question": q, "answer": a}))
                .collect()
        })
    }

    pub fn reopened_answers(&self, root: &str) -> BTreeSet<String> {
        self.with_spans(|s| s.reopened_answers(root).unwrap_or_default())
    }

    pub fn request_began(
        &self,
        root: &str,
        request_id: &str,
        task_id: &str,
    ) -> Option<(String, i64)> {
        self.with_spans(|s| s.request_began(root, request_id, task_id).unwrap_or(None))
    }

    pub fn tree_json(&self, task_id: &str) -> Vec<Value> {
        self.with_spans(|s| s.tree_json(task_id).unwrap_or_default())
    }

    pub fn span_outcome(&self, task_id: &str) -> Value {
        self.with_spans(|s| s.outcome(task_id).unwrap_or(Value::Null))
    }

    // --- tasks (tree.py `TaskTree`) ---

    pub(crate) fn tasks<T>(&self, f: impl FnOnce(&TaskTree<'_>) -> T) -> T {
        let db = self.db();
        let now = self.now_iso();
        let tree = TaskTree::new(Some(&db), Box::new(move || now.clone()));
        f(&tree)
    }

    /// tree.py:62 `add`: the row and the live record.
    pub fn task_add(&self, task: &Task, span_id: &str, tree_id: &str, status: &str) {
        self.live()
            .records
            .insert(task.id.clone(), (tree_id.to_string(), span_id.to_string()));
        let rec = TaskRecord {
            task: task.clone(),
            started_at: self.now_iso(),
            span_id: span_id.to_string(),
            tree_id: Some(tree_id.to_string()),
        };
        // the gateway reads the same live record (Python shares the one TaskTree)
        self.deps.daemon.task_record(&super::LiveTask {
            task_id: task.id.clone(),
            request_id: task.request_id.clone(),
            started_at: rec.started_at.clone(),
            deadline: task.deadline.as_ref().map(ferb_core::when::isoformat),
            span_id: span_id.to_string(),
            tree_id: Some(tree_id.to_string()),
        });
        self.tasks(|t| t.add(rec, status).expect("a task write"));
    }

    /// tree.py:76 `hold`: a live record with no row.
    pub fn task_hold(&self, task_id: &str, span_id: &str, tree_id: &str) {
        self.live().records.insert(
            task_id.to_string(),
            (tree_id.to_string(), span_id.to_string()),
        );
    }

    pub fn task_known(&self, task_id: &str) -> bool {
        self.live().records.contains_key(task_id)
    }

    pub fn task_span(&self, task_id: &str) -> Option<String> {
        self.live().records.get(task_id).map(|(_, s)| s.clone())
    }

    pub fn task_set_span(&self, task_id: &str, span_id: &str) {
        if let Some(r) = self.live().records.get_mut(task_id) {
            r.1 = span_id.to_string();
        }
        self.deps.daemon.task_record_span(task_id, span_id);
    }

    pub fn task_remove(&self, task_id: &str) {
        self.live().records.remove(task_id);
        self.deps.daemon.task_record_removed(task_id);
    }

    /// supervisor.py:1773 `_set_status`.
    pub fn set_status(&self, task_id: &str, status: &str, outcome: Option<&Value>) {
        let at = self.now_iso();
        self.tasks(|t| {
            t.set_status(task_id, status, outcome, &at)
                .expect("a task write")
        });
    }

    pub fn task_rows(&self, tree_id: &str) -> Vec<Map<String, Value>> {
        self.tasks(|t| t.rows(tree_id).unwrap_or_default())
    }

    pub fn task_column(&self, task_id: &str, name: &str) -> Option<Value> {
        self.tasks(|t| t.column(task_id, name).unwrap_or(None))
    }

    // --- plans and continuity ---

    pub fn plans<T>(&self, f: impl FnOnce(&PlanStore<'_>) -> T) -> T {
        let db = self.db();
        let redactor = self.deps.redactor.clone();
        let store = PlanStore::new(&db, std::sync::Arc::new(move |v: &Value| redactor.value(v)));
        f(&store)
    }

    pub fn continuity<T>(&self, f: impl FnOnce(&Continuity<'_>) -> T) -> T {
        let db = self.db();
        let redactor = self.deps.redactor.clone();
        let me = self.me.clone();
        let store = Continuity::new(
            &db,
            std::sync::Arc::new(move |v: &Value| redactor.value(v)),
            std::sync::Arc::new(move |root: &str| {
                me.upgrade()
                    .map(|i| i.task_origins_unlocked(root))
                    .unwrap_or_default()
            }),
            std::sync::Arc::new(journal_origin),
            self.deps.new_chat.clone(),
        );
        f(&store)
    }

    /// `spans.task_origins` read while the store is already held (continuity's lookup runs inside its own read).
    fn task_origins_unlocked(&self, _root: &str) -> BTreeSet<String> {
        // Continuity reads a task's origins only to label the records it returns to a lookup (`tasks`, `entries`),
        // which the forward path doesn't call; the store's lock is held by the caller, so it can't be read here.
        BTreeSet::new()
    }

    pub fn live(&self) -> MutexGuard<'_, super::Live> {
        self.live.lock().unwrap_or_else(|e| e.into_inner())
    }
}
