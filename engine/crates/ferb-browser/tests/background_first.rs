//! Background first in browser tasks, the daemon's half (HAND-533, HAND-535; extension 0.19.11). The extension's
//! replies carry `surfaced`: what brought a browser window forward around a task tab (and whether FERB's own window
//! went back down), or what the page asked for and was refused. The bridge takes those facts out of whatever reply
//! carried them, keeps only their known fields, and the tab's next result tells the worker (`window_shown`), once.
//! Through a scripted fake extension on in-memory connections (signed as the real one signs), an invented profile and
//! tokio's paused clock. The extension's own half is tests/extension_background.rs.

mod common;

use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ferb_browser::bridge::{Hub, Ledger};
use ferb_browser::driver::{Kind, Tasks};
use ferb_browser::pairing::{InstallRow, InstallStore, PairingKey, Pairings, StoreError};
use ferb_browser::protocol::{sign, MIN_EXTENSION_VERSION, PROTOCOL_VERSION};
use ferb_browser::rules::{BrowserApps, BrowserInfo};
use ferb_browser::tools::{verb, Host, Policy, Tools, OWNER_BROWSER};
use ferb_core::interfaces::{BoxFuture, Clock};
use ferb_core::types::{Identity, TabRef, ToolContext};
use ferb_core::when::DateTime;
use serde_json::{json, Map, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use common::Counter;

const KEY: [u8; 32] = [9; 32];
const INSTALL: &str = "install-chrome";

struct Installs;

impl InstallStore for Installs {
    fn row(&self, install_id: &str) -> Result<Option<InstallRow>, StoreError> {
        Ok((install_id == INSTALL).then(|| InstallRow {
            browser: "chrome".into(),
            folder: "Default".into(),
            identity: Identity::Owner,
        }))
    }
    fn install_for(&self, browser: &str, folder: &str) -> Result<Option<String>, StoreError> {
        Ok((browser == "chrome" && folder == "Default").then(|| INSTALL.to_string()))
    }
    fn installs(&self) -> Result<Vec<String>, StoreError> {
        Ok(vec![INSTALL.into()])
    }
    fn key(&self, install_id: &str) -> Result<Option<PairingKey>, StoreError> {
        Ok((install_id == INSTALL).then(|| PairingKey::new(KEY.to_vec())))
    }
    fn key_and_seq(&self, install_id: &str) -> Result<Option<(PairingKey, i64)>, StoreError> {
        Ok((install_id == INSTALL).then(|| (PairingKey::new(KEY.to_vec()), 0)))
    }
    fn version(&self, _: &str) -> Result<Option<String>, StoreError> {
        Ok(Some(MIN_EXTENSION_VERSION.into()))
    }
    fn set_version(&self, _: &str, _: &str) -> Result<(), StoreError> {
        Ok(())
    }
    fn set_last_seq(&self, _: &str, _: i64) -> Result<(), StoreError> {
        Ok(())
    }
    fn unpair(&self, _: &str, _: &str) -> Result<bool, StoreError> {
        Ok(false)
    }
    fn pair(&self, _: &str, _: &InstallRow, _: &PairingKey, _: &str) -> Result<(), StoreError> {
        Ok(())
    }
}

struct Clocked;

impl Clock for Clocked {
    fn now(&self) -> DateTime {
        ferb_core::when::parse("2026-01-01T09:00:00+00:00").unwrap()
    }
    fn monotonic(&self) -> f64 {
        0.0
    }
    fn sleep_until(&self, _: DateTime) -> BoxFuture<'_, ()> {
        Box::pin(async {})
    }
}

struct NoLedger;

impl Ledger for NoLedger {
    fn state(&self, _: &str) -> Option<String> {
        None
    }
    fn reconcile(&self, _: &str, _: &str, _: Vec<String>) {}
}

struct Apps;

impl BrowserApps for Apps {
    fn known(&self, _: &str) -> bool {
        true
    }
    fn browser(&self, _: &str) -> Option<BrowserInfo> {
        None
    }
}

