//! ORCH-660, ORCH-661 through the real supervisor: the live case of 2026-11-16 (task 0abecb83) replayed on a recorded
//! one-step scenario whose plan, report and check are changed by the test (invented hosts and names). The worker did
//! the work through the browser's window with the Mac tools and the check confirmed it on a fresh page: the task ends
//! done and verified with the worker's answer, its Sent card beside it. With no acting call at all, code's literal
//! check still proves the claim false. Each test reads what FERB stored and what the scripted check was given.

mod common;
mod support_control;

use std::sync::Arc;

use serde_json::{json, Value};
use support_control::{run_with, world_with, World};

const SITE: &str = "study.example.edu";
const SUBMITTED: &str = "Every open assignment is completed and submitted.";
const CONFIRMED: &str = "The course site confirms each submission.";

fn answer(call: &Value) -> Value {
    let events = call["events"].as_array().unwrap();
    let text = events
        .iter()
        .find(|e| e["kind"] == "final")
        .and_then(|e| e["data"]["text"].as_str())
        .unwrap();
    serde_json::from_str(text).unwrap()
}

fn answered(call: &mut Value, to: Value) {
    for e in call["events"].as_array_mut().unwrap() {
        if e["kind"] == "final" {
            e["data"]["text"] = json!(ferb_py::json::dumps(&to));
        }
    }
}

fn index(calls: &[Value], key: &str) -> usize {
    calls.iter().position(|c| c["key"] == key).unwrap()
}

/// The plan's two conditions as the live planner wrote them (the first with a literal submit check on the host it
/// named), and the check's verdict on a fresh page read: both met, accept.
fn the_live_plan(calls: &mut [Value]) {
    let mut plan = answer(&calls[0]);
    plan["task"]["success_conditions"] = json!([
        {"text": SUBMITTED, "owner_text": SUBMITTED, "layer": "outcome",
         "check": {"kind": "submitted", "path": null, "text": null, "url_contains": null, "site": SITE,
                   "expected": null, "tool": null}},
        {"text": CONFIRMED, "owner_text": CONFIRMED, "layer": "outcome", "check": null}]);
    answered(&mut calls[0], plan);
    let v = index(calls, "worker.verify");
    answered(
        &mut calls[v],
        json!({"results": [
                {"index": 0, "met": true, "applies": true,
                 "evidence": "A fresh page read shows the assignment's saved submission, scored 100/100.", "how": null},
                {"index": 1, "met": true, "applies": true,
                 "evidence": "A fresh page read shows \"Submitted on Jan 1, 2026\".", "how": null}],
            "next": "accept", "findings": [], "missing": "", "instructions": "", "worker_type": null,
            "question": "", "reason": "", "gaps": []}),
    );
}

fn call(
    db: &ferb_store::Db,
    id: &str,
    task: &str,
    input: Value,
    output: Value,
    error: Option<&str>,
) {
    db.conn()
        .execute(
            "INSERT OR IGNORE INTO spans (id, task_id, kind, started_at, ended_at, input_json, output_json, error) \
             VALUES (?, ?, 'tool.call', '2026-01-01T09:00:01+00:00', '2026-01-01T09:00:02+00:00', ?, ?, ?)",
            rusqlite::params![
                id,
                task,
                ferb_py::json::dumps(&input),
                ferb_py::json::dumps(&output),
                error
            ],
        )
        .unwrap();
}

/// The live task's acting calls, as stored: input into Chrome's window through the Mac tools under the owner's Yes,
/// a send to the course's grader through Chrome (the second ended with an error, its outcome unknown).
fn through_the_window(db: &ferb_store::Db, task: &str) {
    let ok = json!({"ok": true, "after": {"changed": false}, "observed_change": false});
    call(
        db,
        "live-1",
        task,
        json!({"tool": "mac.snapshot", "kind": "read", "target": {"site": "com.google.Chrome"}}),
        ok.clone(),
        None,
    );
    call(
        db,
        "live-2",
        task,
        json!({"tool": "mac.act", "kind": "mac_input", "target": {"site": "com.google.Chrome"}}),
        ok.clone(),
        None,
    );
    call(
        db,
        "live-3",
        task,
        json!({"tool": "mac.act", "kind": "mac_input", "fact_kinds": ["send"],
        "target": {"site": "com.google.Chrome", "recipients": ["Course grader"], "account": "student@study.example.edu"}}),
        ok.clone(),
        None,
    );
    call(
        db,
        "live-4",
        task,
        json!({"tool": "mac.click", "kind": "mac_input", "fact_kinds": ["send"],
        "target": {"site": "com.google.Chrome", "recipients": ["Course grader"]}}),
        json!({"ok": false, "after": {"changed": true}}),
        Some("the owner used the mouse/keyboard"),
    );
}

