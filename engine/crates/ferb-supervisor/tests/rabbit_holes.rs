//! The owner's "no rabbit holes" rule (ORCH-679..681) through the real supervisor, on recorded scenarios whose scripted
//! checks are given the labels a check now writes (invented: no recording has them). The check only labels what is
//! left (`left`, `progress`); code turns a retry, or a gapped accept refused because the worker reported not done, into
//! "Keep trying, or go on with what I have?", and never into a failure (ORCH-680).

mod common;
mod support_control;

use serde_json::{json, Value};
use support_control::{run_with, world_with, World};

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

fn count(w: &World, kind: &str) -> usize {
    journal(w).iter().filter(|(k, _)| k == kind).count()
}

fn started(w: &World) -> Vec<Value> {
    w.daemon.calls.lock().unwrap().started.clone()
}

fn workers(w: &World, suffix: &str) -> Vec<Value> {
    started(w)
        .into_iter()
        .filter(|c| c["task_id"].as_str().is_some_and(|t| t.ends_with(suffix)))
        .collect()
}

const FOUND: &str = "Two good places were found with their prices";
const MISSING: &str = "Neither shows a free table for Friday yet.";

/// A check of the table booking (check_in_after_five) as a check now writes it: `next`, each unmet condition's labels.
fn check(next: &str, kind: &str, reached: &str) -> Value {
    json!({"results": [{"index": 0, "met": null, "applies": true,
            "evidence": "Seen in the worker's report and the tool results.", "how": "Not visible from here.",
            "left": {"kind": kind, "reached": reached}}],
        "next": next, "findings": [], "missing": "The table isn't booked.",
        "instructions": "Try another booking site.", "worker_type": null, "question": "", "reason": "",
        "gaps": if next == "accept_with_gaps" { json!([{"index": 0, "reason": MISSING}]) } else { json!([]) },
        "progress": {"found": FOUND, "missing": MISSING}})
}

/// The table booking with every check labelled `kind`/`reached`.
fn labelled(kind: &'static str, reached: &'static str) -> impl FnOnce(&mut Vec<Value>) {
    move |calls: &mut Vec<Value>| {
        for c in calls.iter_mut().filter(|c| c["key"] == "worker.verify") {
            answered(c, check("retry", kind, reached));
        }
    }
}

/// The recorded steps with the check-in's typed answer taken out (the question's choices are tapped by `approve`).
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

/// The question asked, from its journal record.
fn asked(w: &World) -> Vec<String> {
    journal(w)
        .iter()
        .filter(|(k, _)| k == "plan.question")
        .map(|(_, d)| d["question"].as_str().unwrap_or("").to_string())
        .collect()
}

/// ORCH-679: a fix is worth the retry the check chose: no question, the five attempts and the owner's check-in as
/// before (ORCH-307 unchanged).
// decision: ORCH-679, ORCH-307
#[test]
fn a_fix_left_keeps_the_checks_retries_and_the_check_in_after_five() {
    let name = "check_in_after_five";
    let mut w = world_with(name, labelled("fix", "most"));
    let _ = run_with(&mut w, name, |steps| {
        approve(steps, &[("ap-a00000000001", 2)])
    });
    assert_eq!(count(&w, "task.rabbit_hole_asked"), 0);
    assert_eq!(workers(&w, ".worker").len(), 5);
    let q = asked(&w);
    assert_eq!(q.len(), 1, "{q:?}");
    assert!(q[0].contains("I've tried 5 times"), "{q:?}");
}

/// ORCH-679: a search left after the step's first retry: the first retry is still taken, then the owner is asked with
/// the check's own two sentences and FERB's fixed words; "go on with what I have" ends the step delivered with the gap
/// (ORCH-651's ending), never failed (ORCH-680).
// decision: ORCH-679, ORCH-680, ORCH-677
#[test]
fn a_search_after_a_retry_asks_and_go_on_ends_with_the_gap() {
    let name = "check_in_after_five";
    let mut w = world_with(name, labelled("search", "some"));
    let _ = run_with(&mut w, name, |steps| {
        approve(steps, &[("ap-a00000000001", 2)])
    });
    assert_eq!(
        workers(&w, ".worker").len(),
        2,
        "the first retry is still taken"
    );
    let q = asked(&w);
    assert_eq!(q.len(), 1, "{q:?}");
    assert!(
        q[0].contains(FOUND)
            && q[0].contains(MISSING)
            && q[0].ends_with(ferb_orchestrator::rabbit_holes::ASK_TAIL),
        "the check's two sentences, then FERB's fixed words: {q:?}"
    );
    let (row, out) = root_outcome(&w);
    assert_eq!(row, "done_with_gaps", "{out}");
    assert_eq!(out["status"], "done_with_gaps");
    assert!(
        out["answer"]
            .as_str()
            .unwrap()
            .ends_with(&format!("Not done: {MISSING}")),
        "{out}"
    );
    assert!(journal(&w)
        .iter()
        .any(|(k, d)| k == "task.rabbit_hole_answered" && d["answer"] == "go_on"));
}

