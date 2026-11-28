//! Reply to a message (APP-1146): every bubble the phone's chat draws from an engine entry offers its quote (the
//! owner's messages, FERB's first replies, answers, later answers and notices, as FERB.app's do); a reply goes as
//! `chat.send`'s `reply_to`; a queued one keeps its quote across a restart; the owner's message shows the quote the
//! engine kept on its entry.

mod common;

use std::sync::Arc;

use common::mac::FakeMac;
use common::phone::{online, Phone};
use ferb_mobile::entry::ChatEntry;
use ferb_mobile::fold::{self, Item};
use ferb_mobile::store::{Store, MIGRATIONS};
use ferb_mobile::ReplyQuote;
use serde_json::{json, Value};

fn entry(id: &str, pos: i64, kind: &str, extra: Value, body: Value) -> Value {
    let mut e = json!({"id": id, "conversation_id": "C1", "pos": pos, "seq": pos, "rev": 1, "kind": kind,
                       "state": "done", "open": false, "request_id": "r1",
                       "created_at": format!("2026-01-01T10:{pos:02}:00+00:00"),
                       "updated_at": format!("2026-01-01T10:{pos:02}:00+00:00"),
                       "day": "2026-01-01", "body": body, "deleted": false});
    if let (Some(e), Some(x)) = (e.as_object_mut(), extra.as_object()) {
        e.extend(x.clone());
    }
    e
}

/// A turn with its plan and answer, a later answer, a notice, a message quoting the answer, and one of FERB.app's
/// carried rows.
fn chat() -> Vec<ChatEntry> {
    ChatEntry::list(&json!([
        entry(
            "m1",
            1,
            "message",
            json!({}),
            json!({"text": "Book a table for two at **Nori**"})
        ),
        entry(
            "p1",
            2,
            "plan",
            json!({}),
            json!({"plan_line": "Booking it now.", "lines": ["At Nori, 7pm"]})
        ),
        entry(
            "a1",
            3,
            "answer",
            json!({"answers": "m1", "placement": "inline"}),
            json!({"text": "Booked for 7pm.", "display_text": "## Booked\n- for 7pm", "status": "done"})
        ),
        entry(
            "a2",
            4,
            "answer",
            json!({"answers": "m1", "placement": "inline"}),
            json!({"text": "They confirmed by email.", "status": "done", "follows": "a1"})
        ),
        entry(
            "n1",
            5,
            "notice",
            json!({"request_id": null, "ref": "memory-1"}),
            json!({"text": "Now always on: window seats", "quotes": []})
        ),
        entry(
            "m2",
            6,
            "message",
            json!({"request_id": "r2"}),
            json!({"text": "Make it 8 instead", "reply_to":
                     {"entry_id": "a1", "kind": "answer", "who": "ferb", "excerpt": "## Booked\n- for 7pm"}})
        ),
        entry(
            "l1",
            7,
            "legacy",
            json!({"request_id": null}),
            json!({"kind": "turn", "body": {"id": "6F9619FF-8B86-D011-B42D-00C04FC964FF", "createdAt": 0,
                                              "request": "an older message", "answer": "an older answer",
                                              "status": "done"}})
        ),
    ]))
}

fn quote(id: &str, kind: &str, by_owner: bool, excerpt: &str) -> Option<ReplyQuote> {
    Some(ReplyQuote {
        entry_id: id.into(),
        kind: kind.into(),
        by_owner,
        excerpt: excerpt.into(),
    })
}

// decision: APP-1146 — each bubble drawn from an engine entry carries that entry's id and the words FERB.app quotes;
// FERB.app's saved form carries none (the engine can't quote it); the owner's message shows the quote kept on its entry
#[test]
fn items_carry_their_entry_ids_and_the_owners_message_shows_its_quote() {
    let t = fold::transcript(&chat(), 0);
    let Item::Turn(turn) = &t.items[0] else {
        panic!("{:?}", t.items[0])
    };
    assert_eq!(
        turn.message_quote,
        quote("m1", "message", true, "Book a table for two at Nori")
    );
    assert_eq!(
        turn.first_reply_quote,
        quote("p1", "plan", false, "Booking it now. At Nori, 7pm")
    );
    assert_eq!(
        turn.answer_quote,
        quote("a1", "answer", false, "Booked for 7pm"),
        "the words it shows"
    );
    assert_eq!(turn.reply_to, None);
    let Item::Reply(later) = &t.items[1] else {
        panic!("{:?}", t.items[1])
    };
    assert_eq!(
        later.quote,
        quote("a2", "answer", false, "They confirmed by email.")
    );
    let Item::Notice(notice) = &t.items[2] else {
        panic!("{:?}", t.items[2])
    };
    assert_eq!(
        notice.quote,
        quote("n1", "notice", false, "Now always on: window seats")
    );
    let Item::Turn(reply) = &t.items[3] else {
        panic!("{:?}", t.items[3])
    };
    assert_eq!(
        reply.reply_to,
        quote("a1", "answer", false, "Booked for 7pm"),
        "on one line, as drawn"
    );
    assert_eq!(
        reply.message_quote,
        quote("m2", "message", true, "Make it 8 instead")
    );
    let Item::Turn(carried) = &t.items[4] else {
        panic!("{:?}", t.items[4])
    };
    assert!(carried.legacy);
    assert_eq!(
        (&carried.message_quote, &carried.answer_quote),
        (&None, &None),
        "FERB.app's saved form: nothing the engine can quote"
    );
    // a tap on the quote goes to the item that shows the quoted entry
    assert_eq!(fold::quoted_item(&t.items, "a1"), Some("m1"));
    assert_eq!(fold::quoted_item(&t.items, "a2"), Some("a2"));
    assert_eq!(fold::quoted_item(&t.items, "n1"), Some("n1"));
    assert_eq!(fold::quoted_item(&t.items, "gone"), None);
    // the golden's bytes are FERB.app's: none of this is serialised
    let bytes = serde_json::to_string(&t.items).unwrap();
    assert!(
        !bytes.contains("quote\":") && !bytes.contains("reply_to"),
        "{bytes}"
    );
}

