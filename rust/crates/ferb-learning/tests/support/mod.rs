//! The test world `rust/fixtures/learning/make.py` drives Python in: a migrated store in a temp folder, a fake clock,
//! sequential ids, the fake redactor, a scripted model, and the daemon's wiring of memory's hooks; every event told is
//! recorded as make.py records it. `op` runs one step by name.

#![allow(dead_code)]

use std::cell::{Cell, RefCell};
use std::collections::{BTreeSet, VecDeque};
use std::path::{Path, PathBuf};
use std::rc::{Rc, Weak};

use serde_json::{json, Map, Value};
use tokio::sync::mpsc;

use ferb_context::PromptRoot;
use ferb_core::errors::FerbError;
use ferb_core::interfaces::{AgentRuntime, BoxFuture};
use ferb_core::types::{
    ModelLimits, ModelRequest, RunHandle, RuntimeEvent, RuntimeEventKind, Task,
};
use ferb_learning::calls::{Made, Spans};
use ferb_learning::distill::{Distiller, Notice};
use ferb_learning::import_job::{self, ImportJob};
use ferb_learning::items::{Item, Memory, Saved};
use ferb_learning::journal::{Entry, Journal};
use ferb_learning::learning::{self, Learning, Query};
use ferb_learning::profile::{CoreProfile, Entry as ProfileEntry};
use ferb_learning::store::{Sql, Store};
use ferb_learning::{py, sql, summary, Clock, DateTime, Error, Ids};
use ferb_memory::times::{FixedZone, LocalZone};
use ferb_store::Db;

pub const ZONE: i32 = 19_800; // TZ="FX-05:30"

pub fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../..")
        .canonicalize()
        .unwrap()
}

pub fn fixture(name: &str) -> Value {
    let path = repo().join("rust/fixtures/learning").join(name);
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

pub struct FakeClock(pub RefCell<DateTime>);
impl Clock for FakeClock {
    fn now(&self) -> DateTime {
        *self.0.borrow()
    }
}

pub struct SeqIds(pub Cell<u64>);
impl Ids for SeqIds {
    fn new_id(&self) -> String {
        self.0.set(self.0.get() + 1);
        format!("id-{:06}", self.0.get())
    }
}

/// The fake redactor: "SECRET-" words masked.
pub fn fake_redact(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(at) = rest.find("SECRET-") {
        let tail = &rest[at + 7..];
        let n = tail
            .bytes()
            .take_while(|b| b.is_ascii_alphanumeric())
            .count();
        if n == 0 {
            out.push_str(&rest[..at + 7]);
            rest = tail;
            continue;
        }
        out.push_str(&rest[..at]);
        out.push_str("[redacted]");
        rest = &tail[n..];
    }
    out.push_str(rest);
    out
}

pub fn redact_walk(v: &Value) -> Value {
    match v {
        Value::Object(m) => {
            Value::Object(m.iter().map(|(k, v)| (k.clone(), redact_walk(v))).collect())
        }
        Value::Array(a) => Value::Array(a.iter().map(redact_walk).collect()),
        Value::String(s) => Value::String(fake_redact(s)),
        o => o.clone(),
    }
}

pub type Events = std::sync::Arc<std::sync::Mutex<Vec<Value>>>;

fn ev(events: &Events, name: &str, data: Value) {
    events.lock().unwrap().push(json!([name, data]));
}

pub fn sha_schema(schema: &Value) -> String {
    py::sha256_hex(&ferb_py::json::dumps_with(
        schema,
        ferb_py::json::Style {
            ascii: true,
            sort_keys: true,
            indent: None,
        },
    ))
}

/// The scripted model of make.py's `Runtime`.
pub struct FakeRuntime {
    pub answer: std::sync::Mutex<Value>,
    pub events: Events,
}

impl AgentRuntime for FakeRuntime {
    fn name(&self) -> &str {
        "fake"
    }
    fn start<'a>(
        &'a self,
        task: &'a Task,
        request: &'a ModelRequest,
        run_token: &'a str,
    ) -> BoxFuture<'a, Result<RunHandle, FerbError>> {
        let role = serde_json::to_value(task.role).unwrap();
        let answer = {
            let mut a = self.answer.lock().unwrap();
            if a.get("dynamic").is_some() {
                *a = json!({"answer": dynamic_answer(&request.parts[0])});
            }
            a.clone()
        };
        ev(
            &self.events,
            "model.start",
            json!({"answer": answer, "task": task.id, "goal": task.goal, "parent": task.parent_id,
                   "request_id": task.request_id, "role": role, "device": task.device_id,
                   "system_sha": py::sha256_hex(&request.system), "parts": request.parts,
                   "cacheable": request.cacheable, "token": run_token}),
        );
        let id = task.id.clone();
        Box::pin(async move {
            Ok(RunHandle {
                id: "r".into(),
                runtime: "fake".into(),
                task_id: id,
                state: Map::new(),
            })
        })
    }
    fn events(&self, _run: &RunHandle) -> mpsc::Receiver<RuntimeEvent> {
        let (tx, rx) = mpsc::channel(4);
        let event = |kind, data: Value| RuntimeEvent {
            kind,
            data: data.as_object().cloned().unwrap(),
        };
        tx.try_send(event(
            RuntimeEventKind::Usage,
            json!({"input_tokens": 7, "output_tokens": 3}),
        ))
        .unwrap();
        let a = self.answer.lock().unwrap().clone();
        let last = if let Some(m) = a.get("error") {
            event(RuntimeEventKind::Error, json!({"message": m}))
        } else if let Some(m) = a.get("invalid") {
            event(RuntimeEventKind::InvalidAnswer, json!({"message": m}))
        } else if let Some(t) = a.get("text") {
            event(RuntimeEventKind::Final, json!({"text": t}))
        } else {
            event(
                RuntimeEventKind::Final,
                json!({"text": ferb_py::json::dumps(&a["answer"])}),
            )
        };
        tx.try_send(last).unwrap();
        rx
    }
    fn continue_run<'a>(
        &'a self,
        _run: &'a RunHandle,
        _m: &'a str,
    ) -> BoxFuture<'a, Result<(), FerbError>> {
        Box::pin(async { Ok(()) })
    }
    fn cancel<'a>(&'a self, _run: &'a RunHandle) -> BoxFuture<'a, ()> {
        ev(&self.events, "model.cancel", json!({}));
        Box::pin(async {})
    }
}

pub fn limits(tight: bool) -> ModelLimits {
    ModelLimits {
        model: "fake-model".into(),
        context_window: 1_000_000,
        max_output: 100_000,
        source: "test double".into(),
        window_includes_output: true,
        max_input_chars: tight.then_some(300),
    }
}

pub struct FakeSpans {
    pub n: Cell<u64>,
    pub events: Events,
}

