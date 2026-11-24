//! The bridge against the daemon's REAL phone area (ferb-daemon `phone`, slice S3) on a real ferb-rpc server, in this
//! process: pairing through `link.pair_request`/`link.pair_wait` and the owner's Yes in FERB.app (`phone.pair_decide`),
//! the device list from `link.devices`, a removal on `link.events` cutting the phone off, the daemon's gate deciding
//! every call by its class (`ferb_rpc::classes`), and FERB.app's control of the link through the daemon (S4: the asks
//! and `link.answer`). Every other method of the table is a stub that answers without acting (as ferb-daemon's own
//! tests/phone.rs does): FERB's chat itself is ferbd's, and the lab suites keep the stand-in for it and for S3b's files.
//! 127.0.0.1 only: a local relay; the peer check is ferb-rpc's own tests' (here every peer is the helper).

mod common;

use chrono::{FixedOffset, TimeZone};
use common::*;
use ferb_core::interfaces::{BoxFuture, Clock};
use ferb_daemon::phone::account::{AccountCheck, MacAccount};
use ferb_daemon::phone::link::LinkControl;
use ferb_daemon::phone::{Limits, Phones, QuestionFacts, Questions, Seams};
use ferb_link::{
    bridge::{Bridge, BridgeConfig},
    errors,
    net::Shared,
    secrets::Keys,
};
use ferb_rpc::server::{EventStream, HandlerResult};
use ferb_rpc::{Params, Role, RunTokens, Server, Session};
use serde_json::{Map, Value, json};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// The daemon's clock, still (the unlock window never runs out under a test).
struct Still;
impl Clock for Still {
    fn now(&self) -> ferb_core::when::DateTime {
        FixedOffset::east_opt(0)
            .unwrap()
            .with_ymd_and_hms(2026, 11, 16, 10, 0, 0)
            .unwrap()
    }
    fn monotonic(&self) -> f64 {
        1000.0
    }
    fn sleep_until(&self, _t: ferb_core::when::DateTime) -> BoxFuture<'_, ()> {
        Box::pin(async {})
    }
}

/// The site's test vector for the pairing assertion (PHN-828; site/tests/fixtures/assertion-vector.json): an invented key
/// (the SHA-256 of its label), its kid, the account it names. Never a FERB key.
fn site_vector() -> Value {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../site/tests/fixtures/assertion-vector.json"
    );
    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
}

/// The Mac's account: the vector's.
fn mac_account() -> MacAccount {
    let v = site_vector();
    MacAccount {
        id: v["payload"]["sub"].as_str().unwrap().into(),
        email: v["payload"]["email"].as_str().unwrap().into(),
    }
}

/// The site's assertion for this phone's endpoint, of `sub`/`email`, at the daemon's still clock.
fn assertion(endpoint: &str, sub: &str, email: &str) -> String {
    use base64::Engine;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use p256::ecdsa::signature::Signer;
    use sha2::{Digest, Sha256};
    let v = site_vector();
    let key = p256::ecdsa::SigningKey::from_slice(&Sha256::digest(
        v["test_key_label"].as_str().unwrap().as_bytes(),
    ))
    .unwrap();
    let now = Still.now().timestamp();
    let claims = json!({"v": 1, "kid": v["kid"], "aud": "ferb-pair", "sub": sub, "email": email, "dev": endpoint,
                        "iat": now, "exp": now + 300});
    let payload = URL_SAFE_NO_PAD.encode(claims.to_string());
    let sig: p256::ecdsa::Signature = key.sign(format!("ferb-pair/v1.{payload}").as_bytes());
    format!("fpa1.{payload}.{}", URL_SAFE_NO_PAD.encode(sig.to_bytes()))
}

struct NoQuestions;
impl Questions for NoQuestions {
    fn facts(&self, _id: &str) -> Option<QuestionFacts> {
        None
    }
    fn ask_check(&self, _reason: &str, _exact: &str, _task_id: &str) -> String {
        String::new()
    }
}

struct One(Option<HandlerResult>);
impl EventStream for One {
    fn next(&mut self) -> ferb_rpc::BoxFuture<'_, Option<HandlerResult>> {
        Box::pin(std::future::ready(self.0.take()))
    }
}

