//! Replays rust/fixtures/orchestrator/stores.json (made by make.py / stores.py from the Python build): each scenario
//! seeds a migrated store with the rows Python's setup wrote, runs the same steps through the Rust port, and compares
//! every step's return (or the error it raised) and every row of the tables the steps wrote.
//!
//! decision: ENG-3, ORCH-322, ORCH-323, ORCH-319, ORCH-332, ORCH-505, ORCH-576, ORCH-613, ORCH-629

#![cfg(feature = "store")]

mod common;

use std::collections::BTreeSet;
use std::rc::Rc;
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use common::{fixture, fx, real_tempdir, same, unfx, TestPolicy};
use ferb_context::Py;
use ferb_orchestrator::ports::{BoxFuture, Journal, JournalEntry, Redact};
use ferb_orchestrator::sql::Error;
use ferb_orchestrator::*;
use ferb_store::Db;
use rusqlite::types::Value as Sql;
use serde_json::{json, Map, Value};

// ---------------------------------------------------------------------------------------------------------------
// The scenario's world: a store, the FakeClock, sequential ids, the redactor, the journal
// ---------------------------------------------------------------------------------------------------------------

struct World {
    db: &'static Db,
    seconds: Arc<AtomicI64>,
    ids: Arc<AtomicU64>,
    redactor: Arc<ferb_sanitize::Redactor>,
    root: String,
}

fn iso(seconds: i64) -> String {
    let t = ferb_py::time::DateTime::utc_from_unix(1_767_258_000 + seconds, 0);
    t.isoformat(ferb_py::time::Timespec::Auto)
}

impl World {
    fn now(&self) -> String {
        iso(self.seconds.load(Ordering::SeqCst))
    }

    fn now_fn(&self) -> Arc<dyn Fn() -> String + Send + Sync> {
        let s = self.seconds.clone();
        Arc::new(move || iso(s.load(Ordering::SeqCst)))
    }

    fn ids_fn(&self) -> Arc<dyn Fn() -> String + Send + Sync> {
        let n = self.ids.clone();
        Arc::new(move || format!("id-{:06}", n.fetch_add(1, Ordering::SeqCst) + 1))
    }

    fn redact(&self) -> Redact {
        let r = self.redactor.clone();
        Arc::new(move |v: &Value| r.value(v))
    }

    fn journal(&self) -> Rc<dyn Journal> {
        Rc::new(TestJournal {
            db: self.db,
            now: self.now_fn(),
            ids: self.ids_fn(),
            redact: self.redact(),
        })
    }

    fn spans(&self) -> ferb_store::spans::SpanStore<'static> {
        struct C(Arc<AtomicI64>);
        impl ferb_store::spans::Clock for C {
            fn now(&self) -> ferb_py::time::DateTime {
                ferb_py::time::DateTime::utc_from_unix(
                    1_767_258_000 + self.0.load(Ordering::SeqCst),
                    0,
                )
            }
        }
        struct I(Arc<AtomicU64>);
        impl ferb_store::spans::IdSource for I {
            fn new_id(&self) -> String {
                format!("id-{:06}", self.0.fetch_add(1, Ordering::SeqCst) + 1)
            }
        }
        let r = self.redactor.clone();
        ferb_store::spans::SpanStore::new(
            self.db,
            Box::new(C(self.seconds.clone())),
            Box::new(I(self.ids.clone())),
            Box::new(move |v: &Value| r.value(v)),
            Box::new(|k: &str| matches!(k, "read" | "navigate" | "search" | "open_tab" | "draft")),
        )
    }
}

/// ferb/memory/journal.py `Journal` on the scenario's store (its `journal_origin` rule for the kinds used here).
struct TestJournal {
    db: &'static Db,
    now: Arc<dyn Fn() -> String + Send + Sync>,
    ids: Arc<dyn Fn() -> String + Send + Sync>,
    redact: Redact,
}

