//! The phone library as an app calls it (kb PHN-446): the exported objects, records and callbacks, driven from Rust
//! the way the generated Kotlin and Swift call them, with ferb-mobile's fake Mac behind an app-supplied `Transport`.
//! The same objects reach Kotlin and Swift through UniFFI's scaffolding (README "Checks" runs the generated bindings).

mod common;

use std::sync::Arc;

use common::shared::{conversations, read, scenarios};
use common::{Cancels, FakeMac, ForeignMac, Heard, OFFLINE, Signs, Throws, config, online, open};
use ferb_mobile_ffi::records::{Change, Item, LinkPath, OlderState};
use ferb_mobile_ffi::{AnswerInput, FerbError, Mobile};
use serde_json::{Value, json};

fn connected(mac: &FakeMac, dir: &std::path::Path) -> (Arc<Mobile>, Arc<Heard>) {
    let heard = Arc::new(Heard::default());
    let m = open(dir, config("phone-1"), heard.clone()).unwrap();
    m.connect_transport(ForeignMac::new(mac.clone())).unwrap();
    m.link_event(online("direct")).unwrap();
    (m, heard)
}

/// Every one-conversation scenario: synced from the fake Mac through the exported objects, the transcript the app
/// receives (records, serialized as the core serializes its types) equals FERB.app's fold in `transcripts.json`.
// decision: PHN-446
// decision: PHN-492
// decision: PHN-473
// decision: ENG-22
#[test]
fn the_transcript_an_app_receives_is_what_the_mac_shows() {
    let golden = read("transcripts.json");
    let golden = golden["scenarios"].as_array().unwrap();
    let (mut checked, mut questions) = (0, 0);
    for ((file, name, entries), g) in scenarios().iter().zip(golden) {
        let convs = conversations(entries);
        if convs.len() != 1 {
            continue;
        }
        let (conv, list) = &convs[0];
        let mac = FakeMac::with(conv, list);
        let dir = tempfile::tempdir().unwrap();
        let (m, _) = connected(&mac, dir.path());
        m.sync().unwrap();
        assert_eq!(
            m.current_conversation().unwrap().as_deref(),
            Some(conv.as_str())
        );
        let view = m.transcript(conv.clone()).unwrap();
        let theirs = &g["conversations"][0];
        let items: Vec<Value> = view.items.iter().map(Item::to_json).collect();
        assert_eq!(
            Value::Array(items),
            theirs["items"],
            "{file} {name}: items differ"
        );
        let qs = serde_json::to_value(&view.questions).unwrap();
        assert_eq!(qs, theirs["questions"], "{file} {name}: questions differ");
        questions += view.questions.len();
        checked += 1;
        m.shutdown();
    }
    assert!(checked > 100, "only {checked} scenarios checked");
    assert!(questions > 0, "no scenario had a question");
}

/// A message written offline waits in the outbox, visible; online it goes once, with its submission id, and leaves
/// the outbox at its ack; the Mac's entry then shows it in the chat.
// decision: PHN-401
// decision: PHN-476
#[test]
fn send_waits_offline_then_goes_once() {
    let mac = FakeMac::new("c1");
    let dir = tempfile::tempdir().unwrap();
    let heard = Arc::new(Heard::default());
    let m = open(dir.path(), config("phone-1"), heard.clone()).unwrap();
    m.connect_transport(ForeignMac::new(mac.clone())).unwrap();
    m.link_event(OFFLINE.into()).unwrap();
    let waiting = m.send("Buy invented milk".into()).unwrap();
    assert_eq!(waiting.state, "waiting");
    assert_eq!(waiting.waiting_for.as_deref(), Some("link"));
    assert_eq!(m.outbox().unwrap().len(), 1);
    assert!(mac.lock().sends.is_empty(), "nothing goes while offline");
    assert!(heard.0.lock().unwrap().contains(&Change::Outbox));

    m.link_event(online("direct")).unwrap();
    m.sync().unwrap();
    assert!(m.outbox().unwrap().is_empty(), "removed at its ack");
    assert_eq!(mac.messages_of("sub-1"), 1);
    let sent = mac.lock().sends.clone();
    assert_eq!(sent[0]["client_id"], "phone-1");
    assert_eq!(sent[0]["submission_id"], "sub-1");
    m.sync().unwrap();
    let view = m.transcript("c1".into()).unwrap();
    assert!(
        view.items.iter().any(|i| matches!(i, Item::Turn { turn } if turn.words.as_ref().is_some_and(|w| w.text == "Buy invented milk"))),
        "the Mac's entry shows the message: {:?}",
        view.items
    );
    // cancel: a waiting message comes out; an unknown one is `not_found`
    m.link_event(OFFLINE.into()).unwrap();
    let w = m.send("Second".into()).unwrap();
    assert!(!m.cancel(w.id).unwrap().may_have_arrived);
    assert!(matches!(
        m.cancel("nope".into()),
        Err(FerbError::NotFound { .. })
    ));
}

