#![cfg(target_os = "macos")]
//! Pairing and its attacks (P1): no secret, wrong secret, replayed proof, reused code, expired window, the
//! owner's No, the device limit. All on 127.0.0.1.
mod common;
use common::*;
use ferb_link::{errors, net::Shared, pairing::Invite};
use serde_json::json;
use std::time::Duration;

async fn world() -> (Relay, World) {
    let relay = Relay::start(None, None).await;
    let w = World::start(Shared::Custom(vec![relay.spec()]), None, true).await;
    (relay, w)
}

// decision: PHN-102, PHN-204, PHN-124
#[tokio::test(flavor = "multi_thread")]
async fn only_the_qr_secret_and_the_owners_yes_pair_a_phone() {
    let (relay, w) = world().await;
    let mut admin = w.bridge.admin_events();
    let asked = |admin: &mut tokio::sync::broadcast::Receiver<serde_json::Value>| {
        let mut n = 0;
        while let Ok(e) = admin.try_recv() {
            n += (e["event"] == "pair_request") as u32;
        }
        n
    };
    let mac_id: iroh::EndpointId = w.bridge.id().parse().unwrap();
    let fake_qr = |secret: [u8; 32]| {
        Invite {
            mac: mac_id,
            relays: vec![relay.url()],
            own: None,
            secret,
        }
        .to_url()
    };

    // 1. a stranger who knows the Mac's id and relay, no pairing window open: turned away, nothing shown
    let s1 = w.phone("stranger").await;
    s1.phone.pair(&fake_qr([7; 32])).unwrap();
    assert!(s1.wait_phase("stopped", 15).await, "{}", s1.phone.phase());
    assert_eq!(s1.stop_code(), errors::PAIR_WINDOW_CLOSED);
    assert_eq!(
        asked(&mut admin),
        0,
        "the Mac must not prompt for an endpoint without the secret"
    );

    // 2. window open, wrong secret: refused; the third wrong proof ends the window, so even the right code fails
    let qr = w.bridge.pair_start().unwrap()["url"]
        .as_str()
        .unwrap()
        .to_string();
    for i in 0..3 {
        let s = w.phone("guesser").await;
        s.phone.pair(&fake_qr([i; 32])).unwrap();
        assert!(s.wait_phase("stopped", 15).await);
        assert_eq!(s.stop_code(), errors::PAIR_SECRET_WRONG, "try {i}");
    }
    let real = w.phone("real").await;
    real.phone.pair(&qr).unwrap();
    assert!(real.wait_phase("stopped", 15).await);
    assert_eq!(
        real.stop_code(),
        errors::PAIR_WINDOW_CLOSED,
        "after 3 wrong proofs the window is closed for everyone"
    );
    assert_eq!(asked(&mut admin), 0);

    // 3. the right code: the Mac asks, the owner says Yes, the phone is paired
    let good = w.phone("good").await;
    let qr = w.bridge.pair_start().unwrap()["url"]
        .as_str()
        .unwrap()
        .to_string();
    good.phone.pair(&qr).unwrap();
    assert!(
        wait(15, || w.bridge.status()["pairing"]["state"]
            == "waiting_for_your_yes")
        .await
    );
    assert_eq!(asked(&mut admin), 1);
    // 3b. while the owner thinks, a second endpoint with the same (now used) code is refused: single use
    let thief = w.phone("thief").await;
    thief.phone.pair(&qr).unwrap();
    assert!(thief.wait_phase("stopped", 15).await);
    assert_eq!(thief.stop_code(), errors::PAIR_WINDOW_CLOSED);
    w.decide(true).unwrap();
    assert!(
        good.wait_phase("online", 15).await,
        "{}",
        good.phone.phase()
    );
    assert_eq!(w.bridge.devices().len(), 1);
    assert_eq!(w.bridge.devices()[0].name, "good");
    // 3c. the same code again after success
    let late = w.phone("late").await;
    late.phone.pair(&qr).unwrap();
    assert!(late.wait_phase("stopped", 15).await);
    assert_eq!(late.stop_code(), errors::PAIR_WINDOW_CLOSED);
    assert_eq!(asked(&mut admin), 0);

    // 4. a proof made for one phone identity, replayed by another endpoint (what a relay or observer could
    //    try if it ever saw one): the proof covers both ids, so it is simply a wrong proof
    let qr = w.bridge.pair_start().unwrap()["url"]
        .as_str()
        .unwrap()
        .to_string();
    let invite = Invite::parse(&qr).unwrap();
    let victim_id = iroh::SecretKey::from_bytes(&[9; 32]).public();
    let stolen = ferb_link::pairing::proof(&invite.secret, mac_id, victim_id);
    let ep = ferb_link::net::bind(&ferb_link::net::NetConfig {
        secret: iroh::SecretKey::from_bytes(&[3; 32]),
        own_relay: None,
        shared: w.shared.clone(),
        roots: None,
        force_relay: true,
        accept: false,
        streams_max: 1,
        relay_only: false,
        segmentation_offload: true,
        portmapper: false,
        loopback_only: true,
    })
    .await
    .unwrap();
    ferb_link::net::wait_home(&ep, Duration::from_secs(10)).await;
    let conn = ep
        .connect(
            ferb_link::net::dial_addr(mac_id, &[relay.url().parse().unwrap()]),
            ferb_link::net::ALPN,
        )
        .await
        .unwrap();
    let (mut tx, mut rx) = conn.open_bi().await.unwrap();
    use base64::Engine;
    ferb_link::wire::write_json(&mut tx, ferb_link::wire::K_HELLO, &json!({"v": 2, "min_v": 2, "name": "replay", "pair": base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(stolen)})).await.unwrap();
    let (kind, body) = ferb_link::wire::read_frame(&mut rx, 65536, Duration::from_secs(10), false)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(kind, ferb_link::wire::K_ERROR);
    assert_eq!(
        ferb_link::wire::parse::<errors::LinkError>(&body)
            .unwrap()
            .code,
        errors::PAIR_SECRET_WRONG
    );
    assert_eq!(asked(&mut admin), 0);
    w.bridge.pair_cancel();

    // 5. the owner says No
    let denied = w.phone("denied").await;
    let qr = w.bridge.pair_start().unwrap()["url"]
        .as_str()
        .unwrap()
        .to_string();
    denied.phone.pair(&qr).unwrap();
    assert!(
        wait(15, || w.bridge.status()["pairing"]["state"]
            == "waiting_for_your_yes")
        .await
    );
    w.decide(false).unwrap();
    assert!(denied.wait_phase("stopped", 15).await);
    assert_eq!(denied.stop_code(), errors::PAIR_DENIED);
    assert_eq!(w.bridge.devices().len(), 1);

    // 6. an expired window
    w.limits_set(&json!({"pair_window_s": 1})).unwrap();
    let qr = w.bridge.pair_start().unwrap()["url"]
        .as_str()
        .unwrap()
        .to_string();
    tokio::time::sleep(Duration::from_millis(1500)).await;
    let slow = w.phone("slow").await;
    slow.phone.pair(&qr).unwrap();
    assert!(slow.wait_phase("stopped", 15).await);
    assert_eq!(
        slow.stop_code(),
        errors::PAIR_WINDOW_CLOSED,
        "an expired window is a closed window"
    );
    w.limits_set(&json!({"pair_window_s": 120})).unwrap();

    // 7. the device limit (the daemon's: 5 in FERB, PHN-419; 2 here): a second phone pairs, a third cannot even get
    //    a code
    w.limits_set(&json!({"devices_max": 2})).unwrap();
    let second = w.paired_phone("second").await;
    assert_eq!(w.bridge.devices().len(), 2);
    assert_eq!(common::code(w.bridge.pair_start()), errors::DEVICE_LIMIT);

    // 7b. a pairing link can never replace the Mac a phone is paired with (a link opened from a web page or
    //     another app must not move the phone to an attacker's Mac): refused, and the phone stays where it is
    let evil = Invite {
        mac: iroh::SecretKey::from_bytes(&[66; 32]).public(),
        relays: vec![relay.url()],
        own: None,
        secret: [6; 32],
    }
    .to_url();
    let e = second.phone.pair(&evil).unwrap_err();
    assert_eq!(e.code, errors::ALREADY_PAIRED);
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert!(
        second.phase() == "online" && second.saved().unwrap().mac.unwrap().id == w.bridge.id(),
        "the phone left its Mac: {}",
        second.phone.phase()
    );
    assert!(second.call("ping", serde_json::json!({})).await.is_ok());

    // 8. rename, and the list
    let id = second.phone.id();
    w.rename(&id, "My  phone\n").unwrap();
    assert!(
        w.bridge
            .devices()
            .iter()
            .any(|d| d.id == id && d.name == "My  phone")
    );

    // 9. a stopped phone does not keep dialing
    let before = s1.phone.dial_times().len();
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert_eq!(
        s1.phone.dial_times().len(),
        before,
        "a refused phone must not redial"
    );
}

