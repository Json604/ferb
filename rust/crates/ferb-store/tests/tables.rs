//! The typed stores against what the Python stores did (decision: ENG-3).
//! `rust/fixtures/store/make_tables.py` drove `ferb/store` through scripted, invented operations
//! and wrote `tables.json`: every call's return (or what it raised) and the tables' rows
//! afterwards. Each test here replays one scenario on a store the Rust code made and must get
//! the same returns and the same rows, text columns byte for byte. Temporary folders only.

use std::{
    cell::{Cell, RefCell},
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
    rc::Rc,
};

use ferb_store::{
    chat_history::{self, ChatHistory},
    cleanup::{self, CardSources, JournalSink},
    pyjson::{self, Style},
    pytime::{DateTime, Timespec},
    retention, salvage,
    spans::{self, Clock, Evidence, Finish, IdSource, Span, SpanNode, SpanStore},
    swap::swap,
    task_results::TaskResults,
    Db, Error,
};
use rusqlite::{params_from_iter, types::Value as Sql, Connection};
use serde_json::{json, Map, Value};
use tempfile::TempDir;

fn fixture() -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/store/tables.json");
    serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap()
}

/// The fixture's fake redactor and rewrite: `find` replaced in every string value.
fn replace_text(value: &Value, find: &str, new: &str) -> Value {
    match value {
        Value::String(s) => json!(s.replace(find, new)),
        Value::Array(items) => items.iter().map(|v| replace_text(v, find, new)).collect(),
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(k, v)| (k.clone(), replace_text(v, find, new)))
                .collect(),
        ),
        other => other.clone(),
    }
}

fn redact() -> ferb_store::Redact {
    Box::new(|value| replace_text(value, "SECRET", "[redacted]"))
}

fn sql(value: &Value) -> Sql {
    match value {
        Value::Null => Sql::Null,
        Value::Bool(b) => Sql::Integer(i64::from(*b)),
        Value::Number(n) if n.is_f64() => Sql::Real(n.as_f64().unwrap()),
        Value::Number(n) => Sql::Integer(n.as_i64().unwrap()),
        Value::String(s) => Sql::Text(s.clone()),
        other => panic!("no SQL form for {other}"),
    }
}

