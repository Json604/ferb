//! What a call's record keeps that the model never sees, and what the owner is told about a call (HAND-548,
//! HAND-546). The real `Gateway` on a temp store, with the real call hooks and stand-ins for the parts they drive: an
//! image sanitizer that reports the attempts that failed (HAND-531's faults), and a disturbance sentinel that reports
//! the front app changing during a browser call (HAND-535).
#![cfg(all(feature = "store", target_os = "macos"))]

#[allow(dead_code)]
mod support;

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

use ferb_core::interfaces::{BoxFuture, Clock, IdSource};
use ferb_core::types::{ActionSpec, Target, ToolContext, ToolResult};
use ferb_core::when::{self, DateTime};
use ferb_gateway::call_hooks::{HookParts, ImageSanitizer, RealHooks, Sentinel};
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

/// The browser process the stand-in tab lives in.
const CHROME_PID: i64 = 4242;

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
        bundle_id == "com.google.Chrome"
    }
    fn url_host(&self, url: &str) -> Option<String> {
        ferb_policy::urls::parse(url)
            .map(|p| p.host)
            .filter(|h| !h.is_empty())
    }
}

/// HAND-531's sanitizer as the Mac's reports it: every attempt failed (withheld), or a later one read the image.
struct Images {
    recovered: bool,
}

impl Images {
    fn faults() -> Vec<Value> {
        vec![
            json!({"step": "vision", "domain": "com.apple.Vision", "code": 9}),
            json!({"step": "vision_cpu", "domain": "com.apple.Vision", "code": 9}),
        ]
    }
}

impl ImageSanitizer for Images {
    fn sanitize<'a>(&'a self, image: &'a [u8]) -> BoxFuture<'a, Result<Vec<u8>, String>> {
        Box::pin(async move { self.sanitize_noted(image).await.0 })
    }
    fn sanitize_noted<'a>(
        &'a self,
        image: &'a [u8],
    ) -> BoxFuture<'a, ferb_gateway::call_hooks::SanitizedNoted> {
        let mut notes = Map::new();
        notes.insert("faults".into(), Value::Array(Self::faults()));
        let out = if self.recovered {
            notes.insert("read_by".into(), json!("child"));
            let mut img = image.to_vec();
            img.extend_from_slice(b"+clean");
            Ok(img)
        } else {
            notes.insert("withheld".into(), json!(true));
            Err("image text recognition unavailable".to_string())
        };
        Box::pin(async move { (out, notes) })
    }
}

/// The disturbance sentinel's answer for every sampled call: the front app changed from `before` to `after`.
struct FrontApp {
    after_pid: i64,
}

impl Sentinel for FrontApp {
    fn call_started(&self, _task: &str, _span: &str, _name: &str) {}
    fn before_action<'a>(
        &'a self,
        _task: &'a str,
        _span: &'a str,
        _name: &'a str,
        _arguments: &'a Map<String, Value>,
        _on_screen: bool,
    ) -> BoxFuture<'a, Result<(), String>> {
        Box::pin(async { Ok(()) })
    }
    fn call_ended(&self, _span: &str) {}
    fn after_action<'a>(
        &'a self,
        _span: &'a str,
    ) -> BoxFuture<'a, Result<Map<String, Value>, String>> {
        let name = if self.after_pid == CHROME_PID {
            "Google Chrome"
        } else {
            "Notes"
        };
        let changed = json!({"frontmost_app": {
            "before": {"pid": 77, "bundle_id": "com.brave.Browser", "name": "Brave Browser"},
            "after": {"pid": self.after_pid, "bundle_id": "invented.bundle", "name": name}}});
        Box::pin(async move { Ok(changed.as_object().cloned().unwrap()) })
    }
}

/// `browser.act` (a click in the task's tab on app.example.edu) and `browser.screenshot` (a read with an image), as
/// the gateway sees the real ones; the click's result may carry the extension's own explanation (`window_shown`).
struct Browser {
    name: &'static str,
    explained: bool,
    /// What its put-back answers (HAND-560); None: it can't.
    put: Option<(bool, Option<String>)>,
    /// Its result carries its own `screen_changed_during_call` (as a Mac tool's does, HAND-549).
    own_fact: bool,
}