impl Spans for FakeSpans {
    fn start(
        &self,
        kind: &str,
        task_id: Option<&str>,
        input: Value,
        parent_id: Option<&str>,
    ) -> String {
        self.n.set(self.n.get() + 1);
        let id = format!("span-{}", self.n.get());
        ev(
            &self.events,
            "span.start",
            json!({"id": id, "kind": kind, "task_id": task_id, "parent_id": parent_id, "input": input}),
        );
        id
    }
    fn finish(&self, id: &str, output: Option<Value>, error: Option<String>) {
        ev(
            &self.events,
            "span.finish",
            json!({"id": id, "output": output, "error": error}),
        );
    }
}

/// make.py's dynamic import answers: the recall answer from the chunk's lines, the verify answer from its context.
pub fn dynamic_answer(ctx: &str) -> Value {
    if ctx.contains("\nCandidate v") {
        let mut verdicts = Vec::new();
        let mut cand: Option<Map<String, Value>> = None;
        for line in ctx.split('\n') {
            if let Some(rest) = line.strip_prefix("Candidate ") {
                let (id, rest) = rest.split_once(": kind=").unwrap();
                let (kind, rest) = rest.split_once(" subject=").unwrap();
                let (subject, key) = rest.split_once(" key=").unwrap();
                let mut m = Map::new();
                for (k, v) in [
                    ("id", id),
                    ("kind", kind),
                    ("subject", subject),
                    ("key", key),
                ] {
                    m.insert(k.into(), json!(v));
                }
                cand = Some(m);
            } else if let Some(says) = line.strip_prefix("  says: ") {
                if let Some(c) = cand.as_mut() {
                    c.insert("content".into(), json!(says));
                }
            } else if let Some(q) = line.strip_prefix("  quoted: “") {
                if let Some(mut c) = cand.take() {
                    let (quote, rest) = q.rsplit_once("” (from ").unwrap();
                    let message = rest.strip_suffix(')').unwrap();
                    let key = c["key"].as_str().unwrap().to_owned();
                    let id = c["id"].as_str().unwrap().to_owned();
                    c.insert("supported".into(), json!(key != "work" || id != "v2"));
                    c.insert("message".into(), json!(message));
                    c.insert("quote".into(), json!(quote));
                    c.insert("same_as".into(), json!(""));
                    c.insert("ended".into(), json!(false));
                    c.insert(
                        "until".into(),
                        json!(if key == "work" { "2026-10-10" } else { "" }),
                    );
                    c.insert(
                        "review_in_days".into(),
                        json!(if key == "hobby" { 30 } else { 0 }),
                    );
                    verdicts.push(Value::Object(c));
                }
            }
        }
        return json!({"verdicts": verdicts});
    }
    let mut owners: Vec<(String, Vec<(String, String)>)> = Vec::new();
    for line in ctx.split('\n') {
        let Some(rest) = line.strip_prefix('[') else {
            continue;
        };
        let Some((head, text)) = rest.split_once("] ") else {
            continue;
        };
        let parts: Vec<&str> = head.split(" · ").collect();
        if parts.len() != 3 || parts[1] != "owner" || parts[0].contains(' ') {
            continue;
        }
        let tag = parts[0].rsplit_once('.').unwrap().0.to_owned();
        match owners.iter_mut().find(|(t, _)| *t == tag) {
            Some((_, v)) => v.push((parts[0].into(), text.into())),
            None => owners.push((tag, vec![(parts[0].into(), text.into())])),
        }
    }
    let mut out = Vec::new();
    for (_, refs) in &owners {
        let (r, first) = &refs[0];
        let city = if first.contains("Pune") {
            "Pune"
        } else {
            "Mumbai"
        };
        out.push(json!({"kind": "fact", "subject": "owner", "key": "city", "content": format!("lives in {city}"),
                        "message": r, "quote": format!("I live in {city}")}));
        out.push(json!({"kind": "person", "subject": "person:mira", "key": "work", "content": "Mira works at a bakery",
                        "message": format!("[{r}]"), "quote": "my sister Mira works at a bakery"}));
        out.push(json!({"kind": "preference", "subject": "owner", "key": "drink", "content": "prefers tea",
                        "message": r, "quote": "not in the message"}));
        if refs.len() > 1 {
            out.push(json!({"kind": "preference", "subject": "owner", "key": "hobby", "content": "learning the piano",
                            "message": refs[1].0, "quote": "I am learning the piano"}));
        }
    }
    out.push(json!({"kind": "fact", "subject": "owner", "key": "x", "content": "y", "message": "zz.1", "quote": "q"}));
    json!({"candidates": out})
}

struct LearningSeams;
impl learning::Seams for LearningSeams {
    fn episode(&self, store: &Store, task_id: &str) -> ferb_learning::Result<Option<Value>> {
        let Some(r) = store.one("SELECT * FROM episodes WHERE task_id = ?", sql![task_id])? else {
            return Ok(None);
        };
        let mut ep = Map::new();
        for (n, name) in r.names.iter().enumerate() {
            let v = ferb_learning::store::to_json(r.at(n));
            if let Some(field) = name.strip_suffix("_json") {
                ep.insert(field.into(), ferb_py::json::loads(&r.s_at(n)).unwrap());
            } else {
                ep.insert(name.clone(), v);
            }
        }
        for k in ["origins", "tool_origins"] {
            let set = ferb_sanitize::origin::split(ep[k].as_str());
            ep.insert(k.into(), json!(set.into_iter().collect::<Vec<_>>()));
        }
        ep.insert(
            "external_read".into(),
            json!(ep["external_read"].as_i64() == Some(1)),
        );
        let verified = match &ep["verified"] {
            Value::Null => Value::Null,
            v => json!(v.as_i64() != Some(0)),
        };
        ep.insert("verified".into(), verified);
        let how = store.one(
            "SELECT how FROM episode_how WHERE task_id = ?",
            sql![task_id],
        )?;
        ep.insert(
            "how".into(),
            json!(how.map_or_else(String::new, |h| h.s_at(0))),
        );
        Ok(Some(Value::Object(ep)))
    }
    fn trail_sources(&self, source: &str) -> Vec<(String, String)> {
        if [
            "accessibility",
            "typed_text",
            "clipboard",
            "notification",
            "software",
        ]
        .contains(&source)
        {
            vec![
                ("web".into(), "trail".into()),
                ("owner_account".into(), "trail|".into()),
            ]
        } else if ["browser_tab", "window"].contains(&source) {
            vec![("web".into(), "trail".into())]
        } else {
            vec![("app".into(), "trail".into())]
        }
    }
}

