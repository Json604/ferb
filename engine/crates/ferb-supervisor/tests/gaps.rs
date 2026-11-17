//! The "delivered, with gaps" ending (ORCH-651..653), the checker skipped on an empty not-done report (ORCH-654) and
//! a check's context order (ORCH-655), each through the real supervisor on a recorded chat scenario whose scripted
//! model answers are changed by the test (invented: no recording has these answers). Each test reads what FERB stored
//! and what the scripted models were given; the recorded comparison itself is not asserted (the answers differ).

mod common;
mod support_control;

use std::sync::atomic::Ordering;

use serde_json::{json, Value};
use support_control::{run_with, world_with, World};

/// The scripted final answer of a recorded model call, as JSON.
fn answer(call: &Value) -> Value {
    let events = call["events"].as_array().unwrap();
    let text = events
        .iter()
        .find(|e| e["kind"] == "final")
        .and_then(|e| e["data"]["text"].as_str())
        .unwrap();
    serde_json::from_str(text).unwrap()
}

/// The call's final answer replaced by `to`.
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

fn journal_kinds(w: &World) -> Vec<(String, Value)> {
    let db = w.db.lock().unwrap();
    let mut st = db
        .conn()
        .prepare("SELECT kind, data_json FROM journal ORDER BY seq")
        .unwrap();
    let rows = st
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
        .unwrap();
    rows.map(|r| {
        let (k, d) = r.unwrap();
        (k, serde_json::from_str(&d).unwrap_or(Value::Null))
    })
    .collect()
}

fn started_keys(w: &World) -> Vec<String> {
    w.daemon
        .calls
        .lock()
        .unwrap()
        .started
        .iter()
        .map(|c| c["task_id"].as_str().unwrap_or("").to_string())
        .collect()
}

const PARTICIPANTS: &str = "Five real participants took part in the test";
const WHY: &str = "Only you can bring real participants; none were invented";

/// The one-step task given a second condition the work can't meet: the worker names it, the check accepts with it.
fn one_step_with_a_gap(calls: &mut [Value], reason: &str) {
    let mut plan = answer(&calls[0]);
    plan["task"]["success_conditions"]
        .as_array_mut()
        .unwrap()
        .push(json!({"text": PARTICIPANTS, "owner_text": PARTICIPANTS, "layer": "outcome", "check": null}));
    answered(&mut calls[0], plan);
    let w = index(calls, "worker");
    let mut report = answer(&calls[w]);
    report["gaps"] =
        json!([{"condition": PARTICIPANTS, "reason": "only the owner has participants"}]);
    answered(&mut calls[w], report);
    let v = index(calls, "worker.verify");
    answered(
        &mut calls[v],
        json!({"results": [
                {"index": 0, "met": true, "applies": true, "evidence": "The note says hello (fs.read).", "how": null},
                {"index": 1, "met": null, "applies": true, "evidence": "No participant data exists.", "how": null}],
            "next": "accept_with_gaps", "findings": [], "missing": "", "instructions": "", "worker_type": null,
            "question": "", "reason": "", "gaps": [{"index": 1, "reason": reason}]}),
    );
}

