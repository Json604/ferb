#![cfg(target_os = "macos")]
//! The link after pairing, on 127.0.0.1: the basics, the phone's method list, answers and the unlock proof,
//! revocation on a live connection, the tiers and their limits, floods, and control under bulk.
mod common;
use common::*;
use ferb_link::{errors, net::Shared, wire};
use serde_json::{Value, json};
use std::time::{Duration, Instant};

/// The id of the running task with this goal, once the Mac has pushed it to the phone.
async fn running(p: &TestPhone, goal: &str) -> String {
    let find = || {
        p.status()["running"].as_array().and_then(|a| {
            a.iter()
                .find(|t| t["goal"] == goal)
                .map(|t| t["task_id"].as_str().unwrap().to_string())
        })
    };
    assert!(
        wait(10, || find().is_some()).await,
        "task \"{goal}\" never showed as running: {}",
        p.status()
    );
    find().unwrap()
}

/// One relay standing in for the shared (n0) relays, every byte through it.
async fn shared_world() -> (Relay, World) {
    let relay = Relay::start(None, None).await;
    let w = World::start(Shared::Custom(vec![relay.spec()]), None, true).await;
    (relay, w)
}

// decision: PHN-301, PHN-304
#[tokio::test(flavor = "multi_thread")]
async fn basics_work_on_the_shared_relay() {
    let (_relay, w) = shared_world().await;
    let p = w.paired_phone("phone").await;
    assert_eq!(p.tier(), "shared_relay");

    // a message; its acknowledgement and FERB's reply stream back
    let mut events = vec![];
    let t = Instant::now();
    let r = p
        .phone
        .call("chat.send", json!({"text": "hello"}), None, |e| {
            events.push(e)
        })
        .await
        .unwrap();
    assert_eq!(r["ended"], true);
    assert_eq!(events[0]["type"], "ack");
    assert!(
        events.last().unwrap()["text"]
            .as_str()
            .unwrap()
            .contains("got: hello")
    );
    println!(
        "MEASURE chat.send round trip (local relay): {:?}",
        t.elapsed()
    );

    // a task that asks a question: the question is pushed to the phone, the answer reaches the task
    let p2 = p.phone.clone();
    let task = tokio::spawn(async move {
        let mut evs = vec![];
        p2.call(
            "chat.send",
            json!({"text": "ask which colour?"}),
            None,
            |e| evs.push(e),
        )
        .await
        .map(|_| evs)
    });
    assert!(
        wait(10, || p.status()["approvals"]
            .as_array()
            .is_some_and(|a| a.len() == 1))
        .await,
        "no question pushed: {}",
        p.status()
    );
    let ap = p.status()["approvals"][0].clone();
    assert_eq!(ap["reason"], "which colour?");
    assert_eq!(ap["phone"]["needs_unlock"], false);
    p.call(
        "chat.answer",
        json!({"approval_id": ap["approval_id"], "text": "blue"}),
    )
    .await
    .unwrap();
    let evs = task.await.unwrap().unwrap();
    assert!(
        evs.last().unwrap()["text"]
            .as_str()
            .unwrap()
            .contains("blue")
    );

    // a long task: progress arrives, Stop ends it
    let p2 = p.phone.clone();
    let task = tokio::spawn(async move {
        let mut evs = vec![];
        p2.call("chat.send", json!({"text": "long 30"}), None, |e| {
            evs.push(e)
        })
        .await
        .map(|_| evs)
    });
    let id = running(&p, "long 30").await;
    let t = Instant::now();
    let r = p
        .call("task.stop", json!({"task_ids": [id]}))
        .await
        .unwrap();
    assert_eq!(r["stopped"].as_array().unwrap().len(), 1);
    println!(
        "MEASURE task.stop round trip (local relay): {:?}",
        t.elapsed()
    );
    let evs = task.await.unwrap().unwrap();
    assert_eq!(evs.last().unwrap()["status"], "stopped");

    // status, the task list, the link's own status
    assert!(
        p.call("chat.status", json!({})).await.unwrap()["results"]
            .as_array()
            .unwrap()
            .len()
            >= 2
    );
    assert!(
        p.call("tasks.list", json!({})).await.unwrap()["tasks"]
            .as_array()
            .unwrap()
            .len()
            >= 2
    );
    let link = p.call("link.status", json!({})).await.unwrap();
    assert_eq!(link["tier"], "shared_relay");
    assert!(link["path"].as_str().unwrap().starts_with("relay "));

    // many large pushes in a row arrive whole and in order, and the session stays up (a pushed frame is never
    // read half way)
    let conn = p.phone.connection().unwrap();
    for i in 0..30 {
        w.standin
            .cli(
                "standin.ask",
                json!({"reason": format!("{i} {}", "why ".repeat(400))}),
            )
            .await;
        tokio::time::sleep(Duration::from_millis(120)).await;
    }
    assert!(
        wait(10, || p.status()["approvals"]
            .as_array()
            .is_some_and(|a| a.len() == 30))
        .await,
        "{}",
        p.status()["approvals"]
            .as_array()
            .map(|a| a.len())
            .unwrap_or(0)
    );
    assert!(
        conn.close_reason().is_none() && p.phase() == "online",
        "the session dropped: {:?}",
        conn.close_reason()
    );

    // stop everything
    assert_eq!(
        p.call("stop.engage", json!({})).await.unwrap()["stopped"],
        true
    );
    // FERB's daemon saw every one of these as role phone, never cli
    let st = w.standin.cli("standin.state", json!({})).await;
    assert!(
        st["calls"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|c| c[1] == json!(p.phone.id()))
            .all(|c| c[0] == "phone")
    );
}