/// ferb/policy/security_digest.py's `build`, read here as the summary's seam.
pub struct SummarySeams;
impl summary::Seams for SummarySeams {
    fn test_task(&self, _store: &Store, _task_id: &str) -> ferb_learning::Result<bool> {
        Ok(false)
    }
    fn security_digest(&self, store: &Store, day: &str) -> ferb_learning::Result<Value> {
        let rows = |q: &str| -> ferb_learning::Result<Vec<Value>> {
            Ok(store
                .query(q, sql![day])?
                .iter()
                .map(|r| {
                    Value::Object(
                        r.names
                            .iter()
                            .zip(r.json())
                            .map(|(k, v)| (k.clone(), v))
                            .collect(),
                    )
                })
                .collect())
        };
        let destinations =
            rows("SELECT kind, value, how FROM known_destinations WHERE substr(at,1,10)=?")?;
        let confirmations =
            rows("SELECT rule, granted, used FROM confirmations WHERE substr(at,1,10)=?")?;
        let (mut blocked, mut standing) = (Vec::new(), Vec::new());
        for r in store.query(
            "SELECT id, kind, input_json, output_json, error FROM spans WHERE substr(started_at,1,10)=? \
             AND kind IN ('tool.refused','tool.call')",
            sql![day],
        )? {
            let inp = ferb_py::json::loads(&r.opt("input_json").unwrap_or_else(|| "{}".into())).unwrap();
            let out = ferb_py::json::loads(&r.opt("output_json").filter(|s| !s.is_empty()).unwrap_or_else(|| "{}".into())).unwrap();
            if r.s("kind") == "tool.call" {
                if let Some(Value::Object(c)) = inp.get("standing") {
                    standing.push(json!({"span": r.s("id"), "tool": inp.get("tool"), "target": inp.get("target"),
                        "declared": inp.get("declared"), "standing_task": c.get("task"), "rules": c.get("rules"),
                        "error": r.opt("error")}));
                }
            }
            if let Some(Value::Object(net)) = out.get("network") {
                for refusal in net.get("refused").and_then(Value::as_array).into_iter().flatten() {
                    blocked.push(json!({"span": r.s("id"), "network": refusal}));
                }
            }
            if r.s("kind") == "tool.refused" {
                blocked.push(json!({"span": r.s("id"), "tool": inp.get("tool"), "reason": r.opt("error")}));
            }
        }
        let memory = rows(
            "SELECT id, origin, status FROM memory_items WHERE substr(created_at,1,10)=? \
             AND (origin='third_party' OR id IN (SELECT ms.item_id FROM memory_sources ms \
             LEFT JOIN trail t ON ms.kind='trail' AND ms.source_id=t.id \
             LEFT JOIN journal j ON ms.kind='journal' AND ms.source_id=j.id \
             WHERE t.source IN ('window','browser_tab','email','message','notification') OR COALESCE(j.origin,'') LIKE '%external:%'))",
        )?;
        Ok(
            json!({"new_destinations": destinations, "confirmations": confirmations, "blocked_flows": blocked,
                  "standing_sends": standing, "outside_origin_memory": memory,
                  "meaning": "recorded facts; missing observations are not proof of no activity"}),
        )
    }
}

struct ImportSeams {
    store: Rc<Store>,
    busy: Rc<Cell<i64>>,
}
impl import_job::Seams for ImportSeams {
    fn similar(&self, query: &str) -> Vec<String> {
        let words: BTreeSet<String> = ferb_learning::text::query_terms(query)
            .into_iter()
            .collect();
        self.store
            .query("SELECT id, content FROM memory_items ORDER BY seq", sql![])
            .unwrap()
            .iter()
            .filter(|r| {
                ferb_learning::text::query_terms(&r.s_at(1))
                    .iter()
                    .any(|t| words.contains(t))
            })
            .map(|r| r.s_at(0))
            .collect()
    }
    fn busy(&self) -> bool {
        if self.busy.get() > 0 {
            self.busy.set(self.busy.get() - 1);
            return true;
        }
        false
    }
    fn sleep(&self, _seconds: f64) -> BoxFuture<'static, ()> {
        Box::pin(async {})
    }
}

pub struct Env {
    pub root: PathBuf,
    pub store: Rc<Store>,
    pub clock: Rc<FakeClock>,
    pub ids: Rc<SeqIds>,
    pub events: Events,
    pub answers: Rc<RefCell<VecDeque<Value>>>,
    pub busy: Rc<Cell<i64>>,
    pub journal: Rc<Journal>,
    pub memory: Rc<Memory>,
    pub learning: Rc<Learning>,
    pub profile: Rc<CoreProfile>,
    pub prompts: Rc<PromptRoot>,
    pub spans: Rc<FakeSpans>,
    pub prepared: RefCell<Option<Value>>,
    pub writes: Rc<Cell<u64>>,
    pub fail_at: Rc<Cell<Option<u64>>>,
    pub importer_gen: Cell<u64>,
    _tmp: Option<tempfile::TempDir>,
}

pub fn make_runtime_for(
    events: Events,
    answers: Rc<RefCell<VecDeque<Value>>>,
) -> impl Fn(&str, &str, &[String], &Value) -> ferb_learning::Result<Made> {
    move |role, mode, tools, schema| {
        let answer = answers.borrow_mut().pop_front();
        ev(
            &events,
            "make_runtime",
            json!({"role": role, "mode": mode, "tools": tools, "schema": sha_schema(schema)}),
        );
        let Some(mut answer) = answer else {
            return Err(Error::Runtime("no scripted answer left".into()));
        };
        let tight = answer.get("tight") == Some(&json!(true));
        if answer.get("dynamic").is_some() {
            answer = json!({"dynamic": "import"});
        }
        Ok(Made {
            rt: Box::new(FakeRuntime {
                answer: std::sync::Mutex::new(answer),
                events: events.clone(),
            }),
            limits: Some(limits(tight)),
            model: Some("fake-model-1".into()),
        })
    }
}

