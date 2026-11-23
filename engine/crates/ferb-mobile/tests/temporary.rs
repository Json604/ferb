//! The temporary chat (the owner, 2026-11-23) against the fake Mac: `chat.temporary.get`/`set` and `chat.send` with
//! `fresh: true` in their exact shapes, the turns folded by the fold every conversation uses, `chat.status`'s
//! `temporary` and the change it makes, off dropping the copy held, and nothing of it ever in the phone's store or
//! outbox (a dropped link is an error, never a queued message).

mod common;

use std::sync::Arc;

use common::mac::{FakeMac, SendOutcome};
use common::phone::{online, Phone};
use ferb_mobile::fold::Item;
use ferb_mobile::{Change, ErrorCode, Mobile};
use serde_json::json;

fn connected(phone: &Phone, mac: &FakeMac) -> Mobile {
    let m = phone.open();
    m.connect(Arc::new(mac.clone())).unwrap();
    m.link_event(&online("direct"));
    phone.jobs.run();
    m
}

fn heard(phone: &Phone, change: &Change) -> usize {
    phone
        .heard
        .0
        .lock()
        .unwrap()
        .iter()
        .filter(|c| *c == change)
        .count()
}

fn texts(items: &[Item]) -> Vec<String> {
    items
        .iter()
        .map(|i| serde_json::to_value(i).unwrap().to_string())
        .collect()
}

/// The icon's two calls: `set {on: true}` starts one (the same one while on), `get` reads it, `set {on: false}` ends
/// it and drops the copy held; each answer's shape is read as the contract gives it.
// decision: PHN-818
#[test]
fn set_and_get_have_the_contract_s_shapes_and_off_drops_the_copy() {
    let mac = FakeMac::new("C1");
    let phone = Phone::new();
    let m = connected(&phone, &mac);
    assert!(!m.temporary_chat().unwrap().on);
    assert_eq!(mac.calls_of("chat.temporary.get"), vec![json!({})]);
    assert_eq!(m.last_temporary_chat().unwrap(), None);

    phone.heard.0.lock().unwrap().clear();
    let on = m.set_temporary_chat(true).unwrap();
    assert_eq!(
        mac.calls_of("chat.temporary.set"),
        vec![json!({"on": true})]
    );
    assert!(on.on);
    assert_eq!(on.id.as_deref(), Some("T1"));
    assert_eq!(on.started_at.as_deref(), Some("2026-01-02T10:05:00+00:00"));
    assert_eq!(on.rev, 1);
    assert_eq!(on.transcript.conversation, "temporary:T1");
    assert!(on.transcript.items.is_empty() && on.transcript.outbox.is_empty());
    assert_eq!(heard(&phone, &Change::Temporary), 1);
    assert_eq!(m.last_temporary_chat().unwrap(), Some(on.clone()));
    // on while on keeps the same chat; reading it again changes nothing, so nothing is told
    assert_eq!(
        m.set_temporary_chat(true).unwrap().id.as_deref(),
        Some("T1")
    );
    assert_eq!(m.temporary_chat().unwrap(), on);
    assert_eq!(heard(&phone, &Change::Temporary), 1);

    let off = m.set_temporary_chat(false).unwrap();
    assert_eq!(mac.calls_of("chat.temporary.set")[2], json!({"on": false}));
    assert!(!off.on && off.id.is_none());
    assert_eq!(off.transcript.conversation, "");
    assert_eq!(m.last_temporary_chat().unwrap(), None, "off drops the copy");
    assert_eq!(heard(&phone, &Change::Temporary), 2);
    // a new one is another chat
    assert_eq!(
        m.set_temporary_chat(true).unwrap().id.as_deref(),
        Some("T2")
    );
}

/// A message goes straight to the Mac with `fresh: true` (never the outbox, no submission id), the Mac starts the chat
/// when none is on, and the answer is the chat read again, folded by the same fold as the normal chat. Nothing of it
/// is in the phone's store: not the conversations, not the outbox, not the normal chat, not after the app reopens.
// decision: PHN-818
#[test]
fn a_fresh_message_goes_now_and_nothing_of_it_reaches_the_phone_s_store() {
    let mac = FakeMac::new("C1");
    let phone = Phone::new();
    let m = connected(&phone, &mac);
    let before = m.conversations().unwrap();

    let t = m.send_temporary("what is 2+2").unwrap();
    let sends = mac.lock().sends.clone();
    assert_eq!(
        sends,
        vec![json!({"text": "what is 2+2", "fresh": true, "client_id": "phone-1"})]
    );
    assert!(t.on);
    assert_eq!(t.transcript.conversation, "temporary:T1");
    let shown = texts(&t.transcript.items);
    assert!(
        shown.iter().any(|s| s.contains("what is 2+2")),
        "the owner's turn: {shown:?}"
    );
    assert!(
        shown
            .iter()
            .any(|s| s.contains("Fresh answer to: what is 2+2")),
        "FERB's answer folded into it: {shown:?}"
    );
    assert_eq!(
        mac.calls_of("chat.temporary.get").len(),
        1,
        "read again after it"
    );

    // the phone's store and outbox never saw it
    assert!(m.outbox().unwrap().is_empty());
    assert_eq!(m.conversations().unwrap(), before);
    assert!(m.transcript("C1").unwrap().items.is_empty());
    assert!(
        mac.lock().entries.is_empty(),
        "the Mac's chat entries neither"
    );
    m.close();
    let again = phone.open();
    assert_eq!(again.last_temporary_chat().unwrap(), None);
    assert!(again.outbox().unwrap().is_empty());
    assert!(
        again
            .conversations()
            .unwrap()
            .iter()
            .all(|c| !c.id.starts_with("temporary:")),
        "no temporary conversation stored"
    );
}

