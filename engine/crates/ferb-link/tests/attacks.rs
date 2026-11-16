#![cfg(target_os = "macos")]
//! The attacks of the independent file-access review, one test per finding. Each of these fails on the code as it
//! was before its fix (they were run against that commit in a scratch copy) and passes now. All on 127.0.0.1.
//! A path attack counts as failed only when nothing of the guarded file comes back without its own fresh proof.
mod common;
use common::*;
use ferb_link::{errors, net::Shared, pairing::Invite};
use serde_json::{Value, json};
use std::time::Duration;

async fn world() -> (Relay, World, TestPhone) {
    let relay = Relay::start(None, None).await;
    let w = World::start(Shared::Custom(vec![relay.spec()]), None, false).await;
    let p = w.paired_phone("phone").await;
    (relay, w, p)
}

// decision: PHN-124
#[tokio::test(flavor = "multi_thread")]
async fn finding_1_a_pairing_code_never_moves_a_paired_phone_to_another_mac() {
    let (relay, w, p) = world().await;
    let evil = Invite {
        mac: iroh::SecretKey::from_bytes(&[66; 32]).public(),
        relays: vec![relay.url()],
        own: None,
        secret: [6; 32],
    }
    .to_url();
    let _ = p.phone.pair(&evil);
    tokio::time::sleep(Duration::from_secs(3)).await;
    let mac = p.saved().and_then(|s| s.mac).map(|m| (m.id, m.paired));
    assert_eq!(
        mac,
        Some((w.bridge.id(), true)),
        "the phone left its Mac for the one in the link: {}",
        p.phone.phase()
    );
    assert!(
        p.call("ping", json!({})).await.is_ok(),
        "the phone no longer reaches its own Mac: {}",
        p.phone.phase()
    );
}

