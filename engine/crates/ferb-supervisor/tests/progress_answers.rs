//! ORCH-640: asked mid-task what a running task has so far, the planner has what it needs to answer from the task's
//! own records: the rule in its instructions, task.evidence among its lookups, and the task's status row with the
//! running worker's latest words (redacted, model-written, the task's origins on the block). Scripted on the recorded
//! scenario where the owner writes while the worker runs (steer_running_task): the worker writes a progress message
//! and is held; the owner asks "what do you have so far?"; the planner answers with a reply, which reaches the chat.

mod common;
mod support_control;

use serde_json::{json, Value};
use support_control::{run_with, world_with};

const ASKED: &str = "what do you have so far?";
const WORDS: &str = "So far: Alpha Corp and Beta Labs. Checking their official pages next.";
const REPLY: &str = "So far it has Alpha Corp and Beta Labs. That isn't checked or final yet.";

fn chat_replies(w: &support_control::World) -> Vec<Value> {
    let db = w.db.lock().unwrap();
    let mut st = db
        .conn()
        .prepare("SELECT data_json FROM journal WHERE kind = 'chat.reply' ORDER BY seq")
        .unwrap();
    let rows = st
        .query_map([], |r| r.get::<_, String>(0))
        .unwrap()
        .map(|r| serde_json::from_str(&r.unwrap()).unwrap())
        .collect();
    rows
}

// decision: ORCH-640, ORCH-663
#[test]
fn a_progress_question_mid_task_reaches_the_planner_with_the_tasks_records() {
    let name = "steer_running_task";
    let mut w = world_with(name, |calls| {
        // the worker says what it has so far, then is held (the recording's gate) while the owner asks
        let worker = calls.iter_mut().find(|c| c["key"] == "worker").unwrap();
        let events = worker["events"].as_array_mut().unwrap();
        events.insert(
            0,
            json!({"kind": "message", "data": {"text": WORDS}, "before_gate": true}),
        );
        // the planner's second turn answers in plain words, no control
        let answer = json!({"kind": "reply", "answers_question": false, "task": null, "control": null,
                            "side_entries": null, "text": REPLY});
        let planner = calls
            .iter_mut()
            .filter(|c| c["key"] == "supervisor")
            .nth(1)
            .unwrap();
        planner["events"] = json!([
            {"kind": "usage", "data": {"input_tokens": 10, "output_tokens": 5, "cost_usd": 0.001}},
            {"kind": "final", "data": {"text": answer.to_string()}}
        ]);
    });
    let report = run_with(&mut w, name, |steps| {
        // the owner's message while the worker runs is the progress question
        for st in steps
            .iter_mut()
            .filter(|st| st["do"] == "stream" && st["conn"] == "app")
        {
            let mut sent: Value =
                serde_json::from_str(st["lines"][0]["send"].as_str().unwrap()).unwrap();
            sent["params"]["text"] = json!(ASKED);
            st["lines"][0]["send"] = json!(sent.to_string());
        }
    });
    eprintln!("{name} (scripted): {}", report.verdict());

    let calls = w.daemon.calls.lock().unwrap();
    // the planner's turn for the question: the third call, made while the worker was held
    let made = &calls.made[2];
    assert_eq!(made["runtime_role"], "orchestrator", "the planner's turn");
    assert!(
        made["tools"]
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t == "task.evidence"),
        "task.evidence is among its lookups: {}",
        made["tools"]
    );
    let started = calls
        .started
        .iter()
        .find(|c| c["n"] == json!(2))
        .expect("the planner's turn started");
    let system = started["system"].as_str().unwrap();
    assert!(
        system.contains(
            // ORCH-663: the rule now points at the task's trace
            "for what a running task has done or found so far, its trace looked up first (task.trace task_id=<its"
        ) && system.contains("say plainly that it isn't checked or final yet"),
        "the progress rule is in the planner's instructions"
    );
    assert!(
        !system.contains("answered only from the task status facts"),
        "the old rule is gone"
    );
    let parts: Vec<String> = started["parts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p.as_str().unwrap().to_string())
        .collect();
    let tasks = parts
        .iter()
        .find(|p| p.contains("Layer 6 — tasks"))
        .expect("the task status block");
    let row = tasks
        .lines()
        .find(|l| l.starts_with("{\"running\""))
        .expect("the status facts");
    let facts: Value = serde_json::from_str(row).unwrap();
    let running = &facts["running"][0];
    assert_eq!(running["state"], "running");
    assert_eq!(
        running["latest_words (the worker's, model-written, not checked)"],
        json!({"at": "09:00", "text": WORDS}),
        "the running worker's latest words, in the row: {running}"
    );
    let task_id = running["task_id"].as_str().unwrap().to_string();
    assert!(
        task_id.starts_with("id-"),
        "the row names the task task.evidence reads: {task_id}"
    );
    assert!(
        parts.iter().any(|p| p.contains(ASKED)),
        "the owner's question is the new message"
    );
    drop(calls);

    // the worker's words go when its run ends (its report is in the records then)
    assert_eq!(
        w.sup.inner().runs.latest_words(&task_id),
        None,
        "the ended run's words are gone"
    );

    // the reply path: the planner's words are the chat's reply to the question
    let replies = chat_replies(&w);
    assert!(
        replies.iter().any(|r| r["text"] == REPLY),
        "the reply reached the chat: {replies:?}"
    );
}
