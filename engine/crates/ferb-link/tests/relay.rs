#![cfg(target_os = "macos")]
//! The own relay (its Test, the fallback when it is down), control under bulk on a rate-limited relay, and the
//! phone's redial behaviour. All on 127.0.0.1.
mod common;
use common::*;
use ferb_link::{
    client::Phone,
    errors,
    net::{RelaySpec, Shared},
};
use serde_json::{Value, json};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

// decision: PHN-210
#[tokio::test(flavor = "multi_thread")]
async fn the_relay_test_says_exactly_what_is_wrong() {
    let plain = Relay::start(None, None).await;
    let guarded = Relay::start(Some("s3cret-token"), None).await;
    let w = World::start(Shared::Custom(vec![plain.spec()]), None, true).await;
    let test = |url: String, token: Option<&'static str>| {
        let b = w.bridge.clone();
        async move {
            b.relay_test(&RelaySpec::new(url, token.map(str::to_string)))
                .await
        }
    };
    // the address itself
    for bad in [
        "relay.example.com",
        "ftp://relay.example.com",
        "http://relay.example.com",
        "https://",
    ] {
        assert_eq!(
            common::code(test(bad.into(), None).await),
            errors::OWN_RELAY_URL,
            "{bad}"
        );
    }
    // a name that does not exist
    let e = test("https://no-such-relay.invalid".into(), None)
        .await
        .unwrap_err();
    assert_eq!(e.code, errors::OWN_RELAY_DNS);
    assert!(
        e.message.contains("no-such-relay.invalid") && e.allow.is_some(),
        "{e}"
    );
    // nothing listening
    let closed = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().port()
    };
    let e = test(format!("http://localhost:{closed}"), None)
        .await
        .unwrap_err();
    assert_eq!(e.code, errors::OWN_RELAY_UNREACHABLE, "{e}");
    // a certificate nobody vouches for (self-signed)
    let (_certs, tls) = iroh_relay::server::testing::self_signed_tls_certs_and_config();
    let mut cfg = iroh_relay::server::ServerConfig::default();
    let mut r = iroh_relay::server::RelayConfig::new((std::net::Ipv4Addr::LOCALHOST, 0));
    r.tls = Some(iroh_relay::server::TlsConfig::new(
        (std::net::Ipv4Addr::LOCALHOST, 0),
        iroh_relay::server::CertConfig::Manual { server_config: tls },
    ));
    cfg.relay = Some(r);
    let tls_relay = iroh_relay::server::Server::spawn(cfg).await.unwrap();
    let e = test(
        format!(
            "https://localhost:{}",
            tls_relay.https_addr().unwrap().port()
        ),
        None,
    )
    .await
    .unwrap_err();
    assert_eq!(e.code, errors::OWN_RELAY_TLS, "{e}");
    assert!(e.message.contains("certificate"), "{e}");
    // the same relay with its own certificate pinned (a relay on a shared machine): accepted; a different pin: not
    use base64::Engine;
    let pin = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&_certs[0]);
    let mut pinned = RelaySpec::new(
        format!(
            "https://localhost:{}",
            tls_relay.https_addr().unwrap().port()
        ),
        None,
    );
    pinned.ca = Some(pin);
    let ok = w.bridge.relay_test(&pinned).await.unwrap();
    assert!(
        ok.steps.contains(&"certificate is valid".to_string()),
        "{ok:?}"
    );
    let link = pinned.to_link();
    assert!(link.starts_with("ferbrelay://localhost:"));
    assert_eq!(
        RelaySpec::from_text(&link, None).unwrap(),
        pinned,
        "the one-line relay link carries address, token and certificate"
    );
    let (other_certs, _) = iroh_relay::server::testing::self_signed_tls_certs_and_config();
    let mut wrong_pin = pinned.clone();
    wrong_pin.ca = Some(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&other_certs[0]));
    assert_eq!(
        common::code(w.bridge.relay_test(&wrong_pin).await.map(|_| ())),
        errors::OWN_RELAY_TLS
    );
    // and it works as the own relay end to end: the phone learns the certificate from the Mac
    w.bridge.relay_set(Some(pinned.clone())).await.unwrap();
    assert!(
        wait(20, || w.bridge.status()["relay"]["state"] == "own").await,
        "{}",
        w.bridge.status()["relay"]
    );
    let ph = w.paired_phone("pinned").await;
    assert!(
        wait(20, || ph.tier() == "own_relay").await,
        "{}",
        ph.phone.phase()
    );
    assert_eq!(ph.saved().unwrap().mac.unwrap().own, Some(pinned));
    w.remove(&ph.phone.id()).unwrap();
    w.bridge.relay_set(None).await.unwrap();
    // a web server that is not a relay
    let web = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let web_port = web.local_addr().unwrap().port();
    tokio::spawn(async move {
        while let Ok((mut s, _)) = web.accept().await {
            tokio::spawn(async move {
                let mut buf = [0u8; 2048];
                let _ = s.read(&mut buf).await;
                let _ = s
                    .write_all(
                        b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                    )
                    .await;
            });
        }
    });
    let e = test(format!("http://localhost:{web_port}"), None)
        .await
        .unwrap_err();
    assert_eq!(e.code, errors::OWN_RELAY_NOT_RELAY, "{e}");
    // a relay of another major version (its /healthz says so)
    let old = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let old_port = old.local_addr().unwrap().port();
    tokio::spawn(async move {
        while let Ok((mut s, _)) = old.accept().await {
            tokio::spawn(async move {
                let mut buf = [0u8; 2048];
                let _ = s.read(&mut buf).await;
                let body = r#"{"status":"ok","version":"0.35.0","git_hash":"unknown"}"#;
                let _ = s.write_all(format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes()).await;
            });
        }
    });
    let e = test(format!("http://localhost:{old_port}"), None)
        .await
        .unwrap_err();
    assert_eq!(e.code, errors::OWN_RELAY_VERSION, "{e}");
    assert!(e.message.contains("0.35.0"), "{e}");
    // a relay that wants a token: none, a wrong one, the right one
    let e = test(guarded.url(), None).await.unwrap_err();
    assert_eq!(e.code, errors::OWN_RELAY_ACCESS_DENIED, "{e}");
    assert!(e.allow.as_deref().unwrap().contains("token"));
    assert_eq!(
        common::code(test(guarded.url(), Some("wrong")).await),
        errors::OWN_RELAY_ACCESS_DENIED
    );
    let ok = test(guarded.url(), Some("s3cret-token")).await.unwrap();
    assert_eq!(ok.steps.last().unwrap(), "relay accepted this device");
    assert_eq!(ok.relay_version.as_deref(), Some("1.4.1"), "{ok:?}");
    println!("MEASURE relay test, all steps, local: {} ms", ok.ms);
    assert!(test(plain.url(), None).await.is_ok());
    // none of the failures was saved
    for bad in [format!("http://localhost:{web_port}"), guarded.url()] {
        assert!(
            w.bridge
                .relay_set(Some(RelaySpec::new(bad, None)))
                .await
                .is_err()
        );
    }
    assert!(w.bridge.status()["own_relay"].is_null());
}

/// The phone's own relay state as its last `relay_state` event said it.
fn phone_relay(p: &TestPhone) -> String {
    p.seen
        .lock()
        .unwrap()
        .events
        .iter()
        .rev()
        .find(|e| e["type"] == "relay_state")
        .and_then(|e| e["relay"]["state"].as_str().map(str::to_string))
        .unwrap_or_default()
}