fn unsql(value: Sql) -> Value {
    match value {
        Sql::Null => Value::Null,
        Sql::Integer(n) => json!(n),
        Sql::Real(f) => json!(f),
        Sql::Text(t) => json!(t),
        Sql::Blob(b) => json!({ "blob": hex(&b) }),
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn dump(conn: &Connection, table: &str) -> Value {
    let mut statement = conn
        .prepare(&format!("SELECT * FROM {table} ORDER BY rowid"))
        .unwrap();
    let columns = statement.column_count();
    let rows = statement
        .query_map([], |r| {
            (0..columns)
                .map(|i| r.get::<_, Sql>(i).map(unsql))
                .collect::<rusqlite::Result<Vec<Value>>>()
        })
        .unwrap();
    Value::Array(rows.map(|r| Value::Array(r.unwrap())).collect())
}

/// What Python raised, as the fixture names it.
fn raised(error: &Error) -> Value {
    match error {
        Error::Refused(m) => json!(["Refused", m]),
        Error::Value(m) => json!(["ValueError", m]),
        Error::NewerStore { .. } => json!(["NewerStore", error.to_string()]),
        Error::Sqlite(rusqlite::Error::SqliteFailure(failure, message))
            if failure.code == rusqlite::ErrorCode::ConstraintViolation =>
        {
            json!(["IntegrityError", message.clone().unwrap_or_default()])
        }
        Error::Io(e) if e.kind() == std::io::ErrorKind::NotFound => {
            json!(["FileNotFoundError", e.to_string()])
        }
        other => json!(["?", other.to_string()]),
    }
}

struct StepClock(Rc<Cell<i64>>);

impl Clock for StepClock {
    fn now(&self) -> DateTime {
        let n = self.0.get() + 1;
        self.0.set(n);
        DateTime::from_isoformat("2026-01-02T03:04:05+00:00")
            .unwrap()
            .plus(n, 250_000 * n)
            .unwrap()
    }
}

struct StepIds(Rc<Cell<u32>>);

impl IdSource for StepIds {
    fn new_id(&self) -> String {
        let n = self.0.get() + 1;
        self.0.set(n);
        format!("s-{n:06}")
    }
}

/// The fixture's `FakeCards`: one card version loses two refs; pruning deletes those rows.
struct FakeCards;

impl CardSources for FakeCards {
    fn card_source_plan(
        &self,
        conn: &Connection,
        removing: &[String],
    ) -> ferb_store::Result<Value> {
        let mut statement =
            conn.prepare("SELECT source_ref FROM learning_sources WHERE node_id = 'card-1'")?;
        let have: HashSet<String> = statement
            .query_map([], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        let gone: Vec<&str> = ["trail:a", "trail:b"]
            .into_iter()
            .filter(|r| have.contains(*r))
            .collect();
        let mut removing = removing.to_vec();
        removing.sort();
        let drop = if gone.is_empty() {
            json!({})
        } else {
            json!({ "card-1": gone })
        };
        Ok(
            json!({"tasks": 1, "nodes": 3, "untraced": 0, "stopped": false, "drop": drop,
                  "removing": removing}),
        )
    }

    fn prune_card_sources(
        &self,
        conn: &Connection,
        _stop: &dyn Fn() -> bool,
        plan: &Value,
    ) -> ferb_store::Result<Value> {
        let mut rows = 0;
        for (node, refs) in plan["drop"].as_object().unwrap() {
            for source in refs.as_array().unwrap() {
                rows += conn.execute(
                    "DELETE FROM learning_sources WHERE node_id = ? AND source_ref = ?",
                    [node.as_str(), source.as_str().unwrap()],
                )?;
            }
        }
        Ok(json!({"rows": rows, "stopped": false}))
    }
}

struct Recorded(Vec<Value>);

impl JournalSink for Recorded {
    fn append(&mut self, kind: &str, data: &Value) -> ferb_store::Result<()> {
        self.0.push(json!([kind, sizes_out(data)]));
        Ok(())
    }
}

/// Byte counts depend on the SQLite build's page use: checked to be numbers, then masked as the
/// fixture masks them.
fn sizes_out(value: &Value) -> Value {
    let mut out = value.clone();
    for key in ["bytes", "needed", "bytes_before", "bytes_after"] {
        if let Some(size) = out.get_mut(key) {
            assert!(size.as_u64().is_some(), "{key} is a byte count");
            *size = json!("<bytes>");
        }
    }
    out
}

struct World {
    _folder: TempDir,
    db: Db,
    clock: Rc<Cell<i64>>,
    ids: Rc<Cell<u32>>,
    chat: Rc<Cell<u32>>,
    held: RefCell<HashMap<String, Span>>,
    reversible: Vec<String>,
}

fn span_json(span: &Span) -> Value {
    let mut node = spans::node_json(&SpanNode {
        span: span.clone(),
        children: Vec::new(),
    });
    node.as_object_mut().unwrap().shift_remove("children");
    node
}

fn text<'a>(args: &'a Value, key: &str) -> &'a str {
    args[key]
        .as_str()
        .unwrap_or_else(|| panic!("{key} in {args}"))
}

fn maybe<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key).and_then(Value::as_str)
}

fn int(args: &Value, key: &str) -> i64 {
    args[key].as_i64().unwrap()
}

fn flag(args: &Value, key: &str) -> bool {
    args[key].as_bool().unwrap()
}

fn sets(found: impl IntoIterator<Item = String>) -> Value {
    Value::Array(found.into_iter().map(Value::String).collect())
}

impl World {
    fn new(fixture: &Value) -> World {
        let folder = TempDir::new().unwrap();
        let db = Db::open(folder.path().join("ferb.db")).unwrap();
        db.migrate().unwrap();
        World {
            _folder: folder,
            db,
            clock: Rc::default(),
            ids: Rc::default(),
            chat: Rc::default(),
            held: RefCell::default(),
            reversible: serde_json::from_value(fixture["reversible"].clone()).unwrap(),
        }
    }

