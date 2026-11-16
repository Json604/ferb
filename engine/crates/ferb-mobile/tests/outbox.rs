//! The outbox (PHN-401): messages written while the Mac is unreachable stay visibly waiting and can be cancelled;
//! they go in order when the link returns, exactly once (the engine's submission ids, ENG-14), a refused one is
//! failed with its reason and never sent again. Answers are never queued (PHN-448); a hidden answer's value never
//! reaches the store, a listener or a proof. Forget this Mac deletes everything (PHN-408).

mod common;

use std::sync::{Arc, Mutex};

use common::mac::{FakeMac, SendOutcome};
use common::phone::{online, Phone, KEY, OFFLINE};
use ferb_mobile::store::Store;
use ferb_mobile::{Answer, ErrorCode, Signer};
use serde_json::Value;

fn texts(mac: &FakeMac) -> Vec<String> {
    mac.lock()
        .sends
        .iter()
        .map(|p| p["text"].as_str().unwrap_or_default().to_string())
        .collect()
}

// decision: PHN-401
// decision: ENG-14
// decision: PHN-476
#[test]
fn messages_wait_visibly_while_the_mac_is_off_and_go_in_order_when_it_returns() {
    let mac = FakeMac::new("C1");
    let phone = Phone::new();
    let m = phone.open();
    m.connect(Arc::new(mac.clone())).unwrap();
    // the link is down: kept, visible, cancellable
    let a = m.send("first").unwrap();
    let b = m.send("second").unwrap();
    let c = m.send("third").unwrap();
    assert_eq!(a.state, "waiting");
    assert_eq!(a.waiting_for.as_deref(), Some("link"));
    assert_eq!(m.outbox().unwrap().len(), 3);
    assert!(
        texts(&mac).is_empty(),
        "nothing tried while the link is down"
    );
    let cancelled = m.cancel(&b.id).unwrap();
    assert!(!cancelled.may_have_arrived);
    assert_eq!(
        m.outbox()
            .unwrap()
            .iter()
            .map(|i| i.text.as_str())
            .collect::<Vec<_>>(),
        ["first", "third"]
    );
    // the link returns
    m.link_event(&online("direct"));
    phone.jobs.run();
    assert_eq!(texts(&mac), ["first", "third"], "in order");
    assert!(m.outbox().unwrap().is_empty());
    let sent: Vec<Value> = mac.lock().sends.clone();
    assert!(sent.iter().all(|p| p["client_id"] == "phone-1"));
    assert_eq!(sent[0]["submission_id"], a.id.as_str());
    assert_eq!(sent[1]["submission_id"], c.id.as_str());
    assert!(
        sent.iter().all(|p| p.get("session_id").is_none()),
        "the engine writes in its current conversation"
    );
    // and the chat shows them, from the Mac's entries
    let t = m.transcript("C1").unwrap();
    assert_eq!(t.items.len(), 2);
}

/// A try whose outcome was unknown (the Mac took it, the ack was lost) goes again with the same submission id: the
/// Mac answers with the same acceptance and keeps one message.
// decision: PHN-401
// decision: ENG-14
// decision: PHN-476
#[test]
fn a_retry_after_an_unknown_outcome_never_duplicates() {
    let mac = FakeMac::new("C1");
    mac.lock().script.push_back(SendOutcome::AcceptThenDrop);
    let phone = Phone::new();
    let m = phone.open();
    m.connect(Arc::new(mac.clone())).unwrap();
    m.link_event(&online("direct"));
    phone.jobs.run();
    // the watch is up; the message goes and its ack is lost
    mac.lock().sink = None; // its entry won't arrive by the watch either
    let item = m.send("book the table").unwrap();
    assert_eq!(item.state, "waiting");
    assert!(item.tried, "the outcome is unknown: it may have arrived");
    assert_eq!(mac.messages_of(&item.id), 1);
    m.flush_outbox().unwrap();
    assert_eq!(mac.lock().sends.len(), 2);
    assert_eq!(
        mac.lock().sends[1]["submission_id"],
        item.id.as_str(),
        "the same id again"
    );
    assert_eq!(mac.messages_of(&item.id), 1, "one message on the Mac");
    assert!(m.outbox().unwrap().is_empty());
}