// decision: PHN-208, PHN-425
/// A change of network or the app back in front, while the public relay stands in for the owner's: the phone probes
/// its relay at once and is back on it within seconds, not at the next `own_relay_probe_s` (here ten minutes).
#[tokio::test(flavor = "multi_thread")]
async fn a_network_change_or_the_app_in_front_probes_the_own_relay_at_once() {
    let shared = Relay::start(None, None).await;
    let mut own = Relay::start(Some("tok"), None).await;
    let w = World::start(Shared::Custom(vec![shared.spec()]), Some(own.spec()), true).await;
    w.limits_set(&json!({"own_relay_fallback_after_s": 3, "own_relay_probe_s": 600, "redial_steps_s": [1, 1, 2], "dial_deadline_s": 3})).unwrap();
    let p = w.paired_phone("phone").await;
    assert_eq!(p.tier(), "own_relay");
    assert_eq!(
        p.phone.limits().own_relay_probe_s,
        600,
        "the phone took the Mac's limits"
    );
    for (how, nudge) in [("network", true), ("in front", false)] {
        own.stop().await;
        assert!(
            wait(30, || phone_relay(&p) == "own_fallback").await,
            "{how}: the phone never fell back: {}",
            phone_relay(&p)
        );
        own.restart().await;
        // nobody nudges: the next probe is ten minutes away, the fallback stays
        tokio::time::sleep(Duration::from_secs(3)).await;
        assert_eq!(
            phone_relay(&p),
            "own_fallback",
            "{how}: probed without a nudge"
        );
        let t = Instant::now();
        if nudge {
            p.phone.network_changed_to(Some("cellular")).await;
        } else {
            p.phone.set_active(true);
        }
        assert!(
            wait(10, || matches!(
                phone_relay(&p).as_str(),
                "own" | "own_down"
            ))
            .await,
            "{how}: still {} after the nudge",
            phone_relay(&p)
        );
        println!("MEASURE {how} -> phone off the fallback: {:?}", t.elapsed());
        assert!(
            wait(20, || phone_relay(&p) == "own").await,
            "{how}: the phone's relay connection did not come back: {}",
            phone_relay(&p)
        );
    }
}

// decision: PHN-208, PHN-203, PHN-425, PHN-462, PHN-799
#[tokio::test(flavor = "multi_thread")]
async fn own_relay_down_falls_back_to_the_shared_relay_and_comes_back() {
    let shared = Relay::start(None, None).await;
    let mut own = Relay::start(Some("tok"), None).await;
    let w = World::start(Shared::Custom(vec![shared.spec()]), Some(own.spec()), true).await;
    w.limits_set(&json!({"own_relay_fallback_after_s": 3, "own_relay_probe_s": 2, "redial_steps_s": [1, 1, 2], "dial_deadline_s": 3})).unwrap();
    let p = w.paired_phone("phone").await;
    assert_eq!(p.tier(), "own_relay");
    assert_eq!(
        p.saved().unwrap().mac.unwrap().own,
        Some(own.spec()),
        "the QR carried the relay and its token"
    );
    assert!(wait(5, || w.bridge.status()["relay"]["state"] == "own").await);
    let mut admin = w.bridge.admin_events();

    // the owner's relay goes down
    let t = Instant::now();
    own.stop().await;
    assert!(
        wait(40, || w.bridge.status()["relay"]["state"] == "own_fallback").await,
        "{}",
        w.bridge.status()["relay"]
    );
    println!(
        "MEASURE own relay down -> Mac on the shared relays: {:?}",
        t.elapsed()
    );
    let reason = w.bridge.status()["relay"]["error"].clone();
    assert_eq!(
        reason["code"],
        errors::OWN_RELAY_UNREACHABLE,
        "the owner is told why: {reason}"
    );
    // the phone finds the Mac again through the shared relay, by itself, and says which tier it is on
    assert!(
        wait(60, || p.phase() == "online" && p.tier() == "shared_relay").await,
        "{}",
        p.phone.phase()
    );
    println!(
        "MEASURE own relay down -> phone working again on the shared relay: {:?}",
        t.elapsed()
    );
    // decision: PHN-418 (the owner's relay as the phone last learned it is in every phase it reported while the
    // Mac couldn't be reached, and on the public relay, not only in a welcome's)
    let url = own.spec().url;
    assert_eq!(
        p.phone.phase()["own_relay"],
        json!(url),
        "{}",
        p.phone.phase()
    );
    let away: Vec<Value> = p
        .seen
        .lock()
        .unwrap()
        .events
        .iter()
        .filter(|e| e["type"] == "state" && (e["phase"] == "connecting" || e["phase"] == "waiting"))
        .cloned()
        .collect();
    assert!(
        !away.is_empty(),
        "the phone never reported a phase away from the Mac"
    );
    assert!(
        away.iter().all(|e| e["own_relay"] == json!(url)),
        "{away:?}"
    );
    assert!(
        p.call("chat.status", json!({})).await.is_ok(),
        "the basics work during the outage"
    );
    let e = p
        .phone
        .get(
            json!({"what": "preview", "task_id": "x", "live": true}),
            |_, _| {},
        )
        .await
        .unwrap_err();
    assert_eq!(
        e.code,
        errors::NEEDS_BETTER_PATH,
        "the shared-relay caps apply during the fallback"
    );
    let mut told = false;
    while let Ok(e) = admin.try_recv() {
        told |= e["event"] == "relay" && e["relay"]["state"]["state"] == "own_fallback";
    }
    assert!(told, "the owner's side gets an event for the fallback");

    // it comes back
    let t = Instant::now();
    own.restart().await;
    assert!(
        wait(40, || w.bridge.status()["relay"]["state"] == "own").await,
        "{}",
        w.bridge.status()["relay"]
    );
    assert!(
        wait(60, || p.phase() == "online" && p.tier() == "own_relay").await,
        "{}",
        p.phone.phase()
    );
    // the new session proved the phone before it went online, so the unlock carried over before any call: the
    // `chat.send` below never meets a locked Mac, however late this test's thread runs (kb PHN-799)
    assert_eq!(p.phone.phase()["proved"], true, "{}", p.phone.phase());
    // decision: PHN-425 (the app heard the phone's own fall back to the public relay and its return, as events)
    let states: Vec<String> = p
        .seen
        .lock()
        .unwrap()
        .events
        .iter()
        .filter(|e| e["type"] == "relay_state")
        .filter_map(|e| e["relay"]["state"].as_str().map(str::to_string))
        .collect();
    let fell = states.iter().position(|s| s == "own_fallback");
    assert!(
        fell.is_some_and(|i| states[i..].iter().any(|s| s == "own" || s == "own_down")),
        "relay_state events: {states:?}"
    );
    println!(
        "MEASURE own relay back -> phone on the own relay again: {:?}",
        t.elapsed()
    );
    let (_, b) = {
        p.phone
            .call("chat.send", json!({"text": "file 3000000"}), None, |_| {})
            .await
            .unwrap();
        let st = p.call("chat.status", json!({})).await.unwrap();
        let last = st["results"].as_array().unwrap().last().unwrap()["task_id"]
            .as_str()
            .unwrap()
            .to_string();
        p.phone
            .get(
                json!({"what": "file", "task_id": last, "index": 0}),
                |_, _| {},
            )
            .await
            .unwrap()
    };
    assert_eq!(b.len(), 3_000_000);

    // fallback switched off: the Mac stays on the own relay setting and says it is down
    w.limits_set(&json!({"own_relay_fallback": false})).unwrap();
    own.stop().await;
    assert!(
        wait(30, || w.bridge.status()["relay"]["state"] == "own_down"
            && !w.bridge.status()["relay"]["error"].is_null())
        .await
    );
    tokio::time::sleep(Duration::from_secs(5)).await;
    assert_eq!(w.bridge.status()["relay"]["state"], "own_down");
    assert_eq!(
        common::code(w.bridge.pair_start()),
        errors::OWN_RELAY_UNREACHABLE
    );
}

