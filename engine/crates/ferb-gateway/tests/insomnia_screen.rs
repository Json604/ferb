//! T-1274 through the real `Gateway` on a temp store (the until-back test's harness): insomnia mode's one Yes answers
//! the screen-use question for its session only (APP-984); while the Mac's screen is locked a step that would use the
//! keyboard and pointer asks "Do it when I'm back?" and, on Yes, waits for the unlock before it runs (HAND-645).
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
use ferb_gateway::screen_use::AtTheMac;
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

const T0: &str = "2026-01-01T09:00:00+00:00";

/// The daemon's clock, moved by a test (seconds after T0).
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

/// `mac.click` as the catalog has it; it only counts its runs (the input FERB would post).
#[derive(Default)]
struct Click(std::sync::atomic::AtomicUsize);

impl Tool for Click {
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
            self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let mut content = Map::new();
            content.insert("clicked".into(), json!(true));
            Ok(ToolOutput::from(ToolResult {
                content,
                ..ToolResult::default()
            }))
        })
    }
}

/// The owner at the Mac as a test sets it.
#[derive(Default)]
struct Mac {
    blind: Mutex<bool>,
    clear: Mutex<Option<f64>>,
    unclear: Mutex<Option<f64>>,
    posted: Mutex<Option<f64>>,
    locked: std::sync::atomic::AtomicBool,
}

impl AtTheMac for Mac {
    fn can_tell(&self) -> bool {
        !*self.blind.lock().unwrap()
    }
    fn clear_input_at(&self) -> Option<f64> {
        *self.clear.lock().unwrap()
    }
    fn unclear_input_at(&self) -> Option<f64> {
        *self.unclear.lock().unwrap()
    }
    fn ferb_posted_at(&self) -> Option<f64> {
        *self.posted.lock().unwrap()
    }
    fn away(&self) -> bool {
        false
    }
    fn screen_locked(&self) -> bool {
        self.locked.load(std::sync::atomic::Ordering::SeqCst)
    }
}

#[allow(dead_code)]
struct Run {
    clock: Arc<FakeClock>,
    clicks: Arc<Click>,
    gw: Arc<Gateway>,
    grant: RunGrant,
    mac: Arc<Mac>,
    events: Arc<Mutex<Vec<(String, Value)>>>,
    _dir: tempfile::TempDir,
}