// decision: APP-1146 — an answer placed below is quoted where it is drawn: on the reply below, not on its turn
#[test]
fn an_answer_drawn_below_is_quoted_there() {
    let entries = ChatEntry::list(&json!([
        entry("m1", 1, "message", json!({}), json!({"text": "first"})),
        entry(
            "m2",
            2,
            "message",
            json!({"request_id": "r2"}),
            json!({"text": "second"})
        ),
        entry(
            "a1",
            3,
            "answer",
            json!({"answers": "m1", "placement": "below"}),
            json!({"text": "The first one's answer", "status": "done"})
        ),
    ]));
    let t = fold::transcript(&entries, 0);
    let Item::Turn(first) = &t.items[0] else {
        panic!("{:?}", t.items)
    };
    assert!(first.answered_below);
    assert_eq!(first.answer_quote, None);
    let below = t
        .items
        .iter()
        .find_map(|i| match i {
            Item::Reply(r) => Some(r),
            _ => None,
        })
        .expect("the reply below");
    assert_eq!(
        below.quote,
        quote("a1", "answer", false, "The first one's answer")
    );
    assert_eq!(
        fold::quoted_item(&t.items, "a1"),
        Some(below.entry.as_str())
    );
}

// decision: APP-1146 — a reply goes as chat.send's reply_to (the entry id); the Mac keeps the quote on the message's
// entry and the chat shows it
#[test]
fn a_reply_goes_with_reply_to_and_the_chat_shows_the_quote() {
    let mac = FakeMac::with("C1", &chat());
    let phone = Phone::new();
    let m = phone.open();
    m.connect(Arc::new(mac.clone())).unwrap();
    m.link_event(&online("direct"));
    phone.jobs.run();
    let t = m.transcript("C1").unwrap();
    let Item::Turn(turn) = &t.items[0] else {
        panic!("{:?}", t.items)
    };
    let quoted = turn
        .answer_quote
        .clone()
        .expect("the answer offers a reply");
    m.send_reply("And a high chair", &[], Some(&quoted))
        .unwrap();
    let sent = mac.lock().sends.last().cloned().unwrap();
    assert_eq!(sent["reply_to"], "a1");
    assert_eq!(sent["text"], "And a high chair");
    // a plain message carries none
    m.send("thanks").unwrap();
    assert!(mac.lock().sends.last().unwrap().get("reply_to").is_none());
    mac.deliver();
    let t = m.transcript("C1").unwrap();
    let mine = t
        .items
        .iter()
        .find_map(|i| match i {
            Item::Turn(x)
                if x.words
                    .as_ref()
                    .is_some_and(|w| w.text == "And a high chair") =>
            {
                Some(x)
            }
            _ => None,
        })
        .expect("the sent message, from the Mac's entry");
    assert_eq!(
        mine.reply_to.as_ref().map(|q| q.entry_id.as_str()),
        Some("a1")
    );
    assert!(mine.reply_to.as_ref().is_some_and(|q| !q.by_owner));
}

// decision: APP-1146 — a reply written while the Mac is away waits in the outbox with its quote, across a restart, and
// goes with it when the link returns
#[test]
fn a_queued_reply_keeps_its_quote_across_a_reopen() {
    let mac = FakeMac::with("C1", &chat());
    let phone = Phone::new();
    let quoted = quote("m1", "message", true, "Book a table for two at Nori").unwrap();
    {
        let m = phone.open();
        m.connect(Arc::new(mac.clone())).unwrap();
        let item = m
            .send_reply("for three, not two", &[], Some(&quoted))
            .unwrap();
        assert_eq!(item.state, "waiting");
        assert_eq!(item.reply_to.as_ref(), Some(&quoted));
        m.close();
    }
    let m = phone.open();
    let items = m.outbox().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(
        items[0].reply_to.as_ref(),
        Some(&quoted),
        "kept across the restart"
    );
    m.connect(Arc::new(mac.clone())).unwrap();
    m.link_event(&online("direct"));
    phone.jobs.run();
    assert!(m.outbox().unwrap().is_empty());
    let sent = mac.lock().sends.last().cloned().unwrap();
    assert_eq!(sent["reply_to"], "m1");
    assert_eq!(sent["submission_id"], items[0].id.as_str());
}