// decision: PHN-105, PHN-109, PHN-110, PHN-108
#[tokio::test(flavor = "multi_thread")]
async fn hard_answers_need_the_unlock_proof_and_stale_answers_are_refused() {
    let (_relay, w) = shared_world().await;
    let p = w.paired_phone("phone").await;
    let (w, p) = (&w, &p);
    let ask = move |kind: &'static str, cap: &'static str| {
        w.standin.cli("standin.ask", json!({"kind": kind, "capability": cap, "reason": "Send 500 to Alex?", "exact": "pay 500"}))
    };
    let answered = move |id: String| async move {
        let st = w.standin.cli("standin.state", json!({})).await;
        st["approvals"]
            .as_array()
            .unwrap()
            .iter()
            .find(|a| a["approval_id"] == id)
            .unwrap()["state"]
            .as_str()
            .unwrap()
            .to_string()
    };
    let pushed = move |id: String| async move {
        assert!(
            wait(10, || p.approval(&id).is_some()).await,
            "question {id} not pushed"
        );
        p.approval(&id).unwrap()
    };

    // one of FERB's own checks: Yes without the unlock proof is refused and nothing reaches the daemon
    let id = ask("owner_question", "ferb.confirm").await["approval_id"]
        .as_str()
        .unwrap()
        .to_string();
    let ap = pushed(id.clone()).await;
    assert_eq!(ap["phone"]["needs_unlock"], true);
    assert_eq!(
        ap["exact"], "pay 500",
        "the phone is shown the exact thing it approves"
    );
    let yes = json!({"approval_id": id, "granted": true});
    assert_eq!(
        common::code(p.call("chat.approve", yes.clone()).await),
        errors::UNLOCK_REQUIRED
    );
    assert_eq!(
        common::code(
            p.call("chat.answer", json!({"approval_id": id, "text": "Yes"}))
                .await
        ),
        errors::UNLOCK_REQUIRED
    );
    assert_eq!(
        common::code(
            p.call("chat.correct", json!({"approval_id": id, "text": "yes"}))
                .await
        ),
        errors::UNLOCK_REQUIRED
    );
    // a proof by another key
    let other = TestPhone::start("other", &w.shared, true, None, true).await;
    let challenge = ap["phone"]["challenge"].as_str().unwrap();
    let forged = other.proof(&id, challenge, "chat.approve", &yes);
    assert_eq!(
        common::code(
            p.phone
                .call("chat.approve", yes.clone(), Some(forged), |_| {})
                .await
        ),
        errors::UNLOCK_INVALID
    );
    // a real proof, but for a different answer (a signed No presented with a Yes)
    let no = json!({"approval_id": id, "granted": false});
    let for_no = p.proof(&id, challenge, "chat.approve", &no);
    assert_eq!(
        common::code(
            p.phone
                .call("chat.approve", yes.clone(), Some(for_no), |_| {})
                .await
        ),
        errors::UNLOCK_INVALID
    );
    // a real proof over a wrong challenge
    let bad = p.proof(&id, "AAAA", "chat.approve", &yes);
    assert_eq!(
        common::code(
            p.phone
                .call("chat.approve", yes.clone(), Some(bad), |_| {})
                .await
        ),
        errors::UNLOCK_INVALID
    );
    assert_eq!(answered(id.clone()).await, "pending");
    // signed after the PIN, on a phone that has a fingerprint: not taken for a hard Yes (kb SEC-37)
    assert_eq!(ap["phone"]["fingerprint"], true);
    let pin = p.pin_proof(&id, challenge, "chat.approve", &yes);
    let e = p
        .phone
        .call("chat.approve", yes.clone(), Some(pin), |_| {})
        .await
        .unwrap_err();
    assert!(
        e.code == errors::UNLOCK_INVALID && e.message.contains("fingerprint"),
        "{e}"
    );
    assert_eq!(answered(id.clone()).await, "pending");
    // the real proof
    let good = p.proof(&id, challenge, "chat.approve", &yes);
    p.phone
        .call("chat.approve", yes.clone(), Some(good.clone()), |_| {})
        .await
        .unwrap();
    assert_eq!(answered(id.clone()).await, "granted");
    // replayed: the question is no longer waiting
    assert_eq!(
        common::code(
            p.phone
                .call("chat.approve", yes.clone(), Some(good), |_| {})
                .await
        ),
        errors::QUESTION_NOT_WAITING
    );

    // No never needs the unlock
    let id = ask("owner_question", "ferb.confirm").await["approval_id"]
        .as_str()
        .unwrap()
        .to_string();
    pushed(id.clone()).await;
    p.call("chat.approve", json!({"approval_id": id, "granted": false}))
        .await
        .unwrap();
    assert_eq!(answered(id).await, "denied");

    // a stale Yes: asked, the phone signs, the action is cancelled on the Mac, the old answer arrives
    let id = ask("owner_question", "ferb.confirm").await["approval_id"]
        .as_str()
        .unwrap()
        .to_string();
    let ap = pushed(id.clone()).await;
    let yes = json!({"approval_id": id, "granted": true});
    let proof = p.proof(
        &id,
        ap["phone"]["challenge"].as_str().unwrap(),
        "chat.approve",
        &yes,
    );
    w.standin
        .cli("standin.cancel", json!({"approval_id": id}))
        .await;
    assert_eq!(
        common::code(p.phone.call("chat.approve", yes, Some(proof), |_| {}).await),
        errors::QUESTION_NOT_WAITING
    );
    assert_eq!(answered(id).await, "superseded");

    // ---- a hidden input (a password) goes through the sealed path: unlock proof, one-time challenge, no trace
    const SECRET: &str = "correct-horse-9471-battery";
    let mut admin = w.bridge.admin_events();
    let hidden = |kind: &'static str| async move {
        w.standin.cli("standin.ask", json!({"kind": kind, "capability": "secret.fill", "reason": "Password for the bank site", "options": []})).await["approval_id"].as_str().unwrap().to_string()
    };
    let id = hidden("owner_secret").await;
    let ap = pushed(id.clone()).await;
    assert_eq!(
        (
            ap["phone"]["can_answer"].clone(),
            ap["phone"]["hidden"].clone(),
            ap["phone"]["needs_unlock"].clone()
        ),
        (json!(true), json!(true), json!(true))
    );
    let challenge = ap["phone"]["challenge"].as_str().unwrap().to_string();
    let answer = json!({"approval_id": id, "text": SECRET});
    // no proof
    assert_eq!(
        common::code(p.call("chat.answer", answer.clone()).await),
        errors::UNLOCK_REQUIRED
    );
    // a bare Yes or a correction never answers a hidden input
    assert_eq!(
        common::code(
            p.call("chat.approve", json!({"approval_id": id, "granted": true}))
                .await
        ),
        errors::BAD_REQUEST
    );
    assert_eq!(
        common::code(
            p.call("chat.correct", json!({"approval_id": id, "text": SECRET}))
                .await
        ),
        errors::BAD_REQUEST
    );
    // a proof by another key; a proof for another value; a proof for another question; an ordinary (non-sealed) proof
    let forged = other.sealed_proof(&id, &challenge, SECRET);
    assert_eq!(
        common::code(
            p.phone
                .call("chat.answer", answer.clone(), Some(forged), |_| {})
                .await
        ),
        errors::UNLOCK_INVALID
    );
    let other_value = p.sealed_proof(&id, &challenge, "something-else");
    assert_eq!(
        common::code(
            p.phone
                .call("chat.answer", answer.clone(), Some(other_value), |_| {})
                .await
        ),
        errors::UNLOCK_INVALID
    );
    let id2 = hidden("owner_login_code").await;
    let ap2 = pushed(id2.clone()).await;
    let for_other_question =
        p.sealed_proof(&id2, ap2["phone"]["challenge"].as_str().unwrap(), SECRET);
    assert_eq!(
        common::code(
            p.phone
                .call(
                    "chat.answer",
                    answer.clone(),
                    Some(for_other_question),
                    |_| {}
                )
                .await
        ),
        errors::UNLOCK_INVALID
    );
    let unsealed = p.proof(
        &id,
        &challenge,
        "chat.answer",
        &json!({"approval_id": id, "text": "x"}),
    );
    assert_eq!(
        common::code(
            p.phone
                .call("chat.answer", answer.clone(), Some(unsealed), |_| {})
                .await
        ),
        errors::UNLOCK_INVALID
    );
    assert_eq!(answered(id.clone()).await, "pending");
    // the real one
    let good = p.sealed_proof(&id, &challenge, SECRET);
    let reply = p
        .phone
        .call("chat.answer", answer.clone(), Some(good.clone()), |_| {})
        .await
        .unwrap();
    assert_eq!(answered(id.clone()).await, "granted");
    assert_eq!(
        w.standin
            .cli(
                "standin.fill_matches",
                json!({"approval_id": id, "value": SECRET})
            )
            .await["matches"],
        true,
        "the daemon's fill got exactly the value"
    );
    // replayed: refused, and the second question is still untouched
    assert_eq!(
        common::code(
            p.phone
                .call("chat.answer", answer.clone(), Some(good), |_| {})
                .await
        ),
        errors::QUESTION_NOT_WAITING
    );
    assert_eq!(answered(id2.clone()).await, "pending");
    // stale: signed, then cancelled on the Mac, then delivered
    let stale = p.sealed_proof(&id2, ap2["phone"]["challenge"].as_str().unwrap(), SECRET);
    w.standin
        .cli("standin.cancel", json!({"approval_id": id2}))
        .await;
    assert_eq!(
        common::code(
            p.phone
                .call(
                    "chat.answer",
                    json!({"approval_id": id2, "text": SECRET}),
                    Some(stale),
                    |_| {}
                )
                .await
        ),
        errors::QUESTION_NOT_WAITING
    );
    // declining needs no unlock
    let id3 = hidden("owner_admin_password").await;
    pushed(id3.clone()).await;
    p.call("chat.answer", json!({"approval_id": id3, "text": ""}))
        .await
        .unwrap();
    assert_eq!(answered(id3).await, "denied");
    // the value is nowhere: not in the reply, the daemon's state, anything pushed to the phone, any admin event,
    // or any file the bridge or the stand-in keeps
    tokio::time::sleep(Duration::from_secs(2)).await;
    let mut everything = reply.to_string()
        + &w.standin.cli("standin.state", json!({})).await.to_string()
        + &p.call("chat.status", json!({})).await.unwrap().to_string();
    everything += &p
        .seen
        .lock()
        .unwrap()
        .events
        .iter()
        .map(|e| e.to_string())
        .collect::<String>();
    while let Ok(e) = admin.try_recv() {
        everything += &e.to_string();
    }
    everything += &w.bridge.status().to_string();
    fn files(dir: &std::path::Path, out: &mut Vec<u8>) {
        for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            if e.path().is_dir() {
                files(&e.path(), out)
            } else if let Ok(b) = std::fs::read(e.path()) {
                out.extend(b)
            }
        }
    }
    let mut on_disk = vec![];
    files(&w.dir, &mut on_disk);
    assert!(on_disk.len() > 100, "the state folder was read");
    for needle in [SECRET, "correct-horse", "9471"] {
        assert!(
            !everything.contains(needle),
            "the hidden answer leaked into a reply, an event or a state dump"
        );
        assert!(
            !on_disk
                .windows(needle.len())
                .any(|w| w == needle.as_bytes()),
            "the hidden answer is in a file"
        );
    }

    // a phone paired without a screen-lock key cannot give a hard Yes at all
    w.remove(&other.phone.id()).ok();
    let nolock = TestPhone::start("nolock", &w.shared, true, None, false).await;
    w.pair(&nolock).await;
    let id = ask("owner_question", "ferb.confirm").await["approval_id"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(wait(10, || nolock.approval(&id).is_some()).await);
    let e = nolock
        .call("chat.approve", json!({"approval_id": id, "granted": true}))
        .await
        .unwrap_err();
    assert_eq!(e.code, errors::UNLOCK_REQUIRED);
    assert!(e.message.contains("paired without"), "{e}");
    // nor a hidden input
    let id = hidden("owner_secret").await;
    assert!(wait(10, || nolock.approval(&id).is_some()).await);
    assert_eq!(nolock.approval(&id).unwrap()["phone"]["can_answer"], false);
    let e = nolock
        .call("chat.answer", json!({"approval_id": id, "text": "pw"}))
        .await
        .unwrap_err();
    assert_eq!(e.code, errors::UNLOCK_REQUIRED);
    assert_eq!(answered(id).await, "pending");
}

