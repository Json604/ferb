//! The real gateway (ferb-gateway) in the replay, as the daemon wires it (areas/compose.py): its call pipeline on the
//! scenario's store, the orchestrator's real action ledger, the change record (ferb-fs `changes`) behind its hooks,
//! and scripted tools that answer with the tool results the Python daemon recorded (`gateway_calls`), so every
//! record the gateway itself makes (its `tool.called` and `action.observed` events, its spans, its ledger rows, the
//! ids they take) comes from real code.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use ferb_core::interfaces::{BoxFuture, Clock, IdSource};
use ferb_core::types::{ActionSpec, ToolContext, ToolResult};
use ferb_gateway::call_hooks::{ChangeRecord, HookParts, RealHooks};
use ferb_gateway::gateway::{Deps, FsRoots, Gateway, SyncProtection, POSIX_FILES};
use ferb_gateway::seams::{BadTransition, Describe, DuplicateUnresolved, Ledger, Usage};
use ferb_gateway::tool::{Args, Registry, Tool, ToolError, ToolOutput};
use ferb_gateway::words::LedgerRow;
use ferb_identity::owner_secrets::Places;
use ferb_orchestrator::ledger;
use ferb_policy::confirm::{Apps, AppsUnread, RunningApp};
use ferb_sanitize::{Py, Redactor};
use ferb_store::Db;
use serde_json::{json, Map, Value};

/// Where the gateway's events go: the scenario's bus.
pub type Publish = Arc<dyn Fn(&str, Py) + Send + Sync>;

/// One tool the scenario's model calls: its real description (a file tool's `describe`, paths, locks; the catalog's
/// facts), answering each call with the next recorded result. A file tool also really runs first, so the file is
/// there for the change record and the checker, as it was for Python.
pub struct Scripted {
    pub name: String,
    pub fs: Option<ferb_gateway::adapters::fs::FsTool>,
    pub replies: Mutex<VecDeque<Value>>,
    /// `/FX` in a recorded result, written back as this run's folder (its resolved spelling, as Python's tools
    /// report a path).
    pub root: String,
    /// wait.for: the clock it waits on and where it says it waits (`WaitDeps.status`).
    pub wait: Option<Wait>,
}

/// `WaitDeps.status(worker, task, until, line)`.
pub type Status =
    Arc<dyn Fn(&str, &str, Option<&ferb_clock::py::Time>, Option<&str>) + Send + Sync>;

/// What a scripted `wait.for` uses: the scenario's clock, the daemon's `WaitDeps.status` (the supervisor's
/// `wait_status`), and a flag the replay reads ("the worker's wait.for is running").
#[derive(Clone)]
pub struct Wait {
    pub clock: Arc<ferb_clock::clock::FakeClock>,
    pub status: Status,
    pub running: Arc<std::sync::atomic::AtomicBool>,
}

fn unmask(v: &Value, root: &str) -> Value {
    match v {
        Value::String(s) => Value::String(s.replace("/FX", root)),
        Value::Array(a) => Value::Array(a.iter().map(|x| unmask(x, root)).collect()),
        Value::Object(o) => Value::Object(
            o.iter()
                .map(|(k, x)| (k.clone(), unmask(x, root)))
                .collect(),
        ),
        other => other.clone(),
    }
}

