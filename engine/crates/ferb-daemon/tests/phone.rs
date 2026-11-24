//! Stage 3 S3 (T-1202): the daemon's phone area through a real ferb-rpc server, as the phone link's helper reaches it.
//! Each behaviour of the phone lab's stand-in daemon that the link's suites rely on (ferb-link tests/common/standin.rs,
//! the port of ferb-phone-link@57b3f34 standin/phone.py) has a test here against the real area; the class table
//! (engine/fixtures/phone/classes.json) drives a table test over every method.

mod phone_support;

use std::sync::{Arc, Mutex};

use base64::Engine;
use ferb_daemon::phone::QuestionFacts;
use ferb_rpc::phone_gate as code;
use ferb_rpc::wire;
use phone_support::*;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

fn b64u(raw: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(raw)
}

const P1: &str = "endpoint-one";
const P2: &str = "endpoint-two";

/// Stand-in: `session()` refuses a device not in the daemon's own table, at the hello and on every call.
// decision: PHN-124
#[tokio::test]
async fn only_a_paired_phone_opens_a_session() {
    let w = world(None).await;
    let refused = Conn::open(
        &w,
        json!({"role": "phone", "device": "stranger", "session": "s"}),
    )
    .await
    .err()
    .unwrap();
    assert_eq!(refused.0, code::UNKNOWN_PHONE);
    pair(&w, P1, &Keys::new(1), false, false).await;
    let mut c = Conn::phone(&w, P1, "s1").await;
    assert_eq!(c.ok("ping", json!({})).await["method"], "ping");
}

/// Stand-in: what a locked connection reaches (PHN-125): status, Stop, ping and the session's own calls; everything
/// else of FERB's, chat included, says `locked` until the unlock.
// decision: PHN-125
#[tokio::test]
async fn a_locked_phone_reaches_only_status_stop_and_its_session() {
    let w = world(None).await;
    let keys = Keys::new(1);
    pair(&w, P1, &keys, false, false).await;
    let mut c = Conn::phone(&w, P1, "s1").await;
    for m in ["chat.status", "stop.engage", "task.stop", "ping"] {
        c.call(m, json!({}), None)
            .await
            .unwrap_or_else(|e| panic!("{m}: {e:?}"));
    }
    for m in [
        "chat.send",
        "tasks.list",
        "settings.get",
        "chat.entries.page",
    ] {
        assert_eq!(
            c.call(m, json!({}), None).await.unwrap_err().0,
            code::LOCKED,
            "{m}"
        );
    }
    c.unlock(&keys).await;
    c.ok("tasks.list", json!({})).await;
    c.ok("chat.send", json!({"text": "hi"})).await;
}

/// The unlock lasts the idle time, which use extends, and the hard maximum however much it is used (PHN-15,
/// PHN-141); `phone.lock` ends it.
// decision: PHN-15
// decision: PHN-141
#[tokio::test]
async fn the_unlock_window_expires_on_the_injected_clock() {
    let w = world(None).await;
    let keys = Keys::new(1);
    pair(&w, P1, &keys, false, false).await;
    let mut c = Conn::phone(&w, P1, "s1").await;
    c.unlock(&keys).await;
    w.clock.advance(299.0);
    c.ok("tasks.list", json!({})).await; // in the window; use extends it
    w.clock.advance(299.0);
    c.ok("tasks.list", json!({})).await;
    w.clock.advance(301.0);
    assert_eq!(
        c.call("tasks.list", json!({}), None).await.unwrap_err().0,
        code::LOCKED
    );
    // kept in use every four minutes: still ends at 12 hours
    c.unlock(&keys).await;
    let mut used = 0.0;
    while used < 12.0 * 3600.0 - 240.0 {
        w.clock.advance(240.0);
        used += 240.0;
        c.ok("tasks.list", json!({})).await;
    }
    // 12 hours since the unlock is still in it (the lab's `<=`); a moment past, it ends
    w.clock.advance(240.0);
    c.ok("tasks.list", json!({})).await;
    w.clock.advance(1.0);
    assert_eq!(
        c.call("tasks.list", json!({}), None).await.unwrap_err().0,
        code::LOCKED
    );
    c.unlock(&keys).await;
    c.ok("phone.lock", json!({})).await;
    assert_eq!(
        c.call("tasks.list", json!({}), None).await.unwrap_err().0,
        code::LOCKED
    );
}

