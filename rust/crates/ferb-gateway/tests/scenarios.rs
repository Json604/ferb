//! The gateway against `rust/fixtures/gateway/scenarios.json` (made by `make.py` from the real Python `Gateway`):
//! every call's reply, every published event, every question asked (its whole card), and the stores' tables at the
//! end, byte for byte. Mac-side: the gateway's records are on the SQLite store.
#![cfg(target_os = "macos")]

mod support;

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

use ferb_core::interfaces::{BoxFuture, Clock, IdSource};
use ferb_core::types::{ActionFacts, ActionSpec, Target, TerminalInput, ToolContext, ToolResult};
use ferb_core::when::{self, DateTime};
use ferb_gateway::gateway::{
    CallError, Cancel, Deps, FsRoots, Gateway, StoreIds, StoreTime, SyncProtection, TaskRecord,
    POSIX_FILES,
};
use ferb_gateway::seams::{Describe, NoHooks, QuestionRouter, Usage};
use ferb_gateway::tool::{Args, Facts, Registry, Tool, ToolError, ToolOutput};
use ferb_policy::confirm::{Apps, AppsUnread, RunningApp};
use ferb_policy::small::{Request, Targets};
use ferb_rpc::auth::{Identity, RunGrant};
use ferb_sanitize::Redactor;
use ferb_store::Db;
use serde_json::{json, Map, Value};

use support::ledger::SqlLedger;

// ---------- stand-ins ----------

struct FakeClock(Mutex<DateTime>);
impl Clock for FakeClock {
    fn now(&self) -> DateTime {
        *self.0.lock().unwrap()
    }
    fn monotonic(&self) -> f64 {
        1000.0
    }
    fn sleep_until(&self, _t: DateTime) -> BoxFuture<'_, ()> {
        Box::pin(async {})
    }
}

struct Seq(Mutex<u64>);
impl IdSource for Seq {
    fn new_id(&self) -> String {
        let mut n = self.0.lock().unwrap();
        *n += 1;
        format!("id-{:06}", *n)
    }
}

struct NoApps;
impl Apps for NoApps {
    fn candidates(&self, _name: &str) -> Result<Vec<RunningApp>, AppsUnread> {
        Ok(Vec::new())
    }
    fn installed_bundle(&self, _name: &str) -> Result<Option<String>, AppsUnread> {
        Ok(None)
    }
    fn is_browser_app(&self, _app: &str) -> bool {
        false
    }
}

/// `progress.describe` for the fixture's tools (orchestration's): a kind's live line, else "working".
struct Words;
impl Describe for Words {
    fn describe(
        &self,
        _tool: &str,
        _args: &Map<String, Value>,
        kind: &str,
        _site: Option<&str>,
        _app: Option<&str>,
    ) -> String {
        match kind {
            "send" => "sending".into(),
            "pay" => "paying".into(),
            _ => "working".into(),
        }
    }
    fn app_name(&self, _app: &str) -> String {
        "an app".into()
    }
}

struct NoTrash;
impl ferb_fs::files::TrashLocator for NoTrash {
    fn trash_url(&self, _near: &str, _create: bool) -> Option<String> {
        None
    }
}

struct NoUsage;
impl Usage for NoUsage {
    fn count<'a>(&'a self, _p: &'a str) -> BoxFuture<'a, Map<String, Value>> {
        Box::pin(async { Map::new() })
    }
}

struct Router(Option<Map<String, Value>>);
impl QuestionRouter for Router {
    fn route<'a>(
        &'a self,
        _t: &'a str,
        question: &'a str,
        _o: &'a [String],
    ) -> BoxFuture<'a, Result<Map<String, Value>, String>> {
        let out = match &self.0 {
            None => json!({"ask_owner": true, "question": question, "answer": ""})
                .as_object()
                .cloned()
                .unwrap(),
            Some(m) => m.clone(),
        };
        Box::pin(async move { Ok(out) })
    }
}

// ---------- the fake capability (make.py's SpecCap) ----------

struct SpecTool {
    spec: Value,
    schema: Value,
    calls: Mutex<usize>,
}

fn s(v: &Value, k: &str) -> Option<String> {
    v.get(k)
        .and_then(Value::as_str)
        .map(str::to_string)
        .filter(|x| !x.is_empty())
}

fn host_of(url: &str) -> Option<String> {
    // browser tools' _host: the URL's hostname, lowercased
    let h = ferb_gateway::words::host_of(Some(url));
    (!h.is_empty()).then_some(h)
}