// decision: PHN-124
#[tokio::test(flavor = "multi_thread")]
async fn finding_6_both_screens_show_the_same_code_and_the_mac_says_what_the_phone_can_do() {
    let relay = Relay::start(None, None).await;
    let w = World::start(Shared::Custom(vec![relay.spec()]), None, true).await;
    let p = w.phone("phone").await;
    let mut admin = w.bridge.admin_events();
    let qr = w.bridge.pair_start().unwrap();
    p.phone.pair(qr["url"].as_str().unwrap()).unwrap();
    assert!(
        wait(20, || w.bridge.status()["pairing"]["state"]
            == "waiting_for_your_yes")
        .await
    );
    let mac_code = w.bridge.status()["pairing"]["code"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(
        mac_code.chars().filter(char::is_ascii_digit).count() == 6,
        "the Mac shows no 6-digit code: {:?}",
        w.bridge.status()["pairing"]
    );
    assert!(
        wait(5, || p.phone.phase()["code"].as_str()
            == Some(mac_code.as_str()))
        .await,
        "the phone shows another code: Mac {mac_code}, phone {}",
        p.phone.phase()
    );
    // (the question itself, with what the phone can do, is the daemon's since S4: ferb-daemon phone `pair_request`,
    // checked in tests/real_daemon.rs; here the helper hands it the code both screens show)
    let mut asked = String::new();
    while let Ok(e) = admin.try_recv() {
        if e["event"] == "pair_request" {
            asked = e["code"].as_str().unwrap_or_default().to_string();
        }
    }
    assert_eq!(
        asked, mac_code,
        "the daemon was not asked with the code both screens show"
    );
    assert_eq!(w.standin.pairing_code().as_deref(), Some(mac_code.as_str()));
    // another phone that photographed the same code and proves the secret first shows a different code
    w.decide(false).unwrap();
    let thief = w.phone("phone").await; // the same name on purpose
    let qr = w.bridge.pair_start().unwrap();
    thief.phone.pair(qr["url"].as_str().unwrap()).unwrap();
    assert!(
        wait(20, || w.bridge.status()["pairing"]["state"]
            == "waiting_for_your_yes")
        .await
    );
    assert_ne!(
        w.bridge.status()["pairing"]["code"]
            .as_str()
            .unwrap_or_default(),
        mac_code,
        "two phones got the same code"
    );
    w.decide(false).unwrap();
}

// decision: PHN-124, PHN-114
#[tokio::test(flavor = "multi_thread")]
async fn finding_8_one_unlock_ends_however_much_it_is_used_and_when_the_app_locks() {
    let (_relay, w, p) = world().await;
    w.limits_set(&json!({"unlock_idle_s": 300, "unlock_max_s": 3}))
        .unwrap();
    p.unlock().await.unwrap();
    let started = std::time::Instant::now();
    let mut last_ok = Duration::ZERO;
    while started.elapsed() < Duration::from_secs(7) {
        if p.call("settings.get", json!({})).await.is_ok() {
            last_ok = started.elapsed();
        }
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
    assert!(
        last_ok < Duration::from_millis(3600),
        "kept in use, the unlock was still open after {last_ok:?} (maximum 3 s)"
    );
    assert_eq!(
        common::code(p.call("settings.get", json!({})).await),
        errors::LOCKED
    );
    // the app's own lock call closes the window at once
    w.limits_set(&json!({"unlock_max_s": 900})).unwrap();
    p.unlock().await.unwrap();
    assert!(p.call("settings.get", json!({})).await.is_ok());
    p.call("link.lock", json!({})).await.unwrap();
    assert_eq!(
        common::code(p.call("settings.get", json!({})).await),
        errors::LOCKED
    );
    assert_eq!(
        common::code(p.call("link.files", json!({"path": "~"})).await),
        errors::LOCKED
    );
}

// decision: PHN-124, PHN-114
#[tokio::test(flavor = "multi_thread")]
async fn finding_10_the_audit_has_pairing_wrong_proofs_removals_and_relay_changes_and_it_rotates() {
    let relay = Relay::start(None, None).await;
    let w = World::start(Shared::Custom(vec![relay.spec()]), None, true).await;
    let mac_id: iroh::EndpointId = w.bridge.id().parse().unwrap();
    // a wrong proof
    w.bridge.pair_start().unwrap();
    let guesser = w.phone("guesser").await;
    guesser
        .phone
        .pair(
            &Invite {
                mac: mac_id,
                relays: vec![relay.url()],
                own: None,
                secret: [1; 32],
            }
            .to_url(),
        )
        .unwrap();
    assert!(guesser.wait_phase("stopped", 15).await);
    // a pairing the owner refuses, one he accepts, then a removal
    let denied = w.phone("denied").await;
    let qr = w.bridge.pair_start().unwrap();
    denied.phone.pair(qr["url"].as_str().unwrap()).unwrap();
    assert!(
        wait(20, || w.bridge.status()["pairing"]["state"]
            == "waiting_for_your_yes")
        .await
    );
    w.decide(false).unwrap();
    let p = w.paired_phone("kept").await;
    let id = p.phone.id();
    // a relay change
    let own = Relay::start(Some("tok"), None).await;
    w.bridge.relay_set(Some(own.spec())).await.unwrap();
    assert!(
        wait(30, || w.bridge.status()["relay"]["state"] == "own").await,
        "{}",
        w.bridge.status()["relay"]
    );
    w.remove(&id).unwrap();
    assert!(
        wait(5, || w
            .bridge
            .audit(500)
            .iter()
            .any(|a| a["method"] == "mac.device_revoked"))
        .await
    );
    let audit = w.bridge.audit(500);
    let has = |m: &str, f: &dyn Fn(&Value) -> bool| audit.iter().any(|a| a["method"] == m && f(a));
    let missing: Vec<&str> = [
        (
            "a wrong pairing proof",
            has("mac.pair_wrong_secret", &|_| true),
        ),
        (
            "a pairing request",
            has("mac.pair_request", &|a| a["name"] == "denied"),
        ),
        (
            "a refused pairing",
            has("mac.pair_ended", &|a| a["paired"] == false),
        ),
        (
            "an accepted pairing",
            has("mac.pair_ended", &|a| {
                a["paired"] == true && a["device"] == json!(id)
            }),
        ),
        (
            "a removal",
            has("mac.device_revoked", &|a| a["device"] == json!(id)),
        ),
        ("a relay change", has("mac.relay", &|_| true)),
    ]
    .into_iter()
    .filter(|(_, ok)| !ok)
    .map(|(n, _)| n)
    .collect();
    assert!(
        missing.is_empty(),
        "not in the audit: {missing:?}\n{audit:?}"
    );
    assert!(
        !std::fs::read_to_string(w.dir.join("mac/audit.jsonl"))
            .unwrap()
            .contains("tok"),
        "the relay's token is in the audit"
    );

    // rotation: the file never grows past its limit by more than a line, and only `audit_keep` old ones stay
    w.limits_set(&json!({"audit_max_bytes": 4000, "audit_keep": 2, "calls_per_10s": 100000, "devices_max": 3})).unwrap();
    let q = w.paired_phone("writer").await;
    for _ in 0..400 {
        let _ = q.call("made.up", json!({})).await; // each refusal is one audit line
    }
    let size = |name: &str| {
        std::fs::metadata(w.dir.join("mac").join(name))
            .map(|m| m.len())
            .ok()
    };
    assert!(
        size("audit.jsonl").unwrap() < 4600,
        "the audit file was not rotated: {:?} bytes",
        size("audit.jsonl")
    );
    assert!(
        size("audit.jsonl.1").is_some() && size("audit.jsonl.2").is_some(),
        "no rotated files"
    );
    assert!(
        size("audit.jsonl.3").is_none(),
        "more old audit files are kept than audit_keep"
    );
    let all: u64 = ["audit.jsonl", "audit.jsonl.1", "audit.jsonl.2"]
        .iter()
        .filter_map(|n| size(n))
        .sum();
    assert!(all < 3 * 4600, "{all}");
    // the newest lines are still what the owner's side reads
    assert!(
        w.bridge.audit(5).iter().all(|a| a["method"] == "made.up"),
        "{:?}",
        w.bridge.audit(5)
    );
    use std::os::unix::fs::PermissionsExt;
    for f in ["audit.jsonl", "audit.jsonl.1", "audit.jsonl.2"] {
        assert_eq!(
            std::fs::metadata(w.dir.join("mac").join(f))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600,
            "{f}"
        );
    }
}