impl Tool for Browser {
    fn name(&self) -> &str {
        self.name
    }
    fn facts(&self) -> Facts {
        Facts {
            on_screen: self.name == "browser.act",
            ..Facts::default()
        }
    }
    fn describe(&self, _args: &Args) -> ActionSpec {
        let page = "https://app.example.edu/login".to_string();
        ActionSpec {
            target: Target {
                site: Some("app.example.edu".into()),
                page: Some(page),
                ..Target::default()
            },
            ..ActionSpec::new(if self.name == "browser.act" {
                "click"
            } else {
                "read"
            })
        }
    }
    fn file_paths(&self, _args: &Args) -> Option<Vec<(String, String)>> {
        Some(vec![])
    }
    fn background_pid(&self, _args: &Args) -> Option<i64> {
        Some(CHROME_PID)
    }
    fn put_back<'a>(
        &'a self,
        front: &'a Value,
        previous: &'a Value,
        _task: &'a str,
    ) -> BoxFuture<'a, Option<(bool, Option<String>)>> {
        assert_eq!(front["pid"], json!(CHROME_PID));
        assert_eq!(previous["name"], json!("Brave Browser"));
        let put = self.put.clone();
        Box::pin(async move { put })
    }
    fn run<'a>(
        &'a self,
        _args: &'a Args,
        _ctx: &'a ToolContext,
    ) -> BoxFuture<'a, Result<ToolOutput, ToolError>> {
        Box::pin(async move {
            let mut content = json!({"ok": true}).as_object().cloned().unwrap();
            if self.own_fact {
                content.insert(
                    "screen_changed_during_call".into(),
                    json!({"front_app": {"before": "Brave Browser", "after": "Notes"}, "by": "the app",
                           "put_back": true, "raw_input_asks": true, "why": "invented",
                           "next": "FERB put Brave Browser back in front."}),
                );
            }
            if self.explained {
                content.insert(
                    "window_shown".into(),
                    json!(["the page opened a window of its own (a popup, as a sign-in does), …"]),
                );
            }
            Ok(ToolOutput::from(ToolResult {
                content,
                image: (self.name == "browser.screenshot").then(|| b"PICTURE".to_vec()),
                read_sources: vec![("web".into(), "https://app.example.edu/login".into())],
                ..ToolResult::default()
            }))
        })
    }
}

struct Run {
    gw: Arc<Gateway>,
    db: Arc<Mutex<Db>>,
    grant: RunGrant,
    events: Arc<Mutex<Vec<(String, Value)>>>,
    _dir: tempfile::TempDir,
}

fn task(images: Images, after_pid: i64, explained: bool) -> Run {
    task_with(images, after_pid, explained, None, false)
}

fn task_with(
    images: Images,
    after_pid: i64,
    explained: bool,
    put: Option<(bool, Option<String>)>,
    own_fact: bool,
) -> Run {
    task_in(images, after_pid, explained, put, own_fact, None)
}

/// HAND-570: the Mac as both put-backs and the sampler see it: the front app, and every activation FERB made.
#[derive(Default)]
struct Desk {
    front: i64,
    activations: Vec<i64>,
}

const BRAVE_PID: i64 = 77;

fn app(pid: i64) -> Value {
    let name = if pid == CHROME_PID {
        "Google Chrome"
    } else {
        "Brave Browser"
    };
    json!({"pid": pid, "bundle_id": "invented.bundle", "name": name})
}

/// The sampler over the shared desk: the front app right before and after each call.
struct Sampled {
    desk: Arc<Mutex<Desk>>,
    before: Mutex<i64>,
}

impl Sentinel for Sampled {
    fn call_started(&self, _task: &str, _span: &str, _name: &str) {}
    fn before_action<'a>(
        &'a self,
        _task: &'a str,
        _span: &'a str,
        _name: &'a str,
        _arguments: &'a Map<String, Value>,
        _on_screen: bool,
    ) -> BoxFuture<'a, Result<(), String>> {
        *self.before.lock().unwrap() = self.desk.lock().unwrap().front;
        Box::pin(async { Ok(()) })
    }
    fn call_ended(&self, _span: &str) {}
    fn after_action<'a>(
        &'a self,
        _span: &'a str,
    ) -> BoxFuture<'a, Result<Map<String, Value>, String>> {
        let (before, after) = (
            *self.before.lock().unwrap(),
            self.desk.lock().unwrap().front,
        );
        let mut changed = Map::new();
        if before != after {
            changed.insert(
                "frontmost_app".into(),
                json!({"before": app(before), "after": app(after)}),
            );
        }
        Box::pin(async move { Ok(changed) })
    }
}

/// The live 2026-11-16 click: the sign-in pop-up brings Chrome (the tab's browser) to the front during the call, the
/// extension minimizes it and reports `surfaced` (focused); the browser part's put-back runs as that fact arrives
/// (`fact_path`: whether it can then), and the gateway's own at the call's end, both on the same desk.
struct Popup {
    desk: Arc<Mutex<Desk>>,
    fact_path: bool,
}