    fn spans(&self) -> SpanStore<'_> {
        SpanStore::new(
            &self.db,
            Box::new(StepClock(self.clock.clone())),
            Box::new(StepIds(self.ids.clone())),
            redact(),
            Box::new(|kind| self.reversible.iter().any(|k| k == kind)),
        )
    }

    fn chat(&self) -> ChatHistory<'_> {
        let n = self.chat.clone();
        ChatHistory::new(
            &self.db,
            redact(),
            Box::new(move || {
                n.set(n.get() + 1);
                format!("2026-11-09T12:00:{:02}+00:00", n.get())
            }),
        )
    }

    fn call(&self, name: &str, a: &Value) -> ferb_store::Result<Value> {
        let conn = self.db.conn();
        let scope = Evidence {
            root_id: maybe(a, "root_id"),
            whole: a.get("whole").and_then(Value::as_bool).unwrap_or(false),
        };
        let root = || text(a, "root_id");
        Ok(match name {
            "spans.start" => {
                let span = self.spans().start(
                    text(a, "kind"),
                    &a["input"],
                    maybe(a, "task_id"),
                    maybe(a, "parent_id"),
                    maybe(a, "device_id"),
                )?;
                self.held.borrow_mut().insert(span.id.clone(), span.clone());
                span_json(&span)
            }
            "spans.finish" => {
                let mut held = self.held.borrow_mut();
                let span = held.get_mut(text(a, "id")).unwrap();
                self.spans().finish(
                    span,
                    Finish {
                        output: a.get("output").filter(|o| !o.is_null()),
                        error: maybe(a, "error"),
                        cost_usd: a.get("cost_usd").and_then(Value::as_f64),
                        origin: maybe(a, "origin"),
                        summary: maybe(a, "summary"),
                    },
                )?;
                span_json(span)
            }
            "spans.set_origin" => {
                self.spans().set_origin(text(a, "id"), text(a, "origin"))?;
                Value::Null
            }
            "spans.update_input" => {
                self.spans().update_input(text(a, "id"), &a["values"])?;
                Value::Null
            }
            "spans.tree_json" => json!(self.spans().tree_json(text(a, "task_id"))?),
            "spans.tree" => {
                let tree = self.spans().tree(text(a, "task_id"))?;
                json!(tree.iter().map(spans::node_json).collect::<Vec<_>>())
            }
            "spans.outcome" => self.spans().outcome(text(a, "task_id"))?,
            "spans.tool_origins" => sets(self.spans().tool_origins(root())?),
            "spans.request_origins" => sets(self.spans().request_origins(root())?),
            "spans.asked_origins" => sets(self.spans().asked_origins(root())?),
            "spans.reopened_answers" => sets(self.spans().reopened_answers(root())?),
            "spans.exposure_origins" => sets(self.spans().exposure_origins(root())?),
            "spans.outside_exposure_origins" => {
                sets(self.spans().outside_exposure_origins(root())?)
            }
            "spans.task_origins" => sets(self.spans().task_origins(root())?),
            "spans.request_began" => json!(self.spans().request_began(
                root(),
                text(a, "request_id"),
                text(a, "worker_id")
            )?),
            "spans.tool_outputs" => json!(self.spans().tool_outputs(
                root(),
                int(a, "after"),
                flag(a, "include_context")
            )?),
            "spans.payload_outputs" => {
                let seen: Option<HashSet<i64>> = a["seen"]
                    .as_array()
                    .map(|s| s.iter().map(|v| v.as_i64().unwrap()).collect());
                let found = self
                    .spans()
                    .payload_outputs(root(), int(a, "after"), seen.as_ref())?;
                found
                    .into_iter()
                    .map(|p| {
                        json!({"rowid": p.rowid, "origins": p.origins, "text": p.text, "path": p.path,
                               "offset": p.offset, "next_offset": p.next_offset,
                               "segments": p.segments, "page": p.page})
                    })
                    .collect()
            }
            "spans.latest_acting_call" => {
                json!(self.spans().latest_acting_call(root(), maybe(a, "tool"))?)
            }
            "spans.first_open_tool_call" => json!(self.spans().first_open_tool_call(
                root(),
                int(a, "after"),
                flag(a, "include_context")
            )?),
            "spans.owner_answers" => json!(self.spans().owner_answers(root())?),
            "spans.owner_questions" => self
                .spans()
                .owner_questions(root())?
                .into_iter()
                .map(|(question, answer)| json!({"question": question, "answer": answer}))
                .collect(),
            "spans.last_typed" => json!(self.spans().last_typed(
                text(a, "task_id"),
                text(a, "tool"),
                text(a, "tab")
            )?),
            "spans.typed_in_tab" => {
                json!(self
                    .spans()
                    .typed_in_tab(text(a, "task_id"), text(a, "tab"))?)
            }
            "spans.tool_call" => {
                json!(self
                    .spans()
                    .tool_call(text(a, "task_id"), text(a, "span_id"), scope)?)
            }
            "spans.tool_calls" => self.spans().tool_calls(
                text(a, "task_id"),
                text(a, "query"),
                int(a, "after"),
                int(a, "limit"),
                text(a, "kind"),
                scope,
            )?,
            "spans.evidence_kinds" => {
                json!(self.spans().evidence_kinds(text(a, "task_id"), scope)?)
            }
            "spans.rewrite" => {
                let needles: Option<Vec<String>> =
                    serde_json::from_value(a["needles"].clone()).unwrap();
                let held: Option<(String, Option<String>)> =
                    serde_json::from_value(a["held"].clone()).unwrap();
                let (find, new) = (text(a, "find"), text(a, "new"));
                json!(spans::rewrite_recorded(
                    &self.db,
                    text(a, "ident"),
                    |i, o| (replace_text(i, find, new), replace_text(o, find, new)),
                    needles.as_deref(),
                    held,
                )?)
            }
            "swap" => {
                let columns: Vec<&str> = a["columns"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|c| c.as_str().unwrap())
                    .collect();
                let new: Vec<Sql> = a["new"].as_array().unwrap().iter().map(sql).collect();
                let held = a["held"]
                    .as_array()
                    .map(|h| h.iter().map(sql).collect::<Vec<_>>());
                let written = swap(
                    conn,
                    text(a, "table"),
                    text(a, "key"),
                    &sql(&a["ident"]),
                    &columns,
                    |_| Ok(new.clone()),
                    held,
                )?;
                json!(written.map(|row| row.into_iter().map(unsql).collect::<Vec<_>>()))
            }
            "results.record" => {
                TaskResults::new(&self.db).record(
                    text(a, "task_id"),
                    text(a, "at"),
                    text(a, "status"),
                    text(a, "goal"),
                    text(a, "answer"),
                    flag(a, "shown"),
                    a.get("cards").filter(|c| !c.is_null()),
                    text(a, "name"),
                )?;
                Value::Null
            }
            "results.interrupted" => {
                TaskResults::new(&self.db).interrupted(
                    text(a, "task_id"),
                    text(a, "at"),
                    text(a, "goal"),
                    text(a, "answer"),
                )?;
                Value::Null
            }
            "results.unshown" => json!(TaskResults::new(&self.db).unshown()?),
            "results.page" => {
                json!(TaskResults::new(&self.db).page(text(a, "before"), int(a, "limit"))?)
            }
            "results.latest_of_schedule" => {
                json!(TaskResults::new(&self.db).latest_of_schedule(text(a, "schedule_id"))?)
            }
            "results.record_verdict" => {
                TaskResults::new(&self.db).record_verdict(
                    text(a, "verdict_id"),
                    text(a, "task_id"),
                    text(a, "verdict"),
                    text(a, "note"),
                    text(a, "at"),
                )?;
                Value::Null
            }
            "results.mark_shown" => {
                let ids: Vec<&str> = a["task_ids"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|t| t.as_str().unwrap())
                    .collect();
                TaskResults::new(&self.db).mark_shown(&ids)?;
                Value::Null
            }
            "chat.write" => self.chat().write(&a["entries"], flag(a, "replace"))?,
            "chat.page" => self
                .chat()
                .page(text(a, "day"), maybe(a, "before"), int(a, "limit"))?,
            "chat.open" => json!(self.chat().open()?),
            "chat.days" => json!(self.chat().days()?),
            "chat.clear_notice" => {
                json!(chat_history::clear_notice(
                    &self.db,
                    text(a, "ref"),
                    text(a, "text")
                )?)
            }
            "retention.windows" | "retention.describe" => {
                let cfg = a["cfg"].as_object().unwrap();
                let get = |section: &str, key: &str, default: i64| {
                    cfg.get(&format!("{section}.{key}"))
                        .map_or(default, |v| v.as_i64().unwrap())
                };
                if name == "retention.windows" {
                    json!(retention::windows(&get))
                } else {
                    json!(retention::describe(&get))
                }
            }
            "retention.finished_roots_before" => {
                json!(retention::finished_roots_before(conn, text(a, "cutoff"))?)
            }
            "retention.operational" => json!(retention::operational(conn, text(a, "cutoff"))?),
            "retention.plan" => {
                let plan = retention::plan(
                    conn,
                    text(a, "trace"),
                    text(a, "decision"),
                    text(a, "operational"),
                    int(a, "limit") as usize,
                )?;
                json!({"roots": plan.roots, "trees": plan.trees, "loose": plan.loose,
                       "decisions": plan.decisions, "operational": plan.operational,
                       "left": plan.left})
            }
            "retention.delete_rows" => {
                let ids: Vec<Sql> = a["ids"].as_array().unwrap().iter().map(sql).collect();
                let counts = retention::delete_rows(
                    &self.db,
                    text(a, "table"),
                    text(a, "key"),
                    &ids,
                    int(a, "batch") as usize,
                )
                .collect::<ferb_store::Result<Vec<usize>>>()?;
                json!(counts)
            }
            "cleanup.plan" => cleanup::plan(conn, &FakeCards)?,
            "cleanup.preview" => {
                let free = a["free"].as_u64().unwrap();
                sizes_out(&cleanup::preview(self.db.path(), &FakeCards, |_| Ok(free))?)
            }
            "cleanup.pending" => json!(cleanup::pending(self.db.path())),
            "cleanup.apply" => {
                let after = a.get("stop_after").and_then(Value::as_i64);
                let asked = Cell::new(0i64);
                let stop = || {
                    asked.set(asked.get() + 1);
                    after.is_some_and(|n| asked.get() > n)
                };
                let free = a.get("free").and_then(Value::as_u64).unwrap_or(1 << 60);
                let mut journal = Recorded(Vec::new());
                let out = cleanup::apply(
                    &self.db,
                    &mut journal,
                    &FakeCards,
                    text(a, "ok"),
                    cleanup::Run {
                        stop: &stop,
                        say: &|_| {},
                        free: &|_| Ok(free),
                    },
                )?;
                json!({"out": sizes_out(&out), "journal": journal.0})
            }
            other => panic!("no such call: {other}"),
        })
    }
}