// decision: PHN-304, PHN-310
#[tokio::test(flavor = "multi_thread")]
async fn control_stays_fast_while_bulk_crosses_a_slow_relay() {
    // a relay that gives each client 64 kB/s, as a throttled public relay would
    // (the session's relay is the owner's: timed start-overs and bulk are not for the public relays, kb PHN-445, PHN-444)
    let public = Relay::start(None, None).await;
    let slow = Relay::start(None, Some(64_000)).await;
    let w = World::start(Shared::Custom(vec![public.spec()]), Some(slow.spec()), true).await;
    w.limits_set(&json!({"shared_bulk_burst": 4_000_000, "ratelimit_pause_s": 0}))
        .unwrap();
    let p = w.paired_phone("phone").await;
    let mut base = vec![];
    for _ in 0..10 {
        let t = Instant::now();
        p.call("ping", json!({})).await.unwrap();
        base.push(t.elapsed().as_secs_f64() * 1000.0);
    }
    println!(
        "MEASURE control round trip, idle, 64 kB/s relay: p50 {:.1} ms",
        pct(&mut base, 0.5)
    );
    for window in [4u32, 2, 1] {
        w.limits_set(&json!({"good_window": window})).unwrap();
        p.phone
            .call("chat.send", json!({"text": "file 400000"}), None, |_| {})
            .await
            .unwrap();
        let st = p.call("chat.status", json!({})).await.unwrap();
        let last = st["results"].as_array().unwrap().last().unwrap()["task_id"]
            .as_str()
            .unwrap()
            .to_string();
        // a long task to stop in the middle of the transfer
        let ph = p.phone.clone();
        let long = tokio::spawn(async move {
            ph.call("chat.send", json!({"text": "long 60"}), None, |_| {})
                .await
        });
        tokio::time::sleep(Duration::from_millis(1700)).await;
        let running =
            p.call("chat.status", json!({})).await.unwrap()["running"][0]["task_id"].clone();
        let ph = p.phone.clone();
        let t = Instant::now();
        let bulk = tokio::spawn(async move {
            ph.get(
                json!({"what": "file", "task_id": last, "index": 0}),
                |_, _| {},
            )
            .await
        });
        let mut rtts = vec![];
        let mut stop_ms = None;
        while !bulk.is_finished() {
            let c = Instant::now();
            if rtts.len() == 6 {
                p.call("task.stop", json!({"task_ids": [running]}))
                    .await
                    .unwrap();
                stop_ms = Some(c.elapsed().as_secs_f64() * 1000.0);
            } else {
                p.call("ping", json!({})).await.unwrap();
            }
            rtts.push(c.elapsed().as_secs_f64() * 1000.0);
            tokio::time::sleep(Duration::from_millis(150)).await;
        }
        let (_, bytes) = bulk.await.unwrap().unwrap();
        long.await.unwrap().unwrap();
        assert_eq!(bytes.len(), 400_000);
        let (p50, max) = (pct(&mut rtts, 0.5), pct(&mut rtts, 1.0));
        println!(
            "MEASURE window {window}: 400 kB in {:.1} s; control during it: p50 {p50:.0} ms, max {max:.0} ms ({} calls); Stop took {:.0} ms",
            t.elapsed().as_secs_f64(),
            rtts.len(),
            stop_ms.unwrap_or(f64::NAN)
        );
        if window == 1 {
            assert!(
                p50 < 400.0 && max < 1500.0,
                "control starved behind bulk: p50 {p50} max {max}"
            );
        }
    }
    let st = w.bridge.status();
    println!(
        "MEASURE relay rate-limit notices seen by the Mac: {}",
        st["counters"]["ratelimited"]
    );
}

// decision: PHN-307, PHN-444
#[tokio::test(flavor = "multi_thread")]
async fn the_relays_slow_down_notice_is_told_and_control_goes_on() {
    let slow = Relay::start(None, Some(64_000)).await;
    let w = World::start(Shared::Custom(vec![slow.spec()]), None, true).await;
    let p = w.paired_phone("phone").await;
    // no bulk crosses the public relays (kb PHN-444): long replies, control frames each, make the relay say it
    for _ in 0..12 {
        p.phone
            .call("chat.send", json!({"text": "big 60000"}), None, |_| {})
            .await
            .unwrap();
        if w.bridge.status()["counters"]["ratelimited"]
            .as_u64()
            .unwrap_or(0)
            > 0
        {
            break;
        }
    }
    assert!(
        wait(10, || w.bridge.status()["counters"]["ratelimited"]
            .as_u64()
            .unwrap_or(0)
            > 0)
        .await,
        "the relay sent no notice: {}",
        w.bridge.status()["counters"]
    );
    assert!(
        wait(5, || !w.bridge.status()["shared_relay"]["paused_s"]
            .is_null())
        .await
    );
    assert!(
        p.call("chat.status", json!({})).await.is_ok(),
        "control is not paused"
    );
    assert!(
        wait(5, || p.seen.lock().unwrap().events.iter().any(
            |e| e["type"] == "notice"
                && e["error"]["code"] == errors::RELAY_RATE_LIMITED
                && e["error"]["message"]
                    .as_str()
                    .is_some_and(|m| m.contains("Messages and questions still work"))
        ))
        .await,
        "the phone is told"
    );
}