impl Env {
    pub fn new(name: &str, root: Option<&Path>) -> Rc<Env> {
        let (tmp, root) = match root {
            Some(r) => (None, r.to_path_buf()),
            None => {
                let t = tempfile::tempdir().unwrap();
                let p = t.path().to_path_buf();
                (Some(t), p)
            }
        };
        let folder = root.join(name);
        std::fs::create_dir_all(&folder).unwrap();
        let db = Db::open(folder.join("ferb.db")).unwrap();
        db.migrate().unwrap();
        let store = Store::new(Rc::new(db));
        let events: Events = Default::default();
        let writes = Rc::new(Cell::new(0u64));
        let fail_at: Rc<Cell<Option<u64>>> = Rc::new(Cell::new(None));
        {
            let (w, f) = (writes.clone(), fail_at.clone());
            store.set_fault(Some(Box::new(move |_sql| {
                w.set(w.get() + 1);
                f.get() == Some(w.get())
            })));
        }
        let clock = Rc::new(FakeClock(RefCell::new(
            py::parse("2026-10-20T09:00:00+00:00").unwrap(),
        )));
        let ids = Rc::new(SeqIds(Cell::new(0)));
        let zone: Rc<dyn LocalZone> = Rc::new(FixedZone(ZONE));
        let journal = Rc::new(Journal::new(
            store.clone(),
            clock.clone(),
            ids.clone(),
            Box::new(redact_walk),
        ));
        let publish = |events: &Events| {
            let e = events.clone();
            Box::new(move |kind: &str, data: Value| {
                ev(&e, "publish", json!({"kind": kind, "data": data}))
            })
        };
        let memory = Rc::new(Memory::new(
            store.clone(),
            clock.clone(),
            ids.clone(),
            zone.clone(),
            publish(&events),
            Box::new(fake_redact),
        ));
        let mut l = Learning::new(
            store.clone(),
            clock.clone(),
            zone.clone(),
            publish(&events),
            Box::new(fake_redact),
            Box::new(LearningSeams),
        );
        l.secret_values = Box::new(|| vec!["zebra-quilt-4417".to_owned()]);
        let learning = Rc::new(l);
        let j2 = journal.clone();
        let profile = Rc::new(CoreProfile::new(
            memory.clone(),
            Box::new(move |k, d| j2.append(k, Some(d), None).map(|_| ())),
        ));
        {
            let e = events.clone();
            *profile.on_change.borrow_mut() =
                Box::new(move || ev(&e, "profile.changed", json!({})));
            let e = events.clone();
            *profile.on_joined.borrow_mut() = Box::new(move |items: &[Item]| {
                let mut ids: Vec<&String> = items.iter().map(|i| &i.id).collect();
                ids.sort();
                ev(&e, "profile.joined", json!({"ids": ids}));
            });
        }
        {
            let mut h = memory.hooks.borrow_mut();
            let e = events.clone();
            h.on_change = Box::new(move || ev(&e, "memory.changed", json!({})));
            let (e, lw, mw): (Events, Weak<Learning>, Weak<Memory>) = (
                events.clone(),
                Rc::downgrade(&learning),
                Rc::downgrade(&memory),
            );
            h.on_forget = Box::new(move |plan| {
                ev(
                    &e,
                    "memory.on_forget",
                    json!({"items": plan.items, "trail": plan.trail}),
                );
                let (l, m) = (lw.upgrade().unwrap(), mw.upgrade().unwrap());
                for id in &plan.items {
                    l.drop_fact_index(&format!("memory:{id}"), &m)?;
                }
                Ok(())
            });
            let e = events.clone();
            h.on_settled = Box::new(move |ids, deleted| {
                ev(
                    &e,
                    "memory.settled",
                    json!({"ids": ids, "deleted": deleted}),
                )
            });
            let (e, pw) = (events.clone(), Rc::downgrade(&profile));
            h.on_ended = Box::new(move |ids, new| {
                ev(&e, "memory.ended", json!({"ids": ids, "new": new}));
                pw.upgrade().unwrap().follow(ids, new)
            });
            let (e, pw) = (events.clone(), Rc::downgrade(&profile));
            h.on_saved = Box::new(move |it| {
                ev(&e, "memory.saved", json!({"id": it.id}));
                pw.upgrade().unwrap().saved(it)
            });
            let (e, pw) = (events.clone(), Rc::downgrade(&profile));
            h.on_set_aside = Box::new(move |ids| {
                ev(&e, "memory.set_aside", json!({"ids": ids}));
                pw.upgrade().unwrap().remove(ids, "code")
            });
            let (s, j, z) = (store.clone(), journal.clone(), zone.clone());
            h.rebuild_summary =
                Box::new(move |day| summary::rebuild(&s, &j, &SummarySeams, z.as_ref(), day));
        }
        let prompts = Rc::new(PromptRoot::new(repo().join("ferb")).unwrap());
        let spans = Rc::new(FakeSpans {
            n: Cell::new(0),
            events: events.clone(),
        });
        Rc::new(Env {
            root,
            store,
            clock,
            ids,
            events,
            answers: Rc::new(RefCell::new(VecDeque::new())),
            busy: Rc::new(Cell::new(0)),
            journal,
            memory,
            learning,
            profile,
            prompts,
            spans,
            prepared: RefCell::new(None),
            writes,
            fail_at,
            importer_gen: Cell::new(0),
            _tmp: tmp,
        })
    }

    pub fn distiller(&self) -> Distiller<'static> {
        let e = self.events.clone();
        let e2 = self.events.clone();
        let e3 = self.events.clone();
        Distiller {
            store: self.store.clone(),
            journal: self.journal.clone(),
            memory: self.memory.clone(),
            make_runtime: Box::new(make_runtime_for(self.events.clone(), self.answers.clone())),
            clock: self.clock.clone(),
            zone: self.memory.zone.clone(),
            prompts: self.prompts.clone(),
            summary: Rc::new(SummarySeams),
            purge: Box::new(move || ev(&e, "purge", json!({}))),
            device_id: "dev-1".into(),
            record_usage: Box::new(move |n, d| ev(&e2, "usage", json!({"runtime": n, "data": d}))),
            spans: Some(self.spans.clone()),
            tell: Box::new(move |n: Notice| ev(&e3, "tell", notice_json(&n))),
        }
    }

    pub fn importer(&self) -> ImportJob<'static> {
        let (e1, e2, e3) = (
            self.events.clone(),
            self.events.clone(),
            self.events.clone(),
        );
        ImportJob {
            memory: self.memory.clone(),
            store: self.store.clone(),
            journal: self.journal.clone(),
            make_runtime: Box::new(make_runtime_for(self.events.clone(), self.answers.clone())),
            prompts: self.prompts.clone(),
            redact: Box::new(fake_redact),
            publish: Box::new(move |k, d| ev(&e1, "publish", json!({"kind": k, "data": d}))),
            tell: Box::new(move |n: Notice| ev(&e2, "tell", notice_json(&n))),
            seams: Box::new(ImportSeams {
                store: self.store.clone(),
                busy: self.busy.clone(),
            }),
            ids: self.ids.clone(),
            spans: Some(self.spans.clone()),
            usage: Box::new(move |n, d| ev(&e3, "usage", json!({"runtime": n, "data": d}))),
            device_id: "dev-1".into(),
            home: Some(self.root.join("user")),
            unzstd: None,
            idle_poll: 0.0,
            chunk_chars: ferb_memory::import_extract::CHUNK_CHARS,
        }
    }
}

fn notice_json(n: &Notice) -> Value {
    let mut v = json!({"id": n.id, "refs": n.refs, "text": n.text, "origin": n.origin});
    if let Some(c) = &n.cards {
        v["cards"] = c.clone();
    }
    v
}

pub const TABLES: [&str; 23] = [
    "memory_items",
    "memory_sources",
    "memory_context",
    "memory_set_aside",
    "memory_forgotten",
    "memory_profile",
    "known_destinations",
    "trail",
    "journal",
    "learning_nodes",
    "learning_sources",
    "learning_changes",
    "forgotten_refs",
    "learning_card_inputs",
    "learning_digests",
    "learning_search_version",
    "learning_fts",
    "learning_titles",
    "distill_runs",
    "memory_import_runs",
    "memory_import_done",
    "memory_import_items",
    "memory_questions",
];

