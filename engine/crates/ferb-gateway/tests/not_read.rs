//! What the file tools say about protected folders in or around a path (HAND-595, HAND-597), through the real
//! `Gateway` on a temp store with FERB's default protection lists over a temp home: fs.disk_usage counts a folder that
//! holds a browser's data by size without a question and names it; fs.list names it beside the listing; naming the
//! folder itself raises the one read question for the task, and after the Yes the listing reads it with no fact.
//! A path with nothing protected around it keeps Python's bytes (no `not_read`).
#![cfg(all(feature = "store", target_os = "macos"))]

#[allow(dead_code)]
mod support;

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

use ferb_core::interfaces::{BoxFuture, Clock, IdSource};
use ferb_core::types::{ActionSpec, ToolContext};
use ferb_core::when::{self, DateTime};
use ferb_gateway::gateway::{
    Cancel, Deps, FsRoots, Gateway, StoreIds, StoreTime, SyncProtection, TaskRecord, POSIX_FILES,
};
use ferb_gateway::seams::{Describe, Usage};
use ferb_gateway::tool::{Args, Registry, Tool, ToolError, ToolOutput};
use ferb_identity::owner_secrets::Places;
use ferb_policy::confirm::{Apps, AppsUnread, RunningApp};
use ferb_policy::small::{Request, Targets};
use ferb_rpc::auth::{Identity, RunGrant};
use ferb_sanitize::Redactor;
use ferb_store::Db;
use serde_json::{json, Map, Value};

use support::ledger::RealLedger;

const CHROME: &str = "~/Library/Application Support/Google/Chrome";
const BROWSER: &str = "a browser's data (saved passwords, cookies, history, sign-ins)";

struct FakeClock;
impl Clock for FakeClock {
    fn now(&self) -> DateTime {
        when::parse("2026-01-01T09:00:00+00:00").unwrap()
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

struct NoTrash;
impl ferb_fs::files::TrashLocator for NoTrash {
    fn trash_url(&self, _near: &str, _create: bool) -> Option<String> {
        None
    }
}

struct NoPlaces;
impl Places for NoPlaces {
    fn is_browser(&self, _bundle_id: &str) -> bool {
        false
    }
    fn url_host(&self, _url: &str) -> Option<String> {
        None
    }
}

/// fs.disk_usage as the file tools define it (usage.py: one path, metadata); `declared` false is a registration that
/// doesn't say which files it names.
struct DiskUsage {
    env: ferb_fs::files::FsEnv,
    declared: bool,
}

impl Tool for DiskUsage {
    fn name(&self) -> &str {
        ferb_fs::usage::NAME
    }
    fn describe(&self, args: &Args) -> ActionSpec {
        let mut s = ActionSpec::new("read");
        s.target.path = args.get("path").and_then(Value::as_str).map(str::to_string);
        s
    }
    fn file_paths(&self, args: &Args) -> Option<Vec<(String, String)>> {
        let a: ferb_fs::usage::Args = serde_json::from_value(Value::Object(args.clone())).ok()?;
        self.declared.then(|| {
            ferb_fs::usage::file_paths(&a)
                .into_iter()
                .map(|(p, how)| (p, how.to_string()))
                .collect()
        })
    }
    fn run<'a>(
        &'a self,
        args: &'a Args,
        ctx: &'a ToolContext,
    ) -> BoxFuture<'a, Result<ToolOutput, ToolError>> {
        Box::pin(async move {
            let a: ferb_fs::usage::Args = serde_json::from_value(Value::Object(args.clone()))
                .map_err(|e| ToolError::failed("ValidationError", e.to_string()))?;
            ferb_fs::usage::fs_disk_usage(&a, ctx, &self.env)
                .map(ToolOutput::from)
                .map_err(|e| ToolError::failed("RuntimeError", format!("{e:?}")))
        })
    }
}