// decision: PHN-2, PHN-302
#[tokio::test(flavor = "multi_thread")]
async fn the_phone_backs_off_and_stops_when_it_should() {
    let relay = Relay::start(None, None).await;
    let w = World::start(Shared::Custom(vec![relay.spec()]), None, true).await;
    w.limits_set(
        &json!({"redial_steps_s": [1, 2, 4], "dial_deadline_s": 2, "redial_jitter_pct": 20}),
    )
    .unwrap();
    let p = w.paired_phone("phone").await;
    let state = p.saved();

    // a phone app built for a newer link than this Mac speaks: told to update FERB, and it stops
    let newer = TestPhone::start_with("phone", &w.shared, true, state.clone(), true, |c| {
        c.version = (3, 3)
    })
    .await;
    p.phone.close().await;
    assert!(
        newer.wait_phase("stopped", 15).await,
        "{}",
        newer.phone.phase()
    );
    assert_eq!(newer.stop_code(), errors::VERSION_MISMATCH);
    assert!(
        newer.phone.phase()["error"]["allow"]
            .as_str()
            .unwrap()
            .contains("Update FERB on the Mac"),
        "{}",
        newer.phone.phase()
    );
    let n = newer.phone.dial_times().len();
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert_eq!(
        newer.phone.dial_times().len(),
        n,
        "no redial after a version mismatch"
    );
    newer.phone.close().await;
    // the app from before link version 2 (no sealed answers): told to update the app, clearly, and it stops
    let older = TestPhone::start_with("phone", &w.shared, true, state.clone(), true, |c| {
        c.version = (1, 1)
    })
    .await;
    assert!(
        older.wait_phase("stopped", 15).await,
        "{}",
        older.phone.phase()
    );
    assert_eq!(older.stop_code(), errors::VERSION_MISMATCH);
    assert!(
        older.phone.phase()["error"]["allow"]
            .as_str()
            .unwrap()
            .contains("Update the phone app"),
        "{}",
        older.phone.phase()
    );
    assert!(
        older
            .saved()
            .is_none_or(|s| s.mac.is_some_and(|m| m.paired)),
        "a version mismatch does not unpair the phone"
    );
    older.phone.close().await;

    // the Mac goes away (asleep, offline, FERB quit): the phone says so and backs off
    let p = TestPhone::start("phone", &w.shared, true, state.clone(), true).await;
    assert!(p.wait_phase("online", 15).await);
    let dials_before = p.phone.dial_times().len();
    w.bridge.close().await;
    assert!(
        wait(40, || p.phase() == "waiting").await,
        "{}",
        p.phone.phase()
    );
    assert_eq!(p.stop_code(), errors::MAC_UNREACHABLE);
    assert!(p.phone.phase()["retry_in_s"].as_u64().is_some());
    let e = p.call("chat.status", json!({})).await.unwrap_err();
    assert_eq!(e.code, errors::MAC_UNREACHABLE);
    assert!(e.message.contains("asleep") && e.allow.is_some(), "{e}");
    tokio::time::sleep(Duration::from_secs(22)).await;
    let times = p.phone.dial_times();
    let gaps: Vec<f64> = times[dials_before..]
        .windows(2)
        .map(|w| (w[1] - w[0]).as_secs_f64())
        .collect();
    println!(
        "MEASURE redial gaps with the Mac gone (steps 1,2,4 s, 2 s dial deadline, 20 % jitter): {gaps:.1?}"
    );
    assert!(gaps.len() <= 8, "too many dials in 22 s: {gaps:?}");
    assert!(
        gaps.iter().skip(2).all(|g| *g >= 3.0),
        "later gaps must hold the last step: {gaps:?}"
    );

    // in the background nothing dials; back in front it dials at once
    p.phone.set_active(false);
    let n = p.phone.dial_times().len();
    tokio::time::sleep(Duration::from_secs(6)).await;
    assert_eq!(p.phone.dial_times().len(), n, "no dial in the background");

    // the Mac is back (same state folder): the phone reconnects by itself, still paired
    let bridge = ferb_link::bridge::Bridge::start(ferb_link::bridge::BridgeConfig {
        state_dir: w.dir.join("mac"),
        daemon_socket: w.standin.socket.clone(),
        shared: w.shared.clone(),
        roots: None,
        force_relay: true,
        keys: ferb_link::secrets::Keys::Files,
        accept_state: false,
        portmapper: false,
        loopback_only: true,
    })
    .await
    .unwrap();
    assert_eq!(bridge.id(), w.bridge.id(), "the Mac keeps its identity");
    p.phone.set_active(true);
    assert!(p.wait_phase("online", 30).await, "{}", p.phone.phase());
    assert!(p.call("ping", json!({})).await.is_ok());

    // FERB's daemon not running: the link is up and says exactly that
    drop(w.standin);
    tokio::time::sleep(Duration::from_millis(300)).await;
    let e = p.call("chat.status", json!({})).await.unwrap_err();
    assert_eq!(e.code, errors::FERB_NOT_RUNNING, "{e}");
    assert!(
        wait(10, || p.seen.lock().unwrap().events.iter().any(
            |e| e["type"] == "notice" && e["error"]["code"] == errors::FERB_NOT_RUNNING
        ))
        .await
    );

    // a phone that cannot reach any relay says that, not "the Mac is asleep"
    let closed = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().port()
    };
    let dead = Shared::Custom(vec![RelaySpec::new(
        format!("http://localhost:{closed}"),
        None,
    )]);
    let mut state = state.unwrap();
    state.mac.as_mut().unwrap().relays = vec![format!("http://localhost:{closed}")];
    let lost = TestPhone::start("phone", &dead, true, Some(state), true).await;
    assert!(
        wait(30, || lost.phase() == "waiting").await,
        "{}",
        lost.phone.phase()
    );
    assert_eq!(lost.stop_code(), errors::RELAY_UNREACHABLE);
}

/// By hand (`scripts/test.sh relay trying -- --ignored` is not wired; run the test binary with --ignored): what
/// "trying for a direct connection" costs while a session sits on a relay and no direct path can be made. Here the
/// Mac has no IP transports at all (the test switch), so every attempt fails: the worst case for retries.
// decision: PHN-218
#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn measure_what_trying_for_direct_costs_on_a_relay() {
    let relay = Relay::start(None, None).await;
    let w = World::start(Shared::Custom(vec![relay.spec()]), None, true).await;
    let p = w.paired_phone("phone").await;
    assert!(wait(20, || p.tier() == "shared_relay").await);
    tokio::time::sleep(Duration::from_secs(5)).await;
    let snap = |ep: &iroh::Endpoint| {
        let m = &ep.metrics().socket;
        [
            m.holepunch_attempts.get(),
            m.send_relay.get(),
            m.recv_data_relay.get(),
            m.send_ipv4.get() + m.send_ipv6.get(),
        ]
    };
    let (mac0, phone0) = (snap(&w.bridge.endpoint()), snap(&p.phone.endpoint()));
    let secs = 120;
    tokio::time::sleep(Duration::from_secs(secs)).await;
    let (mac1, phone1) = (snap(&w.bridge.endpoint()), snap(&p.phone.endpoint()));
    let per_min = |a: [u64; 4], b: [u64; 4]| {
        (0..4)
            .map(|i| (b[i] - a[i]) as f64 * 60.0 / secs as f64)
            .collect::<Vec<_>>()
    };
    println!(
        "MEASURE idle session on a relay, direct impossible, per minute [attempts started, bytes sent via relay, bytes received via relay, bytes sent on IP]: Mac {:?}, phone {:?}",
        per_min(mac0, mac1),
        per_min(phone0, phone1)
    );
}