fn only_reads(db: &ferb_store::Db, task: &str) {
    call(
        db,
        "read-1",
        task,
        json!({"tool": "browser.snapshot", "kind": "read", "target": {"site": SITE}}),
        json!({"ok": true}),
        None,
    );
}

fn root_outcome(w: &World) -> (String, Value) {
    let db = w.db.lock().unwrap();
    let (status, outcome): (String, String) = db
        .conn()
        .query_row(
            "SELECT status, outcome_json FROM tasks WHERE id = (SELECT tree_id FROM tasks ORDER BY rowid LIMIT 1)",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    (status, serde_json::from_str(&outcome).unwrap())
}

fn checks_parts(w: &World) -> String {
    let calls = w.daemon.calls.lock().unwrap();
    calls
        .started
        .iter()
        .filter(|c| {
            c["task_id"]
                .as_str()
                .is_some_and(|t| t.ends_with(".verify"))
        })
        .map(|c| c["parts"].to_string())
        .collect()
}

/// The live case: code can't tell (the Mac route), the check confirmed both conditions on the page: the task is done
/// and verified, the owner reads the worker's answer, and the Sent card stands beside it.
// decision: ORCH-660, ORCH-661
#[test]
fn work_done_through_the_window_and_confirmed_by_the_check_ends_done() {
    let name = "one_step_task";
    let mut w = world_with(name, |c| the_live_plan(c));
    *w.daemon.literal.lock().unwrap() = Some(Arc::new(through_the_window));
    let _ = run_with(&mut w, name, |_| {});
    let (row, out) = root_outcome(&w);
    assert_eq!(row, "done", "{out}");
    assert_eq!(out["status"], "done", "{out}");
    assert_eq!(out["verified"], true, "{out}");
    assert_eq!(
        out["answer"], "Said hello.",
        "the worker's own answer: {out}"
    );
    let v = &out["verification"];
    assert_eq!(v[0]["met"], true, "{v}");
    assert_eq!(
        v[0]["path"], "verifier",
        "the check's fresh look stands: {v}"
    );
    assert!(out["cards"]
        .as_array()
        .unwrap()
        .iter()
        .any(|c| c["type"] == "send" && c["recipient"] == "Course grader"));
    assert!(!out.to_string().contains("was submitted"), "{out}");
    // the check was given code's finding as a fact, not a verdict
    assert!(
        checks_parts(&w).contains(
            "FERB's records can't settle it: they show no form on study.example.edu submitted"
        ),
        "the check read what code recorded"
    );
}

/// No acting call at all and the worker claims it submitted: code's literal check still proves it false, over the
/// check's accept; the answer says what didn't happen and no card says otherwise.
// decision: ORCH-660, ORCH-609
#[test]
fn a_claim_with_no_acting_call_still_fails_on_code_s_proof() {
    let name = "one_step_task";
    let mut w = world_with(name, |c| the_live_plan(c));
    *w.daemon.literal.lock().unwrap() = Some(Arc::new(only_reads));
    let _ = run_with(&mut w, name, |_| {});
    let (row, out) = root_outcome(&w);
    assert_eq!(row, "failed", "{out}");
    assert_eq!(out["verification"][0]["met"], false, "{out}");
    assert_eq!(out["verification"][0]["path"], "code", "{out}");
    assert!(
        out["answer"].as_str().unwrap().starts_with(&format!(
            "I couldn't finish this task. No form on {SITE} was submitted."
        )),
        "{out}"
    );
    assert!(
        !out["cards"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|c| c["type"] == "send"),
        "{out}"
    );
}
