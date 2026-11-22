//! HAND-677 (owner, 2026-11-22), HAND-678: browser_pair for a profile whose FERB extension is added but turned off, through the
//! real `Gateway` on a temp store with a stand-in browser.pair. FERB asks the owner exactly what he does (FERB opens
//! that profile's extensions page; he switches FERB on there): once per call, even inside a "don't ask for 30 min"
//! window and even for a profile his words named; a profile whose extension is on keeps the routine pairing question.
//!
//! Live 2026-11-22 (task a843aa54…): the owner's 30-minute answer to the pairing question then answered every routine
//! question of a worker driving Chrome's own menus and pages for eight minutes; he saw none of them.
#![cfg(all(feature = "store", target_os = "macos"))]

#[allow(dead_code)]
mod support;

use std::collections::BTreeSet;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use ferb_core::interfaces::{BoxFuture, Clock, IdSource};
use ferb_core::types::{ActionSpec, ToolContext, ToolResult};
use ferb_core::when::{self, DateTime};
use ferb_gateway::gateway::{
    Cancel, Deps, FsRoots, Gateway, StoreIds, StoreTime, SyncProtection, TaskRecord, POSIX_FILES,
};
use ferb_gateway::seams::{Describe, Usage};
use ferb_gateway::tool::{Args, Registry, Tool, ToolError, ToolOutput};
use ferb_gateway::words::ALLOW_WINDOW;
use ferb_identity::owner_secrets::Places;
use ferb_policy::confirm::{Apps, AppsUnread, RunningApp};
use ferb_policy::small::{Request, Targets};
use ferb_rpc::auth::{Identity, RunGrant};
use ferb_sanitize::Redactor;
use ferb_store::Db;
use serde_json::{json, Map, Value};

use support::ledger::RealLedger;

const T0: &str = "2026-01-01T09:00:00+00:00";

#[derive(Default)]
struct FakeClock(Mutex<f64>);
impl Clock for FakeClock {
    fn now(&self) -> DateTime {
        when::parse(T0).unwrap()
            + chrono::Duration::milliseconds((*self.0.lock().unwrap() * 1000.0) as i64)
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

/// browser.pair as the browser tools have it; it only counts its runs (the daemon's pairing).
#[derive(Default)]
struct Pair(AtomicUsize);

impl Tool for Pair {
    fn name(&self) -> &str {
        "browser.pair"
    }
    fn describe(&self, args: &Args) -> ActionSpec {
        let _ = args;
        ActionSpec::new("click")
    }
    fn run<'a>(
        &'a self,
        _args: &'a Args,
        _ctx: &'a ToolContext,
    ) -> BoxFuture<'a, Result<ToolOutput, ToolError>> {
        Box::pin(async move {
            self.0.fetch_add(1, Ordering::SeqCst);
            let mut content = Map::new();
            content.insert("paired".into(), json!(true));
            Ok(ToolOutput::from(ToolResult {
                content,
                ..ToolResult::default()
            }))
        })
    }
}

/// `mac.click` as the catalog has it (on screen); each click looks and sees nothing change.
struct NoChange;

impl Tool for NoChange {
    fn name(&self) -> &str {
        "mac.click"
    }
    fn describe(&self, args: &Args) -> ActionSpec {
        let _ = args;
        ActionSpec::new("click")
    }
    fn run<'a>(
        &'a self,
        _args: &'a Args,
        _ctx: &'a ToolContext,
    ) -> BoxFuture<'a, Result<ToolOutput, ToolError>> {
        Box::pin(async move {
            let mut content = Map::new();
            content.insert("ok".into(), json!(true));
            content.insert("after".into(), json!({"read": "ax", "changed": false}));
            Ok(ToolOutput::from(ToolResult {
                content,
                ..ToolResult::default()
            }))
        })
    }
}

struct Run {
    pairs: Arc<Pair>,
    gw: Arc<Gateway>,
    grant: RunGrant,
    _dir: tempfile::TempDir,
}

