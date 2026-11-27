//! A message that replies to an entry (owner, 2026-11-27: he can quote any message in the chat, his own or FERB's,
//! when he sends a new one). chat.send's `reply_to` names an entry of the same conversation: the ack, the chat.message
//! row and the message entry's body keep `{entry_id, kind, who, excerpt}` (the row also where the models read the
//! quoted words); the front door and the planner read what he quoted, as data, beside his new words, with the quoted
//! line's `[cN]` tag; an earlier message's line says what it replied to. A quote FERB can't find is dropped: the
//! message goes without it. Through the real supervisor on the recorded plain_reply scenario with stand-in models
//! (temp store, injected ids and time).

mod common;
mod support_control;

use std::sync::atomic::Ordering;

use ferb_core::chat_entry::{self as ce, AnswerBody, Entry, EntryKind, EntryState, MessageBody};
use serde_json::{json, Value};
use support_control::{run_with, world_with, World};

const MSG: &str = "0123456789abcdef0123456789abcd01";
const REPLY: &str = "0123456789abcdef0123456789abcd02";
const LATER: &str = "0123456789abcdef0123456789abcd03";
const MSG_ENTRY: &str = "entry-invented-message";
const REPLY_ENTRY: &str = "entry-invented-reply";
const SAID: &str = "INVENTED-ADVICE Go for the invented phone 7, ideally 128 GB.";

fn row(w: &World, id: &str, at: &str, kind: &str, data: Value, made: &str) {
    let db = w.db.lock().unwrap();
    ferb_supervisor::flow::store::journal_row(db.conn(), id, at, kind, &data, Some(made)).unwrap();
}

fn entry(id: &str, conversation: &str, kind: EntryKind, request: &str, body: Value) -> Entry {
    Entry {
        id: id.into(),
        conversation_id: conversation.into(),
        pos: 0,
        seq: 0,
        rev: 1,
        kind,
        state: EntryState::Done,
        open: false,
        request_id: Some(request.into()),
        task_id: None,
        approval_id: None,
        reference: None,
        answers: None,
        placement: None,
        origin: None,
        client_id: None,
        submission_id: None,
        journal_id: None,
        created_at: "2026-01-01T08:00:00+00:00".into(),
        updated_at: "2026-01-01T08:00:00+00:00".into(),
        day: "2026-01-01".into(),
        body,
        deleted: false,
    }
}

/// The chat `s1`: the owner's message, FERB's reply to it (rows and entries), and a later message of his that replied
/// to that reply. `said` is the reply's words.
fn seed(w: &World, said: &str) {
    row(
        w,
        MSG,
        "2026-01-01T08:00:00+00:00",
        "chat.message",
        json!({"session_id": "s1", "request_id": "seedrequest1", "text": "INVENTED-ASK which invented phone?"}),
        "owner",
    );
    row(
        w,
        REPLY,
        "2026-01-01T08:00:10+00:00",
        "chat.reply",
        json!({"session_id": "s1", "request_id": "seedrequest1", "text": said}),
        "ferb,inferred",
    );
    row(
        w,
        LATER,
        "2026-01-01T08:01:00+00:00",
        "chat.message",
        json!({"session_id": "s1", "request_id": "seedrequest2", "text": "INVENTED-FOLLOW and the bigger one?",
               "reply_to": {"entry_id": REPLY_ENTRY, "kind": "answer", "who": "ferb",
                            "excerpt": "INVENTED-ADVICE Go for…", "line": REPLY, "origin": "ferb,inferred"}}),
        "owner",
    );
    let db = w.db.lock().unwrap();
    let conn = db.conn();
    ferb_store::chat_entries::create(
        conn,
        entry(
            MSG_ENTRY,
            "s1",
            EntryKind::Message,
            "seedrequest1",
            ce::body(&MessageBody {
                text: "INVENTED-ASK which invented phone?".into(),
                ..MessageBody::default()
            }),
        ),
    )
    .unwrap();
    ferb_store::chat_entries::create(
        conn,
        entry(
            REPLY_ENTRY,
            "s1",
            EntryKind::Answer,
            "seedrequest1",
            ce::body(&AnswerBody {
                from: "reply".into(),
                text: said.into(),
                status: "done".into(),
                ..AnswerBody::default()
            }),
        ),
    )
    .unwrap();
    // an entry of another conversation, and one deleted
    ferb_store::chat_entries::create(
        conn,
        entry(
            "entry-elsewhere",
            "s2",
            EntryKind::Answer,
            "seedrequest9",
            ce::body(&AnswerBody {
                from: "reply".into(),
                text: "INVENTED-ELSEWHERE".into(),
                ..AnswerBody::default()
            }),
        ),
    )
    .unwrap();
    ferb_store::chat_entries::create(
        conn,
        entry(
            "entry-gone",
            "s1",
            EntryKind::Message,
            "seedrequest3",
            ce::body(&MessageBody {
                text: "INVENTED-GONE".into(),
                ..MessageBody::default()
            }),
        ),
    )
    .unwrap();
    ferb_store::chat_entries::tombstone(conn, "entry-gone", "2026-01-01T08:02:00+00:00").unwrap();
}