fn same(got: &Value, want: &Value, what: &str) {
    if got != want {
        let (got, want) = (pyjson::dumps_utf8(got), pyjson::dumps_utf8(want));
        let at = got
            .chars()
            .zip(want.chars())
            .position(|(a, b)| a != b)
            .unwrap_or(0);
        let cut = |s: &str| {
            s.chars()
                .skip(at.saturating_sub(80))
                .take(400)
                .collect::<String>()
        };
        panic!(
            "{what}\n  Rust:   …{}\n  Python: …{}",
            cut(&got),
            cut(&want)
        );
    }
}

fn replay(scenario: &str) {
    let fixture = fixture();
    let world = World::new(&fixture);
    let steps = fixture["scenarios"][scenario].as_array().unwrap();
    for (i, step) in steps.iter().enumerate() {
        if let Some(query) = step.get("sql") {
            let params: Vec<Sql> = step["params"].as_array().unwrap().iter().map(sql).collect();
            world
                .db
                .conn()
                .execute(query.as_str().unwrap(), params_from_iter(params))
                .unwrap_or_else(|e| panic!("{scenario} step {i}: {e}"));
        } else if let Some(name) = step.get("call").and_then(Value::as_str) {
            let what = format!("{scenario} step {i}: {name} {}", step["args"]);
            match (world.call(name, &step["args"]), step.get("err")) {
                (Ok(got), None) => same(&got, &step["ret"], &what),
                (Err(error), Some(want)) => same(&raised(&error), want, &what),
                (Ok(got), Some(want)) => panic!("{what}: Python raised {want}, Rust gave {got}"),
                (Err(error), None) => panic!("{what}: Rust failed: {error}"),
            }
            assert!(world.db.conn().is_autocommit(), "{what} left a transaction");
        } else if let Some(tables) = step.get("dump") {
            for (table, rows) in tables.as_object().unwrap() {
                same(
                    &dump(world.db.conn(), table),
                    rows,
                    &format!("{scenario} step {i}: rows of {table}"),
                );
            }
        } else if let Some(tables) = step.get("tables") {
            let mut statement = world
                .db
                .conn()
                .prepare(
                    "SELECT name FROM sqlite_master WHERE type = 'table' AND name IN \
                     ('owner_questions', 'notification_rules', 'learning_receipts', 'learning_jobs') ORDER BY name",
                )
                .unwrap();
            let left: Vec<String> = statement
                .query_map([], |r| r.get(0))
                .unwrap()
                .map(Result::unwrap)
                .collect();
            same(
                &json!(left),
                tables,
                &format!("{scenario} step {i}: tables left"),
            );
        } else {
            panic!("{scenario} step {i}: unknown step {step}");
        }
    }
}

