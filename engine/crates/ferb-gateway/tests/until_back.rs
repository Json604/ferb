//! GATE-615: the phone's "until I'm back at my Mac" through the real `Gateway` on a temp store: a Yes to the screen-use
//! question given with it answers every later screen-use question without asking, until the owner's own input on the
//! Mac after it; then the next one asks again, and the end is published with why and the owner's line. A question that
//! isn't the screen-use question alone is never covered, and it isn't offered while FERB can't tell.
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
use ferb_gateway::screen_use::{AtTheMac, TOPIC};
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
}

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
        list_only: false,
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
    /// A foreground click into `app`; each question asked is answered Yes, the first with "until I'm back" when
    /// `back` (as the phone gate gives it); (the reply, the questions asked, whether each was offered it).
    async fn click(&self, app: &str, back: bool) -> (Value, Vec<(String, bool, bool)>) {
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
        let reply = loop {
            tokio::select! {
                biased;
                r = &mut fut => break r,
                () = tokio::task::yield_now() => {
                    if let Some(ap) = self.gw.pending().into_iter().next() {
                        let offered = self.gw.until_back_offered(&ap.id);
                        asked.push((ap.summary.clone(), ap.screen_use, offered));
                        if back && asked.len() == 1 {
                            self.gw.grant_until_back(&ap.id, "ep1", "Pixel").unwrap();
                        }
                        self.gw.resolve(&ap.id, true, Some(1), None).unwrap();
                    }
                }
            }
        };
        (reply.expect("the call returns").to_json(), asked)
    }

    fn ended(&self) -> Vec<Value> {
        self.events
            .lock()
            .unwrap()
            .iter()
            .filter(|(t, d)| t == TOPIC && d["state"] == "ended")
            .map(|(_, d)| d.clone())
            .collect()
    }
}

// decision: GATE-615
// decision: PHN-586
#[tokio::test]
async fn until_i_m_back_answers_screen_use_until_the_owner_s_own_input() {
    let r = task();
    let since = when::parse(T0).unwrap().timestamp() as f64;
    // input before it was given doesn't end it
    *r.mac.clear.lock().unwrap() = Some(since - 30.0);
    let (_, asked) = r.click("Notes", true).await;
    assert_eq!(asked.len(), 1, "{asked:?}");
    assert!(
        asked[0].1 && asked[0].2,
        "the screen question, offered: {asked:?}"
    );
    assert!(r.gw.until_back().is_some());
    assert_eq!(r.gw.screen_use_status()["active"], true);
    // another app's screen question: answered without asking
    let (reply, asked) = r.click("Mail", false).await;
    assert!(asked.is_empty(), "{asked:?}");
    assert_eq!(reply["content"]["clicked"], true, "{reply}");
    // the owner's own input on the Mac: it ends, said once with why, and the next screen question asks
    *r.mac.clear.lock().unwrap() = Some(since + 1.0);
    let (_, asked) = r.click("Maps", false).await;
    assert_eq!(asked.len(), 1, "{asked:?}");
    let ended = r.ended();
    assert_eq!(ended.len(), 1, "{ended:?}");
    assert_eq!(ended[0]["why"], "owner_input");
    assert!(ended[0]["line"]
        .as_str()
        .unwrap()
        .starts_with("Welcome back."));
    assert_eq!(
        ended[0]["during_hold"], false,
        "a clear return: no question was shown"
    );
    assert_eq!(r.gw.screen_use_status()["active"], false);
}

// decision: GATE-615
#[tokio::test]
async fn it_is_not_offered_while_ferb_can_t_tell_and_a_turn_off_ends_it() {
    let r = task();
    *r.mac.blind.lock().unwrap() = true;
    let (_, asked) = r.click("Notes", false).await;
    assert_eq!(asked.len(), 1);
    assert!(asked[0].1 && !asked[0].2, "{asked:?}");
    assert_eq!(r.gw.screen_use_status()["can_offer"], false);
    *r.mac.blind.lock().unwrap() = false;
    let (_, asked) = r.click("Mail", true).await;
    assert!(asked[0].2);
    assert!(r
        .gw
        .end_until_back(ferb_gateway::screen_use::Ended::TurnedOff)
        .is_some());
    assert_eq!(r.ended()[0]["why"], "turned_off");
    let (_, asked) = r.click("Maps", false).await;
    assert_eq!(asked.len(), 1, "it asks again: {asked:?}");
}

/// GATE-616 (owner 2026-11-17: "no, the 30min dont ask shoudl be there"): a protected folder's read lift stays in
/// the routine class the 30-minute answer covers; its change lift and a payment never are.
// decision: GATE-616
#[test]
fn a_protected_folder_s_read_stays_routine() {
    let read = ferb_policy::confirm::Need::new(
        "escape:read:~/Documents",
        "k".into(),
        ferb_policy::confirm::Values::of(&[("lift", "read")]),
    );
    let write = ferb_policy::confirm::Need::new(
        "escape:read:~/Documents",
        "w".into(),
        ferb_policy::confirm::Values::of(&[("lift", "write")]),
    );
    let pay = ferb_policy::confirm::Need::new(
        ferb_policy::confirm::ALWAYS_PAY,
        "p".into(),
        ferb_policy::confirm::Values::of(&[]),
    );
    assert!(ferb_policy::confirm::routine(&read));
    assert!(!ferb_policy::confirm::routine(&write) && !ferb_policy::confirm::routine(&pay));
}

