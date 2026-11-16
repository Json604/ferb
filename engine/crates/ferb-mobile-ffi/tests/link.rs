#![cfg(target_os = "macos")]
//! The real link behind the exported objects, on 127.0.0.1: ferb-link's own test world (a local relay standing in for
//! the public ones, the stand-in daemon, the Mac's bridge; `ferb-link/tests/common`, shared by path), and the phone
//! side as an app drives it: `Link.start` (its state handed to the app), pairing by the typed code with the six digits
//! on both screens, the device key's proof through the app's `DeviceSigner`, `Mobile.connect`, a locked phone's
//! refusal, `Mobile.unlock` through the app's `Signer`, a message sent through the link's door, Forget, close.

#[allow(clippy::duplicate_mod)]
// another crate's files: never formatted from here (its own `cargo fmt` owns them)
#[rustfmt::skip]
#[path = "../../ferb-link/tests/common/mod.rs"]
mod world;

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use base64::Engine;
use ferb_link::net::Shared;
use ferb_mobile_ffi::records::{Change, LinkPath};
use ferb_mobile_ffi::{
    Clock, DeviceSigner, FerbError, Ids, Link, LinkConfig, LinkListener, LinkPhase, LinkStatus,
    Listener, Mobile, MobileConfig, Signer,
};
use p256::ecdsa::{SigningKey, signature::Signer as _};
use p256::pkcs8::EncodePublicKey;
use serde_json::{Value, json};

fn b64(b: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(b)
}

fn public(k: &SigningKey) -> String {
    b64(k.verifying_key().to_public_key_der().unwrap().as_bytes())
}

/// A software P-256 key standing in for the phone's Keystore keys.
struct Key(SigningKey, Mutex<Vec<String>>);
impl Key {
    fn new() -> Arc<Key> {
        Arc::new(Key(
            SigningKey::random(&mut p256::elliptic_curve::rand_core::OsRng),
            Mutex::new(Vec::new()),
        ))
    }
    fn sign_text(&self, payload: &str) -> String {
        self.1.lock().unwrap().push(payload.to_string());
        let sig: p256::ecdsa::Signature = self.0.sign(payload.as_bytes());
        b64(sig.to_der().as_bytes())
    }
}
impl DeviceSigner for Key {
    fn sign(&self, payload: String) -> Option<String> {
        Some(self.sign_text(&payload))
    }
}
impl Signer for Key {
    fn sign(&self, payload: String) -> Option<String> {
        Some(self.sign_text(&payload))
    }
}

#[derive(Default)]
struct App {
    statuses: Mutex<Vec<LinkStatus>>,
    states: Mutex<Vec<Vec<u8>>>,
    changes: Mutex<Vec<Change>>,
}
impl LinkListener for App {
    fn status_changed(&self, status: LinkStatus) {
        self.statuses.lock().unwrap().push(status);
    }
    fn persist(&self, state: Vec<u8>) {
        self.states.lock().unwrap().push(state);
    }
}
impl Listener for App {
    fn changed(&self, change: Change) {
        self.changes.lock().unwrap().push(change);
    }
}
impl Clock for App {
    fn now_iso(&self) -> String {
        "2026-01-02T10:00:00+00:00".into()
    }
}
impl Ids for App {
    fn new_id(&self) -> String {
        format!("sub-{}", self.changes.lock().unwrap().len())
    }
}