/// Answers go now or not at all; with a signer the exact answer is signed over the Mac's challenge, and a hidden value
/// is signed only as its SHA-256; no signer for a hidden answer is `needs_signer`; a cancelled prompt sends nothing.
// decision: PHN-448
// decision: PHN-417
#[test]
fn answers_are_signed_never_queued() {
    let mac = FakeMac::new("c1");
    let dir = tempfile::tempdir().unwrap();
    let (m, _) = connected(&mac, dir.path());
    let signer = Arc::new(Signs::default());

    m.answer(
        "ap-1".into(),
        AnswerInput::Approve {
            granted: true,
            choice: None,
        },
        Some(signer.clone()),
    )
    .unwrap();
    let (method, params, proof) = mac.lock().calls.last().cloned().unwrap();
    assert_eq!(method, "chat.approve");
    assert_eq!(params, json!({"approval_id": "ap-1", "granted": true}));
    let proof: Value = serde_json::from_str(&proof.unwrap()).unwrap();
    assert_eq!(proof["sig"], "SIG");
    let payload: Value = serde_json::from_str(proof["payload"].as_str().unwrap()).unwrap();
    assert_eq!(payload["challenge"], "C-1");
    assert_eq!(payload["method"], "chat.approve");

    let marker = "invented-hidden-4711";
    m.answer(
        "ap-2".into(),
        AnswerInput::Hidden {
            secret: marker.into(),
        },
        Some(signer.clone()),
    )
    .unwrap();
    let signed = signer.0.lock().unwrap().last().cloned().unwrap();
    assert!(
        !signed.contains(marker),
        "the signed text never carries the value"
    );
    assert!(signed.contains("\"sealed\""));

    let before = mac.lock().calls.len();
    assert!(matches!(
        m.answer(
            "ap-3".into(),
            AnswerInput::Hidden {
                secret: marker.into()
            },
            None
        ),
        Err(FerbError::NeedsSigner { .. })
    ));
    assert!(matches!(
        m.answer(
            "ap-3".into(),
            AnswerInput::Words { text: "yes".into() },
            Some(Arc::new(Cancels))
        ),
        Err(FerbError::NotSigned { .. })
    ));
    let after: Vec<String> = mac.lock().calls[before..]
        .iter()
        .map(|c| c.0.clone())
        .collect();
    assert!(
        !after
            .iter()
            .any(|c| matches!(c.as_str(), "chat.approve" | "chat.answer" | "chat.correct")),
        "nothing answered: {after:?}"
    );

    // offline: never kept to send later
    m.link_event(OFFLINE.into()).unwrap();
    assert!(matches!(
        m.answer(
            "ap-4".into(),
            AnswerInput::Approve {
                granted: false,
                choice: None
            },
            None
        ),
        Err(FerbError::Offline { .. })
    ));
}