// decision: PHN-137, PHN-129
#[tokio::test(flavor = "multi_thread")]
async fn a_hard_yes_takes_the_pin_only_on_a_phone_without_a_fingerprint() {
    let (_relay, w) = shared_world().await;
    let p = TestPhone::start_with(
        "no fingerprint",
        &w.shared,
        w.force_relay,
        None,
        true,
        |c| c.strong_pub = None,
    )
    .await;
    w.pair(&p).await;
    let ch = p.phone.challenge().await.unwrap();
    p.phone
        .call(
            "link.unlock",
            json!({}),
            Some(p.pin_proof_for(&ferb_link::client::Phone::call_payload(
                &ch,
                "link.unlock",
                &json!({}),
            ))),
            |_| {},
        )
        .await
        .unwrap();
    let asked = w.standin.cli("standin.ask", json!({"kind": "owner_question", "capability": "ferb.confirm", "reason": "Send 500 to Alex?", "exact": "pay 500"})).await;
    let id = asked["approval_id"].as_str().unwrap().to_string();
    assert!(
        wait(10, || p.approval(&id).is_some()).await,
        "question not pushed"
    );
    let ap = p.approval(&id).unwrap();
    assert_eq!(
        (&ap["phone"]["needs_unlock"], &ap["phone"]["fingerprint"]),
        (&json!(true), &json!(false)),
        "{ap}"
    );
    let yes = json!({"approval_id": id, "granted": true});
    p.phone
        .call(
            "chat.approve",
            yes.clone(),
            Some(p.pin_proof(
                &id,
                ap["phone"]["challenge"].as_str().unwrap(),
                "chat.approve",
                &yes,
            )),
            |_| {},
        )
        .await
        .unwrap();
    let st = w.standin.cli("standin.state", json!({})).await;
    assert_eq!(
        st["approvals"]
            .as_array()
            .unwrap()
            .iter()
            .find(|a| a["approval_id"] == id)
            .unwrap()["state"],
        "granted"
    );
}