/// ferbd's control of the helper, as a test holds it: on, and the token it was told to keep.
#[derive(Default)]
struct Control {
    token: Mutex<Option<Vec<u8>>>,
}
impl LinkControl for Control {
    fn enabled(&self) -> bool {
        true
    }
    fn set_enabled(&self, _on: bool) -> Result<(), String> {
        Ok(())
    }
    fn keep_relay_token(&self, token: Option<&[u8]>) -> Result<(), String> {
        *self.token.lock().unwrap() = token.map(<[u8]>::to_vec);
        Ok(())
    }
}

struct Real {
    _dir: std::path::PathBuf,
    server: Server,
    phones: Phones,
    published: Arc<Mutex<Vec<(String, Value)>>>,
    control: Arc<Control>,
    bridge: Bridge,
    shared: Shared,
    _relay: Relay,
}

async fn real(devices_max: usize) -> Real {
    let relay = Relay::start(None, None).await;
    let dir = tmp("real");
    let db = ferb_store::Db::open(dir.join("ferb.db")).unwrap();
    db.migrate().unwrap();
    let published = Arc::new(Mutex::new(Vec::new()));
    let seams = Seams {
        db: Arc::new(Mutex::new(db)),
        clock: Arc::new(Still),
        nonce: Seams::system_nonce(),
        redact: Arc::new(|v: &Value| v.clone()),
        questions: Arc::new(NoQuestions),
        prompts: None,
        journal: Arc::new(|_k: &str, _v: Value| {}),
        // PHN-828: the site's test key, and the Mac signed in to the vector's account
        account: {
            let v = site_vector();
            AccountCheck::new(
                &[(
                    v["kid"].as_str().unwrap(),
                    v["test_public_key_sec1_uncompressed_b64"].as_str().unwrap(),
                )],
                Arc::new(|| Some(mac_account())),
            )
        },
        publish: {
            let p = published.clone();
            Arc::new(move |k: &str, v: Map<String, Value>| {
                p.lock().unwrap().push((k.to_owned(), Value::Object(v)))
            })
        },
    };
    let phones = Phones::new(
        seams,
        Limits {
            devices_max,
            ..Limits::default()
        },
    );
    let control = Arc::new(Control::default());
    phones.set_link(control.clone());
    let tokens = Arc::new(RunTokens::new(Arc::new(ferb_rpc::SystemClock::default())));
    let server = Server::new(dir.join("d.sock"), tokens);
    server.set_is_link(|_| true);
    // every other method: a stub that answers without acting
    for m in ferb_rpc::auth::METHODS {
        let name = m.name;
        if ferb_rpc::classes::phone_method(name).is_some_and(|r| r.stream) {
            server.register_stream(name, move |_s: Session, p: Params| {
                let mut ev = Map::new();
                ev.insert("method".into(), json!(name));
                ev.insert("params".into(), Value::Object(p));
                Box::new(One(Some(Ok(ev)))) as Box<dyn EventStream>
            });
        } else {
            server.register_unary(name, move |_s: Session, _p: Params| async move {
                let mut out = Map::new();
                out.insert("ok".into(), json!(true));
                out.insert("method".into(), json!(name));
                if name == "chat.status" {
                    out.insert("approvals".into(), json!([]));
                    out.insert("running".into(), json!([]));
                    out.insert("results".into(), json!([]));
                }
                Ok(out)
            });
        }
    }
    phones.register(&server);
    server.start().await.unwrap();
    server.set_ready();
    let shared = Shared::Custom(vec![relay.spec()]);
    let bridge = Bridge::start(BridgeConfig {
        portmapper: false,
        loopback_only: true,
        state_dir: dir.join("mac"),
        daemon_socket: dir.join("d.sock"),
        shared: shared.clone(),
        roots: None,
        force_relay: false,
        keys: Keys::Files,
        accept_state: false,
    })
    .await
    .expect("the bridge on the real daemon");
    assert!(ferb_link::net::wait_home(&bridge.endpoint(), Duration::from_secs(30)).await);
    Real {
        _dir: dir,
        server,
        phones,
        published,
        control,
        bridge,
        shared,
        _relay: relay,
    }
}

