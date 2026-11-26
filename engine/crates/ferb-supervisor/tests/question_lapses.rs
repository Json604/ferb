//! ORCH-729 (front door step 3b; changes ORCH-586's "the question stays open"): FERB's clarifying question lives only
//! until the owner's next message. That message is still read with the question and its request apart, and a message
//! the planner says answers it is still joined to it (`question.answered`); a message it says doesn't answer it clears
//! it (`question.lapsed`, never `question.kept`), so the question never sits above a later message. Only with the
//! bounded planner: a Python recording's replay keeps the question (`question.kept`). Scripted on the recorded
//! plain_reply scenario through the real supervisor with stand-in models (temp store, injected ids and time).

mod common;
mod support_control;

use std::sync::atomic::Ordering;

use ferb_supervisor::words::{OPEN_QUESTION, OPEN_QUESTION_ONCE};
use serde_json::{json, Value};
use support_control::{run_with, world_with, World};

fn planner(kind: &str, answers: bool, text: &str) -> Value {
    let said = json!({"kind": kind, "answers_question": answers, "task": null, "control": null,
                      "side_entries": null, "text": text})
    .to_string();
    json!({"key": "supervisor", "events": [
        {"kind": "usage", "data": {"input_tokens": 10, "output_tokens": 5}},
        {"kind": "final", "data": {"text": said}}]})
}

/// Messages sent one after another on the chat `s1`, each answered by its own planner call; each planner call's parts.
fn turns(w: &mut World, bounded: bool, said: &[&str], calls: Vec<Value>) -> Vec<Vec<String>> {
    w.daemon.bounded.store(bounded, Ordering::SeqCst);
    {
        // every planner call reads the recorded one's environment and facts
        let mut q = w.daemon.supervise.lock().unwrap();
        let last = q.last().cloned().unwrap();
        for _ in 1..said.len() {
            q.push(last.clone());
        }
    }
    let _ = run_with(w, "plain_reply", |steps| {
        steps.retain(|st| st["do"] != "stream" && st["do"] != "stream_end");
        let at = steps.iter().position(|st| st["do"] == "close").unwrap();
        let mut ours = vec![json!({"do": "calls_from_here", "calls": calls})];
        for (n, text) in said.iter().enumerate() {
            let sent = json!({"jsonrpc": "2.0", "id": n + 2, "method": "chat.send",
                              "params": {"session_id": "s1", "text": text}});
            ours.push(
                json!({"do": "stream", "conn": "app", "lines": [{"send": sent.to_string()}]}),
            );
            ours.push(json!({"do": "stream_end", "conn": "app", "lines": []}));
        }
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
        .filter(|p| p.iter().any(|t| t.starts_with("Layer 5 — conversation:")))
        .collect()
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

fn asked(w: &World) -> String {
    w.db.lock()
        .unwrap()
        .conn()
        .query_row(
            "SELECT COALESCE(asked, '') FROM conversations WHERE id = 's1'",
            [],
            |r| r.get(0),
        )
        .unwrap_or_default()
}

fn requests(w: &World) -> Vec<String> {
    journal(w, "chat.message")
        .iter()
        .map(|m| m["request_id"].as_str().unwrap().to_string())
        .collect()
}

fn holds(parts: &[String], heading: &str) -> bool {
    parts.iter().any(|p| p.starts_with(heading))
}

const ASK: &str = "How long should the cables be?";

// decision: ORCH-729
#[test]
fn a_message_that_does_not_answer_the_question_lets_it_lapse() {
    let mut w = world_with("plain_reply", |calls| calls.clear());
    let planners = turns(
        &mut w,
        true,
        &[
            "order me USB cables",
            "what is 17 times 23?",
            "ok no worries",
        ],
        vec![
            planner("clarify", false, ASK),
            planner("reply", false, "391."),
            planner("reply", false, "Okay."),
        ],
    );
    assert_eq!(planners.len(), 3);
    let ids = requests(&w);
    // the next message is read with the question and its request apart
    assert!(
        holds(&planners[1], OPEN_QUESTION_ONCE),
        "{:#?}",
        planners[1]
    );
    // it didn't answer it: the question lapses, nothing is kept or joined
    assert_eq!(
        journal(&w, "question.lapsed"),
        vec![json!({"request_id": ids[0], "by": ids[1]})]
    );
    assert!(journal(&w, "question.kept").is_empty());
    assert!(journal(&w, "question.answered").is_empty());
    assert_eq!(asked(&w), "");
    // and never sits above a later message; the request stays in the chat like any message
    assert!(!holds(&planners[2], OPEN_QUESTION_ONCE) && !holds(&planners[2], OPEN_QUESTION));
    assert!(planners[2]
        .iter()
        .any(|p| p.starts_with("[c") && p.contains("order me USB cables")));
}

// decision: ORCH-729 (the direct answer still joins)
#[test]
fn a_message_that_answers_the_question_is_still_joined_to_its_request() {
    let mut w = world_with("plain_reply", |calls| calls.clear());
    let _ = turns(
        &mut w,
        true,
        &["order me USB cables", "two metres"],
        vec![
            planner("clarify", false, ASK),
            planner("reply", true, "Two metres it is."),
        ],
    );
    let ids = requests(&w);
    let answered = journal(&w, "question.answered");
    assert_eq!(answered.len(), 1, "{answered:?}");
    assert_eq!(answered[0]["request_id"], json!(ids[0]));
    assert_eq!(answered[0]["by"], json!(ids[1]));
    assert!(journal(&w, "question.lapsed").is_empty());
    assert_eq!(asked(&w), "");
}

// decision: ORCH-729 (a replay of a Python recording keeps the question: the lapse applies with the bounded planner)
#[test]
fn a_replay_of_a_python_recording_keeps_the_question() {
    let mut w = world_with("plain_reply", |calls| calls.clear());
    let planners = turns(
        &mut w,
        false,
        &[
            "order me USB cables",
            "what is 17 times 23?",
            "ok no worries",
        ],
        vec![
            planner("clarify", false, ASK),
            planner("reply", false, "391."),
            planner("reply", false, "Okay."),
        ],
    );
    // kept over each later message, as the owner's store showed it 14 times
    assert_eq!(journal(&w, "question.kept").len(), 2);
    assert!(journal(&w, "question.lapsed").is_empty());
    assert_eq!(asked(&w), ASK);
    assert!(holds(&planners[2], OPEN_QUESTION));
}