/// Every address is an https one of its own host.
struct Https;

impl Policy for Https {
    fn parse(&self, url: &str) -> Option<(String, String)> {
        let rest = url.strip_prefix("https://")?;
        Some(("https".into(), rest.split('/').next().unwrap_or("").into()))
    }
    fn typed_destination(&self, _: &str) -> Option<String> {
        None
    }
}

struct NoHost;

impl Host for NoHost {
    fn pair(
        &self,
        _: &str,
        _: &str,
    ) -> Option<BoxFuture<'_, ferb_browser::Result<Map<String, Value>>>> {
        None
    }
    fn handoff(
        &self,
        _: TabRef,
        _: String,
        _: ToolContext,
    ) -> Option<BoxFuture<'_, ferb_browser::Result<Map<String, Value>>>> {
        None
    }
    fn resolve(&self, p: &str, _: &ToolContext, _: bool) -> ferb_browser::Result<String> {
        Ok(p.into())
    }
    fn owner_path(&self, p: &str, _: &str) -> String {
        p.into()
    }
    fn read(&self, _: &str) -> ferb_browser::Result<Vec<u8>> {
        Ok(vec![])
    }
    fn new_file(&self, _: &str, _: &[u8], _: &ToolContext) -> ferb_browser::Result<()> {
        Ok(())
    }
    fn display_of(&self, _: ferb_browser::mask::Rect) -> Option<i64> {
        None
    }
    fn recordings(&self) -> Option<Arc<dyn ferb_browser::tools::Recordings>> {
        None
    }
}

/// The page FERB loads in its tab brings FERB's window forward (a tab it opened) and also asks for focus, which the
/// extension refused; one fact is of a kind the engine doesn't know, one isn't an object at all.
fn navigated(url: &str, first: bool) -> Value {
    let mut v = json!({"tab_id": 21, "url": url, "title": "Sign in"});
    if first {
        v["surfaced"] = json!([
            {"window": "ferb", "cause": "new_tab", "focused": true, "put_back": true, "new_tab": 22},
            {"cause": "focus", "prevented": true},
            {"window": "ferb", "cause": "made_up", "focused": true, "put_back": true},
            "not a fact",
        ]);
    }
    v
}

/// One connected fake extension answering every command as the real one does when it succeeds; `made`, when given,
/// is the first open's `window_made` (HAND-649).
fn connect(hub: Arc<Hub>, sent: Arc<Mutex<Vec<(String, Value)>>>, made: Option<Value>) {
    let (ext_w, hub_r) = tokio::io::duplex(1 << 20);
    let (hub_w, ext_r) = tokio::io::duplex(1 << 20);
    tokio::spawn(hub.serve(BufReader::new(hub_r), hub_w, Some(4242)));
    tokio::spawn(async move {
        let seq = AtomicI64::new(0);
        let frame = |mtype: &str, body: &Value| {
            let body_json = ferb_py::json::dumps(body);
            let n = seq.fetch_add(1, Ordering::SeqCst) + 1;
            let mac = sign(&KEY, INSTALL, n, mtype, &body_json);
            format!(
                "{}\n",
                json!({"install_id": INSTALL, "type": mtype, "body_json": body_json, "seq": n, "mac": mac})
            )
        };
        let mut w = ext_w;
        let hello = json!({"version": MIN_EXTENSION_VERSION, "protocol": PROTOCOL_VERSION, "browser": "chrome",
                           "paired": true, "nonce": "nonce-1", "tabs": []});
        w.write_all(frame("hello", &hello).as_bytes())
            .await
            .unwrap();
        let mut lines = BufReader::new(ext_r).lines();
        let mut navigations = 0;
        let mut made = made;
        while let Ok(Some(line)) = lines.next_line().await {
            let v: Value = serde_json::from_str(&line).unwrap();
            if v["type"] != "cmd" {
                continue;
            }
            let cmd: Value = serde_json::from_str(v["body"].as_str().unwrap()).unwrap();
            let op = cmd["op"].as_str().unwrap().to_string();
            sent.lock().unwrap().push((op.clone(), cmd["args"].clone()));
            let value = match op.as_str() {
                "open" => {
                    let mut v = json!({"tab_id": 21, "url": "about:blank", "title": "", "window_id": 5,
                                       "group_id": null, "muted": true});
                    if let Some(m) = made.take() {
                        v["window_made"] = m;
                    }
                    v
                }
                "navigate" => {
                    navigations += 1;
                    let url = cmd["args"]["url"].as_str().unwrap_or("").to_string();
                    navigated(&url, navigations == 2)
                }
                _ => json!({}),
            };
            let result =
                json!({"id": cmd["id"], "gen": cmd["gen"], "status": "ok", "value": value});
            if w.write_all(frame("result", &result).as_bytes())
                .await
                .is_err()
            {
                break;
            }
        }
    });
}