fn owner() -> Session {
    Session {
        role: Role::Cli,
        grant: None,
        pid: None,
        token: None,
        app: Some(true),
        phone: None,
    }
}

impl Real {
    /// The QR, the phone's proof, the daemon's question, the owner's Yes in FERB.app.
    async fn pair(&self, p: &TestPhone) {
        let invite = self.phones.code_start().await.expect("phone.pair_start");
        let mac = mac_account();
        p.phone
            .pair_with_account(
                invite["code"].as_str().unwrap(),
                Some(&assertion(&p.phone.id(), &mac.id, &mac.email)),
            )
            .unwrap();
        let asked = wait(30, || {
            self.published
                .lock()
                .unwrap()
                .iter()
                .any(|(k, v)| k == "phone.pair_requested" && v["phone"] == json!(p.phone.id()))
        })
        .await;
        assert!(
            asked,
            "the daemon never asked the owner; phone says {}",
            p.phone.phase()
        );
        // both screens show the same six digits while the owner is asked
        let code = self
            .published
            .lock()
            .unwrap()
            .iter()
            .rev()
            .find(|(k, _)| k == "phone.pair_requested")
            .map(|(_, v)| v["code"].as_str().unwrap_or_default().to_string())
            .unwrap();
        assert!(
            wait(5, || p.phone.phase()["code"].as_str()
                == Some(code.as_str()))
            .await,
            "the two screens differ: Mac {code}, phone {}",
            p.phone.phase()
        );
        let mut yes = Params::new();
        yes.insert("yes".into(), json!(true));
        let decided = self.phones.pair_decide(&yes).expect("the owner's Yes");
        assert_eq!(decided["paired"], true);
        assert!(p.wait_phase("online", 20).await, "{}", p.phone.phase());
    }
}

/// A phone pairs only through the daemon: the helper checks the QR's secret, the daemon asks the owner in FERB.app with
/// the six digits both screens show and what the phone will be able to do, and the owner's Yes there adds it to the
/// daemon's table, which the helper then follows.
// decision: PHN-490
// decision: PHN-102
// decision: PHN-468
// decision: PHN-469
#[tokio::test(flavor = "multi_thread")]
async fn a_phone_pairs_through_the_daemon_and_the_owner_s_yes_in_ferb_app() {
    let w = real(5).await;
    let p = TestPhone::start("Pixel", &w.shared, false, None, true).await;
    w.pair(&p).await;
    let (_, asked) = w
        .published
        .lock()
        .unwrap()
        .iter()
        .find(|(k, _)| k == "phone.pair_requested")
        .cloned()
        .unwrap();
    let code = asked["code"].as_str().unwrap().to_string();
    let question = asked["question"].as_str().unwrap();
    assert!(
        question.contains(&code) && question.contains("files") && question.contains("settings"),
        "{question}"
    );
    assert!(
        w.published
            .lock()
            .unwrap()
            .iter()
            .any(|(k, v)| k == "phone.pair_ended" && v["paired"] == true),
        "FERB.app was not told the pairing ended"
    );
    // the daemon's table is the one list
    let listed = w.phones.list(&owner()).unwrap();
    assert_eq!(listed["phones"][0]["id"], json!(p.phone.id()));
    assert_eq!(w.bridge.devices().len(), 1);
    assert_eq!(w.bridge.devices()[0].name, "Pixel");
}