impl Tool for SpecTool {
    fn name(&self) -> &str {
        self.spec["name"].as_str().unwrap()
    }
    fn facts(&self) -> Facts {
        Facts {
            risk: 2,
            reversible: self
                .spec
                .get("reversible")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            observation: self
                .spec
                .get("observation")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            generic_input: self
                .spec
                .get("generic_input")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            origin: s(&self.spec, "origin"),
            ..Facts::default()
        }
    }
    fn scopes(&self) -> BTreeSet<String> {
        match self.spec.get("scopes").and_then(Value::as_array) {
            Some(a) => a.iter().map(|x| x.as_str().unwrap().to_string()).collect(),
            None => ["probe".to_string()].into_iter().collect(),
        }
    }
    fn identities(&self) -> BTreeSet<String> {
        match self.spec.get("identities").and_then(Value::as_array) {
            Some(a) => a.iter().map(|x| x.as_str().unwrap().to_string()).collect(),
            None => ["owner".to_string(), "ferb".to_string()]
                .into_iter()
                .collect(),
        }
    }
    fn description(&self, _live: &ferb_gateway::catalog::Live) -> String {
        self.name().to_string()
    }
    fn input_schema(&self) -> Value {
        self.schema.clone()
    }
    fn hooks(&self) -> Option<Box<dyn ferb_gateway::pydantic::Hooks + '_>> {
        None
    }
    fn describe(&self, args: &Args) -> ActionSpec {
        let kind = self.spec["kind"].as_str().unwrap();
        let a = |k: &str| {
            args.get(k)
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string()
        };
        let amount = match args.get("amount").and_then(Value::as_f64) {
            None => (kind == "pay").then_some(5.0),
            Some(x) => Some(x),
        };
        let mut site = Some(a("site"))
            .filter(|x| !x.is_empty())
            .or_else(|| s(&self.spec, "site"));
        if self.spec.get("site_from_url").is_some() {
            site = host_of(&a("url"));
        }
        let to = a("to");
        let target = Target {
            recipients: if to.is_empty() { vec![] } else { vec![to] },
            site,
            amount: amount.filter(|x| *x != -1.0),
            account: Some(a("account")).filter(|x| !x.is_empty()),
            contact: s(&self.spec, "contact"),
            ..Target::default()
        };
        let facts = self.spec.get("facts").map(|f| {
            let mut out = ActionFacts::default();
            let m = f.as_object().unwrap();
            for (k, v) in m {
                match k.as_str() {
                    "submits_form" => out.submits_form = v.as_bool(),
                    "field_kinds" => out.field_kinds = serde_json::from_value(v.clone()).unwrap(),
                    "page_url" => out.page_url = v.as_str().map(str::to_string),
                    "page_amount" => out.page_amount = v.as_str().map(str::to_string),
                    "page_merchant" => out.page_merchant = v.as_str().map(str::to_string),
                    "form_method" => out.form_method = v.as_str().map(str::to_string),
                    "form_action_host" => out.form_action_host = v.as_str().map(str::to_string),
                    "tab_chain" => out.tab_chain = v.as_str().map(str::to_string),
                    "page_amounts" => out.page_amounts = serde_json::from_value(v.clone()).unwrap(),
                    other => panic!("fact {other}"),
                }
            }
            out
        });
        ActionSpec {
            kind: kind.to_string(),
            target,
            content: Some(a("text")).filter(|x| !x.is_empty()),
            declared: s(&self.spec, "declared"),
            place: s(&self.spec, "place"),
            effect_kind: None,
            facts,
        }
    }
    fn locks(&self, args: &Args) -> Vec<String> {
        self.spec
            .get("locks")
            .and_then(Value::as_array)
            .map(|l| {
                l.iter()
                    .map(|k| {
                        let mut t = k.as_str().unwrap().to_string();
                        for (name, v) in args {
                            t = t.replace(&format!("{{{name}}}"), &ferb_py::value::py_str(v));
                        }
                        t
                    })
                    .collect()
            })
            .unwrap_or_default()
    }
    fn terminal_input<'a>(
        &'a self,
        args: &'a Args,
        _task: &'a str,
    ) -> BoxFuture<'a, Option<TerminalInput>> {
        Box::pin(async move {
            let t = self.spec.get("terminal")?;
            let text = args
                .get("text")
                .and_then(Value::as_str)
                .filter(|x| !x.is_empty())
                .map(str::to_string)
                .or_else(|| s(t, "text"));
            Some(TerminalInput {
                app: s(t, "app").unwrap_or_default(),
                bundle_id: s(t, "bundle_id").unwrap_or_default(),
                surface: s(t, "surface").unwrap_or_default(),
                window: s(t, "window").unwrap_or_default(),
                text,
                ..TerminalInput::default()
            })
        })
    }
    fn run<'a>(
        &'a self,
        _args: &'a Args,
        _ctx: &'a ToolContext,
    ) -> BoxFuture<'a, Result<ToolOutput, ToolError>> {
        Box::pin(async move {
            let results = self
                .spec
                .get("results")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_else(|| vec![json!({})]);
            let mut n = self.calls.lock().unwrap();
            let r = results[(*n).min(results.len() - 1)].clone();
            *n += 1;
            if let Some(e) = r.get("raise") {
                let class = e["class"].as_str().unwrap();
                let message = e["message"].as_str().unwrap();
                if class == "Refused" {
                    return Err(ToolError::refused(message));
                }
                let mut err = ToolError::failed(class, message);
                if e.get("attempted") == Some(&Value::Bool(false)) {
                    err.before_acting = true;
                }
                return Err(err);
            }
            let content = r.get("content").cloned().unwrap_or(json!({"ok": true}));
            let reads = r
                .get("reads")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .map(|p| {
                            (
                                p[0].as_str().unwrap().to_string(),
                                p[1].as_str().unwrap().to_string(),
                            )
                        })
                        .collect()
                })
                .unwrap_or_default();
            Ok(ToolOutput::from(ToolResult {
                content: content.as_object().cloned().unwrap(),
                is_error: r.get("is_error").and_then(Value::as_bool).unwrap_or(false),
                read_sources: reads,
                ..ToolResult::default()
            }))
        })
    }
}

/// owner.ask (`ferb/act/ask.py`): its schema and facts are the frozen catalog's.
struct OwnerAsk;
impl Tool for OwnerAsk {
    fn name(&self) -> &str {
        "owner.ask"
    }
    fn describe(&self, _args: &Args) -> ActionSpec {
        ActionSpec::new("read")
    }
    fn run<'a>(
        &'a self,
        _a: &'a Args,
        _c: &'a ToolContext,
    ) -> BoxFuture<'a, Result<ToolOutput, ToolError>> {
        Box::pin(async {
            Err(ToolError::failed(
                "RuntimeError",
                "owner.ask runs through the gateway",
            ))
        })
    }
}

// ---------- one scenario ----------

struct Run {
    gw: Arc<Gateway>,
    db: Arc<Mutex<Db>>,
    clock: Arc<FakeClock>,
    events: Arc<Mutex<Vec<Value>>>,
    journal: Arc<Mutex<Vec<Value>>>,
    ledger: Arc<SqlLedger>,
    ids: Arc<dyn IdSource>,
    grants: Vec<(String, RunGrant)>,
    last_retry: String,
    home: String,
    roots: Vec<String>,
}