// decision: PHN-224, PHN-445
#[tokio::test(flavor = "multi_thread")]
async fn a_session_on_a_relay_starts_over_on_a_schedule_that_backs_off_stops_and_begins_again() {
    // the schedule itself, with the owner's 30 s: every 30 s for five minutes, then every five minutes, to one hour
    // (the session's relay is the owner's: timed start-overs and bulk are not for the public relays, kb PHN-445, PHN-444)
    let public = Relay::start(None, None).await;
    let l = ferb_link::limits::Limits::default();
    let mut at = 0;
    let mut all = vec![];
    while let Some(n) = Phone::next_start_over(&l, at) {
        all.push(n);
        at = n;
    }
    assert_eq!(
        &all[..12],
        &[30, 60, 90, 120, 150, 180, 210, 240, 270, 300, 600, 900],
        "{all:?}"
    );
    assert_eq!((all.len(), *all.last().unwrap()), (21, 3600), "{all:?}");
    let never = ferb_link::limits::Limits {
        handover_retry_every_s: 0,
        ..l.clone()
    };
    assert_eq!(Phone::next_start_over(&never, 0), None);

    let relay = Relay::start(None, None).await;
    // a session that cannot go direct (the Mac's test switch says so itself, so nothing is started over) ...
    let w0 = World::start(
        Shared::Custom(vec![public.spec()]),
        Some(relay.spec()),
        true,
    )
    .await;
    w0.bridge
        .limits_set(&json!({"handover_retry_every_s": 1}))
        .unwrap();
    let hopeless = w0.paired_phone("hopeless").await;
    assert!(
        wait(20, || hopeless.tier() == "own_relay"
            && hopeless.phone.phase()["direct"]["trying"] == json!(false))
        .await,
        "{}",
        hopeless.phone.phase()
    );
    hopeless.phone.network_changed().await;
    tokio::time::sleep(Duration::from_secs(4)).await;
    assert_eq!(
        (
            hopeless.phone.fresh_starts(),
            hopeless.phone.failed_starts()
        ),
        (0, 0),
        "a session whose Mac says no direct path can be had was started over"
    );

    // ... and one that stays on the relay although a direct path is thought possible (what a phone behind a filter
    // looks like): the phone has no IP transports. (`start_over_without_hope`: on these test relays nobody ever
    // learns that UDP works, so the Mac says "no direct path can be had" in every relayed session here)
    let w = World::start(
        Shared::Custom(vec![public.spec()]),
        Some(relay.spec()),
        false,
    )
    .await;
    w.limits_set(&json!({"handover_retry_every_s": 2, "handover_retry_fast_for_s": 6, "handover_retry_slow_every_s": 4, "handover_retry_stop_after_s": 16})).unwrap();
    let p = TestPhone::start_with("phone", &w.shared, true, None, true, |c| {
        c.start_over_without_hope = true
    })
    .await;
    w.pair(&p).await;
    p.unlock().await.unwrap();
    assert!(wait(20, || p.tier() == "own_relay").await);
    assert_eq!(
        p.phone.limits().handover_retry_every_s,
        2,
        "the phone did not learn the setting"
    );
    // nothing is started over before something happens (the app's own start is the opening of the app: asked below)
    tokio::time::sleep(Duration::from_secs(4)).await;
    assert_eq!(p.phone.fresh_starts(), 0);
    let (id, dials) = (p.phone.id(), p.phone.dial_times().len());
    let t = Instant::now();
    p.phone.network_changed().await;
    // at 2, 4, 6 s, then every 4 s (10, 14), then nothing more (18 is past the stop)
    assert!(
        wait(12, || p.phone.fresh_starts() == 3).await,
        "{} start-overs in the fast stretch",
        p.phone.fresh_starts()
    );
    let fast = t.elapsed().as_secs_f64();
    assert!(
        (5.5..9.5).contains(&fast),
        "three start-overs took {fast:.1} s"
    );
    assert!(
        wait(14, || p.phone.fresh_starts() == 5).await,
        "{} start-overs with the slower pace",
        p.phone.fresh_starts()
    );
    let slow = t.elapsed().as_secs_f64();
    assert!(
        slow > 13.0,
        "the slower pace was not slower: five start-overs in {slow:.1} s"
    );
    tokio::time::sleep(Duration::from_secs(8)).await;
    assert_eq!(p.phone.fresh_starts(), 5, "the schedule did not stop");
    assert_eq!(p.phone.start_over_due(), None);
    println!(
        "MEASURE start-overs after one change of network (2 s pace for 6 s, then 4 s, stop at 16 s): 3 in {fast:.1} s, 5 in {slow:.1} s, then none"
    );
    let cost = p.phone.last_start()["cost"].clone();
    println!(
        "MEASURE what one start-over costs on an idle session, 127.0.0.1, one relay without address discovery: the new endpoint's first 3 s sent {} bytes and received {} (QUIC's own bytes, before the relay's TLS: the handshake with the Mac, the hello and welcome, the status, the proof of presence)",
        cost["tx"], cost["rx"]
    );
    // the same identity, the same paired phone, no dial, and the session works
    assert_eq!(
        p.phone.id(),
        id,
        "the phone's identity changed: it would have to pair again"
    );
    assert_eq!(
        (w.bridge.devices().len(), p.phone.dial_times().len()),
        (1, dials)
    );
    p.call("tasks.list", json!({}))
        .await
        .expect("the session works after the start-overs");
    // a new change of network begins the schedule again; several in a row (Android reports several) are one
    p.phone.network_changed().await;
    p.phone.network_changed().await;
    assert!(
        wait(6, || p.phone.fresh_starts() == 6).await,
        "no start-over after a new change of network"
    );
    // and so does opening the app (coming back to it many times does not push the next try away)
    assert!(wait(20, || p.phone.start_over_due().is_none()).await);
    let before = p.phone.fresh_starts();
    for _ in 0..5 {
        p.phone.set_active(true);
        tokio::time::sleep(Duration::from_millis(600)).await;
    }
    assert!(
        wait(3, || p.phone.fresh_starts() > before).await,
        "no start-over after the app was opened"
    );

    // a session that is direct is never touched, whatever happens
    let w2 = World::start(
        Shared::Custom(vec![public.spec()]),
        Some(relay.spec()),
        false,
    )
    .await;
    w2.bridge
        .limits_set(&json!({"handover_retry_every_s": 1}))
        .unwrap();
    let d = TestPhone::start_with("direct", &w2.shared, false, None, true, |c| {
        c.start_over_without_hope = true
    })
    .await;
    w2.pair(&d).await;
    assert!(wait(20, || d.tier() == "direct").await);
    let (conn, ports) = (
        d.phone.connection().unwrap().stable_id(),
        ferb_link::net::local_ports(&d.phone.endpoint()),
    );
    d.phone.network_changed().await;
    d.phone.set_active(true);
    tokio::time::sleep(Duration::from_secs(5)).await;
    assert_eq!(
        (d.phone.fresh_starts(), d.phone.failed_starts()),
        (0, 0),
        "a direct session was started over"
    );
    assert_eq!(
        d.phone.start_over_due(),
        None,
        "the schedule goes on while the session is direct"
    );
    assert_eq!(
        (
            d.phone.connection().unwrap().stable_id(),
            ferb_link::net::local_ports(&d.phone.endpoint())
        ),
        (conn, ports),
        "a direct session's connection or sockets changed"
    );
}

