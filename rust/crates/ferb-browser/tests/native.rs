//! The native host's connection reaches the hub through ferb-rpc's Unix-socket server
//! (`bridge::native::Channel`, the `NativeChannel` adapter): the host's RPC hello is the server's,
//! what the host sent right behind it (already read off the socket) and every later line are the
//! hub's, and the browser's pid read from the socket names the app that started the host
//! (decisions: HAND-223, HAND-406, HAND-438). A real socket in a temporary folder; no browser.
#![cfg(target_os = "macos")]

mod common;

use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ferb_browser::bridge::{native::Channel, Hooks, Hub, Ledger};
use ferb_browser::pairing::{sqlite::SqliteInstalls, Pairings};
use ferb_browser::protocol::FERB_EXTENSION_ID;
use ferb_browser::rules::{BrowserApps, BrowserInfo};
use ferb_core::interfaces::{BoxFuture, Clock};
use ferb_core::when::DateTime;
use ferb_rpc::{RunTokens, Server};
use serde_json::{json, Map, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

use common::Counter;

struct Clocked;

impl Clock for Clocked {
    fn now(&self) -> DateTime {
        ferb_core::when::parse("2026-01-01T09:00:00+00:00").unwrap()
    }
    fn monotonic(&self) -> f64 {
        1000.0
    }
    fn sleep_until(&self, _: DateTime) -> BoxFuture<'_, ()> {
        Box::pin(async {})
    }
}

impl ferb_rpc::Clock for Clocked {
    fn monotonic(&self) -> f64 {
        1000.0
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
    fn known(&self, bundle_id: &str) -> bool {
        bundle_id.starts_with("com.google.Chrome")
    }
    fn browser(&self, key: &str) -> Option<BrowserInfo> {
        (key == "chrome").then(|| BrowserInfo {
            bundle_id: "com.google.Chrome".into(),
            label: "Google Chrome".into(),
        })
    }
}

// decisions: HAND-223, HAND-406, HAND-438
#[tokio::test]
async fn a_native_hosts_connection_reaches_the_hub_after_its_hello() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("ferb.db");
    let db = ferb_store::Db::open(&db_path).unwrap();
    db.migrate().unwrap();
    db.close().unwrap();
    let store = Arc::new(SqliteInstalls::new(
        rusqlite::Connection::open(&db_path).unwrap(),
    ));
    let random = Arc::new(Counter::new());
    let pairings = Arc::new(Pairings::new(store, Arc::new(Clocked), random.clone()));
    let asked: Arc<Mutex<Vec<Option<i32>>>> = Arc::default();
    let a = asked.clone();
    let hub = Hub::new(
        pairings,
        Arc::new(NoLedger),
        Arc::new(|_: &str, _: Map<String, Value>| {}),
        Arc::new(|| "dev-1".to_string()),
        random,
        Arc::new(Apps),
        Arc::new(move |pid| {
            a.lock().unwrap().push(pid);
            Some("com.google.Chrome".to_string())
        }),
    );
    let refusals: Arc<Mutex<Vec<Map<String, Value>>>> = Arc::default();
    let (r, unpaired) = (refusals.clone(), Arc::new(Mutex::new(0)));
    let u = unpaired.clone();
    hub.set_hooks(Hooks {
        record_refusal: Arc::new(move |facts| r.lock().unwrap().push(facts)),
        on_unpaired: Arc::new(move || *u.lock().unwrap() += 1),
        ..Hooks::default()
    });
    let path = dir.path().join("run").join("s.sock");
    let server = Server::new(&path, Arc::new(RunTokens::new(Arc::new(Clocked))));
    server.set_native(Channel {
        hub: hub.clone(),
        browser_pid: Arc::new(|_| Some(4242)),
    });
    server.set_extension_ids(|| HashSet::from([FERB_EXTENSION_ID.to_string()]));
    server.start().await.unwrap();

    let (reader, mut writer) = UnixStream::connect(&path).await.unwrap().into_split();
    let mut reader = BufReader::new(reader);
    let params: Map<String, Value> = json!({"role": "native_host", "origin": format!("chrome-extension://{FERB_EXTENSION_ID}/")})
        .as_object()
        .unwrap()
        .clone();
    // the RPC hello and, right behind it in the same write, the extension's unpaired hello
    let ext_hello = json!({"install_id": "0f8fad5b-d9cb-469f-a165-70867728950e", "seq": 1, "type": "hello",
                           "body_json": ferb_py::json::dumps(&json!({"version": "0.19.10", "protocol": 21,
                                                                      "browser": "chrome"})), "mac": ""});
    let mut out = ferb_rpc::wire::request_line(1, "auth.hello", &params);
    out.extend_from_slice(format!("{}\nnot json\n", ferb_py::json::dumps(&ext_hello)).as_bytes());
    writer.write_all(&out).await.unwrap();
    let mut line = String::new();
    tokio::time::timeout(Duration::from_secs(10), reader.read_line(&mut line))
        .await
        .unwrap()
        .unwrap();
    let reply: Value = serde_json::from_str(&line).unwrap();
    assert!(reply.get("result").is_some(), "{reply}");
    line.clear();
    tokio::time::timeout(Duration::from_secs(10), reader.read_line(&mut line))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(line, "{\"type\": \"pairing_required\"}\n");
    // the malformed line is journaled, never acted on (HAND-406)
    for _ in 0..100 {
        if !refusals.lock().unwrap().is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let refused = refusals.lock().unwrap().clone();
    assert_eq!(refused.len(), 1);
    assert_eq!(refused[0]["reason"], "malformed");
    assert_eq!(refused[0]["app"], "com.google.Chrome");
    assert_eq!(*asked.lock().unwrap(), vec![Some(4242)]); // the pid the socket gave
    assert_eq!(*unpaired.lock().unwrap(), 1);
    assert_eq!(
        hub.unpaired(),
        vec![(
            "0f8fad5b-d9cb-469f-a165-70867728950e".to_string(),
            "chrome".to_string()
        )]
    );
    drop(writer);
    for _ in 0..100 {
        if hub.unpaired().is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(
        hub.unpaired().is_empty(),
        "the connection's end forgets the unpaired copy"
    );
    server.close().await;
}