// decision: PHN-313
#[tokio::test(flavor = "multi_thread")]
async fn a_method_only_one_side_knows_names_the_side_that_is_behind() {
    use ferb_link::{LEVEL, client::explain_refusal, errors::LinkError};
    let (_relay, w) = shared_world().await;
    w.limits_set(&json!({"devices_max": 3})).unwrap();
    // an app newer than this Mac's link asks for something the Mac does not know yet
    let newer = TestPhone::start_with("newer app", &w.shared, w.force_relay, None, true, |c| {
        c.level = LEVEL + 1
    })
    .await;
    w.pair(&newer).await;
    let e = newer
        .call("link.future_thing", json!({}))
        .await
        .unwrap_err();
    assert_eq!(e.code, errors::MAC_LINK_OLDER, "{e}");
    assert!(
        e.message.contains("on your Mac is older than this app")
            && e.message.contains("link.future_thing")
            && e.allow.as_deref().unwrap_or_default().contains("restart"),
        "{e}"
    );
    assert_eq!(
        newer.phone.phase()["mac"]["older"],
        true,
        "{}",
        newer.phone.phase()
    );
    // an app from before levels (it says none) asks this Mac for something it no longer does
    let older = TestPhone::start_with("older app", &w.shared, w.force_relay, None, true, |c| {
        c.level = 0
    })
    .await;
    w.pair(&older).await;
    let e = older.call("link.old_thing", json!({})).await.unwrap_err();
    assert_eq!(e.code, errors::APP_OLDER, "{e}");
    assert!(
        e.message
            .contains("This app is older than FERB on your Mac")
            && e.allow
                .as_deref()
                .unwrap_or_default()
                .contains("Update the FERB app"),
        "{e}"
    );
    // the Mac's owner sees which phone runs which build
    let devices = w.bridge.status()["devices"].clone();
    let app = |name: &str| {
        devices
            .as_array()
            .unwrap()
            .iter()
            .find(|d| d["name"] == name)
            .unwrap()["app_build"]
            .clone()
    };
    assert_eq!(
        (&app("newer app")["newer"], &app("older app")["older"]),
        (&json!(true), &json!(true)),
        "{devices}"
    );
    // the same build on both sides: a method neither knows is just not a method
    let same = w.phone("same").await;
    w.pair(&same).await;
    assert_eq!(
        common::code(same.call("link.no_such_thing", json!({})).await),
        errors::METHOD_NOT_ALLOWED
    );
    assert_eq!(
        (
            &same.phone.phase()["mac"]["older"],
            &same.phone.phase()["mac"]["level"]
        ),
        (&json!(false), &json!(LEVEL))
    );

    // a Mac whose link is from before levels (the build that was still running when the app asked for a folder):
    // its reply, word for word, read by this app
    let old_mac = LinkError::new(
        errors::METHOD_NOT_ALLOWED,
        "\"link.tree\" is not something FERB's phone link knows.",
    )
    .allow("Do it in FERB on the Mac; a new FERB feature may need a newer phone link");
    let e = explain_refusal(old_mac.clone(), 0, LEVEL);
    assert_eq!(e.code, errors::MAC_LINK_OLDER);
    assert!(
        e.message.contains("link.tree")
            && e.message.contains("older than this app")
            && !e
                .allow
                .as_deref()
                .unwrap_or_default()
                .contains("Do it in FERB on the Mac"),
        "{e}"
    );
    // from a Mac that is not behind, and for any other refusal, nothing is reworded
    assert_eq!(
        explain_refusal(old_mac.clone(), LEVEL, LEVEL).message,
        old_mac.message
    );
    let daemon = LinkError::new(
        errors::METHOD_NOT_ALLOWED,
        "FERB refused \"x.y\" for a phone (no).",
    );
    assert_eq!(
        explain_refusal(daemon.clone(), 0, LEVEL).message,
        daemon.message
    );
}

