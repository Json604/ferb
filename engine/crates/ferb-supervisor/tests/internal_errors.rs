//! ORCH-707 (owner, 2026-11-23: "Do both"): an attempt that couldn't run because something broke inside FERB (task
//! 047ad9e8: Claude Code's stream couldn't be read, attempts 2, 4 and 5) is tried again without counting as one of the
//! step's five tries (ORCH-307); three such attempts in a row ask the owner, with the reason. Run on the table booking
//! (check_in_after_five) with crashed attempts put in front of its recorded ones (invented: no recording has them).

mod common;
mod support_control;

use serde_json::{json, Value};
use support_control::{run_with, world_with, World};

const CRASH: &str =
    "Failed to decode JSON: JSON message exceeded maximum buffer size of 1048576 bytes...";

/// The recorded calls with `n` crashed worker attempts first, each followed by its check (the recorded check: retry).
fn crashed_first(n: usize) -> impl FnOnce(&mut Vec<Value>) {
    move |calls: &mut Vec<Value>| {
        for c in calls.iter_mut().filter(|c| c["key"] == "worker.verify") {
            relabel(c);
        }
        let check = calls[2].clone();
        let mut crash = calls[1].clone();
        crash["events"] =
            json!([{"kind": "raise", "class": "CLIJSONDecodeError", "message": CRASH}]);
        for _ in 0..n {
            calls.insert(1, check.clone());
            calls.insert(1, crash.clone());
        }
    }
}

/// The check as a check now writes it: a retry with what is left labelled a fix (no question of its own, ORCH-679).
fn relabel(call: &mut Value) {
    let check = json!({"results": [{"index": 0, "met": null, "applies": true,
            "evidence": "Seen in the worker's report and the tool results.", "how": "Not visible from here.",
            "left": {"kind": "fix", "reached": "most"}}],
        "next": "retry", "findings": [], "missing": "The table isn't booked.",
        "instructions": "Try another booking site.", "worker_type": null, "question": "", "reason": "",
        "gaps": [], "progress": {"found": "", "missing": ""}});
    for e in call["events"].as_array_mut().unwrap() {
        if e["kind"] == "final" {
            e["data"]["text"] = json!(ferb_py::json::dumps(&check));
        }
    }
}

/// The recorded typed answers taken out; each question's choice tapped in turn.
fn approve(steps: &mut Vec<Value>, answers: &[(&str, u64)]) {
    steps.retain(|s| {
        !(s["do"] == "call"
            && s["lines"][0]["send"]
                .as_str()
                .is_some_and(|t| t.contains("chat.answer") || t.contains("chat.approve")))
    });
    steps.retain(|s| {
        s["do"] != "wait"
            && !(s["do"] == "call"
                && s["lines"][0]["send"]
                    .as_str()
                    .is_some_and(|t| t.contains("chat.status")))
    });
    let at = steps.iter().position(|s| s["do"] == "stream").unwrap();
    let mut n = 1;
    for (i, (id, choice)) in answers.iter().enumerate() {
        steps.insert(
            at + n,
            json!({"do": "wait", "until": "an approval is pending"}),
        );
        steps.insert(
            at + n + 1,
            json!({"do": "call", "conn": "app", "lines": [{"send": ferb_py::json::dumps(&json!({"jsonrpc": "2.0",
                "id": 90 + i, "method": "chat.approve",
                "params": {"approval_id": id, "granted": true, "choice": choice}}))}]}),
        );
        n += 2;
    }
}

fn journal(w: &World) -> Vec<(String, Value)> {
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

fn asked(w: &World) -> Vec<String> {
    journal(w)
        .iter()
        .filter(|(k, _)| k == "plan.question")
        .map(|(_, d)| d["question"].as_str().unwrap_or("").to_string())
        .collect()
}

fn worker_runs(w: &World) -> usize {
    w.daemon
        .calls
        .lock()
        .unwrap()
        .started
        .iter()
        .filter(|c| {
            c["task_id"]
                .as_str()
                .is_some_and(|t| t.ends_with(".worker"))
        })
        .count()
}

/// Two crashed attempts, then the recorded five: seven runs, and the check-in still comes after five real tries.
// decision: ORCH-707, ORCH-307
#[test]
fn a_crashed_attempt_is_retried_without_counting_as_a_try() {
    let name = "check_in_after_five";
    let mut w = world_with(name, crashed_first(2));
    let _ = run_with(&mut w, name, |steps| {
        approve(steps, &[("ap-a00000000001", 2)])
    });
    assert_eq!(worker_runs(&w), 7, "two crashes, then five tries");
    let q = asked(&w);
    assert_eq!(q.len(), 1, "{q:?}");
    assert!(q[0].contains("I've tried 5 times"), "{q:?}");
    assert!(!journal(&w)
        .iter()
        .any(|(k, _)| k == "task.internal_errors_asked"));
}

/// Three crashed attempts in a row: the owner is asked, with the reason, instead of a fourth by itself; "Keep trying"
/// goes on, the crashes still not counted, and the check-in comes after five real tries.
// decision: ORCH-707, ORCH-679, ORCH-307
#[test]
fn three_crashes_in_a_row_ask_with_the_reason() {
    let name = "check_in_after_five";
    let mut w = world_with(name, crashed_first(3));
    let _ = run_with(&mut w, name, |steps| {
        approve(steps, &[("ap-a00000000001", 1), ("ap-a00000000002", 2)])
    });
    let q = asked(&w);
    assert_eq!(q.len(), 2, "{q:?}");
    assert!(
        q[0].contains(
            "FERB couldn't run this step 3 times in a row because of a problem of its own, not anything you asked. \
             The last time: something broke inside FERB (CLIJSONDecodeError: "
        ) && q[0].contains("CLIJSONDecodeError")
            && q[0].ends_with(ferb_orchestrator::rabbit_holes::ASK_TAIL),
        "{q:?}"
    );
    assert!(q[1].contains("I've tried 5 times"), "{q:?}");
    assert_eq!(worker_runs(&w), 8, "three crashes, then five tries");
    let asked_rows: Vec<Value> = journal(&w)
        .into_iter()
        .filter(|(k, _)| k == "task.internal_errors_asked")
        .map(|(_, d)| d)
        .collect();
    assert_eq!(asked_rows.len(), 1, "{asked_rows:?}");
    assert_eq!(asked_rows[0]["in_a_row"], 3);
}
