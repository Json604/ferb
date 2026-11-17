//! The planner's question that waits on the owner, as a phone's `chat.status` lists it (kb PHN-761, PHN-762): FERB's
//! clarifying question notifies on the phone the way it brings FERB.app's notch down, until the owner says anything
//! after it, from any client.

mod phone_support;

use phone_support::*;
use serde_json::{json, Value};

const P1: &str = "endpoint-asking";

/// One entry of a conversation as ChatProjector stores it (only the columns the rule reads matter).
fn entry(
    w: &World,
    conversation: &str,
    pos: i64,
    kind: &str,
    request: &str,
    body: Value,
    open: bool,
) {
    let db = w.db.lock().unwrap();
    db.conn()
        .execute(
            "INSERT INTO chat_entries (id, conversation_id, pos, seq, rev, kind, state, open, request_id, created_at,
                                       updated_at, day, body_json)
             VALUES (?1, ?2, ?3, ?3, 1, ?4, 'done', ?5, ?6, '2026-11-18T03:36:12+05:30', '2026-11-18T03:36:12+05:30',
                     '2026-11-18', ?7)",
            rusqlite::params![
                format!("{conversation}-{pos}"),
                conversation,
                pos,
                kind,
                open as i64,
                request,
                body.to_string()
            ],
        )
        .unwrap();
}

fn current(w: &World, conversation: &str) {
    let db = w.db.lock().unwrap();
    db.conn()
        .execute(
            "INSERT OR IGNORE INTO conversations (id) VALUES (?1)",
            [conversation],
        )
        .unwrap();
    db.conn()
        .execute(
            "INSERT OR REPLACE INTO current_conversation VALUES (1, ?1)",
            [conversation],
        )
        .unwrap();
}

fn deleted(w: &World, conversation: &str, pos: i64) {
    w.db.lock()
        .unwrap()
        .conn()
        .execute(
            "UPDATE chat_entries SET deleted = 1 WHERE id = ?1",
            [format!("{conversation}-{pos}")],
        )
        .unwrap();
}

fn clarifies(status: &Value) -> Vec<Value> {
    status["approvals"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|a| a["kind"] == "clarify")
        .cloned()
        .collect()
}

fn asks(text: &str) -> Value {
    json!({"from": "clarify", "text": text, "status": "done", "asks": true})
}

