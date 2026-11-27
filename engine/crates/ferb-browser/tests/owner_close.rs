//! FERB closes the owner's own tab when his words ask for it (HAND-724, superseding HAND-201's "never closed"; extension
//! 0.19.15). browser.close's `owner_asked` is the worker's declaration, never code's (rule 4): with it a tab of his the
//! task took over (use_tab) is closed through the extension's `close`, and the result says plainly that it was his;
//! without it the tab is let go of, open, exactly as before. An extension older than 0.19.15 doesn't know the field and
//! keeps refusing, so FERB says it needs updating (nothing sent when its hello says so). Through a scripted fake
//! extension on in-memory connections (signed as the real one signs), an invented profile and tokio's paused clock.
//! The extension's own half is in tests/extension_background.rs.

mod common;

use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ferb_browser::bridge::{Hub, Ledger};
use ferb_browser::driver::{Driver, Kind, Tasks, OWNER_TAB_CLOSED};
use ferb_browser::pairing::{InstallRow, InstallStore, PairingKey, Pairings, StoreError};
use ferb_browser::protocol::{sign, OWNER_CLOSE_EXTENSION_VERSION, PROTOCOL_VERSION};
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
/// The owner's own tab in the fake browser (FERB never opened it).
const OWNERS: i64 = 30;

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
        Ok(None)
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

type Sent = Arc<Mutex<Vec<(String, Value)>>>;

