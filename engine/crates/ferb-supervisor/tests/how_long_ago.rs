//! ORCH-730 (front door step 3c): a reply once called a message from two minutes earlier "last night". The planner's
//! changing tail (the situation block, after the cached part; never the cached conversation) says in plain words how
//! long ago the owner's previous message in this chat was and how long ago FERB's last reply was, from the journal's
//! times and the injected clock. Only with the bounded planner (a Python recording's replay keeps its input). Through
//! the real supervisor on the recorded plain_reply scenario with stand-in models (temp store, injected ids and time).

mod common;
mod support_control;

use std::sync::atomic::Ordering;

use ferb_supervisor::words::{ago, SITUATION_HEADER};
use serde_json::{json, Value};
use support_control::{run_with, world_with};

fn reply(text: &str) -> Value {
    let said = json!({"kind": "reply", "answers_question": false, "task": null, "control": null,
                      "side_entries": null, "text": text})
    .to_string();
    json!({"key": "supervisor", "events": [
        {"kind": "usage", "data": {"input_tokens": 10, "output_tokens": 5}},
        {"kind": "final", "data": {"text": said}}]})
}

/// The chat `s1` with one exchange (the owner's message at `asked`, FERB's reply at `replied`); the scenario's clock
/// reads 2026-01-01 09:00 UTC. The planner call's parts.
fn planner_parts(bounded: bool, asked: &str, replied: &str) -> Vec<String> {
    let mut w = world_with("plain_reply", |calls| calls.clear());
    w.daemon.bounded.store(bounded, Ordering::SeqCst);
    {
        let db = w.db.lock().unwrap();
        for (id, at, kind, text) in [
            ("seedmessage", asked, "chat.message", "INVENTED-MESSAGE"),
            ("seedreply", replied, "chat.reply", "INVENTED-REPLY"),
        ] {
            ferb_supervisor::flow::store::journal_row(
                db.conn(),
                id,
                at,
                kind,
                &json!({"session_id": "s1", "request_id": "seedrequest", "text": text}),
                Some("ferb"),
            )
            .unwrap();
        }
    }
    let _ = run_with(&mut w, "plain_reply", |steps| {
        steps.retain(|st| st["do"] != "stream" && st["do"] != "stream_end");
        let at = steps.iter().position(|st| st["do"] == "close").unwrap();
        let sent = json!({"jsonrpc": "2.0", "id": 2, "method": "chat.send",
                          "params": {"session_id": "s1", "text": "the new message"}});
        let ours = vec![
            json!({"do": "calls_from_here", "calls": [reply("Done.")]}),
            json!({"do": "stream", "conn": "app", "lines": [{"send": sent.to_string()}]}),
            json!({"do": "stream_end", "conn": "app", "lines": []}),
        ];
        for (i, st) in ours.into_iter().enumerate() {
            steps.insert(at + i, st);
        }
    });
    let started = w.daemon.calls.lock().unwrap().started.clone();
    started
        .iter()
        .map(|c| {
            c["parts"]
                .as_array()
                .unwrap()
                .iter()
                .map(|p| p.as_str().unwrap_or_default().to_string())
                .collect::<Vec<String>>()
        })
        .find(|p| p.iter().any(|t| t.starts_with("Layer 5 — conversation:")))
        .expect("a planner call")
}

fn with(parts: &[String], words: &str) -> Vec<String> {
    parts
        .iter()
        .filter(|p| p.contains(words))
        .cloned()
        .collect()
}

// decision: ORCH-730
#[test]
fn the_situation_says_how_long_ago_the_last_message_and_reply_were() {
    let parts = planner_parts(
        true,
        "2026-01-01T08:58:00+00:00",
        "2026-01-01T08:58:40+00:00",
    );
    let line = "In this chat, the owner's previous message was 2 minutes ago, and FERB's last reply was 1 minute ago.";
    let found = with(&parts, line);
    assert_eq!(found.len(), 1, "{parts:#?}");
    assert!(
        found[0].starts_with(SITUATION_HEADER),
        "in the changing tail: {}",
        found[0]
    );
    // days, as the owner's store has them after a quiet weekend
    let parts = planner_parts(
        true,
        "2025-12-29T07:00:00+00:00",
        "2025-12-29T07:01:00+00:00",
    );
    assert_eq!(
        with(
            &parts,
            "the owner's previous message was 3 days ago, and FERB's last reply was 3 days ago."
        )
        .len(),
        1,
        "{parts:#?}"
    );
}

// decision: ORCH-730 (a replay of a Python recording keeps its planner's input)
#[test]
fn a_replay_of_a_python_recording_has_no_such_line() {
    let parts = planner_parts(
        false,
        "2026-01-01T08:58:00+00:00",
        "2026-01-01T08:58:40+00:00",
    );
    assert!(with(&parts, "the owner's previous message was").is_empty());
}

// decision: ORCH-730
#[test]
fn ages_read_in_plain_words() {
    for (seconds, said) in [
        (0.0, "less than a minute ago"),
        (59.9, "less than a minute ago"),
        (60.0, "1 minute ago"),
        (130.0, "2 minutes ago"),
        (3600.0, "1 hour ago"),
        (86_399.0, "23 hours ago"),
        (86_400.0, "1 day ago"),
        (3.5 * 86_400.0, "3 days ago"),
        (-5.0, "less than a minute ago"),
    ] {
        assert_eq!(ago(seconds), said, "{seconds}");
    }
}