fn wait(secs: u64, mut f: impl FnMut() -> bool) -> bool {
    let end = Instant::now() + Duration::from_secs(secs);
    while Instant::now() < end {
        if f() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    f()
}

// decision: PHN-446
// decision: PHN-442
// decision: PHN-408
// decision: PHN-125
// decision: PHN-493
// decision: PHN-494
#[test]
fn an_app_pairs_unlocks_and_sends_through_the_real_link() {
    // the Mac's side runs on its own runtime; the app's calls block on the test thread, as an app's do on its own
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    let relay = rt.block_on(world::Relay::start(None, None));
    let w = rt.block_on(world::World::start(
        Shared::Custom(vec![relay.spec()]),
        None,
        false,
    ));

    let (device, strong, unlock) = (Key::new(), Key::new(), Key::new());
    let app = Arc::new(App::default());
    let config = LinkConfig {
        state: None,
        name: "Invented phone".into(),
        app: "test".into(),
        unlock_pub: Some(public(&unlock.0)),
        device_pub: Some(public(&device.0)),
        strong_pub: Some(public(&strong.0)),
        segmentation_offload: true,
        dev_shared_relays: vec![relay.url()],
        dev_force_relay: false,
        dev_portmapper: false,
    };
    let link = Link::start(config.clone(), app.clone(), Some(device.clone())).unwrap();
    assert!(!link.endpoint_id().is_empty());
    // a first start hands the phone's state, with its new link key, to the app at once
    assert!(wait(5, || !app.states.lock().unwrap().is_empty()));
    let first: Value = serde_json::from_slice(&app.states.lock().unwrap()[0]).unwrap();
    assert!(first["key"].as_str().is_some_and(|k| !k.is_empty()));
    assert_eq!(link.status().phase, LinkPhase::Idle);
    assert!(!link.status().paired);

    // a code that is not one: refused at once, with the link's code
    let bad = link.pair("not a code".into()).unwrap_err();
    assert!(matches!(bad, FerbError::Refused { .. }), "{bad:?}");
    assert_eq!(bad.link_code(), Some("bad_request"));

    // pairing by the typed code: both screens show the same six digits, then the owner's Yes
    let mut admin = w.bridge.admin_events();
    let qr = w.bridge.pair_start().unwrap();
    link.pair(qr["code"].as_str().unwrap().to_string()).unwrap();
    let asked = rt.block_on(async {
        tokio::time::timeout(Duration::from_secs(30), async {
            loop {
                if admin.recv().await.unwrap()["event"] == "pair_request" {
                    return;
                }
            }
        })
        .await
    });
    assert!(asked.is_ok(), "the Mac never asked: {:?}", link.status());
    let mac_code = w.bridge.status()["pairing"]["code"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(
        wait(5, || link.status().pairing_code.as_deref()
            == Some(mac_code.as_str())),
        "codes differ: Mac {mac_code}, phone {:?}",
        link.status()
    );
    assert_eq!(link.status().phase, LinkPhase::PairingWaitForMac);
    w.decide(true).unwrap();
    assert!(
        wait(20, || link.status().phase == LinkPhase::Online),
        "not online: {:?}",
        link.status()
    );
    let st = link.status();
    assert!(st.paired);
    assert_ne!(st.path, LinkPath::Offline);
    assert!(
        app.statuses
            .lock()
            .unwrap()
            .iter()
            .any(|s| s.phase == LinkPhase::Online)
    );
    // the paired state reached the app, and the device key proved this phone's presence through the app's signer
    assert!(wait(5, || app.states.lock().unwrap().iter().any(|s| {
        serde_json::from_slice::<Value>(s).is_ok_and(|v| v["mac"]["paired"] == json!(true))
    })));
    assert!(
        wait(10, || !device.1.lock().unwrap().is_empty()),
        "the link never asked the device key"
    );

    // the core on the link: its state at once; a locked phone's private read is refused as `needs_unlock`
    let dir = tempfile::tempdir().unwrap();
    let m = Mobile::open(
        dir.path().join("phone.db").to_string_lossy().into_owned(),
        vec![3u8; 32],
        MobileConfig {
            client_id: "phone-1".into(),
            utc_offset_s: 0,
            page_limit: None,
            max_bytes: None,
            piece_chars: None,
        },
        app.clone(),
        app.clone(),
        app.clone(),
    )
    .unwrap();
    m.connect(link.clone()).unwrap();
    assert_ne!(m.link_state().unwrap().path, LinkPath::Offline);
    let locked = m.sync().unwrap_err();
    assert!(
        matches!(locked, FerbError::NeedsUnlock { .. }),
        "{locked:?}"
    );
    // Stop works with the phone locked (PHN-125)
    m.stop(vec!["t-none".into()]).unwrap();

    // unlocked through the app's signer; then a message goes through the link's door and is acknowledged
    m.unlock(strong.clone()).unwrap();
    assert!(strong.1.lock().unwrap()[0].contains("link.unlock"));
    let sent = m.send("Hello from the invented phone".into()).unwrap();
    assert!(
        wait(15, || m.outbox().unwrap().is_empty()),
        "the message never left the outbox: {sent:?} {:?}",
        m.outbox()
    );
    let messages = rt
        .block_on(w.standin.cli("standin.state", json!({})))
        .to_string();
    assert!(
        messages.contains("Hello from the invented phone"),
        "the Mac's stand-in never got it: {messages}"
    );

    // Forget on the phone: idle, unpaired, the new state handed over; then close: every call `closed`
    link.forget().unwrap();
    assert!(wait(5, || link.status().phase == LinkPhase::Idle));
    assert!(wait(5, || !link.status().paired));
    link.shutdown();
    link.shutdown();
    assert!(link.is_closed());
    assert!(matches!(link.retry(), Err(FerbError::Closed { .. })));
    assert!(matches!(
        link.pair(qr["code"].as_str().unwrap().into()),
        Err(FerbError::Closed { .. })
    ));
    assert!(
        wait(5, || m.link_state().unwrap().path == LinkPath::Offline),
        "the core heard the link close"
    );
    assert!(matches!(m.sync(), Err(FerbError::Offline { .. })));
    m.shutdown();

    // the state the app kept starts the same phone again (its link id), and a state that is not the link's is refused
    let kept = app.states.lock().unwrap().last().cloned().unwrap();
    let again = Link::start(
        LinkConfig {
            state: Some(kept),
            ..config.clone()
        },
        app.clone(),
        None,
    )
    .unwrap();
    assert_eq!(again.endpoint_id(), link.endpoint_id());
    again.shutdown();
    let refused = Link::start(
        LinkConfig {
            state: Some(b"{\"not\": \"a state\"}".to_vec()),
            ..config
        },
        app.clone(),
        None,
    );
    assert!(matches!(refused, Err(FerbError::BadArgument { .. })));
    drop(w);
    drop(relay);
}