/// Stand-in `verify_call`: a challenge is one use, lives `challenge_ttl_s`, belongs to its session (and phone); a proof
/// for other parameters or another method, or by another phone's key, is refused; and a phone without a lock key
/// can't make one.
// decision: PHN-108
// decision: PHN-112
// decision: PHN-124
#[tokio::test]
async fn proofs_are_for_one_exact_call_with_a_fresh_challenge_of_this_session() {
    let w = world(None).await;
    let (k1, k2) = (Keys::new(1), Keys::new(2));
    pair(&w, P1, &k1, false, false).await;
    pair(&w, P2, &k2, false, false).await;
    let mut a = Conn::phone(&w, P1, "a").await;
    let params = json!({"key": "x", "value": 1});
    // without a proof
    assert_eq!(
        a.call("settings.set", params.clone(), None)
            .await
            .unwrap_err()
            .0,
        code::UNLOCK_REQUIRED
    );
    // a good one, then its replay
    let c = a.challenge().await;
    let p = call_proof(&k1.lock, &c, "settings.set", &params);
    a.call("settings.set", params.clone(), Some(p.clone()))
        .await
        .unwrap();
    let (n, why) = a
        .call("settings.set", params.clone(), Some(p))
        .await
        .unwrap_err();
    assert_eq!(n, code::UNLOCK_INVALID);
    assert!(why.contains("used before"), "{why}");
    // a proof for other parameters
    let c = a.challenge().await;
    let p = call_proof(
        &k1.lock,
        &c,
        "settings.set",
        &json!({"key": "x", "value": 2}),
    );
    let (n, why) = a
        .call("settings.set", params.clone(), Some(p))
        .await
        .unwrap_err();
    assert_eq!(n, code::UNLOCK_INVALID);
    assert!(why.contains("different call"), "{why}");
    // a proof for another method
    let c = a.challenge().await;
    let p = call_proof(&k1.lock, &c, "keys.store", &params);
    assert_eq!(
        a.call("settings.set", params.clone(), Some(p))
            .await
            .unwrap_err()
            .0,
        code::UNLOCK_INVALID
    );
    // expired
    let c = a.challenge().await;
    w.clock.advance(121.0);
    let p = call_proof(&k1.lock, &c, "settings.set", &params);
    assert_eq!(
        a.call("settings.set", params.clone(), Some(p))
            .await
            .unwrap_err()
            .0,
        code::UNLOCK_INVALID
    );
    // another session's challenge
    let mut a2 = Conn::phone(&w, P1, "a2").await;
    let c = a2.challenge().await;
    let p = call_proof(&k1.lock, &c, "settings.set", &params);
    assert_eq!(
        a.call("settings.set", params.clone(), Some(p))
            .await
            .unwrap_err()
            .0,
        code::UNLOCK_INVALID
    );
    // another phone's challenge, signed by that phone's key, on this phone; and this phone's challenge signed by it
    let mut b = Conn::phone(&w, P2, "b").await;
    let c = b.challenge().await;
    let p = call_proof(&k2.lock, &c, "settings.set", &params);
    assert_eq!(
        a.call("settings.set", params.clone(), Some(p))
            .await
            .unwrap_err()
            .0,
        code::UNLOCK_INVALID
    );
    let c = a.challenge().await;
    let p = call_proof(&k2.lock, &c, "settings.set", &params);
    let (n, why) = a
        .call("settings.set", params.clone(), Some(p))
        .await
        .unwrap_err();
    assert_eq!(n, code::UNLOCK_INVALID);
    assert!(why.contains("wrong signature"), "{why}");
    // a session's challenges die with it
    let c = a.challenge().await;
    a.ok("phone.session_end", json!({})).await;
    let p = call_proof(&k1.lock, &c, "settings.set", &params);
    assert_eq!(
        a.call("settings.set", params, Some(p)).await.unwrap_err().0,
        code::UNLOCK_INVALID
    );
}

/// PHN-133: once the phone registered its fingerprint-only key, only that key signs a sensitive call; the PIN-capable
/// key is told so. Without one, the lock key signs (PHN-129).
// decision: PHN-129
// decision: PHN-133
#[tokio::test]
async fn sensitive_calls_take_the_fingerprint_key_where_the_phone_has_one() {
    let w = world(None).await;
    let (k1, k2) = (Keys::new(1), Keys::new(2));
    pair(&w, P1, &k1, true, false).await;
    pair(&w, P2, &k2, false, false).await;
    let mut a = Conn::phone(&w, P1, "a").await;
    let (n, why) = a
        .signed_call(&k1.lock, "stop.lift", json!({}))
        .await
        .unwrap_err();
    assert_eq!(n, code::UNLOCK_INVALID);
    assert!(why.contains("fingerprint, not the PIN"), "{why}");
    a.signed_call(&k1.strong, "stop.lift", json!({}))
        .await
        .unwrap();
    let mut b = Conn::phone(&w, P2, "b").await;
    b.signed_call(&k2.lock, "stop.lift", json!({}))
        .await
        .unwrap();
}

