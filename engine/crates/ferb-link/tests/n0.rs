#![cfg(target_os = "macos")]
//! The basics through n0's PUBLIC relays, every byte relayed (no direct path). Small on purpose: about 0.3 MB.
//! Not part of the normal run: `scripts/test.sh n0`. Do not loop it; it uses other people's free servers.
mod common;
use common::*;
use ferb_link::{
    errors,
    net::{self, Shared},
};
use serde_json::json;
use std::time::Instant;

// decision: PHN-418, PHN-301
#[tokio::test(flavor = "multi_thread")]
#[ignore = "uses n0's public relays; run it by hand, once"]
async fn basics_through_n0s_public_relay() {
    let t = Instant::now();
    let w = World::start(Shared::N0, None, true).await;
    println!(
        "MEASURE n0: Mac endpoint at its home relay after {:?}",
        t.elapsed()
    );
    let t = Instant::now();
    let p = w.paired_phone("phone").await;
    println!(
        "MEASURE n0: pairing (QR to online, the Yes given at once): {:?}",
        t.elapsed()
    );
    assert_eq!(p.tier(), "shared_relay");
    let link = p.call("link.status", json!({})).await.unwrap();
    assert!(link["path"].as_str().unwrap().contains("relay"), "{link}");
    assert!(
        link["path"].as_str().unwrap().contains("iroh.link"),
        "not n0's relay: {link}"
    );

    // messages
    let mut rtts = vec![];
    for i in 0..12 {
        let t = Instant::now();
        let mut evs = vec![];
        p.phone
            .call(
                "chat.send",
                json!({"text": format!("hello {i}")}),
                None,
                |e| evs.push(e),
            )
            .await
            .unwrap();
        assert!(
            evs.last().unwrap()["text"]
                .as_str()
                .unwrap()
                .contains(&format!("hello {i}"))
        );
        rtts.push(t.elapsed().as_secs_f64() * 1000.0);
    }
    println!(
        "MEASURE n0: message -> reply, 12 times: p50 {:.0} ms, max {:.0} ms",
        pct(&mut rtts.clone(), 0.5),
        pct(&mut rtts, 1.0)
    );

    // a question from a task reaches the phone and its answer reaches the task
    let ph = p.phone.clone();
    let task = tokio::spawn(async move {
        let mut evs = vec![];
        ph.call(
            "chat.send",
            json!({"text": "ask which colour?"}),
            None,
            |e| evs.push(e),
        )
        .await
        .map(|_| evs)
    });
    let t = Instant::now();
    assert!(
        wait(15, || p.status()["approvals"]
            .as_array()
            .is_some_and(|a| a.len() == 1))
        .await
    );
    println!(
        "MEASURE n0: question shown on the phone after {:?} (the Mac looks every 1.5 s)",
        t.elapsed()
    );
    let ap = p.status()["approvals"][0].clone();
    p.call(
        "chat.answer",
        json!({"approval_id": ap["approval_id"], "text": "blue"}),
    )
    .await
    .unwrap();
    assert!(
        task.await.unwrap().unwrap().last().unwrap()["text"]
            .as_str()
            .unwrap()
            .contains("blue")
    );

    // one of FERB's own checks: refused without the unlock proof, accepted with it
    let id = w
        .standin
        .cli(
            "standin.ask",
            json!({"capability": "ferb.confirm", "reason": "Send it?", "exact": "send"}),
        )
        .await["approval_id"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(wait(15, || p.approval(&id).is_some()).await);
    let yes = json!({"approval_id": id, "granted": true});
    assert_eq!(
        common::code(p.call("chat.approve", yes.clone()).await),
        errors::UNLOCK_REQUIRED
    );
    let proof = p.proof(
        &id,
        p.approval(&id).unwrap()["phone"]["challenge"]
            .as_str()
            .unwrap(),
        "chat.approve",
        &yes,
    );
    let t = Instant::now();
    p.phone
        .call("chat.approve", yes, Some(proof), |_| {})
        .await
        .unwrap();
    println!("MEASURE n0: signed Yes round trip: {:?}", t.elapsed());

    // stop a running task
    let ph = p.phone.clone();
    let long = tokio::spawn(async move {
        ph.call("chat.send", json!({"text": "long 60"}), None, |_| {})
            .await
    });
    assert!(
        wait(15, || p.status()["running"]
            .as_array()
            .is_some_and(|a| a.iter().any(|t| t["goal"] == "long 60")))
        .await
    );
    let tid = p.status()["running"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["goal"] == "long 60")
        .unwrap()["task_id"]
        .clone();
    // bulk on the shared relay: one small picture is fine; live preview and a large file are refused unsent
    let (_, pic) = p
        .phone
        .get(json!({"what": "preview", "task_id": tid}), |_, _| {})
        .await
        .unwrap();
    assert_eq!(pic.len(), 60_000);
    assert_eq!(
        common::code(
            p.phone
                .get(
                    json!({"what": "preview", "task_id": tid, "live": true}),
                    |_, _| {}
                )
                .await
                .map(|x| x.1.len())
        ),
        errors::NEEDS_BETTER_PATH
    );
    let t = Instant::now();
    assert_eq!(
        p.call("task.stop", json!({"task_ids": [tid]}))
            .await
            .unwrap()["stopped"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    println!("MEASURE n0: Stop round trip: {:?}", t.elapsed());
    long.await.unwrap().unwrap();
    p.phone
        .call("chat.send", json!({"text": "file 3000000"}), None, |_| {})
        .await
        .unwrap();
    let st = p.call("chat.status", json!({})).await.unwrap();
    let last = st["results"].as_array().unwrap().last().unwrap()["task_id"]
        .as_str()
        .unwrap()
        .to_string();
    let e = p
        .phone
        .get(
            json!({"what": "file", "task_id": last, "index": 0}),
            |_, _| {},
        )
        .await
        .unwrap_err();
    assert_eq!(e.code, errors::TRANSFER_TOO_LARGE);
    // a phone that is not paired is turned away through n0 as well
    let stranger = w.phone("stranger").await;
    let url = ferb_link::pairing::Invite {
        mac: w.bridge.id().parse().unwrap(),
        relays: net::home_relays(&w.bridge.endpoint())
            .iter()
            .map(|u| u.to_string())
            .collect(),
        own: None,
        secret: [1; 32],
    }
    .to_url();
    stranger.phone.pair(&url).unwrap();
    assert!(stranger.wait_phase("stopped", 20).await);
    assert_eq!(stranger.stop_code(), errors::PAIR_WINDOW_CLOSED);

    let (m, f) = (
        net::relay_counters(&w.bridge.endpoint()),
        net::relay_counters(&p.phone.endpoint()),
    );
    println!(
        "MEASURE n0: relayed bytes, Mac sent {} + phone sent {} = {} (received: Mac {}, phone {}); rate-limit notices: {}",
        m.sent,
        f.sent,
        m.sent + f.sent,
        m.received,
        f.received,
        m.ratelimited + f.ratelimited
    );
    println!("MEASURE n0: whole run {:?}", t.elapsed());
    assert!(m.sent + f.sent < 2_000_000, "this run must stay small");
    stranger.phone.close().await;
    p.phone.close().await;
    w.bridge.close().await;
}