// decisions: STORE-413, GATE-215, GATE-525, ORCH-617
#[test]
fn spans_are_kept_read_and_exported_as_python_does() {
    replay("spans");
}

// decisions: STORE-401, DMN-544
#[test]
fn task_results_are_kept_and_paged_as_python_does() {
    replay("results");
}

// decisions: APP-805, DMN-555, DMN-556
#[test]
fn the_chat_is_written_paged_thirty_at_a_time_and_cleared_as_python_does() {
    replay("chat");
}

// decisions: STORE-400, STORE-401, STORE-404, STORE-406, STORE-407, STORE-408, STORE-418, STORE-420
#[test]
fn retention_plans_and_deletes_what_python_does() {
    replay("retention");
}

// decisions: STORE-402, STORE-411, STORE-412, STORE-416
#[test]
fn the_clean_up_removes_what_python_does_only_on_the_matching_ok() {
    replay("cleanup");
    replay("cleanup_compact");
}

// decision: STORE-402
#[test]
fn the_clean_up_never_writes_a_protected_table() {
    let fixture = fixture();
    let world = World::new(&fixture);
    let conn = world.db.conn();
    for step in fixture["scenarios"]["cleanup"].as_array().unwrap() {
        if let Some(query) = step.get("sql") {
            let params: Vec<Sql> = step["params"].as_array().unwrap().iter().map(sql).collect();
            conn.execute(query.as_str().unwrap(), params_from_iter(params))
                .unwrap();
        }
    }
    conn.execute_batch(
        "INSERT INTO tasks (id, tree_id, request_id, goal, identity, role, scopes_json, status, created_at, updated_at) \
         VALUES ('K', 'K', 'r', 'g', 'ferb', 'supervisor', '[]', 'done', '2026', '2026'); \
         INSERT INTO chat_history (id, kind, at, day, body_json, updated_at) VALUES ('C', 'turn', '2026', '2026-01-01', '{}', '2026'); \
         INSERT INTO spans (id, kind, started_at, input_json) VALUES ('S', 'task', '2026', '{}'); \
         INSERT INTO journal (id, at, kind, data_json) VALUES ('J', '2026', 'chat.message', '{}');",
    )
    .unwrap();
    let protected = |conn: &Connection| -> Vec<Value> {
        cleanup::PROTECTED
            .iter()
            .map(|table| {
                let rows = cleanup::PROTECTED_ROWS
                    .iter()
                    .find(|(t, _)| t == table)
                    .map_or(String::new(), |(_, rows)| format!(" WHERE {rows}"));
                // a run clears Jev's relevance labels on activity rows; the rows themselves stay
                let columns = if *table == "trail" {
                    "id, at, day, device_id, actor, task_id, source, data_json, sensitive"
                } else {
                    "*"
                };
                let mut statement = conn
                    .prepare(&format!(
                        "SELECT {columns} FROM {table}{rows} ORDER BY rowid"
                    ))
                    .unwrap();
                let columns = statement.column_count();
                let rows: Vec<Value> = statement
                    .query_map([], |r| {
                        (0..columns)
                            .map(|i| r.get::<_, Sql>(i).map(unsql))
                            .collect::<rusqlite::Result<Vec<Value>>>()
                    })
                    .unwrap()
                    .map(|r| Value::Array(r.unwrap()))
                    .collect();
                json!([table, rows])
            })
            .collect()
    };
    let before = protected(conn);
    let plan = cleanup::plan(conn, &FakeCards).unwrap();
    let mut journal = Recorded(Vec::new());
    let out = cleanup::apply(
        &world.db,
        &mut journal,
        &FakeCards,
        plan["ok"].as_str().unwrap(),
        cleanup::Run {
            stop: &|| false,
            say: &|_| {},
            free: &|_| Ok(1 << 60),
        },
    )
    .unwrap();
    assert_eq!(out["compacted"], json!(true));
    assert_eq!(protected(world.db.conn()), before);
    assert!(!cleanup::pending(world.db.path()));
}