/// ORCH-651/652: the worker reports a gap, the check accepts with it: the task ends done_with_gaps (its tasks row
/// too), its answer is the worker's then code's "Not done:" line, never verified; the
/// check read the worker's gaps in its report.
// decision: ORCH-651, ORCH-675
#[test]
fn a_gap_the_worker_names_and_the_check_accepts_ends_delivered_with_gaps() {
    let name = "one_step_task";
    let mut w = world_with(name, |calls| one_step_with_a_gap(calls, WHY));
    let _ = run_with(&mut w, name, |_| {});
    let (row, out) = root_outcome(&w);
    assert_eq!(
        row, "done_with_gaps",
        "the tasks row holds the state (migration 0064)"
    );
    assert_eq!(out["status"], "done_with_gaps", "{out}");
    assert_eq!(out["verified"], false);
    assert_eq!(
        out["answer"],
        json!(format!("Said hello.\n\nNot done: {PARTICIPANTS} — {WHY}."))
    );
    assert_eq!(out["gaps"][0]["condition"], PARTICIPANTS);
    assert!(
        out.get("unconfirmed").is_none(),
        "a gap isn't also unconfirmed: {out}"
    );
    let calls = w.daemon.calls.lock().unwrap();
    let check = calls
        .started
        .iter()
        .find(|c| {
            c["task_id"]
                .as_str()
                .is_some_and(|t| t.ends_with(".verify"))
        })
        .unwrap();
    assert!(
        check["parts"]
            .to_string()
            .contains(r#"\"gaps\": [{\"condition\": \"Five real participants"#),
        "the check's report block holds the worker's gaps"
    );
}

/// ORCH-652: a gapped accept outside code's limits (an unmet condition with no reason) reads as the check's plain
/// accept: the work stands unconfirmed, never "with gaps", and the refusal is journaled.
// decision: ORCH-675
#[test]
fn a_gap_without_a_reason_is_refused_and_reads_as_a_plain_accept() {
    let name = "one_step_task";
    let mut w = world_with(name, |calls| one_step_with_a_gap(calls, "  "));
    let _ = run_with(&mut w, name, |_| {});
    let (_, out) = root_outcome(&w);
    assert_eq!(out["status"], "done_unverified", "{out}");
    assert!(!out["answer"].as_str().unwrap().contains("Not done:"));
    assert!(journal_kinds(&w)
        .iter()
        .any(|(k, d)| k == "task.gaps_refused" && d["why"] == "an unmet condition has no reason"));
}

/// ORCH-652, PRM-357: a check that finds the deliverable's evidence invented names it as an unmet condition (a gap
/// saying so) and its sentences as findings: the answer's sentence is fixed by the worker's fix turn, and the owner
/// reads "Not done:" with the check's reason, never the invented claim as done.
// decision: ORCH-675, PRM-357
#[test]
fn invented_evidence_is_named_as_not_done_never_accepted_silently() {
    let name = "checker_findings_fix";
    let mut w = world_with(name, |calls| {
        let mut plan = answer(&calls[0]);
        plan["task"]["success_conditions"]
            .as_array_mut()
            .unwrap()
            .push(
                json!({"text": "The hours come from the library's own page", "owner_text":
                "The hours come from the library's own page", "layer": "outcome", "check": null}),
            );
        answered(&mut calls[0], plan);
        let v = index(calls, "worker.verify");
        answered(
            &mut calls[v],
            json!({"results": [
                    {"index": 0, "met": true, "applies": true, "evidence": "The hours were read.", "how": null},
                    {"index": 1, "met": false, "applies": true, "evidence": "The garden was made up.", "how": null}],
                "next": "accept_with_gaps",
                "findings": [{"quote": "It has a rooftop garden.", "kind": "unsupported",
                              "evidence": "Nothing about a garden was seen."}],
                "missing": "", "instructions": "", "worker_type": null, "question": "", "reason": "",
                "gaps": [{"index": 1, "reason": "the report's garden detail was invented; the library's page wasn't reached"}]}),
        );
    });
    let _ = run_with(&mut w, name, |_| {});
    let (_, out) = root_outcome(&w);
    assert_eq!(out["status"], "done_with_gaps", "{out}");
    let said = out["answer"].as_str().unwrap();
    assert!(
        !said.contains("rooftop garden"),
        "the invented sentence is gone: {said}"
    );
    assert!(
        said.contains("Not done: The hours come from the library's own page — the report's garden detail was invented"),
        "{said}"
    );
    assert!(
        started_keys(&w).iter().any(|k| k.ends_with(".fix")),
        "the fix turn ran"
    );
}

/// ORCH-653: the step after a gapped one declares an outside effect (its condition's check is `submitted`): FERB
/// asks once, naming the gaps, before it starts; a No leaves it not started, and the task ends delivered with gaps,
/// that step told as not done.
// decision: ORCH-653
#[test]
fn a_submit_step_after_a_gapped_one_asks_first_and_a_no_stops_it() {
    let name = "multi_step_plan";
    let official = "Each temperature comes from the city's official weather office";
    let mut w = world_with(name, |calls| {
        let mut plan = answer(&calls[0]);
        plan["task"]["success_conditions"][0]["check"] = json!({"kind": "submitted", "path": null,
            "text": null, "url_contains": null, "site": "forms.example", "expected": null, "tool": null});
        plan["task"]["plan"]["steps"][0]["conditions"] =
            json!(["Three temperatures are collected", official]);
        answered(&mut calls[0], plan);
        let v = index(calls, "step.collect.verify");
        answered(
            &mut calls[v],
            json!({"results": [
                    {"index": 0, "met": true, "applies": true, "evidence": "Three were read.", "how": null},
                    {"index": 1, "met": null, "applies": true, "evidence": "The cities are invented.", "how": null}],
                "next": "accept_with_gaps", "findings": [], "missing": "", "instructions": "", "worker_type": null,
                "question": "", "reason": "", "gaps": [{"index": 1, "reason": "the cities are invented, so no office exists"}]}),
        );
    });
    let _ = run_with(&mut w, name, |steps| {
        let at = steps.iter().position(|s| s["do"] == "stream").unwrap();
        steps.insert(
            at + 1,
            json!({"do": "wait", "until": "an approval is pending"}),
        );
        steps.insert(
            at + 2,
            json!({"do": "call", "conn": "app", "lines": [{"send": "{\"jsonrpc\": \"2.0\", \"id\": 90, \"method\": \"chat.approve\", \"params\": {\"approval_id\": \"ap-a00000000001\", \"granted\": false}}"}]}),
        );
    });
    let kinds = journal_kinds(&w);
    let asked: Vec<&Value> = kinds
        .iter()
        .filter(|(k, _)| k == "plan.question")
        .map(|(_, d)| d)
        .collect();
    assert_eq!(asked.len(), 1, "{kinds:?}");
    let question = asked[0].to_string();
    assert!(
        question.contains("finished with these parts not done")
            && question.contains(official)
            && question.contains("the cities are invented, so no office exists")
            && question.contains("Go on to"),
        "{question}"
    );
    assert!(kinds
        .iter()
        .any(|(k, d)| k == "plan.after_gaps" && d["go_on"] == false));
    assert!(
        !started_keys(&w).iter().any(|k| k.ends_with(".step.write")),
        "the submit step never started"
    );
    let (_, out) = root_outcome(&w);
    assert_eq!(out["status"], "done_with_gaps", "{out}");
    let said = out["answer"].as_str().unwrap();
    assert!(
        said.starts_with("Avalon 21C, Brindle 18C, Corvo 25C."),
        "{said}"
    );
    assert!(
        said.contains(&format!("Not done: {official} — the cities are invented")),
        "{said}"
    );
    assert!(
        said.contains("— not started: you said not to go on after the parts not done."),
        "{said}"
    );
}

/// ORCH-654: a not-done report from an attempt with no recorded call and no changed file ends as reported, without a
/// checker run: a verify.skipped span says why, and the task fails with the worker's reason (ORCH-568's reply).
// decision: ORCH-654
#[test]
fn a_not_done_report_that_did_nothing_ends_without_the_checker() {
    let name = "check_in_after_five";
    let mut w = world_with(name, |_| {});
    w.daemon.checks_every_report.store(false, Ordering::SeqCst);
    let _ = run_with(&mut w, name, |_| {});
    let keys = started_keys(&w);
    assert!(!keys.iter().any(|k| k.ends_with(".verify")), "{keys:?}");
    assert_eq!(keys.iter().filter(|k| k.ends_with(".worker")).count(), 1);
    let skipped: i64 = w
        .db
        .lock()
        .unwrap()
        .conn()
        .query_row(
            "SELECT COUNT(*) FROM spans WHERE kind = 'verify.skipped' AND input_json LIKE '%no acting call%'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(skipped, 1);
    let (row, out) = root_outcome(&w);
    assert_eq!(row, "failed");
    // ORCH-568's reply for a not-done report with its reason and no check of its own words
    assert_eq!(
        out["answer"],
        "I couldn't finish this task.\n\nThe site errored."
    );
}

/// ORCH-654: a not-done report that asks the owner a question still goes to the checker (it may ask it).
// decision: ORCH-654
#[test]
fn a_not_done_report_with_a_question_still_gets_its_check() {
    let name = "check_in_after_five";
    let mut w = world_with(name, |calls| {
        let at = index(calls, "worker");
        let mut report = answer(&calls[at]);
        report["question"] = json!("Which site should I use?");
        answered(&mut calls[at], report);
    });
    w.daemon.checks_every_report.store(false, Ordering::SeqCst);
    let _ = run_with(&mut w, name, |_| {});
    assert!(started_keys(&w).iter().any(|k| k.ends_with(".verify")));
}

/// ORCH-655: a check's previous-attempts block comes after the task's other blocks, just before FERB's code facts.
// decision: ORCH-655
#[test]
fn a_checks_previous_attempts_come_after_the_steady_blocks() {
    let name = "check_in_after_five";
    let mut w = world_with(name, |_| {});
    let _ = run_with(&mut w, name, |_| {});
    let calls = w.daemon.calls.lock().unwrap();
    let second_check = calls
        .started
        .iter()
        .filter(|c| {
            c["task_id"]
                .as_str()
                .is_some_and(|t| t.ends_with(".verify"))
        })
        .nth(1)
        .unwrap();
    let parts: Vec<String> = second_check["parts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p.as_str().unwrap_or("").to_string())
        .collect();
    let at = |prefix: &str| parts.iter().position(|p| p.starts_with(prefix)).unwrap();
    let attempts = at("Previous attempts (data):\n");
    assert_eq!(
        at("FERB's code facts about how this work was done"),
        attempts + 1
    );
    assert!(at("Success conditions:") < attempts);
    assert!(at("FERB's workspace") < attempts);
}
