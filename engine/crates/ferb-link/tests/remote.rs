#![cfg(target_os = "macos")]
//! The phone as the owner's full remote (SEC-11..SEC-14): every FERB method classified, sensitive calls signed per
//! call, the owner's data behind a recent unlock, files with path safety and the protected-folder question, the
//! audit, large replies as bulk. All on 127.0.0.1.
mod common;
use common::*;
use ferb_link::{
    errors,
    net::Shared,
    role::{self, Class},
};
use serde_json::{Value, json};
use std::time::Duration;

async fn world() -> (Relay, World, TestPhone) {
    let relay = Relay::start(None, None).await;
    let w = World::start(Shared::Custom(vec![relay.spec()]), None, false).await; // a direct path: no tier caps here
    let p = w.paired_phone("phone").await;
    (relay, w, p)
}
fn reached(st: &Value, method: &str) -> usize {
    st["calls"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| c[0] == "phone" && c[2] == method)
        .count()
}

// decision: PHN-111, PHN-112, PHN-114, PHN-117
#[tokio::test(flavor = "multi_thread")]
async fn the_class_of_a_call_decides_what_the_mac_checks() {
    let (_relay, w, p) = world().await;
    w.limits_set(&json!({"calls_per_10s": 1000})).unwrap();
    // plain: goes on
    assert!(p.call("schedules.list", json!({})).await.is_ok());
    assert!(p.call("about.get", json!({})).await.is_ok());

    // never: refused with its reason, and it is a different code from "unknown"
    // (the engine's table, kb PHN-465; a `phone.*` name is the link's own calls' business: refused as such first)
    for row in ferb_rpc::classes::PHONE_METHODS
        .iter()
        .filter(|r| r.class == Class::Never && !r.name.starts_with("phone."))
    {
        let (m, why) = (row.name, row.why);
        let e = p.call(m, json!({})).await.unwrap_err();
        assert_eq!(e.code, errors::NOT_FROM_PHONE, "{m}");
        assert!(e.message.contains(why) && e.allow.is_some(), "{e}");
        assert_eq!(
            w.standin
                .call_from("phone", &p.phone.id(), m, json!({}), None)
                .await
                .unwrap_err()["code"],
            -32001,
            "the daemon's own gate refuses {m} too"
        );
    }
    assert_eq!(
        common::code(p.call("made.up", json!({})).await),
        errors::METHOD_NOT_ALLOWED
    );
    // a model worker's tool bridge has a row of its own now (never), so it is refused with its reason
    assert_eq!(
        common::code(p.call("tools.call", json!({})).await),
        errors::NOT_FROM_PHONE
    );
    assert_eq!(
        common::code(
            p.call(
                "chat.send",
                json!({"text": "x", "input_paths": ["/etc/passwd"]})
            )
            .await
        ),
        errors::PARAM_NOT_ALLOWED
    );
    assert_eq!(
        common::code(
            p.call("chat.send", json!({"text": "x", "test_run": {}}))
                .await
        ),
        errors::PARAM_NOT_ALLOWED
    );

    // sensitive: every one of them is refused without a proof, and nothing reaches the daemon
    for m in ferb_rpc::classes::PHONE_METHODS
        .iter()
        .filter(|r| r.class == Class::Sensitive && !r.stream && !r.name.starts_with("phone."))
        .map(|r| r.name)
    {
        assert_eq!(
            common::code(p.call(m, json!({"key": "x"})).await),
            errors::UNLOCK_REQUIRED,
            "{m}"
        );
    }
    let st = w.standin.cli("standin.state", json!({})).await;
    assert!(
        st["calls"]
            .as_array()
            .unwrap()
            .iter()
            .all(|c| c[0] != "phone"
                || role::class(c[2].as_str().unwrap()) != Some(Class::Sensitive)),
        "a sensitive call got through unsigned"
    );
    // signed for exactly this call: it goes through, once
    let params = json!({"key": "privacy.exclusions", "value": ["bank"]});
    let ch = p.phone.challenge().await.unwrap();
    let proof = p.proof_for(&ferb_link::client::Phone::call_payload(
        &ch,
        "settings.set",
        &params,
    ));
    assert!(
        p.phone
            .call("settings.set", params.clone(), Some(proof.clone()), |_| {})
            .await
            .is_ok()
    );
    assert_eq!(
        reached(
            &w.standin.cli("standin.state", json!({})).await,
            "settings.set"
        ),
        1
    );
    // replayed
    assert_eq!(
        common::code(
            p.phone
                .call("settings.set", params.clone(), Some(proof), |_| {})
                .await
        ),
        errors::UNLOCK_INVALID
    );
    // signed for other parameters, or another method
    let ch = p.phone.challenge().await.unwrap();
    let other = p.proof_for(&ferb_link::client::Phone::call_payload(
        &ch,
        "settings.set",
        &json!({"key": "privacy.exclusions", "value": []}),
    ));
    assert_eq!(
        common::code(
            p.phone
                .call("settings.set", params.clone(), Some(other), |_| {})
                .await
        ),
        errors::UNLOCK_INVALID
    );
    let ch = p.phone.challenge().await.unwrap();
    let wrong_method = p.proof_for(&ferb_link::client::Phone::call_payload(
        &ch,
        "stop.lift",
        &json!({}),
    ));
    assert_eq!(
        common::code(
            p.phone
                .call("keys.delete", json!({}), Some(wrong_method), |_| {})
                .await
        ),
        errors::UNLOCK_INVALID
    );
    // a challenge the Mac never gave, another phone's key, an expired challenge
    let made_up = p.proof_for(&ferb_link::client::Phone::call_payload(
        "AAAAAAAAAAAAAAAAAAAAAA",
        "stop.lift",
        &json!({}),
    ));
    assert_eq!(
        common::code(
            p.phone
                .call("stop.lift", json!({}), Some(made_up), |_| {})
                .await
        ),
        errors::UNLOCK_INVALID
    );
    let stranger = TestPhone::start("x", &w.shared, false, None, true).await;
    let ch = p.phone.challenge().await.unwrap();
    let forged = stranger.proof_for(&ferb_link::client::Phone::call_payload(
        &ch,
        "stop.lift",
        &json!({}),
    ));
    assert_eq!(
        common::code(
            p.phone
                .call("stop.lift", json!({}), Some(forged), |_| {})
                .await
        ),
        errors::UNLOCK_INVALID
    );
    w.limits_set(&json!({"challenge_ttl_s": 1})).unwrap();
    let ch = p.phone.challenge().await.unwrap();
    tokio::time::sleep(Duration::from_millis(1600)).await;
    let late = p.proof_for(&ferb_link::client::Phone::call_payload(
        &ch,
        "stop.lift",
        &json!({}),
    ));
    assert_eq!(
        common::code(
            p.phone
                .call("stop.lift", json!({}), Some(late), |_| {})
                .await
        ),
        errors::UNLOCK_INVALID
    );
    w.limits_set(&json!({"challenge_ttl_s": 120})).unwrap();
    assert_eq!(
        reached(
            &w.standin.cli("standin.state", json!({})).await,
            "stop.lift"
        ),
        0
    );
    assert!(p.sensitive("stop.lift", json!({})).await.is_ok());
    // the six that open something on the Mac's screen: callable (signed), and the reply says a click is needed there
    for m in ferb_rpc::classes::PHONE_METHODS
        .iter()
        .filter(|r| r.on_mac_screen)
        .map(|r| r.name)
    {
        assert_eq!(role::class(m), Some(Class::Sensitive), "{m}");
        let r = p.sensitive(m, json!({})).await.unwrap();
        assert!(
            r["on_mac_screen"]
                .as_str()
                .unwrap()
                .contains("whether it should click it"),
            "{m}: {r}"
        );
    }
    assert_eq!(
        ferb_rpc::classes::PHONE_METHODS
            .iter()
            .filter(|r| r.on_mac_screen)
            .count(),
        6
    );
    // a key (a secret) sent through a sensitive call is in no audit line and no state file
    p.sensitive(
        "keys.store",
        json!({"name": "anthropic", "value": "sk-test-4471-not-a-real-key"}),
    )
    .await
    .unwrap();

    // locked until the phone is unlocked towards the Mac, chat and tasks included (the owner, kb SEC-25); what a
    // locked phone needs still works: it learns that something waits, and it can stop FERB. Locks again when idle.
    w.limits_set(&json!({"unlock_idle_s": 2})).unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    p.call("link.lock", json!({})).await.unwrap();
    for m in ["chat.status", "ping", "stop.engage"] {
        assert!(
            p.call(m, json!({})).await.is_ok(),
            "{m} must work for a locked phone"
        );
    }
    assert!(
        p.sensitive("stop.lift", json!({})).await.is_ok(),
        "a call signed by itself needs no window"
    );
    assert_eq!(
        common::code(
            p.phone
                .call("chat.send", json!({"text": "hello"}), None, |_| {})
                .await
        ),
        errors::LOCKED
    );
    assert_eq!(
        common::code(
            p.phone
                .get(json!({"what": "preview", "task_id": "t"}), |_, _| {})
                .await
                .map(|x| x.1.len())
        ),
        errors::LOCKED
    );
    assert_eq!(
        common::code(
            p.phone
                .get(
                    json!({"what": "file", "task_id": "t", "index": 0}),
                    |_, _| {}
                )
                .await
                .map(|x| x.1.len())
        ),
        errors::LOCKED
    );
    for m in [
        "schedules.list",
        "tasks.list",
        "about.get",
        "settings.get",
        "memory.list",
        "trace.get",
        "secrets.saved",
        "seen.list",
    ] {
        let e = p.call(m, json!({})).await.unwrap_err();
        assert_eq!(e.code, errors::LOCKED, "{m}");
    }
    assert_eq!(
        common::code(p.call("link.unlock", json!({})).await),
        errors::UNLOCK_REQUIRED,
        "unlocking needs the proof too"
    );
    p.unlock().await.unwrap();
    assert!(p.call("settings.get", json!({})).await.is_ok());
    tokio::time::sleep(Duration::from_millis(1200)).await;
    assert!(
        p.call("memory.list", json!({})).await.is_ok(),
        "use keeps it unlocked"
    );
    tokio::time::sleep(Duration::from_millis(1200)).await;
    assert!(p.call("memory.list", json!({})).await.is_ok());
    tokio::time::sleep(Duration::from_millis(2600)).await;
    assert_eq!(
        common::code(p.call("settings.get", json!({})).await),
        errors::LOCKED,
        "idle: locked again"
    );
    p.unlock().await.unwrap();
    p.call("link.lock", json!({})).await.unwrap();
    assert_eq!(
        common::code(p.call("settings.get", json!({})).await),
        errors::LOCKED
    );
    w.limits_set(&json!({"unlock_idle_s": 300, "unlock_max_s": 2}))
        .unwrap();
    // a hard end as well: however much it is used, one unlock does not outlive unlock_max_s
    p.unlock().await.unwrap();
    for _ in 0..3 {
        tokio::time::sleep(Duration::from_millis(900)).await;
        let _ = p.call("settings.get", json!({})).await;
    }
    assert_eq!(
        common::code(p.call("settings.get", json!({})).await),
        errors::LOCKED,
        "the unlock outlived its maximum"
    );
    w.limits_set(&json!({"unlock_max_s": 900})).unwrap();

    // a reply far larger than a control frame arrives whole, as bulk
    p.unlock().await.unwrap();
    let big = p
        .call("trace.get", json!({"task_id": "t", "steps": 60000}))
        .await
        .unwrap();
    assert_eq!(big["trace"].as_str().unwrap().len(), 300_000);

    // a phone paired without a screen lock can prove no unlock: it sees that something waits and can stop FERB,
    // nothing more (the app itself does not open on such a phone)
    w.limits_set(&json!({"devices_max": 3})).unwrap();
    let nolock = TestPhone::start("nolock", &w.shared, false, None, false).await;
    w.pair(&nolock).await;
    assert!(
        nolock.call("chat.status", json!({})).await.is_ok()
            && nolock
                .call("task.stop", json!({"task_ids": ["t"]}))
                .await
                .is_ok()
    );
    assert_eq!(
        common::code(nolock.call("schedules.list", json!({})).await),
        errors::LOCKED
    );
    assert_eq!(
        common::code(nolock.sensitive("stop.lift", json!({})).await),
        errors::UNLOCK_REQUIRED
    );
    assert_eq!(common::code(nolock.unlock().await), errors::UNLOCK_REQUIRED);
    assert_eq!(
        common::code(nolock.call("settings.get", json!({})).await),
        errors::LOCKED
    );

    // the audit on the Mac: what was asked and how it ended; the phone cannot read or change it
    let audit = w.bridge.audit(1000);
    let has = |m: &str, r: &str| {
        audit
            .iter()
            .any(|a| a["method"] == m && a["result"] == r && a["device"] == json!(p.phone.id()))
    };
    assert!(
        has("settings.set", "ok")
            && has("settings.set", errors::UNLOCK_INVALID)
            && has("stop.lift", "ok")
            && has("settings.get", errors::LOCKED)
            && has("link.unlock", "ok")
            && has("notifications.subscribe", errors::NOT_FROM_PHONE)
            && has("made.up", errors::METHOD_NOT_ALLOWED),
        "{audit:?}"
    );
    assert!(
        audit
            .iter()
            .any(|a| a["method"] == "settings.set" && a["class"] == "sensitive")
    );
    assert!(
        audit.iter().any(|a| a["method"] == "mac.pair_request")
            && audit
                .iter()
                .any(|a| a["method"] == "mac.pair_ended" && a["paired"] == true),
        "pairing is in the audit: {audit:?}"
    );
    for m in [
        "link.audit",
        "audit.get",
        "link.limits_set",
        "link.revoke",
        "link.relay_set",
    ] {
        assert_eq!(
            common::code(p.call(m, json!({})).await),
            errors::METHOD_NOT_ALLOWED,
            "{m}: the owner's side is not reachable from a phone"
        );
    }
    let on_disk = std::fs::read_to_string(w.dir.join("mac/audit.jsonl")).unwrap()
        + &std::fs::read_to_string(w.dir.join("mac/link.json")).unwrap();
    assert!(
        !on_disk.contains("sk-test-4471") && !on_disk.contains("privacy.exclusions"),
        "a parameter's value is in the audit or the state"
    );
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(
        std::fs::metadata(w.dir.join("mac/audit.jsonl"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
}

// decision: PHN-444, PHN-461, PHN-301, PHN-302, PHN-113
#[tokio::test(flavor = "multi_thread")]
async fn the_public_relay_carries_text_questions_and_stop_only() {
    let relay = Relay::start(None, None).await;
    let w = World::start(Shared::Custom(vec![relay.spec()]), None, true).await;
    let p = w.paired_phone("phone").await;
    assert_eq!(p.tier(), "shared_relay");
    let small = w.dir.join("small.bin");
    std::fs::write(&small, vec![1u8; 2_000]).unwrap();
    p.unlock().await.unwrap();
    let refused = |r: Result<serde_json::Value, errors::LinkError>| {
        r.map(|_| "ok".to_string()).unwrap_or_else(|e| e.code)
    };
    // even a small file: files never cross the public relays (no size makes one allowed)
    let e = p
        .phone
        .get(json!({"what": "path", "path": small}), |_, _| {})
        .await
        .unwrap_err();
    assert_eq!(e.code, errors::NEEDS_BETTER_PATH);
    assert!(
        e.message.contains("the shared relay") && e.allow.is_some(),
        "{e}"
    );
    // pictures: the screenshot, a question's picture, a remembered image
    for m in ["tasks.preview", "questions.picture", "seen.image"] {
        assert_eq!(
            refused(p.call(m, json!({})).await),
            errors::NEEDS_BETTER_PATH,
            "{m}"
        );
    }
    // a reply too large for one control frame is refused whole, never cut silently; a control-sized one is untouched
    assert_eq!(
        refused(p.call("trace.get", json!({"steps": 30_000})).await),
        errors::NEEDS_BETTER_PATH
    );
    assert_eq!(
        p.call("trace.get", json!({"steps": 2_000})).await.unwrap()["trace"]
            .as_str()
            .unwrap()
            .len(),
        10_000
    );
    // a folder's listing that fits one frame is control; one that would be cut is refused
    std::fs::create_dir_all(w.dir.join("many")).unwrap();
    for i in 0..600 {
        std::fs::write(
            w.dir.join(format!("many/{}-{i:04}.txt", "n".repeat(110))),
            b"x",
        )
        .unwrap();
    }
    assert_eq!(
        refused(
            p.call("link.files", json!({"path": w.dir.join("many")}))
                .await
        ),
        errors::NEEDS_BETTER_PATH
    );
    assert!(
        p.call("link.files", json!({"path": w.dir.join("standin-files")}))
            .await
            .is_ok()
    );
    // the chat: only its latest page, at most `shared_page_max` entries, cut by the engine to one frame
    let page = p.call("chat.entries.page", json!({})).await.unwrap();
    assert_eq!(
        (
            page["entries"].as_array().unwrap().len(),
            page["asked"]["limit"].as_u64()
        ),
        (50, Some(50)),
        "{}",
        page["asked"]
    );
    assert!(
        page["asked"]["max_bytes"]
            .as_u64()
            .is_some_and(|m| m < 65_536),
        "{}",
        page["asked"]
    );
    assert!(
        p.call(
            "chat.entries.page",
            json!({"limit": 20, "max_bytes": 10_000})
        )
        .await
        .unwrap()["asked"]["max_bytes"]
            == 10_000
    );
    for older in [json!({"before_pos": 900}), json!({"day": "2026-11-01"})] {
        assert_eq!(
            refused(p.call("chat.entries.page", older.clone()).await),
            errors::NEEDS_BETTER_PATH,
            "{older}"
        );
    }
    assert_eq!(
        refused(p.call("chat.entries.page", json!({"limit": 51})).await),
        errors::TRANSFER_TOO_LARGE
    );
    assert_eq!(
        refused(
            p.call(
                "chat.history.page",
                json!({"day": "2026-11-01", "before": "x"})
            )
            .await
        ),
        errors::NEEDS_BETTER_PATH
    );
    // the basics, and sensitive and private control calls, are never refused by the tier
    assert!(
        p.phone
            .call("chat.send", json!({"text": "hello"}), None, |_| {})
            .await
            .is_ok()
    );
    assert!(
        p.sensitive("settings.set", json!({"key": "a", "value": 1}))
            .await
            .is_ok()
    );
    assert!(p.call("settings.get", json!({})).await.is_ok());
    assert!(p.call("stop.engage", json!({})).await.is_ok());
    // the Mac says what this tier carries
    assert_eq!(w.bridge.status()["shared_relay"]["bulk"], "refused");
}

// decision: PHN-212, PHN-218
#[test]
fn the_why_line_says_only_what_was_observed() {
    use ferb_link::net::{NetFacts, Tier, why_not_direct};
    let ok = NetFacts {
        known: true,
        udp_v4: true,
        public_v4: true,
        ..Default::default()
    };
    assert_eq!(why_not_direct(Tier::Direct, &ok, Some(&ok)), None);
    assert!(
        why_not_direct(Tier::SharedRelay, &ok, None)
            .unwrap()
            .starts_with("Checking")
    );
    let varies = NetFacts {
        varies: Some(true),
        ..ok.clone()
    };
    let w = why_not_direct(Tier::SharedRelay, &ok, Some(&varies)).unwrap();
    assert!(
        w.contains("phone's network gives it a new public port for every destination")
            && !w.contains("impossible"),
        "{w}"
    );
    let v6 = NetFacts {
        udp_v6: true,
        public_v6: true,
        ..varies.clone()
    };
    assert!(
        why_not_direct(
            Tier::SharedRelay,
            &NetFacts {
                udp_v6: true,
                ..ok.clone()
            },
            Some(&v6)
        )
        .unwrap()
        .contains("neither network shows a known obstacle"),
        "IPv6 on both sides goes around a varying port"
    );
    let no_udp = NetFacts {
        known: true,
        captive: Some(true),
        ..Default::default()
    };
    let w = why_not_direct(Tier::SharedRelay, &no_udp, Some(&ok)).unwrap();
    assert!(
        w.starts_with("The Mac's network does not let") && w.contains("sign-in page"),
        "{w}"
    );
    // nothing known to be in the way: what was tried, and no cause nobody saw (kb NET-18)
    let tried = NetFacts {
        offers_public: Some(true),
        attempts: 3,
        ..ok.clone()
    };
    let w = why_not_direct(
        Tier::OwnRelay,
        &NetFacts {
            offers_public: Some(true),
            ..ok.clone()
        },
        Some(&tried),
    )
    .unwrap();
    assert!(
        w.starts_with("No limits apply here.")
            && w.contains("3 attempts have been made and nothing came through"),
        "{w}"
    );
    for said in [
        &w,
        &why_not_direct(Tier::SharedRelay, &ok, Some(&ok)).unwrap(),
    ] {
        assert!(
            !said.contains("probably")
                && !said.contains("firewall")
                && !said.contains("was not possible")
                && !said.contains("keeps trying"),
            "{said}"
        );
    }
    // a side that has no address the other could reach: that is the reason, and it is ours to say
    let none = NetFacts {
        offers_public: Some(false),
        ..ok.clone()
    };
    assert!(why_not_direct(Tier::OwnRelay, &tried, Some(&none)).unwrap().contains("The phone has not learned an address it can be reached at from outside its own network yet"));
    assert!(
        why_not_direct(Tier::SharedRelay, &none, Some(&tried))
            .unwrap()
            .starts_with("The Mac has not learned")
    );
    // a phone app from before these facts says nothing about its addresses: nothing is claimed about them
    assert!(
        why_not_direct(Tier::SharedRelay, &tried, Some(&ok))
            .unwrap()
            .starts_with("Each side knows")
    );
}

// decision: PHN-213, PHN-218
#[tokio::test(flavor = "multi_thread")]
async fn the_tier_follows_the_path_and_a_wrapped_relay_link_still_parses() {
    use ferb_link::net::RelaySpec;
    // a link pasted into a terminal arrives broken over lines and with spaces: it is still the same link
    let spec = RelaySpec {
        url: "https://192.0.2.7:7443".into(),
        token: Some("tok".into()),
        ca: Some("MIIBcertificatebytes-_x".repeat(8)),
    };
    let link = spec.to_link();
    let wrapped: String = link
        .as_bytes()
        .chunks(40)
        .map(|c| String::from_utf8_lossy(c).to_string())
        .collect::<Vec<_>>()
        .join("\n  ");
    assert_eq!(
        RelaySpec::from_text(&format!("  {wrapped}\n"), None).unwrap(),
        spec
    );

    // a session whose path changes under it: direct first, then only a relay is left (the Mac's test switch)
    let (_relay, w, p) = world().await;
    assert!(wait(20, || p.tier() == "direct").await, "{}", p.tier());
    let facts = p.call("link.status", json!({})).await.unwrap()["path_facts"].clone();
    assert_eq!(
        (facts["kind"].as_str(), facts["family"].is_string()),
        (Some("direct"), true),
        "{facts}"
    );
    assert!(
        !facts.to_string().contains("127.0.0.1") && !facts.to_string().contains("::1"),
        "the address itself is not reported: {facts}"
    );
    // each side says which kinds of address it offers the other (never the addresses) and how often it tried
    assert!(
        wait(10, || w.bridge.status()["devices"][0]["net"]["offers"]
            .as_array()
            .is_some_and(|o| !o.is_empty()))
        .await,
        "{}",
        w.bridge.status()["devices"][0]
    );
    let d = w.bridge.status()["devices"][0].clone();
    assert!(
        d["net"]["offers"].as_array().unwrap().iter().all(|k| [
            "lan",
            "public-v4",
            "carrier-v4",
            "global-v6",
            "local-v6"
        ]
        .contains(&k.as_str().unwrap())),
        "{}",
        d["net"]
    );
    assert!(
        d["direct"]["time_to_direct_ms"].is_u64()
            && d["direct"]["trying"] == false
            && d["direct"]["for_s"].is_null(),
        "{}",
        d["direct"]
    );
    let t = std::time::Instant::now();
    w.bridge.force_relay(true).await.unwrap();
    assert!(
        wait(30, || p.phase() == "online" && p.tier() == "shared_relay").await,
        "{}",
        p.phone.phase()
    );
    // on the relay again: the earlier change to direct is history, never shown as this stretch's result
    assert!(
        wait(10, || {
            w.bridge.status()["devices"][0]["direct"]["last_time_to_direct_ms"].is_u64()
        })
        .await,
        "{}",
        w.bridge.status()["devices"][0]["direct"]
    );
    let d = w.bridge.status()["devices"][0].clone();
    assert!(
        d["direct"]["time_to_direct_ms"].is_null() && d["direct"]["for_s"].is_u64(),
        "{}",
        d["direct"]
    );
    let why = d["why"].as_str().unwrap_or_default().to_string();
    assert!(
        !why.contains("was not possible")
            && !why.contains("probably")
            && !why.contains("keeps trying"),
        "{why}"
    );
    println!(
        "MEASURE path changed under a session (direct -> relay only): tier followed after {:?}",
        t.elapsed()
    );
    assert_eq!(
        p.call("link.status", json!({})).await.unwrap()["path_facts"]["kind"],
        "relay"
    );
    assert_eq!(w.bridge.status()["devices"][0]["tier"], "shared_relay");
    w.bridge.force_relay(false).await.unwrap();
    assert!(
        wait(40, || p.phase() == "online" && p.tier() == "direct").await,
        "{}",
        p.phone.phase()
    );
    assert!(
        w.bridge
            .audit(200)
            .iter()
            .any(|a| a["method"] == "link.path"
                && a["via"].as_str().is_some_and(|v| v.starts_with("direct")))
    );
    // every change of path is written down with what both sides offered at that moment
    assert!(
        w.bridge
            .audit(200)
            .iter()
            .any(|a| a["method"] == "link.path"
                && a["net"]["mac"]["offers"].is_array()
                && a["net"].get("phone").is_some()),
        "{:?}",
        w.bridge.audit(20)
    );
    assert!(
        wait(10, || {
            w.bridge.status()["devices"][0]["direct"]["time_to_direct_ms"].is_u64()
        })
        .await
    );
}

/// "Load more" (kb PHN-589): `link.files` carries the Mac's marker for a folder's next page on to the daemon (beside
/// the path, which a key place's proof covers alone), and a marker of another folder comes back as the Mac's bad
/// request.
// decision: PHN-589
#[tokio::test(flavor = "multi_thread")]
async fn a_folder_s_next_page_goes_by_the_mac_s_marker() {
    let (_relay, w, p) = world().await;
    w.standin.configure(&json!({"list_max": 2}));
    let dir = w.dir.join("paged");
    std::fs::create_dir_all(&dir).unwrap();
    for n in ["a.txt", "b.txt", "c.txt"] {
        std::fs::write(dir.join(n), b"x").unwrap();
    }
    let first = p.call("link.files", json!({"path": dir})).await.unwrap();
    assert_eq!(first["entries"].as_array().unwrap().len(), 2, "{first}");
    assert_eq!(
        (
            first["from"].clone(),
            first["cut"].clone(),
            first["total"].clone()
        ),
        (json!(0), json!(true), json!(3))
    );
    let next = first["next"].clone();
    assert!(next.is_string(), "{first}");
    let last = p
        .call("link.files", json!({"path": dir, "after": next}))
        .await
        .unwrap();
    assert_eq!(last["entries"][0]["name"], "c.txt", "{last}");
    assert_eq!(
        (
            last["from"].clone(),
            last["cut"].clone(),
            last["next"].clone()
        ),
        (json!(2), json!(false), Value::Null)
    );
    assert_eq!(
        common::code(
            p.call(
                "link.files",
                json!({"path": w.dir.join("standin-files"), "after": next})
            )
            .await
        ),
        errors::BAD_REQUEST
    );
}
