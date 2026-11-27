//! ORCH-303, ORCH-304, ORCH-718: the planner reads the running summary plus the last 20 exchanges, and the older conversation
//! folds about every 20-40 exchanges (live 2026-11-25: the owner's chat held 94 short exchanges since 2026-11-17, about
//! 24k tokens; the tail beyond the last 20 never reached FOLD_CHUNK_TOKENS, so no summary was ever made and the planner
//! reread the whole chat on every message). Scripted on the recorded plain_reply scenario through the real supervisor
//! with stand-in models (temp store, injected ids and time): a chat of short exchanges past 20 beyond the last 20 makes
//! one summary call after the planner's decision, and the next planner turn reads the summary and only the last 20
//! exchanges.

mod common;
mod support_control;

use serde_json::{json, Value};
use support_control::{run_with, world_with, World};

const SEEDED: usize = 45;
const SUMMARY: &str = "INVENTED-RUNNING-SUMMARY of the older exchanges";

fn call(key: &str, text: &str) -> Value {
    json!({"key": key, "events": [
        {"kind": "usage", "data": {"input_tokens": 10, "output_tokens": 5, "cost_usd": 0.001}},
        {"kind": "final", "data": {"text": text}}]})
}

fn reply(text: &str) -> Value {
    call(
        "supervisor",
        &json!({"kind": "reply", "answers_question": false, "task": null, "control": null,
                "side_entries": null, "text": text})
        .to_string(),
    )
}

fn message(n: usize) -> String {
    format!("INVENTED-MESSAGE-{n:02} a short question")
}

fn answer(n: usize) -> String {
    format!("INVENTED-REPLY-{n:02} a short answer")
}

/// The chat `s1` with `SEEDED` short exchanges (an owner message and FERB's reply each), a minute apart, before the
/// scenario's message.
fn seed(w: &World) {
    let db = w.db.lock().unwrap();
    for n in 0..SEEDED {
        let at = format!("2025-12-31T{:02}:{:02}:00+00:00", 8 + n / 60, n % 60);
        let request = format!("seedrequest{n:03}");
        ferb_supervisor::flow::store::journal_row(
            db.conn(),
            &format!("seedmessage{n:03}"),
            &at,
            "chat.message",
            &json!({"session_id": "s1", "request_id": request, "text": message(n)}),
            Some("owner"),
        )
        .unwrap();
        ferb_supervisor::flow::store::journal_row(
            db.conn(),
            &format!("seedreply{n:03}"),
            &at,
            "chat.reply",
            &json!({"session_id": "s1", "request_id": request, "text": answer(n)}),
            Some("ferb"),
        )
        .unwrap();
    }
}

fn send(id: u64, text: &str) -> [Value; 2] {
    let sent = json!({"jsonrpc": "2.0", "id": id, "method": "chat.send",
                      "params": {"session_id": "s1", "text": text}});
    [
        json!({"do": "stream", "conn": "app", "lines": [{"send": sent.to_string()}]}),
        json!({"do": "stream_end", "conn": "app", "lines": []}),
    ]
}

fn journal(w: &World, kind: &str) -> Vec<Value> {
    let db = w.db.lock().unwrap();
    let mut st = db
        .conn()
        .prepare("SELECT data_json FROM journal WHERE kind = ? ORDER BY seq")
        .unwrap();
    st.query_map([kind], |r| r.get::<_, String>(0))
        .unwrap()
        .map(|d| serde_json::from_str(&d.unwrap()).unwrap())
        .collect()
}