pub fn dump(env: &Env) -> Map<String, Value> {
    let mut out = Map::new();
    for t in TABLES {
        let rows: Vec<Value> = env
            .store
            .query(&format!("SELECT * FROM {t} ORDER BY rowid"), sql![])
            .unwrap()
            .iter()
            .map(|r| Value::Array(r.json()))
            .collect();
        out.insert(t.into(), Value::Array(rows));
    }
    let idx = env
        .store
        .exists(
            "SELECT 1 FROM sqlite_master WHERE name = 'learning_source_ref'",
            sql![],
        )
        .unwrap();
    out.insert("source_ref_index".into(), json!([[idx]]));
    out
}

pub fn item_json(it: &Item) -> Value {
    it.json()
}

fn items(v: &[Item]) -> Value {
    Value::Array(v.iter().map(Item::json).collect())
}

fn entries(v: &[Entry]) -> Value {
    Value::Array(v.iter().map(Entry::json).collect())
}

fn saved_json(s: &Saved) -> Value {
    json!({"item": s.item.json(), "replaced": items(&s.replaced), "not_replaced": s.not_replaced})
}

fn profile_entries(v: &[ProfileEntry]) -> Value {
    Value::Array(
        v.iter()
            .map(|e| json!({"section": e.section, "position": e.position, "item": e.item.id, "added_by": e.added_by, "seq": e.seq}))
            .collect(),
    )
}

fn st(a: &Value, k: &str) -> String {
    a.get(k).and_then(Value::as_str).unwrap_or("").to_owned()
}

fn opt(a: &Value, k: &str) -> Option<String> {
    a.get(k).and_then(Value::as_str).map(str::to_owned)
}

fn strings(v: Option<&Value>) -> Vec<String> {
    v.and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|x| x.as_str().unwrap_or("").to_owned())
        .collect()
}

fn when(a: &Value, k: &str) -> Option<DateTime> {
    opt(a, k)
        .filter(|s| !s.is_empty())
        .map(|s| py::parse(&s).unwrap())
}

fn sorted(set: impl IntoIterator<Item = String>) -> Value {
    let mut v: Vec<String> = set.into_iter().collect();
    v.sort();
    json!(v)
}

fn stopper(n: i64) -> impl FnMut() -> bool {
    let mut left = n;
    move || {
        if left == 0 {
            return true;
        }
        left -= 1;
        false
    }
}

fn sql_value(v: &Value) -> Sql {
    match v {
        Value::Null => Sql::Null,
        Value::Bool(b) => Sql::Integer(i64::from(*b)),
        Value::Number(n) => n
            .as_i64()
            .map_or_else(|| Sql::Real(n.as_f64().unwrap()), Sql::Integer),
        Value::String(s) => Sql::Text(s.clone()),
        other => Sql::Text(other.to_string()),
    }
}