fn task() -> Run {
    let dir = tempfile::Builder::new().prefix("fub").tempdir().unwrap();
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
    let clicks = Arc::new(Click::default());
    reg.add(clicks.clone()).unwrap();
    let events: Arc<Mutex<Vec<(String, Value)>>> = Arc::default();
    let ev = events.clone();
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
        publish: Box::new(move |t, d| {
            ev.lock().unwrap().push((
                t.to_string(),
                d.to_json(false)
                    .ok()
                    .and_then(|j| serde_json::from_str(&j).ok())
                    .unwrap_or(Value::Null),
            ))
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
    let mac = Arc::new(Mac::default());
    gw.set_at_mac(mac.clone());
    gw.record_request(Request {
        request_id: "req-1".into(),
        words: "tidy my notes".into(),
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
        scopes: ["mac:act"].into_iter().map(str::to_string).collect(),
        identity: Identity::Owner,
        expires_at: 1e9,
        observe_only: false,
        tools: BTreeSet::new(),
        max_calls: 0,
    };
    Run {
        clock: fake,
        clicks,
        gw,
        grant,
        mac,
        events,
        _dir: dir,
    }
}

impl Run {
    /// A foreground click into `app`; each question asked is answered `yes`, or `unlock_after` answers first and then
    /// unlocks the screen once the call has waited a while; (the reply, the questions asked: title and detail).
    async fn click(&self, app: &str, yes: bool) -> (Value, Vec<String>) {
        let args = json!({"app": app, "shot": "s1", "x": 10, "y": 10, "mechanism": "foreground",
                          "allow_foreground": true});
        let fut = self.gw.call_as(
            &self.grant,
            "mac.click",
            args.as_object().unwrap(),
            Cancel::new(),
            Arc::new(|| false),
        );
        tokio::pin!(fut);
        let mut asked = Vec::new();
        let mut answered_at: Option<tokio::time::Instant> = None;
        let reply = loop {
            tokio::select! {
                biased;
                r = &mut fut => break r,
                () = tokio::time::sleep(std::time::Duration::from_millis(100)) => {
                    if let Some(ap) = self.gw.pending().into_iter().next() {
                        asked.push(format!("{} | {} | {} | {}", ap.title, ap.summary, ap.detail, ap.labels.join("/")));
                        self.gw.resolve(&ap.id, yes, Some(1), None).unwrap();
                        answered_at = Some(tokio::time::Instant::now());
                    } else if let Some(t) = answered_at {
                        // still locked a minute later: nothing ran; then the owner is back
                        if t.elapsed() > std::time::Duration::from_secs(60) && self.mac.screen_locked() {
                            assert_eq!(self.clicks.0.load(std::sync::atomic::Ordering::SeqCst), 0, "ran while locked");
                            self.mac.locked.store(false, std::sync::atomic::Ordering::SeqCst);
                        }
                    }
                }
            }
        };
        (reply.expect("the call returns").to_json(), asked)
    }

    fn clicked(&self) -> usize {
        self.clicks.0.load(std::sync::atomic::Ordering::SeqCst)
    }
}

/// The one Yes answers the screen-use question while its session lasts; after it, the question asks again, and a
/// confirmation it gave covers nothing.
// decision: APP-987
// decision: APP-984
#[tokio::test(start_paused = true)]
async fn insomnia_s_one_yes_covers_screen_use_for_its_session_only() {
    let r = task();
    let session: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(Some(T0.to_string())));
    let s = session.clone();
    r.gw.set_standing_screen(Arc::new(move || s.lock().unwrap().clone()));
    let (reply, asked) = r.click("Notes", true).await;
    assert!(asked.is_empty(), "{asked:?}");
    assert_eq!(reply["content"]["clicked"], true, "{reply}");
    let (_, asked) = r.click("Mail", true).await;
    assert!(asked.is_empty(), "{asked:?}");
    // the session ends: the same app asks again
    *session.lock().unwrap() = None;
    let (_, asked) = r.click("Notes", true).await;
    assert_eq!(asked.len(), 1, "{asked:?}");
    // a new session doesn't revive the old one's answers (Mail was answered by the first session only)
    *session.lock().unwrap() = Some("2026-01-01T10:00:00+00:00".to_string());
    let (_, asked) = r.click("Mail", true).await;
    assert!(asked.is_empty(), "the new session's own Yes: {asked:?}");
    assert_eq!(r.clicked(), 4);
}

/// While the screen is locked the step asks to be done when the owner is back; a Yes waits for the unlock and then
/// runs; a No runs nothing; the one Yes of insomnia mode doesn't answer it; unlocked, nothing more is asked.
// decision: HAND-646
// decision: HAND-645
#[tokio::test(start_paused = true)]
async fn a_locked_screen_asks_do_it_when_i_m_back_and_waits_for_the_unlock() {
    let r = task();
    r.gw.set_standing_screen(Arc::new(|| Some(T0.to_string())));
    r.mac
        .locked
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let (reply, asked) = r.click("Notes", false).await;
    assert_eq!(asked.len(), 1, "{asked:?} {reply}");
    assert!(asked[0].contains("Your Mac is locked."), "{asked:?}");
    assert!(
        asked[0].contains("Do it when I'm back/Skip it"),
        "{asked:?}"
    );
    assert!(
        reply["is_error"] == true || reply["content"]["clicked"].is_null(),
        "{reply}"
    );
    assert_eq!(r.clicked(), 0, "a No runs nothing");
    // the No holds for this app in this task (it isn't asked again); another app, Yes: it waits while locked (the
    // helper unlocks after a minute and checks nothing ran before), then runs
    let (reply, _) = r.click("Notes", true).await;
    assert_eq!(r.clicked(), 0, "{reply}");
    let (reply, asked) = r.click("Mail", true).await;
    assert_eq!(asked.len(), 1, "{asked:?}");
    assert_eq!(reply["content"]["clicked"], true, "{reply}");
    assert_eq!(r.clicked(), 1);
    // unlocked: no locked question (the session's one Yes answers the screen question)
    let (_, asked) = r.click("Maps", true).await;
    assert!(asked.is_empty(), "{asked:?}");
    assert_eq!(r.clicked(), 2);
    assert!(!r.gw.screen_locked());
}