/// Stand-in `check_answer` and `answer_needs_no_window`: a plain No needs nothing; an ordinary question is answered in
/// the window, or locked from the paired phone itself; a Yes to a hard own check carries its fingerprint proof with the
/// question's challenge; a routine one is answered from the notification with no fingerprint (PHN-417); a stale answer
/// is refused (PHN-108).
// decision: PHN-105
// decision: PHN-108
// decision: PHN-137
// decision: PHN-415
// decision: PHN-417
// decision: PHN-467
// decision: PHN-470
#[tokio::test]
async fn answers_follow_the_question_s_kind() {
    let w = world(None).await;
    let keys = Keys::new(1);
    pair(&w, P1, &keys, true, true).await;
    // the pairing's own window (PHN-647) lapses first: these are a locked phone's answers
    w.clock.advance(301.0);
    let mut c = Conn::phone(&w, P1, "s").await;
    let q = |pending, hidden, own_check, routine| QuestionFacts {
        pending,
        hidden,
        own_check,
        routine,
    };
    ask(&w, "plain", q(true, false, false, false));
    ask(&w, "hard", q(true, false, true, false));
    ask(&w, "routine", q(true, false, true, true));
    ask(&w, "declined", q(true, false, true, false));
    // locked, not proved present: an ordinary answer needs the window; a No never does
    assert_eq!(
        c.call(
            "chat.approve",
            json!({"approval_id": "plain", "granted": true}),
            None
        )
        .await
        .unwrap_err()
        .0,
        code::LOCKED
    );
    c.ok(
        "chat.approve",
        json!({"approval_id": "declined", "granted": false}),
    )
    .await;
    // the paired phone itself (device key), still locked: ordinary and routine answers from the notification
    assert_eq!(c.present(&keys).await["present"], true);
    c.ok(
        "chat.approve",
        json!({"approval_id": "plain", "granted": true}),
    )
    .await;
    c.ok(
        "chat.approve",
        json!({"approval_id": "routine", "granted": true, "choice": 1}),
    )
    .await;
    // a hard check: the proof, by the fingerprint key, with the challenge its question came with
    let params = json!({"approval_id": "hard", "granted": true});
    assert_eq!(
        c.call("chat.approve", params.clone(), None)
            .await
            .unwrap_err()
            .0,
        code::UNLOCK_REQUIRED
    );
    let status = c.ok("chat.status", json!({})).await;
    let hard = status["approvals"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["approval_id"] == "hard")
        .unwrap()
        .clone();
    assert_eq!(hard["phone"]["needs_unlock"], true);
    assert_eq!(hard["phone"]["fingerprint"], true);
    let challenge = hard["phone"]["challenge"].as_str().unwrap().to_owned();
    let payload = |key: &p256::ecdsa::SigningKey| {
        signed(
            key,
            &json!({"approval_id": "hard", "challenge": challenge, "method": "chat.approve", "params": params})
                .to_string(),
        )
    };
    let (n, why) = c
        .call("chat.approve", params.clone(), Some(payload(&keys.lock)))
        .await
        .unwrap_err();
    assert_eq!(n, code::UNLOCK_INVALID);
    assert!(why.contains("fingerprint"), "{why}");
    let wrong = signed(
        &keys.strong,
        &json!({"approval_id": "hard", "challenge": challenge, "method": "chat.approve",
                "params": {"approval_id": "hard", "granted": true, "choice": 2}})
        .to_string(),
    );
    assert!(c
        .call("chat.approve", params.clone(), Some(wrong))
        .await
        .is_err());
    let yes = payload(&keys.strong);
    assert_eq!(
        c.call("chat.approve", params.clone(), Some(yes.clone()))
            .await
            .unwrap()["state"],
        "granted"
    );
    // replayed: the question is no longer waiting
    assert_eq!(
        c.call("chat.approve", params, Some(yes))
            .await
            .unwrap_err()
            .0,
        code::QUESTION_NOT_WAITING
    );
    assert_eq!(
        c.call(
            "chat.approve",
            json!({"approval_id": "nothing", "granted": true}),
            None
        )
        .await
        .unwrap_err()
        .0,
        code::QUESTION_NOT_WAITING
    );
    // a correction always needs the window
    ask(&w, "plain2", q(true, false, false, false));
    assert_eq!(
        c.call(
            "chat.correct",
            json!({"approval_id": "plain2", "text": "rather this"}),
            None
        )
        .await
        .unwrap_err()
        .0,
        code::LOCKED
    );
}

/// PHN-110: a hidden input is answered only by `chat.answer` with a proof whose signed text holds only the SHA-256 of
/// the value, by either key; a bare Yes or a correction never answers it; an empty text declines; the audit never holds
/// the value (rule 7).
// decision: PHN-109
// decision: PHN-110
// decision: PHN-114
#[tokio::test]
async fn a_hidden_answer_goes_only_through_the_sealed_path() {
    let w = world(None).await;
    let keys = Keys::new(1);
    pair(&w, P1, &keys, true, false).await;
    let mut c = Conn::phone(&w, P1, "s").await;
    ask(
        &w,
        "secret",
        QuestionFacts {
            pending: true,
            hidden: true,
            own_check: false,
            routine: false,
        },
    );
    assert_eq!(
        c.call(
            "chat.approve",
            json!({"approval_id": "secret", "granted": true}),
            None
        )
        .await
        .unwrap_err()
        .0,
        code::BAD_REQUEST
    );
    let status = c.ok("chat.status", json!({})).await;
    let ap = &status["approvals"][0];
    assert_eq!(
        ap["held"], true,
        "a locked session never sees a hidden question's words"
    );
    assert_eq!(ap["reason"], "FERB needs a private answer");
    let challenge = ap["phone"]["challenge"].as_str().unwrap().to_owned();
    let marker = "SECRET-MARKER-7f3a";
    let sealed = b64u(&Sha256::digest(marker.as_bytes()));
    let proof = signed(
        &keys.lock,
        &json!({"approval_id": "secret", "challenge": challenge, "method": "chat.answer", "sealed": sealed}).to_string(),
    );
    let params = json!({"approval_id": "secret", "text": marker});
    // a proof for another value
    let other = signed(
        &keys.lock,
        &json!({"approval_id": "secret", "challenge": challenge, "method": "chat.answer",
                "sealed": b64u(&Sha256::digest(b"other"))})
        .to_string(),
    );
    assert_eq!(
        c.call("chat.answer", params.clone(), Some(other))
            .await
            .unwrap_err()
            .0,
        code::UNLOCK_INVALID
    );
    assert_eq!(
        c.call("chat.answer", params, Some(proof)).await.unwrap()["state"],
        "granted"
    );
    let audit = audit_text(&w);
    assert!(audit.contains("chat.answer"), "{audit}");
    assert!(!audit.contains(marker), "{audit}");
    assert!(!format!("{:?}", w.journal.lock().unwrap()).contains(marker));
}