fn journal_origin(kind: &str, data: &Value) -> String {
    use ferb_sanitize::origin;
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

impl Journal for TestJournal {
    fn append(&self, kind: &str, data: Value, origin: Option<&str>) -> JournalEntry {
        let body = (self.redact)(&data);
        let entry = JournalEntry {
            id: (self.ids)(),
            at: (self.now)(),
            kind: kind.into(),
            origin: origin
                .map(String::from)
                .unwrap_or_else(|| journal_origin(kind, &body)),
            data: body,
        };
        self.db
            .conn()
            .execute(
                "INSERT INTO journal (id, at, kind, data_json, origin) VALUES (?, ?, ?, ?, ?)",
                rusqlite::params![
                    entry.id,
                    entry.at,
                    entry.kind,
                    ferb_py::json::dumps_utf8(&entry.data),
                    entry.origin
                ],
            )
            .unwrap();
        entry
    }

    fn of_kind(&self, kind: &str, field: &str, equals: &str) -> Vec<JournalEntry> {
        let rows = sql::rows(
            self.db.conn(),
            "SELECT * FROM journal WHERE kind = ? AND COALESCE(json_extract(data_json, ?), ?) = ? ORDER BY seq",
            &[Sql::Text(kind.into()), Sql::Text(format!("$.{field}")), Sql::Null, Sql::Text(equals.into())],
        )
        .unwrap();
        rows.iter()
            .map(|r| {
                let data = sql::loads(&sql::col(r, "data_json")).unwrap();
                let origin = match r.get("origin") {
                    Some(Value::String(o)) if !o.is_empty() => o.clone(),
                    _ => journal_origin(&sql::col(r, "kind"), &data),
                };
                JournalEntry {
                    id: sql::col(r, "id"),
                    at: sql::col(r, "at"),
                    kind: sql::col(r, "kind"),
                    data,
                    origin,
                }
            })
            .collect()
    }
}

fn open_world(scenario: &Value) -> (tempfile::TempDir, World) {
    let (dir, real) = real_tempdir();
    let root = real.to_string_lossy().into_owned();
    std::fs::create_dir_all(real.join("user")).unwrap();
    let db: &'static Db = Box::leak(Box::new(Db::open(real.join("ferb.db")).unwrap()));
    db.migrate().unwrap();
    for (table, dump) in scenario["seed"].as_object().unwrap() {
        let dump = unfx(dump, &root);
        db.conn()
            .execute(&format!("DELETE FROM {table}"), [])
            .unwrap();
        let cols: Vec<String> = dump["columns"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c.as_str().unwrap().to_string())
            .collect();
        let names: Vec<String> = cols
            .iter()
            .map(|c| {
                if c == "rowid_" {
                    "rowid".into()
                } else {
                    c.clone()
                }
            })
            .collect();
        for row in dump["rows"].as_array().unwrap() {
            let values: Vec<Sql> = row.as_array().unwrap().iter().map(sql::to_sql).collect();
            db.conn()
                .execute(
                    &format!(
                        "INSERT INTO {table} ({}) VALUES ({})",
                        names.join(","),
                        vec!["?"; names.len()].join(",")
                    ),
                    rusqlite::params_from_iter(values),
                )
                .unwrap();
        }
    }
    let world = World {
        db,
        seconds: Arc::new(AtomicI64::new(scenario["clock_from"].as_i64().unwrap())),
        ids: Arc::new(AtomicU64::new(scenario["ids_from"].as_u64().unwrap())),
        redactor: Arc::new(ferb_sanitize::Redactor::new(&[], &[])),
        root,
    };
    (dir, world)
}

/// Python's step result: `{"out": ...}` or `{"raised": [type, text]}`.
fn want(step: &Value) -> Value {
    match step.get("raised") {
        Some(r) => json!({"raised": r[1]}),
        None => step.get("out").cloned().unwrap_or(Value::Null),
    }
}

fn raised(e: Error) -> Value {
    json!({"raised": e.to_string()})
}

fn out<T: Into<Value>>(r: Result<T, Error>) -> Value {
    match r {
        Ok(v) => v.into(),
        Err(e) => raised(e),
    }
}

fn s(v: &Value) -> String {
    v.as_str().map(String::from).unwrap_or_default()
}

fn opt(v: &Value) -> Option<String> {
    v.as_str().map(String::from)
}

fn strs(v: &Value) -> Vec<String> {
    v.as_array()
        .map(|a| a.iter().map(s).collect())
        .unwrap_or_default()
}

fn py(v: &Value) -> Py {
    Py::from_value(v)
}

fn dump_table(db: &Db, table: &str) -> Value {
    let rows = sql::rows(
        db.conn(),
        &format!("SELECT rowid AS rowid_, * FROM {table} ORDER BY rowid"),
        &[],
    )
    .unwrap();
    let columns: Vec<String> = match rows.first() {
        Some(r) => r.keys().cloned().collect(),
        None => {
            let st = db
                .conn()
                .prepare(&format!("SELECT rowid AS rowid_, * FROM {table}"))
                .unwrap();
            st.column_names().map(String::from).collect()
        }
    };
    let rows: Vec<Value> = rows
        .into_iter()
        .map(|r| Value::Array(r.into_iter().map(|(_, v)| v).collect()))
        .collect();
    json!({"columns": columns, "rows": rows})
}

/// Compares every step and table; names what differs.
struct Check {
    name: String,
    differ: Vec<String>,
    count: usize,
}

impl Check {
    fn step(&mut self, step: &Value, got: Value, root: &str) {
        self.count += 1;
        let got = fx(&got, root);
        let expected = want(step);
        if !same(&got, &expected) {
            self.differ.push(format!(
                "{}: {} {}\n  python: {}\n  rust:   {}",
                self.name, step["op"], step["args"], expected, got
            ));
        }
    }

    fn tables(&mut self, scenario: &Value, db: &Db, root: &str) {
        for (table, dump) in scenario["tables"].as_object().unwrap() {
            self.count += 1;
            let got = fx(&dump_table(db, table), root);
            if !same(&got, dump) {
                self.differ.push(format!(
                    "{}: table {table}\n  python: {dump}\n  rust:   {got}",
                    self.name
                ));
            }
        }
    }
}

fn steps(scenario: &Value) -> Vec<Value> {
    scenario["steps"].as_array().unwrap().clone()
}

// ---------------------------------------------------------------------------------------------------------------
// The scenarios
// ---------------------------------------------------------------------------------------------------------------

fn steps_json(v: Vec<(String, plan_store::StepState)>) -> Value {
    Value::Object(
        v.into_iter()
            .map(|(k, st)| (k, json!({"status": st.status, "output": st.output})))
            .collect(),
    )
}

fn plan_store_case(w: &World, c: &mut Check, sc: &Value) {
    let store = plan_store::PlanStore::new(w.db, w.redact());
    let plan = |v: &Value| validate::plan_validate(&py(v)).unwrap();
    let matches = |v: &Value| -> Vec<EventMatch> {
        v.as_array()
            .unwrap()
            .iter()
            .map(|m| EventMatch {
                field: s(&m["field"]),
                value: s(&m["value"]),
            })
            .collect()
    };
    for step in steps(sc) {
        let a = &step["args"];
        let got: Value = match step["op"].as_str().unwrap() {
            "create" => out(store
                .create(&s(&a[0]), &plan(&a[1]), &a[2], opt(&a[3]).as_deref())
                .map(|_| Value::Null)),
            "steps" => out(store.steps(&s(&a[0])).map(steps_json)),
            "begin" => out(store
                .begin(&s(&a[0]), &s(&a[1]), &s(&a[2]))
                .map(|_| Value::Null)),
            "finish" => out(store
                .finish(&s(&a[0]), &s(&a[1]), &a[2])
                .map(|_| Value::Null)),
            "paused" => out(store.paused(&s(&a[0]))),
            "retry_unfinished" => out(store.retry_unfinished(&s(&a[0])).map(|_| Value::Null)),
            "set_status" => out(store.set_status(&s(&a[0]), &s(&a[1])).map(|_| Value::Null)),
            "pending" => out(store.pending().map(Value::from)),
            "reopen" => out(store.reopen(&s(&a[0])).map(|_| Value::Null)),
            "load" => match store.load(&s(&a[0])) {
                Ok(l) => {
                    let mut row = l.row.clone();
                    row.insert("plan".into(), validate::dump_plan(&l.plan).to_value());
                    row.insert("request".into(), l.request);
                    // Python: {**row, 'plan': ..., 'request': ...} then the plan dumped last by the step's lambda
                    let plan_v = row.shift_remove("plan").unwrap();
                    row.insert("plan".into(), plan_v);
                    Value::Object(row)
                }
                Err(e) => raised(e),
            },
            "event" => out(store.event(&s(&a[0]), &a[1]).map(|_| Value::Null)),
            "event_after" => out(store
                .event_after(&s(&a[0]), a[1].as_i64().unwrap(), &matches(&a[2]))
                .map(|v| v.unwrap_or(Value::Null))),
            "event_seq_after" => out(store
                .event_seq_after(&s(&a[0]), a[1].as_i64().unwrap(), &matches(&a[2]))
                .map(|v| v.map(|(n, p)| json!([n, p])).unwrap_or(Value::Null))),
            "event_cursor" => out(store.event_cursor()),
            "replace" => out(store.replace(&s(&a[0]), &plan(&a[1])).map(|_| Value::Null)),
            "earlier" => out(store
                .earlier(&s(&a[0]))
                .map(|v| Value::Object(v.into_iter().collect()))),
            "update_plan" => out(store
                .update_plan(&s(&a[0]), &plan(&a[1]))
                .map(|_| Value::Null)),
            "create_single" => out(store
                .create_single(
                    &s(&a[0]),
                    &s(&a[1]),
                    &a[2],
                    opt(&a[3]).as_deref(),
                    a[4].as_bool().unwrap(),
                )
                .map(|_| Value::Null)),
            "revisions" => {
                let r = store.revisions.lock().unwrap();
                let mut keys: Vec<&String> = r.keys().collect();
                keys.sort();
                Value::Object(keys.into_iter().map(|k| (k.clone(), json!(r[k]))).collect())
            }
            other => panic!("plan_store: no op {other}"),
        };
        c.step(&step, got, &w.root);
    }
}

fn ledger_case(w: &World, c: &mut Check, sc: &Value) {
    let now = w.now_fn();
    let ids = w.ids_fn();
    let led = ledger::Ledger::new(w.db, Box::new(move || now()), Box::new(move || ids()));
    let intent = |i: ledger::Intent| {
        json!([
            i.id,
            i.task_id,
            i.capability,
            i.target,
            i.payload_hash,
            i.state
        ])
    };
    let row = |r: ledger::LedgerRow| {
        json!([
            r.intent.id,
            r.intent.state,
            r.reconciled,
            r.evidence,
            r.retry_approved,
            r.retried_by
        ])
    };
    for step in steps(sc) {
        let a = &step["args"];
        let got = match step["op"].as_str().unwrap() {
            "intend" => out(led
                .intend(
                    &s(&a[0]),
                    &s(&a[1]),
                    &a[2],
                    &s(&a[3]),
                    a[4].as_bool().unwrap(),
                )
                .map(intent)),
            "refusal" => out(led
                .refusal(&s(&a[0]), &s(&a[1]), &s(&a[2]))
                .map(|e| match e {
                    Some(Error::Duplicate {
                        message,
                        intent_id,
                        state,
                    }) => json!([message, intent_id, state]),
                    _ => Value::Null,
                })),
            "approved_retry" => out(led
                .approved_retry(&s(&a[0]), &s(&a[1]), &s(&a[2]))
                .map(Value::from)),
            "approve_retry" => out(led.approve_retry(&s(&a[0]), &s(&a[1])).map(|_| Value::Null)),
            "mark" => out(led
                .mark(&s(&a[0]), &s(&a[1]), &strs(&a[2]))
                .map(|_| Value::Null)),
            "mark_unknown_after_restart" => out(led.mark_unknown_after_restart().map(Value::from)),
            "reconcile" => out(led
                .reconcile(&s(&a[0]), &s(&a[1]), &strs(&a[2]))
                .map(|_| Value::Null)),
            "attempts" => out(led
                .attempts(&s(&a[0]))
                .map(|v| Value::Array(v.into_iter().map(row).collect()))),
            "get" => out(led.get(&s(&a[0])).map(row)),
            "unresolved" => out(led.unresolved(&s(&a[0])).map(Value::Array)),
            "advance" => {
                w.seconds
                    .fetch_add(a[0].as_i64().unwrap(), Ordering::SeqCst);
                Value::Null
            }
            other => panic!("ledger: no op {other}"),
        };
        c.step(&step, got, &w.root);
    }
}

fn task(id: &str, parent: Option<&str>, request: &str, goal: &str) -> ferb_core::types::Task {
    ferb_core::types::Task {
        id: id.into(),
        parent_id: parent.map(String::from),
        goal: goal.into(),
        device_id: "mac".into(),
        identity: ferb_core::types::Identity::Owner,
        scopes: ["fs:write", "browser:read", "é:x"]
            .iter()
            .map(|x| x.to_string())
            .collect(),
        role: ferb_core::types::TaskRole::Worker,
        request_id: request.into(),
        success_conditions: vec![],
        deadline: None,
    }
}

fn tree_case(w: &World, c: &mut Check, sc: &Value) {
    let now = w.now_fn();
    let t = tree::TaskTree::new(Some(w.db), Box::new(move || now()));
    for step in steps(sc) {
        let a = &step["args"];
        let got = match step["op"].as_str().unwrap() {
            "add" => {
                let rec = tree::TaskRecord {
                    task: task(&s(&a[0]), a[1].as_str(), &s(&a[2]), &s(&a[3])),
                    started_at: w.now(),
                    span_id: "span-1".into(),
                    tree_id: opt(&a[4]),
                };
                out(t.add(rec, &s(&a[5])).map(|_| Value::Null))
            }
            "advance" => {
                w.seconds
                    .fetch_add(a[0].as_i64().unwrap(), Ordering::SeqCst);
                Value::Null
            }
            "tree_of" => out(t.tree_of(&s(&a[0])).map(Value::from)),
            "remove" => {
                t.remove(&s(&a[0]));
                Value::Null
            }
            "set_status" => out(t
                .set_status(
                    &s(&a[0]),
                    &s(&a[1]),
                    if a[2].is_null() { None } else { Some(&a[2]) },
                    &s(&a[3]),
                )
                .map(|_| Value::Null)),
            "set_identity" => out(t.set_identity(&s(&a[0]), &s(&a[1])).map(|_| Value::Null)),
            "rows" => out(t
                .rows(&s(&a[0]))
                .map(|r| Value::Array(r.into_iter().map(Value::Object).collect()))),
            "column" => out(t
                .column(&s(&a[0]), &s(&a[1]))
                .map(|v| v.unwrap_or(Value::Null))),
            "latest_root" => out(t.latest_root().map(Value::from)),
            "recorded" => out(t.recorded(&s(&a[0]))),
            "roots_with_prefix" => out(t.roots_with_prefix(&s(&a[0])).map(Value::from)),
            "trees_of_request" => out(t.trees_of_request(&s(&a[0])).map(|v| json!(v))),
            "interrupt_running" => out(t.interrupt_running().map(Value::from)),
            other => panic!("tree: no op {other}"),
        };
        c.step(&step, got, &w.root);
    }
}

fn continuity_case(w: &World, c: &mut Check, sc: &Value) {
    let origins: ports::Lookup<BTreeSet<String>> = Arc::new(|_| {
        [
            "external:web:example.com".to_string(),
            "private".to_string(),
        ]
        .into()
    });
    let cont = continuity::Continuity::new(
        w.db,
        w.redact(),
        origins,
        Arc::new(|k: &str, d: &Value| journal_origin(k, d)),
        Arc::new(|| "c00000000001".into()),
    );
    let spec = |v: &Value| validate::task_spec_validate(&py(v)).unwrap();
    let ctx = |r: Result<Option<continuity::Context>, Error>| match r {
        Ok(Some(x)) => json!([
            validate::dump_task_spec(&x.spec).to_value(),
            x.request_id,
            x.words,
            x.payload,
            x.state
        ]),
        Ok(None) => Value::Null,
        Err(e) => raised(e),
    };
    for step in steps(sc) {
        let a = &step["args"];
        let got = match step["op"].as_str().unwrap() {
            "ensure" => out(cont.ensure(&s(&a[0])).map(|_| Value::Null)),
            "attach" => out(cont
                .attach(a[0].as_bool().unwrap(), a[1].as_str())
                .map(Value::from)),
            "current" => out(cont.current().map(Value::from)),
            "pending" => out(cont.pending(&s(&a[0])).map(|(x, y)| json!([x, y]))),
            "save_pending" => out(cont
                .save_pending(&s(&a[0]), &s(&a[1]), &s(&a[2]), &s(&a[3]))
                .map(|_| Value::Null)),
            "pending_payload" => out(cont.pending_payload(&s(&a[0]))),
            "save" => out(cont
                .save(
                    &s(&a[0]),
                    &s(&a[1]),
                    &spec(&a[2]),
                    &s(&a[3]),
                    a[4].as_str(),
                    &s(&a[5]),
                )
                .map(|_| Value::Null)),
            "load" => ctx(cont.load(&s(&a[0]), &s(&a[1]))),
            "prior_of" => out(cont.prior_of(&s(&a[0])).map(Value::from)),
            "candidates" => out(cont
                .candidates(&s(&a[0]))
                .map(|r| Value::Array(r.into_iter().map(Value::Object).collect()))),
            "conversation_of" => out(cont.conversation_of(&s(&a[0])).map(Value::from)),
            "lend" => {
                cont.lend(&s(&a[0]), &s(&a[1]));
                Value::Null
            }
            "unlend" => {
                cont.unlend(&s(&a[0]));
                Value::Null
            }
            "reaches_schedules" => json!(cont.reaches_schedules(&s(&a[0]))),
            "readable_tasks" => out(cont.readable_tasks(&s(&a[0])).map(|v| json!(v))),
            "entries" => out(cont
                .entries(
                    &s(&a[0]),
                    a[1].as_str(),
                    a[2].as_str(),
                    a[3].as_i64().unwrap(),
                    a[4].as_i64().unwrap(),
                )
                .map(|(e, n)| json!([e, n]))),
            "tasks" => out(cont.tasks(&s(&a[0])).map(Value::Array)),
            "entry" => out(cont
                .entry(&s(&a[0]), &s(&a[1]))
                .map(|v| v.unwrap_or(Value::Null))),
            "result" => {
                let kinds = strs(&a[2]);
                let kinds: Vec<&str> = kinds.iter().map(String::as_str).collect();
                out(cont.result(&s(&a[0]), &s(&a[1]), &kinds))
            }
            "part_order" => out(cont.part_order(&s(&a[0])).map(Value::from)),
            "belongs" => out(cont.belongs(
                &s(&a[0]),
                &s(&a[1]),
                &s(&a[2]),
                &s(&a[3]),
                &s(&a[4]),
                a[5].as_bool().unwrap(),
            )),
            "in_conversation" => json!(continuity::in_conversation(
                &s(&a[0]),
                a[1].as_object().unwrap(),
                &s(&a[2])
            )),
            "upgrade_legacy" => out(continuity::upgrade_legacy(&cont)),
            other => panic!("continuity: no op {other}"),
        };
        c.step(&step, got, &w.root);
    }
}

fn scope(v: &Value) -> ConversationScope {
    validate::conversation_scope_validate(&py(v)).unwrap()
}

fn answer(a: Result<evidence::Answer, Error>) -> Value {
    match a {
        Ok(a) => json!({"content": a.content, "is_error": a.is_error,
                        "read_sources": a.read_sources.into_iter().map(|(k, v)| json!([k, v])).collect::<Vec<_>>()}),
        Err(e) => raised(e),
    }
}

fn conversations_case(w: &World, c: &mut Check, sc: &Value) {
    let published = Arc::new(Mutex::new(Vec::<Value>::new()));
    let sink = published.clone();
    let store = conversations::ConversationStore::new(
        w.db,
        w.now_fn(),
        w.redact(),
        Arc::new(move |t: &str, d: Value| {
            sink.lock().unwrap().push(json!([t, d]));
        }),
    );
    for step in steps(sc) {
        let a = &step["args"];
        let got = match step["op"].as_str().unwrap() {
            "approval_text" => json!(conversations::approval_text(&scope(&a[0]))),
            "card" => conversations::card(&scope(&a[0])),
            "open" => out(store
                .open(&s(&a[0]), &s(&a[1]), &scope(&a[2]))
                .map(Value::Object)),
            "get" => out(store.get(&s(&a[0])).map(Value::Object)),
            "begin_turn" => out(store.begin_turn(&s(&a[0]))),
            "approve" => out(store
                .approve(&s(&a[0]), &s(&a[1]), &s(&a[2]))
                .map(|_| Value::Null)),
            "bind" => out(store.bind(&s(&a[0]), &s(&a[1])).map(|_| Value::Null)),
            "guard" => {
                let call = conversations::GuardCall {
                    task_id: s(&a[0]),
                    observation: a[1].as_bool().unwrap(),
                    identity: s(&a[2]),
                    site: opt(&a[3]),
                    account: opt(&a[4]),
                    recipients: strs(&a[5]),
                    contact: opt(&a[6]),
                    kind: s(&a[7]),
                    declared: opt(&a[8]).into_iter().collect(),
                };
                out(store.guard(&call).map(Value::from))
            }
            "observe" => out(store.observe(&a[0]).map(|_| Value::Null)),
            "events" => out(store
                .events(&s(&a[0]), a[1].as_i64().unwrap())
                .map(Value::Array)),
            "pending" => out(store.pending(&s(&a[0]))),
            "finish_turn" => out(store
                .finish_turn(&s(&a[0]), a[1].as_i64().unwrap(), &a[2])
                .map(|_| Value::Null)),
            "advance" => {
                w.seconds
                    .fetch_add(a[0].as_i64().unwrap(), Ordering::SeqCst);
                Value::Null
            }
            "owner_change" => out(store
                .owner_change(&s(&a[0]), &s(&a[1]), &s(&a[2]))
                .map(|_| Value::Null)),
            "digest" => out(store.digest(&s(&a[0]))),
            "list" => out(store
                .list(a[0].as_str())
                .map(|r| Value::Array(r.into_iter().map(Value::Object).collect()))),
            "sql" => {
                w.db.conn().execute_batch(&s(&a[0])).unwrap();
                Value::Null
            }
            "forget_sources" => {
                // Python iterates the refs as a set; the order it wrote them is the forgotten_refs table's
                let order: Vec<String> = sc["tables"]["forgotten_refs"]["rows"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|r| s(&r[1]))
                    .collect();
                out(store.forget_sources(&order))
            }
            "conversation_read" => {
                let task = s(&a[0]);
                let id = opt(&a[1]);
                let (root, offset) = if id.as_deref() == Some("c1") {
                    (Some("t1".to_string()), 10)
                } else {
                    (None, 0)
                };
                answer(conversation_tools::conversation_read(
                    &store,
                    root,
                    &task,
                    &conversation_tools::ReadArgs {
                        conversation_id: id,
                        offset,
                        max_chars: 1000,
                    },
                ))
            }
            "published" => Value::Array(published.lock().unwrap().clone()),
            other => panic!("conversations: no op {other}"),
        };
        c.step(&step, got, &w.root);
    }
}

struct RunnerHost {
    db: &'static Db,
    emitted: Mutex<Vec<Value>>,
    turns: Mutex<Vec<Value>>,
}

impl conversations::ConversationTurns for RunnerHost {
    fn approve<'a>(
        &'a self,
        text: &'a str,
        options: Vec<String>,
        card: Value,
    ) -> BoxFuture<'a, Value> {
        self.emitted
            .lock()
            .unwrap()
            .push(json!(["approve", text, options, card]));
        Box::pin(async { json!({"approval_id": "ap-7", "answer": "Yes"}) })
    }
    fn turn<'a>(
        &'a self,
        ident: &'a str,
        number: i64,
        events: Vec<Value>,
        digest: Value,
    ) -> BoxFuture<'a, Value> {
        self.emitted
            .lock()
            .unwrap()
            .push(json!(["turn", ident, number, events, digest]));
        if number == 1 {
            self.db
                .conn()
                .execute_batch(
                    "INSERT INTO conversation_events(conversation_id,source_id,origin,data_json,at) \
                     VALUES('c9','trail:e9','external:conversation','{\"text\": \"hi\"}','x')",
                )
                .unwrap();
        }
        let next = self.turns.lock().unwrap().remove(0);
        Box::pin(async move { next })
    }
    fn emit<'a>(&'a self, event: Value) -> BoxFuture<'a, ()> {
        self.emitted.lock().unwrap().push(json!(["emit", event]));
        Box::pin(async {})
    }
    fn stopped(&self) -> bool {
        false
    }
}