/// ORCH-679: "keep trying" retries with the check's notes and the owner's answer, and the question isn't asked again in
/// the step: the 5-attempt check-in governs from there.
// decision: ORCH-679, ORCH-678, ORCH-307
#[test]
fn keep_trying_retries_with_the_checks_notes_and_isnt_asked_again() {
    let name = "check_in_after_five";
    let mut w = world_with(name, labelled("guess", "most"));
    let _ = run_with(&mut w, name, |steps| {
        approve(steps, &[("ap-a00000000001", 1), ("ap-a00000000002", 2)])
    });
    assert_eq!(count(&w, "task.rabbit_hole_asked"), 1);
    let q = asked(&w);
    assert_eq!(q.len(), 2, "{q:?}");
    assert!(
        q[1].contains("I've tried 5 times"),
        "then the check-in: {q:?}"
    );
    let runs = workers(&w, ".worker");
    assert_eq!(runs.len(), 5);
    let third = runs[2]["parts"].to_string();
    assert!(
        third.contains("What the check found missing: The table isn't booked.")
            && third.contains("Try another booking site.")
            && third.contains("Keep trying"),
        "{third}"
    );
}

/// ORCH-690 (owner, 2026-11-22: "Yes, always ask me"; a named change to ORCH-679, which kept this the check's
/// retry): an input only the owner has is asked about at once, after the first attempt, and "go on" ends the step with
/// its gap.
// decision: ORCH-679, ORCH-690
#[test]
fn an_owner_label_is_asked_about_at_once() {
    let name = "check_in_after_five";
    let mut w = world_with(name, labelled("owner", "most"));
    let _ = run_with(&mut w, name, |steps| {
        approve(steps, &[("ap-a00000000001", 2)])
    });
    assert_eq!(count(&w, "task.rabbit_hole_asked"), 1);
    assert_eq!(workers(&w, ".worker").len(), 1, "no retry first");
    let (row, out) = root_outcome(&w);
    assert_eq!(row, "done_with_gaps", "{out}");
}

/// ORCH-691 (owner, 2026-11-22: "8 failed actions or 4 min"): an attempt FERB's guard stopped is asked about at
/// once, even when the check labelled what is left a `fix` and chose a retry; "keep trying" gives the step fresh counts
/// (the gateway's keep_trying) and the retry goes on; the guard asks again the next time it stops an attempt.
// decision: ORCH-691, ORCH-679
#[test]
fn an_attempt_the_guard_stopped_is_asked_about_and_keep_trying_counts_afresh() {
    let name = "check_in_after_five";
    let mut w = world_with(name, labelled("fix", "none"));
    *w.daemon.stuck.lock().unwrap() = 2;
    let _ = run_with(&mut w, name, |steps| {
        approve(steps, &[("ap-a00000000001", 1), ("ap-a00000000002", 2)])
    });
    assert_eq!(
        count(&w, "task.rabbit_hole_asked"),
        2,
        "asked each time the guard stopped it"
    );
    assert_eq!(workers(&w, ".worker").len(), 2);
    assert_eq!(
        w.daemon.kept.lock().unwrap().len(),
        1,
        "keep trying: fresh counts"
    );
    let (row, out) = root_outcome(&w);
    assert_eq!(row, "done_with_gaps", "{out}");
}

/// ORCH-691: unattended, the guard's question can't be answered: the step waits on the owner (needs_user).
// decision: ORCH-691, ORCH-681
#[test]
fn unattended_the_guards_question_ends_the_step_waiting_on_the_owner() {
    let name = "check_in_after_five";
    let mut w = world_with(name, labelled("fix", "none"));
    *w.daemon.stuck.lock().unwrap() = 1;
    w.daemon
        .unattended
        .lock()
        .unwrap()
        .insert("id-000015".into());
    let _ = run_with(&mut w, name, |steps| approve(steps, &[]));
    assert_eq!(workers(&w, ".worker").len(), 1);
    let (row, _) = root_outcome(&w);
    assert_eq!(row, "needs_user");
}

