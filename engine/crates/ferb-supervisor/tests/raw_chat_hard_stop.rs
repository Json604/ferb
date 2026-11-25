//! ORCH-719 (owner, 2026-11-25): whatever the summary did, the planner never reads more than 40 exchanges or 20k
//! tokens of the conversation word for word; older entries are not shown (still stored, chat.history reads them), and
//! no model call is needed for it. ORCH-724: the cut moves only between blocks anchored after the summary, and what it
//! leaves out folds too. Scripted on the recorded plain_reply scenario through the real supervisor with stand-in models
//! (temp store, injected ids and time; this harness counts a token per UTF-8 byte), the summary call failing.

mod common;
mod support_control;

use ferb_context::tokens::Tokenizer;
use serde_json::{json, Value};
use support_control::{run_with, world_with, World};

fn call(key: &str, text: &str) -> Value {
    json!({"key": key, "events": [
        {"kind": "usage", "data": {"input_tokens": 10, "output_tokens": 5, "cost_usd": 0.001}},
        {"kind": "final", "data": {"text": text}}]})
}

fn failing(key: &str) -> Value {
    json!({"key": key, "events": [
        {"kind": "raise", "class": "RuntimeError", "message": "INVENTED summary service down"}]})
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
    format!("INVENTED-MESSAGE-{n:03} a question")
}

fn answer(n: usize) -> String {
    format!("INVENTED-REPLY-{n:03} an answer")
}

/// The chat `s1` with `count` exchanges (an owner message and FERB's reply each, the reply padded to `pad` bytes),
/// a minute apart, before the scenario's message.
fn seed(w: &World, count: usize, pad: usize) {
    let db = w.db.lock().unwrap();
    for n in 0..count {
        let at = format!("2025-12-31T{:02}:{:02}:00+00:00", 6 + n / 60, n % 60);
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
        let said = format!("{} {}", answer(n), "x".repeat(pad));
        ferb_supervisor::flow::store::journal_row(
            db.conn(),
            &format!("seedreply{n:03}"),
            &at,
            "chat.reply",
            &json!({"session_id": "s1", "request_id": request, "text": said}),
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

/// Two turns on a seeded chat, every summary call failing; the planner calls' parts.
fn two_turns(w: &mut World, calls: Vec<Value>) -> Vec<Vec<String>> {
    w.daemon
        .bounded
        .store(true, std::sync::atomic::Ordering::SeqCst);
    {
        // the second turn's planner reads the recorded one's environment and facts too
        let mut q = w.daemon.supervise.lock().unwrap();
        let last = q.last().cloned().unwrap();
        q.push(last);
    }
    let made = calls.len();
    let _ = run_with(w, "plain_reply", |steps| {
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
    let started = w.daemon.calls.lock().unwrap().started.clone();
    assert!(started.len() <= made, "{started:?}");
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
        .filter(|p| p.iter().any(|t| t.starts_with("Layer 5 — conversation:")))
        .collect()
}

/// The conversation entries a planner call read word for word (one block each, tagged `[cN]`).
fn entries(parts: &[String]) -> Vec<&String> {
    parts.iter().filter(|p| p.starts_with("[c")).collect()
}

fn shown(parts: &[String], n: usize) -> bool {
    entries(parts).iter().any(|p| p.contains(&message(n)))
}

// decision: ORCH-719
// decision: ORCH-724
#[test]
fn a_long_chat_with_a_failing_summary_never_reaches_the_planner_past_40_exchanges() {
    let mut w = world_with("plain_reply", |calls| calls.clear());
    seed(&w, 100, 0);
    let planners = two_turns(
        &mut w,
        vec![
            reply("First reply."),
            failing("summary"),
            reply("Second reply."),
            failing("summary"),
        ],
    );
    assert_eq!(planners.len(), 2);
    assert!(journal(&w, "chat.summary").is_empty());
    assert!(
        !journal(&w, "chat.summary_failed").is_empty(),
        "the fold failed"
    );
    // the first turn: 100 earlier exchanges in blocks of 20; the newest 40 are read, the oldest 60 named by count
    let first = &planners[0];
    for n in 0..100 {
        assert_eq!(shown(first, n), n >= 60, "turn 1, exchange {n}");
    }
    assert!(
        first.iter().any(|p| p.contains(
            "120 earlier entries of this conversation are not shown here (FERB keeps every entry; chat.history \
             reads them in full)."
        )),
        "the header names what is left out"
    );
    // the second: 101 earlier exchanges (the first new one too); the cut moved to the next block: 21 are read
    let second = &planners[1];
    for n in 0..100 {
        assert_eq!(shown(second, n), n >= 80, "turn 2, exchange {n}");
    }
    assert!(entries(second)
        .iter()
        .any(|p| p.contains("the first new message")));
    for parts in &planners {
        let exchanges = entries(parts)
            .iter()
            .filter(|p| p.contains("INVENTED-MESSAGE-") || p.contains("the first new message"))
            .count();
        assert!(exchanges <= 40, "{exchanges} exchanges");
    }
}

// decision: ORCH-719
// decision: ORCH-724
#[test]
fn long_exchanges_never_reach_the_planner_past_20k_tokens() {
    let mut w = world_with("plain_reply", |calls| calls.clear());
    // 30 exchanges of about 1.5k tokens each (45k in all), under 40 exchanges
    seed(&w, 30, 1_500);
    let planners = two_turns(
        &mut w,
        vec![
            reply("First reply."),
            failing("summary"),
            reply("Second reply."),
            failing("summary"),
        ],
    );
    for (turn, parts) in planners.iter().enumerate() {
        let tokens: usize = entries(parts)
            .iter()
            .map(|p| Tokenizer::Utf8Bytes.count(p))
            .sum();
        assert!(
            tokens <= 20_000,
            "turn {turn}: {tokens} tokens word for word"
        );
        assert!(tokens > 10_000, "turn {turn}: the newest blocks that fit");
        assert!(
            shown(parts, 29),
            "turn {turn}: the newest seeded exchange is read"
        );
        assert!(!shown(parts, 0), "turn {turn}: the oldest is not");
    }
}

// decision: ORCH-724
#[test]
fn what_the_hard_stop_leaves_out_folds_and_the_next_planner_reads_the_summary_and_the_rest() {
    let mut w = world_with("plain_reply", |calls| calls.clear());
    // 30 exchanges of about 770 tokens each: what the hard stop leaves out fits one summary call
    seed(&w, 30, 500);
    let summary = "INVENTED-RUNNING-SUMMARY of the long exchanges";
    let planners = two_turns(
        &mut w,
        vec![
            reply("First reply."),
            call("summary", &json!({"summary": summary}).to_string()),
            reply("Second reply."),
        ],
    );
    let folded = journal(&w, "chat.summary");
    assert_eq!(folded.len(), 1, "{folded:?}");
    let covered = folded[0]["covered"].as_array().unwrap().len();
    // the second planner: the summary, then every entry it didn't cover, word for word
    let second = &planners[1];
    assert!(second.iter().any(|p| p.contains(summary)));
    let first_shown = (0..30).find(|&n| shown(second, n)).unwrap();
    assert_eq!(
        first_shown * 2,
        covered,
        "no gap between the summary and what is read"
    );
    for n in first_shown..30 {
        assert!(shown(second, n), "exchange {n}");
    }
}