/// The Mac's put-back as both paths use it: only while the browser is the front app.
fn put_back_on(desk: &Mutex<Desk>, to: i64) -> (bool, Option<String>) {
    let mut d = desk.lock().unwrap();
    if d.front != CHROME_PID {
        return (false, Some("another app is in front now".into()));
    }
    d.front = to;
    d.activations.push(to);
    (true, None)
}

impl Tool for Popup {
    fn name(&self) -> &str {
        "browser.act"
    }
    fn facts(&self) -> Facts {
        Facts {
            on_screen: true,
            ..Facts::default()
        }
    }
    fn describe(&self, _args: &Args) -> ActionSpec {
        ActionSpec {
            target: Target {
                site: Some("app.example.edu".into()),
                page: Some("https://app.example.edu/login".into()),
                ..Target::default()
            },
            ..ActionSpec::new("click")
        }
    }
    fn file_paths(&self, _args: &Args) -> Option<Vec<(String, String)>> {
        Some(vec![])
    }
    fn background_pid(&self, _args: &Args) -> Option<i64> {
        Some(CHROME_PID)
    }
    fn put_back<'a>(
        &'a self,
        _front: &'a Value,
        previous: &'a Value,
        _task: &'a str,
    ) -> BoxFuture<'a, Option<(bool, Option<String>)>> {
        let to = previous["pid"].as_i64().unwrap();
        let out = put_back_on(&self.desk, to);
        Box::pin(async move { Some(out) })
    }
    fn run<'a>(
        &'a self,
        _args: &'a Args,
        _ctx: &'a ToolContext,
    ) -> BoxFuture<'a, Result<ToolOutput, ToolError>> {
        Box::pin(async move {
            let before = self.desk.lock().unwrap().front;
            self.desk.lock().unwrap().front = CHROME_PID; // the pop-up came up with the focus
            let mut fact = json!({"cause": "popup_window", "window": "popup", "focused": true, "put_back": true});
            if self.fact_path {
                let (back, why) = put_back_on(&self.desk, before);
                fact["app_put_back"] = json!(back);
                fact["previous"] = json!("Brave Browser");
                if let Some(w) = why {
                    fact["app_why"] = json!(w);
                }
            }
            let mut content = json!({"ok": true}).as_object().cloned().unwrap();
            content.insert("window_shown".into(), json!([fact]));
            Ok(ToolOutput::from(ToolResult {
                content,
                ..ToolResult::default()
            }))
        })
    }
}