impl Tool for Scripted {
    fn name(&self) -> &str {
        &self.name
    }
    fn describe(&self, args: &Args) -> ActionSpec {
        match &self.fs {
            Some(fs) => fs.describe(args),
            None => ActionSpec::new("read"),
        }
    }
    fn file_paths(&self, args: &Args) -> Option<Vec<(String, String)>> {
        self.fs.as_ref().and_then(|f| f.file_paths(args))
    }
    fn locks(&self, args: &Args) -> Vec<String> {
        self.fs.as_ref().map(|f| f.locks(args)).unwrap_or_default()
    }
    fn real_path(&self, args: &Args, workspace: &str) -> Option<ferb_gateway::tool::RealPath> {
        self.fs.as_ref().and_then(|f| f.real_path(args, workspace))
    }
    fn recovery_reason(&self, args: &Args) -> Option<Option<String>> {
        self.fs.as_ref().and_then(|f| f.recovery_reason(args))
    }
    fn run<'a>(
        &'a self,
        args: &'a Args,
        ctx: &'a ToolContext,
    ) -> BoxFuture<'a, Result<ToolOutput, ToolError>> {
        Box::pin(async move {
            if let Some(wait) = &self.wait {
                // ferb-clock's wait.for on a time condition, as far as the daemon sees it: its line ("Waiting until
                // HH:MM", the tool's own words) while the clock hasn't reached the time, then it wakes
                let minutes = args["any_of"][0]["in_minutes"].as_f64().unwrap_or(0.0);
                let now = wait.clock.now();
                let until = now + chrono::Duration::seconds((minutes * 60.0) as i64);
                let line = format!(
                    "Waiting until {}",
                    ferb_clock::py::fmt(&ferb_clock::zone::Zone::utc().local(&until), "%H:%M")
                );
                (wait.status)(&ctx.worker_id, &ctx.task_id, Some(&until), Some(&line));
                wait.running
                    .store(true, std::sync::atomic::Ordering::SeqCst);
                ferb_core::interfaces::Clock::sleep_until(wait.clock.as_ref(), until).await;
                wait.running
                    .store(false, std::sync::atomic::Ordering::SeqCst);
                (wait.status)(&ctx.worker_id, &ctx.task_id, None, None);
            }
            if let Some(fs) = &self.fs {
                // the real effect (the file written), its result replaced by the recorded one; Python's file tools
                // run in a worker thread (`asyncio.to_thread`), so the daemon's loop runs what is ready meanwhile
                let _ = fs.run(args, ctx).await;
                tokio::task::yield_now().await;
            }
            let reply = self
                .replies
                .lock()
                .unwrap()
                .pop_front()
                .expect("a call the recording made");
            let is_error = reply["is_error"].as_bool().unwrap_or(false);
            let mut content = unmask(&reply["content"], &self.root)
                .as_object()
                .cloned()
                .unwrap_or_default();
            content.shift_remove("after");
            // what the result was read from, as the tool reports it: a file it read; the stored origins task.evidence
            // passes on (evidence.py `_passed`); the recorded footer's last line names them
            let footer = reply["footer"].as_str().unwrap_or("");
            let origins: Vec<String> = footer
                .lines()
                .last()
                .and_then(|l| l.strip_prefix("Origin: "))
                .map(|l| {
                    l.split(" (")
                        .next()
                        .unwrap_or("")
                        .split(", ")
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default();
            let reads = match (
                self.name.as_str(),
                content.get("path").and_then(Value::as_str),
            ) {
                _ if is_error => vec![],
                ("fs.read", Some(p)) => vec![("file".to_string(), p.to_string())],
                ("task.evidence", _) => origins
                    .iter()
                    .map(|o| ("origin".to_string(), o.clone()))
                    .collect(),
                _ => vec![],
            };
            let mut measured = Vec::new();
            ferb_gateway::adapters::numbers(&Value::Object(content.clone()), "", &mut measured);
            Ok(ToolOutput {
                result: ToolResult {
                    content,
                    is_error,
                    read_sources: reads,
                    ..ToolResult::default()
                },
                measured,
            })
        })
    }
}

/// The change record (L39) behind the gateway's hooks: ferb-fs's `Changes.before`.
pub struct Before(pub Arc<ferb_fs::changes::Changes>);

impl ChangeRecord for Before {
    fn before<'a>(
        &'a self,
        task: &'a str,
        tree: &'a str,
        paths: Vec<String>,
    ) -> BoxFuture<'a, Result<Map<String, Value>, String>> {
        Box::pin(async move {
            Ok(self
                .0
                .before(task, tree, &paths)
                .to_json()
                .as_object()
                .cloned()
                .unwrap_or_default())
        })
    }
}

/// The real action ledger (ferb-orchestrator's `ledger`) behind the gateway's seam, as the daemon wires it.
pub struct RealLedger {
    pub db: Arc<Mutex<Db>>,
    pub clock: Arc<dyn Clock>,
    pub ids: Arc<dyn IdSource>,
}

fn row(r: ledger::LedgerRow) -> LedgerRow {
    LedgerRow {
        id: r.intent.id,
        task_id: r.intent.task_id,
        capability: r.intent.capability,
        target: r.intent.target.as_object().cloned().unwrap_or_default(),
        payload_hash: r.intent.payload_hash,
        state: r.intent.state,
        reconciled: r.reconciled,
        evidence: r
            .evidence
            .iter()
            .map(|e| match e {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            })
            .collect(),
        retry_approved: r.retry_approved,
        retried_by: r.retried_by,
    }
}