/// plain_reply (the owner's "hi there" in `s1`) with `reply_to` on its chat.send, the planner's (or the front door's)
/// call answering "Hello.".
fn world(front: bool, said: &str, reply_to: Option<Value>) -> (World, support_control::Report) {
    let events = json!([
        {"kind": "text_delta", "data": {"text": "Hello."}},
        {"kind": "usage", "data": {"input_tokens": 10, "output_tokens": 5}},
        {"kind": "final", "data": {"text": "Hello."}}
    ]);
    let mut w = world_with("plain_reply", move |calls| {
        if front {
            for c in calls.iter_mut().filter(|c| c["key"] == "supervisor") {
                c["events"] = events.clone();
            }
        }
    });
    w.daemon.bounded.store(true, Ordering::SeqCst);
    w.daemon.front.store(front, Ordering::SeqCst);
    seed(&w, said);
    let report = run_with(&mut w, "plain_reply", |steps| {
        if let Some(quoted) = reply_to {
            for st in steps.iter_mut().filter(|st| st["do"] == "stream") {
                let mut sent: Value =
                    serde_json::from_str(st["lines"][0]["send"].as_str().unwrap()).unwrap();
                sent["params"]["reply_to"] = quoted.clone();
                st["lines"][0]["send"] = json!(sent.to_string());
            }
        }
    });
    w.sup.chat_flush();
    (w, report)
}

fn journal(w: &World, kind: &str) -> Vec<Value> {
    let db = w.db.lock().unwrap();
    let mut st = db
        .conn()
        .prepare("SELECT data_json FROM journal WHERE kind = ? ORDER BY seq")
        .unwrap();
    let rows = st
        .query_map([kind], |r| r.get::<_, String>(0))
        .unwrap()
        .map(|r| serde_json::from_str(&r.unwrap()).unwrap())
        .collect();
    rows
}

/// The new message's chat.message row (plain_reply's request) and its message entry.
fn the_message(w: &World) -> (Value, Entry) {
    let row = journal(w, "chat.message")
        .into_iter()
        .find(|r| r["request_id"] == "id-000015")
        .expect("the message's row");
    let db = w.db.lock().unwrap();
    let entry = ferb_store::chat_entries::of_requests(db.conn(), "s1", &["id-000015"])
        .unwrap()
        .into_iter()
        .find(|e| e.kind == EntryKind::Message)
        .expect("the message's entry");
    (row, entry)
}

fn ack(report: &support_control::Report) -> Value {
    report
        .streams
        .iter()
        .flat_map(|(_, lines)| lines.iter())
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .map(|v| v["params"].clone())
        .find(|p| p["type"] == "ack")
        .expect("the ack")
}