// decision: APP-1146 — a quote without an entry id is none; the excerpt the app hands in is cut as one made here
#[test]
fn a_quote_without_an_entry_id_is_none() {
    let mac = FakeMac::new("C1");
    let phone = Phone::new();
    let m = phone.open();
    m.connect(Arc::new(mac.clone())).unwrap();
    let blank = ReplyQuote {
        entry_id: " ".into(),
        kind: "answer".into(),
        by_owner: false,
        excerpt: "x".into(),
    };
    assert_eq!(
        m.send_reply("hi", &[], Some(&blank)).unwrap().reply_to,
        None
    );
    let long = ReplyQuote {
        entry_id: "a1".into(),
        kind: "answer".into(),
        by_owner: false,
        excerpt: "word ".repeat(100),
    };
    let kept = m
        .send_reply("hi", &[], Some(&long))
        .unwrap()
        .reply_to
        .unwrap();
    assert_eq!(
        kept.excerpt.chars().count(),
        ferb_mobile::reply::EXCERPT_CHARS
    );
}

// decision: APP-1146 — a store written before quotes (schema v3, a message waiting) opens upgraded: the message keeps
// waiting, with no quote
#[test]
fn an_older_store_with_a_waiting_message_upgrades() {
    let phone = Phone::new();
    {
        let conn = rusqlite_open(&phone.path());
        for step in &MIGRATIONS[..3] {
            conn.execute_batch(step).unwrap();
        }
        conn.execute_batch(
            "PRAGMA user_version = 3;
             INSERT INTO outbox (id, n, text, created_at, state) VALUES ('o1', 1, 'old', '2026-01-01T10:00:00Z', 'waiting');",
        )
        .unwrap();
    }
    let store = Store::open(&phone.path(), common::phone::KEY.to_vec()).unwrap();
    assert_eq!(store.version().unwrap(), MIGRATIONS.len() as i64);
    let rows = store.outbox().unwrap();
    assert_eq!((rows[0].text.as_str(), &rows[0].reply_to), ("old", &None));
}

/// The phone's database as an older build left it (SQLCipher under the tests' key).
fn rusqlite_open(path: &std::path::Path) -> rusqlite::Connection {
    let c = rusqlite::Connection::open(path).unwrap();
    let hex: String = common::phone::KEY
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    c.execute_batch(&format!("PRAGMA key = \"x'{hex}'\";"))
        .unwrap();
    c
}

// decision: APP-1146 — the temporary chat's bubbles offer their quotes as the chat's do (the Mac quotes from the
// temporary chat it holds in memory)
#[test]
fn the_temporary_chat_offers_its_quotes() {
    let reply = json!({"on": true, "id": "t1", "conversation": "temporary:t1", "rev": 1, "entries": [
        entry("tm1", 1, "message", json!({"conversation_id": "temporary:t1"}), json!({"text": "hi"})),
        entry("ta1", 2, "answer", json!({"conversation_id": "temporary:t1", "answers": "tm1", "placement": "inline"}),
              json!({"text": "Hello", "status": "done"})),
    ]});
    let chat = ferb_mobile::TemporaryChat::read(&reply, 0);
    let Item::Turn(t) = &chat.transcript.items[0] else {
        panic!("{:?}", chat.transcript.items)
    };
    assert_eq!(t.message_quote, quote("tm1", "message", true, "hi"));
    assert_eq!(t.answer_quote, quote("ta1", "answer", false, "Hello"));
}

// decision: APP-1146 — a temporary message that quotes one goes with chat.send's reply_to, as the chat's do; the
// watch's copy of the temporary chat offers its quotes too
#[test]
fn a_temporary_reply_goes_with_reply_to() {
    let mac = FakeMac::new("C1");
    let phone = Phone::new();
    let m = phone.open();
    m.connect(Arc::new(mac.clone())).unwrap();
    m.link_event(&online("direct"));
    phone.jobs.run();
    m.send_temporary("what is 2+2").unwrap();
    mac.temporary_said("answer", "four");
    phone.jobs.run();
    let chat = m.last_temporary_chat().unwrap().expect("on");
    let quoted = chat
        .transcript
        .items
        .iter()
        .find_map(|i| match i {
            Item::Turn(t) => t.answer_quote.clone().or_else(|| t.message_quote.clone()),
            Item::Reply(r) => r.quote.clone(),
            _ => None,
        })
        .expect("a bubble offers a reply");
    m.send_temporary_reply("and 3+3?", &[], Some(&quoted))
        .unwrap();
    let sent = mac.lock().sends.last().cloned().unwrap();
    assert_eq!(
        sent,
        json!({"text": "and 3+3?", "fresh": true, "client_id": "phone-1", "reply_to": quoted.entry_id})
    );
    // a plain one carries none
    m.send_temporary("thanks").unwrap();
    assert!(mac.lock().sends.last().unwrap().get("reply_to").is_none());
}