async fn settle() {
    for _ in 0..50 {
        tokio::task::yield_now().await;
    }
    tokio::time::sleep(Duration::from_millis(50)).await;
}

fn ctx() -> ToolContext {
    ToolContext {
        task_id: "task-a".into(),
        worker_id: "task-a.worker".into(),
        request_id: "r-1".into(),
        identity: Identity::Owner,
        scopes: Default::default(),
        device_id: "dev-1".into(),
        span_id: None,
        workspace: "/tmp/invented".into(),
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

// decision: HAND-533
// decision: HAND-546
#[tokio::test(start_paused = true)]
async fn a_window_the_page_brought_forward_is_the_tabs_next_fact_once() {
    let random = Arc::new(Counter::new());
    let pairings = Arc::new(Pairings::new(
        Arc::new(Installs),
        Arc::new(Clocked),
        random.clone(),
    ));
    let published: Arc<Mutex<Vec<(String, Value)>>> = Arc::default();
    let seen = published.clone();
    let hub = Hub::new(
        pairings,
        Arc::new(NoLedger),
        Arc::new(move |topic: &str, data: Map<String, Value>| {
            seen.lock()
                .unwrap()
                .push((topic.to_string(), Value::Object(data)));
        }),
        Arc::new(|| "dev-1".to_string()),
        random.clone(),
        Arc::new(Apps),
        Arc::new(|_| Some("com.google.Chrome".to_string())),
    );
    let tasks = Tasks {
        running: Arc::new(|t: &str| t == "task-a"),
        root: Arc::new(|t: &str| t.split('.').next().unwrap_or(t).to_string()),
        ..Tasks::default()
    };
    let driver = ferb_browser::driver::Driver::new(
        hub.clone(),
        Kind::Owner {
            choose: Arc::new(|_| Ok(("chrome".to_string(), "Default".to_string()))),
            input: None,
        },
        tasks,
        None,
        random,
    );
    let tools = Tools {
        family: OWNER_BROWSER,
        driver: driver.clone(),
        policy: Arc::new(Https),
        host: Arc::new(NoHost),
    };
    let sent: Arc<Mutex<Vec<(String, Value)>>> = Arc::default();
    connect(hub, sent.clone(), None);
    settle().await;

    // the task's tab: opened blank, then its address (the fake's first navigate carries nothing)
    let tab = driver
        .open_for("https://sign.example/start", "task-a", None, None, false)
        .await
        .expect("open");
    let nav = verb("navigate").unwrap();
    let args = json!({"tab": tab.tab_id, "url": "https://sign.example/login"});
    let first = tools.run(nav, &args, &ctx()).await.expect("navigate");
    let shown = first.content["window_shown"].clone();
    assert_eq!(
        shown,
        json!([
            "the page opened a new tab, and the browser brought FERB's minimized browser window to the front and took \
             the keyboard focus from the owner's app; FERB minimized it again at once. FERB doesn't switch the owner \
             back to their app",
            "the page asked to bring its window to the front (window.focus); FERB refused it in the page, so nothing \
             came forward",
        ]),
        "{:?}",
        first.content
    );
    assert!(
        !first.content.contains_key("surfaced"),
        "the extension's raw list never reaches the worker"
    );
    // HAND-546: the window that came up (not the refused request) goes out for the owner's line, with the task's
    // tree and the tab's site
    let shown: Vec<Value> = published
        .lock()
        .unwrap()
        .iter()
        .filter(|(t, _)| t == "browser.window_shown")
        .map(|(_, d)| d.clone())
        .collect();
    assert_eq!(
        shown,
        vec![
            json!({"task_id": "task-a", "tab": tab.tab_id, "site": "sign.example",
                    "shown": [{"cause": "new_tab", "window": "ferb", "focused": true, "put_back": true}]})
        ]
    );

    // told once: the next result of the tab has nothing more
    let args = json!({"tab": tab.tab_id, "url": "https://sign.example/next"});
    let again = tools.run(nav, &args, &ctx()).await.expect("navigate");
    assert!(
        !again.content.contains_key("window_shown"),
        "{:?}",
        again.content
    );
    assert!(driver.surfaced_for(&tab).is_empty());
    let more = published
        .lock()
        .unwrap()
        .iter()
        .filter(|(t, _)| t == "browser.window_shown")
        .count();
    assert_eq!(more, 1, "nothing new: nothing more for the owner");
}

// decision: HAND-533
#[test]
fn only_known_facts_of_known_shapes_are_kept_and_each_reads_plainly() {
    use ferb_browser::surfaced::{fact, told};
    // a known fact keeps its fields; anything else in it is dropped
    let f = fact(
        &json!({"window": "popup", "cause": "popup_window", "focused": false, "put_back": true,
                         "new_tab": 22, "url": "https://x.example/"}),
    )
    .unwrap();
    assert_eq!(
        Value::Object(f.clone()),
        json!({"cause": "popup_window", "window": "popup", "focused": false, "put_back": true})
    );
    assert_eq!(
        told(&f),
        "the page opened a window of its own (a popup, as a sign-in does), and the browser brought that window to the \
         front; FERB minimized it again at once. FERB doesn't switch the owner back to their app"
    );
    // the owner's own window is never put back: a fact only
    let owner =
        fact(&json!({"window": "owner", "cause": "dialog", "focused": true, "put_back": false}))
            .unwrap();
    assert_eq!(
        told(&owner),
        "the page showed a dialog, and the browser brought the owner's browser window holding this tab to the front \
         and took the keyboard focus from the owner's app; FERB leaves the owner's windows as they are. FERB doesn't \
         switch the owner back to their app"
    );
    let full = fact(&json!({"cause": "fullscreen", "prevented": true})).unwrap();
    assert_eq!(
        told(&full),
        "the page asked to go full screen; FERB refused it in the page, so nothing came forward"
    );
    // not facts: an unknown cause or window, a window fact without a window, a string
    for bad in [
        json!({"window": "ferb", "cause": "made_up"}),
        json!({"window": "somewhere", "cause": "new_tab"}),
        json!({"cause": "new_tab", "put_back": true}),
        json!("new_tab"),
    ] {
        assert_eq!(fact(&bad), None, "{bad}");
    }
}

/// HAND-570: the daemon's put-back as the browser part sees it: what it was asked, and its answer.
struct PutsBack {
    asked: Mutex<Vec<(String, i32)>>,
}

impl Host for PutsBack {
    fn pair(
        &self,
        _: &str,
        _: &str,
    ) -> Option<BoxFuture<'_, ferb_browser::Result<Map<String, Value>>>> {
        None
    }
    fn handoff(
        &self,
        _: TabRef,
        _: String,
        _: ToolContext,
    ) -> Option<BoxFuture<'_, ferb_browser::Result<Map<String, Value>>>> {
        None
    }
    fn resolve(&self, p: &str, c: &ToolContext, e: bool) -> ferb_browser::Result<String> {
        NoHost.resolve(p, c, e)
    }
    fn owner_path(&self, p: &str, w: &str) -> String {
        NoHost.owner_path(p, w)
    }
    fn read(&self, p: &str) -> ferb_browser::Result<Vec<u8>> {
        NoHost.read(p)
    }
    fn new_file(&self, p: &str, d: &[u8], c: &ToolContext) -> ferb_browser::Result<()> {
        NoHost.new_file(p, d, c)
    }
    fn display_of(&self, r: ferb_browser::mask::Rect) -> Option<i64> {
        NoHost.display_of(r)
    }
    fn recordings(&self) -> Option<Arc<dyn ferb_browser::tools::Recordings>> {
        None
    }
    fn put_back_owner_app(
        &self,
        task: &str,
        browser_pid: i32,
    ) -> Option<BoxFuture<'_, Option<Map<String, Value>>>> {
        self.asked
            .lock()
            .unwrap()
            .push((task.to_string(), browser_pid));
        Some(Box::pin(async {
            json!({"app_put_back": true, "previous": "Brave Browser"})
                .as_object()
                .cloned()
        }))
    }
}

