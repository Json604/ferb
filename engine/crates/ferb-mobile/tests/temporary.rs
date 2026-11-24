//! The temporary chat (the owner, 2026-11-23) against the fake Mac: `chat.temporary.get`/`set` and `chat.send` with
//! `fresh: true` in their exact shapes, the turns folded by the fold every conversation uses, `chat.status`'s
//! `temporary` and the change it makes, off dropping the copy held, and nothing of it ever in the phone's store or
//! outbox (a dropped link is an error, never a queued message).

mod common;

use std::sync::Arc;

use common::mac::{FakeMac, SendOutcome};
use common::phone::{online, Phone};
use ferb_mobile::entry::ChatEntry;
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
// decision: PHN-837
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
// decision: PHN-837
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
// decision: PHN-837
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

/// `chat.status`'s `temporary` is read into the status. A start (on another client) and an end each tell `Temporary`
/// and move the one watch; a new rev of the chat already held tells nothing (its watch brings the change, ORCH-716);
/// an end (a status with words and no `temporary`) drops what was held. A locked status that holds its words back
/// says nothing of it: locking the phone never reads as the chat ending.
// decision: PHN-837
#[test]
fn the_status_s_temporary_tells_each_start_and_end_and_its_end_drops_the_copy() {
    let mac = FakeMac::new("C1");
    let phone = Phone::new();
    let m = connected(&phone, &mac);
    assert_eq!(m.status().unwrap().temporary, None);

    // started on another client: the status says so
    mac.lock().start_temporary();
    phone.heard.0.lock().unwrap().clear();
    let s = m.status().unwrap();
    let t = s.temporary.clone().expect("read from chat.status");
    assert_eq!(
        (t.id.as_str(), t.rev, t.started_at.as_deref()),
        ("T1", 1, Some("2026-01-02T10:05:00+00:00"))
    );
    assert_eq!(heard(&phone, &Change::Temporary), 1);
    assert_eq!(m.last_status().unwrap().unwrap().temporary, Some(t));
    assert!(m.last_temporary_chat().unwrap().unwrap().on);

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
    phone.jobs.run();
    push(2, true, false);
    assert_eq!(
        heard(&phone, &Change::Temporary),
        1,
        "a new rev of the chat held: its watch brings the change"
    );
    push(2, false, true);
    assert_eq!(
        heard(&phone, &Change::Temporary),
        1,
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
        2,
        "ended on another client"
    );
    assert_eq!(m.last_status().unwrap().unwrap().temporary, None);
    assert_eq!(m.last_temporary_chat().unwrap(), None, "off drops it");
}

/// A temporary chat's question and task carry `"temporary": true` in `chat.status` (absent otherwise): read so, held in
/// memory with the status (never stored), so the app shows them only in the temporary chat.
// decision: PHN-837
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

/// The temporary chat is drawn as the chat (APP-1126): a turn still working shows the chat's live view over it, the
/// typing dots right after his message, as the normal chat shows them before any frame comes; once answered, nothing
/// live. Before, its transcript carried no live view at all, so nothing showed until FERB's answer was stored.
// decision: APP-1126, PHN-837
#[test]
fn a_working_turn_shows_the_chat_s_live_view_and_an_answered_one_none() {
    let entry = |id: &str, pos: i64, kind: &str, state: &str, body: serde_json::Value| {
        json!({"id": id, "conversation_id": "temporary:T1", "pos": pos, "seq": pos, "rev": 1, "kind": kind,
               "state": state, "open": kind == "message" && state == "working", "request_id": "temporary-r1",
               "client_id": "phone-1", "answers": if kind == "answer" { json!("e1") } else { json!(null) },
               "placement": if kind == "answer" { json!("inline") } else { json!(null) },
               "created_at": "2026-01-02T10:06:00+00:00", "updated_at": "2026-01-02T10:06:00+00:00",
               "day": "2026-01-02", "body": body, "deleted": false})
    };
    let message = entry(
        "e1",
        1,
        "message",
        "working",
        json!({"text": "what is 2+2", "attachments": [], "hidden": []}),
    );
    let chat = |entries: Vec<serde_json::Value>| {
        ferb_mobile::TemporaryChat::read(
            &json!({"on": true, "id": "T1", "conversation": "temporary:T1", "started_at": "2026-01-02T10:05:00+00:00",
                    "rev": 2, "entries": entries}),
            0,
        )
    };
    let working = chat(vec![message.clone()]);
    assert_eq!(working.transcript.live.len(), 1, "{:?}", working.transcript);
    assert!(
        working.transcript.live[0].typing,
        "his message taken, nothing said yet: the typing dots"
    );

    let mut done = message.clone();
    done["state"] = json!("done");
    done["open"] = json!(false);
    let answered = chat(vec![
        done,
        entry("e2", 2, "answer", "done", json!({"text": "4"})),
    ]);
    assert!(
        answered.transcript.live.is_empty(),
        "answered: nothing live"
    );
}