// decision: PHN-135, PHN-102
#[tokio::test(flavor = "multi_thread")]
async fn the_short_code_and_the_old_link_open_the_same_window() {
    let (_relay, w) = world().await;
    w.limits_set(&json!({"devices_max": 3})).unwrap();
    // a phone app from before the short code scans the link: paired as before
    let old = w.phone("old app").await;
    w.pair_with(&old, "url").await;
    // the short code: the same proof, the same Yes
    let new = w.phone("new app").await;
    w.pair_with(&new, "code").await;
    // one window, whichever form was read: the code of a used window does not work as the link either
    let qr = w.bridge.pair_start().unwrap();
    let (code, url) = (
        qr["code"].as_str().unwrap().to_string(),
        qr["url"].as_str().unwrap().to_string(),
    );
    assert_eq!(
        Invite::parse(&code).unwrap(),
        Invite::parse(&url).unwrap(),
        "both say the same Mac, relay and secret"
    );
    assert!(
        qr["qr"].as_array().unwrap().len() < qr["qr_old"].as_array().unwrap().len(),
        "{qr}"
    );
    let first = w.phone("first").await;
    first.phone.pair(&code).unwrap();
    assert!(
        wait(15, || w.bridge.status()["pairing"]["state"]
            == "waiting_for_your_yes")
        .await
    );
    let second = w.phone("second").await;
    second.phone.pair(&url).unwrap();
    assert!(second.wait_phase("stopped", 15).await);
    assert_eq!(second.stop_code(), errors::PAIR_WINDOW_CLOSED);
    w.decide(false).unwrap();
}