/// HAND-570: the window of the task's that took the focus is the moment the owner's app goes back: asked of the
/// daemon while the tab's result is made (not at a later call), its answer on the fact, the worker's line and the
/// owner's.
// decision: HAND-570
#[tokio::test(start_paused = true)]
async fn the_owners_app_goes_back_when_the_fact_arrives() {
    let answering = Arc::new(PutsBack {
        asked: Mutex::new(vec![]),
    });
    let random = Arc::new(Counter::new());
    let pairings = Arc::new(Pairings::new(
        Arc::new(Installs),
        Arc::new(Clocked),
        random.clone(),
    ));
    let published: Arc<Mutex<Vec<(String, Value)>>> = Arc::default();
    let seen = published.clone();
    let hub = Hub::new(
        pairings,
        Arc::new(NoLedger),
        Arc::new(move |topic: &str, data: Map<String, Value>| {
            seen.lock()
                .unwrap()
                .push((topic.to_string(), Value::Object(data)));
        }),
        Arc::new(|| "dev-1".to_string()),
        random.clone(),
        Arc::new(Apps),
        Arc::new(|_| Some("com.google.Chrome".to_string())),
    );
    let tasks = Tasks {
        running: Arc::new(|t: &str| t == "task-a"),
        root: Arc::new(|t: &str| t.split('.').next().unwrap_or(t).to_string()),
        ..Tasks::default()
    };
    let driver = ferb_browser::driver::Driver::new(
        hub.clone(),
        Kind::Owner {
            choose: Arc::new(|_| Ok(("chrome".to_string(), "Default".to_string()))),
            input: None,
        },
        tasks,
        None,
        random,
    );
    let tools = Tools {
        family: OWNER_BROWSER,
        driver: driver.clone(),
        policy: Arc::new(Https),
        host: answering.clone(),
    };
    let sent: Arc<Mutex<Vec<(String, Value)>>> = Arc::default();
    connect(hub, sent.clone(), None);
    settle().await;

    // the task's tab: opened blank, then its address (the fake's first navigate carries nothing)
    let tab = driver
        .open_for("https://sign.example/start", "task-a", None, None, false)
        .await
        .expect("open");
    let nav = verb("navigate").unwrap();
    let args = json!({"tab": tab.tab_id, "url": "https://sign.example/login"});
    let first = tools.run(nav, &args, &ctx()).await.expect("navigate");
    assert_eq!(
        *answering.asked.lock().unwrap(),
        [("task-a".to_string(), 4242)]
    );
    assert_eq!(
        first.content["window_shown"][0],
        json!(
            "the page opened a new tab, and the browser brought FERB's minimized browser window to the front and took \
             the keyboard focus from the owner's app; FERB minimized it again at once. FERB put Brave Browser back in \
             front"
        )
    );
    let shown: Vec<Value> = published
        .lock()
        .unwrap()
        .iter()
        .filter(|(t, _)| t == "browser.window_shown")
        .map(|(_, d)| d["shown"].clone())
        .collect();
    assert_eq!(
        shown,
        vec![
            json!([{"cause": "new_tab", "window": "ferb", "focused": true, "put_back": true,
                     "app_put_back": true, "previous": "Brave Browser"}])
        ]
    );
    // the refused request alone (no window came up) asks nothing more
    let args = json!({"tab": tab.tab_id, "url": "https://sign.example/next"});
    tools.run(nav, &args, &ctx()).await.expect("navigate");
    assert_eq!(answering.asked.lock().unwrap().len(), 1);
}

