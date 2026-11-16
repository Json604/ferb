//! HAND-564, HAND-565 (live 2026-11-16, the third fresh install in a row with a pairing failure): FERB's data folder was
//! wiped and the app reinstalled while Chrome Default's FERB extension kept the previous install's key. Its connection
//! (connection 1) presented 46,331 messages before the owner pressed Pair all; the pairing page was answered at
//! 02:54:04.261, a hello the extension had signed with its older key was refused 4 ms later (`bad_mac`, presented seq
//! 169,687), Pair all reported the profile failed 80 ms after that, and the new key's hello was accepted right after
//! (the store's counter reached 21,837). Here the real hub, an in-memory store and one in-memory connection replay
//! that order with invented installs and keys; the clock is injected.

mod common;

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ferb_browser::bridge::{Hooks, Hub, Ledger, STALE_ANSWER_EVERY_S};
use ferb_browser::pairing::{InstallRow, InstallStore, PairingKey, Pairings, StoreError};
use ferb_browser::protocol::{sign, MIN_EXTENSION_VERSION, PROTOCOL_VERSION};
use ferb_browser::rules::{BrowserApps, BrowserInfo};
use ferb_core::interfaces::{BoxFuture, Clock};
use ferb_core::types::Identity;
use ferb_core::when::DateTime;
use serde_json::{json, Map, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, DuplexStream, Lines};

use common::{b64d, Counter};

const INSTALL: &str = "install-invented";
/// The key the previous FERB install gave this install; this FERB never issued it.
const OLD_KEY: [u8; 32] = [9; 32];

type Row = (InstallRow, Vec<u8>, i64);

/// `ext_installs` in memory, counting the key reads (a refused flood must not cost one per message).
#[derive(Default)]
struct Installs {
    rows: Mutex<HashMap<String, Row>>,
    key_reads: AtomicUsize,
}

impl InstallStore for Installs {
    fn row(&self, id: &str) -> Result<Option<InstallRow>, StoreError> {
        Ok(self.rows.lock().unwrap().get(id).map(|r| r.0.clone()))
    }
    fn install_for(&self, browser: &str, folder: &str) -> Result<Option<String>, StoreError> {
        Ok(self
            .rows
            .lock()
            .unwrap()
            .iter()
            .find(|(_, r)| r.0.browser == browser && r.0.folder == folder)
            .map(|(k, _)| k.clone()))
    }
    fn installs(&self) -> Result<Vec<String>, StoreError> {
        Ok(self.rows.lock().unwrap().keys().cloned().collect())
    }
    fn key(&self, id: &str) -> Result<Option<PairingKey>, StoreError> {
        Ok(self
            .rows
            .lock()
            .unwrap()
            .get(id)
            .map(|r| PairingKey::new(r.1.clone())))
    }
    fn key_and_seq(&self, id: &str) -> Result<Option<(PairingKey, i64)>, StoreError> {
        self.key_reads.fetch_add(1, Ordering::SeqCst);
        Ok(self
            .rows
            .lock()
            .unwrap()
            .get(id)
            .map(|r| (PairingKey::new(r.1.clone()), r.2)))
    }
    fn version(&self, _: &str) -> Result<Option<String>, StoreError> {
        Ok(Some(MIN_EXTENSION_VERSION.into()))
    }
    fn set_version(&self, _: &str, _: &str) -> Result<(), StoreError> {
        Ok(())
    }
    fn set_last_seq(&self, id: &str, seq: i64) -> Result<(), StoreError> {
        if let Some(r) = self.rows.lock().unwrap().get_mut(id) {
            r.2 = seq;
        }
        Ok(())
    }
    fn unpair(&self, _: &str, _: &str) -> Result<bool, StoreError> {
        Ok(false)
    }
    fn pair(
        &self,
        id: &str,
        row: &InstallRow,
        key: &PairingKey,
        _: &str,
    ) -> Result<(), StoreError> {
        let mut rows = self.rows.lock().unwrap();
        rows.retain(|k, r| k != id && !(r.0.browser == row.browser && r.0.folder == row.folder));
        rows.insert(id.into(), (row.clone(), key.expose().to_vec(), 0));
        Ok(())
    }
}

#[derive(Default)]
struct Clocked(Mutex<f64>);