// (lab SEC-4: revocation at once)
#[tokio::test(flavor = "multi_thread")]
async fn revoking_a_phone_cuts_its_live_connection_and_it_stays_out() {
    let (_relay, w) = shared_world().await;
    let p = w.paired_phone("phone").await;
    let p2 = p.phone.clone();
    let stream = tokio::spawn(async move {
        p2.call("chat.send", json!({"text": "long 60"}), None, |_| {})
            .await
    });
    running(&p, "long 60").await;
    let t = Instant::now();
    w.remove(&p.phone.id()).unwrap();
    assert!(p.wait_phase("stopped", 10).await, "{}", p.phone.phase());
    println!("MEASURE revoke -> phone stopped: {:?}", t.elapsed());
    assert_eq!(p.stop_code(), errors::DEVICE_REVOKED);
    assert!(stream.await.unwrap().is_err(), "the running stream ends");
    assert_eq!(
        common::code(p.call("chat.status", json!({})).await),
        errors::DEVICE_REVOKED
    );
    assert!(w.bridge.devices().is_empty());
    // it does not dial again by itself; when made to, it is turned away
    let dials = p.phone.dial_times().len();
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert_eq!(p.phone.dial_times().len(), dials);
    // a fresh start with the saved state (as after an app restart): the pairing is gone on the phone too
    let saved = p.saved().unwrap();
    assert!(saved.mac.is_some_and(|m| !m.paired));
}

