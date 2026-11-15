//! A stop ends a task whose plan is parked on a wait (live 2026-11-14, tree 15737cce: the owner's "cancel them all"
//! and six Stop presses left it running for good). Scripted on the recorded `stop_while_step_asks` scenario: the
//! planner's plan is replaced, the owner's task.stop comes while the plan waits, and the task must end, recorded as
//! stopped, with no step left waiting.

mod common;
mod support_control;

use serde_json::{json, Value};
use support_control::{run_with, world_with};

fn step(id: &str, kind: &str, depends_on: &[&str]) -> Value {
    json!({"id": id, "kind": kind, "label": "", "goal": format!("Do {id}"), "depends_on": depends_on,
           "inputs": [], "worker_type": "default", "conditions": [], "until": null, "event": null,
           "event_match": [], "question": "", "options": [], "conversation": null, "repeat": null,
           "earlier_results": []})
}

fn ask(id: &str) -> Value {
    let mut s = step(id, "ask", &[]);
    s["question"] = json!(format!("Which {id}?"));
    s["options"] = json!(["One", "Two"]);
    s
}

fn wait_on(id: &str, event: &str, matched: Value) -> Value {
    let mut s = step(id, "wait", &[]);
    s["event"] = json!(event);
    s["event_match"] = matched;
    s
}

fn wait_until(id: &str, until: &str) -> Value {
    let mut s = step(id, "wait", &[]);
    s["until"] = json!(until);
    s
}

fn answer(steps: Vec<Value>) -> String {
    json!({"kind": "task", "answers_question": false, "task": {
        "goal": "Make the invented sheet once the invented research is done", "name": "Sheet", "identity": "owner",
        "worker_type": "default", "targets": {"apps": [], "sites": [], "paths": [], "recipients": []},
        "success_conditions": [{"text": "The sheet exists", "owner_text": "", "layer": "outcome", "check": null}],
        "plan": {"steps": steps, "input_paths": [], "big_job": false, "time_estimate": "", "cost_estimate": "",
                 "standing_sends": []},
        "folders": [], "keep_tabs_open": false, "parts": [], "earlier_results": []},
        "control": null, "side_entries": null, "text": "On it."})
    .to_string()
}

/// The plan, the owner's stop while it waits (`until`: the harness's condition), and what the store says after.
fn stopped_while(steps: Vec<Value>, until: &str) -> (String, Vec<(String, String)>, Vec<Value>) {
    let name = "stop_while_step_asks";
    let text = answer(steps);
    let mut w = world_with(name, |calls| {
        let planner = calls.iter_mut().find(|c| c["key"] == "supervisor").unwrap();
        planner["events"] = json!([
            {"kind": "usage", "data": {"input_tokens": 10, "output_tokens": 5, "cost_usd": 0.001}},
            {"kind": "final", "data": {"text": text}}
        ]);
    });
    let until = until.to_string();
    let report = run_with(&mut w, name, |steps| {
        for st in steps.iter_mut() {
            if st["do"] == "wait" {
                st["until"] = json!(until);
            }
        }
        // after the stop: the task's end, then the turn's stream
        let at = steps.iter().position(|st| st["do"] == "close").unwrap();
        steps.insert(at + 1, json!({"do": "wait", "until": "the task ended"}));
    });
    eprintln!("scripted: {}", report.verdict());
    let db = w.db.lock().unwrap();
    let root: (String, String) = db
        .conn()
        .query_row(
            "SELECT id, status FROM tasks WHERE parent_id IS NULL ORDER BY rowid DESC LIMIT 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    let mut st = db
        .conn()
        .prepare("SELECT step_id, status FROM plan_steps WHERE task_id = ? ORDER BY rowid")
        .unwrap();
    let plan: Vec<(String, String)> = st
        .query_map([&root.0], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .map(|r| r.unwrap())
        .collect();
    let mut st = db
        .conn()
        .prepare("SELECT data_json FROM journal WHERE kind = 'task.outcome' ORDER BY seq")
        .unwrap();
    let outcomes: Vec<Value> = st
        .query_map([], |r| r.get::<_, String>(0))
        .unwrap()
        .map(|r| serde_json::from_str(&r.unwrap()).unwrap())
        .collect();
    (root.1, plan, outcomes)
}

fn assert_stopped(status: &str, plan: &[(String, String)], outcomes: &[Value]) {
    assert_eq!(
        status, "cancelled",
        "the task ended as stopped by the owner; plan {plan:?}"
    );
    assert_eq!(
        outcomes.len(),
        1,
        "its outcome is recorded once: {outcomes:?}"
    );
    assert_eq!(outcomes[0]["status"], "cancelled");
    assert!(
        plan.iter().all(|(_, s)| s != "waiting" && s != "running"),
        "no step is left waiting or running: {plan:?}"
    );
}

// The live plan's shape: two questions to the owner, a wait on an event, two steps after them; the stop comes while
// the questions are open and the wait is parked.
// decision: ORCH-323
#[test]
fn a_stop_ends_the_live_plan_shape_asks_and_an_event_wait() {
    let (status, plan, outcomes) = stopped_while(
        vec![
            ask("choose_comparison"),
            ask("choose_profile"),
            wait_on(
                "wait_research",
                "timer.fired",
                json!([{"field": "timer_id", "value": "t-none"}]),
            ),
            step(
                "make_sheet",
                "do",
                &["choose_comparison", "choose_profile", "wait_research"],
            ),
            // (the live plan answered two asks in parts; one ask ends in one step here)
            step("make_doc", "do", &["make_sheet"]),
        ],
        "an approval is pending",
    );
    assert_stopped(&status, &plan, &outcomes);
}

// Only the wait is live (the live tree after the owner answered both questions): the stop must reach the parked step.
// decision: ORCH-323
#[test]
fn a_stop_ends_a_plan_parked_only_on_an_event_wait() {
    let (status, plan, outcomes) = stopped_while(
        vec![
            wait_on("wait", "timer.fired", json!([])),
            step("after", "do", &["wait"]),
        ],
        "a plan step waits",
    );
    assert_stopped(&status, &plan, &outcomes);
    assert_eq!(
        plan.iter()
            .find(|(k, _)| k == "wait")
            .map(|(_, s)| s.as_str()),
        Some("cancelled"),
        "{plan:?}"
    );
}

// The same for a wait on a time far ahead.
// decision: ORCH-323
#[test]
fn a_stop_ends_a_plan_parked_on_a_time_wait() {
    let (status, plan, outcomes) = stopped_while(
        vec![
            wait_until("wait", "2030-01-01T09:00:00+00:00"),
            step("after", "do", &["wait"]),
        ],
        "a plan step waits",
    );
    assert_stopped(&status, &plan, &outcomes);
}