// decision: PHN-761
#[tokio::test]
async fn the_planner_s_question_waits_until_the_owner_says_anything_after_it() {
    let w = world(None).await;
    let keys = Keys::new(1);
    pair(&w, P1, &keys, false, true).await;
    let mut c = Conn::phone(&w, P1, "s").await;
    c.unlock(&keys).await;
    current(&w, "owner");
    // nothing asked yet: only the gateway's questions
    assert!(clarifies(&c.ok("chat.status", json!({})).await).is_empty());

    // the owner writes; FERB asks back (the turn's own answer asks)
    entry(
        &w,
        "owner",
        1,
        "message",
        "r1",
        json!({"text": "draft the mail"}),
        true,
    );
    entry(
        &w,
        "owner",
        2,
        "answer",
        "r1",
        asks("Which email account should I use?"),
        true,
    );
    let s = c.ok("chat.status", json!({})).await;
    let q = clarifies(&s);
    assert_eq!(q.len(), 1, "{s}");
    assert_eq!(q[0]["approval_id"], "clarify:r1");
    assert_eq!(q[0]["reason"], "Which email account should I use?");
    assert_eq!(q[0]["options"], json!([]));
    // answered by a message, never by a proof: routine on the phone's side
    assert_eq!(
        q[0]["phone"],
        json!({"can_answer": true, "needs_unlock": false})
    );

    // FERB says something else after it (a later reply, a notice's answer): his question still waits
    entry(
        &w,
        "owner",
        3,
        "answer",
        "r0",
        json!({"from": "task", "text": "Done.", "status": "done"}),
        false,
    );
    assert_eq!(
        clarifies(&c.ok("chat.status", json!({})).await)[0]["approval_id"],
        "clarify:r1"
    );

    // he answers, from any client (the Mac, this phone, another): it waits no more
    entry(
        &w,
        "owner",
        4,
        "message",
        "r2",
        json!({"text": "brave work profile"}),
        true,
    );
    assert!(clarifies(&c.ok("chat.status", json!({})).await).is_empty());

    // FERB asks again: a new question, a new id (notified once more)
    entry(
        &w,
        "owner",
        5,
        "answer",
        "r2",
        asks("Who goes in To and who in CC?"),
        true,
    );
    let q = clarifies(&c.ok("chat.status", json!({})).await);
    assert_eq!(q[0]["approval_id"], "clarify:r2");
    assert_eq!(q[0]["reason"], "Who goes in To and who in CC?");

    // a newer question takes its place: one waits, the newest
    entry(
        &w,
        "owner",
        6,
        "answer",
        "r3",
        asks("And the subject line?"),
        true,
    );
    let q = clarifies(&c.ok("chat.status", json!({})).await);
    assert_eq!((q.len(), &q[0]["approval_id"]), (1, &json!("clarify:r3")));

    // the owner deleted it from the chat: what was asked before it is not brought back
    deleted(&w, "owner", 6);
    assert_eq!(
        clarifies(&c.ok("chat.status", json!({})).await)[0]["approval_id"],
        "clarify:r2"
    );
    deleted(&w, "owner", 5);
    assert!(
        clarifies(&c.ok("chat.status", json!({})).await).is_empty(),
        "r1 was answered at 4"
    );

    // a fresh conversation: what the old one asked waits no more
    entry(&w, "owner", 7, "answer", "r4", asks("Which folder?"), true);
    assert_eq!(clarifies(&c.ok("chat.status", json!({})).await).len(), 1);
    current(&w, "fresh");
    assert!(clarifies(&c.ok("chat.status", json!({})).await).is_empty());
    entry(&w, "fresh", 1, "answer", "r5", asks("Which day?"), true);
    assert_eq!(
        clarifies(&c.ok("chat.status", json!({})).await)[0]["approval_id"],
        "clarify:r5"
    );
}

/// A locked phone learns that FERB asks, and its words only when the connection proved the phone's device key, as
/// for any question (PHN-126); FERB.app's own `chat.status` is not the phone gate's and is unchanged.
// decision: PHN-126
// decision: PHN-762
#[tokio::test]
async fn a_locked_phone_learns_the_planner_asks_with_its_words_held_as_any_question_s() {
    let w = world(None).await;
    let keys = Keys::new(1);
    pair(&w, P1, &keys, false, true).await;
    // the pairing's own window (PHN-647) lapses first: this is a locked phone's status
    w.clock.advance(301.0);
    current(&w, "owner");
    entry(
        &w,
        "owner",
        1,
        "message",
        "r1",
        json!({"text": "draft the mail"}),
        true,
    );
    entry(
        &w,
        "owner",
        2,
        "answer",
        "r1",
        asks("WORDS-OF-THE-QUESTION"),
        true,
    );
    let mut c = Conn::phone(&w, P1, "s").await;
    let s = c.ok("chat.status", json!({})).await;
    assert_eq!(s["locked"], true);
    let q = clarifies(&s);
    assert_eq!(q.len(), 1, "{s}");
    assert_eq!(
        (&q[0]["approval_id"], &q[0]["held"], &q[0]["reason"]),
        (
            &json!("clarify:r1"),
            &json!(true),
            &json!("FERB has a question")
        )
    );
    assert!(!s.to_string().contains("WORDS-OF-THE-QUESTION"), "{s}");
    c.present(&keys).await;
    let q = clarifies(&c.ok("chat.status", json!({})).await);
    assert_eq!(
        (&q[0]["reason"], &q[0]["held"]),
        (&json!("WORDS-OF-THE-QUESTION"), &json!(false))
    );
}