/// Stand-in `status_for`: unlocked, everything with each question's phone part; locked, words only for the paired phone
/// itself (PHN-126), never a hidden question's; any other holder of the link key learns only that something waits.
// decision: PHN-125
// decision: PHN-126
// decision: PHN-422
#[tokio::test]
async fn a_locked_status_holds_its_words_back() {
    let w = world(None).await;
    let keys = Keys::new(1);
    pair(&w, P1, &keys, false, true).await;
    // the pairing's own window (PHN-647) lapses first: this is a locked phone's status
    w.clock.advance(301.0);
    ask(
        &w,
        "q",
        QuestionFacts {
            pending: true,
            hidden: false,
            own_check: false,
            routine: false,
        },
    );
    let mut c = Conn::phone(&w, P1, "s").await;
    let s = c.ok("chat.status", json!({})).await;
    assert_eq!(s["locked"], true);
    assert_eq!(s["words"], false);
    let text = s.to_string();
    for words in [
        "the words of q",
        "the goal's words",
        "the answer's words",
        "a notice's words",
        "a running goal",
    ] {
        assert!(!text.contains(words), "{words} in {text}");
    }
    assert_eq!(s["approvals"][0]["reason"], "FERB has a question");
    c.present(&keys).await;
    let s = c.ok("chat.status", json!({})).await;
    assert_eq!(s["words"], true);
    assert_eq!(s["approvals"][0]["reason"], "the words of q");
    assert_eq!(s["approvals"][0]["held"], false);
    c.unlock(&keys).await;
    let s = c.ok("chat.status", json!({})).await;
    assert!(s.get("locked").is_none());
    assert_eq!(
        s["approvals"][0]["phone"],
        json!({"can_answer": true, "needs_unlock": false})
    );
}

/// Stand-in `present` and `_open_window`: the device key proves the paired phone itself; a session that proved it
/// takes the phone's unlock from an earlier session (a reconnect does not ask again); one that did not, does not.
// decision: PHN-126
#[tokio::test]
async fn a_reconnect_that_proves_the_device_key_keeps_the_unlock() {
    let w = world(None).await;
    let keys = Keys::new(1);
    pair(&w, P1, &keys, false, true).await;
    let mut first = Conn::phone(&w, P1, "first").await;
    first.unlock(&keys).await;
    let mut second = Conn::phone(&w, P1, "second").await;
    assert_eq!(
        second
            .call("tasks.list", json!({}), None)
            .await
            .unwrap_err()
            .0,
        code::LOCKED
    );
    // a wrong proof changes nothing
    let c = second.challenge().await;
    let bad = call_proof(&keys.lock, &c, "link.present", &json!({}));
    assert_eq!(
        second
            .call("phone.present", json!({}), Some(bad))
            .await
            .unwrap()["present"],
        false
    );
    assert_eq!(second.present(&keys).await["present"], true);
    second.ok("tasks.list", json!({})).await;
    // the older session's end leaves the phone's unlock alone
    first.ok("phone.session_end", json!({})).await;
    second.ok("tasks.list", json!({})).await;
}

/// Stand-in `keys`: a phone paired before its device and fingerprint keys existed registers them once, under a proof
/// signed for exactly those keys; a key already there is replaced only by pairing again.
// decision: PHN-133
// decision: PHN-443
#[tokio::test]
async fn late_keys_are_registered_once() {
    let w = world(None).await;
    let keys = Keys::new(1);
    pair(&w, P1, &keys, false, false).await;
    let mut c = Conn::phone(&w, P1, "s").await;
    assert_eq!(
        c.call("phone.keys", json!({"device_pub": "not a key"}), None)
            .await
            .unwrap_err()
            .0,
        code::BAD_REQUEST
    );
    let params = json!({"device_pub": spki(&keys.device), "strong_pub": spki(&keys.strong)});
    assert_eq!(
        c.call("phone.keys", params.clone(), None)
            .await
            .unwrap_err()
            .0,
        code::UNLOCK_REQUIRED
    );
    let ch = c.challenge().await;
    let p = call_proof(&keys.lock, &ch, "link.keys", &params);
    let got = c.call("phone.keys", params, Some(p)).await.unwrap();
    assert_eq!(got["strong_pub"], json!(spki(&keys.strong)));
    // now the fingerprint key alone signs a key registration, and nothing is replaced
    let other = Keys::new(9);
    let params = json!({"strong_pub": spki(&other.strong)});
    let ch = c.challenge().await;
    let p = call_proof(&keys.strong, &ch, "link.keys", &params);
    let got = c.call("phone.keys", params, Some(p)).await.unwrap();
    assert_eq!(got["strong_pub"], json!(spki(&keys.strong)));
}

