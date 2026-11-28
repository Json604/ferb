//! ORCH-753: a browser snapshot keeps the earlier ids of the elements it finds again (HAND-488's carry, as the
//! screenshot's own snapshot does) and tells the gateway which; a load in the tab, or another node, refuses them.
//! Through a scripted fake extension on in-memory connections (signed as the real one signs), an invented profile
//! and tokio's paused clock.

mod common;

use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;
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

/// The page's elements in each snapshot, by the snapshot's number: (local id, role, label, backend node).
fn elements(snap: i64) -> Value {
    let link = |id: i64, label: &str, node: i64| json!({"id": id, "role": "link", "label": label, "backend_node": node, "path": format!("/a[{id}]")});
    match snap {
        1 => json!([link(1, "Home", 101), link(2, "Inbox", 102)]),
        // the same Home node; Inbox's node was replaced (a new node with the same words); a new link
        2 => json!([
            link(1, "Home", 101),
            link(2, "Inbox", 202),
            link(3, "Sent", 103)
        ]),
        _ => json!([
            link(1, "Home", 101),
            link(2, "Inbox", 202),
            link(3, "Sent", 103)
        ]),
    }
}

/// One connected fake extension: opens tab 21, answers each snapshot with `elements`, and sends a load of the tab
/// (a tab_event "updated") when told to through `load`.
fn connect(hub: Arc<Hub>, load: tokio::sync::mpsc::UnboundedReceiver<()>) {
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
        let w = Arc::new(tokio::sync::Mutex::new(ext_w));
        let hello = json!({"version": MIN_EXTENSION_VERSION, "protocol": PROTOCOL_VERSION, "browser": "chrome",
                           "paired": true, "nonce": "nonce-1", "tabs": []});
        w.lock()
            .await
            .write_all(frame("hello", &hello).as_bytes())
            .await
            .unwrap();
        let mut load = load;
        let mut lines = BufReader::new(ext_r).lines();
        loop {
            tokio::select! {
                Some(()) = load.recv() => {
                    let ev = json!({"event": "updated", "tab_id": 21, "url": "https://mail.example/inbox",
                                    "title": "Inbox"});
                    w.lock().await.write_all(frame("tab_event", &ev).as_bytes()).await.unwrap();
                }
                line = lines.next_line() => {
                    let Ok(Some(line)) = line else { break };
                    let v: Value = serde_json::from_str(&line).unwrap();
                    if v["type"] != "cmd" {
                        continue;
                    }
                    let cmd: Value = serde_json::from_str(v["body"].as_str().unwrap()).unwrap();
                    let value = match cmd["op"].as_str().unwrap() {
                        "open" => json!({"tab_id": 21, "url": "https://mail.example/inbox", "title": "Inbox",
                                         "window_id": 5, "group_id": null, "muted": true}),
                        "snapshot" => {
                            let snap = cmd["args"]["snap"].as_i64().unwrap();
                            json!({"frames": [{"frame_id": 0, "url": "https://mail.example/inbox", "title": "Inbox",
                                               "text": "Inbox", "elements": elements(snap)}]})
                        }
                        _ => json!({}),
                    };
                    let result =
                        json!({"id": cmd["id"], "gen": cmd["gen"], "status": "ok", "value": value});
                    if w.lock().await.write_all(frame("result", &result).as_bytes()).await.is_err() {
                        break;
                    }
                }
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

// decision: ORCH-753
#[tokio::test(start_paused = true)]
async fn a_snapshot_keeps_the_ids_of_the_elements_it_finds_again_until_the_page_loads() {
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
    let (loads, load) = tokio::sync::mpsc::unbounded_channel();
    connect(hub, load);
    settle().await;
    let tab = driver
        .open_for("https://mail.example/inbox", "task-a", None, None, false)
        .await
        .expect("open");
    let snap = verb("snapshot").unwrap();
    let args = json!({"tab": tab.tab_id});
    let resolves = |id: &str| {
        driver
            .resolve(&driver.element_ref(&tab, id))
            .map(|r| r.4.id)
    };

    let first = tools.run(snap, &args, &ctx()).await.expect("snapshot");
    assert_eq!(
        first.carried,
        Some(Map::new()),
        "a first look carries nothing"
    );
    let second = tools.run(snap, &args, &ctx()).await.expect("snapshot");
    // Home is the same node: its earlier id names it; Inbox's node was replaced: its earlier id went
    assert_eq!(
        second.carried,
        Some(json!({"s1e1": "s2e1"}).as_object().cloned().unwrap())
    );
    assert_eq!(resolves("s1e1").expect("carried"), "s2e1");
    assert_eq!(resolves("s2e2").expect("this snapshot's"), "s2e2");
    assert!(resolves("s1e2").is_err(), "another node: refused");
    assert!(
        !second.content.contains_key("carried") && !second.content.contains_key("ids"),
        "the model's content is as before: {:?}",
        second.content
    );
    // a third look on the same page: ids of both earlier looks still work
    let third = tools.run(snap, &args, &ctx()).await.expect("snapshot");
    let carried = third.carried.clone().unwrap();
    assert_eq!(carried["s1e1"], json!("s3e1"));
    assert_eq!(carried["s2e1"], json!("s3e1"));
    assert_eq!(carried["s2e3"], json!("s3e3"));
    assert_eq!(resolves("s1e1").expect("carried twice"), "s3e1");

    // the page loads again: the next snapshot carries nothing, and every earlier id is refused
    loads.send(()).unwrap();
    settle().await;
    let fourth = tools.run(snap, &args, &ctx()).await.expect("snapshot");
    assert_eq!(fourth.carried, Some(Map::new()));
    for id in ["s1e1", "s3e1", "s3e3"] {
        assert!(resolves(id).is_err(), "{id} after a load");
    }
    assert_eq!(resolves("s4e1").expect("the new look's"), "s4e1");
}
