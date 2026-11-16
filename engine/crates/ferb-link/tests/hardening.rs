#![cfg(target_os = "macos")]
//! What must hold when a part of the Mac side is not trusted: the daemon against a bridge that lies, the state file
//! against another local program, the owner's controls against other programs, a copied link key against the lock.
//! And the capabilities built on top: uploads that stop and go on, folders, the click on a Mac prompt. 127.0.0.1 only.
mod common;
use common::*;
use ferb_link::{
    bridge::{Bridge, BridgeConfig},
    client::Phone,
    errors,
    net::Shared,
    secrets::Keys,
};
use serde_json::{Value, json};
use std::time::Duration;

async fn world() -> (Relay, World, TestPhone) {
    let relay = Relay::start(None, None).await;
    let w = World::start(Shared::Custom(vec![relay.spec()]), None, false).await;
    let p = w.paired_phone("phone").await;
    // (files cross only a direct path or the owner's relay, kb PHN-444: both ends on this Mac find each other)
    assert!(
        wait(20, || p.tier() == "direct").await,
        "tier stayed {}",
        p.tier()
    );
    (relay, w, p)
}

// decision: PHN-124
#[tokio::test(flavor = "multi_thread")]
async fn the_daemon_trusts_nothing_the_bridge_says_about_a_phone() {
    let (_relay, w, p) = world().await;
    let id = p.phone.id();
    let dir = std::fs::canonicalize(&w.dir).unwrap();
    std::fs::create_dir_all(dir.join("h/vault")).unwrap();
    std::fs::write(dir.join("h/vault/key.txt"), b"SECRET").unwrap();
    std::fs::write(dir.join("h/plain.txt"), b"plain").unwrap();
    let s = |rel: &str| dir.join(rel).to_string_lossy().to_string();
    // A program that speaks to the daemon as the phone link would (the bridge taken over, or an impostor of it):
    // 1. it cannot name a phone the daemon does not know
    assert_eq!(
        w.standin
            .call_from("phone", "0000", "ping", json!({}), None)
            .await
            .unwrap_err()["code"],
        -32020
    );
    // 2. for a real phone it gets nothing of the owner's without that phone's own proofs: no claim of "unlocked" or
    //    "fresh" exists to be made (the old stand-in took `fresh: true` from the bridge)
    for (m, params) in [
        (
            "phone.files",
            json!({"path": s("h"), "fresh": true, "unlocked": true}),
        ),
        ("settings.get", json!({})),
        ("chat.attach", json!({})),
        ("tasks.list", json!({})),
    ] {
        assert_eq!(
            w.standin
                .call_from("phone", &id, m, params, None)
                .await
                .unwrap_err()["code"],
            -32011,
            "{m}"
        );
    }
    assert_eq!(
        w.standin
            .call_from(
                "phone",
                &id,
                "settings.set",
                json!({"key": "a", "value": 1}),
                None
            )
            .await
            .unwrap_err()["code"],
        -32012
    );
    // 3. a proof the phone made for another connection is no use on this one (the challenge is the connection's)
    let ch = p.phone.challenge().await.unwrap();
    let proof = p.proof_for(&Phone::call_payload(&ch, "link.unlock", &json!({})));
    assert_eq!(
        w.standin
            .call_from("phone", &id, "phone.unlock", json!({}), Some(proof))
            .await
            .unwrap_err()["code"],
        -32013
    );
    // 4. the helper's own calls are not open to a phone session (the daemon gives role link only to the helper, by
    //    its code signature: ferb-rpc's phone_entry tests)
    assert_eq!(
        w.standin
            .call_from("phone", &id, "link.devices", json!({}), None)
            .await
            .unwrap_err()["code"],
        -32001
    );
    assert_eq!(
        w.standin
            .call_from("phone", "evil", "ping", json!({}), None)
            .await
            .unwrap_err()["code"],
        -32020
    );
    // the real phone, through the real bridge, still works
    assert_eq!(
        p.phone
            .get(json!({"what": "path", "path": s("h/plain.txt")}), |_, _| {})
            .await
            .unwrap()
            .1,
        b"plain"
    );
}