pub async fn op(env: &Env, name: &str, a: &Value) -> ferb_learning::Result<Value> {
    let m = &env.memory;
    let l = &env.learning;
    let p = &env.profile;
    let none = Ok(Value::Null);
    match name {
        "clock" => {
            *env.clock.0.borrow_mut() = py::parse(&st(a, "at")).unwrap();
            none
        }
        "sql" => {
            let params: Vec<Sql> = a
                .get("params")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .map(sql_value)
                .collect();
            env.store
                .conn()
                .execute(&st(a, "sql"), rusqlite::params_from_iter(params))
                .unwrap();
            none
        }
        "answers" => none,
        "busy" => {
            env.busy.set(a["polls"].as_i64().unwrap());
            none
        }
        "journal.append" => {
            let data = a.get("data").filter(|d| !d.is_null()).cloned();
            Ok(env
                .journal
                .append(&st(a, "kind"), data, opt(a, "origin").as_deref())?
                .json())
        }
        "journal.entries" => Ok(entries(&env.journal.entries(
            opt(a, "since").as_deref(),
            a.get("limit").and_then(Value::as_i64),
        )?)),
        "journal.entries_of" => {
            let kinds = strings(a.get("kinds"));
            Ok(entries(&env.journal.entries_of(
                &kinds.iter().map(String::as_str).collect::<Vec<_>>(),
            )?))
        }
        "journal.kinds_after" => {
            let kinds = strings(a.get("kinds"));
            Ok(entries(&env.journal.kinds_after(
                &kinds.iter().map(String::as_str).collect::<Vec<_>>(),
                &st(a, "id"),
            )?))
        }
        "journal.of_kind" => Ok(entries(&env.journal.of_kind(
            &st(a, "kind"),
            &st(a, "field"),
            &st(a, "equals"),
            opt(a, "missing").as_deref(),
        )?)),
        "journal.recent_values" => Ok(json!(env.journal.recent_values(
            &st(a, "kind"),
            &st(a, "field"),
            a["limit"].as_i64().unwrap()
        )?)),
        "journal.by_ids" => Ok(entries(&env.journal.by_ids(&strings(a.get("ids")))?)),
        "journal.latest_kind" => {
            let kinds = strings(a.get("kinds"));
            Ok(json!(env.journal.latest_kind(
                &kinds.iter().map(String::as_str).collect::<Vec<_>>(),
                &st(a, "field"),
                &st(a, "equals")
            )?))
        }
        "journal.of_running_tasks" => Ok(entries(&env.journal.of_running_tasks(&st(a, "kind"))?)),
        "memory.save" => {
            let sources: Vec<(String, String)> = a["sources"]
                .as_array()
                .unwrap()
                .iter()
                .map(|s| {
                    (
                        s[0].as_str().unwrap().to_owned(),
                        s[1].as_str().unwrap().to_owned(),
                    )
                })
                .collect();
            let saved = m.save(
                &st(a, "kind"),
                &st(a, "subject"),
                &st(a, "key"),
                &st(a, "content"),
                &st(a, "origin"),
                &sources,
                a.get("confidence").and_then(Value::as_f64).unwrap_or(0.7),
                &strings(a.get("replaces")),
                &st(a, "about"),
                a.get("from_owner").and_then(Value::as_bool),
            )?;
            Ok(saved_json(&saved))
        }
        "memory.get" => Ok(m.get(&st(a, "id"))?.map_or(Value::Null, |i| i.json())),
        "memory.items" => {
            let status = match a.get("status") {
                Some(Value::Null) => None,
                Some(v) => Some(v.as_str().unwrap().to_owned()),
                None => Some("active".to_owned()),
            };
            Ok(items(&m.items(
                status.as_deref(),
                opt(a, "subject").as_deref(),
                a.get("valid_now").and_then(Value::as_bool).unwrap_or(true),
            )?))
        }
        "memory.current_items" => Ok(items(&m.current_items()?)),
        "memory.history" => Ok(items(&m.history(&st(a, "id"))?)),
        "memory.telling_words" => Ok(json!(
            m.telling_words(&st(a, "text"), when(a, "at").as_ref())?
        )),
        "memory.current_ids" => Ok(sorted(m.current_ids(opt(a, "origin").as_deref())?)),
        "memory.evidence_days" => Ok(json!(m.evidence_days(&st(a, "id"))?)),
        "memory.from_mac_scan" => Ok(json!(m.from_mac_scan(&st(a, "id"))?)),
        "memory.commit" => {
            m.commit(&strings(a.get("ids")), &st(a, "reason"))?;
            none
        }
        "memory.set_about" => {
            m.set_about(&st(a, "id"), &st(a, "about"))?;
            none
        }
        "memory.set_review" => {
            m.set_review(&st(a, "id"), when(a, "at").as_ref())?;
            none
        }
        "memory.due_for_review" => Ok(items(&m.due_for_review()?)),
        "memory.retract" => {
            m.retract(
                &st(a, "id"),
                &st(a, "reason"),
                a.get("never_again")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
            )?;
            none
        }
        "memory.expire_at" => {
            m.expire_at(&st(a, "id"), &when(a, "at").unwrap())?;
            none
        }
        "memory.retire" => {
            m.retire(&st(a, "id"), &st(a, "reason"))?;
            none
        }
        "memory.looks_over" => Ok(json!(m.looks_over(&st(a, "id"), &st(a, "why"))?)),
        "memory.restore" => Ok(json!(m.restore(&st(a, "id"))?)),
        "memory.edit" => {
            let s = &a["source"];
            Ok(m.edit(
                &st(a, "id"),
                &st(a, "content"),
                &(s[0].as_str().unwrap().into(), s[1].as_str().unwrap().into()),
            )?
            .json())
        }
        "memory.plan_forget" | "memory.forget" => {
            let keep = a
                .get("keep_sources")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let plan = if name == "memory.plan_forget" {
                m.plan_forget(&st(a, "id"), keep)?
            } else {
                m.forget(&st(a, "id"), keep)?
            };
            Ok(
                json!({"items": plan.items, "trail": plan.trail, "rebuild_days": plan.rebuild_days,
                      "delete_summaries": plan.delete_summaries, "kept": plan.kept,
                      "destinations": plan.destinations.iter().map(|(k, v)| json!([k, v])).collect::<Vec<_>>(),
                      "describe": plan.describe(&env.store, false)?, "plain": plan.describe(&env.store, true)?,
                      "keeps": plan.keeps()}),
            )
        }
        "memory.forgets_on_record" => {
            Ok(json!(ferb_learning::items::forgets_on_record(&env.store)?))
        }
        "export" => {
            let folder = env.root.join(env_name(env)).join("export");
            let written = ferb_learning::export::export(m, &folder)?;
            let mut out = Map::new();
            for w in &written {
                out.insert(
                    w.file_name().unwrap().to_string_lossy().into(),
                    json!(std::fs::read_to_string(w).unwrap()),
                );
            }
            let mut files: Vec<String> = std::fs::read_dir(&folder)
                .unwrap()
                .map(|e| e.unwrap().file_name().to_string_lossy().into())
                .collect();
            files.sort();
            out.insert("_files".into(), json!(files));
            Ok(Value::Object(out))
        }
        "profile.entries" => Ok(profile_entries(&p.entries()?)),
        "profile.render" => {
            let (t, n) = p.render()?;
            Ok(json!([t, n]))
        }
        "profile.prompt_text" => Ok(json!(p.prompt_text()?)),
        "profile.pending_text" => Ok(json!(p.pending_text()?)),
        "profile.settle" => {
            p.settle(&st(a, "why"))?;
            none
        }
        "profile.used_tokens" => Ok(json!(p.used_tokens()?)),
        "profile.view" => p.view(),
        "profile.due" => Ok(json!(p.due()?)),
        "profile.holds_up" => {
            let it = m.get(&st(a, "id"))?.ok_or_else(|| {
                Error::Crash(
                    "AttributeError",
                    "'NoneType' object has no attribute 'status'".into(),
                )
            })?;
            Ok(json!(p.holds_up(&it, None)?))
        }
        "profile.eligible" => Ok(json!(p
            .eligible()?
            .iter()
            .map(|i| i.id.clone())
            .collect::<Vec<_>>())),
        "profile.add" => {
            p.add(
                &st(a, "id"),
                &st(a, "section"),
                &opt(a, "by").unwrap_or_else(|| "owner".into()),
            )?;
            none
        }
        "profile.remove" => {
            p.remove(
                &strings(a.get("ids")),
                &opt(a, "by").unwrap_or_else(|| "code".into()),
            )?;
            none
        }
        "profile.seed" => Ok(json!(p.seed()?)),
        "profile.pass_context" => Ok(json!(p.pass_context()?)),
        "profile.apply" => p.apply(&a["answer"]),
        "profile.run_pass" => {
            let mk = make_runtime_for(env.events.clone(), env.answers.clone());
            let e = env.events.clone();
            let usage = move |n: &str, d: &Value| ev(&e, "usage", json!({"runtime": n, "data": d}));
            p.run_pass(&mk, &env.prompts, "dev-1", Some(env.spans.as_ref()), &usage)
                .await
        }
        "learning.put" => {
            let labels = a
                .get("labels")
                .map(|l| strings(Some(l)).into_iter().collect::<BTreeSet<_>>());
            l.put(
                &st(a, "kind"),
                &st(a, "key"),
                &a["body"],
                &strings(a.get("sources")),
                &opt(a, "origin").unwrap_or_else(|| "inferred".into()),
                &opt(a, "made_by").unwrap_or_else(|| "test:1".into()),
                opt(a, "batch").as_deref(),
                &opt(a, "state").unwrap_or_else(|| "candidate".into()),
                a.get("owner").and_then(Value::as_bool).unwrap_or(false),
                labels,
                a.get("supersede").and_then(Value::as_bool).unwrap_or(true),
            )
        }
        "learning.get" => Ok(l
            .get(
                &st(a, "id"),
                a.get("sources").and_then(Value::as_bool).unwrap_or(true),
            )?
            .unwrap_or(Value::Null)),
        "learning.items" => {
            let kinds = strings(a.get("kinds"));
            Ok(json!(l.items(
                opt(a, "kind").as_deref(),
                a.get("history").and_then(Value::as_bool).unwrap_or(false),
                a.get("sources").and_then(Value::as_bool).unwrap_or(true),
                &kinds.iter().map(String::as_str).collect::<Vec<_>>()
            )?))
        }
        "learning.versions" => Ok(json!(l.versions(&st(a, "kind"), &st(a, "key"))?)),
        "learning.labels" => Ok(sorted(l.labels(&strings(a.get("sources")))?)),
        "learning.forget" => Ok(json!(l.forget(&strings(a.get("refs")))?)),
        "learning.drop_fact_index" => Ok(json!(l.drop_fact_index(&st(a, "ref"), m)?)),
        "learning.rollback" => Ok(json!(l.rollback(&st(a, "batch"))?)),
        "learning.confirm" => l.confirm(&st(a, "id")),
        "learning.edit" => l.edit(&st(a, "id"), &st(a, "text")),
        "learning.retire" => Ok(l.retire(&st(a, "id"))?.unwrap_or(Value::Null)),
        "learning.digest" => Ok(l.digest()?.unwrap_or(Value::Null)),
        "learning.identity" => Ok(json!(l.identity()?)),
        "learning.descendants" => Ok(sorted(
            l.descendants(&strings(a.get("refs")).into_iter().collect())?,
        )),
        "learning.forgotten" => Ok(json!(l.forgotten(&st(a, "ref"))?)),
        "learning.search" => {
            let leave = strings(a.get("leave_out"));
            let q = Query {
                kind: opt(a, "kind")
                    .as_deref()
                    .map(|_| a["kind"].as_str().unwrap()),
                app: a.get("app").and_then(Value::as_str),
                person: a.get("person").and_then(Value::as_str),
                since: a.get("since").and_then(Value::as_str),
                until: a.get("until").and_then(Value::as_str),
                limit: a.get("limit").and_then(Value::as_u64).map(|n| n as usize),
                leave_out: leave.iter().map(String::as_str).collect(),
                no_sources: !a.get("sources").and_then(Value::as_bool).unwrap_or(true),
                focus: a.get("focus").and_then(Value::as_str).unwrap_or(""),
                ..Query::default()
            };
            Ok(json!(l.search(
                a.get("query").and_then(Value::as_str).unwrap_or(""),
                &q
            )?))
        }
        "learning.weighted_terms" => Ok(json!(l.weighted_terms(&strings(a.get("words")))?)),
        "learning.reindex_search" => {
            let mut stop = stopper(a.get("stop_after").and_then(Value::as_i64).unwrap_or(-1));
            let bytes = a
                .get("batch_bytes")
                .and_then(Value::as_u64)
                .map_or(ferb_learning::notes::REINDEX_BATCH_BYTES, |n| n as usize);
            Ok(json!(learning::reindex_search(
                &env.store, bytes, &mut stop
            )?))
        }
        "learning.search_indexed" => Ok(json!(learning::search_indexed(&env.store)?)),
        "learning.prepare_episode" => {
            let apps = a.get("apps").and_then(Value::as_object).cloned();
            let got = l.prepare_episode(&st(a, "task_id"), apps.as_ref())?;
            *env.prepared.borrow_mut() = got.clone();
            Ok(got.unwrap_or(Value::Null))
        }
        "learning.write_episode" => {
            let prepared = env.prepared.borrow().clone();
            Ok(l.write_episode(prepared.as_ref())?.unwrap_or(Value::Null))
        }
        "learning.capture_episode" => {
            let apps = a.get("apps").and_then(Value::as_object).cloned();
            Ok(l.capture_episode(&st(a, "task_id"), apps.as_ref())?
                .unwrap_or(Value::Null))
        }
        "learning.card_inputs" => Ok(json!(l.card_inputs(opt(a, "task_id").as_deref())?)),
        "learning.cards_to_read" => Ok(json!(l.cards_to_read()?)),
        "learning.card_read" => Ok(json!(l.card_read(
            &st(a, "task_id"),
            &st(a, "inputs"),
            opt(a, "error").as_deref()
        )?)),
        "learning.stuck_cards" => Ok(json!(l.stuck_cards()?)),
        "learning.card_source_refs" => Ok(l
            .card_source_refs(&st(a, "task_id"))?
            .map_or(Value::Null, sorted)),
        "learning.card_source_plan" => {
            l.card_source_plan(&mut || false, &strings(a.get("removing")))
        }
        "learning.prune_card_sources" => {
            let mut stop = stopper(a.get("stop_after").and_then(Value::as_i64).unwrap_or(-1));
            let told = RefCell::new(Vec::new());
            let counts = l.prune_card_sources(
                &mut stop,
                &mut |d, t| told.borrow_mut().push(json!([d, t])),
                &mut || {
                    told.borrow_mut().push(json!("rebuilding"));
                    Ok(())
                },
                None,
            )?;
            Ok(json!({"counts": counts, "told": told.into_inner()}))
        }
        "learning.index_summaries" => {
            learning::index_summaries(l, &SummarySeams, &mut || false)?;
            none
        }
        "learning.current_card" => Ok(l
            .current_card(
                &st(a, "task_id"),
                a.get("sources").and_then(Value::as_bool).unwrap_or(true),
            )?
            .unwrap_or(Value::Null)),
        "learning.generation" => Ok(json!(l.generation.get())),
        "learning.shown" => {
            let node = l.get(&st(a, "id"), false)?.ok_or_else(|| {
                Error::Crash("TypeError", "'NoneType' object is not subscriptable".into())
            })?;
            let (kind, key) = (
                node["kind"].as_str().unwrap(),
                node["key"].as_str().unwrap(),
            );
            Ok(
                json!({"text": ferb_learning::notes::shown_text(kind, key, &node["body"], node["recorded_at"].as_str().unwrap()),
                      "state": ferb_learning::notes::shown_state(kind, &node["body"], node["state"].as_str().unwrap())}),
            )
        }
        "summary.build" => {
            let ev = summary::build(&env.store, &SummarySeams, m.zone.as_ref(), &st(a, "day"))?;
            Ok(evidence_json(&ev))
        }
        "summary.rebuild" => Ok(json!(summary::rebuild(
            &env.store,
            &env.journal,
            &SummarySeams,
            m.zone.as_ref(),
            &st(a, "day")
        )?)),
        "summary.summarized" => Ok(json!(summary::summarized(&env.store, &st(a, "day"))?)),
        "distill.pending_days" => Ok(json!(env.distiller().pending_days()?)),
        "distill.run_day" => env.distiller().run_day(&st(a, "day")).await,
        "distill.catch_up" => Ok(json!(env.distiller().catch_up().await?)),
        "distill.yesterday" => Ok(json!(ferb_learning::distill::yesterday(
            &env.clock.now(),
            m.zone.as_ref()
        ))),
        "playbooks.candidates" => {
            let emb = FakeEmbedder;
            Ok(json!(ferb_learning::playbooks::candidates(l, Some(&emb))?
                .iter()
                .map(|(b, g)| json!([
                    b.as_ref().map(|b| b["id"].clone()),
                    g.iter().map(|c| c["key"].clone()).collect::<Vec<_>>()
                ]))
                .collect::<Vec<_>>()))
        }
        "playbooks.merge" => {
            let emb = FakeEmbedder;
            let mk = make_runtime_for(env.events.clone(), env.answers.clone());
            let j = env.journal.clone();
            let record = move |k: &str, d: Value| j.append(k, Some(d), None).map(|_| ());
            let e = env.events.clone();
            let usage = move |n: &str, d: &Value| ev(&e, "usage", json!({"runtime": n, "data": d}));
            Ok(json!(
                ferb_learning::playbooks::merge(
                    l,
                    &mk,
                    &env.prompts,
                    Some(&emb),
                    "dev-1",
                    Some(env.spans.as_ref()),
                    &record,
                    &usage
                )
                .await?
            ))
        }
        "playbooks.repeat_stats" => Ok(json!(ferb_learning::playbooks::repeat_stats(&env.store)?)),
        "playbooks.verified_cards" => Ok(json!(ferb_learning::playbooks::verified_cards(l)?
            .iter()
            .map(|c| c["id"].clone())
            .collect::<Vec<_>>())),
        "mac.interpret" => {
            let mk = make_runtime_for(env.events.clone(), env.answers.clone());
            let e = env.events.clone();
            let usage = move |n: &str, d: &Value| ev(&e, "usage", json!({"runtime": n, "data": d}));
            ferb_learning::mac_profile::interpret(
                &mk,
                &env.prompts,
                &a["facts"],
                "dev-1",
                Some(env.spans.as_ref()),
                &usage,
            )
            .await
        }
        "mac.store" => {
            let j = env.journal.clone();
            let append = move |k: &str, d: Value| j.append(k, Some(d), None);
            let (out, held) = p.holding_joins(|| {
                ferb_learning::mac_profile::store(m, &append, &a["facts"], &a["answer"])
            });
            Ok(json!({"out": out?, "held": held.iter().map(|i| i.id.clone()).collect::<Vec<_>>()}))
        }
        "mac.previous" => Ok(json!(ferb_learning::mac_profile::previous(m)?
            .iter()
            .map(|i| i.id.clone())
            .collect::<Vec<_>>())),
        "import.estimate" => env.importer().estimate(&st(a, "source"), &st(a, "path")),
        "import.create" => Ok(json!(env
            .importer()
            .create(&st(a, "source"), &st(a, "path"))?)),
        "import.run" => env.importer().run(&st(a, "run")).await,
        "import.restart" => none,
        "import.end" => {
            env.importer()
                .end(&st(a, "run"), &st(a, "status"), &st(a, "error"))?;
            none
        }
        "import.status" => env.importer().status(),
        "import.running" => Ok(json!(env.importer().running()?)),
        "import.tell_done" => {
            env.importer().tell_done(&st(a, "run"))?;
            none
        }
        q if q.starts_with("questions.") => {
            let mq = ferb_learning::questions::MemoryQuestions {
                store: env.store.clone(),
            };
            match q {
                "questions.keep" => mq.keep(
                    &st(a, "id"),
                    &st(a, "kind"),
                    &st(a, "target"),
                    &st(a, "reason"),
                    &st(a, "request_id"),
                    &st(a, "at"),
                )?,
                "questions.waiting" => return Ok(json!(mq.waiting()?)),
                "questions.answered" => mq.answered(&st(a, "id"))?,
                "questions.drop_about" => {
                    let kinds = a
                        .get("kinds")
                        .filter(|k| !k.is_null())
                        .map(|k| strings(Some(k)));
                    mq.drop_about(&strings(a.get("targets")), kinds.as_deref())?
                }
                "questions.drop_kinds" => mq.drop_kinds(&strings(a.get("kinds")))?,
                _ => panic!("no such op {q}"),
            }
            none
        }
        "upgrade.clean" => {
            let j = env.journal.clone();
            let record = move |k: &str, d: Value| j.append(k, Some(d), None).map(|_| ());
            Ok(ferb_learning::upgrade::clean_before_tier1(m, &record)?.unwrap_or(Value::Null))
        }
        "upgrade.close_fact_nodes" => {
            let j = env.journal.clone();
            let record = move |k: &str, d: Value| j.append(k, Some(d), None).map(|_| ());
            Ok(json!(ferb_learning::upgrade::close_fact_nodes(
                &env.store,
                &py::iso(&env.clock.now()),
                &record
            )?))
        }
        "tool.learned" => {
            let args = ferb_learning::tools::Args {
                query: st(a, "query"),
                item_id: opt(a, "item_id"),
                kind: opt(a, "kind"),
                app: opt(a, "app"),
                person: opt(a, "person"),
                since: opt(a, "since"),
                until: opt(a, "until"),
                offset: a.get("offset").and_then(Value::as_u64).unwrap_or(0) as usize,
                max_chars: a.get("max_chars").and_then(Value::as_u64).unwrap_or(12000) as usize,
            };
            let out = ferb_learning::tools::run(Some(l), &args, None)?;
            Ok(json!({"content": out.content, "is_error": out.is_error}))
        }
        other => panic!("no such op {other}"),
    }
}