/// make.py's `ROOT`.
const ROOT: &str = "/tmp/ferbgw";

fn start() -> DateTime {
    when::parse("2026-01-01T09:00:00+00:00").unwrap()
}

fn build(sc: &Value) -> Run {
    // make.py's one fixed temp folder: a protected path's question is keyed by a hash of the absolute path
    let dir = std::path::PathBuf::from(ROOT);
    let home = dir.join(sc["name"].as_str().unwrap());
    let _ = std::fs::remove_dir_all(&home);
    let _ = std::fs::remove_dir_all(dir.join("user"));
    std::fs::create_dir_all(home.join("workspace")).unwrap();
    let db = Db::open(home.join("ferb.db")).unwrap();
    db.migrate().unwrap();
    let db = Arc::new(Mutex::new(db));
    let clock = Arc::new(FakeClock(Mutex::new(start())));
    let ids: Arc<dyn IdSource> = Arc::new(Seq(Mutex::new(0)));
    let redactor = Arc::new(Redactor::new(&[], &[]));
    let ledger = Arc::new(SqlLedger {
        db: db.clone(),
        clock: clock.clone(),
        ids: ids.clone(),
    });
    let events = Arc::new(Mutex::new(Vec::new()));
    let journal = Arc::new(Mutex::new(Vec::new()));
    let user = dir.join("user");
    std::fs::create_dir_all(&user).unwrap();
    let never_read = json!(ferb_policy::protection::default_never_read());
    let never_write = json!(ferb_policy::protection::DEFAULT_NEVER_WRITE);
    let terminals = json!(ferb_policy::protection::DEFAULT_TERMINAL_APPS);
    let ports = json!([]);
    let lists = ferb_policy::protection::Lists {
        never_read: &never_read,
        never_write: &never_write,
        terminal_apps: &terminals,
        local_ports: &ports,
    };
    let protection = ferb_policy::protection::Protection::new(
        user.to_str().unwrap(),
        home.to_str().unwrap(),
        &lists,
        None,
        &POSIX_FILES,
    )
    .unwrap();
    let mut reg = Registry::new();
    let schemas = &sc["tool_schemas"];
    for t in sc
        .get("tools")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
    {
        let name = t["name"].as_str().unwrap().to_string();
        reg.add(Arc::new(SpecTool {
            schema: schemas[&name].clone(),
            spec: t,
            calls: Mutex::new(0),
        }))
        .unwrap();
    }
    let env = ferb_fs::files::FsEnv {
        home: user.to_str().unwrap().to_string(),
        trash: Arc::new(NoTrash),
        full_disk_access: Arc::new(|| true),
    };
    for name in sc
        .get("fs_tools")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
    {
        let info = match name.as_str().unwrap() {
            "fs.list" => ferb_fs::files::FS_LIST,
            "fs.read" => ferb_fs::files::FS_READ,
            "fs.write" => ferb_fs::files::FS_WRITE,
            "fs.edit" => ferb_fs::files::FS_EDIT,
            "fs.copy" => ferb_fs::files::FS_COPY,
            other => panic!("{other}"),
        };
        reg.add(Arc::new(ferb_gateway::adapters::fs::FsTool::new(
            info,
            env.clone(),
        )))
        .unwrap();
    }
    reg.add(Arc::new(OwnerAsk)).unwrap();
    for (name, text) in sc
        .get("files")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default()
    {
        let p = name
            .replacen('~', user.to_str().unwrap(), 1)
            .replace("$HOME", home.to_str().unwrap());
        std::fs::create_dir_all(std::path::Path::new(&p).parent().unwrap()).unwrap();
        std::fs::write(&p, text.as_str().unwrap()).unwrap();
    }
    let route = sc.get("route").and_then(Value::as_object).cloned();
    let own: Vec<String> = sc
        .get("own")
        .and_then(Value::as_array)
        .map(|a| a.iter().map(|x| x.as_str().unwrap().to_string()).collect())
        .unwrap_or_default();
    let pay_same = sc
        .get("pay_same_site_forms")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let ev = events.clone();
    let jr = journal.clone();
    let deps = Deps {
        db: db.clone(),
        clock: clock.clone(),
        ids: ids.clone(),
        redactor,
        ledger: ledger.clone(),
        publish: Box::new(move |topic, data| ev.lock().unwrap().push(json!([topic, data]))),
        roots: FsRoots {
            workspace: home.join("workspace").to_str().unwrap().to_string(),
            owner_roots: vec![],
            untrusted_roots: vec![],
            home: user.to_str().unwrap().to_string(),
            protection: Arc::new(SyncProtection(protection)),
        },
        device_id: "dev".into(),
        recovery: Arc::new(ferb_gateway::adapters::fs::FsRecovery {
            db: db.clone(),
            clock: clock.clone(),
            ids: ids.clone(),
            root: home.join("recovery").to_str().unwrap().to_string(),
        }),
        snapshots: None,
        usage: Arc::new(NoUsage),
        apps: Arc::new(NoApps),
        describe: Arc::new(Words),
        journal_problem: Box::new(move |kind, data| jr.lock().unwrap().push(json!([kind, data]))),
        profile_name: Box::new(|_b, f| f.to_string()),
        profile_account: Box::new(|_b, _f| String::new()),
        owner_contacts: Box::new(move || own.clone()),
        pay_same_site_forms: Box::new(move || pay_same),
        on_path: Box::new(|_p, _t| {}),
        proc_task: Box::new(|_p| None),
        route_question: Some(Arc::new(Router(route))),
        hooks: Arc::new(NoHooks),
        made_in_tree: None,
        excluded_host: None,
    };
    let counter = Mutex::new(0u64);
    let gw = Arc::new(Gateway::new(reg, deps, move || {
        let mut n = counter.lock().unwrap();
        *n += 1;
        format!("ap-a{:011}", *n)
    }));
    let canon = std::fs::canonicalize(&dir)
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    let roots = vec![canon, ROOT.to_string()];
    Run {
        gw,
        db,
        clock,
        events,
        journal,
        ledger,
        ids,
        grants: Vec::new(),
        last_retry: String::new(),
        home: home.to_str().unwrap().to_string(),
        roots,
    }
}