/// Pairing: the helper's checked request waits for the owner's Yes with the six digits; a No, no answer in time or a
/// cancel pairs nothing; at most 5 phones (PHN-419), the 6th refused at the request and at the Yes.
// decision: PHN-102
// decision: PHN-419
// decision: PHN-469
#[tokio::test]
async fn pairing_waits_for_the_owner_s_yes_and_stops_at_five() {
    let w = world(None).await;
    let mut link = Conn::link(&w).await;
    // malformed
    assert_eq!(
        link.call(
            "link.pair_request",
            json!({"endpoint": "e", "code": "12345"}),
            None
        )
        .await
        .unwrap_err()
        .0,
        code::BAD_REQUEST
    );
    // a No
    let r = link
        .ok(
            "link.pair_request",
            json!({"endpoint": "e0", "name": "Pixel\u{0}", "code": "111 222", "unlock_pub": "junk",
                                        "account": assertion(&w, "e0")}),
        )
        .await;
    let id = r["request_id"].as_str().unwrap().to_owned();
    let listed = w.phones.list(&app_session()).unwrap();
    assert_eq!(listed["pairing"]["code"], "111 222");
    assert_eq!(listed["pairing"]["name"], "Pixel");
    assert!(w
        .published
        .lock()
        .unwrap()
        .iter()
        .any(|(k, _)| k == "phone.pair_requested"));
    let mut no = ferb_rpc::Params::new();
    no.insert("yes".into(), json!(false));
    assert_eq!(w.phones.pair_decide(&no).unwrap()["paired"], false);
    assert_eq!(
        link.ok("link.pair_wait", json!({"request_id": id})).await["paired"],
        false
    );
    // no answer in time
    let r = link
        .ok(
            "link.pair_request",
            json!({"endpoint": "e0", "code": "111 222", "account": assertion(&w, "e0")}),
        )
        .await;
    let id = r["request_id"].as_str().unwrap().to_owned();
    w.clock.advance(121.0);
    assert_eq!(
        link.ok("link.pair_wait", json!({"request_id": id})).await["paired"],
        false
    );
    let mut yes = ferb_rpc::Params::new();
    yes.insert("yes".into(), json!(true));
    assert!(
        w.phones.pair_decide(&yes).is_err(),
        "nothing waits any more"
    );
    // five pair; the junk lock key was not kept
    for n in 0..5 {
        pair(&w, &format!("p{n}"), &Keys::new(n as u8 + 1), false, false).await;
    }
    let (n, _) = link
        .call(
            "link.pair_request",
            json!({"endpoint": "p5", "code": "111 222", "account": assertion(&w, "p5")}),
            None,
        )
        .await
        .unwrap_err();
    assert_eq!(n, code::DEVICE_LIMIT);
    let devices = link.ok("link.devices", json!({})).await;
    assert_eq!(devices["devices"].as_array().unwrap().len(), 5);
    assert_eq!(devices["max"], 5);
}