fn temp_texts(m: &Mobile) -> Vec<String> {
    texts(&m.last_temporary_chat().unwrap().unwrap().transcript.items)
}

/// The owner (2026-11-24, ORCH-715): "isnt everything same but only its a non saving chat???". The one watch follows
/// the conversation the chat tab shows: the temporary chat while it is on, read from its start (its id the epoch), so
/// its turns and live frames (the streamed plan line, steps, words) arrive as the chat's do, told as `Temporary` and
/// read from memory (`last_temporary_chat`), never with a `chat.temporary.get` per change. The chat's own changes wait
/// for its watch to come back, which catches them up from its cursor when the temporary chat ends. Nothing of the
/// temporary chat reaches the store. Before, the phone only read the whole chat again on each status rev.
// decision: ORCH-715
// decision: PHN-837
#[test]
fn the_one_watch_follows_the_temporary_chat_and_streams_it_like_the_chat() {
    let mac = FakeMac::new("C1");
    let phone = Phone::new();
    let m = connected(&phone, &mac);
    assert_eq!(mac.watched(), "C1");

    m.set_temporary_chat(true).unwrap();
    phone.jobs.run();
    assert_eq!(
        mac.watched(),
        "temporary:T1",
        "the watch follows the chat tab"
    );
    let watch = mac.calls_of("chat.entries.watch").last().unwrap().clone();
    assert_eq!(
        (
            watch["conversation"].clone(),
            watch["epoch"].clone(),
            watch["after_seq"].clone()
        ),
        (json!("temporary:T1"), json!("T1"), json!(0)),
        "from its start"
    );

    phone.heard.0.lock().unwrap().clear();
    mac.temporary_working("look this up");
    assert!(temp_texts(&m).iter().any(|t| t.contains("look this up")));
    let live = m.last_temporary_chat().unwrap().unwrap().transcript.live;
    assert!(live[0].typing, "his turn working: the typing dots");
    mac.live(
        "T1-r1",
        json!({"type": "progress", "planning": true, "text": "Reading the page"}),
    );
    let live = m.last_temporary_chat().unwrap().unwrap().transcript.live;
    assert_eq!(
        live[0].line.as_ref().map(|l| l.text.clone()),
        Some("Reading the page".into()),
        "the plan line streams in, as in the chat"
    );
    mac.temporary_said("answer", "Here it is");
    assert!(temp_texts(&m).iter().any(|t| t.contains("Here it is")));
    let heard_now = phone.heard.0.lock().unwrap().clone();
    assert!(
        heard_now
            .iter()
            .filter(|c| **c == Change::Temporary)
            .count()
            >= 3
    );
    assert!(
        !heard_now
            .iter()
            .any(|c| matches!(c, Change::Transcript { conversation } | Change::Live { conversation } if conversation.starts_with("temporary:"))),
        "told as Temporary only: {heard_now:?}"
    );
    assert!(
        mac.calls_of("chat.temporary.get").is_empty(),
        "never read again per change"
    );

    // the normal chat said something meanwhile: it waits for its own watch
    mac.commit(
        ChatEntry::list(&json!([{"id": "n1", "conversation_id": "C1", "pos": 1, "seq": 0, "rev": 1,
            "kind": "notice", "state": "done", "open": false, "created_at": "2026-01-02T10:07:00+00:00",
            "updated_at": "2026-01-02T10:07:00+00:00", "day": "2026-01-02", "body": {"text": "A reminder"},
            "deleted": false}]))
        .remove(0),
    );
    assert!(m.transcript("C1").unwrap().items.is_empty());

    // ended on another client: its watch says so; the phone drops it and watches the chat again, caught up
    mac.temporary_ended();
    assert_eq!(m.last_temporary_chat().unwrap(), None);
    phone.jobs.run();
    assert_eq!(mac.watched(), "C1");
    assert!(texts(&m.transcript("C1").unwrap().items)
        .iter()
        .any(|t| t.contains("A reminder")));
    assert!(m
        .conversations()
        .unwrap()
        .iter()
        .all(|c| !c.id.starts_with("temporary:")));
}

