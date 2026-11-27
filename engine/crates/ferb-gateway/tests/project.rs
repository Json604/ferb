//! Project mode's question through the real `Gateway` (HAND-624, HAND-625): the first publish of a folder asks once,
//! naming the folder, branch and exact address, and the Yes makes the folder a project with that address approved,
//! kept across tasks in the journal; the same address doesn't ask again; another address asks again, with the
//! project's shorter card; a No records nothing and runs nothing; a folder that holds FERB's own state is refused
//! unasked; a task working in a project gets the toolchain folders (HAND-622). The publish tool here is a stand-in
//! with the real tool's checks (its push is ferb-shell's, tested there under the real sandbox).
#![cfg(all(feature = "store", target_os = "macos"))]

#[allow(dead_code)]
mod support;

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

use ferb_core::interfaces::{BoxFuture, Clock, IdSource};
use ferb_core::types::{ActionSpec, ToolContext, ToolResult};
use ferb_core::when::{self, DateTime};
use ferb_gateway::gateway::{
    Cancel, Deps, FsRoots, Gateway, StoreIds, StoreTime, SyncProtection, TaskRecord, POSIX_FILES,
};
use ferb_gateway::questions::Approval;
use ferb_gateway::seams::{Describe, Usage};
use ferb_gateway::tool::{Args, Registry, Tool, ToolError, ToolOutput};
use ferb_identity::owner_secrets::Places;
use ferb_policy::confirm::{Apps, AppsUnread, RunningApp};
use ferb_policy::projects;
use ferb_policy::small::{Request, Targets};
use ferb_rpc::auth::{Identity, RunGrant};
use ferb_sanitize::Redactor;
use ferb_store::Db;
use serde_json::{json, Map, Value};

use support::ledger::RealLedger;

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

struct NoPlaces;
impl Places for NoPlaces {
    fn is_browser(&self, _bundle_id: &str) -> bool {
        false
    }
    fn url_host(&self, _url: &str) -> Option<String> {
        None
    }
}

/// project.publish as the gateway sees it: its prepared push, and a run that pushes only to an approved address (the
/// real tool's check), recording each run and the folders its context carried.
struct FakePublish {
    folder: Mutex<String>,
    url: Mutex<String>,
    runs: Mutex<Vec<(String, Vec<String>)>>,
    db: Arc<Mutex<Db>>,
    /// "push" or "fetch" (HAND-642).
    action: Mutex<String>,
    /// What a forced push would replace (HAND-641), as ferb-shell's plan writes it.
    force: Mutex<Option<Value>>,
    gh: Mutex<bool>,
    /// The remote commit each forced run replaced.
    leases: Mutex<Vec<Option<String>>>,
}

