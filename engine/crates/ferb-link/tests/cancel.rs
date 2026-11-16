//! A call the app cancels through the door (T-1218): its task is aborted, its streams are dropped, the Mac's bridge
//! ends the daemon's stream at its next event (`forward_stream`), and no result or error follows for its id.
mod common;

use common::*;
use ferb_link::{ffi, net::Shared};
use p256::ecdsa::{SigningKey, signature::Signer};
use p256::pkcs8::EncodePublicKey;
use serde_json::{Value, json};
use std::time::{Duration, Instant};

fn parse(s: &str) -> Value {
    serde_json::from_str(s).unwrap_or(Value::Null)
}

/// The next event for `id` (a result, an error or a stream event), within `secs`.
fn next_for(h: u64, id: &str, secs: u64) -> Option<Value> {
    let end = Instant::now() + Duration::from_secs(secs);
    while Instant::now() < end {
        let ev = parse(&ffi::client_poll(h, 200));
        let found = match &ev {
            Value::Array(all) => all.iter().find(|e| e["id"] == json!(id)).cloned(),
            e if e["id"] == json!(id) => Some(e.clone()),
            _ => None,
        };
        if found.is_some() {
            return found;
        }
    }
    None
}

fn call(h: u64, id: &str, method: &str, params: Value, proof: Option<Value>) -> Value {
    let mut c = json!({"cmd": "call", "id": id, "method": method, "params": params});
    if let Some(p) = proof {
        c["proof"] = p;
    }
    assert_eq!(parse(&ffi::client_command(h, &c.to_string()))["ok"], true);
    next_for(h, id, 20).unwrap_or_else(|| panic!("no answer to {method}"))
}

// decision: PHN-446
#[test]
fn a_cancelled_call_ends_the_daemon_s_stream_and_says_nothing_more() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let relay = rt.block_on(Relay::start(None, None));
    let w = rt.block_on(World::start(
        Shared::Custom(vec![relay.spec()]),
        None,
        false,
    ));
    let unlock = SigningKey::random(&mut p256::elliptic_curve::rand_core::OsRng);
    let unlock_pub = b64(unlock
        .verifying_key()
        .to_public_key_der()
        .unwrap()
        .as_bytes());
    let started = parse(&ffi::client_start(
        &json!({"name": "phone", "app": "test", "unlock_pub": unlock_pub,
                "dev": {"shared_relays": [relay.url()], "portmapper": false, "loopback": true}})
        .to_string(),
    ));
    let h = started["handle"].as_u64().expect("a client");
    let invite = w.bridge.pair_start().unwrap();
    let paired = ffi::client_command(h, &json!({"cmd": "pair", "qr": invite["code"]}).to_string());
    assert_eq!(parse(&paired)["ok"], true, "{paired}");
    let end = Instant::now() + Duration::from_secs(30);
    while w.standin.pairing_code().is_none() && Instant::now() < end {
        std::thread::sleep(Duration::from_millis(50));
    }
    w.decide(true).unwrap();
    let online = || {
        parse(&ffi::client_command(
            h,
            &json!({"cmd": "phase"}).to_string(),
        ))["phase"]["phase"]
            == json!("online")
    };
    let end = Instant::now() + Duration::from_secs(30);
    while !online() && Instant::now() < end {
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(online(), "the client never came online");
    // the unlock, signed by the app's (software, here) key
    let ch = call(h, "ch", "link.challenge", json!({}), None);
    let challenge = ch["result"]["challenge"].as_str().unwrap().to_string();
    let payload = parse(&ffi::client_command(
        h,
        &json!({"cmd": "call_payload", "challenge": challenge, "method": "link.unlock", "params": {}})
            .to_string(),
    ))["payload"]
        .as_str()
        .unwrap()
        .to_string();
    let sig: p256::ecdsa::Signature = unlock.sign(payload.as_bytes());
    let unlocked = call(
        h,
        "un",
        "link.unlock",
        json!({}),
        Some(json!({"payload": payload, "sig": b64(sig.to_der().as_bytes())})),
    );
    assert_eq!(unlocked["type"], "result", "{unlocked}");
    // a long task's stream: the first event, then the app cancels
    let first = call(h, "s1", "chat.send", json!({"text": "long 30"}), None);
    assert_eq!(first["type"], "event", "{first}");
    let state = || rt.block_on(w.standin.cli("standin.state", json!({})));
    assert_eq!(state()["streams_left"], 0);
    let cancelled = ffi::client_command(
        h,
        &json!({"cmd": "cancel", "id": "c1", "target": "s1"}).to_string(),
    );
    assert_eq!(parse(&cancelled)["ok"], true, "{cancelled}");
    let end = Instant::now() + Duration::from_secs(10);
    while state()["streams_left"] == 0 && Instant::now() < end {
        std::thread::sleep(Duration::from_millis(100));
    }
    assert_eq!(
        state()["streams_left"],
        1,
        "the daemon's stream did not end"
    );
    // no result or error ever follows for the cancelled call (events it had already sent may still be queued)
    let end = Instant::now() + Duration::from_secs(2);
    while Instant::now() < end {
        if let Some(ev) = next_for(h, "s1", 1) {
            assert_eq!(ev["type"], "event", "{ev}");
        }
    }
    ffi::client_close(h);
}