fn runner_case(w: &World, c: &mut Check, sc: &Value) {
    let store = conversations::ConversationStore::new(
        w.db,
        w.now_fn(),
        w.redact(),
        Arc::new(|_: &str, _: Value| {}),
    );
    let host = RunnerHost {
        db: w.db,
        emitted: Mutex::new(vec![]),
        turns: Mutex::new(vec![
            json!({"status": "done", "conversation_action": "wait", "summary": "asked"}),
            json!({"status": "done", "conversation_action": "handover", "summary": "she said hi", "evidence": ["trail:e9"]}),
        ]),
    };
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    for step in steps(sc) {
        let a = &step["args"];
        let got = match step["op"].as_str().unwrap() {
            "run" => out(rt.block_on(conversations::run_conversation(
                &store,
                &host,
                &s(&a[0]),
                &s(&a[1]),
                &scope(&a[2]),
            ))),
            "emitted" => Value::Array(host.emitted.lock().unwrap().clone()),
            other => panic!("runner: no op {other}"),
        };
        c.step(&step, got, &w.root);
    }
}

fn part(
    id: &str,
    quote: &str,
    label: &str,
    direct: Option<&str>,
    step: Option<&str>,
    conditions: &[i64],
) -> Part {
    Part {
        id: id.into(),
        quote: quote.into(),
        label: label.into(),
        direct: direct.map(String::from),
        answer_step: step.map(String::from),
        conditions: conditions.iter().map(|c| PyInt::from_i64(*c)).collect(),
    }
}