/// One connected fake extension of `version`, answering as the real one: `close` closes a tab FERB opened, and one of
/// the owner's only with `owner_asked` when `knows_owner_asked` (0.19.15), else refuses it (`not_ferbs`).
fn connect(hub: Arc<Hub>, sent: Sent, version: &str, knows_owner_asked: bool) {
    let (ext_w, hub_r) = tokio::io::duplex(1 << 20);
    let (hub_w, ext_r) = tokio::io::duplex(1 << 20);
    tokio::spawn(hub.serve(BufReader::new(hub_r), hub_w, Some(4242)));
    let version = version.to_string();
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
        let hello = json!({"version": version, "protocol": PROTOCOL_VERSION, "browser": "chrome",
                           "paired": true, "nonce": "nonce-1", "tabs": []});
        w.write_all(frame("hello", &hello).as_bytes())
            .await
            .unwrap();
        let mut lines = BufReader::new(ext_r).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            let v: Value = serde_json::from_str(&line).unwrap();
            if v["type"] != "cmd" {
                continue;
            }
            let cmd: Value = serde_json::from_str(v["body"].as_str().unwrap()).unwrap();
            let op = cmd["op"].as_str().unwrap().to_string();
            let args = cmd["args"].clone();
            sent.lock().unwrap().push((op.clone(), args.clone()));
            let value = match op.as_str() {
                "open" => json!({"tab_id": 21, "url": "about:blank", "title": "", "window_id": 5,
                                 "group_id": null, "muted": true}),
                "list_tabs" => json!({"tabs": [
                    {"tab_id": OWNERS, "url": "https://news.example/today", "title": "Today's news"},
                    {"tab_id": 21, "url": "about:blank", "title": ""},
                ]}),
                "adopt_tab" => json!({"adopted": true, "tab_id": args["tab_id"]}),
                "release_tab" => {
                    json!({"tab_id": args["tab_id"], "released": true, "kept_open": false,
                                        "url": "https://news.example/today", "window_id": 3})
                }
                "close" if args["tab_id"] == OWNERS => {
                    if knows_owner_asked && args["owner_asked"] == true {
                        json!({"closed": true, "owner_tab": true})
                    } else {
                        json!({"closed": false, "released": false, "reason": "not_ferbs",
                               "detail": "FERB closes only tabs it opened; this one isn't recorded as FERB's, so it stays open"})
                    }
                }
                "close" => json!({"closed": true}),
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

/// A connected owner browser of `version` with the owner's tab 30 taken over for task-a (use_tab).
async fn world(version: &str, knows: bool) -> (Tools, Arc<Driver>, Sent, String) {
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
    let driver = Driver::new(
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
    let sent: Sent = Arc::default();
    connect(hub, sent.clone(), version, knows);
    settle().await;
    let used = tools
        .run(
            verb("use_tab").unwrap(),
            &json!({"tab_ref": format!("chrome:Default:{OWNERS}")}),
            &ctx(),
        )
        .await
        .expect("use_tab");
    let tab = used.content["tab"].as_str().unwrap().to_string();
    sent.lock().unwrap().clear();
    (tools, driver, sent, tab)
}

fn ops(sent: &Sent) -> Vec<(String, Value)> {
    sent.lock().unwrap().clone()
}

// decision: HAND-724
#[tokio::test(start_paused = true)]
async fn the_owners_tab_is_closed_when_the_worker_says_his_words_asked() {
    let (tools, driver, sent, tab) = world(OWNER_CLOSE_EXTENSION_VERSION, true).await;
    let close = verb("close").unwrap();
    let out = tools
        .run(close, &json!({"tab": tab, "owner_asked": true}), &ctx())
        .await
        .expect("close");
    assert_eq!(
        ops(&sent),
        vec![(
            "close".to_string(),
            json!({"tab_id": OWNERS, "owner_asked": true})
        )],
        "closed through the extension's close, owner_asked passed through; never let go of first"
    );
    assert_eq!(out.content["closed"], true, "{:?}", out.content);
    assert_eq!(out.content["borrowed"], true);
    assert_eq!(out.content["owner_tab_closed"], true);
    assert_eq!(out.content["detail"], OWNER_TAB_CLOSED);
    assert!(
        driver.tab(&tab).is_none(),
        "no longer the task's: nothing left to let go of at its end"
    );
}

// decision: HAND-724
#[tokio::test(start_paused = true)]
async fn without_owner_asked_his_tab_is_let_go_of_open_as_before() {
    let (tools, _driver, sent, tab) = world(OWNER_CLOSE_EXTENSION_VERSION, true).await;
    let close = verb("close").unwrap();
    // unsaid is false: the field's default
    let out = tools
        .run(close, &json!({"tab": tab}), &ctx())
        .await
        .expect("close");
    assert_eq!(
        ops(&sent),
        vec![(
            "release_tab".to_string(),
            json!({"tab_id": OWNERS, "keep_open": false})
        )]
    );
    assert_eq!(out.content["closed"], false, "{:?}", out.content);
    assert_eq!(out.content["released"], true);
    assert!(!out.content.contains_key("owner_tab_closed"));
    // said false: the same
    let (tools, _driver, sent, tab) = world(OWNER_CLOSE_EXTENSION_VERSION, true).await;
    let out = tools
        .run(close, &json!({"tab": tab, "owner_asked": false}), &ctx())
        .await
        .expect("close");
    assert_eq!(ops(&sent)[0].0, "release_tab");
    assert_eq!(out.content["closed"], false);
}

// decision: HAND-724
#[tokio::test(start_paused = true)]
async fn a_tab_ferb_opened_closes_with_the_same_bytes_whatever_owner_asked_says() {
    let (tools, driver, sent, _) = world(OWNER_CLOSE_EXTENSION_VERSION, true).await;
    let opened = driver
        .open_for("https://shop.example/", "task-a", None, None, false)
        .await
        .expect("open");
    sent.lock().unwrap().clear();
    let out = tools
        .run(
            verb("close").unwrap(),
            &json!({"tab": opened.tab_id, "owner_asked": true}),
            &ctx(),
        )
        .await
        .expect("close");
    assert_eq!(
        ops(&sent),
        vec![("close".to_string(), json!({"tab_id": 21}))],
        "FERB's own tab: the close it always sent"
    );
    assert_eq!(out.content["closed"], true);
    assert!(!out.content.contains_key("owner_tab_closed"));
}

// decision: HAND-724
#[tokio::test(start_paused = true)]
async fn an_older_extension_is_never_asked_and_ferb_says_it_needs_updating() {
    let (tools, driver, sent, tab) = world("0.19.14", false).await;
    let out = tools
        .run(
            verb("close").unwrap(),
            &json!({"tab": tab, "owner_asked": true}),
            &ctx(),
        )
        .await
        .expect("a result");
    assert!(out.is_error, "an older extension can't close his tab");
    let text = out.content["error"].as_str().unwrap().to_string();
    assert!(
        text.contains("the FERB extension in this profile is 0.19.14")
            && text.contains("needs 0.19.15 or later")
            && text.contains("is still open"),
        "{text}"
    );
    assert!(ops(&sent).is_empty(), "nothing sent");
    assert!(driver.tab(&tab).is_some(), "still the task's, still held");
}

// decision: HAND-724
#[tokio::test(start_paused = true)]
async fn an_extension_that_still_refuses_his_tab_gets_the_same_update_message() {
    // its hello says 0.19.15 but it answers as an older one: the reply, not the version, is the fact
    let (tools, driver, sent, tab) = world(OWNER_CLOSE_EXTENSION_VERSION, false).await;
    let out = tools
        .run(
            verb("close").unwrap(),
            &json!({"tab": tab, "owner_asked": true}),
            &ctx(),
        )
        .await
        .expect("a result");
    assert!(out.is_error, "refused");
    let text = out.content["error"].as_str().unwrap();
    assert!(
        text.contains("says 0.19.15 but answered as an older version")
            && text.contains("needs 0.19.15 or later")
            && text.contains("is still open"),
        "{text}"
    );
    assert_eq!(ops(&sent).len(), 1);
    assert!(driver.tab(&tab).is_some(), "not let go of: still held");
}