/// The same watch, the same handling: a lock closes it and the unlock's sync starts it again where it was; a link
/// that drops lets go of what is live and comes back to it from its cursor; a daemon restart ends the temporary chat
/// (its watch answers `ended`) and the phone is back on the chat.
// decision: PHN-837
#[test]
fn lock_link_loss_and_restart_are_handled_as_the_chat_s() {
    let mac = FakeMac::new("C1");
    let phone = Phone::new();
    let m = connected(&phone, &mac);
    m.set_temporary_chat(true).unwrap();
    phone.jobs.run();
    mac.temporary_said("message", "first");
    mac.temporary_said("answer", "one");

    m.lock().unwrap();
    assert_eq!(mac.watched(), "", "locked: no watch");
    mac.temporary_said("message", "second");
    m.unlocked();
    m.sync().unwrap();
    phone.jobs.run();
    assert_eq!(mac.watched(), "temporary:T1");
    let watch = mac.calls_of("chat.entries.watch").last().unwrap().clone();
    assert_eq!(watch["after_seq"], json!(2), "from where it was");
    assert!(temp_texts(&m).iter().any(|t| t.contains("second")));

    m.link_event(common::phone::OFFLINE);
    assert_eq!(mac.watched(), "");
    mac.temporary_said("answer", "two");
    m.link_event(&online("direct"));
    phone.jobs.run();
    assert_eq!(mac.watched(), "temporary:T1");
    assert!(temp_texts(&m).iter().any(|t| t.contains("two")));

    // the Mac restarted: the temporary chat is gone, nothing said it on this watch
    m.link_event(common::phone::OFFLINE);
    mac.lock().temporary = None;
    m.link_event(&online("direct"));
    phone.jobs.run();
    assert_eq!(m.last_temporary_chat().unwrap(), None);
    assert_eq!(mac.watched(), "C1", "back on the chat");
}

/// Files in a temporary chat go as the chat's do: this phone's finished uploads by id with the message (`fresh`), the
/// Mac turning them into its files.
// decision: PHN-838
#[test]
fn a_temporary_message_carries_its_files_as_the_chat_s() {
    let mac = FakeMac::new("C1");
    let phone = Phone::new();
    let m = connected(&phone, &mac);
    let files = [ferb_mobile::store::Attachment {
        upload_id: "U-1".into(),
        name: "photo.jpg".into(),
    }];
    m.send_temporary_with("what is this", &files).unwrap();
    assert_eq!(
        mac.lock().sends.last().unwrap().clone(),
        json!({"text": "what is this", "fresh": true, "client_id": "phone-1", "attachments": ["U-1"]})
    );
    assert!(m.outbox().unwrap().is_empty());
}