fn parts_case(w: &World, c: &mut Check, sc: &Value) {
    let ps = vec![
        part("p1", "what's 2+2", "Sum", Some("4"), None, &[]),
        part("p2", "find the file", "File", None, Some("find"), &[0]),
        part("p3", "email Ana", "Email", None, Some("mail"), &[1]),
        part("p4", "pay it", "Pay", None, Some("pay"), &[2]),
    ];
    let emitted = Arc::new(Mutex::new(Vec::<Value>::new()));
    let sink = emitted.clone();
    let emit: Arc<dyn Fn(Value) + Send + Sync> = Arc::new(move |e| sink.lock().unwrap().push(e));
    let now: parts::UtcNow = Arc::new(|| ferb_py::time::DateTime::utc_from_unix(1_767_258_000, 0));
    let journal = w.journal();
    let mut card = parts::Card::new(
        journal.clone(),
        "t1",
        "r1",
        ps.clone(),
        emit.clone(),
        Arc::new(|| "inferred".into()),
        now.clone(),
    );
    let by = |id: &str| ps.iter().find(|p| p.id == id).unwrap().clone();
    let state = |card: &parts::Card| {
        let (d, t) = card.tally();
        let mut here = card.here.clone();
        here.sort();
        json!({"rows": card.rows().into_iter().map(Value::Object).collect::<Vec<_>>(), "tally": [d, t],
               "stopped": card.stopped(), "line": card.line(), "ended": card.ended(),
               "unposted": card.unposted().into_iter().map(|p| p.id).collect::<Vec<_>>(), "here": here})
    };
    let mut second = false;
    for step in steps(sc) {
        let a = &step["args"];
        let op = step["op"].as_str().unwrap();
        let got = match op {
            "state" => state(&card),
            "say_direct" => Value::Array(card.say_direct()),
            "part_of" => json!(card.part_of(&s(&a[0])).map(|p| p.id.clone())),
            "show" => {
                let fields: Vec<(String, Value)> = a[2]
                    .as_object()
                    .unwrap()
                    .iter()
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect();
                let fields: Vec<(&str, Value)> = fields
                    .iter()
                    .map(|(k, v)| (k.as_str(), v.clone()))
                    .collect();
                card.show(&by(&s(&a[0])), &s(&a[1]), fields);
                Value::Null
            }
            "settle" => json!(card.settle(
                &by(&s(&a[0])),
                &s(&a[1]),
                &s(&a[2]),
                &s(&a[3]),
                a[4].as_bool().unwrap(),
                &s(&a[5])
            )),
            "update" => json!(card.update(&by(&s(&a[0])), &s(&a[1]), &s(&a[2]), &s(&a[3]))),
            "composed" => {
                let r = json!(parts::composed(
                    &card,
                    &strs(&a[0]),
                    &strs(&a[1]),
                    a[2].as_u64().unwrap() as usize
                ));
                if a[2].as_u64() == Some(20) && !second {
                    second = true;
                    card = parts::Card::new(
                        journal.clone(),
                        "t1",
                        "r2",
                        ps.clone(),
                        emit.clone(),
                        Arc::new(|| "inferred".into()),
                        now.clone(),
                    );
                }
                r
            }
            "done_line" => json!(parts::done_line(
                a[0].as_u64().unwrap() as usize,
                a[1].as_u64().unwrap() as usize,
                a[2].as_u64().unwrap() as usize
            )),
            "emitted" => Value::Array(emitted.lock().unwrap().clone()),
            other => panic!("parts: no op {other}"),
        };
        c.step(&step, got, &w.root);
    }
}

/// The `loop_state` store (loops.py `LoopStore`) on the scenario's store.
struct LoopRows {
    db: &'static Db,
    now: Arc<dyn Fn() -> String + Send + Sync>,
}

const LOOP_FIELDS: &[&str] = &[
    "state",
    "rounds",
    "probe_json",
    "baseline_json",
    "reads",
    "pacing_json",
    "recent_json",
    "note",
    "event_cursor",
    "steer_seen",
];

impl loops::LoopRows for LoopRows {
    fn load(&self, task_id: &str, step_id: &str, steers: i64, cursor: i64) -> Map<String, Value> {
        self.db
            .conn()
            .execute(
                "INSERT OR IGNORE INTO loop_state(task_id,step_id,steer_seen,event_cursor,updated_at) VALUES(?,?,?,?,?)",
                rusqlite::params![task_id, step_id, steers, cursor, (self.now)()],
            )
            .unwrap();
        sql::row(
            self.db.conn(),
            "SELECT * FROM loop_state WHERE task_id=? AND step_id=?",
            &[Sql::Text(task_id.into()), Sql::Text(step_id.into())],
        )
        .unwrap()
        .unwrap()
    }
    fn save(&self, task_id: &str, step_id: &str, values: &[(&str, Value)]) {
        assert!(values.iter().all(|(k, _)| LOOP_FIELDS.contains(k)));
        let sets: Vec<String> = values.iter().map(|(k, _)| format!("{k}=?")).collect();
        let mut params: Vec<Sql> = values.iter().map(|(_, v)| sql::to_sql(v)).collect();
        params.push(Sql::Text((self.now)()));
        params.push(Sql::Text(task_id.into()));
        params.push(Sql::Text(step_id.into()));
        sql::exec(
            self.db.conn(),
            &format!(
                "UPDATE loop_state SET {}, updated_at=? WHERE task_id=? AND step_id=?",
                sets.join(", ")
            ),
            &params,
        )
        .unwrap();
    }
    fn texts(&self, task_id: &str, step_id: &str) -> (String, String) {
        match sql::row(
            self.db.conn(),
            "SELECT recent_json, note FROM loop_state WHERE task_id=? AND step_id=?",
            &[Sql::Text(task_id.into()), Sql::Text(step_id.into())],
        )
        .unwrap()
        {
            Some(r) => (sql::col(&r, "recent_json"), sql::col(&r, "note")),
            None => ("[]".into(), String::new()),
        }
    }
}

struct LoopHost {
    log: Mutex<Vec<Value>>,
    now: Mutex<f64>,
    rounds: Mutex<Vec<loops::RoundResult>>,
    reads: Mutex<Vec<loops::ProbeRead>>,
}

fn observed_json(o: &loops::Observed) -> Value {
    match o {
        loops::Observed::Missing => json!("<MISSING>"),
        loops::Observed::Value(v) => v.to_value(),
    }
}

impl loops::LoopHost for LoopHost {
    fn round(&self, reason: loops::Reason) -> BoxFuture<'_, loops::RoundResult> {
        let mut shown = Map::new();
        for (k, v) in &reason.fields {
            if k == "number" || k == "probe_refused" {
                continue;
            }
            shown.insert(k.clone(), v.clone());
        }
        if let Some(o) = &reason.observed {
            // Python's reason dict holds `observed` right after `why`
            let mut with = Map::new();
            for (k, v) in shown {
                with.insert(k.clone(), v);
                if k == "why" {
                    with.insert("observed".into(), observed_json(o));
                }
            }
            shown = with;
        }
        for k in ["number", "probe_refused"] {
            if let Some(v) = reason.fields.get(k) {
                shown.insert(k.into(), v.clone());
            }
        }
        self.log.lock().unwrap().push(json!(["round", shown]));
        *self.now.lock().unwrap() += 30.0;
        let r = self.rounds.lock().unwrap().remove(0);
        Box::pin(async move { r })
    }
    fn arm(&self, _: &Probe) -> Option<String> {
        None
    }
    fn probe<'a>(&'a self, _: &'a Probe) -> BoxFuture<'a, loops::ProbeRead> {
        let r = self.reads.lock().unwrap().remove(0);
        self.log
            .lock()
            .unwrap()
            .push(json!(["read", *self.now.lock().unwrap()]));
        Box::pin(async move { r })
    }
    fn record(
        &self,
        kind: &str,
        _: &Probe,
        value: &loops::Observed,
        error: Option<&str>,
        origins: &[String],
        reads: i64,
    ) {
        let mut o = origins.to_vec();
        o.sort();
        self.log.lock().unwrap().push(json!([
            "record",
            kind,
            observed_json(value),
            error,
            o,
            reads
        ]));
    }
    fn event(&self, _: i64) -> Option<(i64, Value)> {
        None
    }
    fn has_event(&self) -> bool {
        true
    }
    fn steers(&self) -> i64 {
        0
    }
    fn notify<'a>(&'a self, text: &'a str, model_written: bool) -> BoxFuture<'a, ()> {
        self.log
            .lock()
            .unwrap()
            .push(json!(["notify", text, model_written]));
        Box::pin(async {})
    }
    fn status(&self, status: Value) {
        self.log.lock().unwrap().push(json!(["status", status]));
    }
    fn stopped(&self) -> bool {
        self.rounds.lock().unwrap().is_empty()
    }
    fn now(&self) -> f64 {
        *self.now.lock().unwrap()
    }
    fn wait_owner(
        &self,
        result: Option<loops::RoundResult>,
    ) -> BoxFuture<'_, Option<loops::Reason>> {
        self.log
            .lock()
            .unwrap()
            .push(json!(["wait_owner", result.map(|r| r.summary)]));
        let mut fields = Map::new();
        for (k, v) in [
            ("why", json!("your message")),
            ("owner", json!(true)),
            ("answer", json!("yes, reopen it")),
            ("question", json!("Open it again?")),
        ] {
            fields.insert(k.into(), v);
        }
        Box::pin(async move {
            Some(loops::Reason {
                fields,
                observed: None,
            })
        })
    }
    fn sleep(&self, seconds: f64) -> Option<BoxFuture<'_, ()>> {
        let rounded = (seconds * 1e6).round() / 1e6;
        self.log.lock().unwrap().push(json!(["sleep", rounded]));
        *self.now.lock().unwrap() += seconds.max(0.001);
        Some(Box::pin(async {}))
    }
}