// decision: STORE-5
#[test]
fn the_rule_tables_are_python_s() {
    let fixture = fixture();
    assert_eq!(
        json!(retention::OPERATIONAL_KINDS.to_vec()),
        fixture["operational_kinds"]
    );
    let rules: Vec<Value> = retention::RULES
        .iter()
        .map(|r| {
            json!({"records": r.records, "where": r.place, "kept": r.kept, "window": r.window,
                   "setting": r.setting.map(|(s, k)| vec![s, k]), "default": r.default})
        })
        .collect();
    same(&json!(rules), &fixture["rules"], "retention.RULES");
    let pairs =
        |list: &[(&str, &str)]| -> Value { list.iter().map(|(a, b)| json!([a, b])).collect() };
    let c = &fixture["cleanup"];
    assert_eq!(pairs(&cleanup::EMPTIED), c["emptied"]);
    assert_eq!(pairs(&cleanup::DROPPED), c["dropped"]);
    let mut trimmed = cleanup::TRIMMED.to_vec();
    trimmed.sort();
    assert_eq!(json!(trimmed), c["trimmed"]);
    assert_eq!(json!(cleanup::PROTECTED.to_vec()), c["protected"]);
    let rows: Map<String, Value> = cleanup::PROTECTED_ROWS
        .iter()
        .map(|(t, r)| (t.to_string(), json!(r)))
        .collect();
    assert_eq!(Value::Object(rows), c["protected_rows"]);
}