impl Clock for Clocked {
    fn now(&self) -> DateTime {
        ferb_core::when::parse("2026-01-01T09:00:00+00:00").unwrap()
    }
    fn monotonic(&self) -> f64 {
        *self.0.lock().unwrap()
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

/// A signed hello (or an unsigned one with no key), as background.js's `post` writes it.
fn hello(key: Option<&[u8]>, seq: i64, nonce: &str) -> String {
    let body = json!({"version": MIN_EXTENSION_VERSION, "protocol": PROTOCOL_VERSION, "browser": "chrome",
                      "paired": key.is_some(), "nonce": nonce, "tabs": []});
    let body_json = ferb_py::json::dumps(&body);
    let line = match key {
        Some(key) => {
            let mac = sign(key, INSTALL, seq, "hello", &body_json);
            json!({"install_id": INSTALL, "type": "hello", "body_json": body_json, "seq": seq, "mac": mac})
        }
        None => json!({"install_id": INSTALL, "type": "hello", "body_json": body_json}),
    };
    format!("{line}\n")
}

fn tab_event(key: &[u8], seq: i64) -> String {
    let body_json = ferb_py::json::dumps(&json!({"event": "created", "tab_id": seq}));
    let mac = sign(key, INSTALL, seq, "tab_event", &body_json);
    format!(
        "{}\n",
        json!({"install_id": INSTALL, "type": "tab_event", "body_json": body_json, "seq": seq, "mac": mac})
    )
}

struct World {
    hub: Arc<Hub>,
    pairings: Arc<Pairings>,
    store: Arc<Installs>,
    clock: Arc<Clocked>,
    refusals: Arc<Mutex<Vec<Map<String, Value>>>>,
}

fn world() -> World {
    let store = Arc::new(Installs::default());
    let clock = Arc::new(Clocked::default());
    let random = Arc::new(Counter::new());
    let pairings = Arc::new(Pairings::new(store.clone(), clock.clone(), random.clone()));
    let hub = Hub::new(
        pairings.clone(),
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
    World {
        hub,
        pairings,
        store,
        clock,
        refusals,
    }
}

/// One extension connection: what it writes to the hub, and the hub's lines back.
fn connect(w: &World) -> (DuplexStream, Lines<BufReader<DuplexStream>>) {
    let (ext_w, hub_r) = tokio::io::duplex(1 << 22);
    let (hub_w, ext_r) = tokio::io::duplex(1 << 22);
    tokio::spawn(Arc::clone(&w.hub).serve_from(
        BufReader::new(hub_r),
        hub_w,
        Some(33450),
        "chrome-extension://storecopy/".into(),
    ));
    (ext_w, BufReader::new(ext_r).lines())
}

async fn settle() {
    tokio::time::sleep(Duration::from_millis(50)).await;
}

/// Every line the hub wrote that is waiting now.
async fn drain(lines: &mut Lines<BufReader<DuplexStream>>) -> Vec<Value> {
    let mut out = vec![];
    while let Ok(Ok(Some(l))) =
        tokio::time::timeout(Duration::from_millis(20), lines.next_line()).await
    {
        out.push(serde_json::from_str(&l).unwrap());
    }
    out
}

// decision: HAND-564, HAND-565
#[tokio::test(start_paused = true)]
async fn a_fresh_pairing_converges_over_a_connection_that_kept_presenting_an_older_key() {
    let w = world();
    let (mut ext, mut from_hub) = connect(&w);
    // before Pair all: this FERB has no record of the install, so every hello is unpaired
    ext.write_all(hello(Some(&OLD_KEY), 169_000, "n0").as_bytes())
        .await
        .unwrap();
    settle().await;
    assert_eq!(
        drain(&mut from_hub).await,
        vec![json!({"type": "pairing_required"})]
    );

    // Pair all: the page hands over its token on this same connection
    let token = w.pairings.issue_token("chrome", "Default", Identity::Owner);
    let pair = json!({"install_id": INSTALL, "type": "pair",
                      "body_json": ferb_py::json::dumps(&json!({"token": token}))});
    ext.write_all(format!("{pair}\n").as_bytes()).await.unwrap();
    settle().await;
    let back = drain(&mut from_hub).await;
    assert_eq!(back.len(), 1, "{back:?}");
    assert_eq!(back[0]["type"], "paired");
    let new_key = b64d(back[0]["key"].as_str().unwrap());
    let binding = w
        .pairings
        .for_profile("chrome", "Default")
        .unwrap()
        .unwrap();

    // what the extension had queued before it took the new key: older-key hellos and messages, a flood of them
    let reads_before = w.store.key_reads.load(Ordering::SeqCst);
    for i in 0..2_000 {
        ext.write_all(hello(Some(&OLD_KEY), 169_687 + 2 * i, &format!("n{i}")).as_bytes())
            .await
            .unwrap();
        ext.write_all(tab_event(&OLD_KEY, 169_688 + 2 * i).as_bytes())
            .await
            .unwrap();
    }
    settle().await;
    let back = drain(&mut from_hub).await;
    // answered once, naming the hello it refuses
    assert_eq!(back.len(), 1, "{back:?}");
    assert_eq!(back[0]["type"], "refused");
    assert_eq!(
        back[0]["stale_key"],
        json!({"nonce": "n0", "seq": 169_687, "reason": "bad_mac"})
    );
    // journaled once, with HAND-530's sender facts
    {
        let rows = w.refusals.lock().unwrap();
        assert_eq!(rows.len(), 1, "{rows:?}");
        assert_eq!(rows[0]["reason"], "bad_mac");
        assert_eq!(rows[0]["profile"], "chrome/Default");
        assert_eq!(rows[0]["presented_seq"], 169_687);
    }
    // the older-key messages besides hellos were dropped unread: a key read per hello, and one for the first
    // refusal's sender facts (HAND-530's accepted counter)
    assert_eq!(
        w.store.key_reads.load(Ordering::SeqCst) - reads_before,
        2_001
    );
    // a stale key's refusal is not this pairing's failure: nothing blocks the profile, it needs pairing again only
    // while nothing authenticates
    assert_eq!(w.hub.blocking(&binding), None);
    assert!(w.hub.stale_key(&binding));

    // past the answer interval, one more older-key hello is answered again (once)
    *w.clock.0.lock().unwrap() += STALE_ANSWER_EVERY_S;
    ext.write_all(hello(Some(&OLD_KEY), 200_000, "late").as_bytes())
        .await
        .unwrap();
    ext.write_all(hello(Some(&OLD_KEY), 200_002, "later").as_bytes())
        .await
        .unwrap();
    settle().await;
    let back = drain(&mut from_hub).await;
    assert_eq!(back.len(), 1, "{back:?}");
    assert_eq!(back[0]["stale_key"]["nonce"], "late");

    // the extension takes the new key and says hello with it on the same connection: accepted
    ext.write_all(hello(Some(&new_key), 1, "fresh").as_bytes())
        .await
        .unwrap();
    settle().await;
    let back = drain(&mut from_hub).await;
    assert_eq!(back.len(), 1, "{back:?}");
    assert_eq!(back[0]["type"], "welcome");
    assert!(w.hub.existing("chrome", "Default").unwrap().is_connected());
    assert_eq!(w.hub.problem(&binding), None);
    assert!(!w.hub.stale_key(&binding));
    // and the repeats are one row with their count
    let rows = w.refusals.lock().unwrap().clone();
    assert_eq!(rows.len(), 2, "{rows:?}");
    assert_eq!(rows[1]["reason"], "bad_mac");
    assert_eq!(rows[1]["repeats_on_connection"], 2_001);
    assert_eq!(rows[1]["dropped_on_connection"], 2_000);
}

/// HAND-565: a copy that dropped its key (extension 0.19.12 after a stale-key refusal) says hello unsigned; FERB still
/// has the install's record, so that is refused as before (journaled, nothing changes), and the copy is told it isn't
/// paired rather than refused, so it waits for the owner's Pair.
// decision: HAND-565
#[tokio::test(start_paused = true)]
async fn an_unsigned_hello_from_a_known_install_is_told_it_isnt_paired() {
    let w = world();
    w.store.rows.lock().unwrap().insert(
        INSTALL.into(),
        (
            InstallRow {
                browser: "chrome".into(),
                folder: "Default".into(),
                identity: Identity::Owner,
            },
            vec![7; 32],
            5,
        ),
    );
    let (mut ext, mut from_hub) = connect(&w);
    ext.write_all(hello(None, 0, "n1").as_bytes())
        .await
        .unwrap();
    settle().await;
    assert_eq!(
        drain(&mut from_hub).await,
        vec![json!({"type": "pairing_required"})]
    );
    let rows = w.refusals.lock().unwrap().clone();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["reason"], "bad_mac");
    let binding = w
        .pairings
        .for_profile("chrome", "Default")
        .unwrap()
        .unwrap();
    assert!(w.hub.stale_key(&binding));
    assert_eq!(
        w.store.rows.lock().unwrap()[INSTALL].2,
        5,
        "nothing changed"
    );
}