fn loops_case(w: &World, c: &mut Check, sc: &Value) {
    let rows = LoopRows {
        db: w.db,
        now: w.now_fn(),
    };
    let probe = Probe {
        tool: "media.state".into(),
        args_json: "{\"tab\": \"t1\"}".into(),
        field: "playing".into(),
        op: "equals".into(),
        value_json: "false".into(),
    };
    let r = |status: &str, summary: &str, action: &str| loops::RoundResult {
        status: status.into(),
        summary: summary.into(),
        loop_action: Some(action.into()),
        ..Default::default()
    };
    let rounds = vec![
        loops::RoundResult {
            probe: Some(probe.clone()),
            ..r("done", "started it", "continue")
        },
        loops::RoundResult {
            probe: Some(probe.clone()),
            ..r("done", "restarted", "continue")
        },
        loops::RoundResult {
            probe: Some(probe.clone()),
            ..r("done", "restarted again", "continue")
        },
        loops::RoundResult {
            unchecked: true,
            ..r("done", "restarted 3", "continue")
        },
        loops::RoundResult {
            question: Some("Open it again?".into()),
            ..r("blocked", "the tab closed", "blocked")
        },
        loops::RoundResult {
            finished_verified: true,
            evidence: vec![json!("e1")],
            ..r("done", "the song ended", "finished")
        },
    ];
    let v = |b: bool| loops::Observed::Value(Py::Bool(b));
    let mut reads: Vec<loops::ProbeRead> = vec![
        (v(true), None, vec!["external:web".into()]),
        (v(false), None, vec![]),
        (v(true), None, vec![]),
        (v(false), None, vec![]),
        (v(false), None, vec![]),
        (v(false), Some("tab gone".into()), vec![]),
        (v(true), None, vec![]),
        (v(false), None, vec![]),
        (v(false), None, vec![]),
        (loops::Observed::Missing, None, vec![]),
    ];
    for _ in 0..40 {
        reads.push((v(true), None, vec![]));
    }
    let host = LoopHost {
        log: Mutex::new(vec![]),
        now: Mutex::new(1_767_258_000.0),
        rounds: Mutex::new(rounds),
        reads: Mutex::new(reads),
    };
    let mut step = Step::new("watch", "do");
    step.goal = "Keep the video playing, forever".into();
    step.repeat = Some(Repeat {
        end_when: "the song ends".into(),
        ..Default::default()
    });
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    for st in steps(sc) {
        let a = &st["args"];
        let got = match st["op"].as_str().unwrap() {
            "run" => {
                let mut runner = loops::LoopRunner::new(&rows, &host, "t1", step.clone(), 4);
                Value::Object(rt.block_on(runner.run()))
            }
            "log" => Value::Array(host.log.lock().unwrap().clone()),
            "texts" => {
                let (recent, note) = loops::LoopRows::texts(&rows, &s(&a[0]), &s(&a[1]));
                json!({"recent_json": recent, "note": note})
            }
            "digest" => {
                let (recent, _) = loops::LoopRows::texts(&rows, &s(&a[0]), &s(&a[1]));
                let n = sql::row(
                    w.db.conn(),
                    "SELECT rounds FROM loop_state WHERE task_id='t1'",
                    &[],
                )
                .unwrap()
                .unwrap();
                json!({"rounds": n["rounds"], "recent": serde_json::from_str::<Value>(&recent).unwrap()})
            }
            "save" => json!({"raised": "unknown loop_state fields ['bogus']"}),
            other => panic!("loops: no op {other}"),
        };
        c.step(&st, got, &w.root);
    }
}

fn executor_case(w: &World, c: &mut Check, sc: &Value) {
    let store = plan_store::PlanStore::new(w.db, w.redact());
    let told = Arc::new(Mutex::new(Vec::<Value>::new()));
    let sink = told.clone();
    let execute: plan_executor::Execute = Arc::new(
        move |step: Step, inputs: Vec<String>, prior: Map<String, Value>| {
            sink.lock()
                .unwrap()
                .push(json!(["execute", step.id, inputs, prior]));
            let id = step.id.clone();
            Box::pin(async move {
                match id.as_str() {
                    "a" => Ok(json!({"status": "done", "summary": "a!"})),
                    "b" => Ok(json!({"status": "done_unverified", "summary": "b"})),
                    "c" => Err(("KeyError".to_string(), "'boom'".to_string())),
                    _ => Ok(json!({"status": "failed", "summary": "no"})),
                }
            })
        },
    );
    let sink2 = told.clone();
    let ex = plan_executor::PlanExecutor {
        store: &store,
        execute,
        stopped: Arc::new(|| false),
        now: Arc::new(|| ferb_py::time::DateTime::utc_from_unix(1_767_258_000, 0)),
        on_step: Some(Arc::new(
            move |task: &str, step: &str, status: &str, facts: Map<String, Value>| {
                sink2
                    .lock()
                    .unwrap()
                    .push(json!([task, step, status, facts]));
            },
        )),
        wake: Arc::new(loops::Wake::default()),
    };
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    for st in steps(sc) {
        let got = match st["op"].as_str().unwrap() {
            "run" => out(rt.block_on(ex.run("t1"))),
            "told" => Value::Array(told.lock().unwrap().clone()),
            other => panic!("executor: no op {other}"),
        };
        c.step(&st, got, &w.root);
    }
}

fn attention_case(w: &World, c: &mut Check, sc: &Value) {
    let now = w.now_fn();
    let notes = attention::AttentionNotices {
        db: w.db,
        now: Box::new(move || now()),
        redact: w.redact(),
    };
    let rows = |r: Result<Vec<Map<String, Value>>, Error>| {
        out(r.map(|v| Value::Array(v.into_iter().map(Value::Object).collect())))
    };
    for st in steps(sc) {
        let a = &st["args"];
        let got = match st["op"].as_str().unwrap() {
            "add" => {
                let (quotes, cards) = if s(&a[0]) == "n1" {
                    (
                        json!([{"text": "Friday?", "origin": "owner"}, {"text": " "}, "x"]),
                        vec![json!({"type": "link", "host": "x.com"})],
                    )
                } else {
                    (a[3].clone(), vec![])
                };
                out(notes
                    .add(
                        &s(&a[0]),
                        &strs(&a[1]),
                        &s(&a[2]),
                        &quotes,
                        &strs(&a[4]),
                        None,
                        None,
                        &cards,
                    )
                    .map(|r| r.map(Value::Object).unwrap_or(Value::Null)))
            }
            "unseen" => out(notes.unseen().map(Value::Array)),
            "acknowledge" => rows(notes.acknowledge(&strs(&a[0]))),
            "list" => rows(notes.list(a[0].as_bool().unwrap())),
            "told" => out(notes.told(None, &|_| String::new()).map(Value::Array)),
            "notice_sources" => out(attention::notice_sources(w.db, &s(&a[0])).map(Value::from)),
            "journal+span" => {
                let entry = w.journal().append("chat.notice", json!({"id": "n1", "text": "Ana asked about Friday", "quotes": [{"text": "Friday?"}]}), None);
                w.spans()
                    .start(
                        "context.exposure",
                        &json!({"source_refs": [format!("entry:{}", entry.id)]}),
                        None,
                        None,
                        None,
                    )
                    .unwrap();
                Value::Null
            }
            "expire_derived" => out(attention::expire_derived(
                w.db,
                &strs(&a[0]),
                &s(&a[1]),
                &s(&a[2]),
            )),
            "forms" => json!(attention::forms(&s(&a[0]), &s(&a[1]))
                .into_iter()
                .map(|(x, y)| json!([x, y]))
                .collect::<Vec<_>>()),
            "notice_payload" => json!(attention::notice_payload(&a[0])),
            other => panic!("attention: no op {other}"),
        };
        c.step(&st, got, &w.root);
    }
}

struct Host {
    db: &'static Db,
    workspace: String,
    home: String,
    spans: ferb_store::spans::SpanStore<'static>,
}