fn task_in(
    images: Images,
    after_pid: i64,
    explained: bool,
    put: Option<(bool, Option<String>)>,
    own_fact: bool,
    popup: Option<(Arc<Mutex<Desk>>, bool)>,
) -> Run {
    let dir = tempfile::Builder::new().prefix("fsn").tempdir().unwrap();
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
    let mut reg = Registry::new();
    for name in ["browser.act", "browser.screenshot"] {
        if let (Some((desk, fact_path)), "browser.act") = (&popup, name) {
            reg.add(Arc::new(Popup {
                desk: desk.clone(),
                fact_path: *fact_path,
            }))
            .unwrap();
            continue;
        }
        reg.add(Arc::new(Browser {
            name,
            explained,
            put: put.clone(),
            own_fact,
        }))
        .unwrap();
    }
    let sentinel: Arc<dyn Sentinel> = match &popup {
        Some((desk, _)) => Arc::new(Sampled {
            desk: desk.clone(),
            before: Mutex::new(0),
        }),
        None => Arc::new(FrontApp { after_pid }),
    };
    let events: Arc<Mutex<Vec<(String, Value)>>> = Arc::default();
    let seen = events.clone();
    let deps = Deps {
        db: db.clone(),
        clock: clock.clone(),
        ids: ids.clone(),
        redactor: redactor.clone(),
        ledger,
        publish: Box::new(move |topic, data: Py| {
            seen.lock()
                .unwrap()
                .push((topic.to_string(), ferb_gateway::gateway::py_value(&data)));
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
        route_question: Some(Arc::new(Router)),
        hooks: Arc::new(RealHooks::new(HookParts {
            sentinel: Some(sentinel),
            previews: None,
            evidence: None,
            changes: None,
            images: Some(Arc::new(images)),
            app_windows: None,
            shadow: None,
            publish: Arc::new(|_t, _d| {}),
            journal: Arc::new(|_k, _d| {}),
            workspace: home.join("workspace").to_str().unwrap().to_string(),
            home: user.to_str().unwrap().to_string(),
        })),
        made_in_tree: None,
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
        words: "check my dashboard on app.example.edu".into(),
        targets: Targets {
            sites: ["app.example.edu".to_string()].into_iter().collect(),
            ..Targets::default()
        },
        payload: String::new(),
        asked_origins: BTreeSet::new(),
    });
    let now = clock.now();
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
    let grant = RunGrant {
        task_id: "task-1".into(),
        worker_id: "w1".into(),
        scopes: ["browser:read", "browser:act"]
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
        events,
        _dir: dir,
    }
}

impl Run {
    /// One call; the reply as the model receives it.
    async fn call(&self, tool: &str) -> Value {
        let args = json!({"tab": "t1", "element": "e1", "action": "click"});
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
        let mut out = reply.to_json();
        if let Some(m) = out.as_object_mut() {
            m.remove("image");
        }
        out
    }

    /// The call's tool.call span output, as `ferb trace` reads it.
    fn span_output(&self, tool: &str) -> Value {
        let db = self.db.lock().unwrap();
        let out: String = db
            .conn()
            .query_row(
                "SELECT output_json FROM spans WHERE kind = 'tool.call' AND json_extract(input_json, '$.tool') = ?",
                [tool],
                |r| r.get(0),
            )
            .unwrap();
        serde_json::from_str(&out).unwrap()
    }

    fn shown(&self) -> Vec<Value> {
        self.events
            .lock()
            .unwrap()
            .iter()
            .filter(|(t, _)| t == "browser.window_shown")
            .map(|(_, d)| d.clone())
            .collect()
    }
}

// decision: HAND-548
#[tokio::test]
async fn an_images_faults_are_on_its_calls_span_and_nowhere_the_model_reads() {
    let withheld = task(Images { recovered: false }, 77, false);
    let reply = withheld.call("browser.screenshot").await;
    assert_eq!(
        reply["content"]["image_withheld"],
        json!("image redaction unavailable"),
        "{reply}"
    );
    let said = reply.to_string();
    assert!(
        !said.contains("faults")
            && !said.contains("com.apple.Vision")
            && !said.contains("image_redaction"),
        "the model reads none of it: {said}"
    );
    let out = withheld.span_output("browser.screenshot");
    assert_eq!(
        out["image_redaction"],
        json!({"faults": Images::faults(), "withheld": true}),
        "{out}"
    );

    // read by a later attempt: the model gets the image as before; the span says which attempt and what failed first
    let recovered = task(Images { recovered: true }, 77, false);
    let reply = recovered.call("browser.screenshot").await;
    assert!(reply["content"].get("image_withheld").is_none(), "{reply}");
    assert!(!reply.to_string().contains("faults"), "{reply}");
    let out = recovered.span_output("browser.screenshot");
    assert_eq!(
        out["image_redaction"],
        json!({"faults": Images::faults(), "read_by": "child"}),
        "{out}"
    );
}

// decision: HAND-546
#[tokio::test]
async fn the_calls_own_browser_coming_to_the_front_is_told_once_explained_or_not_ours_is_not() {
    // Chrome (the tab's browser) came to the front during the click: one event for the owner's line
    let ours = task(Images { recovered: true }, CHROME_PID, false);
    let reply = ours.call("browser.act").await;
    assert_eq!(
        reply["content"]["screen_changed_during_call"]["front_app"],
        json!({"before": "Brave Browser", "after": "Google Chrome"}),
        "the worker's fact is as before: {reply}"
    );
    assert_eq!(
        ours.shown(),
        vec![json!({"task_id": "task-1", "site": "app.example.edu",
                    "shown": [{"cause": "front_app", "app": "Google Chrome"}]})]
    );
    // another app came to the front (the owner's own switch, as far as structure says): nothing for the owner
    let other = task(Images { recovered: true }, 91, false);
    other.call("browser.act").await;
    assert!(other.shown().is_empty());
    // the extension already explained it (its window_shown fact): that fact's line is the owner's, not this one
    let explained = task(Images { recovered: true }, CHROME_PID, true);
    explained.call("browser.act").await;
    assert!(explained.shown().is_empty());
}

// decision: HAND-560
#[tokio::test]
async fn the_samplers_fact_joins_a_tools_own_and_the_tools_fields_win() {
    // the tool reported its own fact (a Mac tool's: it put the app back, raw input asks now); the sampler saw a change
    let run = task_with(Images { recovered: true }, 91, false, None, true);
    let reply = run.call("browser.act").await;
    assert_eq!(
        reply["content"]["screen_changed_during_call"],
        json!({"front_app": {"before": "Brave Browser", "after": "Notes"}, "by": "the app", "put_back": true,
               "raw_input_asks": true, "why": "invented", "next": "FERB put Brave Browser back in front."}),
        "{reply}"
    );
    // without a fact of its own, the sampler's stands as before
    let plain = task(Images { recovered: true }, 91, false);
    let reply = plain.call("browser.act").await;
    assert_eq!(
        reply["content"]["screen_changed_during_call"],
        json!({"front_app": {"before": "Brave Browser", "after": "Notes"}, "put_back": false}),
        "{reply}"
    );
}

// decision: HAND-560
#[tokio::test]
async fn the_calls_own_browser_at_the_front_puts_the_owners_app_back_unless_someone_switched() {
    let back = task_with(
        Images { recovered: true },
        CHROME_PID,
        false,
        Some((true, None)),
        false,
    );
    let reply = back.call("browser.act").await;
    assert_eq!(
        reply["content"]["screen_changed_during_call"]["put_back"],
        json!(true),
        "the worker is told it went back: {reply}"
    );
    assert_eq!(
        back.shown(),
        vec![json!({"task_id": "task-1", "site": "app.example.edu",
                    "shown": [{"cause": "front_app", "app": "Google Chrome", "put_back": true,
                               "previous": "Brave Browser"}]})]
    );
    // someone switched to another app since: nothing activated (the Mac's put-back says why), told as it is
    let switched = task_with(
        Images { recovered: true },
        CHROME_PID,
        false,
        Some((false, Some("another app is in front now".into()))),
        false,
    );
    let reply = switched.call("browser.act").await;
    let fact = &reply["content"]["screen_changed_during_call"];
    assert_eq!(fact["put_back"], json!(false), "{reply}");
    assert_eq!(fact["why"], json!("another app is in front now"));
    assert_eq!(
        switched.shown()[0]["shown"][0],
        json!({"cause": "front_app", "app": "Google Chrome", "put_back": false, "previous": "Brave Browser",
               "why": "another app is in front now"})
    );
}

/// HAND-570: when the page's own fact already told the owner (its put-back ran when the fact arrived), the call's end
/// tries once more only while the browser is still in front, and tells the worker; the owner's line isn't repeated.
// decision: HAND-570
#[tokio::test]
async fn at_the_calls_end_the_owners_app_goes_back_once_more_without_a_second_line() {
    let run = task_with(
        Images { recovered: true },
        CHROME_PID,
        true,
        Some((true, None)),
        false,
    );
    let reply = run.call("browser.act").await;
    assert_eq!(
        reply["content"]["screen_changed_during_call"]["put_back"],
        json!(true),
        "{reply}"
    );
    assert!(run.shown().is_empty(), "told once, by the page's fact");
}

/// HAND-570, the live sequence of 2026-11-16 03:06: in one browser.act a sign-in pop-up took the focus (the
/// extension's surfaced fact, focused) and the sampler saw Brave Browser give way to Google Chrome. Before, the page's
/// fact "explained" the change, so the call-end put-back never ran and Chrome stayed in front until the owner switched
/// back by hand. Now Brave is re-activated exactly once: when the fact arrives, or, when that couldn't, at the call's
/// end.
// decision: HAND-570
#[tokio::test]
async fn the_live_popup_sequence_puts_the_owners_app_back_exactly_once() {
    for fact_path in [true, false] {
        let desk = Arc::new(Mutex::new(Desk {
            front: BRAVE_PID,
            activations: vec![],
        }));
        let run = task_in(
            Images { recovered: true },
            0,
            false,
            None,
            false,
            Some((desk.clone(), fact_path)),
        );
        let reply = run.call("browser.act").await;
        let d = desk.lock().unwrap();
        assert_eq!(
            d.activations,
            vec![BRAVE_PID],
            "once (fact path: {fact_path}): {reply}"
        );
        assert_eq!(d.front, BRAVE_PID);
        drop(d);
        let fact = &reply["content"]["window_shown"][0];
        if fact_path {
            // put back as the fact arrived: the line's fields are on the fact; the call's end found nothing to do
            assert_eq!(fact["app_put_back"], json!(true), "{reply}");
            assert!(
                reply["content"].get("screen_changed_during_call").is_none(),
                "{reply}"
            );
        } else {
            // put back at the call's end: the worker's fact says so, and the owner isn't told twice
            assert_eq!(
                reply["content"]["screen_changed_during_call"]["put_back"],
                json!(true),
                "{reply}"
            );
        }
        assert!(
            run.shown().is_empty(),
            "the page's fact is the owner's line, told by the browser part"
        );
    }
}
