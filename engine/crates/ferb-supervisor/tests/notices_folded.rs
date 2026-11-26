//! ORCH-728 (front door step 3a): FERB's own notices leave the planner's word-for-word window. A run of consecutive
//! notices is one line (how many FERB posted there, their tags and times, chat.history reads them); a notice that is
//! the last entry before the owner's new message is still shown in full. Nothing leaves the journal. Only with the
//! bounded planner (a Python recording's replay keeps its input). Through the real supervisor on the recorded
//! plain_reply scenario with stand-in models (temp store, injected ids and time), and the rule on invented entries.

mod common;
mod support_control;

use std::sync::atomic::Ordering;

use ferb_clock::zone::Zone;
use ferb_supervisor::history::{fold_notices, Entry};
use serde_json::{json, Value};
use support_control::{run_with, world_with, World};

fn reply(text: &str) -> Value {
    let said = json!({"kind": "reply", "answers_question": false, "task": null, "control": null,
                      "side_entries": null, "text": text})
    .to_string();
    json!({"key": "supervisor", "events": [
        {"kind": "usage", "data": {"input_tokens": 10, "output_tokens": 5}},
        {"kind": "final", "data": {"text": said}}]})
}

fn row(w: &World, id: &str, at: &str, kind: &str, data: Value) {
    let db = w.db.lock().unwrap();
    ferb_supervisor::flow::store::journal_row(db.conn(), id, at, kind, &data, Some("ferb"))
        .unwrap();
}

fn notice(n: usize) -> String {
    format!("INVENTED-NOTICE-{n} Now always on: an invented fact")
}

/// The chat `s1`: an exchange, five notices, an exchange, two notices; then the scenario's message. The planner's
/// conversation entries (one block each, tagged `[cN]`).
fn planner_entries(bounded: bool) -> (Vec<String>, i64) {
    let mut w = world_with("plain_reply", |calls| calls.clear());
    w.daemon.bounded.store(bounded, Ordering::SeqCst);
    let exchange = |n: usize, minute: usize| {
        row(
            &w,
            &format!("seedmessage{n}"),
            &format!("2026-01-01T08:{minute:02}:00+00:00"),
            "chat.message",
            json!({"session_id": "s1", "request_id": format!("seedrequest{n}"), "text": format!("INVENTED-MESSAGE-{n}")}),
        );
        row(
            &w,
            &format!("seedreply{n}"),
            &format!("2026-01-01T08:{minute:02}:30+00:00"),
            "chat.reply",
            json!({"session_id": "s1", "request_id": format!("seedrequest{n}"), "text": format!("INVENTED-REPLY-{n}")}),
        );
    };
    let posted = |n: usize, minute: usize| {
        row(
            &w,
            &format!("seednotice{n}"),
            &format!("2026-01-01T08:{minute:02}:00+00:00"),
            "chat.notice",
            json!({"id": format!("memory-always-on-{n}"), "text": notice(n), "quotes": []}),
        );
    };
    exchange(0, 0);
    for n in 0..5 {
        posted(n, 10 + n);
    }
    exchange(1, 20);
    posted(5, 30);
    posted(6, 31);
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
    let parts: Vec<String> = started
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
        .expect("a planner call");
    let kept: i64 =
        w.db.lock()
            .unwrap()
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM journal WHERE kind = 'chat.notice'",
                [],
                |r| r.get(0),
            )
            .unwrap();
    (
        parts.into_iter().filter(|p| p.starts_with("[c")).collect(),
        kept,
    )
}

// decision: ORCH-728
#[test]
fn a_run_of_notices_is_one_line_and_the_last_entry_stays_whole() {
    let (entries, kept) = planner_entries(true);
    assert_eq!(kept, 7, "nothing leaves the journal");
    let line = |needle: &str| entries.iter().filter(|e| e.contains(needle)).count();
    assert_eq!(
        line("[c3]–[c7] (Thu 01 Jan 08:10 to Thu 01 Jan 08:14) FERB posted 5 notices here by itself (not shown; \
              chat.history reads them in full)."),
        1,
        "{entries:#?}"
    );
    assert_eq!(
        line("[c10] (Thu 01 Jan 08:30) FERB posted 1 notice here by itself (not shown; chat.history reads it in full)."),
        1,
        "{entries:#?}"
    );
    for n in 0..6 {
        assert_eq!(line(&notice(n)), 0, "notice {n} is folded: {entries:#?}");
    }
    assert_eq!(
        line(&notice(6)),
        1,
        "the last entry before the new message stays whole"
    );
    // the owner's and FERB's words are all there, each once
    for n in 0..2 {
        assert_eq!(line(&format!("INVENTED-MESSAGE-{n}")), 1);
        assert_eq!(line(&format!("INVENTED-REPLY-{n}")), 1);
    }
    assert_eq!(entries.len(), 7, "{entries:#?}");
}

// decision: ORCH-728 (a replay of a Python recording keeps its planner's input)
#[test]
fn a_replay_of_a_python_recording_reads_every_notice() {
    let (entries, _) = planner_entries(false);
    for n in 0..7 {
        assert_eq!(
            entries.iter().filter(|e| e.contains(&notice(n))).count(),
            1,
            "{entries:#?}"
        );
    }
}

fn entry(n: usize, kind: &str) -> Entry {
    Entry {
        at: ferb_clock::py::parse(&format!("2026-01-01T09:{n:02}:00+00:00")).unwrap(),
        text: format!("[c{}] (Thu 01 Jan 09:{n:02}) {kind}", n + 1),
        payload: kind.into(),
        reference: String::new(),
        chat: true,
        origins: Default::default(),
        handle: format!("entry:e{n}"),
        kind: kind.into(),
        refs: Vec::new(),
        opens: kind == "chat.message",
        values: None,
        result: String::new(),
    }
}

// decision: ORCH-728
#[test]
fn every_run_folds_and_the_line_keeps_every_notice_as_its_source() {
    let kinds = [
        "chat.notice",
        "chat.notice",
        "chat.message",
        "chat.reply",
        "chat.notice",
    ];
    let entries: Vec<Entry> = kinds.iter().enumerate().map(|(n, k)| entry(n, k)).collect();
    let folded = fold_notices(entries.clone(), &Zone::utc());
    let shown: Vec<&str> = folded.iter().map(|e| e.text.as_str()).collect();
    assert_eq!(shown.len(), 4, "{shown:#?}");
    assert!(shown[0]
        .starts_with("[c1]–[c2] (Thu 01 Jan 09:00 to Thu 01 Jan 09:01) FERB posted 2 notices"));
    assert_eq!(folded[0].handle, "entry:e0");
    assert_eq!(folded[0].refs, vec!["entry:e1".to_string()]);
    assert_eq!(folded[0].origins.iter().collect::<Vec<_>>(), vec!["ferb"]);
    assert_eq!(
        &folded[1..],
        &entries[2..],
        "the rest as it was, the last notice whole"
    );
    // a notice before the owner's new message only when it is the last entry: a lone notice in the middle folds
    let middle = fold_notices(
        vec![
            entry(0, "chat.message"),
            entry(1, "chat.notice"),
            entry(2, "chat.reply"),
        ],
        &Zone::utc(),
    );
    assert!(middle[1]
        .text
        .contains("FERB posted 1 notice here by itself"));
}