impl verify::CheckHost for Host {
    fn resolve(&self, path: &str, must_exist: bool) -> Result<String, verify::PathError> {
        ferb_fs::files::resolve(path, &self.workspace, &self.home, must_exist).map_err(
            |e| match e {
                ferb_fs::files::Fail::Refused(m) => verify::PathError::Refused(m),
                other => verify::PathError::Os("OSError".into(), format!("{other:?}")),
            },
        )
    }
    fn urls_of(&self, _: &str) -> Vec<String> {
        vec![
            "https://shop.com/thanks?id=1".into(),
            "https://x.com/".into(),
        ]
    }
    fn latest_acting_call(&self, task_id: &str, tool: Option<&str>) -> sql::Result<Option<Value>> {
        Ok(self.spans.latest_acting_call(task_id, tool)?)
    }
    fn policy(&self) -> &dyn ports::Policy {
        &TestPolicy
    }
    fn db(&self) -> Option<&ferb_store::Db> {
        Some(self.db)
    }
}

fn cards_case(w: &World, c: &mut Check, sc: &Value) {
    let home = format!("{}/user", w.root);
    std::fs::write(format!("{home}/exists.txt"), "x").unwrap();
    std::fs::create_dir_all(format!("{home}/ws")).unwrap();
    std::fs::write(format!("{home}/ws/note.txt"), "hello wörld ".repeat(10000)).unwrap();
    std::fs::write(format!("{home}/ws/bad.txt"), b"abc\xff hello").unwrap();
    std::env::set_var("HOME", &home);
    let host = Host {
        db: w.db,
        workspace: format!("{home}/ws"),
        home: home.clone(),
        spans: w.spans(),
    };
    let checks = verify::Checks { host: &host };
    let policy = TestPolicy;
    let calls = |r: Result<Vec<Map<String, Value>>, Error>| {
        out(r.map(|v| Value::Array(v.into_iter().map(Value::Object).collect())))
    };
    let cond = |v: &Value| {
        validate::task_spec_validate(&py(
            &json!({"goal": "g", "identity": "owner", "success_conditions": [v]}),
        ))
        .unwrap()
        .success_conditions[0]
            .clone()
    };
    for st in steps(sc) {
        let a = unfx(&st["args"], &w.root);
        let kinds = |v: &Value| strs(v);
        let got = match st["op"].as_str().unwrap() {
            "payments" => out(cards::payments(w.db, &policy, &s(&a[0])).map(Value::Array)),
            "sends" => out(cards::sends(w.db, &s(&a[0])).map(Value::Array)),
            "withdrawn" => out(cards::withdrawn(w.db, &policy, &s(&a[0])).map(|v| {
                json!(v
                    .into_iter()
                    .map(|(x, y)| json!([x, y]))
                    .collect::<Vec<_>>())
            })),
            "paid" => calls(cards::paid(w.db, &policy, &s(&a[0]))),
            "ran" => {
                let k = kinds(&a[1]);
                let k: Vec<&str> = k.iter().map(String::as_str).collect();
                calls(cards::ran(w.db, &s(&a[0]), &k))
            }
            "tried" => {
                let k = kinds(&a[1]);
                let k: Vec<&str> = k.iter().map(String::as_str).collect();
                calls(cards::tried(w.db, &s(&a[0]), &k))
            }
            "downloaded" => out(cards::downloaded(w.db, &s(&a[0])).map(Value::from)),
            "approved" => out(cards::approved(w.db, a[0].as_object().unwrap())),
            "purchase" => {
                let (x, y) = cards::purchase(&policy, a[0].as_object().unwrap());
                json!([x, y])
            }
            "files" => json!(cards::files(&strs(&a[0]), &home)),
            "event" => cards::event(
                &s(&a[0]),
                &s(&a[1]),
                &s(&a[2]),
                &s(&a[3]),
                &s(&a[4]),
                a[5].as_str(),
                a[6].as_str(),
                a[7].as_str(),
                a[8].as_str(),
            ),
            "note" => cards::note(&s(&a[0]), &s(&a[1]), &s(&a[2]), &s(&a[3])),
            "links" => json!(cards::links(&strs(&a[0]), &s(&a[1]))),
            "code_checkable" => json!(checks.code_checkable(&cond(&a[0]), Some("t1"))),
            "check" => {
                let condition = cond(&a[0]);
                let index: i64 = condition
                    .text
                    .rsplit(' ')
                    .next()
                    .and_then(|n| n.parse().ok())
                    .unwrap_or(0);
                out(checks.run(index, &condition, "t1").map(|r| r.as_dict()))
            }
            "not_done_said" => json!(verify::not_done_said(&cond(&a[0]))),
            "span" => {
                let spans = w.spans();
                let mut sp = spans
                    .start(
                        "tool.call",
                        &json!({"tool": "mac.act", "kind": "click"}),
                        Some("t1"),
                        None,
                        None,
                    )
                    .unwrap();
                spans
                    .finish(
                        &mut sp,
                        ferb_store::spans::Finish {
                            output: Some(&json!({"after": {"changed": true}, "ok": false})),
                            error: Some("lost"),
                            ..Default::default()
                        },
                    )
                    .unwrap();
                Value::Null
            }
            other => panic!("cards: no op {other}"),
        };
        c.step(&st, got, &w.root);
    }
}

fn record_sources(row: &Value) -> BTreeSet<String> {
    // ferb/memory/lineage.py `record_sources` for the records these scenarios hold
    let inp = row.get("input").cloned().unwrap_or(Value::Null);
    match row.get("kind").and_then(|k| k.as_str()) {
        Some("context.source") | Some("context.exposure") => inp
            .get("source_refs")
            .and_then(|r| r.as_array())
            .map(|a| a.iter().map(s).collect())
            .unwrap_or_default(),
        _ => BTreeSet::new(),
    }
}

fn evidence_case(w: &World, c: &mut Check, sc: &Value) {
    let spans = w.spans();
    let secrets = vec!["secret-value-123".to_string(), "é\"q".to_string()];
    let te = evidence::TaskEvidence {
        spans: &spans,
        parent: Arc::new(|_| None),
        root: Arc::new(|_| Some("t1".into())),
        captures: None,
        secrets: {
            let s2 = secrets.clone();
            Arc::new(move || s2.clone())
        },
        readable: Arc::new(|_| ["t1".to_string()].into()),
        record_sources: Arc::new(record_sources),
    };
    let wrap = ports::wrap_with_id("0123456789ab");
    for st in steps(sc) {
        let a = &st["args"];
        let got = match st["op"].as_str().unwrap() {
            "task_evidence" => {
                let m = a[1].as_object().unwrap();
                let mut args = evidence::EvidenceArgs::default();
                for (k, v) in m {
                    match k.as_str() {
                        "query" => args.query = s(v),
                        "kind" => args.kind = s(v),
                        "after" => args.after = v.as_i64().unwrap(),
                        "limit" => args.limit = v.as_i64().unwrap(),
                        "span_id" => args.span_id = opt(v),
                        "span_ids" => args.span_ids = Some(strs(v)),
                        "offset" => args.offset = v.as_u64().unwrap() as usize,
                        "max_chars" => args.max_chars = v.as_u64().unwrap() as usize,
                        "evidence_id" => args.evidence_id = opt(v),
                        "task_id" => args.task_id = opt(v),
                        _ => panic!("arg {k}"),
                    }
                }
                answer(te.run(&args, &s(&a[0])))
            }
            "page_bounds" => {
                let (x, y) = evidence::page_bounds(
                    &s(&a[0]),
                    a[1].as_u64().unwrap() as usize,
                    a[2].as_u64().unwrap() as usize,
                    &strs(&a[3]),
                );
                json!([x, y])
            }
            "record_text" => json!(evidence::record_text(a[0].as_object().unwrap())),
            "how_facts" => out(evidence::how_facts(w.db, &s(&a[0]), a[1].as_i64().unwrap())),
            "recorded_calls" => out(evidence::recorded_calls(
                &spans,
                &s(&a[0]),
                &s(&a[1]),
                &|n: &str| n.starts_with("fs.read") || n.starts_with("memory"),
                &w.redact(),
                &secrets,
                &wrap,
                3,
                a[2].as_i64().unwrap(),
                a[3].as_i64().unwrap(),
            )
            .map(|bs| {
                json!(bs
                    .into_iter()
                    .map(|b| json!([b.text, b.reference, b.protected, b.priority, b.fold]))
                    .collect::<Vec<_>>())
            })),
            "memory_origin" => json!(evidence::memory_origin(&s(&a[0]))),
            "bound" => json!(evidence::bound(a[0].as_str(), a[1].as_bool().unwrap())),
            other => panic!("evidence: no op {other}"),
        };
        c.step(&st, got, &w.root);
    }
}