/// A task whose request says `words`; Chrome's "Profile 30" ("IIT") has FERB turned off, "Profile 2" ("Work") on.
fn task(words: &str) -> Run {
    let dir = tempfile::Builder::new().prefix("fpo").tempdir().unwrap();
    let home = dir.path().join("ferb");
    let user = dir.path().join("user");
    std::fs::create_dir_all(home.join("workspace")).unwrap();
    std::fs::create_dir_all(&user).unwrap();
    let db = Db::open(home.join("ferb.db")).unwrap();
    db.migrate().unwrap();
    let db = Arc::new(Mutex::new(db));
    let fake = Arc::new(FakeClock::default());
    let clock: Arc<dyn Clock> = fake.clone();
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
    let mut reg = Registry::new();
    let pairs = Arc::new(Pair::default());
    reg.add(pairs.clone()).unwrap();
    reg.add(Arc::new(NoChange)).unwrap();
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
        profile_name: Box::new(|_b, f| {
            if f == "Profile 30" {
                "IIT".into()
            } else {
                "Work".into()
            }
        }),
        profile_account: Box::new(|_b, _f| String::new()),
        profile_turned_off: Box::new(|b, f| b == "chrome" && f == "Profile 30"),
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
        words: words.into(),
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
        scopes: ["browser:act", "browser:read", "mac:act"]
            .into_iter()
            .map(str::to_string)
            .collect(),
        identity: Identity::Owner,
        expires_at: 1e9,
        observe_only: false,
        tools: BTreeSet::new(),
        max_calls: 0,
    };
    Run {
        pairs,
        gw,
        grant,
        _dir: dir,
    }
}

/// One question as the owner saw it: (the card's question, its detail, the options, FERB's full line).
type Asked = (String, String, Vec<String>, String);

impl Run {
    /// A foreground click into `app`, each question answered with `answer`; (the reply, the questions asked).
    async fn foreground_click(&self, app: &str, answer: &str) -> (Value, Vec<Asked>) {
        let args = json!({"app": app, "shot": "s1", "x": 10, "y": 10, "mechanism": "foreground",
                          "allow_foreground": true});
        self.call("mac.click", args, answer).await
    }

    /// browser.pair for `profile`; each question asked is answered with `answer` (an option's label); (the reply, the
    /// questions asked).
    async fn pair(&self, profile: &str, answer: &str) -> (Value, Vec<Asked>) {
        let args = json!({"profile": profile, "why": "the owner's task"});
        self.call("browser.pair", args, answer).await
    }

    /// `tool` with `args`; each question asked is answered with `answer` (an option's label).
    async fn call(&self, tool: &str, args: Value, answer: &str) -> (Value, Vec<Asked>) {
        let fut = self.gw.call_as(
            &self.grant,
            tool,
            args.as_object().unwrap(),
            Cancel::new(),
            Arc::new(|| false),
        );
        tokio::pin!(fut);
        let mut asked: Vec<Asked> = Vec::new();
        let reply = loop {
            tokio::select! {
                biased;
                r = &mut fut => break r,
                () = tokio::task::yield_now() => {
                    if let Some(ap) = self.gw.pending().into_iter().next() {
                        asked.push((ap.summary.clone(), ap.detail.clone(), ap.options.clone(), ap.reason.clone()));
                        let at = ap.options.iter().position(|o| o == answer)
                            .unwrap_or_else(|| panic!("{answer:?} isn't offered: {:?}", ap.options));
                        self.gw.resolve(&ap.id, answer != "No", Some(at + 1), None).unwrap(); // options count from 1
                    }
                }
            }
        };
        (reply.expect("the call returns").to_json(), asked)
    }
}