// decisions: STORE-1, STORE-2, STORE-3, STORE-4
#[test]
fn a_home_is_checked_and_set_aside_as_python_does() {
    let fixture = fixture();
    let folder = TempDir::new().unwrap();
    let home: PathBuf = folder.path().canonicalize().unwrap().join("home");
    fs::create_dir(&home).unwrap();
    let home_text = home.to_string_lossy().into_owned();
    let clean = |value: &Value| -> Value {
        let text = serde_json::to_string(value).unwrap();
        let home_json = serde_json::to_string(&home_text).unwrap();
        serde_json::from_str(&text.replace(&home_json[1..home_json.len() - 1], "<HOME>")).unwrap()
    };
    let ticks = Cell::new(0u32);
    let now = || {
        ticks.set(ticks.get() + 1);
        DateTime::new(2026, 11, 1, 2, 15, ticks.get() % 60, 123_456, None).unwrap()
    };
    let toml: RefCell<Map<String, Value>> = RefCell::default();
    let toml_error = |text: &str| -> Option<String> {
        toml.borrow()
            .get(text)
            .map(|message| message.as_str().unwrap().to_owned())
    };
    let seams = salvage::Seams {
        now: &now,
        toml_error: &toml_error,
    };
    let listing = || -> Value {
        fn walk(folder: &Path, out: &mut Vec<PathBuf>) {
            for entry in fs::read_dir(folder).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    walk(&path, out);
                } else {
                    out.push(path);
                }
            }
        }
        let mut files = Vec::new();
        walk(&home, &mut files);
        files.sort();
        let mut out = Map::new();
        for path in files {
            let name = path
                .strip_prefix(&home)
                .unwrap()
                .to_string_lossy()
                .into_owned();
            let value = if name == salvage::PENDING || name == salvage::CHECKED {
                clean(&json!(fs::read_to_string(&path).unwrap()))
            } else {
                json!(fs::metadata(&path).unwrap().len())
            };
            out.insert(name, value);
        }
        Value::Object(out)
    };
    let entries = |list: Vec<salvage::Entry>| json!(list);
    let mut pending_before: Vec<salvage::Entry> = Vec::new();
    let mut moved: Vec<Vec<salvage::Entry>> = Vec::new();
    for (i, step) in fixture["salvage"].as_array().unwrap().iter().enumerate() {
        if let Some(name) = step.get("put").and_then(Value::as_str) {
            let path = home.join(name);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, unbase64(step["b64"].as_str().unwrap())).unwrap();
            continue;
        }
        if let Some(map) = step.get("toml") {
            *toml.borrow_mut() = map.as_object().unwrap().clone();
            continue;
        }
        let name = step["call"].as_str().unwrap();
        let a = &step["args"];
        let what = format!("salvage step {i}: {name} {a}");
        let build = || a["build"].as_str().unwrap();
        let got: ferb_store::Result<Value> = match name {
            "check_home" => salvage::check_home(&home, build(), &seams).map(|m| {
                moved.push(m.clone());
                entries(m)
            }),
            "check_store" => salvage::check_store(&home, build(), &seams).map(|m| {
                moved.push(m.clone());
                entries(m)
            }),
            "check_files" => salvage::check_files(&home, &seams).map(|m| {
                moved.push(m.clone());
                entries(m)
            }),
            "damage" => salvage::damage(&home.join(text(a, "name")), flag(a, "full"))
                .map(|found| json!(found)),
            "refuse_newer" => salvage::refuse_newer(&home).map(|()| Value::Null),
            "pending" => {
                pending_before = salvage::pending(&home);
                Ok(entries(pending_before.clone()))
            }
            "notice" => Ok(json!(pending_before
                .iter()
                .map(salvage::notice)
                .collect::<Vec<_>>())),
            "describe" => Ok(json!(salvage::describe(&pending_before))),
            "told" => {
                // the fixture told the moves of its 1st and 3rd set-aside calls, then the 2nd
                let set_aside: Vec<&Vec<salvage::Entry>> =
                    moved.iter().filter(|m| !m.is_empty()).collect();
                let files = set_aside
                    .iter()
                    .find(|m| m[0]["file"] != "ferb.db")
                    .unwrap();
                let which: Vec<salvage::Entry> = match text(a, "which") {
                    "first+third" => [set_aside[0].clone(), (*files).clone()].concat(),
                    "second" => set_aside[1].clone(),
                    _ => pending_before.clone(),
                };
                salvage::told(&home, &which).map(|()| Value::Null)
            }
            other => panic!("no such call: {other}"),
        };
        match (got, step.get("err")) {
            (Ok(got), None) => same(&clean(&got), &step["ret"], &what),
            (Err(error), Some(want)) => same(&clean(&raised(&error)), want, &what),
            (Ok(got), Some(want)) => panic!("{what}: Python raised {want}, Rust gave {got}"),
            (Err(error), None) => panic!("{what}: Rust failed: {error}"),
        }
        same(
            &listing(),
            &step["files"],
            &format!("{what}: files in the home"),
        );
    }
}