// decision: PHN-202, PHN-205, PHN-207, PHN-444, PHN-461, PHN-302
#[tokio::test(flavor = "multi_thread")]
async fn tiers_decide_what_bulk_may_cross() {
    // public relay S; own relay O (with an access token); the Mac starts without an own relay
    let s = Relay::start(None, None).await;
    let o = Relay::start(Some("tok-123"), None).await;
    let w = World::start(Shared::Custom(vec![s.spec()]), None, true).await;
    let p = w.paired_phone("phone").await;
    assert_eq!(p.tier(), "shared_relay");
    let bulk = |p: &TestPhone| -> Vec<serde_json::Value> {
        p.seen
            .lock()
            .unwrap()
            .events
            .iter()
            .filter(|e| e["type"] == "bulk")
            .cloned()
            .collect()
    };
    assert_eq!(
        bulk(&p).last().map(|e| e["allowed"].clone()),
        Some(json!(false)),
        "the app is told the path carries control only: {:?}",
        bulk(&p)
    );
    let task = |text: &'static str| {
        let ph = p.phone.clone();
        tokio::spawn(async move {
            ph.call("chat.send", json!({"text": text}), None, |_| {})
                .await
        })
    };
    let _long = task("long 120");
    let tid = running(&p, "long 120").await;

    // --- the public relay: no picture, no preview, no file either way, before anything is read or begun
    let calls_before = w.standin.cli("standin.state", json!({})).await["calls"]
        .as_array()
        .unwrap()
        .len();
    for (params, words) in [
        (
            json!({"what": "preview", "task_id": tid, "live": true}),
            "Live preview",
        ),
        (
            json!({"what": "preview", "task_id": tid}),
            "A picture of what FERB is doing",
        ),
    ] {
        let e = p.phone.get(params, |_, _| {}).await.unwrap_err();
        assert_eq!(e.code, errors::NEEDS_BETTER_PATH);
        assert_eq!(
            e.message,
            format!(
                "{words} needs a direct connection or your own relay; you are on the shared relay."
            )
        );
        assert!(e.allow.is_some());
    }
    p.phone
        .call("chat.send", json!({"text": "file 1000"}), None, |_| {})
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
    assert_eq!(
        (e.code.as_str(), e.message.as_str()),
        (
            errors::NEEDS_BETTER_PATH,
            "This file needs a direct connection or your own relay; you are on the shared relay."
        )
    );
    p.unlock().await.unwrap();
    assert_eq!(
        common::code(p.phone.put("small.bin", &[1u8; 10], |_, _| {}).await),
        errors::NEEDS_BETTER_PATH
    );
    assert_eq!(
        common::code(p.call("link.put_group", json!({"name": "x"})).await),
        errors::NEEDS_BETTER_PATH
    );
    assert_eq!(
        common::code(p.call("link.tree", json!({"path": w.dir})).await),
        errors::NEEDS_BETTER_PATH
    );
    let st = w.standin.cli("standin.state", json!({})).await;
    let asked: Vec<&serde_json::Value> = st["calls"].as_array().unwrap()[calls_before..]
        .iter()
        .filter(|c| {
            matches!(
                c[2].as_str(),
                Some(
                    "tasks.preview"
                        | "phone.read"
                        | "phone.put_begin"
                        | "phone.put_group"
                        | "phone.tree"
                )
            )
        })
        .collect();
    assert!(
        asked.is_empty(),
        "FERB was asked for bulk the public relay may not carry: {asked:?}"
    );
    // control is untouched by all of it
    assert!(p.call("chat.status", json!({})).await.is_ok());

    // --- "use my own relay": tested, saved, told to the phone, and the tier follows
    let mut wrong = o.spec();
    wrong.token = Some("wrong".into());
    assert_eq!(
        common::code(w.bridge.relay_set(Some(wrong)).await),
        errors::OWN_RELAY_ACCESS_DENIED
    );
    assert!(
        w.bridge.status()["own_relay"].is_null(),
        "a relay that fails its test is not saved"
    );
    w.bridge.relay_set(Some(o.spec())).await.unwrap();
    assert!(
        wait(30, || p.tier() == "own_relay").await,
        "dials {} saved {:?} phone: {} mac: {}",
        p.phone.dial_times().len(),
        p.saved(),
        p.phone.phase(),
        w.bridge.status()["relay"]
    );
    assert_eq!(
        p.saved().unwrap().mac.unwrap().own,
        Some(o.spec()),
        "the phone learned the relay (and its token) from the Mac"
    );
    assert_eq!(w.bridge.status()["relay"]["state"], "own");
    // the path improved: the app is told it may offer what was refused again
    assert!(
        wait(10, || bulk(&p).last().is_some_and(
            |e| e["allowed"] == true && e["tier"] == "own_relay"
        ))
        .await,
        "{:?}",
        bulk(&p)
    );

    // --- own relay: live preview, a 300 kB picture, an 8 MB file, a 3 MB upload with a message
    w.standin
        .cli("standin.preview_size", json!({"bytes": 300_000}))
        .await;
    let _long2 = task("long 121");
    let tid = running(&p, "long 121").await;
    let (_, bytes) = p
        .phone
        .get(
            json!({"what": "preview", "task_id": tid, "live": true}),
            |_, _| {},
        )
        .await
        .unwrap();
    assert_eq!(bytes.len(), 300_000);
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
    let (meta, b) = p
        .phone
        .get(
            json!({"what": "file", "task_id": last, "index": 0}),
            |_, _| {},
        )
        .await
        .unwrap();
    assert_eq!(
        (b.len(), meta["tier"].as_str().unwrap()),
        (8_000_000, "own_relay")
    );
    println!(
        "MEASURE 8 MB file over the own relay (local): {:?}",
        t.elapsed()
    );
    p.unlock().await.unwrap();
    let up = p
        .phone
        .put("photo.jpg", &vec![7u8; 3_000_000], |_, _| {})
        .await
        .unwrap();
    p.phone
        .call(
            "chat.send",
            json!({"text": "look at this", "attachments": [up["upload_id"]]}),
            None,
            |_| {},
        )
        .await
        .unwrap();
    let st = w.standin.cli("standin.state", json!({})).await;
    let msg = st["messages"].as_array().unwrap().last().unwrap().clone();
    let path = msg["input_paths"][0].as_str().unwrap();
    assert!(
        path.contains("/mac/inbox/") && std::fs::metadata(path).unwrap().len() == 3_000_000,
        "{msg}"
    );
    drop(s);
}