// decision: ORCH-303
// decision: ORCH-718 (the fold as a Python recording's replay keeps it; ORCH-745 ended it for the front door)
#[test]
fn a_chat_of_short_exchanges_folds_and_the_next_planner_reads_the_last_20() {
    let name = "plain_reply";
    let mut w = world_with(name, |calls| calls.clear());
    seed(&w);
    {
        // the second turn's planner reads the recorded one's environment and facts too
        let mut q = w.daemon.supervise.lock().unwrap();
        let last = q.last().cloned().unwrap();
        q.push(last);
    }
    let calls = vec![
        reply("First reply."),
        call("summary", &json!({"summary": SUMMARY}).to_string()),
        reply("Second reply."),
    ];
    let report = run_with(&mut w, name, |steps| {
        steps.retain(|st| st["do"] != "stream" && st["do"] != "stream_end");
        let at = steps.iter().position(|st| st["do"] == "close").unwrap();
        let mut ours = vec![json!({"do": "calls_from_here", "calls": calls})];
        ours.extend(send(2, "the first new message"));
        ours.push(json!({"do": "wait", "until": "model calls made: 2"}));
        ours.push(json!({"do": "plan_event"}));
        ours.extend(send(3, "the second new message"));
        for (i, st) in ours.into_iter().enumerate() {
            steps.insert(at + i, st);
        }
    });
    eprintln!("scripted: {}", report.verdict());

    // the first turn: the planner, then one summary call in the background
    let made = w.daemon.calls.lock().unwrap().made.clone();
    assert_eq!(made.len(), 3, "planner, summary, planner: {made:?}");
    let summaries = journal(&w, "chat.summary");
    assert_eq!(summaries.len(), 1, "one summary folded: {summaries:?}");
    assert!(journal(&w, "chat.summary_failed").is_empty());
    let summary = &summaries[0];
    assert_eq!(summary["session_id"], "s1");
    assert_eq!(summary["text"], SUMMARY);
    // what it folded: every entry beyond the last 20 exchanges at the first turn (the seeded 45 and the first new
    // message make 46: the oldest 26, message and reply each)
    let covered: Vec<String> = summary["covered"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect();
    let folded: Vec<String> = (0..SEEDED - 19)
        .flat_map(|n| {
            [
                format!("entry:seedmessage{n:03}"),
                format!("entry:seedreply{n:03}"),
            ]
        })
        .collect();
    assert_eq!(covered, folded);

    // the second turn's planner: the summary and the last 20 exchanges (19 seeded and the first new one), nothing older
    let started = w.daemon.calls.lock().unwrap().started.clone();
    let planner = started
        .iter()
        .find(|c| c["n"] == 2)
        .expect("the second planner call");
    let parts = planner["parts"].to_string();
    assert!(parts.contains(SUMMARY), "the summary is read");
    for n in 0..SEEDED {
        let shown = parts.contains(&message(n));
        assert_eq!(shown, n >= SEEDED - 19, "message {n} shown: {shown}");
        assert_eq!(parts.contains(&answer(n)), n >= SEEDED - 19, "reply {n}");
    }
    assert!(parts.contains("the first new message"));
    assert!(parts.contains("First reply."));
    // the summary call read the folded entries and the bound of what it covers, nothing of the recent ones
    let summary_call = started
        .iter()
        .find(|c| c["n"] == 1)
        .expect("the summary call");
    let read = summary_call["parts"].to_string();
    assert!(read.contains(&message(0)) && read.contains(&answer(SEEDED - 20)));
    assert!(!read.contains(&message(SEEDED - 19)));
}

// decision: ORCH-718
#[test]
fn twenty_exchanges_beyond_the_last_20_are_needed_before_a_short_chat_folds() {
    let name = "plain_reply";
    let mut w = world_with(name, |calls| calls.clear());
    {
        // 38 seeded and the new message: 19 beyond the last 20, under every token figure
        let db = w.db.lock().unwrap();
        for n in 0..38 {
            let at = format!("2025-12-31T08:{n:02}:00+00:00");
            ferb_supervisor::flow::store::journal_row(
                db.conn(),
                &format!("seedmessage{n:03}"),
                &at,
                "chat.message",
                &json!({"session_id": "s1", "request_id": format!("seedrequest{n:03}"), "text": message(n)}),
                Some("owner"),
            )
            .unwrap();
        }
    }
    let calls = vec![reply("First reply.")];
    let _ = run_with(&mut w, name, |steps| {
        steps.retain(|st| st["do"] != "stream" && st["do"] != "stream_end");
        let at = steps.iter().position(|st| st["do"] == "close").unwrap();
        let mut ours = vec![json!({"do": "calls_from_here", "calls": calls})];
        ours.extend(send(2, "the new message"));
        ours.push(json!({"do": "plan_event"}));
        for (i, st) in ours.into_iter().enumerate() {
            steps.insert(at + i, st);
        }
    });
    let made = w.daemon.calls.lock().unwrap().made.clone();
    assert_eq!(made.len(), 1, "the planner only: {made:?}");
    assert!(journal(&w, "chat.summary").is_empty());
}