/// The public relay: an older page is refused before anything is asked, as `NeedsBetterPath` with its words; the
/// link state says the path; a refusal from the Mac is `refused` with the link's code.
// decision: PHN-444
#[test]
fn link_state_and_older_pages_on_the_public_relay() {
    // the longest one-conversation scenario, read two entries a page, so older pages exist
    let (conv, list) = scenarios()
        .into_iter()
        .filter_map(|(_, _, e)| {
            let c = conversations(&e);
            (c.len() == 1).then(|| c.into_iter().next().unwrap())
        })
        .max_by_key(|(_, l)| l.len())
        .unwrap();
    assert!(list.len() > 4);
    let mac = FakeMac::with(&conv, &list);
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = config("phone-1");
    cfg.page_limit = Some(2);
    let m = open(dir.path(), cfg, Arc::new(Heard::default())).unwrap();
    m.connect_transport(ForeignMac::new(mac.clone())).unwrap();
    m.link_event(online("direct")).unwrap();
    m.sync().unwrap();
    assert_eq!(
        m.transcript(conv.clone()).unwrap().older,
        OlderState::Available
    );
    m.link_event(online("shared_relay")).unwrap();
    let st = m.link_state().unwrap();
    assert_eq!(st.path, LinkPath::PublicRelay);
    assert!(!st.bulk_allowed);
    match m.load_older(conv.clone()).unwrap() {
        OlderState::NeedsBetterPath { message } => assert!(!message.is_empty()),
        other => panic!("{other:?}"),
    }
    mac.lock()
        .script
        .push_back(common::shared::mac::SendOutcome::Refuse(
            "FERB can't take that now.".into(),
        ));
    m.link_event(online("direct")).unwrap();
    let item = m.send("refused one".into()).unwrap();
    assert_eq!(item.state, "failed");
    assert_eq!(item.reason.as_deref(), Some("FERB can't take that now."));
    assert!(matches!(m.stop(vec!["t-1".into()]), Ok(())));
    mac.lock().online = false;
    let e = m.stop(vec!["t-1".into()]).unwrap_err();
    assert!(matches!(e, FerbError::Offline { .. }), "{e:?}");
    assert_eq!(e.link_code(), Some("mac_unreachable"));
    assert_eq!(e.code(), "offline");
}

/// Forget this Mac deletes the store's files; every call then answers `closed`, and the listener hears `Closed`.
// decision: PHN-408
#[test]
fn forget_deletes_the_store_and_closes() {
    let mac = FakeMac::new("c1");
    let dir = tempfile::tempdir().unwrap();
    let (m, heard) = connected(&mac, dir.path());
    m.sync().unwrap();
    let path = m.store_path().unwrap();
    assert!(std::path::Path::new(&path).exists());
    m.forget_this_mac().unwrap();
    assert!(
        !std::path::Path::new(&path).exists(),
        "the store's file is gone"
    );
    assert!(heard.0.lock().unwrap().contains(&Change::Closed));
    assert!(m.is_closed());
    assert!(matches!(m.forget_this_mac(), Err(FerbError::Closed { .. })));
    assert!(matches!(m.sync(), Err(FerbError::Closed { .. })));
}