fn setup(r: &mut Run, sc: &Value) {
    let requests = sc
        .get("requests")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_else(|| vec![json!({"id": "req-1", "words": ""})]);
    for q in requests {
        let sites: BTreeSet<String> = q
            .get("sites")
            .and_then(Value::as_array)
            .map(|a| a.iter().map(|x| x.as_str().unwrap().to_string()).collect())
            .unwrap_or_default();
        r.gw.record_request(Request {
            request_id: q["id"].as_str().unwrap().into(),
            words: s(&q, "words").unwrap_or_default(),
            targets: Targets {
                sites,
                ..Targets::default()
            },
            payload: s(&q, "payload").unwrap_or_default(),
            asked_origins: BTreeSet::new(),
        });
    }
    let tasks = sc
        .get("tasks")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_else(|| vec![json!({"id": "task-1"})]);
    let now = r.clock.now();
    for t in tasks {
        let id = t["id"].as_str().unwrap().to_string();
        let tree = s(&t, "tree");
        let request = s(&t, "request").unwrap_or_else(|| "req-1".into());
        let scopes: BTreeSet<String> = t
            .get("scopes")
            .and_then(Value::as_array)
            .map(|a| a.iter().map(|x| x.as_str().unwrap().to_string()).collect())
            .unwrap_or_else(|| ["probe".to_string()].into_iter().collect());
        let identity = t
            .get("identity")
            .and_then(Value::as_str)
            .unwrap_or("owner")
            .to_string();
        let span_id = {
            let db = r.db.lock().unwrap();
            match &tree {
                Some(tr) => db
                    .conn()
                    .query_row(
                        "SELECT id FROM spans WHERE kind = 'task' AND task_id = ?",
                        [tr],
                        |x| x.get::<_, String>(0),
                    )
                    .unwrap(),
                None => {
                    let store = ferb_store::spans::SpanStore::new(
                        &db,
                        Box::new(StoreTime(r.clock.clone())),
                        Box::new(StoreIds(r.ids.clone())),
                        Box::new(|v: &Value| v.clone()),
                        Box::new(|_k: &str| false),
                    );
                    let input = t.get("span_input").cloned().unwrap_or(json!({}));
                    store
                        .start("task", &input, Some(&id), None, None)
                        .unwrap()
                        .id
                }
            }
        };
        {
            // TaskTree.add's row (tree.py:62)
            let db = r.db.lock().unwrap();
            let at = when::isoformat(&now);
            db.conn()
                .execute(
                    "INSERT INTO tasks (id, tree_id, parent_id, request_id, goal, identity, role, scopes_json, status, created_at, updated_at) \
                     VALUES (?, ?, ?, ?, 'g', ?, 'worker', ?, 'running', ?, ?)",
                    rusqlite::params![id, tree.clone().unwrap_or_else(|| id.clone()), s(&t, "parent"), request, identity, ferb_py::json::dumps(&json!(scopes)), at, at],
                )
                .unwrap();
        }
        r.gw.add_task(TaskRecord {
            task_id: id.clone(),
            request_id: request,
            started_at: now,
            deadline: Some(now + chrono::Duration::minutes(10)),
            span_id,
            tree_id: tree,
        });
        let grant = RunGrant {
            task_id: id.clone(),
            worker_id: s(&t, "worker").unwrap_or_else(|| "w1".into()),
            scopes,
            identity: if identity == "ferb" {
                Identity::Ferb
            } else {
                Identity::Owner
            },
            expires_at: 1e9,
            observe_only: t
                .get("observe_only")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            tools: t
                .get("tools")
                .and_then(Value::as_array)
                .map(|a| a.iter().map(|x| x.as_str().unwrap().to_string()).collect())
                .unwrap_or_default(),
            max_calls: t.get("max_calls").and_then(Value::as_u64).unwrap_or(0),
        };
        r.grants.push((s(&t, "grant").unwrap_or(id), grant));
    }
    for k in sc
        .get("known")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
    {
        let db = r.db.lock().unwrap();
        let redactor = Redactor::new(&[], &[]);
        let d = ferb_policy::stores::KnownDestinations {
            db: &db,
            clock: r.clock.as_ref(),
            ids: r.ids.as_ref(),
            redact: &redactor,
        };
        d.record(
            k[0].as_str().unwrap(),
            k[1].as_str().unwrap(),
            k[2].as_str().unwrap(),
            k.get(3).and_then(Value::as_str),
            None,
        )
        .unwrap();
    }
    for st in sc
        .get("standing")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
    {
        let db = r.db.lock().unwrap();
        let scope: ferb_policy::standing::SendScope =
            serde_json::from_value(st["scope"].clone()).unwrap();
        ferb_policy::stores::StandingPermissions { db: &db }
            .grant(
                st["task"].as_str().unwrap(),
                &[scope],
                st["approval"].as_str().unwrap(),
            )
            .unwrap();
    }
}

/// What a recorded Approval snapshot holds (make.py `jsonable(ap)`).
fn approval_json(ap: &ferb_gateway::questions::Approval) -> Value {
    json!({
        "id": ap.id, "capability": ap.capability, "reason": ap.reason, "request_id": ap.request_id, "kind": ap.kind,
        "state": ap.state.as_str(), "options": ap.options, "choice": ap.choice, "answer": ap.answer,
        "summary": ap.summary, "from_task": ap.from_task, "why": ap.why, "detail": ap.detail, "exact": ap.exact,
        "taken": ap.taken, "key": ap.key, "task_id": ap.task_id, "about": ap.about, "part": ap.part,
        "runtime_switch": ap.runtime_switch, "hidden": ap.hidden, "options_only": ap.options_only,
        "corrects": ap.corrects, "labels": ap.labels, "note": ap.note, "title": ap.title,
        "secret_labels": ap.secret_labels, "site": ap.site, "picture": ap.picture
    })
}