impl Tool for FakePublish {
    fn name(&self) -> &str {
        projects::TOOL
    }
    fn describe(&self, args: &Args) -> ActionSpec {
        let mut s = ActionSpec::new(projects::KIND);
        s.target.path = args
            .get("folder")
            .and_then(Value::as_str)
            .map(str::to_string);
        s
    }
    fn prepare_publish<'a>(
        &'a self,
        _args: &'a Args,
    ) -> BoxFuture<'a, Option<Result<Map<String, Value>, String>>> {
        let url = self.url.lock().unwrap().clone();
        let folder = self.folder.lock().unwrap().clone();
        let action = self.action.lock().unwrap().clone();
        let force = self.force.lock().unwrap().clone();
        let gh = *self.gh.lock().unwrap();
        Box::pin(async move {
            let mut v = json!({"action": action, "folder": folder, "url": url, "host": "git.example.invalid",
                           "path": "o/site.git", "branch": "main", "commit": "a".repeat(40)});
            if let Some(f) = force {
                v["force"] = f;
            }
            if gh {
                v["gh"] = json!(true);
            }
            Some(Ok(v.as_object().unwrap().clone()))
        })
    }
    fn run<'a>(
        &'a self,
        _args: &'a Args,
        ctx: &'a ToolContext,
    ) -> BoxFuture<'a, Result<ToolOutput, ToolError>> {
        Box::pin(async move {
            let url = self.url.lock().unwrap().clone();
            let folder = self.folder.lock().unwrap().clone();
            let action = self.action.lock().unwrap().clone();
            let list = projects::store::list(&self.db.lock().unwrap()).unwrap();
            let p = list.iter().find(|p| p.folder == folder);
            let allowed = if action == "fetch" {
                p.is_some_and(|p| p.gets_from(&url))
            } else {
                p.is_some_and(|p| p.destinations.contains(&url))
            };
            if !allowed {
                return Err(ToolError::refused("not approved"));
            }
            if *self.gh.lock().unwrap()
                && !p.is_some_and(|p| p.gh_hosts.iter().any(|h| h == "git.example.invalid"))
            {
                return Err(ToolError::refused("gh not allowed"));
            }
            if self.force.lock().unwrap().is_some() {
                let clock = FakeClock;
                static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(9000);
                let ids = Seq(Mutex::new(
                    NEXT.fetch_add(10, std::sync::atomic::Ordering::SeqCst),
                ));
                let taken = projects::store::take_force(
                    &self.db.lock().unwrap(),
                    &clock,
                    &ids,
                    &ctx.task_id,
                    &folder,
                    &url,
                    "main",
                    &"a".repeat(40),
                )
                .unwrap();
                if taken.is_none() {
                    return Err(ToolError::refused("no Yes to overwrite"));
                }
                self.leases.lock().unwrap().push(taken);
            }
            self.runs
                .lock()
                .unwrap()
                .push((url.clone(), ctx.folders.clone()));
            let mut content = Map::new();
            content.insert("pushed".into(), json!(true));
            Ok(ToolResult {
                content,
                ..Default::default()
            }
            .into())
        })
    }
}

struct Run {
    gw: Arc<Gateway>,
    fake: Arc<FakePublish>,
    db: Arc<Mutex<Db>>,
    grant: RunGrant,
    user: std::path::PathBuf,
    _dir: tempfile::TempDir,
}

/// A temp home with Chrome's data folder (on FERB's default never-read list) beside an ordinary app's folder.
fn task() -> Run {
    let dir = tempfile::Builder::new().prefix("fpj").tempdir().unwrap();
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
    let site = user.join("code/site");
    std::fs::create_dir_all(&site).unwrap();
    let site = std::fs::canonicalize(&site).unwrap();
    let fake = Arc::new(FakePublish {
        folder: Mutex::new(site.to_str().unwrap().to_string()),
        url: Mutex::new("https://git.example.invalid/o/site.git".into()),
        runs: Mutex::new(Vec::new()),
        db: db.clone(),
        action: Mutex::new("push".into()),
        force: Mutex::new(None),
        gh: Mutex::new(false),
        leases: Mutex::new(Vec::new()),
    });
    let mut reg = Registry::new();
    reg.add(fake.clone()).unwrap();
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
        profile_turned_off: Box::new(|_b, _f| false),
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
        words: "publish my site in ~/code/site".into(),
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
        scopes: ["shell"].into_iter().map(str::to_string).collect(),
        identity: Identity::Owner,
        expires_at: 1e9,
        observe_only: false,
        tools: BTreeSet::new(),
        max_calls: 0,
    };
    Run {
        gw,
        fake,
        db: db.clone(),
        grant,
        user,
        _dir: dir,
    }
}

impl Run {
    /// One call, every question answered Yes; (the reply as the model reads it, the questions' summaries).
    async fn call(&self, yes: bool) -> (Value, Vec<Approval>) {
        let folder = self.fake.folder.lock().unwrap().clone();
        let args = json!({ "folder": folder });
        let tool = ferb_policy::projects::TOOL;
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
                        asked.push(ap.clone());
                        self.gw.resolve(&ap.id, yes, Some(if yes { 1 } else { 2 }), None).unwrap();
                    }
                }
            }
        };
        (reply.expect("the call returns").to_json(), asked)
    }

    fn projects(&self) -> Vec<projects::Project> {
        projects::store::list(&self.db.lock().unwrap()).unwrap()
    }
}