/// A message the Mac refuses is shown failed with the reason and never sent again; the next one still goes.
// decision: PHN-401
// decision: PHN-476
#[test]
fn a_refused_message_is_failed_with_its_reason_not_retried() {
    let mac = FakeMac::new("C1");
    mac.lock().script.extend([SendOutcome::Refuse(
        "that submission_id was already used for a different message".into(),
    )]);
    let phone = Phone::new();
    let m = phone.open();
    m.connect(Arc::new(mac.clone())).unwrap();
    m.send("one").unwrap();
    m.send("two").unwrap();
    m.link_event(&online("direct"));
    phone.jobs.run();
    let left = m.outbox().unwrap();
    assert_eq!(left.len(), 1);
    assert_eq!(left[0].state, "failed");
    assert_eq!(
        left[0].reason.as_deref(),
        Some("that submission_id was already used for a different message")
    );
    assert_eq!(texts(&mac), ["one", "two"]);
    m.flush_outbox().unwrap();
    m.sync().unwrap();
    assert_eq!(
        texts(&mac),
        ["one", "two"],
        "a failed one is never sent again"
    );
    m.cancel(&left[0].id).unwrap();
    assert!(m.outbox().unwrap().is_empty());
}

/// A locked phone's message waits for the unlock; one left sending by a process that ended waits again.
// decision: PHN-401
// decision: PHN-476
#[test]
fn a_message_waits_for_the_unlock_and_survives_a_restart() {
    let mac = FakeMac::new("C1");
    mac.lock()
        .script
        .push_back(SendOutcome::Fail("locked".into()));
    let phone = Phone::new();
    {
        let m = phone.open();
        m.connect(Arc::new(mac.clone())).unwrap();
        m.link_event(&online("direct"));
        phone.jobs.run();
        let item = m.send("hello").unwrap();
        assert_eq!(item.state, "waiting");
        assert_eq!(item.waiting_for.as_deref(), Some("unlock"));
        m.close();
    }
    // a row left `sending` (the app was killed mid-send)
    {
        let store = Store::open(&phone.path(), KEY.to_vec()).unwrap();
        let mut row = store.outbox().unwrap().remove(0);
        row.state = "sending".into();
        store.outbox_update(&row).unwrap();
    }
    let m = phone.open();
    let items = m.outbox().unwrap();
    assert_eq!(items[0].state, "waiting");
    assert!(items[0].tried);
    m.connect(Arc::new(mac.clone())).unwrap();
    m.link_event(&online("direct"));
    phone.jobs.run();
    assert!(m.outbox().unwrap().is_empty());
    assert_eq!(texts(&mac), ["hello", "hello"]);
}

struct Signs(Mutex<Vec<String>>);
impl Signer for Signs {
    fn sign(&self, payload: &str) -> Option<String> {
        self.0.lock().unwrap().push(payload.to_string());
        Some("SIG".into())
    }
}

const MARKER: &str = "MARKER-7731-not-a-real-code";

