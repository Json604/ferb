//! ORCH-753 (ORCH-719 item 2): a repeated look at the same place reaches the model as what changed, while the call's
//! record keeps the whole look. Through the real `Gateway` on a temp store, with stand-in hands that return scripted
//! looks and the ids they carried; an injected clock and ids (TEST-508).
#![cfg(all(feature = "store", target_os = "macos"))]

#[allow(dead_code)]
mod support;

use std::collections::{BTreeSet, VecDeque};
use std::sync::{Arc, Mutex};

use ferb_core::interfaces::{BoxFuture, Clock, IdSource};
use ferb_core::types::{ActionSpec, ToolContext, ToolResult};
use ferb_core::when::{self, DateTime};
use ferb_gateway::gateway::{
    Cancel, Deps, FsRoots, Gateway, StoreIds, StoreTime, SyncProtection, TaskRecord, POSIX_FILES,
};
use ferb_gateway::look_view::{rebuild, same_look, NOTE_PAGE, NOTE_WINDOW};
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
        "looking".into()
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

/// One scripted look: what the hand returns and the earlier ids it carried ({earlier: now}).
type Scripted = (Map<String, Value>, Map<String, Value>);

/// A look tool as the hands have it: each run returns the next scripted look.
struct Hand {
    name: &'static str,
    looks: Mutex<VecDeque<Scripted>>,
}

impl Tool for Hand {
    fn name(&self) -> &str {
        self.name
    }
    fn describe(&self, _args: &Args) -> ActionSpec {
        ActionSpec::new("read")
    }
    fn run<'a>(
        &'a self,
        _args: &'a Args,
        _ctx: &'a ToolContext,
    ) -> BoxFuture<'a, Result<ToolOutput, ToolError>> {
        let (content, carried) = self.looks.lock().unwrap().pop_front().expect("a look");
        Box::pin(async move {
            Ok(ToolOutput::from(ToolResult {
                content,
                carried: Some(carried),
                ..ToolResult::default()
            }))
        })
    }
}

struct Run {
    gw: Arc<Gateway>,
    db: Arc<Mutex<Db>>,
    grant: RunGrant,
    hands: Vec<Arc<Hand>>,
    _dir: tempfile::TempDir,
}

fn task() -> Run {
    let dir = tempfile::Builder::new().prefix("flv").tempdir().unwrap();
    let home = dir.path().join("ferb");
    let user = dir.path().join("user");
    std::fs::create_dir_all(home.join("workspace")).unwrap();
    std::fs::create_dir_all(&user).unwrap();
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
    let mut reg = Registry::new();
    let hands: Vec<Arc<Hand>> = ["browser.snapshot", "mac.snapshot"]
        .into_iter()
        .map(|name| {
            Arc::new(Hand {
                name,
                looks: Mutex::default(),
            })
        })
        .collect();
    for h in &hands {
        reg.add(h.clone()).unwrap();
    }
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
        words: "read my dashboard".into(),
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
        scopes: ["browser:read", "mac:read"]
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
        gw,
        db,
        grant,
        hands,
        _dir: dir,
    }
}

/// What the worker knows of each place, rebuilt from what it was sent alone.
#[derive(Default)]
struct Worker {
    known: Map<String, Value>,
}