impl Run {
    fn grant(&self, label: &str) -> RunGrant {
        self.grants
            .iter()
            .find(|(g, _)| g == label)
            .unwrap()
            .1
            .clone()
    }

    async fn call(&mut self, step: &Value) -> Value {
        let grant = self.grant(
            step.get("grant")
                .and_then(Value::as_str)
                .unwrap_or("task-1"),
        );
        let mut arguments: Map<String, Value> = step["arguments"]
            .as_object()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .map(|(k, v)| match v {
                Value::String(t) => (k, Value::String(t.replace("$HOME", &self.home))),
                other => (k, other),
            })
            .collect();
        if arguments.get("retry_action") == Some(&json!("$last")) {
            arguments.insert("retry_action".into(), json!(self.last_retry));
        }
        let mut answers: Vec<Value> = step["answers"].as_array().cloned().unwrap_or_default();
        answers.reverse();
        let start = self.events.lock().unwrap().len();
        let cancel = Cancel::new();
        let left = step.get("caller_left").is_some_and(|v| v == true);
        let gw = self.gw.clone();
        let name = step["tool"].as_str().unwrap().to_string();
        let fut = gw.call_as(
            &grant,
            &name,
            &arguments,
            cancel.clone(),
            Arc::new(move || left),
        );
        tokio::pin!(fut);
        let mut asked = Vec::new();
        let mut seen: BTreeSet<String> = BTreeSet::new();
        let result = loop {
            tokio::select! {
                biased;
                r = &mut fut => break r,
                () = tokio::task::yield_now() => {
                    let fresh: Vec<_> = self.gw.pending().into_iter().filter(|a| !seen.contains(&a.id)).collect();
                    if let Some(ap) = fresh.into_iter().next() {
                        seen.insert(ap.id.clone());
                        asked.push(approval_json(&ap));
                        let a = answers.pop().unwrap_or_else(|| panic!("unanswered question {}", ap.reason));
                        if let Some(p) = a.get("pick") {
                            self.gw.resolve(&ap.id, true, Some(p.as_u64().unwrap() as usize), None).unwrap();
                        } else if let Some(c) = a.get("choice") {
                            let i = ap.options.iter().position(|o| o == c.as_str().unwrap()).expect("an option") + 1;
                            self.gw.resolve(&ap.id, true, Some(i), None).unwrap();
                        } else if let Some(t) = a.get("text") {
                            self.gw.resolve(&ap.id, true, None, t.as_str()).unwrap();
                        } else if a.get("deny").is_some() {
                            self.gw.resolve(&ap.id, false, None, None).unwrap();
                        } else if a.get("supersede").is_some() {
                            self.gw.supersede(&ap.id);
                        } else if a.get("cancel").is_some() {
                            self.gw.cancel_tasks(&[ap.task_id.clone()].into_iter().collect());
                        } else if a.get("leave").is_some() {
                            cancel.cancel();
                        } else {
                            panic!("{a}");
                        }
                    }
                }
            }
        };
        let mut out = json!({"asked": asked});
        match result {
            Ok(reply) => {
                if let Some(Value::String(r)) = reply.content.get("retry_action") {
                    self.last_retry = r.clone();
                }
                out["reply"] = reply.to_json();
            }
            Err(CallError::Cancelled) => out["raised"] = json!("CancelledError"),
            Err(CallError::Internal(e)) => panic!("internal: {e}"),
        }
        out["events"] = json!(self.events.lock().unwrap()[start..].to_vec());
        out
    }

    async fn step(&mut self, step: &Value) -> Value {
        let start = self.events.lock().unwrap().len();
        let ids = |v: &Value| -> BTreeSet<String> {
            v.as_array()
                .unwrap()
                .iter()
                .map(|x| x.as_str().unwrap().to_string())
                .collect()
        };
        match step["do"].as_str().unwrap() {
            "call" => return self.call(step).await,
            "stop" => self.gw.set_stopped(true),
            "lift" => self.gw.set_stopped(false),
            "cancel_tasks" => self.gw.cancel_tasks(&ids(&step["ids"])),
            "forget_tasks" => self.gw.forget_tasks(&ids(&step["ids"])),
            "unattended" => self
                .gw
                .set_unattended(step["request"].as_str().unwrap(), true),
            "clock" => {
                let mut t = self.clock.0.lock().unwrap();
                *t += chrono::Duration::milliseconds(
                    (step["seconds"].as_f64().unwrap() * 1000.0) as i64,
                );
            }
            "reopen_noes" => {
                let reopened = self
                    .gw
                    .reopen_noes(
                        step["tree"].as_str().unwrap(),
                        step["request"].as_str().unwrap(),
                        step["source_ref"].as_str().unwrap(),
                    )
                    .unwrap();
                return json!({"reopened": reopened, "events": self.events.lock().unwrap()[start..].to_vec()});
            }
            "restart" => return json!({"unknown": self.ledger.mark_unknown_after_restart()}),
            "failed_act" => {
                return json!({"failed_act": self.gw.failed_act(step["worker"].as_str().unwrap())})
            }
            other => panic!("{other}"),
        }
        json!({"events": self.events.lock().unwrap()[start..].to_vec()})
    }