/// No link, or one that drops: an error, and nothing waits in the outbox to go later (the words stay with the app).
// decision: PHN-818
#[test]
fn a_dropped_link_is_an_error_and_nothing_is_queued() {
    let mac = FakeMac::new("C1");
    let phone = Phone::new();
    let m = connected(&phone, &mac);
    mac.lock()
        .script
        .push_back(SendOutcome::Fail("mac_unreachable".into()));
    let e = m.send_temporary("hello").unwrap_err();
    assert_eq!(e.code, ErrorCode::Offline);
    assert!(m.outbox().unwrap().is_empty());
    m.flush_outbox().unwrap();
    assert_eq!(mac.lock().sends.len(), 1, "never sent again by itself");
    assert!(mac.lock().temporary.is_none());

    m.link_event(common::phone::OFFLINE);
    let e = m.send_temporary("hello").unwrap_err();
    assert_eq!(e.code, ErrorCode::Offline);
    assert_eq!(mac.lock().sends.len(), 1, "with no link nothing is tried");
    let e = m.set_temporary_chat(true).unwrap_err();
    assert_eq!(e.code, ErrorCode::Offline);
    assert!(m.outbox().unwrap().is_empty());
}

/// `chat.status`'s `temporary` is read into the status; a start, a moved rev and an end each tell `Temporary`; the
/// same rev tells nothing; an end (a status with words and no `temporary`) drops the copy held. A locked status that
/// holds its words back says nothing of it: locking the phone never reads as the chat ending.
// decision: PHN-818
#[test]
fn the_status_s_temporary_tells_each_move_and_its_end_drops_the_copy() {
    let mac = FakeMac::new("C1");
    let phone = Phone::new();
    let m = connected(&phone, &mac);
    assert_eq!(m.status().unwrap().temporary, None);

    m.set_temporary_chat(true).unwrap();
    phone.heard.0.lock().unwrap().clear();
    let s = m.status().unwrap();
    let t = s.temporary.clone().expect("read from chat.status");
    assert_eq!(
        (t.id.as_str(), t.rev, t.started_at.as_deref()),
        ("T1", 1, Some("2026-01-02T10:05:00+00:00"))
    );
    assert_eq!(heard(&phone, &Change::Temporary), 1);
    assert_eq!(m.last_status().unwrap().unwrap().temporary, Some(t));

    let push = |rev: i64, temporary: bool, locked: bool| {
        let mut status = json!({"approvals": [], "running": [], "results": [], "stopped": false,
                                "locked": locked, "words": !locked});
        if temporary {
            status["temporary"] =
                json!({"id": "T1", "rev": rev, "started_at": "2026-01-02T10:05:00+00:00"});
        }
        m.link_event(&json!({"type": "status", "status": status}).to_string());
    };
    push(1, true, false);
    assert_eq!(
        heard(&phone, &Change::Temporary),
        1,
        "the same rev: nothing"
    );
    push(2, true, false);
    assert_eq!(heard(&phone, &Change::Temporary), 2, "its turns changed");
    push(2, false, true);
    assert_eq!(
        heard(&phone, &Change::Temporary),
        2,
        "locked, words held: nothing"
    );
    assert_eq!(
        m.last_status().unwrap().unwrap().temporary.map(|t| t.rev),
        Some(2),
        "carried over while held"
    );
    assert!(m.last_temporary_chat().unwrap().is_some());
    push(3, false, false);
    assert_eq!(
        heard(&phone, &Change::Temporary),
        3,
        "ended on another client"
    );
    assert_eq!(m.last_status().unwrap().unwrap().temporary, None);
    assert_eq!(m.last_temporary_chat().unwrap(), None, "off drops it");
}

/// A temporary chat's question and task carry `"temporary": true` in `chat.status` (absent otherwise): read so, held in
/// memory with the status (never stored), so the app shows them only in the temporary chat.
// decision: PHN-818
#[test]
fn a_temporary_chat_s_question_and_task_are_marked_in_the_status() {
    let mac = FakeMac::new("C1");
    let phone = Phone::new();
    let m = connected(&phone, &mac);
    mac.lock().status = json!({"stopped": false, "results": [],
        "approvals": [
            {"approval_id": "q-t", "kind": "approve", "reason": "Open it?", "options": ["Yes", "No"], "task_id": "t-t",
             "temporary": true, "phone": {"can_answer": true, "needs_unlock": false}},
            {"approval_id": "q-n", "kind": "approve", "reason": "Pay?", "options": ["Yes", "No"],
             "phone": {"can_answer": true, "needs_unlock": false}}],
        "running": [{"task_id": "t-t", "name": "Look", "goal": "Look", "temporary": true}, {"task_id": "t-n"}]});
    let s = m.status().unwrap();
    assert!(s.questions[0].temporary && !s.questions[1].temporary);
    assert!(s.running[0].temporary && !s.running[1].temporary);
    m.close();
    let again = phone.open();
    assert_eq!(
        again.last_status().unwrap(),
        None,
        "the status is never stored"
    );
}
