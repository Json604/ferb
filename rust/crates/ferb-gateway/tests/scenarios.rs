//! The gateway against `rust/fixtures/gateway/scenarios.json` (made by `make.py` from the real Python `Gateway`):
//! every call's reply, every published event, every question asked (its whole card), every hidden input's value,
//! and the stores' tables at the end, byte for byte. Mac-side: the gateway's records are on the SQLite store.
//!
//! Each run works in a folder of its own under the system's temp folder, removed when it ends: make.py's folder is
//! written `/FX` in the fixture, and what FERB hashed with an absolute path in it (P19's payload hash, an escape's
//! exact-command key) is hashed again from its recorded preimage with this run's folder.
#![cfg(target_os = "macos")]

mod support;

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex, OnceLock, Weak};

use ferb_core::interfaces::{BoxFuture, Clock, IdSource};
use ferb_core::types::{ActionFacts, ActionSpec, Target, TerminalInput, ToolContext, ToolResult};
use ferb_core::when::{self, DateTime};
use ferb_gateway::call_hooks::{HookParts, RealHooks};
use ferb_gateway::gateway::{
    CallError, Cancel, Deps, FsRoots, Gateway, HiddenError, SecretAsk, StoreIds, StoreTime,
    SyncProtection, TaskRecord, POSIX_FILES,
};
use ferb_gateway::seams::{Describe, QuestionRouter, Usage};
use ferb_gateway::tool::{Args, Facts, Registry, Tool, ToolError, ToolOutput};
use ferb_identity::owner_secrets::{Keychain, OwnerSecrets, Places};
use ferb_identity::Secret;
use ferb_policy::confirm::{Apps, AppsUnread, RunningApp};
use ferb_policy::payments::Pressed;
use ferb_policy::small::{Request, Targets};
use ferb_rpc::auth::{Identity, RunGrant};
use ferb_sanitize::{Py, Redactor};
use ferb_store::Db;
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

use support::ledger::RealLedger;

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

/// S4 (c): a folder's count, by ferb-fs's `usage` with the gateway's closed check (`usage(path, closed, True)`).
struct RealUsage;
impl Usage for RealUsage {
    fn count<'a>(
        &'a self,
        path: &'a str,
        closed: ferb_core::types::PathCheck,
    ) -> BoxFuture<'a, Map<String, Value>> {
        Box::pin(async move {
            let check = |p: &str, a: &str| (closed.0)(std::path::Path::new(p), a);
            ferb_fs::usage::usage(path, Some(&check), true).unwrap_or_default()
        })
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

/// The owner's Keychain as a dict (tests/support/hidden_input.py's keychain_helper): never the real one.
#[derive(Default)]
struct DictKeychain(Mutex<Vec<(String, String)>>);
impl Keychain for DictKeychain {
    fn store(&self, account: &str, value: &Secret) -> ferb_identity::Result<()> {
        let mut k = self.0.lock().unwrap();
        k.retain(|(a, _)| a != account);
        k.push((account.to_string(), value.expose().to_string()));
        Ok(())
    }
    fn lookup(&self, account: &str) -> ferb_identity::Result<Option<Secret>> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .iter()
            .find(|(a, _)| a == account)
            .map(|(_, v)| Secret::new(v.clone())))
    }
    fn delete(&self, _account: &str) -> ferb_identity::Result<bool> {
        Ok(false)
    }
}

/// FERB's one browser list (`is_browser`) and a browser's reading of an address (`urls.parse`).
struct BrowserPlaces;
impl Places for BrowserPlaces {
    fn is_browser(&self, bundle_id: &str) -> bool {
        [
            "com.google.Chrome",
            "com.apple.Safari",
            "company.thebrowser.Browser",
        ]
        .contains(&bundle_id)
    }
    fn url_host(&self, url: &str) -> Option<String> {
        ferb_policy::urls::parse(url)
            .map(|p| p.host)
            .filter(|h| !h.is_empty())
    }
}

/// The timer store on the scenario's database (`Timers(db, clock, ids)`).
struct DbBook {
    db: Arc<Mutex<Db>>,
    clock: Arc<dyn Clock>,
    ids: Arc<dyn IdSource>,
}
impl ferb_clock::tools::TimerBook for DbBook {
    fn add_timer(
        &self,
        kind: &str,
        at: &ferb_clock::py::Time,
        message: &str,
        task_id: &str,
        request_id: &str,
    ) -> Result<(String, ferb_clock::py::Time), String> {
        let db = self.db.lock().unwrap();
        let ids = self.ids.clone();
        let timers =
            ferb_clock::timers::Timers::new(&db, self.clock.as_ref(), move || ids.new_id());
        let out = timers.add_timer(kind, at, message, task_id, request_id);
        drop(timers);
        out
    }
}

// ---------- the fake capability (make.py's SpecCap) ----------

/// A task tab's page as the browser has it now (make.py's SpecPage).
#[derive(Clone)]
struct Page {
    url: String,
    lines: Vec<String>,
    open: bool,
    picture: Option<String>,
}