/// Answers are never queued; a hidden answer needs the phone's key and its value reaches only the Mac's call:
/// never the proof, the store, a listener or the outbox.
// decision: PHN-448
// decision: PHN-401
// decision: PHN-476
#[test]
fn answers_are_never_queued_and_a_hidden_value_touches_nothing_kept() {
    let mac = FakeMac::new("C1");
    let phone = Phone::new();
    let m = phone.open();
    m.connect(Arc::new(mac.clone())).unwrap();
    let e = m
        .answer(
            "a1",
            Answer::Approve {
                granted: true,
                choice: Some(0),
            },
            None,
        )
        .unwrap_err();
    assert_eq!(
        e.code,
        ErrorCode::Offline,
        "no link: not kept to send later"
    );
    assert!(m.outbox().unwrap().is_empty());
    m.link_event(&online("direct"));
    phone.jobs.run();
    let e = m
        .answer(
            "a1",
            Answer::Hidden {
                secret: MARKER.into(),
            },
            None,
        )
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::NeedsSigner);
    let signer = Arc::new(Signs(Mutex::new(Vec::new())));
    m.answer(
        "a1",
        Answer::Hidden {
            secret: MARKER.into(),
        },
        Some(signer.clone()),
    )
    .unwrap();
    let calls = mac.lock().calls.clone();
    let (method, params, proof) = calls.iter().find(|c| c.0 == "chat.answer").unwrap().clone();
    assert_eq!(method, "chat.answer");
    assert_eq!(
        params["text"], MARKER,
        "the value goes to the Mac's call only"
    );
    let proof: Value = serde_json::from_str(&proof.unwrap()).unwrap();
    let payload = proof["payload"].as_str().unwrap();
    assert!(
        !payload.contains(MARKER),
        "the signed text carries only the value's SHA-256"
    );
    assert!(payload.contains("\"sealed\""));
    assert_eq!(signer.0.lock().unwrap()[0], payload);
    // a Yes with the fingerprint: the exact call is signed
    m.answer(
        "a2",
        Answer::Approve {
            granted: true,
            choice: None,
        },
        Some(signer.clone()),
    )
    .unwrap();
    let signed: Value = serde_json::from_str(&signer.0.lock().unwrap()[1]).unwrap();
    assert_eq!(signed["method"], "chat.approve");
    assert_eq!(signed["params"]["granted"], true);
    // nothing kept holds it
    let heard = format!("{:?}", phone.heard.0.lock().unwrap());
    assert!(!heard.contains(MARKER));
    m.close();
    let store = Store::open(&phone.path(), KEY.to_vec()).unwrap();
    assert!(store.outbox().unwrap().is_empty());
    for c in store.conversations().unwrap() {
        for e in store.entries(&c.id).unwrap() {
            assert!(!e.to_json().to_string().contains(MARKER));
        }
    }
    drop(store);
    let bytes = std::fs::read(phone.path()).unwrap();
    assert!(!bytes.windows(MARKER.len()).any(|w| w == MARKER.as_bytes()));
}

/// Forget this Mac: the files are gone, every call answers closed, and a new store at the same place is empty.
// decision: PHN-408
// decision: PHN-476
#[test]
fn forget_this_mac_deletes_everything() {
    let (_, entries) = common::longest();
    let mac = FakeMac::with("C1", &entries);
    let phone = Phone::new();
    let m = phone.open();
    m.connect(Arc::new(mac.clone())).unwrap();
    m.link_event(&online("direct"));
    phone.jobs.run();
    m.send("kept until sent").unwrap();
    m.link_event(OFFLINE);
    m.send("waiting").unwrap();
    assert!(!m.transcript("C1").unwrap().items.is_empty());
    m.forget_this_mac().unwrap();
    assert!(!phone.path().exists(), "the database file is gone");
    for suffix in ["-journal", "-wal", "-shm"] {
        let mut p = phone.path().into_os_string();
        p.push(suffix);
        assert!(!std::path::Path::new(&p).exists());
    }
    assert_eq!(m.transcript("C1").unwrap_err().code, ErrorCode::Closed);
    assert_eq!(m.send("x").unwrap_err().code, ErrorCode::Closed);
    assert!(mac.lock().sink.is_none(), "the watch stopped");
    let again = Store::open(&phone.path(), [9u8; 32].to_vec()).unwrap();
    assert!(again.conversations().unwrap().is_empty());
    assert!(again.outbox().unwrap().is_empty());
}