impl Run {
    fn at(&self, s: f64) {
        *self.clock.0.lock().unwrap() = s;
    }

    fn events(&self, state: &str) -> Vec<Value> {
        self.events
            .lock()
            .unwrap()
            .iter()
            .filter(|(t, d)| t == TOPIC && d["state"] == state)
            .map(|(_, d)| d.clone())
            .collect()
    }

    /// A grant from the phone's Yes to Notes, then FERB's own click into Mail at `posted` and an unclear event 0.2 s
    /// after it, looked at a moment later: the hold starts.
    async fn held(&self) -> f64 {
        let since = when::parse(T0).unwrap().timestamp() as f64;
        self.click("Notes", true).await;
        *self.mac.posted.lock().unwrap() = Some(since + 5.0);
        *self.mac.unclear.lock().unwrap() = Some(since + 5.2);
        self.at(5.5);
        let g = self.gw.until_back().expect("still in force");
        assert!(g.hold.is_some());
        since
    }
}

/// GATE-618: an unclear event holds: FERB posts nothing (a click waits before it runs), "Are you back?" is published
/// for FERB.app's quiet line; any input during the hold ends it, with the welcome line marked as said in place of the
/// question; the waiting click then needs the owner's Yes, since what "until I'm back" answered covers nothing now.
// decision: GATE-618
#[tokio::test]
async fn an_unclear_event_holds_screen_use_and_input_during_the_hold_ends_it() {
    let r = task();
    let since = r.held().await;
    let checking = r.events("checking");
    assert_eq!(checking.len(), 1);
    assert_eq!(checking[0]["title"], "Are you back?");
    assert_eq!(checking[0]["first"], true);
    let before = r.clicks.0.load(std::sync::atomic::Ordering::SeqCst);
    let gw = r.gw.clone();
    let grant = r.grant.clone();
    let waiting = tokio::spawn(async move {
        let args = json!({"app": "Notes", "shot": "s1", "x": 10, "y": 10, "mechanism": "foreground",
                          "allow_foreground": true});
        gw.call_as(
            &grant,
            "mac.click",
            args.as_object().unwrap(),
            Cancel::new(),
            Arc::new(|| false),
        )
        .await
        .map(|r| r.to_json())
    });
    tokio::time::sleep(std::time::Duration::from_millis(600)).await;
    assert_eq!(
        r.clicks.0.load(std::sync::atomic::Ordering::SeqCst),
        before,
        "FERB posts nothing during the hold"
    );
    assert!(!waiting.is_finished());
    // the owner moves the pointer
    *r.mac.unclear.lock().unwrap() = Some(since + 7.0);
    r.at(7.2);
    assert!(r.gw.until_back().is_none());
    let ended = r.ended();
    assert_eq!(ended[0]["why"], "owner_input");
    assert_eq!(ended[0]["during_hold"], true);
    assert!(ended[0]["line"]
        .as_str()
        .unwrap()
        .starts_with("Welcome back."));
    // the click goes on to the gate: it asks the owner now (answered by no standing Yes)
    let mut asked = 0;
    while !waiting.is_finished() {
        if let Some(ap) = r.gw.pending().into_iter().next() {
            asked += 1;
            r.gw.resolve(&ap.id, false, None, None).unwrap();
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert_eq!(
        asked, 1,
        "the owner is asked again, even for the app the phone's Yes named"
    );
    assert_eq!(r.clicks.0.load(std::sync::atomic::Ordering::SeqCst), before);
}

/// GATE-618, GATE-619: a hold that passes in silence resumes quietly (no line, no end), the waiting click runs under
/// the grant without asking, and an unclear event in the quiet time is let pass while a clear one still ends it.
// decision: GATE-618
// decision: GATE-619
#[tokio::test]
async fn a_silent_hold_resumes_quietly_and_the_quiet_time_lets_unclear_events_pass() {
    let r = task();
    let since = r.held().await;
    r.at(5.5 + ferb_gateway::screen_use::HOLD_S + 0.1);
    assert!(r.gw.until_back().is_some_and(|g| g.hold.is_none()));
    assert_eq!(r.events("resumed").len(), 1);
    assert!(r.ended().is_empty(), "nothing said after a silent hold");
    let (_, asked) = r.click("Mail", false).await;
    assert!(asked.is_empty(), "screen use goes on: {asked:?}");
    *r.mac.posted.lock().unwrap() = Some(since + 30.0);
    *r.mac.unclear.lock().unwrap() = Some(since + 30.1);
    r.at(31.0);
    assert!(
        r.gw.until_back().is_some_and(|g| g.hold.is_none()),
        "let pass"
    );
    assert_eq!(r.events("checking").len(), 1);
    *r.mac.clear.lock().unwrap() = Some(since + 40.0);
    r.at(41.0);
    assert!(r.gw.until_back().is_none());
    assert_eq!(r.ended()[0]["during_hold"], false);
}

/// APP-958 (owner, 2026-11-17): after an "Are you back?" that nobody answered, FERB doesn't ask it again for 10
/// minutes (GATE-619's number, confirmed).
// decision: APP-958
#[test]
fn a_silent_are_you_back_is_not_asked_again_for_ten_minutes() {
    assert_eq!(ferb_gateway::screen_use::QUIET_S, 600.0);
    assert_eq!(ferb_gateway::screen_use::HOLD_S, 10.0);
}