fn dup(e: ferb_orchestrator::sql::Error) -> Option<DuplicateUnresolved> {
    match e {
        ferb_orchestrator::sql::Error::Duplicate {
            message,
            intent_id,
            state,
        } => Some(DuplicateUnresolved {
            message,
            intent_id,
            state,
        }),
        _ => None,
    }
}

impl RealLedger {
    pub fn with<T>(&self, f: impl FnOnce(&ledger::Ledger<'_>) -> T) -> T {
        let db = self.db.lock().unwrap();
        let clock = self.clock.clone();
        let ids = self.ids.clone();
        let l = ledger::Ledger::new(
            &db,
            Box::new(move || ferb_core::when::isoformat(&clock.now())),
            Box::new(move || ids.new_id()),
        );
        f(&l)
    }
}

impl Ledger for RealLedger {
    fn intend(
        &self,
        task_id: &str,
        capability: &str,
        target: &Map<String, Value>,
        payload_hash: &str,
    ) -> Result<String, DuplicateUnresolved> {
        let target = Value::Object(target.clone());
        self.with(|l| l.intend(task_id, capability, &target, payload_hash, true))
            .map(|i| i.id)
            .map_err(|e| dup(e).expect("a refusal of the ledger"))
    }
    fn mark(&self, intent_id: &str, state: &str, evidence: &[String]) -> Result<(), BadTransition> {
        self.with(|l| l.mark(intent_id, state, evidence))
            .map_err(|e| BadTransition(format!("{e:?}")))
    }
    fn refusal(
        &self,
        task_id: &str,
        capability: &str,
        payload_hash: &str,
    ) -> Option<DuplicateUnresolved> {
        self.with(|l| l.refusal(task_id, capability, payload_hash))
            .ok()
            .flatten()
            .and_then(dup)
    }
    fn approved_retry(
        &self,
        task_id: &str,
        capability: &str,
        payload_hash: &str,
    ) -> Option<String> {
        self.with(|l| l.approved_retry(task_id, capability, payload_hash))
            .ok()
            .flatten()
    }
    fn attempts(&self, intent_id: &str) -> Vec<LedgerRow> {
        self.with(|l| l.attempts(intent_id))
            .map(|rows| rows.into_iter().map(row).collect())
            .unwrap_or_default()
    }
    fn approve_retry(&self, intent_id: &str, ask_span: &str) -> Result<(), BadTransition> {
        self.with(|l| l.approve_retry(intent_id, ask_span))
            .map_err(|e| BadTransition(format!("{e:?}")))
    }
    fn get(&self, intent_id: &str) -> Option<LedgerRow> {
        self.with(|l| l.get(intent_id)).ok().map(row)
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

/// `progress.describe` (orchestration's), for the words of a retry question.
struct Words;
impl Describe for Words {
    fn describe(
        &self,
        tool: &str,
        args: &Map<String, Value>,
        kind: &str,
        site: Option<&str>,
        app: Option<&str>,
    ) -> String {
        struct P;
        impl ferb_orchestrator::ports::Policy for P {
            fn site_key(&self, value: Option<&str>) -> String {
                value.unwrap_or("").to_lowercase()
            }
            fn account_key(&self, value: Option<&str>) -> String {
                ferb_policy::confirm::account_key(value)
            }
            fn url_host(&self, url: &str) -> Option<String> {
                ferb_policy::urls::parse(url).map(|u| u.host)
            }
            fn from_task(&self, text: &str) -> String {
                ferb_policy::confirm::from_task(text)
            }
            fn reads_as_ferbs_check(&self, text: &str) -> bool {
                ferb_policy::confirm::reads_as_ferbs_check(text)
            }
        }
        ferb_orchestrator::progress::describe(&P, tool, args, kind, site, app)
    }
    fn app_name(&self, _app: &str) -> String {
        "an app".into()
    }
}

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

struct NoTrash;
impl ferb_fs::files::TrashLocator for NoTrash {
    fn trash_url(&self, _near: &str, _create: bool) -> Option<String> {
        None
    }
}

struct BrowserPlaces;
impl Places for BrowserPlaces {
    fn is_browser(&self, _bundle_id: &str) -> bool {
        false
    }
    fn url_host(&self, url: &str) -> Option<String> {
        ferb_policy::urls::parse(url)
            .map(|p| p.host)
            .filter(|h| !h.is_empty())
    }
}

/// What a scenario's gateway is built from.
pub struct Parts {
    pub db: Arc<Mutex<Db>>,
    pub clock: Arc<dyn Clock>,
    pub ids: Arc<dyn IdSource>,
    pub redactor: Arc<Redactor>,
    pub publish: Publish,
    /// FERB's home (`<root>/<NN>/home`) and the owner's (`<root>/<NN>/user`).
    pub home: String,
    pub user: String,
    pub changes: Arc<ferb_fs::changes::Changes>,
    /// Every recorded tool call of the scenario (`gateway_calls`).
    pub calls: Vec<Value>,
    /// The run's folder as the tools report it (resolved).
    pub real_root: String,
    pub wait: Wait,
    /// The approvals' id counter (`"ap-" + token_hex(6)` scripted), shared with the supervisor's own questions.
    pub question_ids: Arc<std::sync::atomic::AtomicUsize>,
    pub device_id: String,
}

/// The daemon's gateway for one scenario.
pub fn build(p: Parts) -> Gateway {
    let workspace = format!("{}/workspace", p.home);
    let env = ferb_fs::files::FsEnv {
        home: p.user.clone(),
        trash: Arc::new(NoTrash),
        full_disk_access: Arc::new(|| false),
    };
    let mut reg = Registry::new();
    let mut names: Vec<String> = Vec::new();
    for c in &p.calls {
        let n = c["tool"].as_str().unwrap().to_string();
        if !names.contains(&n) {
            names.push(n);
        }
    }
    for name in names {
        let fs = match name.as_str() {
            "fs.write" => Some(ferb_fs::files::FS_WRITE),
            "fs.read" => Some(ferb_fs::files::FS_READ),
            _ => None,
        }
        .map(|info| ferb_gateway::adapters::fs::FsTool::new(info, env.clone()));
        let replies = p
            .calls
            .iter()
            .filter(|c| c["tool"] == name.as_str())
            .map(|c| c["reply"].clone())
            .collect();
        let wait = (name == "wait.for").then(|| p.wait.clone());
        reg.add(Arc::new(Scripted {
            name,
            fs,
            wait,
            replies: Mutex::new(replies),
            root: p.real_root.clone(),
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
        ferb_policy::protection::Protection::new(&p.user, &p.home, &lists, None, &POSIX_FILES)
            .unwrap();
    let publish = p.publish.clone();
    let changes = p.changes.clone();
    let made = p.changes.clone();
    let deps = Deps {
        db: p.db.clone(),
        clock: p.clock.clone(),
        ids: p.ids.clone(),
        redactor: p.redactor.clone(),
        ledger: Arc::new(RealLedger {
            db: p.db.clone(),
            clock: p.clock.clone(),
            ids: p.ids.clone(),
        }),
        publish: Box::new(move |topic, data| publish(topic, data)),
        roots: FsRoots {
            workspace: workspace.clone(),
            owner_roots: vec![],
            untrusted_roots: vec![],
            home: p.user.clone(),
            protection: Arc::new(SyncProtection(protection)),
        },
        device_id: p.device_id.clone(),
        recovery: Arc::new(ferb_gateway::adapters::fs::FsRecovery {
            db: p.db.clone(),
            clock: p.clock.clone(),
            ids: p.ids.clone(),
            root: format!("{}/recovery", p.home),
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
        hooks: Arc::new(RealHooks::new(HookParts {
            sentinel: None,
            previews: None,
            evidence: None,
            changes: Some(Arc::new(Before(changes))),
            images: None,
            app_windows: None,
            shadow: None,
            publish: Arc::new(|_t, _d| {}),
            journal: Arc::new(|_k, _d| {}),
            workspace,
            home: p.user.clone(),
        })),
        made_in_tree: Some(Box::new(move |root: &str, path: &str| {
            made.made_in_tree(root, path)
        })),
        excluded_host: None,
        owner_secrets: None,
        places: Arc::new(BrowserPlaces),
        username_holds: None,
    };
    let counter = p.question_ids.clone();
    Gateway::new(reg, deps, move || {
        format!(
            "ap-a{:011}",
            counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1
        )
    })
}