// decision: HAND-677, HAND-678
#[tokio::test]
async fn a_turned_off_profile_asks_its_own_question_every_time_even_inside_a_dont_ask_window() {
    let r = task("do my week 7 assignments");
    // the routine pairing question of a profile with FERB on: the owner answers "don't ask for 30 min"
    let (reply, asked) = r.pair("chrome/Profile 2", ALLOW_WINDOW).await;
    assert_eq!(asked.len(), 1, "{asked:?}");
    assert_eq!(
        asked[0].0, "Let FERB work in your Chrome “Work” profile?",
        "{asked:?}"
    );
    assert_eq!(reply["content"]["paired"], true, "{reply}");
    // the turned-off profile: asked even so, saying what the owner does; the window isn't offered for it
    let (reply, asked) = r.pair("chrome/Profile 30", "Yes").await;
    assert_eq!(asked.len(), 1, "{asked:?}");
    let (summary, detail, options, line) = &asked[0];
    assert_eq!(summary, "Turn FERB on in your Chrome “IIT” profile?");
    assert!(
        detail.contains("Chrome lets only you turn it on") && detail.contains("switch FERB on"),
        "{detail}"
    );
    assert!(
        line.contains("this opens the extensions page of your Chrome profile “IIT”")
            && line.contains("switch FERB on there"),
        "{line}"
    );
    assert!(!options.iter().any(|o| o == ALLOW_WINDOW), "{options:?}");
    assert_eq!(reply["content"]["paired"], true, "{reply}");
    // a second call (a retry): asked again, never covered by the first Yes
    let (_, asked) = r.pair("chrome/Profile 30", "Yes").await;
    assert_eq!(asked.len(), 1, "{asked:?}");
    assert_eq!(r.pairs.0.load(Ordering::SeqCst), 3);
}

// decision: HAND-677, HAND-678
#[tokio::test]
async fn a_turned_off_profile_the_owner_named_is_still_asked_about_and_a_no_runs_nothing() {
    let r = task("do the assignments in my IIT profile");
    let (reply, asked) = r.pair("chrome/Profile 30", "No").await;
    assert_eq!(
        asked.len(),
        1,
        "his own step is said even when he named it: {asked:?}"
    );
    assert!(
        reply.get("error").is_some() || reply["content"]["paired"].is_null(),
        "{reply}"
    );
    assert_eq!(r.pairs.0.load(Ordering::SeqCst), 0);
    // a named profile whose FERB is on is the owner's word already: no question (unchanged)
    let r = task("do the assignments in my Work profile");
    let (_, asked) = r.pair("chrome/Profile 2", "Yes").await;
    assert!(asked.is_empty(), "{asked:?}");
    assert_eq!(r.pairs.0.load(Ordering::SeqCst), 1);
}

/// ORCH-691 through the real gateway: eight clicks into one app that change nothing make `stuck` say so for the
/// worker, once, and latch it for the step's check; the counts then start again.
// decision: ORCH-691, ORCH-692
#[tokio::test]
async fn eight_unchanged_clicks_into_one_app_stop_the_attempt() {
    let r = task("tidy my notes");
    for n in 1..=8 {
        let args = json!({"app": "Notes", "shot": "s1", "x": 10, "y": 10});
        let reply =
            r.gw.call_as(
                &r.grant,
                "mac.click",
                args.as_object().unwrap(),
                Cancel::new(),
                Arc::new(|| false),
            )
            .await
            .unwrap()
            .to_json();
        assert_eq!(reply["content"]["ok"], true, "{reply}");
        let stuck = r.gw.stuck("w1");
        assert_eq!(stuck.is_some(), n == 8, "{n}: {stuck:?}");
    }
    assert!(r.gw.stuck("w1").is_none(), "said once");
    assert!(r.gw.take_stuck("task-1").is_some());
    assert!(r.gw.take_stuck("task-1").is_none());
}

/// GATE-623 (owner, 2026-11-22: "No, always ask for screen input"; supersedes GATE-207's foreground coverage):
/// inside a "don't ask for 30 min" window, taking the owner's screen still asks and never offers the window; once
/// answered, the one Yes still covers that app for the task (unchanged).
// decision: GATE-623, GATE-207
#[tokio::test]
async fn a_dont_ask_window_never_answers_screen_input() {
    let r = task("tidy my notes");
    let (_, asked) = r.pair("chrome/Profile 2", ALLOW_WINDOW).await;
    assert_eq!(asked.len(), 1, "the window opens: {asked:?}");
    let (reply, asked) = r.foreground_click("Notes", "Yes").await;
    assert_eq!(
        asked.len(),
        1,
        "screen use asks inside the window: {asked:?}"
    );
    assert!(!asked[0].2.iter().any(|o| o == ALLOW_WINDOW), "{asked:?}");
    assert_eq!(reply["content"]["ok"], true, "{reply}");
    let (_, asked) = r.foreground_click("Notes", "Yes").await;
    assert!(
        asked.is_empty(),
        "one Yes covers Notes for the task: {asked:?}"
    );
}