/// Removal (PHN-419): on the Mac, or by a second phone with its fingerprint; the removed phone's calls and hello are
/// refused at once, its unlock is gone, and the helper hears of it.
// decision: PHN-419
// decision: PHN-114
#[tokio::test]
async fn a_removed_phone_is_refused_at_once() {
    let w = world(None).await;
    let (k1, k2) = (Keys::new(1), Keys::new(2));
    pair(&w, P1, &k1, true, false).await;
    pair(&w, P2, &k2, true, false).await;
    let mut link = Conn::link(&w).await;
    let mut events = Conn::link(&w).await;
    let watching = tokio::spawn(async move {
        let _ = events.call("link.events", json!({}), None).await;
    });
    let mut lost = Conn::phone(&w, P1, "s").await;
    lost.unlock(&k1).await;
    let mut other = Conn::phone(&w, P2, "t").await;
    // a phone removes another: sensitive, fingerprint key
    assert_eq!(
        other
            .call("phone.remove", json!({"id": P1}), None)
            .await
            .unwrap_err()
            .0,
        code::UNLOCK_REQUIRED
    );
    other.unlock(&k2).await;
    let listed = other.ok("phone.list", json!({})).await;
    assert_eq!(listed["phones"].as_array().unwrap().len(), 2);
    assert_eq!(
        other
            .signed_call(&k2.strong, "phone.remove", json!({"id": P1}))
            .await
            .unwrap()["removed"],
        P1
    );
    assert_eq!(
        lost.call("tasks.list", json!({}), None)
            .await
            .unwrap_err()
            .0,
        code::UNKNOWN_PHONE
    );
    assert_eq!(
        lost.call("chat.status", json!({}), None)
            .await
            .unwrap_err()
            .0,
        code::UNKNOWN_PHONE
    );
    assert_eq!(
        Conn::open(
            &w,
            json!({"role": "phone", "device": P1, "session": "again"})
        )
        .await
        .err()
        .unwrap()
        .0,
        code::UNKNOWN_PHONE
    );
    assert_eq!(
        link.ok("link.devices", json!({})).await["devices"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let audit = audit_text(&w);
    assert!(
        audit.contains(&format!(
            "{P2} phone.remove sensitive ok {{\"removed\": \"{P1}\"}}"
        )),
        "{audit}"
    );
    // a phone may not rename, decide a pairing or read the audit (PHN-106)
    for m in [
        "phone.rename",
        "phone.pair_decide",
        "phone.audit",
        "link.devices",
    ] {
        assert_eq!(
            other.call(m, json!({}), None).await.unwrap_err().0,
            wire::UNAUTHORIZED,
            "{m}"
        );
    }
    watching.abort();
}

/// The class table drives every call: for each method of engine/fixtures/phone/classes.json, locked and unlocked,
/// with and without a proof, against the stub handlers (the real ones are FERB.app's): `never` is refused by its role,
/// `plain` and `private` need the window unless the row allows a locked phone, `sensitive` needs a proof for exactly
/// this call, an on-screen one says `needs_the_mac` while nothing clicks, refused parameters are refused.
// decision: PHN-112
// decision: PHN-117
// decision: PHN-465
// decision: PHN-472
#[tokio::test]
async fn the_class_table_drives_every_call() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../fixtures/phone/classes.json"
    );
    let table: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    let methods = table["methods"].as_object().unwrap();
    assert_eq!(methods.len(), ferb_rpc::auth::METHODS.len());
    let w = world(None).await;
    let keys = Keys::new(1);
    pair(&w, P1, &keys, false, false).await;
    let gate_codes = [
        code::LOCKED,
        code::UNLOCK_REQUIRED,
        code::UNLOCK_INVALID,
        code::NEEDS_THE_MAC,
        wire::UNAUTHORIZED,
    ];
    let passed = |r: &Reply| match r {
        Ok(_) => true,
        Err((c, _)) => !gate_codes.contains(c),
    };
    let mut checked = 0;
    for (name, row) in methods {
        if ferb_rpc::auth::PHONE_SESSION.contains(&name.as_str())
            || name == "phone.remove"
            || name == "phone.device_report"
        {
            continue; // their own tests (phone.device_report: tests/phone_device.rs, the device key's proof)
        }
        let class = row["class"].as_str().unwrap();
        let locked_rule = row["locked"].as_str().unwrap();
        let mut locked = Conn::phone(&w, P1, &format!("locked-{name}")).await;
        let mut open = Conn::phone(&w, P1, &format!("open-{name}")).await;
        open.unlock(&keys).await;
        let params = json!({});
        let l = locked.call(name, params.clone(), None).await;
        let u = open.call(name, params.clone(), None).await;
        match class {
            "never" => {
                assert_eq!(l.as_ref().unwrap_err().0, wire::UNAUTHORIZED, "{name}");
                assert_eq!(u.as_ref().unwrap_err().0, wire::UNAUTHORIZED, "{name}");
            }
            "plain" | "private" => {
                match locked_rule {
                    "allowed" => assert!(passed(&l), "{name}: {l:?}"),
                    "refused" => assert_eq!(l.as_ref().unwrap_err().0, code::LOCKED, "{name}"),
                    // an answer to no waiting question: refused as not waiting, locked or not
                    "answer" => assert_eq!(
                        l.as_ref().unwrap_err().0,
                        code::QUESTION_NOT_WAITING,
                        "{name}"
                    ),
                    other => panic!("{other}"),
                }
                if locked_rule == "answer" {
                    assert_eq!(
                        u.as_ref().unwrap_err().0,
                        code::QUESTION_NOT_WAITING,
                        "{name}"
                    );
                } else {
                    assert!(passed(&u), "{name}: {u:?}");
                }
            }
            "sensitive" => {
                let on_screen = row["on_mac_screen"] == true;
                let want = if on_screen {
                    code::NEEDS_THE_MAC
                } else {
                    code::UNLOCK_REQUIRED
                };
                assert_eq!(l.as_ref().unwrap_err().0, want, "{name}");
                assert_eq!(u.as_ref().unwrap_err().0, want, "{name}");
                let signed = open.signed_call(&keys.lock, name, params.clone()).await;
                if on_screen {
                    assert_eq!(signed.unwrap_err().0, code::NEEDS_THE_MAC, "{name}");
                } else {
                    assert!(passed(&signed), "{name}: {signed:?}");
                    // and a proof made for other parameters is not this call's
                    let c = open.challenge().await;
                    let p = call_proof(&keys.lock, &c, name, &json!({"other": 1}));
                    assert_eq!(
                        open.call(name, params.clone(), Some(p))
                            .await
                            .unwrap_err()
                            .0,
                        code::UNLOCK_INVALID,
                        "{name}"
                    );
                }
            }
            other => panic!("{other}"),
        }
        for refused in row["refused_params"].as_array().unwrap() {
            let mut p = serde_json::Map::new();
            p.insert(refused.as_str().unwrap().into(), json!(["/etc/hosts"]));
            assert_eq!(
                open.call(name, Value::Object(p), None).await.unwrap_err().0,
                wire::UNAUTHORIZED,
                "{name} {refused}"
            );
        }
        checked += 1;
    }
    assert!(checked >= 127, "{checked}");
}