// Live 2026-11-21: macOS opened FERB's new window full screen on a Space of its own (the owner's Chrome window was
// full screen); the extension's fact about it (0.19.13) is the open's line for the worker and the trace, once, and
// only its known fields count.
// decision: HAND-649
#[tokio::test(start_paused = true)]
async fn a_window_made_full_screen_is_the_opens_fact_once() {
    use ferb_browser::surfaced::window_made;
    let random = Arc::new(Counter::new());
    let pairings = Arc::new(Pairings::new(
        Arc::new(Installs),
        Arc::new(Clocked),
        random.clone(),
    ));
    let hub = Hub::new(
        pairings,
        Arc::new(NoLedger),
        Arc::new(|_: &str, _: Map<String, Value>| {}),
        Arc::new(|| "dev-1".to_string()),
        random.clone(),
        Arc::new(Apps),
        Arc::new(|_| Some("com.google.Chrome".to_string())),
    );
    let tasks = Tasks {
        running: Arc::new(|t: &str| t == "task-a"),
        root: Arc::new(|t: &str| t.split('.').next().unwrap_or(t).to_string()),
        ..Tasks::default()
    };
    let driver = ferb_browser::driver::Driver::new(
        hub.clone(),
        Kind::Owner {
            choose: Arc::new(|_| Ok(("chrome".to_string(), "Default".to_string()))),
            input: None,
        },
        tasks,
        None,
        random,
    );
    let tools = Tools {
        family: OWNER_BROWSER,
        driver: driver.clone(),
        policy: Arc::new(Https),
        host: Arc::new(NoHost),
    };
    let sent: Arc<Mutex<Vec<(String, Value)>>> = Arc::default();
    let made = json!({"fullscreen": true, "minimized": true, "tries": 1, "window_id": 5});
    connect(hub, sent.clone(), Some(made));
    settle().await;

    let open = verb("open").unwrap();
    let args = json!({"url": "https://sign.example/start"});
    let first = tools.run(open, &args, &ctx()).await.expect("open");
    assert_eq!(
        first.content["window_made"],
        json!(
            "this profile had no window, so FERB made its own minimized one; macOS first opened it full screen on a \
             Space of its own (the owner's screen may have switched Space for a moment); it is minimized now, and later \
             tabs in this profile open in it"
        ),
        "{:?}",
        first.content
    );
    let second = tools.run(open, &args, &ctx()).await.expect("open");
    assert!(
        !second.content.contains_key("window_made"),
        "{:?}",
        second.content
    );
    // only the known fields of a known shape
    assert_eq!(
        window_made(Some(
            &json!({"fullscreen": false, "minimized": false, "tries": 2, "x": 1})
        )),
        Some(
            json!({"fullscreen": false, "minimized": false, "tries": 2})
                .as_object()
                .unwrap()
                .clone()
        )
    );
    for bad in [
        json!({"fullscreen": true}),
        json!("made"),
        json!({"fullscreen": 1, "minimized": true}),
    ] {
        assert_eq!(window_made(Some(&bad)), None, "{bad}");
    }
}