impl Run {
    /// One look: the hand returns `look` with `carried`; (what the model received, the call's record, its span id).
    async fn look(
        &self,
        tool: &str,
        args: Value,
        look: Map<String, Value>,
        carried: Map<String, Value>,
    ) -> (Map<String, Value>, Map<String, Value>, String) {
        let hand = self.hands.iter().find(|h| h.name == tool).unwrap();
        hand.looks.lock().unwrap().push_back((look, carried));
        let reply = self
            .gw
            .call_as(
                &self.grant,
                tool,
                args.as_object().unwrap(),
                Cancel::new(),
                Arc::new(|| false),
            )
            .await
            .expect("the call returns");
        assert!(!reply.is_error, "{reply:?}");
        let db = self.db.lock().unwrap();
        let (id, out): (String, String) = db
            .conn()
            .query_row(
                "SELECT id, output_json FROM spans WHERE kind = 'tool.call' ORDER BY rowid DESC LIMIT 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        let recorded: Value = serde_json::from_str(&out).unwrap();
        (
            reply.content.as_object().cloned().unwrap(),
            recorded.as_object().cloned().unwrap(),
            id,
        )
    }
}

fn el(id: &str, label: &str) -> Value {
    json!({"id": id, "role": "link", "label": label})
}

/// A page of `n` links named `label 1..n` with ids of snapshot `gen`, and its text.
fn page(url: &str, gen: u32, n: usize, label: &str, text: &str) -> Map<String, Value> {
    let elements: Vec<Value> = (1..=n)
        .map(|i| el(&format!("s{gen}e{i}"), &format!("{label} {i}")))
        .collect();
    json!({"tab": "t1", "url": url, "title": "Dashboard", "elements": elements, "text": text,
           "scroll": {"y": 0, "height": 2000, "viewport_h": 800}})
    .as_object()
    .cloned()
    .unwrap()
}

/// Every element of the snapshots since the page loaded (the hands carry an id on through each later snapshot, as
/// long as the node is found again) carried to the same position in snapshot `to`; `from` is the last earlier one.
fn carried_all(from: u32, to: u32, n: usize) -> Map<String, Value> {
    (1..=from)
        .flat_map(|g| (1..=n).map(move |i| (format!("s{g}e{i}"), json!(format!("s{to}e{i}")))))
        .collect()
}

fn text(n: usize, changed: &[usize]) -> String {
    (1..=n)
        .map(|i| {
            if changed.contains(&i) {
                format!("line {i} (new)")
            } else {
                format!("line {i}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Checks one look: the record holds it whole; the model got it whole or as what changed; either way the worker can
/// rebuild it from what it was sent.
fn check(
    w: &mut Worker,
    sent: &Map<String, Value>,
    recorded: &Map<String, Value>,
    look: &Map<String, Value>,
    whole: bool,
) {
    // the record is the whole look, whatever the model got
    for (k, v) in look {
        assert_eq!(recorded.get(k), Some(v), "the record keeps {k}");
    }
    if whole {
        assert_eq!(sent, look, "sent whole");
        w.known = sent.clone();
    } else {
        assert!(
            sent.contains_key("changed_since"),
            "a what-changed view: {sent:?}"
        );
        assert!(!sent.contains_key("text"), "{sent:?}");
        let rebuilt = rebuild(&w.known, sent);
        assert!(
            same_look(&rebuilt, look),
            "nothing lost:\nrebuilt {rebuilt:?}\nlook {look:?}"
        );
        w.known = rebuilt;
    }
}

// decision: ORCH-753
#[tokio::test]
async fn a_repeated_look_is_what_changed_and_its_record_stays_whole() {
    let run = task();
    let mut w = Worker::default();
    let url = "https://dash.example.edu/home#top";
    let t = json!({"tab": "t1"});
    // 1. the first look at the page: whole
    let p1 = page(url, 1, 10, "Item", &text(10, &[]));
    let (sent, rec, first) = run
        .look("browser.snapshot", t.clone(), p1.clone(), Map::new())
        .await;
    check(&mut w, &sent, &rec, &p1, true);

    // 2. the same page: one label changed, one scrolled out of view, one line of text changed, and one element
    // whose id didn't carry (shown again, though its content is the same)
    let mut p2 = page(url, 2, 10, "Item", &text(10, &[4]));
    let els = p2["elements"].as_array_mut().unwrap();
    els[2]["label"] = json!("Item 3 (2 new)");
    els[3]["in_view"] = json!(false);
    let mut carried = carried_all(1, 2, 10);
    carried.remove("s1e7");
    let (sent, rec, _) = run
        .look("browser.snapshot", t.clone(), p2.clone(), carried)
        .await;
    check(&mut w, &sent, &rec, &p2, false);
    assert_eq!(sent["changed_since"], json!(first));
    assert_eq!(sent["note"], json!(NOTE_PAGE));
    assert_eq!(sent["unchanged"], json!(8));
    let shown: Vec<&str> = sent["elements"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["id"].as_str().unwrap())
        .collect();
    assert_eq!(shown, ["s2e3", "s2e7"], "changed, and not carried");
    assert_eq!(sent["gone"], json!({"s1e3": "Item 3", "s1e7": "Item 7"}));
    assert_eq!(sent["moved"], json!([{"id": "s1e4", "in_view": false}]));
    assert_eq!(
        sent["text_changes"],
        json!([{"line": 4, "removed": 1, "added": ["line 4 (new)"]}])
    );
    // the page's own facts are always there
    for k in ["tab", "url", "title", "scroll"] {
        assert_eq!(sent[k], p2[k], "{k}");
    }
    let whole_size = ferb_py::json::dumps(&Value::Object(p2.clone())).len();
    assert!(ferb_py::json::dumps(&Value::Object(sent.clone())).len() < whole_size);

    // 3. more than half of the elements changed: whole
    let p3 = page(url, 3, 10, "Other", &text(10, &[4]));
    let (sent, rec, _) = run
        .look(
            "browser.snapshot",
            t.clone(),
            p3.clone(),
            carried_all(2, 3, 10),
        )
        .await;
    check(&mut w, &sent, &rec, &p3, true);

    // 4.-8. five looks with small changes: what changed; 9. the 6th in a row: whole
    for gen in 4..=9u32 {
        let p = page(url, gen, 10, "Other", &text(10, &[4, gen as usize]));
        let (sent, rec, _) = run
            .look(
                "browser.snapshot",
                t.clone(),
                p.clone(),
                carried_all(gen - 1, gen, 10),
            )
            .await;
        check(&mut w, &sent, &rec, &p, gen == 9);
    }

    // 10. the worker asks for the whole page
    let p10 = page(url, 10, 10, "Other", &text(10, &[4]));
    let (sent, rec, _) = run
        .look(
            "browser.snapshot",
            json!({"tab": "t1", "whole": true}),
            p10.clone(),
            carried_all(9, 10, 10),
        )
        .await;
    check(&mut w, &sent, &rec, &p10, true);

    // 11. a small change: what changed; then the run's memory is compacted: 12. whole
    let p11 = page(url, 11, 10, "Other", &text(10, &[4, 5]));
    let (sent, rec, _) = run
        .look(
            "browser.snapshot",
            t.clone(),
            p11.clone(),
            carried_all(10, 11, 10),
        )
        .await;
    check(&mut w, &sent, &rec, &p11, false);
    run.gw.looks_reset("w1");
    let p12 = page(url, 12, 10, "Other", &text(10, &[4, 5]));
    let (sent, rec, _) = run
        .look(
            "browser.snapshot",
            t.clone(),
            p12.clone(),
            carried_all(11, 12, 10),
        )
        .await;
    check(&mut w, &sent, &rec, &p12, true);

    // 13. the same page with another fragment is the same place: what changed
    let p13 = page(
        "https://dash.example.edu/home#list",
        13,
        10,
        "Other",
        &text(10, &[4]),
    );
    let (sent, rec, _) = run
        .look(
            "browser.snapshot",
            t.clone(),
            p13.clone(),
            carried_all(12, 13, 10),
        )
        .await;
    check(&mut w, &sent, &rec, &p13, false);

    // 14. a new URL: whole
    let p14 = page(
        "https://dash.example.edu/settings",
        14,
        10,
        "Other",
        &text(10, &[4]),
    );
    let (sent, rec, _) = run
        .look(
            "browser.snapshot",
            t.clone(),
            p14.clone(),
            carried_all(13, 14, 10),
        )
        .await;
    check(&mut w, &sent, &rec, &p14, true);

    // a run that ended forgets its looks: another run's first look is whole
    run.gw.forget_calls("w1");
    let p15 = page(
        "https://dash.example.edu/settings",
        15,
        10,
        "Other",
        &text(10, &[4]),
    );
    let (sent, rec, _) = run
        .look(
            "browser.snapshot",
            t.clone(),
            p15.clone(),
            carried_all(14, 15, 10),
        )
        .await;
    check(&mut w, &sent, &rec, &p15, true);
}

fn window(
    gen: u32,
    title: &str,
    id: i64,
    labels: &[&str],
    open: Option<&str>,
) -> Map<String, Value> {
    let elements: Vec<Value> = labels
        .iter()
        .enumerate()
        .map(|(i, l)| json!({"id": format!("s{gen}e{}", i + 1), "role": "button", "label": l, "actions": ["press"]}))
        .collect();
    let mut m = json!({"app": "Notes", "bundle_id": "com.apple.Notes", "window": title, "elements": elements,
                       "text": "Groceries\nmilk\neggs", "offset": 0, "next_offset": null,
                       "windows": [{"title": title, "id": id, "current": true}]})
    .as_object()
    .cloned()
    .unwrap();
    if let Some(o) = open {
        m.insert("open".into(), json!([o]));
    }
    m
}

// decision: ORCH-753
#[tokio::test]
async fn a_mac_window_looked_at_again_is_what_changed_and_another_window_is_whole() {
    let run = task();
    let mut w = Worker::default();
    let a = json!({"app": "Notes"});
    let labels = ["New Note", "Delete", "Share", "Search", "Format", "Lock"];
    let w1 = window(1, "Groceries", 41, &labels, Some("sheet"));
    let (sent, rec, _) = run
        .look("mac.snapshot", a.clone(), w1.clone(), Map::new())
        .await;
    check(&mut w, &sent, &rec, &w1, true);
    // the same window: one button became disabled, the sheet closed
    let mut w2 = window(2, "Groceries", 41, &labels, None);
    w2["elements"][1]["disabled"] = json!(true);
    let carried: Map<String, Value> = (1..=6)
        .map(|i| (format!("s1e{i}"), json!(format!("s2e{i}"))))
        .collect();
    let (sent, rec, _) = run
        .look("mac.snapshot", a.clone(), w2.clone(), carried)
        .await;
    check(&mut w, &sent, &rec, &w2, false);
    assert_eq!(sent["note"], json!(NOTE_WINDOW));
    assert_eq!(sent["open"], Value::Null, "a field that went is null");
    assert!(!sent.contains_key("windows"), "unchanged: left out");
    assert!(!sent.contains_key("text_changes"), "the text is the same");
    // a later page of the same window is sent as it is, and changes nothing
    let mut page2 = window(3, "Groceries", 41, &labels, None);
    page2.insert("offset".into(), json!(200));
    let (sent, _, _) = run
        .look(
            "mac.snapshot",
            json!({"app": "Notes", "offset": 200}),
            page2.clone(),
            Map::new(),
        )
        .await;
    assert_eq!(sent, page2);
    // another window: whole
    let w4 = window(4, "Recipes", 42, &labels, None);
    let (sent, rec, _) = run
        .look("mac.snapshot", a.clone(), w4.clone(), Map::new())
        .await;
    check(&mut w, &sent, &rec, &w4, true);
}
