//! PHN-846 through a real ferb-rpc server: the owner turns his phone's app lock off with one signed call; that phone's
//! device-proved connections then reach chat and tasks without the unlock window, and nothing else changes (files, a
//! hard Yes, a hidden answer, sensitive calls keep their proofs). On again needs no proof; removing or pairing again
//! clears it.

mod phone_support;

use ferb_daemon::phone::QuestionFacts;
use ferb_rpc::phone_gate as code;
use phone_support::*;
use serde_json::{json, Value};

const P1: &str = "endpoint-one";
const P2: &str = "endpoint-two";

fn refused(r: Result<Value, (i64, String)>) -> i64 {
    r.unwrap_err().0
}

/// This phone's row of `phone.list` as FERB.app sees it.
fn row_of(w: &World, endpoint: &str) -> Value {
    let listed = w.phones.list(&app_session()).unwrap();
    listed["phones"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["id"] == endpoint)
        .cloned()
        .unwrap_or(Value::Null)
}

// decision: PHN-846
// decision: PHN-129
#[tokio::test]
async fn only_the_phone_s_signed_call_turns_the_lock_off_and_on_again_needs_nothing() {
    let w = world(None).await;
    let keys = Keys::new(1);
    pair(&w, P1, &keys, true, true).await;
    w.clock.advance(301.0); // the pairing's own window lapses (PHN-647)
    let mut c = Conn::phone(&w, P1, "s").await;
    c.present(&keys).await;
    // no proof, the PIN key where the phone has a fingerprint key, another phone's key: nothing changes
    assert_eq!(
        refused(c.call("phone.lock_off", json!({}), None).await),
        code::UNLOCK_REQUIRED
    );
    assert_eq!(
        refused(c.signed_call(&keys.lock, "phone.lock_off", json!({})).await),
        code::UNLOCK_INVALID
    );
    assert_eq!(
        refused(
            c.signed_call(&Keys::new(9).strong, "phone.lock_off", json!({}))
                .await
        ),
        code::UNLOCK_INVALID
    );
    // a proof made for another call doesn't count either
    let ch = c.challenge().await;
    let other = call_proof(&keys.strong, &ch, "phone.lock_on", &json!({}));
    assert_eq!(
        refused(c.call("phone.lock_off", json!({}), Some(other)).await),
        code::UNLOCK_INVALID
    );
    assert_eq!(row_of(&w, P1)["lock_off"], false);
    assert_eq!(
        refused(c.call("tasks.list", json!({}), None).await),
        code::LOCKED
    );
    // the fingerprint's signed call: off, recorded for this phone, kept across a restart of the daemon
    let out = c
        .signed_call(&keys.strong, "phone.lock_off", json!({}))
        .await
        .unwrap();
    assert_eq!(out["lock_off"], true);
    assert_eq!(row_of(&w, P1)["lock_off"], true);
    let audit = audit_text(&w);
    assert!(audit.contains("phone.lock_off"), "{audit}");
    let w = reopen(w).await;
    assert_eq!(row_of(&w, P1)["lock_off"], true);
    // on again: no proof, even from a locked connection that proved nothing
    let mut bare = Conn::phone(&w, P1, "t").await;
    assert_eq!(bare.ok("phone.lock_on", json!({})).await["lock_off"], false);
    assert_eq!(row_of(&w, P1)["lock_off"], false);
    assert!(audit_text(&w).contains("phone.lock_on"));
    // a phone with no fingerprint key signs it with its screen lock's key
    let k2 = Keys::new(2);
    pair(&w, P2, &k2, false, true).await;
    let mut d = Conn::phone(&w, P2, "u").await;
    d.signed_call(&k2.lock, "phone.lock_off", json!({}))
        .await
        .unwrap();
    assert_eq!(row_of(&w, P2)["lock_off"], true);
    assert_eq!(row_of(&w, P1)["lock_off"], false, "per phone");
}

