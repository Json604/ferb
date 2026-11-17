//! PHN-646..649 through a real ferb-rpc server: a phone forgets itself, a pairing Yes opens the phone's FERB, the
//! device key's presence keeps it open while the app is unlocked and in front, and the unlock statement the phone
//! signed in its opening prompt before the link was up.

mod phone_support;

use ferb_rpc::phone_gate as code;
use phone_support::*;
use serde_json::{json, Value};

const P1: &str = "endpoint-one";
const P2: &str = "endpoint-two";

/// The phone's unlock statement: `{domain, nonce, made}` signed by `key`.
fn statement(key: &p256::ecdsa::SigningKey, nonce: &str, made: i64) -> Value {
    signed(
        key,
        &json!({"domain": "ferb.unlock-statement.v1", "nonce": nonce, "made": made}).to_string(),
    )
}

/// The device key's presence, saying the app is unlocked and in front when `active`.
async fn presence(c: &mut Conn, keys: &Keys, active: bool) -> Value {
    let ch = c.challenge().await;
    let params = if active {
        json!({"active": true})
    } else {
        json!({})
    };
    let p = call_proof(&keys.device, &ch, "link.present", &params);
    c.call("phone.present", json!({}), Some(p)).await.unwrap()
}

fn locked(r: Result<Value, (i64, String)>) -> i64 {
    r.unwrap_err().0
}

// decision: PHN-646
#[tokio::test]
async fn a_phone_forgets_itself_and_only_itself() {
    let w = world(None).await;
    let (k1, k2) = (Keys::new(1), Keys::new(2));
    pair(&w, P1, &k1, false, true).await;
    pair(&w, P2, &k2, false, true).await;
    let mut one = Conn::phone(&w, P1, "s1").await;
    // a connection that hasn't proved the phone's device key can't remove it
    assert_eq!(
        locked(one.call("phone.forget_self", json!({}), None).await),
        code::UNLOCK_INVALID
    );
    one.present(&k1).await;
    let out = one.ok("phone.forget_self", json!({})).await;
    assert_eq!(out["removed"], P1);
    let listed = w.phones.list(&app_session()).unwrap();
    let ids: Vec<&str> = listed["phones"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["id"].as_str().unwrap_or_default())
        .collect();
    assert!(!ids.contains(&P1), "{listed:?}");
    assert!(ids.contains(&P2), "the other phone stays: {listed:?}");
    // the removed phone's calls are refused, the other phone goes on
    assert_eq!(
        locked(
            Conn::open(&w, json!({"role": "phone", "device": P1, "session": "s2"}))
                .await
                .map(|_| Value::Null)
        ),
        code::UNKNOWN_PHONE
    );
    let mut two = Conn::phone(&w, P2, "t").await;
    two.unlock(&k2).await;
    two.ok("tasks.list", json!({})).await;
    let audit = audit_text(&w);
    assert!(audit.contains("phone.forget_self"), "{audit}");
}

// decision: PHN-647
#[tokio::test]
async fn a_pairing_yes_opens_the_new_phone_with_no_unlock() {
    let w = world(None).await;
    let keys = Keys::new(1);
    pair(&w, P1, &keys, false, true).await;
    let mut c = Conn::phone(&w, P1, "s").await;
    // a connection that hasn't proved it is that phone gets nothing from the pairing
    assert_eq!(
        locked(c.call("tasks.list", json!({}), None).await),
        code::LOCKED
    );
    c.present(&keys).await;
    c.ok("tasks.list", json!({})).await;
    // the same idle rule as a fingerprint's unlock
    w.clock.advance(301.0);
    assert_eq!(
        locked(c.call("tasks.list", json!({}), None).await),
        code::LOCKED
    );
}