/// Every call is the daemon's to admit, by its class: locked, a plain call is refused with the daemon's code; after
/// the phone's signed unlock it goes on; a sensitive call needs its own signed proof; a never method stops at the
/// bridge; what reached the daemon came only through the gate.
// decision: PHN-112
// decision: PHN-124
// decision: PHN-125
#[tokio::test(flavor = "multi_thread")]
async fn the_daemon_s_gate_decides_every_call() {
    let w = real(5).await;
    let p = TestPhone::start("phone", &w.shared, false, None, true).await;
    w.pair(&p).await;
    // the pairing's Yes opened the phone (PHN-647): it locks first, as the app does when it is put away
    p.call("link.lock", json!({})).await.expect("link.lock");
    assert_eq!(
        code(p.call("schedules.list", json!({})).await),
        errors::LOCKED
    );
    assert!(
        p.call("chat.status", json!({})).await.is_ok(),
        "a locked phone still learns that something waits"
    );
    p.unlock().await.expect("the signed unlock");
    let plain = p.call("schedules.list", json!({})).await.unwrap();
    assert_eq!(plain["method"], "schedules.list");
    assert_eq!(
        code(
            p.call("settings.set", json!({"key": "a", "value": 1}))
                .await
        ),
        errors::UNLOCK_REQUIRED
    );
    let signed = p
        .sensitive("settings.set", json!({"key": "a", "value": 1}))
        .await
        .unwrap();
    assert_eq!(signed["method"], "settings.set");
    // the phone area's own calls reach the daemon's gate by their rows (T-1214): FERB.app's are never a phone's
    assert_eq!(
        code(p.call("phone.pair_decide", json!({"yes": true})).await),
        errors::NOT_FROM_PHONE
    );
    assert_eq!(
        code(p.call("link.devices", json!({})).await),
        errors::NOT_FROM_PHONE
    );
    assert_eq!(
        code(p.call("phone.pair_start", json!({})).await),
        errors::NOT_FROM_PHONE
    );
    // the session's and the files' own calls only through the link's (link.unlock, link.files, ...)
    assert_eq!(
        code(p.call("phone.files", json!({"path": "~"})).await),
        errors::BAD_REQUEST
    );
    assert_eq!(
        code(p.call("phone.unlock", json!({})).await),
        errors::BAD_REQUEST
    );
    // the daemon audited what the phone did, by class
    let mut limit = Params::new();
    limit.insert("limit".into(), json!(100));
    let audit = w.phones.audit_rows(&limit).unwrap();
    let rows = audit["rows"].as_array().unwrap();
    assert!(
        rows.iter()
            .any(|r| r["method"] == "settings.set" && r["outcome"] == "ok"),
        "{audit:?}"
    );
}

/// The owner removes the phone in FERB.app: the daemon tells the helper on `link.events` and the phone's connection is
/// cut at once; it can't come back without pairing again.
// decision: PHN-490
// decision: PHN-106
// decision: PHN-469
#[tokio::test(flavor = "multi_thread")]
async fn a_phone_removed_in_ferb_app_is_cut_off_at_once() {
    let w = real(5).await;
    let p = TestPhone::start("phone", &w.shared, false, None, true).await;
    w.pair(&p).await;
    let mut id = Params::new();
    id.insert("id".into(), json!(p.phone.id()));
    w.phones.remove(&owner(), &id).expect("phone.remove");
    assert!(
        wait(10, || p.stop_code() == errors::DEVICE_REVOKED
            || p.stop_code() == errors::NOT_PAIRED)
        .await,
        "the phone was not cut off: {}",
        p.phone.phase()
    );
    assert!(w.bridge.devices().is_empty());
    assert!(
        w.published
            .lock()
            .unwrap()
            .iter()
            .any(|(k, _)| k == "phone.removed")
    );
}

