#![cfg(target_os = "macos")]
//! The product path against a real own relay on a VM. Not part of the normal run:
//!   FERB_VM_RELAY_LINK='ferbrelay://…' FERB_VM_SSH=alias scripts/test.sh vm
//! FERB_VM_SSH (optional) is an ssh alias on which `systemctl stop|start ferb-relay` may be run for the outage
//! part. The fallback goes through n0's public relays (a few kB). The link is read from the environment only.
mod common;
use common::*;
use ferb_link::{
    errors,
    net::{RelaySpec, Shared},
};
use serde_json::json;
use std::time::{Duration, Instant};

// decision: PHN-205, PHN-208, PHN-211
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a real relay: set FERB_VM_RELAY_LINK"]
async fn the_product_path_through_a_real_own_relay() {
    let Ok(link) = std::env::var("FERB_VM_RELAY_LINK") else {
        panic!("set FERB_VM_RELAY_LINK")
    };
    let spec = RelaySpec::from_text(&link, None).unwrap();
    let w = World::start(Shared::N0, None, true).await;
    w.limits_set(&json!({"own_relay_fallback_after_s": 5, "own_relay_probe_s": 5, "redial_steps_s": [1, 1, 2], "dial_deadline_s": 5})).unwrap();

    // Test: ok; a wrong token and a missing certificate say what is wrong
    let ok = w.bridge.relay_test(&spec).await.unwrap();
    println!(
        "MEASURE vm: relay Test ok in {} ms, relay version {:?}",
        ok.ms, ok.relay_version
    );
    // an endpoint that uses only this relay still learns its public address (the relay's address discovery)
    assert!(
        ok.steps
            .iter()
            .any(|s| s.starts_with("address discovery answers"))
            && ok.warnings.is_empty(),
        "{ok:?}"
    );
    let mut wrong = spec.clone();
    wrong.token = Some("wrong-token".into());
    assert_eq!(
        common::code(w.bridge.relay_test(&wrong).await.map(|_| ())),
        errors::OWN_RELAY_ACCESS_DENIED
    );
    if spec.ca.is_some() {
        let mut unpinned = spec.clone();
        unpinned.ca = None;
        assert_eq!(
            common::code(w.bridge.relay_test(&unpinned).await.map(|_| ())),
            errors::OWN_RELAY_TLS
        );
    }
    assert_eq!(
        common::code(w.bridge.relay_set(Some(wrong)).await),
        errors::OWN_RELAY_ACCESS_DENIED
    );

    // set it; a phone pairs and is on the own relay having typed nothing
    w.bridge.relay_set(Some(spec.clone())).await.unwrap();
    assert!(
        wait(30, || w.bridge.status()["relay"]["state"] == "own").await,
        "{}",
        w.bridge.status()["relay"]
    );
    let p = w.paired_phone("phone").await;
    assert!(
        wait(20, || p.tier() == "own_relay").await,
        "{}",
        p.phone.phase()
    );
    assert_eq!(p.saved().unwrap().mac.unwrap().own, Some(spec.clone()));

    // (this test's own endpoints have their IP transports removed, so a separate ordinary endpoint is asked)
    let probe = ferb_link::net::bind(&ferb_link::net::NetConfig {
        secret: iroh::SecretKey::from_bytes(&[4; 32]),
        own_relay: Some(spec.clone()),
        shared: Shared::Custom(vec![]),
        roots: None,
        force_relay: false,
        accept: false,
        streams_max: 1,
        relay_only: false,
        segmentation_offload: true,
        portmapper: false,
        loopback_only: false, // (a real relay on the internet, by hand)
    })
    .await
    .unwrap();
    assert!(
        wait(15, || {
            let f = ferb_link::net::facts(&probe);
            f.known && (f.public_v4 || f.public_v6)
        })
        .await,
        "with only the own relay an endpoint learned no public address: {:?}",
        ferb_link::net::facts(&probe)
    );
    println!(
        "MEASURE vm: an endpoint with only the own relay learned its public address (udp v4 {}, v6 {})",
        ferb_link::net::facts(&probe).udp_v4,
        ferb_link::net::facts(&probe).udp_v6
    );
    probe.close().await;
    let mut rtts = vec![];
    for _ in 0..30 {
        let t = Instant::now();
        p.call("ping", json!({})).await.unwrap();
        rtts.push(t.elapsed().as_secs_f64() * 1000.0);
    }
    println!(
        "MEASURE vm: control round trip through the VM, 30 calls: p50 {:.0} ms, p95 {:.0} ms, max {:.0} ms",
        pct(&mut rtts.clone(), 0.5),
        pct(&mut rtts.clone(), 0.95),
        pct(&mut rtts, 1.0)
    );

    // bulk: an 8 MB file down, a 3 MB upload, live preview
    p.phone
        .call("chat.send", json!({"text": "file 8000000"}), None, |_| {})
        .await
        .unwrap();
    let st = p.call("chat.status", json!({})).await.unwrap();
    let last = st["results"].as_array().unwrap().last().unwrap()["task_id"]
        .as_str()
        .unwrap()
        .to_string();
    let t = Instant::now();
    let ph = p.phone.clone();
    let bulk = tokio::spawn(async move {
        ph.get(
            json!({"what": "file", "task_id": last, "index": 0}),
            |_, _| {},
        )
        .await
    });
    let mut during = vec![];
    while !bulk.is_finished() {
        let c = Instant::now();
        p.call("ping", json!({})).await.unwrap();
        during.push(c.elapsed().as_secs_f64() * 1000.0);
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    let (meta, b) = bulk.await.unwrap().unwrap();
    let secs = t.elapsed().as_secs_f64();
    assert_eq!(
        (b.len(), meta["tier"].as_str().unwrap()),
        (8_000_000, "own_relay")
    );
    println!(
        "MEASURE vm: 8 MB download through the VM in {secs:.1} s ({:.2} MB/s); control during it p50 {:.0} ms, max {:.0} ms ({} calls)",
        8.0 / secs,
        pct(&mut during.clone(), 0.5),
        pct(&mut during.clone(), 1.0),
        during.len()
    );
    let t = Instant::now();
    p.unlock().await.unwrap();
    p.phone
        .put("photo.jpg", &vec![7u8; 3_000_000], |_, _| {})
        .await
        .unwrap();
    println!(
        "MEASURE vm: 3 MB upload through the VM in {:.1} s ({:.2} MB/s)",
        t.elapsed().as_secs_f64(),
        3.0 / t.elapsed().as_secs_f64()
    );
    let ph = p.phone.clone();
    let _long = tokio::spawn(async move {
        ph.call("chat.send", json!({"text": "long 100"}), None, |_| {})
            .await
    });
    assert!(
        wait(15, || p.status()["running"]
            .as_array()
            .is_some_and(|a| a.iter().any(|t| t["goal"] == "long 100")))
        .await
    );
    let tid = p.status()["running"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["goal"] == "long 100")
        .unwrap()["task_id"]
        .clone();
    w.standin
        .cli("standin.preview_size", json!({"bytes": 250_000}))
        .await;
    let t = Instant::now();
    let mut frames = 0;
    while t.elapsed() < Duration::from_secs(5) {
        let (_, pic) = p
            .phone
            .get(
                json!({"what": "preview", "task_id": tid, "live": true}),
                |_, _| {},
            )
            .await
            .unwrap();
        assert_eq!(pic.len(), 250_000);
        frames += 1;
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    println!("MEASURE vm: live preview, 250 kB frames: {frames} in 5 s through the VM");
    assert_eq!(
        w.bridge.status()["shared_relay"]["bulk"],
        "refused",
        "the public relays carry no bulk (kb PHN-444)"
    );

    // the outage: stop the relay service -> fallback to n0 with the reason and the caps -> start -> return
    let Ok(ssh) = std::env::var("FERB_VM_SSH") else {
        return;
    };
    let run = |cmd: &'static str| {
        std::process::Command::new("ssh")
            .args([ssh.as_str(), cmd])
            .status()
            .unwrap()
            .success()
    };
    let t = Instant::now();
    assert!(run("systemctl stop ferb-relay"));
    let back_up = scopeguard(|| {
        run("systemctl start ferb-relay");
    });
    assert!(
        wait(60, || w.bridge.status()["relay"]["state"] == "own_fallback").await,
        "{}",
        w.bridge.status()["relay"]
    );
    let reason = w.bridge.status()["relay"]["error"].clone();
    assert_eq!(reason["code"], errors::OWN_RELAY_UNREACHABLE, "{reason}");
    println!(
        "MEASURE vm: relay stopped -> Mac on n0 after {:.1} s (fallback after 5 s in this test)",
        t.elapsed().as_secs_f64()
    );
    assert!(
        wait(90, || p.phase() == "online" && p.tier() == "shared_relay").await,
        "{}",
        p.phone.phase()
    );
    println!(
        "MEASURE vm: relay stopped -> phone working again through n0 after {:.1} s",
        t.elapsed().as_secs_f64()
    );
    assert!(p.call("chat.status", json!({})).await.is_ok());
    let e = p
        .phone
        .get(
            json!({"what": "preview", "task_id": tid, "live": true}),
            |_, _| {},
        )
        .await
        .unwrap_err();
    assert_eq!(
        e.message,
        "Live preview needs a direct connection or your own relay; you are on the shared relay."
    );
    let t = Instant::now();
    drop(back_up);
    assert!(
        wait(60, || w.bridge.status()["relay"]["state"] == "own").await,
        "{}",
        w.bridge.status()["relay"]
    );
    assert!(
        wait(90, || p.phase() == "online" && p.tier() == "own_relay").await,
        "{}",
        p.phone.phase()
    );
    println!(
        "MEASURE vm: relay started -> phone back on the own relay after {:.1} s",
        t.elapsed().as_secs_f64()
    );
    let n0 = ferb_link::net::relay_counters(&w.bridge.endpoint());
    println!(
        "MEASURE vm: done (the Mac's current endpoint relayed {} bytes)",
        n0.sent
    );
    p.phone.close().await;
    w.bridge.close().await;
}

struct Guard<F: FnMut()>(F);
impl<F: FnMut()> Drop for Guard<F> {
    fn drop(&mut self) {
        (self.0)()
    }
}
fn scopeguard<F: FnMut()>(f: F) -> Guard<F> {
    Guard(f)
}