struct SpecTool {
    spec: Value,
    schema: Value,
    calls: Mutex<usize>,
    running: Arc<std::sync::atomic::AtomicBool>,
    armed: Mutex<Option<String>>,
    page: Mutex<Option<Page>>,
    amounts: Value,
    gw: Arc<OnceLock<Weak<Gateway>>>,
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

fn strs(v: &Value) -> Vec<String> {
    v.as_array()
        .map(|a| a.iter().map(|x| x.as_str().unwrap().to_string()).collect())
        .unwrap_or_default()
}

impl Tool for SpecTool {
    fn name(&self) -> &str {
        self.spec["name"].as_str().unwrap()
    }
    fn facts(&self) -> Facts {
        let flag = |k: &str| self.spec.get(k).and_then(Value::as_bool).unwrap_or(false);
        Facts {
            risk: 2,
            reversible: flag("reversible"),
            observation: flag("observation"),
            generic_input: flag("generic_input"),
            probe_safe: flag("probe_safe"),
            origin: s(&self.spec, "origin"),
            ..Facts::default()
        }
    }
    fn scopes(&self) -> BTreeSet<String> {
        match self.spec.get("scopes") {
            Some(a) => strs(a).into_iter().collect(),
            None => ["probe".to_string()].into_iter().collect(),
        }
    }
    fn identities(&self) -> BTreeSet<String> {
        match self.spec.get("identities") {
            Some(a) => strs(a).into_iter().collect(),
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
    fn try_describe(&self, args: &Args) -> Result<ActionSpec, ToolError> {
        if let Some(e) = self.spec.get("describe_raises") {
            return Err(ToolError::failed(
                e["class"].as_str().unwrap(),
                e["message"].as_str().unwrap(),
            ));
        }
        Ok(self.describe(args))
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
        let recipients = if to.is_empty() {
            vec![]
        } else if self.spec.get("many_to").is_some() {
            to.split(',').map(|r| r.trim().to_string()).collect()
        } else {
            vec![to]
        };
        let path = s(&self.spec, "path_from").and_then(|k| Some(a(&k)).filter(|p| !p.is_empty()));
        let target = Target {
            recipients,
            site,
            amount: amount.filter(|x| *x != -1.0),
            account: Some(a("account")).filter(|x| !x.is_empty()),
            contact: s(&self.spec, "contact"),
            path,
            ..Target::default()
        };
        let facts = self.spec.get("facts").map(|f| {
            let mut out = ActionFacts::default();
            for (k, v) in f.as_object().unwrap() {
                let text = || v.as_str().map(str::to_string);
                match k.as_str() {
                    "submits_form" => out.submits_form = v.as_bool(),
                    "field_kinds" => out.field_kinds = serde_json::from_value(v.clone()).unwrap(),
                    "page_url" => out.page_url = text(),
                    "page_amount" => out.page_amount = text(),
                    "page_merchant" => out.page_merchant = text(),
                    "form_method" => out.form_method = text(),
                    "form_action_host" => out.form_action_host = text(),
                    "tab_chain" => out.tab_chain = text(),
                    "control_label" => out.control_label = text(),
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
            let given = args.get("text").and_then(Value::as_str).unwrap_or("");
            if given == "RETURN" {
                // a Return key: covered by the Yes FERB just typed text under (the driver keeps that arm)
                return Some(TerminalInput {
                    app: s(t, "app").unwrap_or_default(),
                    bundle_id: s(t, "bundle_id").unwrap_or_default(),
                    surface: s(t, "surface").unwrap_or_default(),
                    keys: Some("return".into()),
                    covered_by: self.armed.lock().unwrap().clone(),
                    window: s(t, "window").unwrap_or_default(),
                    ..TerminalInput::default()
                });
            }
            let text = Some(given.to_string())
                .filter(|x| !x.is_empty())
                .or_else(|| s(t, "text"));
            Some(TerminalInput {
                app: s(t, "app").unwrap_or_default(),
                bundle_id: s(t, "bundle_id").unwrap_or_default(),
                surface: s(t, "surface").unwrap_or_default(),
                window: s(t, "window").unwrap_or_default(),
                enter: t.get("enter").and_then(Value::as_bool).unwrap_or(false),
                text,
                ..TerminalInput::default()
            })
        })
    }
    fn browser_page<'a>(&'a self, _args: &'a Args) -> BoxFuture<'a, Option<String>> {
        let page = s(&self.spec, "browser_page");
        Box::pin(async move { page })
    }
    fn pay_picture<'a>(
        &'a self,
        args: &'a Args,
        _owner: &'a str,
    ) -> BoxFuture<'a, Option<Vec<u8>>> {
        let tab = args
            .get("tab")
            .and_then(Value::as_str)
            .filter(|t| !t.is_empty());
        let shot = tab.and_then(|_| {
            self.page
                .lock()
                .unwrap()
                .as_ref()
                .and_then(|p| p.picture.clone())
        });
        Box::pin(async move { shot.map(String::into_bytes) })
    }
    fn pay_press<'a>(
        &'a self,
        args: &'a Args,
        _owner: &'a str,
        declared: Option<&'a str>,
    ) -> BoxFuture<'a, Option<Pressed>> {
        // `pay_press.read` over the page as it is now: its address, and the amounts on its lines holding the
        // approved amount's last digits (each line's amounts as the browser tools read them, recorded by make.py)
        let page = self.page.lock().unwrap().clone();
        let tab = args
            .get("tab")
            .and_then(Value::as_str)
            .filter(|t| !t.is_empty());
        let out = match (page, tab) {
            (Some(p), Some(_)) => {
                if !p.open {
                    Some(Pressed {
                        url: None,
                        amounts: None,
                    })
                } else {
                    let needle = ferb_policy::payments::digits_of(declared);
                    let mut amounts: Vec<String> = Vec::new();
                    if !needle.is_empty() {
                        let n = ferb_policy::py::casefold(&needle);
                        for line in &p.lines {
                            if ferb_policy::py::casefold(line).contains(&n) {
                                for a in strs(&self.amounts[line]) {
                                    if !amounts.contains(&a) {
                                        amounts.push(a);
                                    }
                                }
                            }
                        }
                    }
                    Some(Pressed {
                        url: Some(p.url).filter(|u| !u.is_empty()),
                        amounts: Some(amounts),
                    })
                }
            }
            _ => None,
        };
        Box::pin(async move { out })
    }
    fn run<'a>(
        &'a self,
        _args: &'a Args,
        ctx: &'a ToolContext,
    ) -> BoxFuture<'a, Result<ToolOutput, ToolError>> {
        Box::pin(async move {
            if let Some(t) = &ctx.terminal {
                *self.armed.lock().unwrap() = t.input.enter.then(|| t.confirmation_id.clone());
            }
            for n in self
                .spec
                .get("note")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default()
            {
                ferb_core::call_facts::note(n[0].as_str().unwrap(), n[1].as_str().unwrap());
            }
            if self.spec.get("hang").is_some() {
                self.running
                    .store(true, std::sync::atomic::Ordering::SeqCst);
                std::future::pending::<()>().await;
            }
            if let Some(origin) = s(&self.spec, "login_code") {
                let gw = self.gw.get().and_then(Weak::upgrade).expect("the gateway");
                let code = gw.ask_login_code(ctx, &origin).await;
                return Ok(ToolOutput::from(ToolResult {
                    content: json!({"ok": true, "code_given": matches!(code, Ok(Some(_)))})
                        .as_object()
                        .cloned()
                        .unwrap(),
                    ..ToolResult::default()
                }));
            }
            if self.spec.get("escape_echo").is_some() {
                let e = ctx.escape.as_ref().map(|e| {
                    json!({"rule": e.rule, "confirmation_id": e.confirmation_id, "command": e.command,
                           "cwd": e.cwd, "has_secret": e.secret.is_some()})
                });
                return Ok(ToolOutput::from(ToolResult {
                    content: json!({"ok": true, "escape": e})
                        .as_object()
                        .cloned()
                        .unwrap(),
                    ..ToolResult::default()
                }));
            }
            let results = self
                .spec
                .get("results")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_else(|| vec![json!({})]);
            let r = {
                let mut n = self.calls.lock().unwrap();
                let r = results[(*n).min(results.len() - 1)].clone();
                *n += 1;
                r
            };
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

/// This run's folder (make.py's `/tmp/ferbgw`, written `/FX`): a fresh folder of the same length under `/tmp` for
/// each run, removed when it ends. The same length because FERB clips long lines (a call's summary, the step in a
/// question): a path of another length would clip them elsewhere. `path` is the folder as made (`/tmp/...`, as
/// make.py names its own), `canon` as the OS resolves it (`/private/tmp/...`, what FERB's real paths read).
struct Root {
    _dir: tempfile::TempDir,
    path: String,
    canon: String,
}

fn root() -> Root {
    let dir = tempfile::Builder::new()
        .prefix("fg")
        .rand_bytes(4)
        .tempdir_in("/tmp")
        .unwrap();
    let path = dir.path().to_str().unwrap().to_string();
    assert_eq!(path.len(), "/tmp/ferbgw".len());
    let canon = std::fs::canonicalize(dir.path())
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    Root {
        _dir: dir,
        path,
        canon,
    }
}

struct Run {
    gw: Arc<Gateway>,
    db: Arc<Mutex<Db>>,
    clock: Arc<FakeClock>,
    events: Arc<Mutex<Vec<Value>>>,
    journal: Arc<Mutex<Vec<Value>>>,
    ledger: Arc<RealLedger>,
    ids: Arc<dyn IdSource>,
    tools: Vec<Arc<SpecTool>>,
    secrets: Arc<OwnerSecrets>,
    keychain: Arc<DictKeychain>,
    redactor: Arc<Redactor>,
    grants: Vec<(String, RunGrant)>,
    /// Each task's request.
    requests: Vec<(String, String)>,
    last_retry: String,
    span: String,
    orphan: String,
    home: String,
    /// The folder as made, then as resolved (masked as `/FX`, longest first).
    roots: Vec<String>,
}

fn start() -> DateTime {
    when::parse("2026-01-01T09:00:00+00:00").unwrap()
}

fn build(sc: &Value, root: &Root) -> Run {
    let dir = std::path::PathBuf::from(&root.path);
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
    let ledger = Arc::new(RealLedger {
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
    let gw_cell: Arc<OnceLock<Weak<Gateway>>> = Arc::new(OnceLock::new());
    let mut reg = Registry::new();
    let mut tools = Vec::new();
    let schemas = &sc["tool_schemas"];
    let amounts = sc.get("page_amounts").cloned().unwrap_or(json!({}));
    for t in sc
        .get("tools")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
    {
        let name = t["name"].as_str().unwrap().to_string();
        let page = t.get("page").map(|p| Page {
            url: s(p, "url").unwrap_or_default(),
            lines: strs(&p["lines"]),
            open: p.get("open").and_then(Value::as_bool).unwrap_or(true),
            picture: s(p, "picture"),
        });
        let tool = Arc::new(SpecTool {
            schema: schemas[&name].clone(),
            spec: t,
            calls: Mutex::new(0),
            running: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            armed: Mutex::new(None),
            page: Mutex::new(page),
            amounts: amounts.clone(),
            gw: gw_cell.clone(),
        });
        tools.push(tool.clone());
        reg.add(tool).unwrap();
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
    if sc.get("time_tools").is_some_and(|v| v == true) {
        // the real timer tools on this store (ferb/clock/tools.py), through the adapter
        for name in [ferb_clock::tools::SET_TIMER, ferb_clock::tools::REMIND_AT] {
            reg.add(Arc::new(ferb_gateway::adapters::TimeTool {
                name,
                book: Some(Arc::new(DbBook {
                    db: db.clone(),
                    clock: clock.clone(),
                    ids: ids.clone(),
                })),
                clock: clock.clone(),
                zone: ferb_clock::zone::Zone::utc(),
            }))
            .unwrap();
        }
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
    let own: Vec<String> = strs(sc.get("own").unwrap_or(&Value::Null));
    let pay_same = sc
        .get("pay_same_site_forms")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let procs = sc.get("procs").cloned().unwrap_or(json!({}));
    let made: Option<Vec<String>> = sc.get("made").map(|m| {
        strs(m)
            .into_iter()
            .map(|p| {
                let p = p.replace("$HOME", home.to_str().unwrap());
                std::fs::canonicalize(&p)
                    .map(|c| c.to_str().unwrap().to_string())
                    .unwrap_or(p)
            })
            .collect()
    });
    let keychain = Arc::new(DictKeychain::default());
    let secrets = Arc::new(OwnerSecrets::with_keychain(
        &home,
        keychain.clone(),
        Arc::new(BrowserPlaces),
    ));
    let ev = events.clone();
    let jr = journal.clone();
    // the daemon's events (ferb/daemon/events.py) redact each event once, FERB's own marked words kept as built
    let bus = sc
        .get("bus_redacts")
        .is_some_and(|v| v == true)
        .then(|| redactor.clone());
    let deps = Deps {
        db: db.clone(),
        clock: clock.clone(),
        ids: ids.clone(),
        redactor: redactor.clone(),
        ledger: ledger.clone(),
        publish: Box::new(move |topic, data: Py| {
            let data = match &bus {
                Some(r) => r.py(&data),
                None => data,
            };
            ev.lock()
                .unwrap()
                .push(json!([topic, ferb_gateway::gateway::py_value(&data)]))
        }),
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
        usage: Arc::new(RealUsage),
        apps: Arc::new(NoApps),
        describe: Arc::new(Words),
        journal_problem: Box::new(move |kind, data| jr.lock().unwrap().push(json!([kind, data]))),
        profile_name: Box::new(|_b, f| f.to_string()),
        profile_account: Box::new(|_b, _f| String::new()),
        owner_contacts: Box::new(move || own.clone()),
        pay_same_site_forms: Box::new(move || pay_same),
        on_path: Box::new(|_p, _t| {}),
        proc_task: Box::new(move |p| procs.get(p).and_then(Value::as_str).map(str::to_string)),
        route_question: Some(Arc::new(Router(route))),
        // the real hooks with nothing wired, as make.py's Gateway has them (no sentinel, evidence, changes, ...)
        hooks: Arc::new(RealHooks::new(HookParts {
            sentinel: None,
            previews: None,
            evidence: None,
            changes: None,
            images: None,
            app_windows: None,
            shadow: None,
            publish: Arc::new(|_t, _d| {}),
            journal: Arc::new(|_k, _d| {}),
            workspace: home.join("workspace").to_str().unwrap().to_string(),
            home: user.to_str().unwrap().to_string(),
        })),
        made_in_tree: made.map(|m| {
            Box::new(move |_root: &str, path: &str| m.iter().any(|x| x == path))
                as ferb_gateway::gateway::MadeHere
        }),
        excluded_host: None,
        owner_secrets: Some(secrets.clone()),
        places: Arc::new(BrowserPlaces),
        username_holds: None,
    };
    let counter = Mutex::new(0u64);
    let gw = Arc::new(Gateway::new(reg, deps, move || {
        let mut n = counter.lock().unwrap();
        *n += 1;
        format!("ap-a{:011}", *n)
    }));
    let _ = gw_cell.set(Arc::downgrade(&gw));
    Run {
        gw,
        db,
        clock,
        events,
        journal,
        ledger,
        ids,
        tools,
        secrets,
        keychain,
        redactor,
        grants: Vec::new(),
        requests: Vec::new(),
        last_retry: String::new(),
        span: String::new(),
        orphan: String::new(),
        home: home.to_str().unwrap().to_string(),
        roots: vec![root.canon.clone(), root.path.clone()],
    }
}

fn store<'a>(r: &Run, db: &'a Db) -> ferb_store::spans::SpanStore<'a> {
    let red = r.redactor.clone();
    ferb_store::spans::SpanStore::new(
        db,
        Box::new(StoreTime(r.clock.clone())),
        Box::new(StoreIds(r.ids.clone())),
        Box::new(move |v: &Value| red.value(v)),
        Box::new(|_k: &str| false),
    )
}

fn setup(r: &mut Run, sc: &Value) {
    let requests = sc
        .get("requests")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_else(|| vec![json!({"id": "req-1", "words": ""})]);
    for q in requests {
        let sites: BTreeSet<String> = strs(q.get("sites").unwrap_or(&Value::Null))
            .into_iter()
            .collect();
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
        let scopes: BTreeSet<String> = match t.get("scopes") {
            Some(a) => strs(a).into_iter().collect(),
            None => ["probe".to_string()].into_iter().collect(),
        };
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
                    let input = t.get("span_input").cloned().unwrap_or(json!({}));
                    store(r, &db)
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
        r.requests.push((id.clone(), request.clone()));
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
            tools: strs(t.get("tools").unwrap_or(&Value::Null))
                .into_iter()
                .collect(),
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
        let d = ferb_policy::stores::KnownDestinations {
            db: &db,
            clock: r.clock.as_ref(),
            ids: r.ids.as_ref(),
            redact: r.redactor.as_ref(),
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

/// What a step's job gives back once driven to its end.
enum Done {
    Reply(Result<ferb_gateway::reply::Reply, CallError>),
    Hidden(Result<Option<Secret>, HiddenError>),
    Left,
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

    fn tool(&self, name: &str) -> Arc<SpecTool> {
        self.tools
            .iter()
            .find(|t| t.name() == name)
            .unwrap()
            .clone()
    }

    fn set_page(&self, change: &Value) {
        let tool = self.tool(change["tool"].as_str().unwrap());
        let mut page = tool.page.lock().unwrap();
        let p = page.as_mut().unwrap();
        if let Some(u) = s(change, "url") {
            p.url = u;
        }
        if change.get("lines").is_some() {
            p.lines = strs(&change["lines"]);
        }
    }

    /// One answer to a question the job asked (make.py `answer`); true: the waiter left (`leave`).
    fn answer(
        &self,
        ap: &ferb_gateway::questions::Approval,
        a: &Value,
        cancel: &Cancel,
        taken: &mut Vec<Value>,
    ) -> bool {
        if let Some(change) = a.get("page") {
            self.set_page(change);
        }
        if a.get("take").is_some() {
            let first = self.gw.take_picture(&ap.id);
            let again = self.gw.take_picture(&ap.id);
            let text = |b: Option<Vec<u8>>| {
                b.map_or(Value::Null, |b| json!(String::from_utf8(b).unwrap()))
            };
            taken.push(json!([text(first), text(again)]));
        }
        if let Some(p) = a.get("pick") {
            self.gw
                .resolve(&ap.id, true, Some(p.as_u64().unwrap() as usize), None)
                .unwrap();
        } else if let Some(c) = a.get("choice") {
            let i = ap
                .options
                .iter()
                .position(|o| o == c.as_str().unwrap())
                .expect("an option")
                + 1;
            self.gw.resolve(&ap.id, true, Some(i), None).unwrap();
        } else if let Some(t) = a.get("text") {
            self.gw.resolve(&ap.id, true, None, t.as_str()).unwrap();
        } else if a.get("deny").is_some() {
            self.gw.resolve(&ap.id, false, None, None).unwrap();
        } else if a.get("supersede").is_some() {
            self.gw.supersede(&ap.id);
        } else if a.get("cancel").is_some() {
            self.gw
                .cancel_tasks(&[ap.task_id.clone()].into_iter().collect());
        } else if a.get("leave").is_some() {
            cancel.cancel();
            return true;
        } else {
            panic!("{a}");
        }
        false
    }

    /// Drives `job` to its end, answering each question with the step's answers in order (make.py `drive`).
    async fn drive(
        &self,
        step: &Value,
        job: impl std::future::Future<Output = Done>,
        cancel: Cancel,
        drop_on_leave: bool,
    ) -> (Value, Done) {
        let mut answers: Vec<Value> = step["answers"].as_array().cloned().unwrap_or_default();
        answers.reverse();
        let grant = self.grant(
            step.get("grant")
                .and_then(Value::as_str)
                .unwrap_or("task-1"),
        );
        tokio::pin!(job);
        let mut asked = Vec::new();
        let mut taken = Vec::new();
        let mut bad = Vec::new();
        let mut seen: BTreeSet<String> = BTreeSet::new();
        let done = loop {
            tokio::select! {
                biased;
                r = &mut job => break r,
                () = tokio::task::yield_now() => {
                    if step.get("cancel_running").is_some_and(|v| v == true)
                        && self.tools.iter().any(|t| t.running.load(std::sync::atomic::Ordering::SeqCst))
                    {
                        for t in &self.tools {
                            t.running.store(false, std::sync::atomic::Ordering::SeqCst);
                        }
                        self.gw.cancel_tasks(&[grant.task_id.clone()].into_iter().collect());
                    }
                    let fresh: Vec<_> = self.gw.pending().into_iter().filter(|a| !seen.contains(&a.id)).collect();
                    if let Some(ap) = fresh.into_iter().next() {
                        seen.insert(ap.id.clone());
                        asked.push(approval_json(&ap));
                        let mut a = answers.pop().unwrap_or_else(|| panic!("unanswered question {}", ap.reason));
                        while let Some(t) = a.get("bad_text") {
                            match self.gw.resolve(&ap.id, true, None, t.as_str()) {
                                Err(ferb_gateway::questions::ResolveError::Invalid(m)) => bad.push(json!(m)),
                                other => panic!("an answer the question refuses was taken: {other:?}"),
                            }
                            a = answers.pop().unwrap();
                        }
                        if self.answer(&ap, &a, &cancel, &mut taken) && drop_on_leave {
                            break Done::Left;
                        }
                    }
                }
            }
        };
        assert!(answers.is_empty(), "answers left {answers:?}");
        let mut out = json!({"asked": asked});
        if !taken.is_empty() {
            out["taken"] = json!(taken);
        }
        if !bad.is_empty() {
            out["refused_answers"] = json!(bad);
        }
        (out, done)
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
        let start = self.events.lock().unwrap().len();
        let cancel = Cancel::new();
        let left = step.get("caller_left").is_some_and(|v| v == true);
        let gw = self.gw.clone();
        let name = step["tool"].as_str().unwrap().to_string();
        let job = async {
            Done::Reply(
                gw.call_as(
                    &grant,
                    &name,
                    &arguments,
                    cancel.clone(),
                    Arc::new(move || left),
                )
                .await,
            )
        };
        let (mut out, done) = self.drive(step, job, cancel.clone(), false).await;
        match done {
            Done::Reply(Ok(reply)) => {
                if let Some(Value::String(r)) = reply.content.get("retry_action") {
                    self.last_retry = r.clone();
                }
                out["reply"] = reply.to_json();
            }
            Done::Reply(Err(CallError::Cancelled)) => out["raised"] = json!("CancelledError"),
            Done::Reply(Err(CallError::Internal(e))) => out["raised"] = json!(e),
            _ => unreachable!(),
        }
        out["events"] = json!(self.events.lock().unwrap()[start..].to_vec());
        out
    }

    fn ctx(&self, step: &Value) -> ToolContext {
        let grant = self.grant(
            step.get("grant")
                .and_then(Value::as_str)
                .unwrap_or("task-1"),
        );
        let request = self
            .requests
            .iter()
            .find(|(t, _)| *t == grant.task_id)
            .map(|r| r.1.clone())
            .unwrap();
        let span = match step.get("span").and_then(Value::as_str) {
            Some("$span") => Some(self.span.clone()),
            Some(other) => Some(other.to_string()),
            None => None,
        };
        ToolContext {
            task_id: grant.task_id.clone(),
            worker_id: grant.worker_id.clone(),
            request_id: request,
            identity: ferb_core::types::Identity::Owner,
            scopes: grant.scopes.clone(),
            device_id: "dev".into(),
            span_id: span,
            workspace: format!("{}/workspace", self.home),
            owner_roots: vec![],
            untrusted_roots: vec![],
            intent_id: None,
            observe_only: false,
            probe: false,
            path_closed: None,
            escape: None,
            terminal: None,
            folders: vec![],
        }
    }

    async fn hidden(&mut self, step: &Value) -> Value {
        let ctx = self.ctx(step);
        let start = self.events.lock().unwrap().len();
        let gw = self.gw.clone();
        let kw = step.get("kw").cloned().unwrap_or(json!({}));
        let ask = SecretAsk {
            handle: s(&kw, "handle"),
            remember: s(&kw, "remember"),
            shown: s(&kw, "shown"),
            field: s(&kw, "field"),
            kind: s(&kw, "kind"),
        };
        let target = s(step, "target").or_else(|| s(step, "origin")).unwrap();
        let secret = step["do"] == "ask_secret";
        let job = async {
            Done::Hidden(if secret {
                gw.ask_secret(&ctx, &target, ask).await
            } else {
                gw.ask_login_code(&ctx, &target).await
            })
        };
        // the waiter leaving is the job dropped (Python cancels its task)
        let (mut out, done) = self.drive(step, job, Cancel::new(), true).await;
        match done {
            Done::Hidden(Ok(v)) => out["returned"] = v.map_or(Value::Null, |s| json!(s.expose())),
            Done::Hidden(Err(HiddenError::Refused(m))) => {
                out["raised"] = json!(format!("Refused: {m}"))
            }
            Done::Hidden(Err(e)) => out["raised"] = json!(format!("{e:?}")),
            Done::Left => out["raised"] = json!("CancelledError"),
            Done::Reply(_) => unreachable!(),
        }
        out["events"] = json!(self.events.lock().unwrap()[start..].to_vec());
        out
    }

    async fn step(&mut self, step: &Value) -> Value {
        let start = self.events.lock().unwrap().len();
        let ids = |v: &Value| -> BTreeSet<String> { strs(v).into_iter().collect() };
        match step["do"].as_str().unwrap() {
            "call" => return self.call(step).await,
            "ask_secret" | "ask_login_code" => return self.hidden(step).await,
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
            "probe" => {
                let grant = self.grant(
                    step.get("grant")
                        .and_then(Value::as_str)
                        .unwrap_or("task-1"),
                );
                let args: Map<String, Value> = step
                    .get("arguments")
                    .and_then(Value::as_object)
                    .cloned()
                    .unwrap_or_default()
                    .into_iter()
                    .map(|(k, v)| match v {
                        Value::String(t) => (k, Value::String(t.replace("$HOME", &self.home))),
                        other => (k, other),
                    })
                    .collect();
                let got = self
                    .gw
                    .probe(
                        &grant,
                        step["tool"].as_str().unwrap(),
                        &args,
                        step.get("span_id")
                            .and_then(Value::as_str)
                            .unwrap_or("span-loop"),
                    )
                    .await
                    .unwrap();
                return json!({"probe": got, "events": self.events.lock().unwrap()[start..].to_vec()});
            }
            "destination" => {
                let (ok, why) = self
                    .gw
                    .destination_allowed(
                        step["proc"].as_str().unwrap(),
                        step["host"].as_str().unwrap(),
                        step.get("port").and_then(Value::as_u64).unwrap_or(443) as u16,
                        step.get("local_approved").is_some_and(|v| v == true),
                    )
                    .unwrap();
                return json!({"allowed": [ok, why]});
            }
            "take_picture" => {
                let id = step["id"].as_str().unwrap();
                let image = self.gw.take_picture(id);
                return json!({"picture": image.map(|b| String::from_utf8(b).unwrap()), "held": self.gw.has_picture(id)});
            }
            "approvals" => {
                let all: Vec<Value> = self.gw.approvals().iter().map(approval_json).collect();
                return json!({"approvals": all});
            }
            "span" => {
                let grant = self.grant(
                    step.get("grant")
                        .and_then(Value::as_str)
                        .unwrap_or("task-1"),
                );
                let parent: String = {
                    let db = self.db.lock().unwrap();
                    db.conn()
                        .query_row(
                            "SELECT id FROM spans WHERE kind = 'task' AND task_id = ?",
                            [&grant.task_id],
                            |x| x.get(0),
                        )
                        .unwrap()
                };
                let id = {
                    let db = self.db.lock().unwrap();
                    let spans = store(self, &db);
                    let id = spans
                        .start(
                            "tool.call",
                            &step["input"],
                            Some(&grant.task_id),
                            Some(&parent),
                            Some("dev"),
                        )
                        .unwrap()
                        .id;
                    drop(spans);
                    id
                };
                self.span = id.clone();
                return json!({"span": id});
            }
            "username" => self
                .secrets
                .set_username(
                    step["target"].as_str().unwrap(),
                    step["name"].as_str().unwrap(),
                    step["source"].as_str().unwrap(),
                )
                .unwrap(),
            "page" => self.set_page(step),
            "orphan" => {
                // what a daemon that died mid-action left: the latest row's identical action, still executing
                let (task, capability, target, hash): (String, String, String, String) = {
                    let db = self.db.lock().unwrap();
                    db.conn()
                        .query_row(
                            "SELECT task_id, capability, target_json, payload_hash FROM action_ledger ORDER BY rowid DESC LIMIT 1",
                            [],
                            |x| Ok((x.get(0)?, x.get(1)?, x.get(2)?, x.get(3)?)),
                        )
                        .unwrap()
                };
                let target: Value = serde_json::from_str(&target).unwrap();
                let id = self
                    .ledger
                    .with(|l| {
                        let i = l.intend(&task, &capability, &target, &hash, false)?;
                        l.mark(&i.id, "executing", &["span-before-the-restart".to_string()])?;
                        Ok::<_, ferb_orchestrator::sql::Error>(i.id)
                    })
                    .unwrap();
                self.orphan = id.clone();
                return json!({"orphan": id});
            }
            "reconcile" => {
                let evidence = strs(&step["evidence"]);
                let row = self
                    .ledger
                    .with(|l| {
                        l.reconcile(&self.orphan, step["observed"].as_str().unwrap(), &evidence)?;
                        l.get(&self.orphan)
                    })
                    .unwrap();
                let i = &row.intent;
                return json!({"row": {
                    "intent": {"id": i.id, "task_id": i.task_id, "capability": i.capability, "target": i.target,
                               "payload_hash": i.payload_hash, "state": i.state},
                    "reconciled": row.reconciled, "evidence": row.evidence, "retry_approved": row.retry_approved,
                    "retried_by": row.retried_by}});
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

/// make.py's own folder, as made and as resolved.
const PY_ROOTS: [&str; 2] = ["/private/tmp/ferbgw", "/tmp/ferbgw"];

/// make.py's /FX for the run's folder, and "<volatile>" for a file's identity facts (both sides).
fn masked(v: &Value, roots: &[String], keys: &[String]) -> Value {
    match v {
        Value::String(t) => {
            let mut t = t.clone();
            for r in roots {
                t = t.replace(r.as_str(), "/FX");
            }
            // a root a long line was clipped inside ("/tmp/fgA…"): where it differs from make.py's, it is the root
            for r in roots.iter().map(String::as_str).chain(PY_ROOTS) {
                let common = if r.starts_with("/private/") {
                    "/private/tmp/".len()
                } else {
                    "/tmp/".len()
                };
                for k in (common + 1..r.len()).rev() {
                    t = t.replace(&format!("{}…", &r[..k]), "/FX…");
                }
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
        let cols = strs(&t["columns"]);
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

/// make.py's `unordered`: rows written in Python's set order, compared as a set with those columns masked.
fn unordered(tables: &mut Value, spec: Option<&Value>) {
    let Some(spec) = spec.and_then(Value::as_object) else {
        return;
    };
    for (table, cols) in spec {
        let t = &mut tables[table];
        let names = strs(&t["columns"]);
        let at: Vec<usize> = strs(cols)
            .iter()
            .map(|c| names.iter().position(|n| n == c).unwrap())
            .collect();
        let mut rows: Vec<Value> = t["rows"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| {
                Value::Array(
                    r.as_array()
                        .unwrap()
                        .iter()
                        .enumerate()
                        .map(|(i, v)| {
                            if at.contains(&i) {
                                json!("<set order>")
                            } else {
                                v.clone()
                            }
                        })
                        .collect(),
                )
            })
            .collect();
        rows.sort_by_key(ferb_py::json::dumps);
        t["rows"] = Value::Array(rows);
    }
}

/// The scenario with every hash of a path in make.py's folder hashed again with this run's (`hash_inputs`).
fn rehashed(sc: &Value, root: &Root) -> Value {
    let Some(inputs) = sc.get("hash_inputs").and_then(Value::as_object) else {
        return sc.clone();
    };
    let mut text = sc.to_string();
    for (digest, pre) in inputs {
        let pre = pre
            .as_str()
            .unwrap()
            .replace("<real>", &root.canon)
            .replace("<made>", &root.path);
        let now = hex::encode(Sha256::digest(pre.as_bytes()));
        text = text.replace(digest.as_str(), &now);
    }
    serde_json::from_str(&text).unwrap()
}

/// Every stored byte of the scenario's home (the database and its journal files), every reply, event, journal entry
/// and table holds no hidden value but where it belongs (a hidden input's own return).
fn canary_absent(r: &Run, sc: &Value, results: &[Value], tables: &Value) -> Option<String> {
    let canary = sc.get("canary").and_then(Value::as_str)?;
    for f in std::fs::read_dir(&r.home).unwrap() {
        let f = f.unwrap().path();
        if f.is_file() && f.file_name().unwrap() != "owner-secret-handles.json" {
            let bytes = std::fs::read(&f).unwrap();
            if bytes.windows(canary.len()).any(|w| w == canary.as_bytes()) {
                return Some(format!("the hidden value is in {}", f.display()));
            }
        }
    }
    let shown: Vec<Value> = results
        .iter()
        .map(|x| {
            let mut x = x.clone();
            if let Some(o) = x.as_object_mut() {
                o.shift_remove("returned");
            }
            x
        })
        .collect();
    let all = json!([shown, tables, r.journal.lock().unwrap().clone()]).to_string();
    all.contains(canary)
        .then(|| "the hidden value reached a record".to_string())
}

async fn replay(sc: &Value, root: &Root) -> Vec<String> {
    let sc = &rehashed(sc, root);
    let mut r = build(sc, root);
    setup(&mut r, sc);
    let mut problems = Vec::new();
    let name = sc["name"].as_str().unwrap();
    let keys: Vec<String> = strs(&sc["volatile"]);
    let none: Vec<String> = Vec::new();
    let roots = r.roots.clone();
    let mut results = Vec::new();
    for (i, (step, want)) in sc["steps"]
        .as_array()
        .unwrap()
        .iter()
        .zip(sc["results"].as_array().unwrap())
        .enumerate()
    {
        let got = r.step(step).await;
        results.push(got.clone());
        let got = masked(&got, &roots, &keys);
        let want = masked(want, &none, &keys);
        if let Some(d) = diff(&format!("{name} step {i}"), &got, &want) {
            problems.push(d);
        }
    }
    let raw_tables = r.tables(&sc["tables"]);
    let mut tables = masked_tables(&masked(&raw_tables, &roots, &keys), &keys);
    unordered(&mut tables, sc.get("unordered"));
    let want_tables = masked_tables(&masked(&sc["tables"], &none, &keys), &keys);
    if let Some(d) = diff(&format!("{name} tables"), &tables, &want_tables) {
        problems.push(d);
    }
    let journal = json!(r.journal.lock().unwrap().clone());
    if let Some(d) = diff(&format!("{name} journal"), &journal, &sc["journal"]) {
        problems.push(d);
    }
    if let Some(want) = sc.get("secrets") {
        let file = std::fs::read_to_string(format!("{}/owner-secret-handles.json", r.home)).ok();
        let mut keychain = r.keychain.0.lock().unwrap().clone();
        keychain.sort();
        let kc: Map<String, Value> = keychain.into_iter().map(|(a, v)| (a, json!(v))).collect();
        let got = json!({"file": file, "keychain": kc});
        if let Some(d) = diff(&format!("{name} secrets"), &got, want) {
            problems.push(d);
        }
    }
    if let Some(p) = canary_absent(&r, sc, &results, &raw_tables) {
        problems.push(format!("{name}: {p}"));
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
    let root = root();
    let mut failed = Vec::new();
    let mut count = 0;
    for sc in f["scenarios"].as_array().unwrap() {
        if only.as_deref().is_some_and(|o| o != sc["name"]) {
            continue;
        }
        count += 1;
        let problems = rt.block_on(replay(sc, &root));
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

/// `_known_from` (SQ6, GATE-205): Python records a Yes's destinations in its set's order, which changes from run to
/// run (string hashing is seeded per process), so the fixture compares them as a set. Rust records them in the
/// order the needs list them (first seen), the same on every run: the ids follow the recipients as given.
// decision: GATE-205
#[test]
fn a_yes_records_its_destinations_in_the_order_the_needs_give_them() {
    let f = fixture();
    let sc = f["scenarios"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["name"] == "known_from_records_each_destination")
        .unwrap()
        .clone();
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let root = root();
    let mut r = build(&sc, &root);
    setup(&mut r, &sc);
    rt.block_on(r.step(&sc["steps"][0]));
    let db = r.db.lock().unwrap();
    let mut st = db
        .conn()
        .prepare("SELECT value FROM known_destinations ORDER BY rowid")
        .unwrap();
    let values: Vec<String> = st
        .query_map([], |x| x.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(values, ["b@x.test", "a@x.test", "c@x.test"]);
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
    let root = root();
    let mut r = build(&sc, &root);
    setup(&mut r, &sc);
    // a modification time whose nanoseconds pass the Luhn check
    let mut ns: u64 = 1_794_538_250_392_642_000;
    while !luhn(&ns.to_string()) {
        ns += 1;
    }
    let path = format!("{}/user/card.txt", root.path);
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
            ledger: Arc::new(RealLedger {
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
            usage: Arc::new(RealUsage),
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
            hooks: Arc::new(ferb_gateway::seams::NoHooks),
            made_in_tree: None,
            excluded_host: None,
            owner_secrets: None,
            places: Arc::new(BrowserPlaces),
            username_holds: None,
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

/// A gateway with no tools on a fresh store, its events kept in `events`.
fn bare_gateway(events: Arc<Mutex<Vec<(String, Value)>>>) -> (Gateway, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let user = dir.path().join("user").to_str().unwrap().to_string();
    let home = dir.path().join("home").to_str().unwrap().to_string();
    let db = Db::open(dir.path().join("ferb.db")).unwrap();
    db.migrate().unwrap();
    let db = Arc::new(Mutex::new(db));
    let clock = Arc::new(FakeClock(Mutex::new(start())));
    let ids: Arc<dyn IdSource> = Arc::new(Seq(Mutex::new(0)));
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
        ferb_policy::protection::Protection::new(&user, &home, &lists, None, &POSIX_FILES).unwrap();
    let deps = Deps {
        db: db.clone(),
        clock: clock.clone(),
        ids: ids.clone(),
        redactor: Arc::new(Redactor::new(&[], &[])),
        ledger: Arc::new(RealLedger {
            db: db.clone(),
            clock: clock.clone(),
            ids: ids.clone(),
        }),
        publish: Box::new(move |t, d| {
            events
                .lock()
                .unwrap()
                .push((t.to_string(), ferb_gateway::gateway::py_value(&d)))
        }),
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
        usage: Arc::new(RealUsage),
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
        hooks: Arc::new(ferb_gateway::seams::NoHooks),
        made_in_tree: None,
        excluded_host: None,
        owner_secrets: None,
        places: Arc::new(BrowserPlaces),
        username_holds: None,
    };
    let mut n = 0;
    let gw = Gateway::new(Registry::new(), deps, move || {
        n += 1;
        format!("ap-{n}")
    });
    (gw, dir)
}

/// A task's question in the one store (the supervisor's `_ask_and_wait`): its task, part and flags kept on it; the
/// published event extended or left to the caller; the same id counter as every other question.
#[test]
fn a_tasks_question_lives_in_the_gateways_store() {
    use ferb_gateway::gateway::{OwnEvent, OwnQuestion};
    let events = Arc::new(Mutex::new(Vec::new()));
    let (gw, _dir) = bare_gateway(events.clone());
    let mut extra = serde_json::Map::new();
    extra.insert("question_kind".into(), json!("check_in"));
    let ap = gw.ask_own(&OwnQuestion {
        capability: "plan.ask".into(),
        reason: "(from the task) Which file?".into(),
        options: vec!["a.txt".into(), "b.txt".into()],
        request_id: "req-1".into(),
        task_id: "t1".into(),
        part: "p2".into(),
        from_task: true,
        corrects: true,
        exact: "a.txt".into(),
        event: OwnEvent::Extend(extra),
        ..OwnQuestion::default()
    });
    assert_eq!(ap.id, "ap-1");
    assert_eq!((ap.task_id.as_str(), ap.part.as_str()), ("t1", "p2"));
    assert!(ap.from_task && ap.corrects && !ap.options_only);
    {
        let ev = events.lock().unwrap();
        assert_eq!(ev.len(), 1);
        assert_eq!(ev[0].1["task_id"], "t1");
        assert_eq!(ev[0].1["question_kind"], "check_in");
    }
    let mut switch = serde_json::Map::new();
    switch.insert("runtime".into(), json!("claude"));
    let quiet = gw.ask_own(&OwnQuestion {
        capability: "task.check_in".into(),
        reason: "Switch?".into(),
        options: vec!["Yes".into(), "No".into()],
        request_id: "req-1".into(),
        task_id: "t1".into(),
        options_only: true,
        runtime_switch: Some(switch.clone()),
        event: OwnEvent::Quiet,
        ..OwnQuestion::default()
    });
    assert_eq!(quiet.id, "ap-2");
    assert_eq!(
        events.lock().unwrap().len(),
        1,
        "a quiet question publishes nothing"
    );
    assert_eq!(gw.approval("ap-2").unwrap().runtime_switch, Some(switch));
    assert_eq!(gw.question_card("ap-2")["options_only"], json!(true));
    assert_eq!(gw.pending().len(), 2);
    gw.resolve("ap-1", true, None, Some("my words")).unwrap();
    assert!(gw.set_hidden("ap-1", &["password".into()]));
    assert_eq!(
        gw.approval("ap-1").unwrap().hidden,
        vec!["password".to_string()]
    );
    assert!(!gw.set_hidden("ap-9", &[]));
}

#[test]
fn ferbs_own_questions_are_asked_answered_and_set_aside() {
    use ferb_gateway::gateway::OwnQuestion;
    let events = Arc::new(Mutex::new(Vec::new()));
    let (gw, _dir) = bare_gateway(events.clone());
    let q = OwnQuestion {
        capability: "memory.forget".into(),
        reason: "Forget for good?\nThe note about the invented plant.".into(),
        options: vec!["Yes".into(), "No".into()],
        request_id: "own-1".into(),
        approval_id: None,
        summary: Some("Forget for good?".into()),
        detail: Some("The note about the invented plant.".into()),
        labels: vec!["Forget".into(), "Keep".into()],
        ..OwnQuestion::default()
    };
    let ap = gw.ask_own(&q);
    assert_eq!(ap.id, "ap-1");
    assert!(gw.waiting("ap-1") && !gw.waiting("ap-2"));
    {
        let ev = events.lock().unwrap();
        assert_eq!(ev[0].0, "approval.requested");
        let d = &ev[0].1;
        assert_eq!(d["approval_id"], "ap-1");
        assert_eq!(d["capability"], "memory.forget");
        assert_eq!(d["kind"], "owner_question");
        assert_eq!(d["task_id"], Value::Null);
        assert_eq!(d["options"], json!(["Yes", "No"]));
        assert_eq!(d["summary"], "Forget for good?");
        assert_eq!(d["labels"], json!(["Forget", "Keep"]));
    }
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let answered = rt.block_on(async {
        let cancel = Cancel::new();
        let wait = gw.await_own("ap-1", &cancel);
        tokio::pin!(wait);
        tokio::select! {
            biased;
            _ = &mut wait => panic!("answered before the owner answered"),
            () = tokio::task::yield_now() => {}
        }
        gw.resolve("ap-1", true, None, Some("yes")).unwrap();
        wait.await.unwrap().unwrap()
    });
    assert_eq!(answered.choice.as_deref(), Some("Yes"));
    // asked again after a restart: the same id; set aside, never a No
    let again = gw.ask_own(&OwnQuestion {
        approval_id: Some("ap-9".into()),
        ..q
    });
    assert_eq!(again.id, "ap-9");
    assert!(gw.close_own("ap-9"));
    assert!(!gw.close_own("ap-9"), "closed once");
    assert_eq!(
        gw.approval("ap-9").unwrap().state,
        ferb_gateway::questions::State::Superseded
    );
    {
        let ev = events.lock().unwrap();
        assert_eq!(ev.last().unwrap().0, "approval.resolved");
        assert_eq!(ev.last().unwrap().1["state"], "superseded");
    }
    // a cancelled wait leaves the question open
    let open = gw.ask_own(&OwnQuestion {
        request_id: "own-2".into(),
        approval_id: None,
        ..OwnQuestion::default()
    });
    let cancel = Cancel::new();
    cancel.cancel();
    assert_eq!(
        rt.block_on(gw.await_own(&open.id, &cancel)),
        Err(CallError::Cancelled)
    );
    assert!(gw.waiting(&open.id));
    gw.forget_request("own-2");
    assert_eq!(
        rt.block_on(gw.await_own(&open.id, &Cancel::new())),
        Ok(None)
    );
}