/// PHN-646, PHN-647, PHN-649 through the real bridge and daemon: a pairing Yes opens the new phone (its first private
/// call needs no unlock once its connection proved the device key); the unlock statement the phone signed before the
/// link was up opens it once; "Forget this Mac" on the phone removes that phone only and cuts it off.
// decision: PHN-646
// decision: PHN-647
// decision: PHN-649
#[tokio::test(flavor = "multi_thread")]
async fn a_phone_opens_on_its_pairing_unlocks_by_statement_and_forgets_itself() {
    let w = real(5).await;
    let p = TestPhone::start("phone", &w.shared, false, None, true).await;
    w.pair(&p).await;
    let other = TestPhone::start("other", &w.shared, false, None, true).await;
    w.pair(&other).await;
    // the presence proof is the link's own, on its own thread: the first private call may race it
    let mut open = false;
    for _ in 0..50 {
        if p.call("schedules.list", json!({})).await.is_ok() {
            open = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }
    assert!(
        open,
        "the paired phone's first private call needed an unlock"
    );
    // the statement: made and signed by the phone itself, with its age beside it
    let payload =
        json!({"domain": "ferb.unlock-statement.v1", "nonce": "nonce-real-0123456789", "made": 1})
            .to_string();
    let proof = p.pin_proof_for(&payload);
    p.phone
        .call(
            "link.unlock_statement",
            json!({"age_ms": 2000}),
            Some(proof.clone()),
            |_| {},
        )
        .await
        .expect("the statement opens it");
    assert_eq!(
        code(
            p.phone
                .call(
                    "link.unlock_statement",
                    json!({"age_ms": 2000}),
                    Some(proof),
                    |_| {}
                )
                .await
        ),
        errors::UNLOCK_INVALID,
        "a statement opens once"
    );
    // Forget this Mac: this phone, and only it
    let id = p.phone.id();
    p.phone
        .call("link.forget", json!({}), None, |_| {})
        .await
        .expect("link.forget");
    assert!(
        wait(10, || w.bridge.devices().iter().all(|d| d.id != id)).await,
        "the phone that forgot itself is still listed"
    );
    assert_eq!(w.bridge.devices().len(), 1, "the other phone stays");
    assert!(
        w.published
            .lock()
            .unwrap()
            .iter()
            .any(|(k, v)| k == "phone.removed" && v["phone"] == json!(id))
    );
}

/// FERB.app's controls reach the helper only through the daemon (S4): the pairing code, the status with each phone's
/// connection, the relay's test in plain words, Always use my relay; the daemon's limit refuses a code before one is
/// drawn, with the code FERB.app and the link read as `device_limit`.
// decision: PHN-489
// decision: PHN-441
// decision: PHN-419
// decision: PHN-491 (the tier the helper names reaches the daemon; the refusal itself is unit-tested in ferb-daemon phone::files)
#[tokio::test(flavor = "multi_thread")]
async fn ferb_app_s_controls_reach_the_helper_through_the_daemon() {
    let w = real(1).await;
    assert!(
        w.phones.helper_connected(),
        "the helper does not listen on link.events"
    );
    let p = TestPhone::start("phone", &w.shared, false, None, true).await;
    w.pair(&p).await;
    let st = w.phones.link_status().await.unwrap();
    assert_eq!(
        (st["running"].clone(), st["enabled"].clone()),
        (json!(true), json!(true))
    );
    let dev = &st["devices"][0];
    assert_eq!(dev["id"], json!(p.phone.id()));
    assert!(
        matches!(
            dev["tier"].as_str(),
            Some("direct" | "shared_relay" | "own_relay")
        ),
        "{st:?}"
    );
    assert!(st["own_relay"].is_null());
    // full (1): no code is drawn
    let full = w.phones.code_start().await.unwrap_err();
    assert_eq!(
        full.failure,
        ferb_rpc::server::Failure::Coded(ferb_rpc::phone_gate::DEVICE_LIMIT)
    );
    // a relay that isn't one: refused in plain words, nothing kept
    let mut bad = Params::new();
    bad.insert("url".into(), json!("https://127.0.0.1:9"));
    let e = w.phones.relay_test(&bad).await.unwrap_err();
    assert!(
        !e.message.contains("own_relay_") && e.message.ends_with('.'),
        "{}",
        e.message
    );
    let e = w.phones.relay_set(&bad).await.unwrap_err();
    assert!(!e.message.is_empty());
    assert!(w.control.token.lock().unwrap().is_none());
    let mut on = Params::new();
    on.insert("on".into(), json!(true));
    assert_eq!(
        w.phones.relay_always(&on).await.unwrap()["own_relay_always"],
        true
    );
    assert_eq!(
        w.phones.link_status().await.unwrap()["own_relay_always"],
        true
    );
    assert_eq!(w.phones.code_cancel().await.unwrap()["ok"], true);
    drop(w.server);
}

/// T-1214, and a bridge bug found with it: a phone's `phone.list` (the Mac's name, this phone's pairing date),
/// `phone.remove` and its own device record go through the bridge to the daemon's gate (the bridge refused every
/// `phone.*` name before, so `mac_info` could never have worked against a real Mac); FERB's request for one precise fix
/// reaches that phone only (`location_wanted`), and its answer reaches the daemon.
// decision: PHN-525
// decision: GATE-614
// decision: PHN-523
#[tokio::test(flavor = "multi_thread")]
async fn the_phone_s_record_and_its_precise_fix_cross_the_real_bridge() {
    let w = real(5).await;
    let p = TestPhone::start("Moto", &w.shared, false, None, true).await;
    w.pair(&p).await;
    let other = TestPhone::start("Tab", &w.shared, false, None, true).await;
    w.pair(&other).await;
    p.unlock().await.expect("the signed unlock");
    let listed = p.call("phone.list", json!({})).await.expect("phone.list");
    assert!(
        listed["phones"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["this"] == true && r["name"] == "Moto"),
        "{listed}"
    );
    p.call(
        "phone.device_report",
        json!({"time_zone": "Asia/Kolkata", "location": "on",
               "place": {"city": "Pune", "region": "Maharashtra", "country": "India", "country_code": "IN"}}),
    )
    .await
    .expect("the phone's record");
    let views = w.phones.device_views();
    let me = views.iter().find(|v| v.name == "Moto").unwrap();
    assert!(me.connected);
    assert_eq!(
        me.record.as_ref().unwrap().place.as_ref().unwrap().city,
        "Pune"
    );
    let id = me.id.clone();
    let rx = w.phones.want_fix(&id, "fix-1").expect("asked");
    assert!(
        wait(10, || p.seen.lock().unwrap().events.iter().any(
            |e| e["type"] == "location_wanted" && e["request_id"] == "fix-1"
        ))
        .await,
        "the phone never heard the request"
    );
    assert!(
        !other
            .seen
            .lock()
            .unwrap()
            .events
            .iter()
            .any(|e| e["type"] == "location_wanted"),
        "only the phone asked hears it"
    );
    let got = p
        .call(
            "phone.device_report",
            json!({"request_id": "fix-1", "exact": {"latitude": 18.5, "longitude": 73.8, "accuracy_m": 20.0}}),
        )
        .await
        .unwrap();
    assert_eq!(got["delivered"], true);
    let fix = tokio::time::timeout(Duration::from_secs(5), rx)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!((fix.latitude, fix.longitude), (18.5, 73.8));
}

/// PHN-828 end to end: a phone signed in to another FERB account sends its assertion in the hello; the daemon refuses
/// it before the owner is asked, and the phone stops with the daemon's code and words (the QR's code is used up).
// decision: PHN-828
#[tokio::test(flavor = "multi_thread")]
async fn a_phone_of_another_account_is_refused_through_the_real_bridge() {
    let w = real(5).await;
    let p = TestPhone::start("Pixel", &w.shared, false, None, true).await;
    let invite = w.phones.code_start().await.expect("phone.pair_start");
    p.phone
        .pair_with_account(
            invite["code"].as_str().unwrap(),
            Some(&assertion(
                &p.phone.id(),
                "11111111-2222-4333-8444-555555555555",
                "bob@example.com",
            )),
        )
        .unwrap();
    assert!(
        wait(30, || p.stop_code() == errors::ACCOUNT_MISMATCH).await,
        "{}",
        p.phone.phase()
    );
    let mac = mac_account();
    assert_eq!(
        p.phone.phase()["error"]["message"],
        format!(
            "This phone is signed in as bob@example.com, but FERB on this Mac is signed in as {}. Sign in with the \
             same account on both.",
            mac.email
        )
    );
    assert!(
        !w.published
            .lock()
            .unwrap()
            .iter()
            .any(|(k, _)| k == "phone.pair_requested"),
        "the owner was never asked"
    );
    // without an assertion at all (an older phone app): "couldn't confirm"
    let q = TestPhone::start("Old", &w.shared, false, None, true).await;
    let invite = w.phones.code_start().await.expect("phone.pair_start");
    q.phone.pair(invite["code"].as_str().unwrap()).unwrap();
    assert!(
        wait(30, || q.stop_code() == errors::ACCOUNT_UNCONFIRMED).await,
        "{}",
        q.phone.phase()
    );
    drop(w.server);
}