fn misc_case(w: &World, c: &mut Check, sc: &Value) {
    let runs = test_runs::TestRuns::new(w.db);
    let folder = format!("{}/skills", w.root);
    std::fs::create_dir_all(format!("{folder}/write-notes/ref")).unwrap();
    std::fs::write(format!("{folder}/write-notes/SKILL.md"), "---\nname: Write notes\ndescription: \"How I write\"\nname: Again\nother: x\n---\nThe body\r\nline").unwrap();
    std::fs::write(format!("{folder}/write-notes/ref/a.md"), "ref text").unwrap();
    std::fs::create_dir_all(format!("{folder}/plain")).unwrap();
    std::fs::write(format!("{folder}/plain/SKILL.md"), "no front matter").unwrap();
    std::fs::create_dir_all(format!("{folder}/broken")).unwrap();
    std::fs::write(format!("{folder}/broken/SKILL.md"), b"\xff").unwrap();
    std::os::unix::fs::symlink(format!("{folder}/write-notes"), format!("{folder}/linked"))
        .unwrap();
    std::fs::write(format!("{folder}/a-file.md"), "x").unwrap();
    let skills = skills::Skills { directory: folder };
    for st in steps(sc) {
        let a = &st["args"];
        let got = match st["op"].as_str().unwrap() {
            "session" => out(runs.session(&s(&a[0]))),
            "request" => out(runs.request(a[0].as_str())),
            "task" => out(runs.task(a[0].as_str())),
            "add" => out(runs.add(&s(&a[0]), &s(&a[1])).map(|_| Value::Null)),
            "write" => out(episodes::write(w.db, &a[0]).map(|_| Value::Null)),
            "get" => out(episodes::get(w.db, &s(&a[0])).map(|v| v.unwrap_or(Value::Null))),
            "target_words" => {
                let t: TargetsSpec = validate::task_spec_validate(&py(
                    &json!({"goal": "g", "identity": "owner", "targets": a[0]}),
                ))
                .unwrap()
                .targets;
                json!(context_layers::target_words(&TestPolicy, &t))
            }
            "ranked_memory_blocks" => json!(context_layers::ranked_memory_blocks(
                a[0].as_array().unwrap()
            )
            .into_iter()
            .map(|(b, o)| json!([
                b.text,
                b.reference,
                b.priority,
                b.origin,
                b.source_refs,
                b.payload,
                b.fold,
                o
            ]))
            .collect::<Vec<_>>()),
            "catalog" => json!(skills.catalog()),
            "skill_read" => match skills.read(&s(&a[0]), &s(&a[1])) {
                Ok(v) => v,
                Err(e) => json!({"raised": e}),
            },
            other => panic!("misc: no op {other}"),
        };
        c.step(&st, got, &w.root);
    }
}

fn decide_case(w: &World, c: &mut Check, sc: &Value) {
    use ferb_orchestrator::decide;
    let posted = Arc::new(Mutex::new(Vec::<Value>::new()));
    let replies = Arc::new(Mutex::new(vec![
        (429u16, None),
        (
            200,
            Some(
                json!({"answers": {"q0": {"noul": 0.2}, "q1": {"choice": "ignore", "confidence": 0.9}}}),
            ),
        ),
        (500, None),
        (200, Some(json!({"answers": {"q0": {"choice": "nope"}}}))),
        (402, None),
    ]));
    let p2 = posted.clone();
    let post: decide::Post = Arc::new(move |url, headers, body, timeout| {
        let headers: Map<String, Value> = headers.into_iter().map(|(k, v)| (k, json!(v))).collect();
        p2.lock()
            .unwrap()
            .push(json!([url, headers, body, timeout]));
        let r = replies.lock().unwrap().remove(0);
        Box::pin(async move { Ok(r) })
    });
    let p3 = posted.clone();
    let sleep: decide::Sleep = Arc::new(move |sec| {
        p3.lock().unwrap().push(json!(["sleep", sec]));
        Box::pin(async {})
    });
    let key: decide::Key = Arc::new(|| Box::pin(async { Some("key-123".to_string()) }));
    let client = decide::JevClient::new(key, post, sleep, decide::JEV_MODEL).unwrap();
    let mono = Arc::new(Mutex::new(10.0f64));
    let m2 = mono.clone();
    let rng = Arc::new(Mutex::new([0.05, 0.5, 0.01].repeat(10)));
    let prompts = |path: &str| {
        let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../ferb")
            .join(path);
        ferb_orchestrator::py::strip(&std::fs::read_to_string(p).unwrap()).to_string()
    };
    let d = decide::ShadowDecider::new(
        w.db,
        w.now_fn(),
        w.ids_fn(),
        Some(client),
        w.redact(),
        Arc::new(|| ["memory.worth_keeping".to_string()].into()),
        Arc::new(move || rng.lock().unwrap().remove(0)),
        Arc::new(move || {
            *m2.lock().unwrap() += 0.25;
            *m2.lock().unwrap()
        }),
        decide::questions(&prompts),
    );
    let ask =
        |k: &str, b: &str, r: Option<&str>, crit: Option<Vec<(String, String)>>| decide::Ask {
            key: k.into(),
            baseline: b.into(),
            reference: r.map(String::from),
            criteria: crit,
        };
    let pairs = |v: Vec<(String, String)>| {
        Value::Object(v.into_iter().map(|(k, x)| (k, json!(x))).collect())
    };
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    for st in steps(sc) {
        let a = &st["args"];
        let got = match st["op"].as_str().unwrap() {
            "run" => {
                let r = rt.block_on(async {
                    let mut out_ = vec![];
                    out_.push(pairs(
                        d.ask(
                            &json!({"text": "pw: hunter2"}),
                            vec![
                                ask("memory.worth_keeping#1", "yes", Some("m1"), None),
                                ask("trail.relevance", "note", Some("e1"), None),
                            ],
                        )
                        .await?,
                    ));
                    out_.push(pairs(
                        d.advice(
                            &json!({"s": 1}),
                            &[
                                ask("trail.relevance", "note", Some("e2"), None),
                                ask(
                                    "action.alignment",
                                    "aligned",
                                    None,
                                    Some(vec![("a".into(), "A".into()), ("b".into(), "B".into())]),
                                ),
                            ],
                        )
                        .await?,
                    ));
                    out_.push(pairs(
                        d.answers(
                            &json!({"s": 2}),
                            &[ask("trail.attention", "no", Some("e3"), None)],
                        )
                        .await?,
                    ));
                    out_.push(pairs(
                        d.ask(
                            &json!({"s": 3}),
                            vec![ask("trail.relevance", "note", Some("e4"), None)],
                        )
                        .await?,
                    ));
                    d.drain().await?;
                    out_.push(pairs(
                        d.answers(
                            &json!({"s": 4}),
                            &[ask("trail.attention", "no", Some("e5"), None)],
                        )
                        .await?,
                    ));
                    d.mark_effect("trail.attention#2", Some("e3"), "considered")?;
                    d.mark_effect("trail.attention", None, "x")?;
                    Ok::<_, Error>(Value::Array(out_))
                });
                out(r)
            }
            "posted" => Value::Array(posted.lock().unwrap().clone()),
            "stats" => out(d.stats().map(Value::Array)),
            "flagged_refs" => out(d
                .flagged_refs(&s(&a[0]), a[1].as_i64().unwrap())
                .map(Value::Array)),
            "questions" => json!(d
                .questions
                .iter()
                .map(|q| json!([
                    q.id,
                    q.api(),
                    q.stricter,
                    q.shadow_sample,
                    q.effect,
                    q.shadow_only,
                    q.live
                ]))
                .collect::<Vec<_>>()),
            "value" => {
                let q = d.questions.iter().find(|q| q.id == s(&a[0])).unwrap();
                match q.value(a[1].as_object().unwrap()) {
                    Ok((v, conf)) => json!([v, conf]),
                    Err(e) => json!({"raised": e}),
                }
            }
            other => panic!("decide: no op {other}"),
        };
        c.step(&st, got, &w.root);
    }
}

struct Questions {
    items: Mutex<Vec<(steering::Question, String)>>,
    log: Arc<Mutex<Vec<Value>>>,
}

impl steering::Questions for Questions {
    fn pending(&self) -> Vec<steering::Question> {
        self.items
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, st)| st == "pending")
            .map(|(q, _)| q.clone())
            .collect()
    }
    fn supersede(&self, id: &str) {
        self.log.lock().unwrap().push(json!(["supersede", id]));
        self.items
            .lock()
            .unwrap()
            .iter_mut()
            .find(|(q, _)| q.id == id)
            .unwrap()
            .1 = "superseded".into();
    }
    fn resolve(&self, id: &str, text: &str) {
        self.log
            .lock()
            .unwrap()
            .push(json!(["resolve", id, true, text]));
        self.items
            .lock()
            .unwrap()
            .iter_mut()
            .find(|(q, _)| q.id == id)
            .unwrap()
            .1 = "answered".into();
    }
    fn state_of(&self, id: &str) -> String {
        self.items
            .lock()
            .unwrap()
            .iter()
            .find(|(q, _)| q.id == id)
            .unwrap()
            .1
            .clone()
    }
    fn tree_of(&self, task_id: &str) -> Option<String> {
        Some(task_id.split('.').next().unwrap().to_string())
    }
    fn publish(&self, topic: &str, data: Value) {
        self.log
            .lock()
            .unwrap()
            .push(json!(["publish", topic, data]));
    }
}

struct JournalAppender(Rc<dyn Journal>);

impl steering::Appender for JournalAppender {
    fn append(&self, kind: &str, data: Value) -> String {
        self.0.append(kind, data, None).id
    }
}

struct Standing {
    held: Vec<(SendScope, String)>,
    log: Arc<Mutex<Vec<Value>>>,
    journal: Rc<dyn Journal>,
    redactor: Arc<ferb_sanitize::Redactor>,
}

fn scope_dump(s: &SendScope) -> Value {
    validate::dump_send_scope(s).to_value()
}