// decision: PHN-202, PHN-207
#[tokio::test(flavor = "multi_thread")]
async fn a_direct_path_is_detected_and_has_no_small_caps() {
    // not forced through the relay: both ends on this Mac find each other directly
    let relay = Relay::start(None, None).await;
    let w = World::start(Shared::Custom(vec![relay.spec()]), None, false).await;
    let p = w.paired_phone("phone").await;
    assert!(
        wait(20, || p.tier() == "direct").await,
        "tier stayed {}",
        p.tier()
    );
    assert!(p.call("link.status", json!({})).await.unwrap()["path"] == "direct");
    p.phone
        .call("chat.send", json!({"text": "file 6000000"}), None, |_| {})
        .await
        .unwrap();
    let st = p.call("chat.status", json!({})).await.unwrap();
    let last = st["results"].as_array().unwrap().last().unwrap()["task_id"]
        .as_str()
        .unwrap()
        .to_string();
    let (meta, b) = p
        .phone
        .get(
            json!({"what": "file", "task_id": last, "index": 0}),
            |_, _| {},
        )
        .await
        .unwrap();
    assert_eq!(
        (b.len(), meta["tier"].as_str().unwrap()),
        (6_000_000, "direct")
    );
    assert_eq!(
        w.bridge.status()["shared_relay"]["bulk"],
        "refused",
        "the public relays carry no bulk (kb PHN-444)"
    );
}