struct Run {
    gw: Arc<Gateway>,
    grant: RunGrant,
    user: std::path::PathBuf,
    _dir: tempfile::TempDir,
}

/// A temp home with Chrome's data folder (on FERB's default never-read list) beside an ordinary app's folder.
fn task(declared: bool) -> Run {
    let dir = tempfile::Builder::new().prefix("fnr").tempdir().unwrap();
    let home = dir.path().join("ferb");
    let user = dir.path().join("user");
    std::fs::create_dir_all(home.join("workspace")).unwrap();
    let support = user.join("Library/Application Support");
    std::fs::create_dir_all(support.join("Google/Chrome/Default")).unwrap();
    std::fs::write(
        support.join("Google/Chrome/Default/Login Data"),
        vec![b'x'; 4096],
    )
    .unwrap();
    std::fs::create_dir_all(support.join("Other App")).unwrap();
    std::fs::write(support.join("Other App/cache.bin"), vec![b'y'; 1000]).unwrap();
    let db = Db::open(home.join("ferb.db")).unwrap();
    db.migrate().unwrap();
    let db = Arc::new(Mutex::new(db));
    let clock: Arc<dyn Clock> = Arc::new(FakeClock);
    let ids: Arc<dyn IdSource> = Arc::new(Seq(Mutex::new(0)));
    let redactor = Arc::new(Redactor::new(&[], &[]));
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
    let env = ferb_fs::files::FsEnv {
        home: user.to_str().unwrap().to_string(),
        trash: Arc::new(NoTrash),
        full_disk_access: Arc::new(|| true),
    };
    let mut reg = Registry::new();
    reg.add(Arc::new(ferb_gateway::adapters::fs::FsTool::new(
        ferb_fs::files::FS_LIST,
        env.clone(),
    )))
    .unwrap();
    reg.add(Arc::new(DiskUsage { env, declared })).unwrap();
    let deps = Deps {
        db: db.clone(),
        clock: clock.clone(),
        ids: ids.clone(),
        redactor: redactor.clone(),
        ledger: Arc::new(RealLedger {
            db: db.clone(),
            clock: clock.clone(),
            ids: ids.clone(),
        }),
        publish: Box::new(|_t, _d| {}),
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
        proc_task: Box::new(|_p| None),
        route_question: None,
        hooks: Arc::new(ferb_gateway::seams::NoHooks),
        made_in_tree: None,
        excluded_host: None,
        owner_secrets: None,
        places: Arc::new(NoPlaces),
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
        words: "where is my storage going".into(),
        targets: Targets::default(),
        payload: String::new(),
        asked_origins: BTreeSet::new(),
    });
    let now = clock.now();
    let span_id = {
        let d = db.lock().unwrap();
        let spans = ferb_store::spans::SpanStore::new(
            &d,
            Box::new(StoreTime(clock.clone())),
            Box::new(StoreIds(ids.clone())),
            Box::new(|v: &Value| v.clone()),
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
    let grant = RunGrant {
        task_id: "task-1".into(),
        worker_id: "w1".into(),
        scopes: ["fs:read"].into_iter().map(str::to_string).collect(),
        identity: Identity::Owner,
        expires_at: 1e9,
        observe_only: false,
        tools: BTreeSet::new(),
        max_calls: 0,
    };
    Run {
        gw,
        grant,
        user,
        _dir: dir,
    }
}

impl Run {
    /// One call, every question answered Yes; (the reply as the model reads it, the questions' summaries).
    async fn call(&self, tool: &str, path: &str) -> (Value, Vec<String>) {
        let args = json!({ "path": path });
        let fut = self.gw.call_as(
            &self.grant,
            tool,
            args.as_object().unwrap(),
            Cancel::new(),
            Arc::new(|| false),
        );
        tokio::pin!(fut);
        let mut asked = Vec::new();
        let reply = loop {
            tokio::select! {
                biased;
                r = &mut fut => break r,
                () = tokio::task::yield_now() => {
                    if let Some(ap) = self.gw.pending().into_iter().next() {
                        asked.push(format!("{} | {}", ap.summary, ap.reason));
                        self.gw.resolve(&ap.id, true, Some(1), None).unwrap();
                    }
                }
            }
        };
        (reply.expect("the call returns").to_json(), asked)
    }

    fn at(&self, rel: &str) -> String {
        self.user.join(rel).to_str().unwrap().to_string()
    }
}

/// fs.disk_usage over a tree holding a browser's data: no question, the folder's size in the total, and the fact
/// naming the folder, its rule and why.
// decision: HAND-595, HAND-597
#[tokio::test]
async fn disk_usage_counts_a_protected_folder_by_size_and_names_it() {
    let r = task(true);
    let (reply, asked) = r
        .call("fs.disk_usage", &r.at("Library/Application Support"))
        .await;
    assert!(asked.is_empty(), "{asked:?}");
    let c = &reply["content"];
    assert_eq!(c["bytes"], json!(4096 + 1000), "{reply}");
    assert_eq!(c["complete"], json!(true));
    assert_eq!(
        c["not_read"]["folders"],
        json!([{"path": CHROME, "rule": format!("read:{CHROME}"), "why": BROWSER}])
    );
    assert_eq!(
        c["not_read"]["fact"],
        json!(ferb_fs::protected::DISK_USAGE_FACT)
    );
    eprintln!(
        "fs.disk_usage reply: {}",
        serde_json::to_string_pretty(&reply).unwrap()
    );

    // a path with nothing protected in or around it: Python's bytes, no `not_read`
    let (reply, _) = r
        .call(
            "fs.disk_usage",
            &r.at("Library/Application Support/Other App"),
        )
        .await;
    assert!(reply["content"].get("not_read").is_none(), "{reply}");
}

/// fs.list names the protected folder under the listing; naming that folder asks the owner once for the task, and
/// after the Yes it is read and no longer reported.
// decision: HAND-595, HAND-597, GATE-200, GATE-203
#[tokio::test]
async fn a_listing_names_the_folder_and_naming_it_asks_the_owner_once() {
    let r = task(true);
    let (reply, asked) = r
        .call("fs.list", &r.at("Library/Application Support"))
        .await;
    assert!(asked.is_empty(), "{asked:?}");
    let c = &reply["content"];
    let names: Vec<&str> = c["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["Google", "Other App"]);
    assert_eq!(
        c["not_read"]["folders"][0]["rule"],
        json!(format!("read:{CHROME}"))
    );
    assert_eq!(c["not_read"]["fact"], json!(ferb_fs::protected::LIST_FACT));

    let chrome = r.at("Library/Application Support/Google/Chrome");
    let (reply, asked) = r.call("fs.list", &chrome).await;
    assert_eq!(asked.len(), 1, "{asked:?}");
    assert!(
        asked[0].contains(&format!(
            "let FERB read {CHROME} for this task (rule read:{CHROME})"
        )),
        "{asked:?}"
    );
    assert_eq!(
        reply["content"]["entries"][0]["name"],
        json!("Default"),
        "{reply}"
    );
    assert!(reply["content"].get("not_read").is_none(), "{reply}");
    // the Yes covers the task: the next read of that folder asks nothing
    let (_, asked) = r.call("fs.list", &format!("{chrome}/Default")).await;
    assert!(asked.is_empty(), "{asked:?}");
}

/// A disk-usage tool registered without its file paths is refused before it runs: the gateway's rule that ferbd's
/// registration must satisfy (compose/tools/misc.rs `DiskUsage` declares the path as a metadata read).
#[tokio::test]
async fn disk_usage_without_declared_files_is_refused() {
    let r = task(false);
    let (reply, _) = r
        .call("fs.disk_usage", &r.at("Library/Application Support"))
        .await;
    assert!(
        reply
            .to_string()
            .contains("doesn't declare the files it names"),
        "{reply}"
    );
}