fn unbase64(text: &str) -> Vec<u8> {
    const TABLE: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = Vec::new();
    let (mut bits, mut count) = (0u32, 0);
    for byte in text.bytes().filter(|b| *b != b'=') {
        let value = TABLE.iter().position(|t| *t == byte).unwrap() as u32;
        bits = (bits << 6) | value;
        count += 6;
        if count >= 8 {
            count -= 8;
            out.push((bits >> count) as u8);
        }
    }
    out
}

#[test]
fn times_read_and_print_as_python_s_datetime() {
    for case in fixture()["python_says"]["times"].as_array().unwrap() {
        let text = case[0].as_str().unwrap();
        let got = match DateTime::from_isoformat(text) {
            Err(_) => Value::Null,
            Ok(when) => {
                let mut out = json!({
                    "iso": when.isoformat(Timespec::Auto),
                    "seconds": when.isoformat(Timespec::Seconds),
                    "minute": when.compact_minute(),
                    "aware": when.offset.is_some(),
                });
                if when.offset.is_some() {
                    out["utc"] = json!(when
                        .to_utc()
                        .ok()
                        .map(|utc| utc.isoformat(Timespec::Microseconds)));
                }
                out
            }
        };
        same(&got, &case[1], &format!("fromisoformat({text:?})"));
    }
}

#[test]
fn json_is_judged_and_written_as_python_s_json() {
    let says = &fixture()["python_says"];
    for case in says["json"].as_array().unwrap() {
        let text = case[0].as_str().unwrap();
        let got = json!(pyjson::check(text).err());
        same(&got, &case[1], &format!("json.loads({text:?})"));
    }
    for case in says["bytes"].as_array().unwrap() {
        let bytes: Vec<u8> = (0..case[0].as_str().unwrap().len() / 2)
            .map(|i| u8::from_str_radix(&case[0].as_str().unwrap()[2 * i..2 * i + 2], 16).unwrap())
            .collect();
        let got = json!(pyjson::decode_utf8(&bytes).err());
        same(&got, &case[1], &format!("decode({})", case[0]));
    }
    for case in says["dumps"].as_array().unwrap() {
        let value = &case[0];
        let style = |ascii, sort_keys, indent| Style {
            ascii,
            sort_keys,
            indent,
        };
        assert_eq!(json!(pyjson::dumps(value)), case[1]);
        assert_eq!(json!(pyjson::dumps_utf8(value)), case[2]);
        assert_eq!(
            json!(pyjson::dumps_with(value, style(true, true, None))),
            case[3]
        );
        assert_eq!(
            json!(pyjson::dumps_with(value, style(false, false, Some(2)))),
            case[4]
        );
    }
}