// decision: HAND-624
#[tokio::test]
async fn the_first_publish_asks_once_and_the_yes_makes_a_project() {
    let r = task();
    let folder = r.fake.folder.lock().unwrap().clone();
    let (reply, asked) = r.call(true).await;
    assert_eq!(asked.len(), 1, "{asked:?}");
    let a = &asked[0];
    eprintln!(
        "first card: {:?} | {:?} | {:?} | {:?}",
        a.summary, a.detail, a.exact, a.labels
    );
    assert_eq!(a.summary, "Work on this as a project in ~/code/site?");
    assert!(
        a.detail.contains(
            "FERB's commands can then read and change files there and publish its git repository \
                                when you ask. Your saved logins and keys stay closed to them."
        ),
        "{}",
        a.detail
    );
    assert_eq!(
        a.exact,
        "https://git.example.invalid/o/site.git\nbranch main"
    );
    assert_eq!(reply["content"]["pushed"], json!(true), "{reply}");
    let got = r.projects();
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].folder, folder);
    assert_eq!(
        got[0].destinations,
        ["https://git.example.invalid/o/site.git"]
    );

    // the same address: no question
    let (reply, asked) = r.call(true).await;
    assert!(asked.is_empty(), "{asked:?}");
    assert_eq!(reply["content"]["pushed"], json!(true), "{reply}");

    // the remote changed: asked again, with the project's card
    *r.fake.url.lock().unwrap() = "https://other.example.invalid/x/site.git".into();
    let (_, asked) = r.call(true).await;
    assert_eq!(asked.len(), 1, "{asked:?}");
    eprintln!(
        "second card: {:?} | {:?} | {:?}",
        asked[0].summary, asked[0].detail, asked[0].labels
    );
    assert_eq!(
        asked[0].summary,
        "Publish ~/code/site to git.example.invalid/o/site.git?"
    );
    assert_eq!(r.fake.runs.lock().unwrap().len(), 3);
    assert_eq!(r.projects()[0].destinations.len(), 2);
}

// decision: HAND-624
#[tokio::test]
async fn a_no_records_nothing_and_pushes_nothing() {
    let r = task();
    let (reply, asked) = r.call(false).await;
    assert_eq!(asked.len(), 1);
    assert!(r.fake.runs.lock().unwrap().is_empty(), "{reply}");
    assert!(r.projects().is_empty());
}

// decision: HAND-621
#[tokio::test]
async fn a_folder_that_holds_ferbs_own_state_is_refused_unasked() {
    let r = task();
    // the temp root holds both the owner's home and FERB's home
    let root = std::path::Path::new(&*r.fake.folder.lock().unwrap())
        .ancestors()
        .nth(4)
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    *r.fake.folder.lock().unwrap() = root;
    let (reply, asked) = r.call(true).await;
    assert!(asked.is_empty(), "{asked:?}");
    assert!(reply.to_string().contains("can't be a project"), "{reply}");
    assert!(r.fake.runs.lock().unwrap().is_empty());
}

// decision: HAND-622
#[tokio::test]
async fn a_task_working_in_a_project_gets_the_toolchain_folders() {
    let r = task();
    let folder = r.fake.folder.lock().unwrap().clone();
    r.gw.set_task_folders("task-1", vec![format!("{folder}/web")]);
    let _ = r.call(true).await; // the Yes makes it a project
    let _ = r.call(true).await;
    let runs = r.fake.runs.lock().unwrap().clone();
    let home = std::fs::canonicalize(r.user.clone()).unwrap();
    let go = format!("{}/go", r.user.to_str().unwrap());
    let go_real = format!("{}/go", home.to_str().unwrap());
    assert!(
        runs[1].1.iter().any(|f| *f == go || *f == go_real),
        "{runs:?}"
    );
}

fn force_facts() -> Value {
    json!({"remote_commit": "b".repeat(40), "short": "bbbbbbb", "subject": "Their change",
           "author": "Sam", "date": "2026-11-15", "dropped": 2, "local_short": "aaaaaaa"})
}