/// A phone's message is the owner's, from that phone: the phone is the client (APP-916), whatever id it sent, and a
/// path on the Mac or the test tag never ride along.
// decision: PHN-111
// decision: PHN-471
#[tokio::test]
async fn a_phone_s_turn_names_the_phone_as_its_client() {
    let w = world(None).await;
    let keys = Keys::new(1);
    pair(&w, P1, &keys, false, false).await;
    let mut c = Conn::phone(&w, P1, "s").await;
    c.unlock(&keys).await;
    c.ok(
        "chat.send",
        json!({"text": "hi", "client_id": "ferb-app-install-id", "submission_id": "x1"}),
    )
    .await;
    let sent = c.events[0]["params"].clone();
    assert_eq!(sent["client_id"], format!("phone:{P1}"));
    assert_eq!(sent["submission_id"], "x1");
    for bad in [
        json!({"text": "x", "input_paths": ["/etc/hosts"]}),
        json!({"text": "x", "test_run": "t"}),
    ] {
        assert_eq!(
            c.call("chat.send", bad, None).await.unwrap_err().0,
            wire::UNAUTHORIZED
        );
    }
    c.ok("chat.entries.mark", json!({"client_id": "someone-else"}))
        .await;
    let marked = w.calls.lock().unwrap().last().unwrap().1.clone();
    assert_eq!(marked["client_id"], format!("phone:{P1}"));
}

/// PHN-120: with something that clicks prompts, an on-screen call opens FERB's own check on the phone; a signed Yes
/// clicks, a refused click says it must be done at the Mac; a No leaves it.
// decision: PHN-120
// decision: PHN-472
#[tokio::test]
async fn a_prompt_on_the_mac_is_clicked_on_the_phone_s_signed_yes() {
    for clickable in [true, false] {
        let prompts = Arc::new(FakePrompts {
            clickable,
            clicked: Mutex::new(vec![]),
        });
        let w = world(Some(prompts.clone())).await;
        let keys = Keys::new(1);
        pair(&w, P1, &keys, true, false).await;
        let mut c = Conn::phone(&w, P1, "s").await;
        c.unlock(&keys).await;
        let opened = c
            .signed_call(&keys.strong, "setup.permission_request", json!({}))
            .await
            .unwrap();
        assert_eq!(opened["mac_prompt"]["open"], true);
        let id = opened["mac_prompt"]["approval_id"]
            .as_str()
            .unwrap()
            .to_owned();
        let status = c.ok("chat.status", json!({})).await;
        let ap = status["approvals"]
            .as_array()
            .unwrap()
            .iter()
            .find(|a| a["approval_id"] == json!(id))
            .unwrap()
            .clone();
        let challenge = ap["phone"]["challenge"].as_str().unwrap().to_owned();
        let params = json!({"approval_id": id, "granted": true});
        let proof = signed(
            &keys.strong,
            &json!({"approval_id": id, "challenge": challenge, "method": "chat.approve", "params": params}).to_string(),
        );
        let answered = c.call("chat.approve", params, Some(proof)).await.unwrap();
        assert_eq!(answered["mac_prompt"]["clicked"], clickable, "{answered}");
        if !clickable {
            assert_eq!(answered["mac_prompt"]["needs_the_mac"], true);
        }
        assert_eq!(prompts.clicked.lock().unwrap().len(), 1);
    }
}

/// The audit (PHN-114, PHN-130): private, sensitive and answering calls and every refusal, with method, phone, time,
/// outcome; never a parameter's value; rows past 90 days go.
// decision: PHN-114
// decision: PHN-130
#[tokio::test]
async fn the_audit_records_what_phones_did_and_never_a_value() {
    let w = world(None).await;
    let keys = Keys::new(1);
    pair(&w, P1, &keys, false, false).await;
    let mut c = Conn::phone(&w, P1, "s").await;
    let marker = "VALUE-MARKER-99";
    let _ = c.call("settings.get", json!({"key": marker}), None).await; // locked: refused, audited
    c.unlock(&keys).await;
    c.ok("settings.get", json!({"key": marker})).await;
    c.signed_call(
        &keys.lock,
        "keys.store",
        json!({"provider": "x", "key": marker}),
    )
    .await
    .unwrap();
    c.ok("tasks.list", json!({})).await; // plain: not audited
    let audit = audit_text(&w);
    assert!(
        audit.contains(&format!("{P1} settings.get private locked")),
        "{audit}"
    );
    assert!(
        audit.contains(&format!("{P1} settings.get private ok")),
        "{audit}"
    );
    assert!(
        audit.contains(&format!("{P1} keys.store sensitive ok")),
        "{audit}"
    );
    assert!(
        audit.contains(&format!("{P1} phone.unlock link ok")),
        "{audit}"
    );
    assert!(!audit.contains("tasks.list"), "{audit}");
    assert!(!audit.contains(marker), "{audit}");
    // 91 days later the old rows go with the next one
    w.clock.advance(91.0 * 86400.0);
    c.unlock(&keys).await;
    let audit = audit_text(&w);
    assert_eq!(audit.lines().count(), 1, "{audit}");
}

