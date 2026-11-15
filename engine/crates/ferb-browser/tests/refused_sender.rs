//! HAND-530: a hello refused for its MAC names its sender (live 2026-11-14: `bad_mac` for chrome/Profile 15 repeated
//! 1,119 times in ten minutes while a connection FERB accepted for the same profile worked, and nothing said which
//! copy of the extension, which browser process or which key counter the refused one was). A profile's extension
//! connects and is accepted; a second connection, from another origin and browser process, presents the same install
//! with a key FERB no longer holds. Invented installs and keys; in-memory connections.

mod common;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use ferb_browser::bridge::{Hooks, Hub, Ledger};
use ferb_browser::pairing::{InstallRow, InstallStore, PairingKey, Pairings, StoreError};
use ferb_browser::protocol::{sign, MIN_EXTENSION_VERSION, PROTOCOL_VERSION};
use ferb_browser::rules::{BrowserApps, BrowserInfo};
use ferb_core::interfaces::{BoxFuture, Clock};
use ferb_core::types::Identity;
use ferb_core::when::DateTime;
use serde_json::{json, Map, Value};
use tokio::io::{AsyncWriteExt, BufReader};

use common::Counter;

const INSTALL: &str = "install-invented";
const KEY: [u8; 32] = [7; 32];
/// The key an earlier pairing gave the same install (its store was wiped; the extension kept it).
const OLD_KEY: [u8; 32] = [9; 32];

struct Installs(Mutex<i64>);

impl InstallStore for Installs {
    fn row(&self, install_id: &str) -> Result<Option<InstallRow>, StoreError> {
        Ok((install_id == INSTALL).then(|| InstallRow {
            browser: "chrome".into(),
            folder: "Profile 9".into(),
            identity: Identity::Owner,
        }))
    }
    fn install_for(&self, browser: &str, folder: &str) -> Result<Option<String>, StoreError> {
        Ok((browser == "chrome" && folder == "Profile 9").then(|| INSTALL.to_string()))
    }
    fn installs(&self) -> Result<Vec<String>, StoreError> {
        Ok(vec![INSTALL.into()])
    }
    fn key(&self, install_id: &str) -> Result<Option<PairingKey>, StoreError> {
        Ok((install_id == INSTALL).then(|| PairingKey::new(KEY.to_vec())))
    }
    fn key_and_seq(&self, install_id: &str) -> Result<Option<(PairingKey, i64)>, StoreError> {
        let seq = *self.0.lock().unwrap();
        Ok((install_id == INSTALL).then(|| (PairingKey::new(KEY.to_vec()), seq)))
    }
    fn version(&self, _: &str) -> Result<Option<String>, StoreError> {
        Ok(Some(MIN_EXTENSION_VERSION.into()))
    }
    fn set_version(&self, _: &str, _: &str) -> Result<(), StoreError> {
        Ok(())
    }
    fn set_last_seq(&self, _: &str, seq: i64) -> Result<(), StoreError> {
        *self.0.lock().unwrap() = seq;
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

fn hello(key: &[u8], seq: i64, nonce: &str) -> String {
    let body = json!({"version": MIN_EXTENSION_VERSION, "protocol": PROTOCOL_VERSION, "browser": "chrome",
                      "paired": true, "nonce": nonce, "tabs": []});
    let body_json = ferb_py::json::dumps(&body);
    let mac = sign(key, INSTALL, seq, "hello", &body_json);
    format!(
        "{}\n",
        json!({"install_id": INSTALL, "type": "hello", "body_json": body_json, "seq": seq, "mac": mac})
    )
}

// decision: HAND-530
#[tokio::test(start_paused = true)]
async fn a_hello_refused_for_its_mac_names_its_sender_beside_the_accepted_connection() {
    let random = Arc::new(Counter::new());
    let pairings = Arc::new(Pairings::new(
        Arc::new(Installs(Mutex::new(0))),
        Arc::new(Clocked),
        random.clone(),
    ));
    let hub = Hub::new(
        pairings,
        Arc::new(NoLedger),
        Arc::new(|_: &str, _: Map<String, Value>| {}),
        Arc::new(|| "dev-1".to_string()),
        random,
        Arc::new(Apps),
        Arc::new(|_| Some("com.google.Chrome".to_string())),
    );
    let refusals: Arc<Mutex<Vec<Map<String, Value>>>> = Arc::default();
    let r = refusals.clone();
    hub.set_hooks(Hooks {
        record_refusal: Arc::new(move |facts| r.lock().unwrap().push(facts)),
        ..Hooks::default()
    });
    // the copy FERB paired: accepted
    let (mut paired_w, hub_r) = tokio::io::duplex(1 << 16);
    let (hub_w, _paired_r) = tokio::io::duplex(1 << 16);
    tokio::spawn(Arc::clone(&hub).serve_from(
        BufReader::new(hub_r),
        hub_w,
        Some(501),
        "chrome-extension://storecopy/".into(),
    ));
    paired_w
        .write_all(hello(&KEY, 1, "n1").as_bytes())
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(
        refusals.lock().unwrap().is_empty(),
        "the paired copy is accepted"
    );
    // a second connection with the install's old key
    let (mut stale_w, hub_r2) = tokio::io::duplex(1 << 16);
    let (hub_w2, _stale_r) = tokio::io::duplex(1 << 16);
    tokio::spawn(Arc::clone(&hub).serve_from(
        BufReader::new(hub_r2),
        hub_w2,
        Some(777),
        "chrome-extension://devcopy/".into(),
    ));
    stale_w
        .write_all(hello(&OLD_KEY, 250_591, "n2").as_bytes())
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
    let got = refusals.lock().unwrap().clone();
    assert_eq!(got.len(), 1, "{got:?}");
    let f = &got[0];
    assert_eq!(f["reason"], "bad_mac");
    assert_eq!(f["profile"], "chrome/Profile 9");
    assert_eq!(f["extension"], "chrome-extension://devcopy/");
    assert_eq!(f["browser_pid"], 777);
    assert_eq!(f["messages_on_connection"], 1);
    assert_eq!(f["presented_seq"], 250_591);
    assert_eq!(f["accepted_seq"], 1);
    assert_eq!(f["paired_connection"], "another");
    assert_eq!(f["paired_connection_browser_pid"], 501);
    assert_eq!(
        f["paired_connection_extension"],
        "chrome-extension://storecopy/"
    );
    assert!(
        !f.contains_key("mac") && !f.contains_key("key"),
        "no MAC or key in the facts: {f:?}"
    );
}