// decision: PHN-648
#[tokio::test]
async fn presence_while_the_app_is_open_keeps_the_window_but_never_past_twelve_hours_or_a_lock() {
    let w = world(None).await;
    let keys = Keys::new(1);
    pair(&w, P1, &keys, false, true).await;
    let mut c = Conn::phone(&w, P1, "s").await;
    c.unlock(&keys).await;
    presence(&mut c, &keys, false).await;
    // four minutes at a time with no private call: the app's presence moves the idle end
    for _ in 0..5 {
        w.clock.advance(240.0);
        presence(&mut c, &keys, true).await;
    }
    c.ok("tasks.list", json!({})).await;
    // a presence that doesn't say the app is open changes nothing
    w.clock.advance(240.0);
    presence(&mut c, &keys, false).await;
    w.clock.advance(100.0);
    assert_eq!(
        locked(c.call("tasks.list", json!({}), None).await),
        code::LOCKED
    );
    // never past the 12 h limit
    c.unlock(&keys).await;
    for _ in 0..(12 * 60 / 4 + 2) {
        w.clock.advance(240.0);
        presence(&mut c, &keys, true).await;
    }
    assert_eq!(
        locked(c.call("tasks.list", json!({}), None).await),
        code::LOCKED
    );
    // and not after the app locked
    c.unlock(&keys).await;
    c.ok("phone.lock", json!({})).await;
    presence(&mut c, &keys, true).await;
    assert_eq!(
        locked(c.call("tasks.list", json!({}), None).await),
        code::LOCKED
    );
}

// decision: PHN-649
#[tokio::test]
async fn an_unlock_statement_opens_once_fresh_from_this_phone_on_a_proved_connection() {
    let w = world(None).await;
    let (k1, k2) = (Keys::new(1), Keys::new(2));
    pair(&w, P1, &k1, false, true).await;
    pair(&w, P2, &k2, false, true).await;
    let mut c = Conn::phone(&w, P1, "s").await;
    let st = statement(&k1.lock, "nonce-0123456789abcdef", 1_700_000_000_000);
    // a connection that hasn't proved the phone's device key (yet: the phone tries again)
    assert_eq!(
        locked(
            c.call(
                "phone.unlock_statement",
                json!({"age_ms": 4000}),
                Some(st.clone())
            )
            .await
        ),
        code::LOCKED
    );
    c.present(&k1).await;
    // too old by the phone's steady clock: the app asks again
    assert_eq!(
        locked(
            c.call(
                "phone.unlock_statement",
                json!({"age_ms": 121_000}),
                Some(st.clone())
            )
            .await
        ),
        code::UNLOCK_REQUIRED
    );
    let r = c
        .call(
            "phone.unlock_statement",
            json!({"age_ms": 4000}),
            Some(st.clone()),
        )
        .await;
    assert!(r.is_ok(), "{r:?}");
    c.ok("tasks.list", json!({})).await;
    // once only
    assert_eq!(
        locked(
            c.call(
                "phone.unlock_statement",
                json!({"age_ms": 4000}),
                Some(st.clone())
            )
            .await
        ),
        code::UNLOCK_INVALID
    );
}