fn env_name(env: &Env) -> String {
    env.store
        .db
        .path()
        .parent()
        .unwrap()
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into()
}

pub fn evidence_json(ev: &summary::DayEvidence) -> Value {
    json!({"day": ev.day, "digest": ev.digest, "lines": ev.lines,
           "handles": Value::Object(ev.handles.iter().map(|(k, v)| (k.clone(), json!(v.iter().map(|(a, b)| json!([a, b])).collect::<Vec<_>>()))).collect()),
           "owner_words": Value::Object(ev.owner_words.iter().map(|(k, (a, b))| (k.clone(), json!([a, b]))).collect())})
}

/// tests/support/fake_embedder.py: each word of the concept table maps to one of 256 dimensions.
pub struct FakeEmbedder;
impl ferb_memory::embeddings::Embedder for FakeEmbedder {
    fn name(&self) -> &str {
        "fake-embedder:256"
    }
    fn dimensions(&self) -> usize {
        256
    }
    fn encode(&self, text: &str) -> Option<Vec<i8>> {
        const CONCEPTS: [(&str, &str); 29] = [
            ("piano", "music"),
            ("keyboard", "music"),
            ("melody", "music"),
            ("chords", "music"),
            ("sonata", "music"),
            ("beginner", "novice"),
            ("learning", "novice"),
            ("newbie", "novice"),
            ("permit", "permit"),
            ("consulate", "permit"),
            ("appointment", "permit"),
            ("ofc", "permit"),
            ("biometrics", "permit"),
            ("email", "mail"),
            ("mail", "mail"),
            ("outreach", "mail"),
            ("mailing", "mail"),
            ("inbox", "mail"),
            ("groceries", "food"),
            ("blinkit", "food"),
            ("orderly", "food"),
            ("dinner", "food"),
            ("tone", "style"),
            ("linkedinish", "style"),
            ("buzzwords", "style"),
            ("jargon", "style"),
            ("", ""),
            ("", ""),
            ("", ""),
        ];
        let mut vec = vec![0.0f64; 256];
        for word in ferb_learning::text::query_terms(text) {
            let Some((_, concept)) = CONCEPTS.iter().find(|(w, _)| !w.is_empty() && *w == word)
            else {
                continue;
            };
            let digest = py::sha256_hex(concept);
            // int(hexdigest, 16) % 256: the last byte
            let last = u8::from_str_radix(&digest[62..], 16).unwrap();
            vec[last as usize] += 1.0;
        }
        if vec.iter().all(|v| *v == 0.0) {
            return None;
        }
        ferb_memory::embeddings::quantize(&vec).ok()
    }
}

/// `Value` with the temp root written as make.py writes it ("/FX").
pub fn hide_root(v: Value, root: &Path) -> Value {
    let text = v
        .to_string()
        .replace(&root.to_string_lossy().to_string(), "/FX");
    serde_json::from_str(&text).unwrap()
}