/// A forced push asks every time, even for an address already approved, with what it would drop; the Yes is used
/// once, for exactly that commit.
// decision: HAND-641
#[tokio::test]
async fn a_forced_push_asks_every_time_and_says_what_it_drops() {
    let r = task();
    let _ = r.call(true).await; // the folder is a project, its address approved
    *r.fake.force.lock().unwrap() = Some(force_facts());
    let (reply, asked) = r.call(true).await;
    assert_eq!(asked.len(), 1, "{asked:?}");
    let a = &asked[0];
    eprintln!(
        "force card: {:?} | {:?} | {:?} | {:?}",
        a.summary, a.detail, a.exact, a.labels
    );
    assert_eq!(
        a.summary,
        "Overwrite main on git.example.invalid/o/site.git?"
    );
    assert_eq!(
        a.detail,
        "The newest commit there is bbbbbbb \u{201c}Their change\u{201d} by Sam, 2026-11-15. Publishing replaces the \
         branch with the one in ~/code/site, which drops 2 commits that only exist there. FERB can't undo this. If \
         anyone adds to the branch before FERB pushes, FERB stops instead of overwriting it."
    );
    assert_eq!(
        a.exact,
        "https://git.example.invalid/o/site.git\nbranch main: bbbbbbb \u{2192} aaaaaaa"
    );
    assert_eq!(reply["content"]["pushed"], json!(true), "{reply}");
    assert_eq!(
        r.fake.leases.lock().unwrap()[0].as_deref(),
        Some("b".repeat(40).as_str())
    );
    // the same forced push again: asked again
    let (_, asked) = r.call(true).await;
    assert_eq!(asked.len(), 1, "{asked:?}");
    // a No: nothing pushed
    let runs = r.fake.runs.lock().unwrap().len();
    let (_, asked) = r.call(false).await;
    assert_eq!(asked.len(), 1);
    assert_eq!(r.fake.runs.lock().unwrap().len(), runs);
}

/// Getting from an address the project may push to asks nothing; a new address asks once and is remembered.
// decision: HAND-642
#[tokio::test]
async fn a_fetch_from_an_approved_address_asks_nothing_and_a_new_one_asks_once() {
    let r = task();
    let _ = r.call(true).await; // push address approved
    *r.fake.action.lock().unwrap() = "fetch".into();
    let (reply, asked) = r.call(true).await;
    assert!(asked.is_empty(), "{asked:?}");
    assert_eq!(reply["content"]["pushed"], json!(true), "{reply}");
    *r.fake.url.lock().unwrap() = "https://git.example.invalid/o/other.git".into();
    let (_, asked) = r.call(true).await;
    assert_eq!(asked.len(), 1, "{asked:?}");
    eprintln!(
        "fetch card: {:?} | {:?} | {:?}",
        asked[0].summary, asked[0].detail, asked[0].labels
    );
    assert_eq!(
        asked[0].summary,
        "Get git.example.invalid/o/site.git into ~/code/site with your git login?"
    );
    let (_, asked) = r.call(true).await;
    assert!(asked.is_empty(), "remembered: {asked:?}");
    assert_eq!(
        r.projects()[0].sources,
        ["https://git.example.invalid/o/other.git"]
    );
}

/// The GitHub CLI's login: asked once per project.
// decision: HAND-643
#[tokio::test]
async fn the_github_cli_login_is_asked_once_per_project() {
    let r = task();
    let _ = r.call(true).await;
    *r.fake.gh.lock().unwrap() = true;
    let (reply, asked) = r.call(true).await;
    assert_eq!(asked.len(), 1, "{asked:?}");
    eprintln!(
        "gh card: {:?} | {:?} | {:?}",
        asked[0].summary, asked[0].detail, asked[0].labels
    );
    assert_eq!(
        asked[0].summary,
        "Use your GitHub CLI login for this project?"
    );
    assert_eq!(reply["content"]["pushed"], json!(true), "{reply}");
    let (_, asked) = r.call(true).await;
    assert!(asked.is_empty(), "{asked:?}");
}