impl steering::StandingHost for Standing {
    fn redacted_scope(&self, scope: &SendScope) -> SendScope {
        validate::send_scope_validate(&Py::from_value(&self.redactor.value(&scope_dump(scope))))
            .unwrap()
    }
    fn redact_text(&self, text: &str) -> String {
        s(&self.redactor.value(&json!(text)))
    }
    fn in_force(&self, _: &str) -> Vec<(SendScope, String)> {
        self.held.clone()
    }
    fn scopes(&self, _: &str) -> Vec<SendScope> {
        self.held.iter().map(|(x, _)| x.clone()).collect()
    }
    fn revoke(&self, tree_id: &str) {
        self.log.lock().unwrap().push(json!(["revoke", tree_id]));
    }
    fn grant_each(&self, tree_id: &str, scopes: Vec<(SendScope, String)>) {
        self.log.lock().unwrap().push(json!([
            "grant_each",
            tree_id,
            scopes
                .iter()
                .map(|(x, a)| json!([scope_dump(x), a]))
                .collect::<Vec<_>>()
        ]));
    }
    fn grant(&self, tree_id: &str, scopes: Vec<SendScope>, approval_id: &str) {
        self.log.lock().unwrap().push(json!([
            "grant",
            tree_id,
            scopes.iter().map(scope_dump).collect::<Vec<_>>(),
            approval_id
        ]));
    }
    fn owner_typed_scopes(&self, _: &str, revised: &[SendScope]) -> Vec<SendScope> {
        revised
            .iter()
            .filter(|x| !x.recipients.iter().any(|r| r == "zed@y.com"))
            .cloned()
            .collect()
    }
    fn unattended_request(&self, request_id: &str) -> bool {
        request_id == "r-sched"
    }
    fn stated(&self, tree_id: &str, text: &str) -> String {
        self.journal
            .append(
                "plan.stated",
                json!({"task_id": tree_id, "text": text}),
                None,
            )
            .id
    }
    fn update_plan(&self, tree_id: &str, standing_sends: &[SendScope]) {
        self.log.lock().unwrap().push(json!([
            "update_plan",
            tree_id,
            standing_sends.iter().map(scope_dump).collect::<Vec<_>>()
        ]));
    }
}

fn steering_case(w: &World, c: &mut Check, sc: &Value) {
    let log = Arc::new(Mutex::new(Vec::<Value>::new()));
    let q = |id: &str, request: &str, task: &str, cap: &str, kind: &str, only: bool| {
        (
            steering::Question {
                id: id.into(),
                request_id: request.into(),
                task_id: task.into(),
                capability: cap.into(),
                kind: kind.into(),
                options_only: only,
            },
            "pending".to_string(),
        )
    };
    let questions = Questions {
        items: Mutex::new(vec![
            q("q1", "r1", "", "ferb.confirm", "owner_question", false),
            q(
                "q2",
                "r9",
                "t1.worker",
                "ferb.retry",
                "owner_question",
                false,
            ),
            q(
                "q3",
                "r9",
                "t2.worker",
                "ferb.confirm",
                "owner_question",
                false,
            ),
            q("q4", "r1", "", "owner.ask", "owner_question", false),
            q("q5", "r1", "", "owner.ask", "owner_question", true),
            q("q6", "r1", "", "memory.forget", "owner_question", false),
            q("q7", "r1", "", "owner.ask", "secret", false),
            q("q8", "r1", "", "conversation.approve", "other", false),
            q("q9", "r1", "", "owner.ask", "owner_question", false),
        ]),
        log: log.clone(),
    };
    let journal = w.journal();
    let appender = JournalAppender(journal.clone());
    let mut state = steering::Steering {
        messages: vec!["first".into(), "second".into()],
        ..Default::default()
    };
    state.delivered.insert("run-2".into(), 1);
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let scope_of = |v: &Value| validate::send_scope_validate(&py(v)).unwrap();
    for st in steps(sc) {
        let a = &st["args"];
        let got = match st["op"].as_str().unwrap() {
            "settle_questions" => {
                steering::settle_questions(
                    &questions,
                    &appender,
                    &s(&a[0]),
                    &s(&a[1]),
                    &strs(&a[2]),
                    a[3].as_str(),
                );
                Value::Null
            }
            "log" => {
                let v = if st["out"]
                    .as_array()
                    .is_some_and(|o| o.len() == 2 && o[1].is_object())
                {
                    let delivered: Map<String, Value> = state
                        .delivered
                        .iter()
                        .map(|(k, v)| (k.clone(), json!(v)))
                        .collect();
                    json!([log.lock().unwrap().clone(), Value::Object(delivered)])
                } else {
                    Value::Array(log.lock().unwrap().clone())
                };
                log.lock().unwrap().clear();
                v
            }
            "restate_scope" => {
                let standing = if a[0] == json!("unchanged") {
                    steering::Standing::Unchanged
                } else {
                    steering::Standing::Revised(
                        a[0].as_array().unwrap().iter().map(scope_of).collect(),
                    )
                };
                let held: Vec<(SendScope, String)> = a[1]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|p| (scope_of(&p[0]), s(&p[1])))
                    .collect();
                let plan_sends: Vec<SendScope> = vec![scope_of(
                    &json!({"capability": "browser.act", "identity": "owner", "account": "me", "site": "x.com", "recipients": [], "content": null}),
                )];
                let host = Standing {
                    held,
                    log: log.clone(),
                    journal: journal.clone(),
                    redactor: w.redactor.clone(),
                };
                let tree = steering::TreeScope {
                    id: a[5].as_str().unwrap(),
                    plan_sends: if a[2].as_bool().unwrap() {
                        Some(&plan_sends)
                    } else {
                        None
                    },
                    unattended: a[3].as_bool().unwrap(),
                    request_id: a[4].as_str().unwrap(),
                };
                match steering::restate_scope(&host, &tree, &standing) {
                    None => Value::Null,
                    Some(r) => json!([
                        r.text,
                        r.scopes.iter().map(scope_dump).collect::<Vec<_>>(),
                        r.approval_id,
                        r.widened
                    ]),
                }
            }
            "deliver" => {
                let run = steering::Run {
                    id: s(&a[0]),
                    task_id: "t1.worker".into(),
                };
                let fail = opt(&a[1]);
                let log2 = log.clone();
                let rid = run.id.clone();
                json!(rt.block_on(steering::deliver(
                    &appender,
                    "t1",
                    &mut state,
                    &run,
                    move |text| {
                        log2.lock().unwrap().push(json!(["continue", rid, text]));
                        Box::pin(async move {
                            match fail.as_deref() {
                                None => Ok(()),
                                Some("Unsupported") => Err(steering::ContinueError::Unsupported),
                                Some(kind) => {
                                    Err(steering::ContinueError::Failed(kind.to_string()))
                                }
                            }
                        })
                    }
                )))
            }
            other => panic!("steering: no op {other}"),
        };
        c.step(&st, got, &w.root);
    }
}

fn chat_history_case(w: &World, c: &mut Check, sc: &Value) {
    let origins: ports::Lookup<BTreeSet<String>> =
        Arc::new(|_| ["external:web:x.com".to_string()].into());
    let cont = continuity::Continuity::new(
        w.db,
        w.redact(),
        origins,
        Arc::new(|k: &str, d: &Value| journal_origin(k, d)),
        Arc::new(|| "x".into()),
    );
    let search: evidence::Search = Arc::new(|_, _, _, _, _| {
        Box::pin(async { vec!["id-000003".to_string(), "id-000001".into(), "nope".into()] })
    });
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    for st in steps(sc) {
        let a = &st["args"];
        let task = s(&a[0]);
        let mut args = evidence::HistoryArgs::default();
        for (k, v) in a[1].as_object().unwrap() {
            match k.as_str() {
                "query" => args.query = s(v),
                "since" => args.since = opt(v),
                "until" => args.until = opt(v),
                "entry_id" => args.entry_id = opt(v),
                "task_id" => args.task_id = opt(v),
                "include_tasks" => args.include_tasks = v.as_bool().unwrap(),
                "after" => args.after = v.as_i64().unwrap(),
                "limit" => args.limit = v.as_i64().unwrap(),
                "offset" => args.offset = v.as_u64().unwrap() as usize,
                "max_chars" => args.max_chars = v.as_u64().unwrap() as usize,
                _ => panic!("arg {k}"),
            }
        }
        let root = if task == "orphan" {
            None
        } else {
            Some(task.clone())
        };
        let with = if a[2].as_bool().unwrap() {
            None
        } else {
            Some(&search)
        };
        let got = answer(rt.block_on(evidence::chat_history(&cont, root, with, &args)));
        c.step(&st, got, &w.root);
    }
}

#[test]
fn stores_match_python() {
    let all = fixture("stores.json");
    let mut differ = Vec::new();
    let mut total = 0;
    for scenario in all.as_array().unwrap() {
        let name = scenario["name"].as_str().unwrap().to_string();
        let (_dir, w) = open_world(scenario);
        let mut c = Check {
            name: name.clone(),
            differ: vec![],
            count: 0,
        };
        match name.as_str() {
            "plan_store" => plan_store_case(&w, &mut c, scenario),
            "ledger" => ledger_case(&w, &mut c, scenario),
            "tree" => tree_case(&w, &mut c, scenario),
            "continuity" => continuity_case(&w, &mut c, scenario),
            "conversations" => conversations_case(&w, &mut c, scenario),
            "conversation_runner" => runner_case(&w, &mut c, scenario),
            "parts" => parts_case(&w, &mut c, scenario),
            "loops" => loops_case(&w, &mut c, scenario),
            "plan_executor" => executor_case(&w, &mut c, scenario),
            "attention" => attention_case(&w, &mut c, scenario),
            "cards" => cards_case(&w, &mut c, scenario),
            "evidence" => evidence_case(&w, &mut c, scenario),
            "misc" => misc_case(&w, &mut c, scenario),
            "decide" => decide_case(&w, &mut c, scenario),
            "steering" => steering_case(&w, &mut c, scenario),
            "chat_history" => chat_history_case(&w, &mut c, scenario),
            other => panic!("no scenario {other}"),
        }
        c.tables(scenario, w.db, &w.root);
        total += c.count;
        differ.extend(c.differ);
    }
    if let Ok(path) = std::env::var("FERB_DIFF") {
        std::fs::write(path, differ.join("\n")).unwrap();
    }
    let shown: Vec<String> = differ
        .iter()
        .take(12)
        .map(|d| d.chars().take(3000).collect())
        .collect();
    assert!(
        differ.is_empty(),
        "{} of {total} differ:\n{}",
        differ.len(),
        shown.join("\n")
    );
    eprintln!("stores: {total} comparisons, none differ");
}