/// ORCH-680: a malformed label (a kind no prompt names, a check with no progress) changes nothing: today's path.
// decision: ORCH-680
#[test]
fn a_malformed_label_changes_nothing() {
    let name = "check_in_after_five";
    let mut w = world_with(name, |calls| {
        for c in calls.iter_mut().filter(|c| c["key"] == "worker.verify") {
            let mut v = check("retry", "dig deeper", "most");
            v["results"][0]["left"] = json!("search");
            v["progress"] = json!(7);
            answered(c, v);
        }
    });
    let _ = run_with(&mut w, name, |steps| {
        approve(steps, &[("ap-a00000000001", 2)])
    });
    assert_eq!(count(&w, "task.rabbit_hole_asked"), 0);
    assert_eq!(workers(&w, ".worker").len(), 5);
}

/// ORCH-681: a gapped accept that code refuses only because the worker reported not done asks the owner, where today's
/// code ended the task failed; "go on" delivers with the gap.
// decision: ORCH-681, ORCH-680, ORCH-675
#[test]
fn a_refused_gapped_accept_asks_instead_of_failing() {
    let name = "check_in_after_five";
    let mut w = world_with(name, |calls| {
        for c in calls.iter_mut().filter(|c| c["key"] == "worker.verify") {
            answered(c, check("accept_with_gaps", "fix", "some"));
        }
    });
    let _ = run_with(&mut w, name, |steps| {
        approve(steps, &[("ap-a00000000001", 2)])
    });
    assert_eq!(
        workers(&w, ".worker").len(),
        1,
        "asked at once: no retry needed"
    );
    assert!(journal(&w)
        .iter()
        .any(|(k, d)| k == "task.gaps_refused" && d["why"] == "the worker didn't report done"));
    assert_eq!(count(&w, "task.rabbit_hole_asked"), 1);
    let (row, out) = root_outcome(&w);
    assert_eq!(row, "done_with_gaps", "{out}");
}

/// ORCH-681: in a run nobody attends the question can't be answered: the step ends waiting on the owner (needs_user),
/// as any question does there, never failed.
// decision: ORCH-681, ORCH-680
#[test]
fn unattended_the_question_ends_the_step_waiting_on_the_owner() {
    let name = "check_in_after_five";
    let mut w = world_with(name, labelled("stuck", "some"));
    // the recorded request's id (its ack): nobody attends this run
    w.daemon
        .unattended
        .lock()
        .unwrap()
        .insert("id-000015".into());
    let _ = run_with(&mut w, name, |steps| approve(steps, &[]));
    assert_eq!(workers(&w, ".worker").len(), 2);
    assert!(journal(&w)
        .iter()
        .any(|(k, d)| k == "task.rabbit_hole_answered" && d["answer"] == "unanswered"));
    let (row, out) = root_outcome(&w);
    assert_eq!(row, "needs_user", "{out}");
    assert!(
        out["answer"]
            .as_str()
            .unwrap()
            .contains(ferb_orchestrator::rabbit_holes::ASK_TAIL),
        "{out}"
    );
}

/// ORCH-679: "go on" ends a plan step delivered with its gap, and the step waiting on it runs.
// decision: ORCH-679, ORCH-651
#[test]
fn go_on_lets_the_waiting_steps_run() {
    let name = "multi_step_plan";
    let mut w = world_with(name, |calls| {
        let at = calls
            .iter()
            .position(|c| {
                c["key"]
                    .as_str()
                    .is_some_and(|k| k.ends_with("step.collect"))
            })
            .unwrap();
        let mut report = answer(&calls[at]);
        report["status"] = json!("not_done");
        report["reason"] = json!("One city's office page never loaded.");
        answered(&mut calls[at], report);
        let v = calls
            .iter()
            .position(|c| {
                c["key"]
                    .as_str()
                    .is_some_and(|k| k.ends_with("step.collect.verify"))
            })
            .unwrap();
        answered(&mut calls[v], check("accept_with_gaps", "search", "most"));
    });
    let _ = run_with(&mut w, name, |steps| {
        approve(steps, &[("ap-a00000000001", 2)])
    });
    assert_eq!(count(&w, "task.rabbit_hole_asked"), 1);
    assert!(
        !workers(&w, ".step.write").is_empty(),
        "the step waiting on the gapped one ran"
    );
    let (_, out) = root_outcome(&w);
    assert_ne!(out["status"], "failed", "{out}");
}