    fn tables(&self, want: &Value) -> Value {
        let db = self.db.lock().unwrap();
        let mut out = serde_json::Map::new();
        for (table, _) in want.as_object().unwrap() {
            let mut st = db
                .conn()
                .prepare(&format!("SELECT * FROM {table} ORDER BY rowid"))
                .unwrap();
            let cols: Vec<String> = st.column_names().map(str::to_string).collect();
            let n = cols.len();
            let rows: Vec<Value> = st
                .query_map([], |r| {
                    let mut row = Vec::new();
                    for i in 0..n {
                        let v: rusqlite::types::Value = r.get(i)?;
                        row.push(match v {
                            rusqlite::types::Value::Null => Value::Null,
                            rusqlite::types::Value::Integer(i) => json!(i),
                            rusqlite::types::Value::Real(f) => json!(f),
                            rusqlite::types::Value::Text(t) => json!(t),
                            rusqlite::types::Value::Blob(b) => json!({"hex": b}),
                        });
                    }
                    Ok(Value::Array(row))
                })
                .unwrap()
                .map(Result::unwrap)
                .collect();
            out.insert(table.clone(), json!({"columns": cols, "rows": rows}));
        }
        Value::Object(out)
    }
}

/// The first place two JSON values differ.
fn diff(path: &str, a: &Value, b: &Value) -> Option<String> {
    match (a, b) {
        (Value::Object(x), Value::Object(y)) => {
            let kx: Vec<&String> = x.keys().collect();
            let ky: Vec<&String> = y.keys().collect();
            if kx != ky {
                return Some(format!("{path}: keys {kx:?} != {ky:?}"));
            }
            x.iter()
                .find_map(|(k, v)| diff(&format!("{path}.{k}"), v, &y[k]))
        }
        (Value::Array(x), Value::Array(y)) => {
            if x.len() != y.len() {
                return Some(format!(
                    "{path}: {} items != {}\n  got  {}\n  want {}",
                    x.len(),
                    y.len(),
                    Value::Array(x.clone()),
                    Value::Array(y.clone())
                ));
            }
            x.iter()
                .zip(y)
                .enumerate()
                .find_map(|(i, (p, q))| diff(&format!("{path}[{i}]"), p, q))
        }
        _ if ferb_py::json::dumps(a) != ferb_py::json::dumps(b) => {
            Some(format!("{path}:\n  got  {a}\n  want {b}"))
        }
        _ => None,
    }
}

/// make.py's /FX for the temp folder, and "<volatile>" for a file's identity facts (both sides).
fn masked(v: &Value, roots: &[String], keys: &[String]) -> Value {
    match v {
        Value::String(t) => {
            let mut t = t.clone();
            for r in roots {
                t = t.replace(r.as_str(), "/FX");
            }
            if t.starts_with('{') || t.starts_with('[') {
                if let Ok(inner) = serde_json::from_str::<Value>(&t) {
                    let m = masked(&inner, roots, keys);
                    if m != inner {
                        return Value::String(ferb_py::json::dumps_utf8(&m));
                    }
                }
            }
            Value::String(t)
        }
        Value::Array(a) => Value::Array(a.iter().map(|x| masked(x, roots, keys)).collect()),
        Value::Object(o) => Value::Object(
            o.iter()
                .map(|(k, x)| {
                    let x = if keys.contains(k) && !x.is_null() {
                        json!("<volatile>")
                    } else {
                        masked(x, roots, keys)
                    };
                    (k.clone(), x)
                })
                .collect(),
        ),
        other => other.clone(),
    }
}

/// The recovery table's identity columns, by name.
fn masked_tables(v: &Value, keys: &[String]) -> Value {
    let mut out = v.clone();
    for (_, t) in out.as_object_mut().unwrap().iter_mut() {
        let cols: Vec<String> = t["columns"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c.as_str().unwrap().to_string())
            .collect();
        for row in t["rows"].as_array_mut().unwrap() {
            for (i, cell) in row.as_array_mut().unwrap().iter_mut().enumerate() {
                if keys.contains(&cols[i]) && !cell.is_null() {
                    *cell = json!("<volatile>");
                }
            }
        }
    }
    out
}

async fn replay(sc: &Value) -> Vec<String> {
    let mut r = build(sc);
    setup(&mut r, sc);
    let mut problems = Vec::new();
    let name = sc["name"].as_str().unwrap();
    let keys: Vec<String> = sc["volatile"]
        .as_array()
        .map(|a| a.iter().map(|k| k.as_str().unwrap().to_string()).collect())
        .unwrap_or_default();
    let none: Vec<String> = Vec::new();
    for (i, (step, want)) in sc["steps"]
        .as_array()
        .unwrap()
        .iter()
        .zip(sc["results"].as_array().unwrap())
        .enumerate()
    {
        let got = masked(&r.step(step).await, &r.roots, &keys);
        let want = masked(want, &none, &keys);
        if let Some(d) = diff(&format!("{name} step {i}"), &got, &want) {
            problems.push(d);
        }
    }
    let tables = masked_tables(&masked(&r.tables(&sc["tables"]), &r.roots, &keys), &keys);
    let want_tables = masked_tables(&masked(&sc["tables"], &none, &keys), &keys);
    if let Some(d) = diff(&format!("{name} tables"), &tables, &want_tables) {
        problems.push(d);
    }
    let journal = json!(r.journal.lock().unwrap().clone());
    if let Some(d) = diff(&format!("{name} journal"), &journal, &sc["journal"]) {
        problems.push(d);
    }
    problems
}

fn fixture() -> Value {
    let text = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../fixtures/gateway/scenarios.json"
    ))
    .unwrap();
    serde_json::from_str(&text).unwrap()
}

// decision: ENG-3
#[test]
fn every_scenario_replays_as_python_ran_it() {
    let f = fixture();
    let only = std::env::var("SCENARIO").ok();
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    // one run at a time in the fixed folder
    std::fs::create_dir_all(ROOT).unwrap();
    let lock = std::fs::File::create(format!("{ROOT}.lock")).unwrap();
    // SAFETY: flock on a file descriptor this test owns; it is released when the file closes.
    assert_eq!(
        unsafe { libc_flock(std::os::fd::AsRawFd::as_raw_fd(&lock), 2) },
        0
    );
    let mut failed = Vec::new();
    let mut count = 0;
    for sc in f["scenarios"].as_array().unwrap() {
        if only.as_deref().is_some_and(|o| o != sc["name"]) {
            continue;
        }
        count += 1;
        let problems = rt.block_on(replay(sc));
        if !problems.is_empty() {
            failed.push(format!("{}\n{}", sc["name"], problems.join("\n")));
        }
    }
    assert!(
        failed.is_empty(),
        "{} of {count} scenarios differ:\n{}",
        failed.len(),
        failed.join("\n\n")
    );
}