// decision: PHN-846
// decision: PHN-125
// decision: PHN-126
#[tokio::test]
async fn a_lock_off_phone_reaches_chat_and_tasks_without_the_unlock_and_everything_else_keeps_its_proof(
) {
    let w = world(None).await;
    let keys = Keys::new(1);
    pair(&w, P1, &keys, true, true).await;
    let mut c = Conn::phone(&w, P1, "s").await;
    c.present(&keys).await;
    c.signed_call(&keys.strong, "phone.lock_off", json!({}))
        .await
        .unwrap();
    // an hour later, the app locked meanwhile (`phone.lock`): no unlock, no window, chat and tasks still open
    c.ok("phone.lock", json!({})).await;
    w.clock.advance(3600.0);
    for (m, p) in [
        ("tasks.list", json!({})),
        ("chat.entries.days", json!({})),
        ("chat.entries.page", json!({"conversation": "c1"})),
        ("chat.history.days", json!({})),
        ("chat.temporary.get", json!({})),
        (
            "chat.entries.mark",
            json!({"conversation": "c1", "entry_id": "e1"}),
        ),
        ("attention.ack", json!({"ids": ["a1"]})),
        ("insomnia.status", json!({})),
        ("phone.list", json!({})),
    ] {
        c.call(m, p, None)
            .await
            .unwrap_or_else(|e| panic!("{m}: {e:?}"));
    }
    // sending is the owner's words, as from FERB.app (a stream)
    c.start("chat.send", json!({"text": "Water the plants at 6"}))
        .await;
    let sent = c.next_line().await;
    assert_eq!(sent["method"], "chat.send", "{sent}");
    // an ordinary answer from a device-proved connection, as before; a correction now too
    ask(&w, "plain", q(true, false, false, false));
    c.ok(
        "chat.approve",
        json!({"approval_id": "plain", "granted": true}),
    )
    .await;
    ask(&w, "c", q(true, false, false, false));
    c.ok(
        "chat.correct",
        json!({"approval_id": "c", "text": "the other folder"}),
    )
    .await;
    // what keeps its proof: a hard Yes, a hidden answer, files, settings, memory, a sensitive call
    ask(&w, "hard", q(true, false, true, false));
    assert_eq!(
        refused(
            c.call(
                "chat.approve",
                json!({"approval_id": "hard", "granted": true}),
                None
            )
            .await
        ),
        code::UNLOCK_REQUIRED
    );
    ask(&w, "secret", q(true, true, false, false));
    assert_eq!(
        refused(
            c.call(
                "chat.answer",
                json!({"approval_id": "secret", "text": "hunter2"}),
                None
            )
            .await
        ),
        code::UNLOCK_REQUIRED
    );
    for (m, p) in [
        ("phone.files", json!({"path": "~/Documents"})),
        ("phone.recent", json!({})),
        ("phone.search", json!({"path": "~", "query": "x"})),
        ("settings.get", json!({})),
        ("memory.list", json!({})),
        ("secrets.saved", json!({})),
        ("trace.get", json!({"task_id": "t1"})),
    ] {
        assert_eq!(refused(c.call(m, p, None).await), code::LOCKED, "{m}");
    }
    for m in ["settings.set", "stop.lift", "phone.remove", "memory.forget"] {
        assert_eq!(
            refused(c.call(m, json!({}), None).await),
            code::UNLOCK_REQUIRED,
            "{m}"
        );
    }
    // a connection that hasn't proved the phone's device key gets nothing more than before
    let mut bare = Conn::phone(&w, P1, "t").await;
    assert_eq!(
        refused(bare.call("tasks.list", json!({}), None).await),
        code::LOCKED
    );
    // the unlock opens the rest as before
    c.unlock(&keys).await;
    c.ok("settings.get", json!({})).await;
}

fn q(pending: bool, hidden: bool, own_check: bool, routine: bool) -> QuestionFacts {
    QuestionFacts {
        pending,
        hidden,
        own_check,
        routine,
    }
}

// decision: PHN-846
// decision: PHN-646
#[tokio::test]
async fn removing_the_phone_or_pairing_it_again_turns_the_lock_back_on() {
    let w = world(None).await;
    let keys = Keys::new(1);
    pair(&w, P1, &keys, true, true).await;
    w.clock.advance(301.0);
    let mut c = Conn::phone(&w, P1, "s").await;
    c.present(&keys).await;
    c.signed_call(&keys.strong, "phone.lock_off", json!({}))
        .await
        .unwrap();
    c.ok("tasks.list", json!({})).await;
    // the phone forgets itself; paired again it starts with its lock on, and gets nothing without the unlock
    c.ok("phone.forget_self", json!({})).await;
    pair(&w, P1, &keys, true, true).await;
    assert_eq!(row_of(&w, P1)["lock_off"], false);
    w.clock.advance(301.0);
    let mut again = Conn::phone(&w, P1, "s2").await;
    again.present(&keys).await;
    assert_eq!(
        refused(again.call("tasks.list", json!({}), None).await),
        code::LOCKED
    );
    // the owner removing it from another phone clears it too
    let k2 = Keys::new(2);
    pair(&w, P2, &k2, true, true).await;
    again
        .signed_call(&keys.strong, "phone.lock_off", json!({}))
        .await
        .unwrap();
    let mut other = Conn::phone(&w, P2, "o").await;
    other
        .signed_call(&k2.strong, "phone.remove", json!({"id": P1}))
        .await
        .unwrap();
    let rows = {
        let db = w.db.lock().unwrap();
        ferb_store::phones::all(db.conn()).unwrap()
    };
    assert!(
        rows.iter()
            .filter(|p| p.endpoint == P1)
            .all(|p| !p.lock_off),
        "{rows:?}"
    );
}