// decision: PHN-125, PHN-126
#[tokio::test(flavor = "multi_thread")]
async fn a_copy_of_the_link_key_gets_what_a_locked_phone_gets_and_no_words() {
    let (_relay, w, p) = world().await;
    w.limits_set(&json!({"devices_max": 2})).unwrap();
    w.standin
        .cli(
            "standin.ask",
            json!({"reason": "WORDS-OF-THE-QUESTION which folder?", "options": []}),
        )
        .await;
    w.standin.cli("standin.ask", json!({"kind": "owner_secret", "capability": "secret.fill", "reason": "PRIVATE-WORDS the bank password", "options": []})).await;
    // the owner's phone, unlocked: everything, worded
    assert!(
        wait(10, || p.status()["approvals"]
            .as_array()
            .is_some_and(|a| a.len() == 2))
        .await
    );
    assert!(p.status().to_string().contains("WORDS-OF-THE-QUESTION"));
    // the app goes to the background and tells the Mac it locked: the same phone, device-bound, still gets the words
    // of an ordinary question (for its notifications) for a while after a real unlock; never a private one's
    p.call("link.lock", json!({})).await.unwrap();
    assert!(
        wait(10, || p.status()["locked"] == true).await,
        "{}",
        p.status()
    );
    let st = p.call("chat.status", json!({})).await.unwrap();
    assert!(
        st["words"] == true
            && st.to_string().contains("WORDS-OF-THE-QUESTION")
            && !st.to_string().contains("PRIVATE-WORDS"),
        "{st}"
    );
    assert_eq!(
        common::code(
            p.phone
                .call("chat.send", json!({"text": "hi"}), None, |_| {})
                .await
        ),
        errors::LOCKED,
        "worded or not, a locked phone sends nothing"
    );
    // the same link key somewhere else (copied off the phone): no device key, no fingerprint. It connects as the
    // phone, and gets only that something waits, of which kind. No words, no chat, no files, no task list.
    let thief = TestPhone::start("copy", &w.shared, false, p.saved(), true).await;
    assert!(
        thief.wait_phase("online", 20).await,
        "{}",
        thief.phone.phase()
    );
    tokio::time::sleep(Duration::from_secs(1)).await;
    let st = thief.call("chat.status", json!({})).await.unwrap();
    assert!(st["locked"] == true && st["words"] == false, "{st}");
    assert!(
        !st.to_string().contains("WORDS-OF-THE-QUESTION")
            && !st.to_string().contains("PRIVATE-WORDS"),
        "the copy read the question: {st}"
    );
    assert_eq!(st["approvals"].as_array().unwrap().len(), 2);
    for (m, params) in [
        ("tasks.list", json!({})),
        ("about.get", json!({})),
        ("link.files", json!({"path": "~"})),
        ("settings.get", json!({})),
    ] {
        assert_eq!(
            common::code(thief.call(m, params).await),
            errors::LOCKED,
            "{m}"
        );
    }
    assert_eq!(
        common::code(
            thief
                .phone
                .call(
                    "chat.send",
                    json!({"text": "zip my documents"}),
                    None,
                    |_| {}
                )
                .await
        ),
        errors::LOCKED
    );
    assert_eq!(
        common::code(thief.unlock().await),
        errors::UNLOCK_INVALID,
        "its keys are not the phone's"
    );
    // (on the public relay the path refuses it first, kb PHN-444; on a direct path the daemon's lock does)
    let want = if thief.tier() == "direct" {
        errors::LOCKED
    } else {
        errors::NEEDS_BETTER_PATH
    };
    assert_eq!(
        common::code(thief.phone.put("x", b"x", |_, _| {}).await),
        want
    );
    // nor can it register keys of its own: that needs the phone's real unlock, signed for exactly those keys
    let ch = thief.phone.challenge().await.unwrap();
    let keys = json!({"device_pub": "AAAA"});
    let forged = thief.proof_for(&Phone::call_payload(&ch, "link.keys", &keys));
    assert!(
        thief
            .phone
            .call("link.keys", keys, Some(forged), |_| {})
            .await
            .is_err()
    );
    // it can stop FERB (a brake is never locked), and that is all
    assert!(thief.call("stop.engage", json!({})).await.is_ok());
    // the real phone unlocks again, and the unlock carries over to its next connection (it proves it is the phone)
    p.unlock().await.unwrap();
    let again = p.call("chat.status", json!({})).await.unwrap();
    assert!(
        again["locked"].is_null() && again.to_string().contains("WORDS-OF-THE-QUESTION"),
        "{again}"
    );
    // an ordinary question can be answered by the locked paired phone (from its notification); the copy cannot,
    // and nobody answers a private one or one of FERB's own checks that way
    p.call("link.lock", json!({})).await.unwrap();
    let st = p.call("chat.status", json!({})).await.unwrap();
    let plain = st["approvals"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["phone"]["needs_unlock"] == false)
        .unwrap()["approval_id"]
        .as_str()
        .unwrap()
        .to_string();
    let hidden = st["approvals"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["phone"]["hidden"] == true)
        .unwrap()["approval_id"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(
        common::code(
            thief
                .call(
                    "chat.answer",
                    json!({"approval_id": plain, "text": "the copy's answer"})
                )
                .await
        ),
        errors::LOCKED
    );
    assert!(
        p.call("chat.answer", json!({"approval_id": hidden, "text": "x"}))
            .await
            .is_err()
    );
    assert!(
        p.call(
            "chat.answer",
            json!({"approval_id": plain, "text": "the Reports folder"})
        )
        .await
        .is_ok(),
        "the paired phone could not answer from its notification"
    );
}

// decision: PHN-490
// decision: PHN-143
// decision: PHN-124
#[tokio::test(flavor = "multi_thread")]
async fn the_keys_come_only_from_ferb_and_a_changed_state_file_is_never_used() {
    use base64::Engine;
    let relay = Relay::start(None, None).await;
    let dir = tmp("k");
    let standin = Standin::start(&dir).await;
    let b64 = |b: &[u8]| base64::engine::general_purpose::STANDARD.encode(b);
    // the keys as FERB's daemon hands them over (one Keychain item it reads through ferb-secrets)
    let line =
        json!({"keys": {"identity": b64(&[7u8; 32]), "state-key": b64(&[8u8; 32])}}).to_string();
    let keys = Keys::Given(ferb_link::secrets::parse_given(&line).unwrap());
    let cfg = |keys: Keys, accept: bool| BridgeConfig {
        state_dir: dir.join("mac"),
        daemon_socket: standin.socket.clone(),
        shared: Shared::Custom(vec![relay.spec()]),
        roots: None,
        force_relay: true,
        keys,
        accept_state: accept,
        portmapper: false,
        loopback_only: true,
    };
    let want_id = iroh::SecretKey::from_bytes(&[7u8; 32]).public().to_string();
    let b = Bridge::start(cfg(keys.clone(), false))
        .await
        .expect("start");
    assert_eq!(
        b.id(),
        want_id,
        "the Mac's identity is not the one FERB keeps"
    );
    // the owner sets his relay (through the daemon, as Settings → Phone does): the token is never written down here
    let own = Relay::start(Some("TOKEN-NEVER-ON-DISK"), None).await;
    let set = standin
        .ask(
            "relay_set",
            json!({"url": own.url(), "token": "TOKEN-NEVER-ON-DISK"}),
        )
        .await;
    assert_eq!(set["result"]["saved"], true, "{set}");
    let status = standin.ask("status", json!({})).await;
    assert_eq!(status["result"]["own_relay"]["has_token"], true, "{status}");
    assert!(
        !status.to_string().contains("TOKEN-NEVER-ON-DISK"),
        "{status}"
    );
    b.close().await;
    // no socket of its own, and no secret in any file of its state
    for f in std::fs::read_dir(dir.join("mac")).unwrap().flatten() {
        if f.file_name() == "inbox" {
            continue; // (the stand-in daemon's upload folder, not the helper's)
        }
        let meta = std::fs::symlink_metadata(f.path()).unwrap();
        assert!(
            meta.is_file(),
            "the helper left {:?}, which is not a file",
            f.file_name()
        );
        let text = std::fs::read(f.path()).unwrap_or_default();
        assert!(
            !text.windows(32).any(|w| w == [7u8; 32] || w == [8u8; 32])
                && !String::from_utf8_lossy(&text).contains("TOKEN-NEVER-ON-DISK"),
            "a secret is in {:?}",
            f.file_name()
        );
    }
    // without the keys FERB keeps there is no start: nothing is made here in their place
    let none = Keys::Given(ferb_link::secrets::parse_given(r#"{"keys": {}}"#).unwrap());
    assert!(Bridge::start(cfg(none, false)).await.is_err());
    // another program edits the file (its own relay): in FERB the file is set aside and the link starts from the
    // defaults, so nothing it wrote is used and the phones still connect
    let mut edited: Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join("mac/link.json")).unwrap()).unwrap();
    edited["own_relay"] = json!({"url": "https://relay.invalid"});
    std::fs::write(dir.join("mac/link.json"), edited.to_string()).unwrap();
    let b = Bridge::start(cfg(keys.clone(), false))
        .await
        .expect("start after an edit");
    assert!(
        b.status()["own_relay"].is_null(),
        "an edited relay was used"
    );
    assert!(dir.join("mac/link.json.changed").exists());
    b.close().await;
    // with keys a person keeps in files, the lab's rule: refused until the owner says the change is his
    let fdir = tmp("kf");
    let fcfg = |accept: bool| BridgeConfig {
        state_dir: fdir.join("mac"),
        daemon_socket: standin.socket.clone(),
        shared: Shared::Custom(vec![relay.spec()]),
        roots: None,
        force_relay: true,
        keys: Keys::Files,
        accept_state: accept,
        portmapper: false,
        loopback_only: true,
    };
    Bridge::start(fcfg(false))
        .await
        .expect("files")
        .close()
        .await;
    let b = Bridge::start(fcfg(false)).await.expect("files again");
    b.limits_set(&json!({"calls_per_10s": 77})).unwrap();
    b.close().await;
    let mut edited: Value =
        serde_json::from_str(&std::fs::read_to_string(fdir.join("mac/link.json")).unwrap())
            .unwrap();
    edited["limits"]["calls_per_10s"] = json!(100000);
    std::fs::write(fdir.join("mac/link.json"), edited.to_string()).unwrap();
    let e = Bridge::start(fcfg(false))
        .await
        .err()
        .expect("an edited state file was loaded");
    assert!(
        e.message.contains("was changed since it was sealed") && e.allow.is_some(),
        "{e}"
    );
    edited.as_object_mut().unwrap().remove("seal");
    std::fs::write(fdir.join("mac/link.json"), edited.to_string()).unwrap();
    assert!(
        Bridge::start(fcfg(false))
            .await
            .err()
            .is_some_and(|e| e.message.contains("lost its seal"))
    );
    let b = Bridge::start(fcfg(true)).await.expect("--accept-state");
    assert_eq!(b.limits_get().calls_per_10s, 100000);
    b.close().await;
    assert!(Bridge::start(fcfg(false)).await.is_ok());
}

// decision: PHN-489
// decision: PHN-441
// decision: PHN-106
#[tokio::test(flavor = "multi_thread")]
async fn the_owners_controls_reach_the_helper_only_through_the_daemon() {
    let (_relay, w, p) = world().await;
    // the helper listens on no socket of its own (the lab's admin socket is gone): the daemon asks, the helper answers
    assert!(!w.dir.join("mac/admin.sock").exists());
    let invite = w.standin.ask("pair_start", json!({})).await;
    assert!(
        invite["result"]["url"]
            .as_str()
            .is_some_and(|u| u.starts_with("ferblink://"))
            && invite["result"]["code"]
                .as_str()
                .is_some_and(|c| c.starts_with("FERB-")),
        "{invite}"
    );
    assert_eq!(
        w.standin.ask("pair_cancel", json!({})).await["result"]["ok"],
        true
    );
    assert_eq!(w.bridge.status()["pairing"]["state"], "closed");
    let st = w.standin.ask("status", json!({})).await["result"].clone();
    assert!(
        st["devices"]
            .as_array()
            .unwrap()
            .iter()
            .any(|d| d["id"] == json!(p.phone.id()) && d["online"] == true),
        "{st}"
    );
    let bad = w.standin.ask("made_up", json!({})).await;
    assert_eq!(bad["error"]["code"], errors::BAD_REQUEST, "{bad}");
    // the Yes is the owner's in FERB.app, never the helper's: the helper has no way to say it
    assert!(w.decide(true).is_err(), "nothing waits");
    // at the daemon's limit the code is refused before it is drawn
    w.limits_set(&json!({"devices_max": 1})).unwrap();
    let at_max = w.standin.ask("pair_start", json!({})).await;
    assert_eq!(at_max["error"]["code"], errors::DEVICE_LIMIT, "{at_max}");
}

// decision: PHN-127
#[tokio::test(flavor = "multi_thread")]
async fn uploads_stop_and_go_on_folders_arrive_whole_and_the_disk_keeps_its_floor() {
    let (_relay, w, p) = world().await;
    let inbox = w.dir.join("mac/inbox");
    let big: Vec<u8> = (0..40_000_000u32).map(|i| (i % 249) as u8).collect();
    let file = w.dir.join("big.bin");
    std::fs::write(&file, &big).unwrap();
    // an upload that is stopped part way (the app's Pause, or a lost connection) ...
    let upload_id = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
    let (ph, f2, id2) = (p.phone.clone(), file.clone(), upload_id.clone());
    let first = tokio::spawn(async move {
        ph.put_file(
            &f2,
            json!({"name": "big.bin"}),
            |a| *id2.lock().unwrap() = a["upload_id"].as_str().unwrap_or_default().to_string(),
            |_, _| {},
        )
        .await
    });
    assert!(
        wait(10, || w.bridge.status()["transfers"]
            .as_array()
            .is_some_and(|t| t.iter().any(
                |x| x["to_phone"] == false && x["done"].as_u64().unwrap_or(0) > 200_000
            )))
        .await,
        "the Mac does not list the upload under way: {}",
        w.bridge.status()["transfers"]
    );
    first.abort();
    let id = upload_id.lock().unwrap().clone();
    assert!(!id.is_empty());
    assert!(
        wait(10, || w.bridge.status()["transfers"]
            .as_array()
            .is_some_and(|t| t.is_empty()))
        .await
    );
    let part = std::fs::read_dir(&inbox)
        .unwrap()
        .flatten()
        .find(|e| e.file_name().to_string_lossy().ends_with("big.bin"))
        .expect("the part that arrived was not kept")
        .metadata()
        .unwrap()
        .len();
    assert!(part > 0 && part < 40_000_000, "{part}");
    // ... goes on where it stopped, and arrives whole
    let mut started_at = 0u64;
    let mut seen = false;
    let done = p
        .phone
        .put_file(
            &file,
            json!({"name": "big.bin", "resume": id}),
            |_| {},
            |got, _| {
                if !seen {
                    seen = true;
                    started_at = got;
                }
            },
        )
        .await
        .unwrap();
    assert!(
        started_at > 0 && started_at == part,
        "it started again from {started_at}, the Mac had {part}"
    );
    let landed = done["saved"].as_str().unwrap();
    assert_eq!(
        std::fs::read(landed).unwrap(),
        big,
        "the file that went on is not the file that was sent"
    );
    use base64::Engine;
    use sha2::Digest;
    assert_eq!(
        done["sha256"],
        json!(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(sha2::Sha256::digest(&big)))
    );
    // another phone cannot go on with (or attach) this phone's upload
    w.limits_set(&json!({"devices_max": 2})).unwrap();
    let other = w.paired_phone("other").await;
    assert_eq!(
        common::code(
            other
                .phone
                .put_file(
                    &file,
                    json!({"name": "big.bin", "resume": done["upload_id"]}),
                    |_| {},
                    |_, _| {}
                )
                .await
        ),
        errors::NOT_FOUND
    );
    assert!(
        other
            .phone
            .call(
                "chat.send",
                json!({"text": "look", "attachments": [done["upload_id"]]}),
                None,
                |_| {}
            )
            .await
            .is_err()
    );

    // a folder: one folder on the Mac, every file at its place inside; names cannot climb out of it
    let group = p
        .call("link.put_group", json!({"name": "Holiday 2026"}))
        .await
        .unwrap();
    let gid = group["group"].as_str().unwrap();
    for (rel, body) in [("a.txt", "A"), ("sub/b.txt", "B"), ("sub/deep/c.txt", "C")] {
        let f = w.dir.join("one.txt");
        std::fs::write(&f, body).unwrap();
        p.phone
            .put_file(
                &f,
                json!({"name": rel.rsplit('/').next().unwrap(), "group": gid, "rel": rel}),
                |_| {},
                |_, _| {},
            )
            .await
            .unwrap();
    }
    let folder = std::path::PathBuf::from(group["saved"].as_str().unwrap());
    assert!(
        folder.starts_with(std::fs::canonicalize(&inbox).unwrap()) || folder.starts_with(&inbox)
    );
    assert_eq!(
        (
            std::fs::read(folder.join("a.txt")).unwrap(),
            std::fs::read(folder.join("sub/deep/c.txt")).unwrap()
        ),
        (b"A".to_vec(), b"C".to_vec())
    );
    let f = w.dir.join("one.txt");
    for evil in [
        "../escape.txt",
        "sub/../../escape.txt",
        "/etc/escape.txt",
        "sub//x/../../../escape.txt",
    ] {
        let r = p
            .phone
            .put_file(
                &f,
                json!({"name": "escape.txt", "group": gid, "rel": evil}),
                |_| {},
                |_, _| {},
            )
            .await;
        assert!(
            r.is_err()
                || std::path::PathBuf::from(r.unwrap()["saved"].as_str().unwrap())
                    .starts_with(&folder),
            "{evil}"
        );
    }
    assert!(
        !inbox.join("escape.txt").exists()
            && !w.dir.join("mac/escape.txt").exists()
            && !w.dir.join("escape.txt").exists()
    );
    // the folder rides with a message as one attachment
    p.phone
        .call(
            "chat.send",
            json!({"text": "the photos", "attachments": [gid]}),
            None,
            |_| {},
        )
        .await
        .unwrap();
    let st = w.standin.cli("standin.state", json!({})).await;
    assert_eq!(
        st["messages"].as_array().unwrap().last().unwrap()["input_paths"][0],
        json!(folder.to_string_lossy())
    );

    // Mac -> phone, a folder: its files listed in one call, the guarded ones left out and counted
    let home = std::fs::canonicalize(&w.dir).unwrap().join("docs");
    std::fs::create_dir_all(home.join("sub")).unwrap();
    for (rel, body) in [("r.txt", "R"), ("sub/s.txt", "S")] {
        std::fs::write(home.join(rel), body).unwrap();
    }
    let tree = p.call("link.tree", json!({"path": home})).await.unwrap();
    let rels: Vec<&str> = tree["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["rel"].as_str().unwrap())
        .collect();
    assert_eq!(
        (rels, tree["skipped"].as_u64()),
        // (the lab's stand-in also left out `.env` and `sub/id_rsa` and counted them: FERB's guarded places
        // are the daemon's, slice S3b, kb T-1203)
        (vec!["r.txt", "sub/s.txt"], Some(0)),
        "{tree}"
    );

    // the free-disk floor: an upload that would leave less than `upload_free_floor` free is refused before a byte
    w.limits_set(&json!({"upload_free_floor": u64::MAX / 4}))
        .unwrap();
    tokio::time::sleep(Duration::from_millis(500)).await;
    let before = std::fs::read_dir(&inbox).unwrap().count();
    let e = p.phone.put("small.txt", b"x", |_, _| {}).await.unwrap_err();
    assert!(
        e.code == errors::MAC_DISK_FULL && e.allow.is_some() && e.message.contains("free"),
        "{e}"
    );
    assert_eq!(
        std::fs::read_dir(&inbox).unwrap().count(),
        before,
        "a refused upload left a file"
    );
}

// decision: PHN-128, PHN-130
#[tokio::test(flavor = "multi_thread")]
async fn a_phone_that_takes_unusually_much_is_said_so_once_and_the_audit_is_kept_by_age() {
    let (_relay, w, p) = world().await;
    w.limits_set(
        &json!({"fetch_notice_files": 5, "fetch_notice_window_s": 3, "calls_per_10s": 10000}),
    )
    .unwrap();
    let f = std::fs::canonicalize(&w.dir).unwrap().join("f.txt");
    std::fs::write(&f, b"hello").unwrap();
    let mut admin = w.bridge.admin_events();
    let notices = |p: &TestPhone| {
        p.seen
            .lock()
            .unwrap()
            .events
            .iter()
            .filter(|e| e["type"] == "notice" && e["error"]["code"] == "heavy_fetch")
            .count()
    };
    for _ in 0..12 {
        p.phone
            .get(json!({"what": "path", "path": f}), |_, _| {})
            .await
            .unwrap();
    }
    assert!(
        wait(5, || notices(&p) == 1).await,
        "the phone's chat got {} notices",
        notices(&p)
    );
    let mut on_mac = 0;
    while let Ok(e) = admin.try_recv() {
        on_mac += (e["event"] == "heavy_fetch") as u32;
    }
    assert_eq!(on_mac, 1, "once per burst, not per file");
    assert!(
        w.bridge
            .audit(100)
            .iter()
            .any(|a| a["method"] == "mac.heavy_fetch" && a["files"].as_u64().unwrap() > 5)
    );
    // never a cap: the files kept coming. After a quiet window a new burst is told again.
    tokio::time::sleep(Duration::from_millis(3500)).await;
    for _ in 0..8 {
        p.phone
            .get(json!({"what": "path", "path": f}), |_, _| {})
            .await
            .unwrap();
    }
    assert!(wait(5, || notices(&p) == 2).await);

    // the audit: pieces older than `audit_keep_days` go; `audit_keep` pieces is only the limit on size
    w.limits_set(&json!({"audit_max_bytes": 3000, "audit_keep": 4, "audit_keep_days": 90}))
        .unwrap();
    for _ in 0..200 {
        let _ = p.call("made.up", json!({})).await;
    }
    let piece = |i: u32| w.dir.join(format!("mac/audit.jsonl.{i}"));
    assert!(piece(1).exists() && piece(3).exists() && !piece(5).exists());
    let old = std::time::SystemTime::now() - Duration::from_secs(91 * 86_400);
    std::fs::File::options()
        .write(true)
        .open(piece(3))
        .unwrap()
        .set_modified(old)
        .unwrap();
    for _ in 0..40 {
        let _ = p.call("made.up", json!({})).await;
    }
    let ages: Vec<u64> = (1..=4)
        .filter_map(|i| std::fs::metadata(piece(i)).ok())
        .map(|m| m.modified().unwrap().elapsed().unwrap().as_secs())
        .collect();
    assert!(
        ages.iter().all(|a| *a < 90 * 86_400),
        "a piece older than 90 days is still there: {ages:?}"
    );
    let all: String = (1..=4)
        .filter_map(|i| std::fs::read_to_string(piece(i)).ok())
        .collect::<String>()
        + &std::fs::read_to_string(w.dir.join("mac/audit.jsonl")).unwrap();
    assert!(
        all.contains("mac.audit_cut_early"),
        "the size limit cut the audit early without saying so"
    );
}

// ---- the seal across builds (SEC-24, SEC-44) -------------------------------------------------------------------
// The fixtures are states that ferb-link built from 1388e17 and from 9c90d9a wrote and sealed themselves (file
// keys, a made-up phone); nothing in them was touched afterwards.

fn sealed_fixture(build: &str) -> std::path::PathBuf {
    use base64::Engine;
    let from = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(format!("sealed-by-{build}"));
    let dir = tmp("seal");
    std::fs::copy(from.join("link.json"), dir.join("link.json")).unwrap();
    let key = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(
            std::fs::read_to_string(from.join("state-key.b64"))
                .unwrap()
                .trim(),
        )
        .unwrap();
    std::fs::write(dir.join("state-key.key"), key).unwrap();
    dir
}
fn open_state(dir: &std::path::Path) -> Result<ferb_link::store::Store, errors::LinkError> {
    ferb_link::store::Store::open(dir, Keys::Files, false)
}
/// The rule, written out here a second time and by other means than the code under test: the seal is the keyed
/// hash of the file's JSON text without the spaces and line breaks between its parts and without its last
/// member, `seal`.
fn seal_by_the_rule(dir: &std::path::Path, body: &Value) -> String {
    use base64::Engine;
    let key: [u8; 32] = std::fs::read(dir.join("state-key.key"))
        .unwrap()
        .try_into()
        .unwrap();
    base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(blake3::keyed_hash(&key, body.to_string().as_bytes()).as_bytes())
}
/// A state file as some build would write it: `body` in that build's own order of members, then the seal.
fn write_sealed(dir: &std::path::Path, body: &Value) {
    let text = serde_json::to_string_pretty(body).unwrap();
    let text = format!(
        "{},\n  \"seal\": \"{}\"\n}}",
        text.trim_end().strip_suffix('}').unwrap().trim_end(),
        seal_by_the_rule(dir, body)
    );
    std::fs::write(dir.join("link.json"), text).unwrap();
}

// decision: PHN-144
#[test]
fn a_state_sealed_by_an_earlier_build_opens_as_it_is() {
    for build in ["1388e17", "9c90d9a"] {
        let dir = sealed_fixture(build);
        let before = std::fs::read_to_string(dir.join("link.json")).unwrap();
        // (1388e17 did not know the setting 9c90d9a added: this is the file that 9c90d9a refused)
        assert_eq!(
            before.contains("handover_fresh_after_s"),
            build != "1388e17"
        );
        let store = open_state(&dir)
            .unwrap_or_else(|e| panic!("the state {build} sealed was refused: {e}"));
        // (its paired phone is the daemon's now, kb PHN-469: the member is read past, the relay setting kept)
        assert!(
            store
                .own_relay()
                .is_some_and(|r| r.url.starts_with("https://relay.invalid")),
            "{build}"
        );
        assert_eq!(store.limits().handover_fresh_after_s, 10);
        drop(store);
        // it was written again by this build, with the setting it knows, and opens again
        assert!(
            std::fs::read_to_string(dir.join("link.json"))
                .unwrap()
                .contains("handover_fresh_after_s")
        );
        open_state(&dir).expect("the second start");
    }
}

// decision: PHN-144
#[test]
fn a_state_file_edited_in_any_way_that_says_something_else_is_refused() {
    for build in ["1388e17", "9c90d9a"] {
        let dir = sealed_fixture(build);
        let good = std::fs::read_to_string(dir.join("link.json")).unwrap();
        let refused = |text: String, what: &str| {
            std::fs::write(dir.join("link.json"), text).unwrap();
            let e = open_state(&dir)
                .err()
                .unwrap_or_else(|| panic!("{build}: {what} was loaded"));
            assert!(
                e.message.contains("since it was sealed") || e.message.contains("lost its seal"),
                "{what}: {e}"
            );
            let allow = e.allow.clone().unwrap_or_default();
            assert!(
                e.message.contains("Nothing was removed") && allow.contains("--accept-state"),
                "{what}: {e}"
            );
        };
        let swap = |from: &str, to: &str| {
            assert!(good.contains(from), "the fixture has no {from}");
            good.replacen(from, to, 1)
        };
        refused(
            swap(
                "\"devices\": [",
                "\"devices\": [{\"id\": \"evil\", \"name\": \"evil\", \"paired_at\": 1, \"last_seen\": 1794300100, \"unlock_pub\": \"AAAA\"},",
            ),
            "a phone added",
        );
        refused(
            swap("Zml4dHVyZS11bmxvY2s", "YW5vdGhlci1rZXk"),
            "a phone's lock key changed",
        );
        refused(
            swap("\"devices_max\": 2", "\"devices_max\": 20"),
            "a limit loosened",
        );
        refused(swap("relay.invalid", "relay.example"), "the relay changed");
        refused(swap("\"  two", "\" two"), "a space inside a name");
        // the same members in another order say the same to a reader, but no build writes that: refused, since
        // only a file as the link wrote it is taken (nothing needs the looser rule)
        let v: Value = serde_json::from_str(&good).unwrap();
        // (built by hand: with ferb-rpc in the build serde_json keeps members in their order, so a plain re-print
        // would be the same text)
        let obj = v.as_object().unwrap();
        let mut reordered = serde_json::Map::new();
        for k in obj.keys().rev().filter(|k| *k != "seal") {
            reordered.insert(k.clone(), obj[k].clone());
        }
        reordered.insert("seal".into(), obj["seal"].clone());
        refused(
            serde_json::to_string_pretty(&Value::Object(reordered)).unwrap(),
            "members in another order",
        );
        // the seal anywhere but last, cut off, or emptied
        let seal = v["seal"].as_str().unwrap().to_string();
        refused(
            good.replacen("{", &format!("{{\"seal\": \"{seal}\","), 1)
                .replacen(&format!(",\n  \"seal\": \"{seal}\"\n}}"), "}", 1),
            "the seal moved to the front",
        );
        refused(
            good.replacen(&format!(",\n  \"seal\": \"{seal}\""), "", 1),
            "the seal cut off",
        );
        refused(swap(&seal, ""), "the seal emptied");
        // a second document's worth of text after the seal
        refused(
            format!(
                "{},\"more\":1}}",
                good.trim_end().strip_suffix('}').unwrap()
            ),
            "a member after the seal",
        );
        // spaces and line breaks between the parts say nothing (an editor that saved the file without a change,
        // a copy that lost its indentation): still the file the link wrote
        let flat: String = good.lines().map(str::trim_start).collect();
        assert_ne!(flat, good);
        for text in [flat, format!("\n{good}\n\n"), good.replace("\n", "\r\n")] {
            std::fs::write(dir.join("link.json"), text).unwrap();
            assert!(
                open_state(&dir)
                    .unwrap_or_else(|e| panic!("{build}: only its spacing changed: {e}"))
                    .own_relay()
                    .is_some()
            );
            std::fs::write(dir.join("link.json"), &good).unwrap();
        }
    }
}

// decision: PHN-144
#[test]
fn a_setting_one_build_knows_and_another_does_not_never_breaks_the_seal() {
    let dir = sealed_fixture("9c90d9a");
    drop(open_state(&dir).unwrap());
    // (serde_json's Value keeps members sorted here, so these files are also in another order than this build
    // writes: a file is judged by its own text, whoever wrote it)
    let mine: Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join("link.json")).unwrap()).unwrap();
    let mut body = mine.clone();
    body.as_object_mut().unwrap().remove("seal");
    assert_eq!(
        mine["seal"],
        json!(seal_of_file_text(&dir)),
        "this build seals by another rule than the one written down"
    );
    // an older build: it never heard of the newest setting
    let mut older = body.clone();
    assert!(
        older["limits"]
            .as_object_mut()
            .unwrap()
            .remove("handover_fresh_after_s")
            .is_some()
    );
    write_sealed(&dir, &older);
    let s = open_state(&dir).expect("a state sealed without a newer setting");
    assert_eq!(
        (s.own_relay().is_some(), s.limits().handover_fresh_after_s),
        (true, 10)
    );
    drop(s);
    // a future build: it knows settings and whole parts this build has never heard of
    let mut future = body.clone();
    future["limits"]["a_setting_of_next_year"] = json!(5);
    future["a_part_of_next_year"] = json!({"x": [1, 2, 3]});
    future["relay_marks"]["a-phone-of-next-year"] = json!("y");
    write_sealed(&dir, &future);
    let s = open_state(&dir).expect("a state sealed by a build that knows more");
    assert!(s.own_relay().is_some());
    drop(s);
    open_state(&dir).expect("and what this build then wrote");
    // ... and the future build's file with one of its values changed is still refused
    write_sealed(&dir, &future);
    let text = std::fs::read_to_string(dir.join("link.json")).unwrap();
    std::fs::write(
        dir.join("link.json"),
        text.replacen(
            "\"a_setting_of_next_year\": 5",
            "\"a_setting_of_next_year\": 6",
            1,
        ),
    )
    .unwrap();
    assert!(open_state(&dir).is_err());
}
/// The file's text without the spaces between its parts and without its last member (it has no space inside a
/// string that matters here: the fixture's name is checked apart).
fn compact_without_seal(dir: &std::path::Path) -> Vec<u8> {
    let v: Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join("link.json")).unwrap()).unwrap();
    let seal = v["seal"].as_str().unwrap();
    let text = std::fs::read_to_string(dir.join("link.json")).unwrap();
    let mut out = String::new();
    let mut in_string = false;
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                in_string = !in_string;
                out.push(c)
            }
            '\\' if in_string => {
                out.push(c);
                out.extend(chars.next())
            }
            c if c.is_whitespace() && !in_string => {}
            c => out.push(c),
        }
    }
    out.strip_suffix(&format!(",\"seal\":\"{seal}\"}}"))
        .map(|s| format!("{s}}}"))
        .expect("the seal is the last member")
        .into_bytes()
}
fn seal_of_file_text(dir: &std::path::Path) -> String {
    use base64::Engine;
    let key: [u8; 32] = std::fs::read(dir.join("state-key.key"))
        .unwrap()
        .try_into()
        .unwrap();
    base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(blake3::keyed_hash(&key, &compact_without_seal(dir)).as_bytes())
}