extern "C" {
    #[link_name = "flock"]
    fn libc_flock(fd: i32, op: i32) -> i32;
}

fn luhn(digits: &str) -> bool {
    let sum: u32 = digits
        .chars()
        .rev()
        .enumerate()
        .map(|(i, c)| {
            let d = c.to_digit(10).unwrap();
            if i % 2 == 1 {
                let x = d * 2;
                if x > 9 {
                    x - 9
                } else {
                    x
                }
            } else {
                d
            }
        })
        .sum();
    sum.is_multiple_of(10)
}

/// Fixed against Python (journal 23, `ferb/act/fs/files.py:51`): a file's 19-digit `mtime_ns` passing the Luhn
/// check was redacted as a card number in every tool result (and turned from a number into the text "[redacted]").
/// The file tools' numbers are their own measurements (`ToolOutput::measured`), so the result's redaction keeps
/// them; text in the same result is still redacted.
// decision: GATE-212
#[test]
fn a_files_mtime_ns_is_never_read_as_a_card_number() {
    let sc = json!({
        "name": "mtime_ns", "fs_tools": ["fs.read"], "tool_schemas": {},
        "tasks": [{"id": "task-1", "scopes": ["fs:read", "fs:write"]}],
        "files": {"~/card.txt": "4111 1111 1111 1111\n"}
    });
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    std::fs::create_dir_all(ROOT).unwrap();
    let lock = std::fs::File::create(format!("{ROOT}.lock")).unwrap();
    // SAFETY: flock on a file descriptor this test owns; released when the file closes.
    assert_eq!(
        unsafe { libc_flock(std::os::fd::AsRawFd::as_raw_fd(&lock), 2) },
        0
    );
    let mut r = build(&sc);
    setup(&mut r, &sc);
    // a modification time whose nanoseconds pass the Luhn check
    let mut ns: u64 = 1_794_538_250_392_642_000;
    while !luhn(&ns.to_string()) {
        ns += 1;
    }
    let path = format!("{ROOT}/user/card.txt");
    let file = std::fs::File::options().write(true).open(&path).unwrap();
    file.set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_nanos(ns))
        .unwrap();
    drop(file);
    // what Python's redaction made of it
    let python = Redactor::new(&[], &[]).value(&json!({"mtime_ns": ns}));
    assert_eq!(python, json!({"mtime_ns": "[redacted]"}));
    let got = rt.block_on(r.call(&json!({"do": "call", "tool": "fs.read", "arguments": {"path": "~/card.txt"}, "answers": []})));
    let content = &got["reply"]["content"];
    assert_eq!(content["mtime_ns"], json!(ns), "{content}");
    assert_eq!(
        content["text"],
        json!("[redacted]\n"),
        "file text is still redacted: {content}"
    );
}

/// A file tool that answers with what the Python daemon's tool answered (the daemon recordings), and reads
/// everything else off the real adapter: what it does, the files it names, its locks, what S1 copies.
struct Recorded {
    inner: ferb_gateway::adapters::fs::FsTool,
    replies: Mutex<Vec<Value>>,
}

impl Tool for Recorded {
    fn name(&self) -> &str {
        self.inner.name()
    }
    fn describe(&self, args: &Args) -> ActionSpec {
        self.inner.describe(args)
    }
    fn file_paths(&self, args: &Args) -> Option<Vec<(String, String)>> {
        self.inner.file_paths(args)
    }
    fn locks(&self, args: &Args) -> Vec<String> {
        self.inner.locks(args)
    }
    fn real_path(&self, args: &Args, workspace: &str) -> Option<ferb_gateway::tool::RealPath> {
        self.inner.real_path(args, workspace)
    }
    fn recovery_reason(&self, args: &Args) -> Option<Option<String>> {
        self.inner.recovery_reason(args)
    }
    fn run<'a>(
        &'a self,
        _args: &'a Args,
        _ctx: &'a ToolContext,
    ) -> BoxFuture<'a, Result<ToolOutput, ToolError>> {
        Box::pin(async move {
            let reply = self.replies.lock().unwrap().remove(0);
            let is_error = reply["is_error"].as_bool().unwrap();
            let mut content = reply["content"].as_object().unwrap().clone();
            content.shift_remove("after");
            let reads = match (self.name(), content.get("path").and_then(Value::as_str)) {
                ("fs.read", Some(p)) if !is_error => vec![("file".to_string(), p.to_string())],
                _ => vec![],
            };
            let mut measured = Vec::new();
            ferb_gateway::adapters::numbers(&Value::Object(content.clone()), "", &mut measured);
            Ok(ToolOutput {
                result: ToolResult {
                    content: content.clone(),
                    is_error,
                    read_sources: reads,
                    ..ToolResult::default()
                },
                measured,
            })
        })
    }
}