// decision: PHN-225
#[tokio::test(flavor = "multi_thread")]
async fn a_start_over_costs_the_session_nothing() {
    // (the session's relay is the owner's: timed start-overs and bulk are not for the public relays, kb PHN-445, PHN-444)
    let public = Relay::start(None, None).await;
    let relay = Relay::start(None, None).await;
    let w = World::start(
        Shared::Custom(vec![public.spec()]),
        Some(relay.spec()),
        false,
    )
    .await;
    w.limits_set(&json!({"handover_retry_every_s": 3, "handover_retry_fast_for_s": 600, "calls_per_10s": 100_000})).unwrap();
    let p = TestPhone::start_with("phone", &w.shared, true, None, true, |c| {
        c.start_over_without_hope = true
    })
    .await;
    w.pair(&p).await;
    p.unlock().await.unwrap();
    assert!(wait(20, || p.tier() == "own_relay").await);
    // a question waits on the phone (asked by FERB itself: a task this phone started would hold its call open, and
    // nothing is started over while a call is under way), and the phone is unlocked towards the Mac
    w.standin
        .cli("standin.ask", json!({"reason": "shall I go on?"}))
        .await;
    assert!(
        wait(10, || p.status()["approvals"]
            .as_array()
            .is_some_and(|a| a.len() == 1))
        .await,
        "{}",
        p.status()
    );
    let question = p.status()["approvals"][0]["approval_id"]
        .as_str()
        .unwrap()
        .to_string();
    let events_before = p.seen.lock().unwrap().events.len();
    let mut admin = w.bridge.admin_events();
    let ports = ferb_link::net::local_ports(&p.phone.endpoint());

    // calls all the while, one after the other: none may fail, and the Mac must say "connected" throughout
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let caller = {
        let (ph, stop, bridge) = (p.phone.clone(), stop.clone(), w.bridge.clone());
        tokio::spawn(async move {
            let (mut n, mut failed, mut slowest, mut offline) = (0u32, vec![], 0f64, 0u32);
            while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                let t = Instant::now();
                // (a call that needs the unlock window: it fails with `locked` if the unlock did not carry over)
                if let Err(e) = ph.call("tasks.list", json!({}), None, |_| {}).await {
                    failed.push(e.code);
                }
                slowest = slowest.max(t.elapsed().as_secs_f64() * 1000.0);
                offline += (bridge.status()["devices"][0]["online"] != json!(true)) as u32;
                n += 1;
                tokio::time::sleep(Duration::from_millis(15)).await;
            }
            (n, failed, slowest, offline)
        })
    };
    p.phone.network_changed().await;
    assert!(
        wait(20, || p.phone.fresh_starts() == 4).await,
        "{} start-overs",
        p.phone.fresh_starts()
    );
    let last = p.phone.last_start();
    tokio::time::sleep(Duration::from_millis(500)).await;
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    let (calls, failed, slowest, offline) = caller.await.unwrap();
    assert!(
        failed.is_empty(),
        "{} of {calls} calls failed across the start-overs: {failed:?}",
        failed.len()
    );
    assert_eq!(
        offline, 0,
        "the Mac showed the phone as not connected during a start-over"
    );
    assert_eq!(p.phone.failed_starts(), 0);
    // the phone never left "online", and the app was not asked to unlock again
    let phases: Vec<String> = p.seen.lock().unwrap().events[events_before..]
        .iter()
        .filter(|e| e["type"] == "state")
        .map(|e| e["phase"].as_str().unwrap_or("?").to_string())
        .collect();
    assert!(
        phases.iter().all(|ph| ph == "online"),
        "the phone's state left online: {phases:?}"
    );
    // the Mac never said the phone went away
    while let Ok(e) = admin.try_recv() {
        assert_ne!(
            e["event"], "phone_offline",
            "the Mac said the phone disconnected"
        );
    }
    // new sockets each time, the presence proven before the move, the same identity
    assert_eq!(
        (&last["moved"], &last["ports_changed"], &last["proved"]),
        (&json!(true), &json!(true), &json!(true)),
        "{last}"
    );
    assert!(
        ferb_link::net::local_ports(&p.phone.endpoint())
            .iter()
            .all(|x| !ports.contains(x))
    );
    // the question is still there and can be answered on the new connection; nothing was asked twice
    assert!(
        wait(5, || p.approval(&question).is_some()).await,
        "the question was lost: {}",
        p.status()
    );
    assert_eq!(p.status()["approvals"].as_array().unwrap().len(), 1);
    p.call(
        "chat.approve",
        json!({"approval_id": question, "granted": true}),
    )
    .await
    .expect("the answer after a start-over");
    // the Mac holds one connection for the phone and has the record of the start-overs (kinds, counts, times)
    assert!(
        wait(10, || w.bridge.status()["devices"][0]["handover"]["tries"]
            .as_u64()
            >= Some(4))
        .await,
        "{}",
        w.bridge.status()["devices"][0]
    );
    let dev = w.bridge.status()["devices"][0].clone();
    assert_eq!(
        (&dev["handover"]["moved"], &dev["online"]),
        (&json!(4), &json!(true)),
        "{dev}"
    );
    let lines: Vec<Value> = w
        .bridge
        .audit(100_000)
        .into_iter()
        .filter(|a| a["method"] == "link.startover")
        .collect();
    assert!(
        lines.len() >= 4 && lines.iter().all(|a| a["result"] == "ok"),
        "{lines:?}"
    );
    let text = lines
        .iter()
        .map(|a| format!("{} {}", a["words"], a["facts"]))
        .collect::<String>();
    assert!(
        !text.contains(&p.phone.id()) && !text.contains("127.0.0.1") && !text.contains("localhost"),
        "the record names an address or an id: {text}"
    );
    let cost = &w.bridge.status()["devices"][0]["handover"]["last"];
    let _ = cost;
    println!(
        "MEASURE one start-over on 127.0.0.1 (one relay): the session moved in {} ms, calls were held {} ms; slowest of {calls} calls across four start-overs {slowest:.0} ms",
        last["took_ms"], last["held_ms"]
    );
    println!(
        "MEASURE as the Mac's Activity list says it: {}",
        lines.last().unwrap()["words"]
    );
}

