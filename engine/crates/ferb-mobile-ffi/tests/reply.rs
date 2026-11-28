//! Reply to a message (APP-1146) as an app calls it: the transcript's items carry the quotes their bubbles offer,
//! `send_with(text, attachments, reply_to)` sends one as `chat.send`'s `reply_to`, a queued reply keeps its quote
//! across a reopen, and `quoted_item` finds the item a quote points to.

mod common;

use std::sync::Arc;

use common::{FakeMac, ForeignMac, Heard, OFFLINE, config, online, open};
use ferb_mobile::entry::ChatEntry;
use ferb_mobile_ffi::records::{Item, ReplyQuote, TurnItem};
use ferb_mobile_ffi::shown::quoted_item;
use serde_json::{Value, json};

fn entry(id: &str, pos: i64, kind: &str, extra: Value, body: Value) -> Value {
    let mut e = json!({"id": id, "conversation_id": "c1", "pos": pos, "seq": pos, "rev": 1, "kind": kind,
                       "state": "done", "open": false, "request_id": "r1",
                       "created_at": format!("2026-01-01T10:{pos:02}:00+00:00"),
                       "updated_at": format!("2026-01-01T10:{pos:02}:00+00:00"),
                       "day": "2026-01-01", "body": body, "deleted": false});
    if let (Some(e), Some(x)) = (e.as_object_mut(), extra.as_object()) {
        e.extend(x.clone());
    }
    e
}

fn chat() -> Vec<ChatEntry> {
    ChatEntry::list(&json!([
        entry(
            "m1",
            1,
            "message",
            json!({}),
            json!({"text": "Find a flight to Pune"})
        ),
        entry(
            "a1",
            2,
            "answer",
            json!({"answers": "m1", "placement": "inline"}),
            json!({"text": "The 9:40 is cheapest.", "status": "done"})
        ),
        entry(
            "m2",
            3,
            "message",
            json!({"request_id": "r2"}),
            json!({"text": "Book that one", "reply_to":
                     {"entry_id": "a1", "kind": "answer", "who": "ferb", "excerpt": "The 9:40 is cheapest."}})
        ),
    ]))
}

fn turns(items: &[Item]) -> Vec<&TurnItem> {
    items
        .iter()
        .filter_map(|i| match i {
            Item::Turn { turn } => Some(turn),
            _ => None,
        })
        .collect()
}

// decision: APP-1146
#[test]
fn a_reply_through_the_library_carries_its_quote_everywhere() {
    let mac = FakeMac::with("c1", &chat());
    let dir = tempfile::tempdir().unwrap();
    let heard = Arc::new(Heard::default());
    let m = open(dir.path(), config("phone-1"), heard.clone()).unwrap();
    m.connect_transport(ForeignMac::new(mac.clone())).unwrap();
    m.link_event(online("direct")).unwrap();
    m.sync().unwrap();
    let view = m.transcript("c1".into()).unwrap();
    let t = turns(&view.items);
    let answer = t[0]
        .answer_quote
        .clone()
        .expect("the answer offers a reply");
    assert_eq!(
        answer,
        ReplyQuote {
            entry_id: "a1".into(),
            kind: "answer".into(),
            by_owner: false,
            excerpt: "The 9:40 is cheapest.".into()
        }
    );
    assert_eq!(
        t[0].message_quote
            .as_ref()
            .map(|q| (q.entry_id.as_str(), q.by_owner)),
        Some(("m1", true))
    );
    assert_eq!(
        t[1].reply_to.as_ref(),
        Some(&answer),
        "the owner's message shows what it quotes"
    );
    assert_eq!(
        quoted_item(view.items.clone(), "a1".into()).as_deref(),
        Some("m1")
    );
    assert_eq!(quoted_item(view.items.clone(), "nope".into()), None);

    // offline: the reply waits with its quote, across a reopen
    m.link_event(OFFLINE.into()).unwrap();
    let item = m
        .send_with("And a window seat".into(), Vec::new(), Some(answer.clone()))
        .unwrap();
    assert_eq!(item.reply_to.as_ref(), Some(&answer));
    m.shutdown();
    let m = open(dir.path(), config("phone-1"), heard).unwrap();
    assert_eq!(m.outbox().unwrap()[0].reply_to.as_ref(), Some(&answer));
    m.connect_transport(ForeignMac::new(mac.clone())).unwrap();
    m.link_event(online("direct")).unwrap();
    m.sync().unwrap();
    assert!(m.outbox().unwrap().is_empty());
    let sent = mac.lock().sends.last().cloned().unwrap();
    assert_eq!(sent["reply_to"], "a1");
    m.sync().unwrap();
    let view = m.transcript("c1".into()).unwrap();
    let mine = turns(&view.items)
        .into_iter()
        .find(|t| {
            t.words
                .as_ref()
                .is_some_and(|w| w.text == "And a window seat")
        })
        .expect("the sent reply, from the Mac's entry");
    assert_eq!(
        mine.reply_to.as_ref().map(|q| q.entry_id.as_str()),
        Some("a1")
    );
    m.shutdown();
}