fn first_call_parts(w: &World) -> Vec<String> {
    let calls = w.daemon.calls.lock().unwrap();
    calls
        .started
        .iter()
        .find(|c| calls.made[c["n"].as_u64().unwrap() as usize]["runtime_role"] == "orchestrator")
        .expect("a planner or front door call")["parts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p.as_str().unwrap_or_default().to_string())
        .collect()
}

// decision: ORCH-750
#[test]
fn a_quote_is_kept_on_the_ack_the_row_and_the_entry_and_read_as_data_beside_his_words() {
    for front in [true, false] {
        let (w, report) = world(front, SAID, Some(json!(REPLY_ENTRY)));
        let shown =
            json!({"entry_id": REPLY_ENTRY, "kind": "answer", "who": "ferb", "excerpt": SAID});
        // every client reads the quote from the message entry's body; the app's live turn from the ack
        let (row, message) = the_message(&w);
        assert_eq!(message.body["reply_to"], shown, "{}", message.body);
        let body: MessageBody = message.body_as().unwrap();
        assert_eq!(body.text, "hi there");
        assert_eq!(body.reply_to.unwrap().excerpt, SAID);
        assert_eq!(ack(&report)["reply_to"], shown);
        // the row keeps where the models read the quoted words: its line and that line's origin
        let mut kept = shown.clone();
        kept["line"] = json!(REPLY);
        kept["origin"] = json!("ferb,inferred");
        assert_eq!(row["reply_to"], kept, "{row}");
        assert_eq!(row["text"], "hi there", "his words stay his words");
        // the front door's (and the planner's) input: the quote beside the new message, with its line's tag
        let parts = first_call_parts(&w);
        let all = parts.join("\n");
        let block = format!(
            "{}[c2] (FERB): \"{SAID}\"",
            ferb_supervisor::words::REPLYING_TO
        );
        assert!(
            parts.iter().any(|p| p == &block),
            "front {front}: the quote block:\n{all}"
        );
        let layer5 = parts
            .iter()
            .position(|p| p.starts_with("Layer 5 \u{2014} owner's complete new message:\n"))
            .expect("Layer 5");
        assert_eq!(
            parts[layer5],
            "Layer 5 \u{2014} owner's complete new message:\nhi there"
        );
        assert_eq!(parts[layer5 - 1], block, "right before his new message");
        // an earlier message's line says what it replied to, by the quoted line's tag
        assert!(
            all.contains(
                "[c3] (Thu 01 Jan 08:01) Owner, replying to [c2] (FERB): \"INVENTED-ADVICE Go for…\": \
                 INVENTED-FOLLOW and the bigger one?"
            ),
            "front {front}:\n{all}"
        );
        assert!(journal(&w, "chat.reply_to_dropped").is_empty());
    }
}

// decision: ORCH-750
#[test]
fn a_quoted_secret_stays_redacted_and_a_long_text_is_cut() {
    let begin = concat!("-----BEGIN RSA PRIV", "ATE KEY-----");
    let end = concat!("-----END RSA PRIV", "ATE KEY-----");
    let body = "QUJDZx9Qw3Er7Ty1Ui5Op2As";
    let said = format!(
        "INVENTED-KEY here it is:\n{begin}\n{body}\n{end}\n{}",
        "x".repeat(400)
    );
    let (w, report) = world(true, &said, Some(json!(REPLY_ENTRY)));
    let (row, message) = the_message(&w);
    let excerpt = message.body["reply_to"]["excerpt"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(
        excerpt.starts_with("INVENTED-KEY here it is: "),
        "{excerpt}"
    );
    // (the seeded reply's own line is not redacted: the test wrote it into the journal raw)
    let block = first_call_parts(&w)
        .into_iter()
        .find(|p| p.starts_with(ferb_supervisor::words::REPLYING_TO))
        .expect("the quote block");
    assert!(block.contains("[redacted]"), "{block}");
    for shown in [
        excerpt.clone(),
        row["reply_to"].to_string(),
        ack(&report).to_string(),
        block,
    ] {
        assert!(!shown.contains(body), "the key's body: {shown}");
        assert!(!shown.contains(begin), "{shown}");
    }
    assert!(excerpt.ends_with('…'), "{excerpt}");
    assert_eq!(excerpt.chars().count(), ce::REPLY_EXCERPT_CHARS + 1);
}

// decision: ORCH-750
#[test]
fn a_quote_ferb_cant_find_is_dropped_and_the_message_still_goes() {
    for (named, why) in [
        (json!("entry-nobody-made"), "unknown"),
        (json!("entry-elsewhere"), "other_conversation"),
        (json!("entry-gone"), "deleted"),
        (json!(42), "unknown"),
    ] {
        let (w, report) = world(true, SAID, Some(named.clone()));
        let (row, message) = the_message(&w);
        assert!(row.get("reply_to").is_none(), "{named}: {row}");
        assert!(message.body.get("reply_to").is_none(), "{named}");
        assert!(ack(&report).get("reply_to").is_none(), "{named}");
        let reply = report
            .streams
            .iter()
            .flat_map(|(_, lines)| lines.iter())
            .any(|l| l.contains("\"type\": \"reply\"") || l.contains("\"type\":\"reply\""));
        assert!(reply, "{named}: the message was still answered");
        let dropped = journal(&w, "chat.reply_to_dropped");
        assert_eq!(dropped.len(), 1, "{named}");
        assert_eq!(dropped[0]["why"], why, "{named}");
        assert_eq!(dropped[0]["request_id"], "id-000015");
        assert!(!first_call_parts(&w)
            .join("\n")
            .contains(ferb_supervisor::words::REPLYING_TO.trim_end()));
    }
}

#[test]
fn without_a_quote_nothing_changes() {
    let (w, report) = world(true, SAID, None);
    let (row, message) = the_message(&w);
    assert!(row.get("reply_to").is_none());
    assert!(!message.body.to_string().contains("reply_to"));
    assert!(ack(&report).get("reply_to").is_none());
    assert!(journal(&w, "chat.reply_to_dropped").is_empty());
}

#[test]
fn deleting_the_quoted_turn_takes_the_quote_from_the_message_that_quoted_it() {
    let (w, _) = world(true, SAID, Some(json!(REPLY_ENTRY)));
    let done = w.sup.inner().delete_turn("seedrequest1", true).unwrap();
    assert_eq!(done["deleted"]["quotes_removed"], 1, "{done}");
    let (_, message) = the_message(&w);
    assert!(message.body.get("reply_to").is_none(), "{}", message.body);
    assert_eq!(message.body["text"], "hi there", "the message stays");
    assert!(message.rev > 1);
}

struct NoReads;

impl ferb_supervisor::history::TimelineReads for NoReads {
    fn outcome_json(&self, _task_id: &str) -> Option<String> {
        None
    }
    fn notice_sources(&self, _notice_id: &str) -> Vec<String> {
        Vec::new()
    }
}

#[test]
fn a_line_shows_no_quote_whose_line_is_gone_and_the_excerpt_alone_for_one_with_no_line() {
    use ferb_supervisor::seams::JournalRow;
    let row = |id: &str, request: &str, quoted: Value| {
        JournalRow::from_json(&json!({"id": id, "at": "2026-01-01T08:00:00+00:00", "kind": "chat.message",
            "data": {"session_id": "s1", "request_id": request, "text": format!("INVENTED-{id}"), "reply_to": quoted},
            "origin": "owner"}))
        .unwrap()
    };
    let quote = |line: Option<&str>| {
        let mut q =
            json!({"entry_id": "e", "kind": "plan", "who": "ferb", "excerpt": "INVENTED-PLAN"});
        if let Some(line) = line {
            q["line"] = json!(line);
        }
        q
    };
    let rows = vec![
        row("m1", "r1", quote(Some("deleted-row"))),
        row("m2", "r2", quote(None)),
    ];
    let zone = ferb_clock::zone::Zone::utc();
    let (entries, _) = ferb_supervisor::history::timeline(&ferb_supervisor::history::Timeline {
        current: &std::collections::BTreeSet::new(),
        conversation: Some("s1"),
        rows: &rows,
        task_ids: Some(&std::collections::BTreeSet::new()),
        covered: &std::collections::BTreeSet::new(),
        reads: &NoReads,
        zone: &zone,
        lean: true,
    });
    let lines: Vec<&str> = entries.iter().map(|e| e.text.as_str()).collect();
    assert_eq!(lines[0], "[c1] (Thu 01 Jan 08:00) Owner: INVENTED-m1");
    assert_eq!(
        lines[1],
        "[c2] (Thu 01 Jan 08:00) Owner, replying to an earlier message (FERB): \"INVENTED-PLAN\": INVENTED-m2"
    );
}