// decision: PHN-225, PHN-126
#[tokio::test(flavor = "multi_thread")]
async fn the_unlock_moves_with_the_session_only_on_the_device_keys_signature() {
    // (the session's relay is the owner's: timed start-overs and bulk are not for the public relays, kb PHN-445, PHN-444)
    let public = Relay::start(None, None).await;
    let relay = Relay::start(None, None).await;
    let w = World::start(
        Shared::Custom(vec![public.spec()]),
        Some(relay.spec()),
        false,
    )
    .await;
    // (two start-overs, at 2 and 4 s, then none)
    w.limits_set(&json!({"handover_retry_every_s": 2, "handover_retry_fast_for_s": 4, "handover_retry_slow_every_s": 0})).unwrap();
    // a phone as the Android app is: the device key is the app's, the link asks the app for each signature
    let key: Arc<std::sync::Mutex<Option<ferb_link::client::DeviceSign>>> = Default::default();
    let k = key.clone();
    let p = TestPhone::start_with("phone", &w.shared, true, None, true, move |c| {
        c.start_over_without_hope = true;
        *k.lock().unwrap() = c.device_sign.take();
    })
    .await;
    let sign = key.lock().unwrap().clone().unwrap();
    w.pair(&p).await;
    // as the app does when a session begins: this connection is this phone; then the owner unlocks
    p.phone.present(|payload| sign(payload)).await.unwrap();
    p.unlock().await.unwrap();
    assert!(wait(20, || p.tier() == "own_relay").await);
    p.call("tasks.list", json!({})).await.expect("unlocked");

    // 1. the app answers the link's `prove` event with the device key's signature: the unlock is there at once
    let answering = Arc::new(std::sync::atomic::AtomicBool::new(true));
    let app = {
        let (seen, ph, sign, answering) = (
            p.seen.clone(),
            p.phone.clone(),
            sign.clone(),
            answering.clone(),
        );
        tokio::spawn(async move {
            let mut done = 0;
            loop {
                let asked: Vec<Value> = seen
                    .lock()
                    .unwrap()
                    .events
                    .iter()
                    .filter(|e| e["type"] == "prove")
                    .skip(done)
                    .cloned()
                    .collect();
                for e in asked {
                    done += 1;
                    if answering.load(std::sync::atomic::Ordering::Relaxed) {
                        // (the app signs only a `link.present` text)
                        let payload = e["payload"].as_str().unwrap();
                        assert_eq!(
                            serde_json::from_str::<Value>(payload).unwrap()["method"],
                            "link.present"
                        );
                        ph.proof(e["id"].as_u64().unwrap(), sign(payload));
                    }
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
    };
    p.phone.network_changed().await;
    assert!(wait(8, || p.phone.fresh_starts() == 1).await);
    let first = p.phone.last_start();
    assert_eq!(
        (&first["moved"], &first["proved"]),
        (&json!(true), &json!(true)),
        "{first}"
    );
    assert_eq!(p.phone.phase()["proved"], true);
    p.call("tasks.list", json!({}))
        .await
        .expect("the unlock moved with the session");

    // 2. no signature comes (an app that does not answer, or a phone whose key is gone): the session moves all the
    //    same, but the new connection is locked, as any new connection is, until the phone proves itself or unlocks
    answering.store(false, std::sync::atomic::Ordering::Relaxed);
    assert!(
        wait(12, || p.phone.fresh_starts() == 2).await,
        "{}",
        p.phone.last_start()
    );
    let second = p.phone.last_start();
    assert_eq!(
        (&second["moved"], &second["proved"]),
        (&json!(true), &json!(false)),
        "{second}"
    );
    assert_eq!(
        p.phone.phase()["proved"],
        false,
        "the app is not told to prove the phone itself"
    );
    assert_eq!(p.phone.start_over_due(), None);
    assert_eq!(
        common::code(p.call("tasks.list", json!({})).await),
        errors::LOCKED,
        "a connection that proved nothing got the phone's unlock"
    );
    // a wrong key's signature opens nothing either
    let other = p256::ecdsa::SigningKey::random(&mut p256::elliptic_curve::rand_core::OsRng);
    let _ = p
        .phone
        .present(|payload| {
            use p256::ecdsa::signature::Signer;
            let s: p256::ecdsa::Signature = other.sign(payload.as_bytes());
            Some(b64(s.to_der().as_bytes()))
        })
        .await;
    assert_eq!(
        common::code(p.call("tasks.list", json!({})).await),
        errors::LOCKED,
        "another key's signature carried the unlock over"
    );
    // the phone's own does (what the app does when it sees a new session that has not proven itself)
    p.phone.present(|payload| sign(payload)).await.unwrap();
    p.call("tasks.list", json!({}))
        .await
        .expect("the unlock carried over after the phone proved itself");
    println!(
        "MEASURE a start-over whose proof the app signs: calls held {} ms; one where no signature comes: {} ms",
        first["held_ms"], second["held_ms"]
    );
    app.abort();
}

// decision: PHN-225, PHN-445
#[tokio::test(flavor = "multi_thread")]
async fn a_start_over_waits_for_a_file_and_one_that_does_not_get_through_changes_nothing() {
    // a relay that gives each client 64 kB/s, so a file takes seconds
    // (the session's relay is the owner's: timed start-overs and bulk are not for the public relays, kb PHN-445, PHN-444)
    let public = Relay::start(None, None).await;
    let slow = Relay::start(None, Some(64_000)).await;
    let w = World::start(
        Shared::Custom(vec![public.spec()]),
        Some(slow.spec()),
        false,
    )
    .await;
    // (the Mac lets one phone connect once a minute here: the second start-over is turned away at the door)
    w.limits_set(&json!({"handover_retry_every_s": 1, "handover_retry_fast_for_s": 600, "shared_bulk_burst": 4_000_000, "ratelimit_pause_s": 0, "connects_per_min": 1})).unwrap();
    let p = TestPhone::start_with("phone", &w.shared, true, None, true, |c| {
        c.start_over_without_hope = true
    })
    .await;
    w.pair(&p).await;
    p.unlock().await.unwrap();
    assert!(wait(20, || p.tier() == "own_relay").await);
    p.phone
        .call("chat.send", json!({"text": "file 300000"}), None, |_| {})
        .await
        .unwrap();
    let st = p.call("chat.status", json!({})).await.unwrap();
    let task = st["results"].as_array().unwrap().last().unwrap()["task_id"]
        .as_str()
        .unwrap()
        .to_string();
    let ph = p.phone.clone();
    let t = Instant::now();
    let file = tokio::spawn(async move {
        ph.get(
            json!({"what": "file", "task_id": task, "index": 0}),
            |_, _| {},
        )
        .await
    });
    tokio::time::sleep(Duration::from_millis(300)).await;
    p.phone.network_changed().await;
    let mut during = 0;
    while !file.is_finished() {
        during = during.max(p.phone.fresh_starts() + p.phone.failed_starts());
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let took = t.elapsed().as_secs_f64();
    let (_, bytes) = file.await.unwrap().expect("the file arrived whole");
    assert_eq!(bytes.len(), 300_000);
    assert!(
        took > 2.5,
        "the file was too fast to prove anything: {took:.1} s"
    );
    assert_eq!(
        during, 0,
        "the session was started over while a file travelled"
    );
    // due all that time, it happens once the file is through
    assert!(
        wait(4, || p.phone.fresh_starts() == 1).await,
        "no start-over after the file"
    );
    println!(
        "MEASURE a start-over due while a 300 kB file crossed a 64 kB/s relay ({took:.1} s): none during it, one {:.1} s after it began",
        t.elapsed().as_secs_f64()
    );
    // the next one is turned away by the Mac (too many connections): the session stays as it is and works
    assert!(
        wait(6, || p.phone.failed_starts() >= 1).await,
        "{}",
        p.phone.last_start()
    );
    assert_eq!(
        p.phone.last_start()["reason"],
        errors::TOO_MANY_REQUESTS,
        "{}",
        p.phone.last_start()
    );
    assert_eq!((p.phone.fresh_starts(), p.phase().as_str()), (1, "online"));
    p.call("tasks.list", json!({}))
        .await
        .expect("the session works after a start-over that did not get through");
    // two in a row that do not get through end the schedule (each one holds calls for a moment)
    assert!(
        wait(6, || p.phone.failed_starts() == 2
            && p.phone.start_over_due().is_none())
        .await,
        "{} not through, due {:?}",
        p.phone.failed_starts(),
        p.phone.start_over_due()
    );
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert_eq!(
        (p.phone.fresh_starts(), p.phone.failed_starts()),
        (1, 2),
        "the schedule went on after two start-overs in a row did not get through"
    );
    p.call("tasks.list", json!({}))
        .await
        .expect("the session works");
    assert!(
        wait(10, || w.bridge.audit(1000).iter().any(|a| a["method"]
            == "link.startover"
            && a["result"] == "not_through"))
        .await,
        "the Mac has no record of the start-over that did not get through"
    );
}

// decision: PHN-220
#[tokio::test(flavor = "multi_thread")]
async fn a_change_of_the_relay_never_strands_a_phone() {
    let shared = Relay::start(None, None).await;
    let own_a = Relay::start(Some("tok-a"), None).await;
    let own_b = Relay::start(Some("tok-b"), None).await;
    let w = World::start(Shared::Custom(vec![shared.spec()]), None, true).await;
    w.limits_set(&json!({"redial_steps_s": [1, 1, 2], "dial_deadline_s": 3}))
        .unwrap();
    let waiting = || {
        let h = w.bridge.status()["relay_handover"].clone();
        json!({"phones_waiting": h["phones_waiting"], "on_shared_relays": h["on_shared_relays"]})
    };
    let p = w.paired_phone("phone").await;
    assert_eq!(p.tier(), "shared_relay");
    assert_eq!(
        waiting(),
        json!({"phones_waiting": 0, "on_shared_relays": false}),
        "no own relay: nothing to hand over"
    );

    // 1. the phone is off (the app closed, no network, between two networks: the Mac cannot tell these apart)
    //    when the owner sets his own relay. This is what stranded the owner's phone: it kept dialing the shared
    //    relay, where the Mac no longer was, and no retry could ever find it.
    let state = p.saved();
    p.phone.close().await;
    drop(p);
    w.bridge.relay_set(Some(own_a.spec())).await.unwrap();
    assert!(
        wait(10, || w.bridge.status()["relay"]["state"] == "own").await,
        "{}",
        w.bridge.status()["relay"]
    );
    assert!(
        wait(10, || waiting()
            == json!({"phones_waiting": 1, "on_shared_relays": true}))
        .await,
        "{}",
        waiting()
    );
    tokio::time::sleep(Duration::from_secs(3)).await; // (it may be off for weeks; the Mac stays findable meanwhile)
    let t = Instant::now();
    let p = TestPhone::start("phone", &w.shared, true, state, true).await;
    assert!(
        wait(40, || p.phase() == "online" && p.tier() == "own_relay").await,
        "the phone never found the Mac again: {}",
        p.phone.phase()
    );
    println!(
        "MEASURE relay set while the phone was off -> phone working on the own relay: {:?}",
        t.elapsed()
    );
    assert_eq!(
        p.saved().unwrap().mac.unwrap().own,
        Some(own_a.spec()),
        "it was handed the relay with its token"
    );
    assert!(p.call("ping", json!({})).await.is_ok());
    // every paired phone has the setting: the Mac leaves the shared relays (an own-relay user sends nothing there)
    assert!(
        wait(10, || waiting()
            == json!({"phones_waiting": 0, "on_shared_relays": false}))
        .await,
        "{}",
        waiting()
    );

    // 2. connected when the setting changes (to another relay): told over the link, moves by itself
    w.bridge.relay_set(Some(own_b.spec())).await.unwrap();
    assert!(
        wait(40, || p.phase() == "online"
            && p.tier() == "own_relay"
            && p.saved().unwrap().mac.unwrap().own == Some(own_b.spec()))
        .await,
        "{}",
        p.phone.phase()
    );
    assert!(
        wait(10, || waiting()["on_shared_relays"] == false).await,
        "{}",
        waiting()
    );

    // 3. off while one own relay is replaced by another (or its token changes): it knows only the old one
    let state = p.saved();
    p.phone.close().await;
    drop(p);
    w.bridge.relay_set(Some(own_a.spec())).await.unwrap();
    assert!(
        wait(10, || waiting()
            == json!({"phones_waiting": 1, "on_shared_relays": true}))
        .await,
        "{}",
        waiting()
    );
    let t = Instant::now();
    let p = TestPhone::start("phone", &w.shared, true, state, true).await;
    assert!(
        wait(60, || p.phase() == "online"
            && p.tier() == "own_relay"
            && p.saved().unwrap().mac.unwrap().own == Some(own_a.spec()))
        .await,
        "{}",
        p.phone.phase()
    );
    println!(
        "MEASURE own relay replaced while the phone was off -> phone working on the new one: {:?}",
        t.elapsed()
    );
    assert!(wait(10, || waiting()["on_shared_relays"] == false).await);

    // 4. off while the own relay is switched off
    let state = p.saved();
    p.phone.close().await;
    drop(p);
    w.bridge.relay_set(None).await.unwrap();
    assert_eq!(
        waiting(),
        json!({"phones_waiting": 0, "on_shared_relays": false})
    );
    let p = TestPhone::start("phone", &w.shared, true, state.clone(), true).await;
    assert!(
        wait(60, || p.phase() == "online" && p.tier() == "shared_relay").await,
        "{}",
        p.phone.phase()
    );

    // a stranger who knows the Mac's id finds it on the shared relay while it waits there, and gets nothing:
    // the same door as everywhere (no pairing window is open)
    p.phone.close().await;
    drop(p);
    w.bridge.relay_set(Some(own_b.spec())).await.unwrap();
    assert!(wait(10, || waiting()["on_shared_relays"] == true).await);
    let mut strangers = state.clone().unwrap();
    strangers.key = {
        use base64::Engine;
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode([5u8; 32])
    };
    let s = TestPhone::start("stranger", &w.shared, true, Some(strangers), true).await;
    assert!(s.wait_phase("stopped", 30).await, "{}", s.phone.phase());
    assert_eq!(s.stop_code(), errors::NOT_PAIRED);
    assert_eq!(
        waiting()["phones_waiting"],
        1,
        "a stranger's visit hands nothing over"
    );
}

// decision: PHN-222
/// An idle endpoint keeps the one address-discovery connection it has: a later periodic report (iroh makes one
/// every 20 to 26 s) opens no new one. iroh 1.4.1 as published opened new ones to every relay of its map at
/// every report: about 100 kB a minute each way on the owner's phone, doing nothing.
#[tokio::test(flavor = "multi_thread")]
async fn an_idle_endpoint_makes_no_new_discovery_connections() {
    use iroh::{Endpoint, RelayConfig, RelayMap, RelayMode, Watcher, endpoint::presets};
    // a relay with address discovery (QUIC) on 127.0.0.1, its certificates self-signed
    let server = iroh_relay::server::Server::spawn(iroh_relay::server::testing::server_config())
        .await
        .unwrap();
    let quic = iroh_relay::RelayQuicConfig::new(server.quic_addr().unwrap().port());
    let relay = RelayConfig::new(server.https_url().unwrap(), Some(quic));
    // (on 127.0.0.1 alone and no port mapping: nothing it sends leaves this Mac, as the test guard requires)
    let ep = Endpoint::builder(presets::Minimal)
        .relay_mode(RelayMode::Custom(RelayMap::from_iter([relay])))
        .ca_tls_config(iroh::tls::CaTlsConfig::insecure_skip_verify())
        .portmapper_config(iroh::endpoint::PortmapperConfig::Disabled)
        .clear_ip_transports()
        .bind_addr("127.0.0.1:0")
        .unwrap()
        .bind()
        .await
        .unwrap();
    let reports = || ep.metrics().net_report.reports.get();
    let sent = || ep.metrics().socket.send_ipv4.get() + ep.metrics().socket.send_ipv6.get();
    assert!(
        wait(15, || ep.net_report().get().is_some_and(|r| r.udp_v4)).await,
        "no first report with address discovery"
    );
    tokio::time::sleep(Duration::from_secs(1)).await;
    let (reports_before, sent_before) = (reports(), sent());
    assert!(
        wait(40, || reports() > reports_before).await,
        "no periodic report in 40 s"
    );
    tokio::time::sleep(Duration::from_secs(2)).await;
    let used = sent() - sent_before;
    println!("MEASURE one periodic report of an idle endpoint sent {used} bytes");
    // a new discovery connection starts with a packet of at least 1200 bytes; a kept one sends a keep-alive
    assert!(
        used < 1000,
        "an idle endpoint sent {used} bytes around one periodic report: it made a new discovery connection"
    );
    ep.close().await;
    server.shutdown().await.ok();
}

// decision: PHN-445, PHN-462
#[tokio::test(flavor = "multi_thread")]
async fn on_the_public_relay_the_phone_starts_no_timed_start_overs() {
    let relay = Relay::start(None, None).await;
    let w = World::start(Shared::Custom(vec![relay.spec()]), None, false).await;
    // the lab's schedule at its fastest, and a phone that would start over even without hope: still none here
    w.limits_set(&json!({"handover_retry_every_s": 1, "handover_retry_fast_for_s": 600}))
        .unwrap();
    let p = TestPhone::start_with("phone", &w.shared, false, None, true, |c| {
        c.force_relay = true;
        c.start_over_without_hope = true
    })
    .await;
    w.pair(&p).await;
    p.unlock().await.unwrap();
    assert!(wait(20, || p.tier() == "shared_relay").await);
    assert_eq!(p.phone.limits().handover_retry_every_s, 1);
    p.phone.network_changed_to(Some("cellular")).await;
    // (bounded real time: the lab's schedule would have started over about five times in it)
    tokio::time::sleep(Duration::from_secs(6)).await;
    assert_eq!(
        (p.phone.fresh_starts(), p.phone.failed_starts()),
        (0, 0),
        "a timed start-over on the public relay"
    );
    assert_eq!(
        (p.phase().as_str(), p.tier().as_str()),
        ("online", "shared_relay"),
        "the session goes on where it is"
    );
    // iroh's own try for a direct path is untouched: the phone still reports its network to the Mac
    assert!(p.call("chat.status", json!({})).await.is_ok());
}

// decision: PHN-453, PHN-219, PHN-462
#[tokio::test(flavor = "multi_thread")]
async fn direct_is_used_whenever_it_exists_unless_always_use_my_relay_is_on() {
    let public = Relay::start(None, None).await;
    let own = Relay::start(Some("tok-9"), None).await;
    let w = World::start(Shared::Custom(vec![public.spec()]), Some(own.spec()), false).await;
    let p = w.paired_phone("phone").await;
    // the default (the owner, 2026-11-16): direct whenever a direct path exists, the own relay when it does not
    assert!(!w.bridge.limits_get().own_relay_always);
    assert!(
        wait(30, || p.tier() == "direct").await,
        "tier stayed {}",
        p.tier()
    );
    // the switch on: the session moves to the owner's relay and stays there though direct would work
    w.limits_set(&json!({"own_relay_always": true})).unwrap();
    assert!(
        wait(40, || p.phase() == "online" && p.tier() == "own_relay").await,
        "{}",
        p.phone.phase()
    );
    tokio::time::sleep(Duration::from_secs(4)).await;
    assert_eq!(p.tier(), "own_relay", "it stayed on the owner's relay");
    assert!(
        p.phone.limits().own_relay_always,
        "the phone was told the setting"
    );
    // never the public relay meanwhile
    assert_eq!(w.bridge.status()["relay"]["state"], "own");
    // the switch off again: back to direct
    w.limits_set(&json!({"own_relay_always": false})).unwrap();
    assert!(
        wait(40, || p.phase() == "online" && p.tier() == "direct").await,
        "{}",
        p.phone.phase()
    );
}