/// PHN-419: a phone unused for 30 days is warned about once on the Mac and never removed; using it clears the warning.
// decision: PHN-419
#[tokio::test]
async fn an_unused_phone_is_warned_about_never_removed() {
    let w = world(None).await;
    let keys = Keys::new(1);
    pair(&w, P1, &keys, false, false).await;
    w.clock.advance(29.0 * 86400.0);
    assert!(w.phones.sweep_unused().is_empty());
    w.clock.advance(2.0 * 86400.0);
    let listed = w.phones.list(&app_session()).unwrap();
    assert_eq!(listed["phones"][0]["unused_warning"], true);
    assert!(w.phones.sweep_unused().is_empty(), "once");
    assert_eq!(
        w.journal
            .lock()
            .unwrap()
            .iter()
            .filter(|(k, _)| k == "phone.unused_warning")
            .count(),
        1
    );
    let mut c = Conn::phone(&w, P1, "s").await;
    c.ok("ping", json!({})).await;
    let listed = w.phones.list(&app_session()).unwrap();
    assert_eq!(listed["phones"][0]["unused_warning"], false);
    assert_eq!(listed["phones"].as_array().unwrap().len(), 1);
}

/// GATE-615 (owner 2026-11-17: no fingerprint for screen use from the phone; a button "don't ask to use screen until I
/// sent the next message on mac or return to mac"): "until I'm back at my Mac" is offered only on the screen-use
/// question, goes only with a Yes, only from this paired phone with its unlock window open (no fingerprint), and is
/// given with the name in FERB's list; the phone reads it in `chat.status`; the phone locking does not end it; an answer
/// that is not taken takes it back unsaid; removing the phone ends it.
// decision: GATE-615
// decision: PHN-586
// decision: PHN-587
#[tokio::test]
async fn until_i_m_back_goes_only_with_a_yes_to_the_screen_question_from_an_unlocked_phone() {
    let w = world(None).await;
    let (k1, k2) = (Keys::new(1), Keys::new(2));
    pair(&w, P1, &k1, true, true).await;
    pair(&w, P2, &k2, true, false).await;
    // the pairings' own windows (PHN-647) lapse first: these phones start locked
    w.clock.advance(301.0);
    let routine = QuestionFacts {
        pending: true,
        hidden: false,
        own_check: true,
        routine: true,
    };
    for id in ["screen", "fail-screen", "other"] {
        ask(&w, id, routine.clone());
    }
    w.questions.screen.lock().unwrap().insert("screen".into());
    w.questions
        .screen
        .lock()
        .unwrap()
        .insert("fail-screen".into());
    let mut c = Conn::phone(&w, P1, "s").await;
    // the paired phone, present but locked: a plain Yes from the notification, never "until I'm back"
    assert_eq!(c.present(&k1).await["present"], true);
    let back = |id: &str, granted: bool| json!({"approval_id": id, "granted": granted, "until_back": true});
    assert_eq!(
        c.call("chat.approve", back("screen", true), None)
            .await
            .unwrap_err()
            .0,
        code::LOCKED
    );
    c.unlock(&k1).await;
    // offered on the screen question alone, in what the phone reads
    let status = c.ok("chat.status", json!({})).await;
    let part = |id: &str| {
        status["approvals"]
            .as_array()
            .unwrap()
            .iter()
            .find(|a| a["approval_id"] == id)
            .unwrap()["phone"]
            .clone()
    };
    assert_eq!(part("screen")["until_back"], true);
    assert_eq!(part("other").get("until_back"), None);
    assert_eq!(status["screen_use"]["active"], false);
    // only with a Yes, only on the screen question
    for (id, granted) in [("screen", false), ("other", true)] {
        assert_eq!(
            c.call("chat.approve", back(id, granted), None)
                .await
                .unwrap_err()
                .0,
            code::BAD_REQUEST,
            "{id}"
        );
    }
    assert!(w.questions.until_back.lock().unwrap().is_none());
    // an answer that is not taken takes it back, unsaid
    c.call("chat.approve", back("fail-screen", true), None)
        .await
        .unwrap_err();
    assert!(w.questions.until_back.lock().unwrap().is_none());
    assert_eq!(
        w.questions.ended.lock().unwrap().clone(),
        vec![("answer_failed".to_owned(), Some(P1.to_owned()))]
    );
    // the Yes goes on to FERB without the extra field
    c.ok("chat.approve", back("screen", true)).await;
    assert_eq!(
        w.questions.until_back.lock().unwrap().clone(),
        Some(("screen".into(), P1.into(), format!("Phone {P1}")))
    );
    let sent = w.calls.lock().unwrap().last().unwrap().1.clone();
    assert_eq!(sent, json!({"approval_id": "screen", "granted": true}));
    assert_eq!(
        c.ok("chat.status", json!({})).await["screen_use"]["active"],
        true
    );
    // the phone locking does not end it (the owner is away from the Mac, phone in pocket)
    c.ok("phone.lock", json!({})).await;
    assert!(w.questions.until_back.lock().unwrap().is_some());
    // removing the phone that gave it ends it
    let mut other = Conn::phone(&w, P2, "t").await;
    other.unlock(&k2).await;
    other
        .signed_call(&k2.strong, "phone.remove", json!({"id": P1}))
        .await
        .unwrap();
    assert!(w.questions.until_back.lock().unwrap().is_none());
    assert_eq!(
        w.questions.ended.lock().unwrap().last().cloned(),
        Some(("phone_removed".to_owned(), Some(P1.to_owned())))
    );
}