// decision: PHN-649
#[tokio::test]
async fn an_unlock_statement_is_refused_when_replayed_stolen_signed_by_another_key_or_older_than_a_lock(
) {
    let w = world(None).await;
    let (k1, k2) = (Keys::new(1), Keys::new(2));
    pair(&w, P1, &k1, false, true).await;
    pair(&w, P2, &k2, false, true).await;
    let st = statement(&k1.lock, "nonce-replay-0123456789", 0);
    let mut c = Conn::phone(&w, P1, "s").await;
    c.present(&k1).await;
    c.call(
        "phone.unlock_statement",
        json!({"age_ms": 1000}),
        Some(st.clone()),
    )
    .await
    .unwrap();
    // the same statement again, on another connection of the same phone
    let mut again = Conn::phone(&w, P1, "s2").await;
    again.present(&k1).await;
    assert_eq!(
        locked(
            again
                .call(
                    "phone.unlock_statement",
                    json!({"age_ms": 1000}),
                    Some(st.clone())
                )
                .await
        ),
        code::UNLOCK_INVALID
    );
    // the statement on the other phone's connection: its keys don't verify it
    let mut other = Conn::phone(&w, P2, "t").await;
    other.present(&k2).await;
    let fresh = statement(&k1.lock, "nonce-other-phone-01234", 0);
    assert_eq!(
        locked(
            other
                .call(
                    "phone.unlock_statement",
                    json!({"age_ms": 1000}),
                    Some(fresh)
                )
                .await
        ),
        code::UNLOCK_INVALID
    );
    // the device key (no prompt) can't make one
    let by_device = statement(&k1.device, "nonce-device-key-012345", 0);
    assert_eq!(
        locked(
            again
                .call(
                    "phone.unlock_statement",
                    json!({"age_ms": 1000}),
                    Some(by_device)
                )
                .await
        ),
        code::UNLOCK_INVALID
    );
    // a challenge-signed proof is not a statement
    let ch = again.challenge().await;
    let proof = call_proof(&k1.lock, &ch, "link.unlock", &json!({}));
    assert_eq!(
        locked(
            again
                .call(
                    "phone.unlock_statement",
                    json!({"age_ms": 1000}),
                    Some(proof)
                )
                .await
        ),
        code::UNLOCK_INVALID
    );
    // made before the app locked
    let before = statement(&k1.lock, "nonce-before-lock-01234", 0);
    w.clock.advance(5.0);
    again.ok("phone.lock", json!({})).await;
    w.clock.advance(1.0);
    assert_eq!(
        locked(
            again
                .call(
                    "phone.unlock_statement",
                    json!({"age_ms": 3000}),
                    Some(before.clone())
                )
                .await
        ),
        code::UNLOCK_INVALID
    );
    // a phone's wrong wall clock refuses and admits nothing: only its steady age counts
    let skewed = statement(&k1.lock, "nonce-wrong-clock-01234", -99_999_999_999);
    again
        .call(
            "phone.unlock_statement",
            json!({"age_ms": 500}),
            Some(skewed),
        )
        .await
        .unwrap();
    again.ok("tasks.list", json!({})).await;
}

// decision: PHN-647, PHN-649
#[tokio::test]
async fn a_restart_asks_for_the_unlock_again() {
    let w = world(None).await;
    let keys = Keys::new(1);
    pair(&w, P1, &keys, false, true).await;
    let mut c = Conn::phone(&w, P1, "s").await;
    c.present(&keys).await;
    c.ok("tasks.list", json!({})).await;
    // the daemon's windows live in memory only: a new daemon over the same store starts every phone locked
    let w2 = reopen(w).await;
    let mut c = Conn::phone(&w2, P1, "s").await;
    c.present(&keys).await;
    assert_eq!(
        locked(c.call("tasks.list", json!({}), None).await),
        code::LOCKED
    );
}

/// The statements the phones' own code made and signed (Android's Keystore on the emulator, iOS's keys on the
/// simulator; engine/fixtures/phone/*-statements.json): each vector's key registered as a phone's lock key, the real
/// gate takes each statement once.
// decision: PHN-649
#[tokio::test]
async fn the_phones_own_statements_open_through_the_real_gate() {
    for file in ["android-statements.json", "ios-statements.json"] {
        let path = format!("{}/../../fixtures/phone/{file}", env!("CARGO_MANIFEST_DIR"));
        let doc: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        for (i, v) in doc["vectors"].as_array().unwrap().iter().enumerate() {
            let w = world(None).await;
            let device = Keys::new(9);
            let endpoint = format!("endpoint-{i}");
            let mut link = Conn::link(&w).await;
            let asked = link
                .ok(
                    "link.pair_request",
                    json!({"endpoint": endpoint, "name": "Phone", "app": "FERB", "code": "123 456",
                           "unlock_pub": v["key"], "device_pub": spki(&device.device)}),
                )
                .await;
            let mut yes = ferb_rpc::Params::new();
            yes.insert("yes".into(), json!(true));
            yes.insert("request_id".into(), asked["request_id"].clone());
            w.phones.pair_decide(&yes).unwrap();
            let mut c = Conn::phone(&w, &endpoint, "s").await;
            c.present(&device).await;
            c.ok("phone.lock", json!({})).await; // the pairing's own window closed: only the statement opens it
            w.clock.advance(1.0);
            let proof = json!({"payload": v["payload"], "sig": v["sig"]});
            let r = c
                .call(
                    "phone.unlock_statement",
                    json!({"age_ms": 500}),
                    Some(proof),
                )
                .await;
            assert!(r.is_ok(), "{file} {}: {r:?}", v["what"]);
            c.ok("tasks.list", json!({})).await;
        }
    }
}