/// The daemon recordings' file-tool calls (rust/fixtures/daemon/chat_scenarios.json `gateway_calls`), replayed
/// through this gateway with the recorded tool results: the same replies the Python gateway gave inside those runs.
/// task.evidence and wait.for calls are not replayed: their read sources and the waits' clock aren't in the recording.
// decision: ENG-3
#[test]
fn the_daemon_recordings_file_calls_get_the_same_replies() {
    let text = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../fixtures/daemon/chat_scenarios.json"
    ))
    .unwrap();
    let d: Value = serde_json::from_str(&text).unwrap();
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let mut replayed = 0;
    let mut problems = Vec::new();
    for (n, sc) in d
        .get("scenarios")
        .unwrap_or(&d)
        .as_array()
        .unwrap()
        .iter()
        .enumerate()
    {
        let calls: Vec<Value> = sc["gateway_calls"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter(|c| c["tool"].as_str().unwrap().starts_with("fs.") && c.get("reply").is_some())
            .collect();
        if calls.is_empty() {
            continue;
        }
        let nn = format!("/FX/{:02}", n + 1);
        let (home, user) = (format!("{nn}/home"), format!("{nn}/user"));
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(dir.path().join("ferb.db")).unwrap();
        db.migrate().unwrap();
        let db = Arc::new(Mutex::new(db));
        let clock = Arc::new(FakeClock(Mutex::new(start())));
        let ids: Arc<dyn IdSource> = Arc::new(Seq(Mutex::new(0)));
        let env = ferb_fs::files::FsEnv {
            home: user.clone(),
            trash: Arc::new(NoTrash),
            full_disk_access: Arc::new(|| true),
        };
        let mut reg = Registry::new();
        for (name, info) in [
            ("fs.write", ferb_fs::files::FS_WRITE),
            ("fs.read", ferb_fs::files::FS_READ),
        ] {
            let replies: Vec<Value> = calls
                .iter()
                .filter(|c| c["tool"] == name)
                .map(|c| c["reply"].clone())
                .collect();
            reg.add(Arc::new(Recorded {
                inner: ferb_gateway::adapters::fs::FsTool::new(info, env.clone()),
                replies: Mutex::new(replies),
            }))
            .unwrap();
        }
        let never_read = json!(ferb_policy::protection::default_never_read());
        let never_write = json!(ferb_policy::protection::DEFAULT_NEVER_WRITE);
        let terminals = json!(ferb_policy::protection::DEFAULT_TERMINAL_APPS);
        let ports = json!([]);
        let lists = ferb_policy::protection::Lists {
            never_read: &never_read,
            never_write: &never_write,
            terminal_apps: &terminals,
            local_ports: &ports,
        };
        let protection =
            ferb_policy::protection::Protection::new(&user, &home, &lists, None, &POSIX_FILES)
                .unwrap();
        let deps = Deps {
            db: db.clone(),
            clock: clock.clone(),
            ids: ids.clone(),
            redactor: Arc::new(Redactor::new(&[], &[])),
            ledger: Arc::new(SqlLedger {
                db: db.clone(),
                clock: clock.clone(),
                ids: ids.clone(),
            }),
            publish: Box::new(|_t, _d| {}),
            roots: FsRoots {
                workspace: format!("{home}/workspace"),
                owner_roots: vec![],
                untrusted_roots: vec![],
                home: user.clone(),
                protection: Arc::new(SyncProtection(protection)),
            },
            device_id: "dev".into(),
            recovery: Arc::new(ferb_gateway::adapters::fs::FsRecovery {
                db: db.clone(),
                clock: clock.clone(),
                ids: ids.clone(),
                root: dir.path().join("recovery").to_str().unwrap().into(),
            }),
            snapshots: None,
            usage: Arc::new(NoUsage),
            apps: Arc::new(NoApps),
            describe: Arc::new(Words),
            journal_problem: Box::new(|_k, _d| {}),
            profile_name: Box::new(|_b, f| f.to_string()),
            profile_account: Box::new(|_b, _f| String::new()),
            owner_contacts: Box::new(Vec::new),
            pay_same_site_forms: Box::new(|| true),
            on_path: Box::new(|_p, _t| {}),
            proc_task: Box::new(|_p| None),
            route_question: None,
            hooks: Arc::new(NoHooks),
            made_in_tree: None,
            excluded_host: None,
        };
        let gw = Gateway::new(reg, deps, || "ap-1".to_string());
        gw.record_request(Request {
            request_id: "req".into(),
            words: String::new(),
            targets: Targets::default(),
            payload: String::new(),
            asked_origins: BTreeSet::new(),
        });
        let answer = if sc["name"] == "confirm_no" { 2 } else { 1 };
        for c in &calls {
            let task = c["task_id"].as_str().unwrap().to_string();
            let tree = task.split('.').next().unwrap().to_string();
            let span_id = {
                let db = db.lock().unwrap();
                let store = ferb_store::spans::SpanStore::new(
                    &db,
                    Box::new(StoreTime(clock.clone())),
                    Box::new(StoreIds(ids.clone())),
                    Box::new(|v: &Value| v.clone()),
                    Box::new(|_k: &str| false),
                );
                store
                    .start("task", &json!({}), Some(&task), None, None)
                    .unwrap()
                    .id
            };
            gw.add_task(TaskRecord {
                task_id: task.clone(),
                request_id: "req".into(),
                started_at: start(),
                deadline: None,
                span_id,
                tree_id: Some(tree),
            });
            let grant = RunGrant {
                task_id: task,
                worker_id: c["worker_id"].as_str().unwrap().into(),
                scopes: ["fs:read".to_string(), "fs:write".to_string()]
                    .into_iter()
                    .collect(),
                identity: Identity::Owner,
                expires_at: 1e9,
                observe_only: false,
                tools: BTreeSet::new(),
                max_calls: 0,
            };
            let args = c["arguments"].as_object().unwrap().clone();
            let reply = rt.block_on(async {
                let fut = gw.call(&grant, c["tool"].as_str().unwrap(), &args);
                tokio::pin!(fut);
                loop {
                    tokio::select! {
                        biased;
                        r = &mut fut => break r,
                        () = tokio::task::yield_now() => {
                            if let Some(ap) = gw.pending().into_iter().next() {
                                gw.resolve(&ap.id, true, Some(answer), None).unwrap();
                            }
                        }
                    }
                }
            });
            let got = reply.unwrap().to_json();
            replayed += 1;
            if let Some(d) = diff(&format!("{} {}", sc["name"], c["tool"]), &got, &c["reply"]) {
                problems.push(d);
            }
        }
    }
    assert_eq!(replayed, 8);
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}