// decision: PHN-309, PHN-305
#[tokio::test(flavor = "multi_thread")]
async fn floods_and_bad_frames_do_not_hurt_the_link() {
    let (relay, w) = shared_world().await;
    w.limits_set(&json!({"stall_s": 2, "calls_per_10s": 30, "connects_per_min": 4}))
        .unwrap();
    let p = w.paired_phone("phone").await;
    let conn = p.phone.connection().unwrap();

    // a frame header claiming 4 GiB: refused before anything is allocated; the connection stays usable
    let (mut tx, mut rx) = conn.open_bi().await.unwrap();
    tx.write_all(&[0xff, 0xff, 0xff, 0xff, wire::K_REQ])
        .await
        .unwrap();
    tx.write_all(&[0u8; 1000]).await.ok();
    let (kind, body) = wire::read_frame(&mut rx, 65536, Duration::from_secs(5), false)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(kind, wire::K_ERROR);
    assert_eq!(
        wire::parse::<errors::LinkError>(&body).unwrap().code,
        errors::FRAME_TOO_LARGE
    );
    // one byte over the control limit
    let (mut tx, mut rx) = conn.open_bi().await.unwrap();
    let big = json!({"method": "chat.send", "params": {"text": "x".repeat(64 * 1024)}});
    wire::write_json(&mut tx, wire::K_REQ, &big).await.ok();
    let (_, body) = wire::read_frame(&mut rx, 65536, Duration::from_secs(5), false)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        wire::parse::<errors::LinkError>(&body).unwrap().code,
        errors::FRAME_TOO_LARGE
    );
    assert!(p.call("ping", json!({})).await.is_ok());

    // slow loris: two bytes of a header, then silence: that stream is reset after the stall time, nothing else
    let (mut tx, mut rx) = conn.open_bi().await.unwrap();
    let t = Instant::now();
    tx.write_all(&[0, 0]).await.unwrap();
    let mut b = [0u8; 1];
    let r = tokio::time::timeout(Duration::from_secs(8), rx.read_exact(&mut b)).await;
    assert!(
        matches!(r, Ok(Err(_))),
        "the stalled stream must be reset: {r:?}"
    );
    println!(
        "MEASURE slow-loris stream reset after {:?} (stall_s = 2)",
        t.elapsed()
    );
    assert!(p.call("ping", json!({})).await.is_ok());

    // streams: the phone cannot hold more than the Mac allows (8); the rest wait, nothing breaks
    tokio::time::sleep(Duration::from_millis(500)).await;
    let mut held = vec![];
    for _ in 0..12 {
        match tokio::time::timeout(Duration::from_millis(400), conn.open_bi()).await {
            Ok(Ok((mut tx, rx))) => {
                tx.write_all(&[0]).await.unwrap(); // makes the stream exist on the Mac; it is reset as stalled later
                held.push((tx, rx));
            }
            _ => break,
        }
    }
    assert!(
        held.len() <= 8,
        "the phone held {} streams at once; the Mac allows 8",
        held.len()
    );
    assert!(held.len() >= 3, "{}", held.len());
    println!(
        "MEASURE streams a phone could hold open at once: {}",
        held.len()
    );
    drop(held);
    tokio::time::sleep(Duration::from_millis(2500)).await;
    assert!(p.call("ping", json!({})).await.is_ok());

    // a flood of calls: over the limit the answer is too_many_requests with a wait, and it clears
    let mut codes = vec![];
    for _ in 0..40 {
        codes.push(common::code(p.call("ping", json!({})).await));
    }
    let refused = codes
        .iter()
        .filter(|c| *c == errors::TOO_MANY_REQUESTS)
        .count();
    assert!((10..40).contains(&refused), "refused {refused} of 40");
    let e = p.call("ping", json!({})).await.unwrap_err();
    assert!(e.retry_after_s.is_some());

    // 12 silent strangers while a pairing window is open (4 slots): the paired phone is not delayed
    w.bridge.pair_start().unwrap();
    let mac_id: iroh::EndpointId = w.bridge.id().parse().unwrap();
    let addr = ferb_link::net::dial_addr(mac_id, &[relay.url().parse().unwrap()]);
    let stranger = ferb_link::net::bind(&ferb_link::net::NetConfig {
        secret: iroh::SecretKey::from_bytes(&[5; 32]),
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
    ferb_link::net::wait_home(&stranger, Duration::from_secs(10)).await;
    let mut silent = vec![];
    for _ in 0..12 {
        silent.push(
            stranger
                .connect(addr.clone(), ferb_link::net::ALPN)
                .await
                .unwrap(),
        );
    }
    tokio::time::sleep(Duration::from_millis(300)).await;
    let busy = silent.iter().filter(|c| c.close_reason().is_some()).count();
    assert!(
        busy >= 8,
        "at least 8 of 12 must be turned away at once, got {busy}"
    );
    tokio::time::sleep(Duration::from_secs(11)).await; // also lets the call window clear
    let t = Instant::now();
    assert!(p.call("ping", json!({})).await.is_ok());
    println!(
        "MEASURE paired phone's call during the stranger flood: {:?}",
        t.elapsed()
    );
    for c in &silent {
        assert!(
            tokio::time::timeout(Duration::from_secs(8), c.closed())
                .await
                .is_ok(),
            "a silent stranger must be closed"
        );
    }
    w.bridge.pair_cancel();

    // a reconnect storm from a paired phone (limit 4 a minute here): told to slow down, and the client obeys
    let state = p.saved();
    p.phone.close().await;
    let mut last = None;
    for _ in 0..6 {
        let again = TestPhone::start("phone", &w.shared, true, state.clone(), true).await;
        again.wait_phase("online", 10).await;
        if again.phase() != "online" {
            last = Some(again);
            break;
        }
        again.phone.close().await;
    }
    let storm = last.expect("the storm was never refused");
    assert!(
        storm.wait_phase("waiting", 10).await,
        "{}",
        storm.phone.phase()
    );
    assert_eq!(storm.stop_code(), errors::TOO_MANY_REQUESTS);
    assert!(
        storm.phone.phase()["retry_in_s"].as_u64().unwrap() >= 20,
        "the phone must wait as told: {}",
        storm.phone.phase()
    );
    let dials = storm.phone.dial_times().len();
    tokio::time::sleep(Duration::from_secs(4)).await;
    assert_eq!(
        storm.phone.dial_times().len(),
        dials,
        "no dial while told to wait"
    );
}

/// The planner's question that waits (`kind` "clarify", answered by a message) is in the status of an app that knows
/// it, pushed and asked for; an app from before (level 3) is never sent one, so it never answers it as a gateway
/// question. Any message, from either phone, ends it.
// decision: PHN-763
#[tokio::test(flavor = "multi_thread")]
async fn the_planner_s_question_reaches_only_an_app_that_knows_it() {
    use ferb_link::LEVEL_CLARIFY;
    let (_relay, w) = shared_world().await;
    w.limits_set(&json!({"devices_max": 3})).unwrap();
    let new = w.paired_phone("new app").await;
    let old = TestPhone::start_with("old app", &w.shared, w.force_relay, None, true, |c| {
        c.level = LEVEL_CLARIFY - 1
    })
    .await;
    w.pair(&old).await;
    old.unlock().await.expect("unlock");
    let clarify = |s: &Value| {
        s["approvals"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|a| a["kind"] == "clarify")
            .cloned()
            .collect::<Vec<_>>()
    };
    new.call("chat.send", json!({"text": "clarify Which account?"}))
        .await
        .unwrap();
    assert!(
        wait(10, || clarify(&new.status()).len() == 1).await,
        "not pushed: {}",
        new.status()
    );
    let q = clarify(&new.status())[0].clone();
    assert!(
        q["approval_id"].as_str().unwrap().starts_with("clarify:"),
        "{q}"
    );
    assert_eq!(q["reason"], "Which account?");
    assert_eq!(
        clarify(&new.call("chat.status", json!({})).await.unwrap()).len(),
        1
    );
    // the older app: neither pushed nor answered
    assert!(clarify(&old.call("chat.status", json!({})).await.unwrap()).is_empty());
    assert!(clarify(&old.status()).is_empty(), "{}", old.status());
    // the owner answers from the other phone: it waits no more anywhere
    old.call("chat.send", json!({"text": "the work one"}))
        .await
        .unwrap();
    assert!(
        wait(10, || clarify(&new.status()).is_empty()).await,
        "still pushed: {}",
        new.status()
    );
}