/// Close is idempotent, and after it every call answers the stable code `closed` (none panics or hangs).
// decision: PHN-446
// decision: PHN-492
#[test]
fn close_is_idempotent_and_every_call_after_it_is_closed() {
    let mac = FakeMac::new("c1");
    let dir = tempfile::tempdir().unwrap();
    let (m, _) = connected(&mac, dir.path());
    m.sync().unwrap();
    m.shutdown();
    m.shutdown();
    assert!(m.is_closed());
    let c = "c1".to_string();
    let results: Vec<(&str, Option<FerbError>)> = vec![
        ("sync", m.sync().err()),
        ("conversations", m.conversations().err()),
        ("current", m.current_conversation().err()),
        ("transcript", m.transcript(c.clone()).err()),
        ("outbox", m.outbox().err()),
        ("send", m.send("x".into()).err()),
        ("flush", m.flush_outbox().err()),
        ("cancel", m.cancel("x".into()).err()),
        (
            "answer",
            m.answer("a".into(), AnswerInput::Words { text: "x".into() }, None)
                .err(),
        ),
        ("stop", m.stop(vec![]).err()),
        ("older", m.load_older(c.clone()).err()),
        ("mark", m.mark_read(c.clone(), 1).err()),
        ("link_state", m.link_state().err()),
        ("forget", m.forget_this_mac().err()),
        (
            "connect_transport",
            m.connect_transport(ForeignMac::new(mac.clone())).err(),
        ),
        ("link_event", m.link_event(OFFLINE.into()).err()),
        ("disconnect", m.disconnect().err()),
        ("store_path", m.store_path().err()),
    ];
    for (what, r) in results {
        let e = r.unwrap_or_else(|| panic!("{what} answered after close"));
        assert_eq!(e.code(), "closed", "{what}: {e:?}");
    }
    // the store's file stays after a close (only Forget this Mac deletes it), and opens again
    let again = open(dir.path(), config("phone-1"), Arc::new(Heard::default())).unwrap();
    assert_eq!(again.current_conversation().unwrap().as_deref(), Some("c1"));
    // a wrong key is `wrong_key`; a short one `bad_key`
    again.shutdown();
    let wrong = ferb_mobile_ffi::Mobile::open(
        dir.path()
            .join("ferb-phone.db")
            .to_string_lossy()
            .into_owned(),
        vec![9u8; 32],
        config("phone-1"),
        Arc::new(common::FixedClock),
        Arc::new(common::Counted::default()),
        Arc::new(Heard::default()),
    );
    assert!(matches!(wrong, Err(FerbError::WrongKey { .. })));
    let short = ferb_mobile_ffi::Mobile::open(
        dir.path().join("other.db").to_string_lossy().into_owned(),
        vec![1u8; 5],
        config("phone-1"),
        Arc::new(common::FixedClock),
        Arc::new(common::Counted::default()),
        Arc::new(Heard::default()),
    );
    assert!(matches!(short, Err(FerbError::BadKey { .. })));
}

/// No global state: two cores in one process, each with its own store and Mac, never see each other's chat; closing
/// one leaves the other working.
// decision: PHN-446
// decision: PHN-492
#[test]
fn two_objects_at_once() {
    let (a_mac, b_mac) = (FakeMac::new("ca"), FakeMac::new("cb"));
    let (da, db) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let (a, _) = connected(&a_mac, da.path());
    let (b, _) = connected(&b_mac, db.path());
    a.sync().unwrap();
    b.sync().unwrap();
    a.send("to A".into()).unwrap();
    b.send("to B".into()).unwrap();
    assert_eq!(a_mac.lock().sends.len(), 1);
    assert_eq!(b_mac.lock().sends.len(), 1);
    assert_eq!(a_mac.lock().sends[0]["text"], "to A");
    assert_eq!(b_mac.lock().sends[0]["text"], "to B");
    a.shutdown();
    assert!(a.is_closed() && !b.is_closed());
    b.sync().unwrap();
    assert_eq!(b.current_conversation().unwrap().as_deref(), Some("cb"));
    // the same, at once from threads
    let handles: Vec<_> = (0..4)
        .map(|i| {
            let b = b.clone();
            std::thread::spawn(move || b.send(format!("thread {i}")).unwrap())
        })
        .collect();
    for h in handles {
        h.join().unwrap();
    }
    assert_eq!(b_mac.lock().sends.len(), 5);
}

/// An app's callback that throws never takes the library down: the core goes on, the call returns.
// decision: PHN-446
#[test]
fn a_throwing_listener_is_contained() {
    let mac = FakeMac::new("c1");
    let dir = tempfile::tempdir().unwrap();
    let m = Mobile::open(
        dir.path()
            .join("ferb-phone.db")
            .to_string_lossy()
            .into_owned(),
        common::KEY.to_vec(),
        config("phone-1"),
        Arc::new(common::FixedClock),
        Arc::new(common::Counted::default()),
        Arc::new(Throws),
    )
    .unwrap();
    m.connect_transport(ForeignMac::new(mac.clone())).unwrap();
    m.link_event(online("direct")).unwrap();
    m.sync().unwrap();
    m.send("still works".into()).unwrap();
    assert_eq!(mac.messages_of("sub-1"), 1);
}
