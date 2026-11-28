//! The owner's check of 2026-11-23 (ORCH-694, ORCH-696, ORCH-697), through the real supervisor on recorded scenarios
//! whose plan, report, check and recorded calls are changed by the test (invented hosts, sentences and pages): the
//! check opens with the recorded calls, each run's newest look at each place in full and the rest one line each
//! (ORCH-719's middle path, ORCH-757), and gets code's own fresh look (ORCH-696); it is never rerun, and
//! a pass with no look is can't tell at once; its token is capped and a run past the cap ends unconfirmed; a not-done
//! report after acting is routed by one call without tools, the 5-attempt check-in kept (ORCH-697). The c252e651
//! regression (a rerun dropped the first check's findings) runs on the Python recordings' own path, where reruns stay.
//! Each test reads what FERB stored and what the scripted models were given.

mod common;
mod support_control;

use std::sync::atomic::Ordering;
use std::sync::Arc;

use serde_json::{json, Value};
use support_control::{run_with, world_with, World};

const CLAIM_SITE: &str = "www.trips.example";
const CLAIMED: &str = "The eligible price-drop refund of around 438 is claimed.";
const SENTENCE: &str = "The site shows it as processed today.";
const CLAIM_ANSWER: &str =
    "Claimed your 433 price-drop refund for booking TRIP-1. The site shows it as processed today.";

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

fn call(db: &ferb_store::Db, id: &str, task: &str, input: Value, output: Value) {
    db.conn()
        .execute(
            "INSERT OR IGNORE INTO spans (id, task_id, kind, started_at, ended_at, input_json, output_json) \
             VALUES (?, ?, 'tool.call', '2026-01-01T09:00:01+00:00', '2026-01-01T09:00:02+00:00', ?, ?)",
            rusqlite::params![
                id,
                task,
                ferb_py::json::dumps(&input),
                ferb_py::json::dumps(&output)
            ],
        )
        .unwrap();
}

/// The step's calls: a long read of the trips page early on (its words never reach the check), then three plain
/// clicks in tab t21 and the worker's own snapshot after them.
fn long_read_then_clicks(db: &ferb_store::Db, task: &str) {
    let at = json!({"site": CLAIM_SITE});
    call(
        db,
        &format!("{task}-0"),
        task,
        json!({"tool": "browser.snapshot", "kind": "read", "args": {"tab": "t21"}, "target": at}),
        json!({"tab": "t21", "text": format!("BIGPAGE {}", "booking row ".repeat(4_000)),
               "truncated": true, "total_text_chars": 120_000}),
    );
    for n in 1..=3 {
        call(
            db,
            &format!("{task}-{n}"),
            task,
            json!({"tool": "browser.act", "kind": "click", "args": {"tab": "t21"}, "target": at}),
            json!({"ok": true, "after": {"read": "page", "changed": true}}),
        );
    }
    call(
        db,
        &format!("{task}-4"),
        task,
        json!({"tool": "browser.snapshot", "kind": "read", "args": {"tab": "t21"}, "target": at}),
        json!({"tab": "t21", "text": "Refund requested."}),
    );
}

/// One submitted condition on the trips site, the worker's answer, and the check's verdict (with findings).
fn planned(calls: &mut [Value], findings: Value) {
    let mut plan = answer(&calls[0]);
    plan["task"]["success_conditions"] = json!([{"text": CLAIMED, "owner_text": CLAIMED, "layer": "outcome",
        "check": {"kind": "submitted", "path": null, "text": null, "url_contains": null, "site": CLAIM_SITE,
                  "expected": null, "tool": null}}]);
    answered(&mut calls[0], plan);
    let w = index(calls, "worker");
    let mut r = answer(&calls[w]);
    r["answer"] = json!(CLAIM_ANSWER);
    answered(&mut calls[w], r);
    let v = index(calls, "worker.verify");
    answered(
        &mut calls[v],
        json!({"results": [{"index": 0, "met": true, "applies": true,
                            "evidence": "The page shows the refund as processed.", "how": null}],
            "next": "accept", "findings": findings, "missing": "", "instructions": "", "worker_type": null,
            "question": "", "reason": "", "gaps": []}),
    );
}

fn lean(name: &str, edit: impl FnOnce(&mut Vec<Value>)) -> World {
    let w = world_with(name, edit);
    w.daemon.lean.store(true, Ordering::SeqCst);
    *w.daemon.literal.lock().unwrap() = Some(Arc::new(long_read_then_clicks));
    w
}

fn page_look(text: &str) -> Option<Result<Value, String>> {
    Some(Ok(
        json!({"content": {"tab": "t21", "text": text}, "is_error": false, "origins": []}),
    ))
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

fn checks_parts(w: &World) -> Vec<String> {
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

/// The model calls made for the check's role, as (mode, tools).
fn checker_calls(w: &World) -> Vec<(String, Value)> {
    w.daemon
        .calls
        .lock()
        .unwrap()
        .made
        .iter()
        .filter(|m| {
            m["runtime_role"]
                .as_str()
                .is_some_and(|r| r.starts_with("checker"))
        })
        .map(|m| {
            (
                m["mode"].as_str().unwrap_or("").to_string(),
                m["tools"].clone(),
            )
        })
        .collect()
}

fn count(w: &World, sql: &str) -> i64 {
    w.db.lock()
        .unwrap()
        .conn()
        .query_row(sql, [], |r| r.get(0))
        .unwrap()
}

fn journal(w: &World, kind: &str) -> Vec<Value> {
    let db = w.db.lock().unwrap();
    let mut st = db
        .conn()
        .prepare("SELECT data_json FROM journal WHERE kind = ? ORDER BY seq")
        .unwrap();
    let rows = st
        .query_map([kind], |r| r.get::<_, String>(0))
        .unwrap()
        .map(|r| serde_json::from_str(&r.unwrap()).unwrap())
        .collect();
    rows
}

/// The check gets code's own look at tab t21, which counts as the check's look, and the recorded calls first: the
/// newest look at the tab in full, the earlier long read of it one line naming its span (ORCH-757); done and
/// verified, one check, its token capped at CHECK_CALLS.
// decision: ORCH-696, ORCH-719, ORCH-757
#[test]
fn the_check_gets_codes_fresh_look_and_every_call() {
    let name = "one_step_task";
    let mut w = lean(name, |c| planned(c, json!([])));
    *w.daemon.code_look.lock().unwrap() = page_look("Refund processed today.");
    let _ = run_with(&mut w, name, |_| {});
    let (row, out) = root_outcome(&w);
    assert_eq!(row, "done", "{out}");
    assert_eq!(out["verified"], true, "{out}");
    let parts = checks_parts(&w);
    assert_eq!(parts.len(), 1, "one check, no rerun");
    let p = &parts[0];
    assert!(
        !p.contains("BIGPAGE"),
        "the look a later one at the same tab replaced is one line"
    );
    assert!(p.contains("1. span_id="), "{p}");
    assert!(
        p.contains("Refund requested."),
        "the look after the last action, in full"
    );
    assert!(
        p.contains("FERB's fresh look at tab t21 on www.trips.example"),
        "{p}"
    );
    assert!(p.contains("Refund processed today."), "{p}");
    assert_eq!(
        *w.daemon.probes.lock().unwrap(),
        vec![("browser.snapshot".to_string(), json!({"tab": "t21"}))]
    );
    assert_eq!(
        count(
            &w,
            "SELECT COUNT(*) FROM spans WHERE kind = 'verify.refused'"
        ),
        0
    );
    assert_eq!(
        count(
            &w,
            "SELECT COUNT(*) FROM spans WHERE kind = 'verify.look' AND error IS NULL"
        ),
        1
    );
    let capped = w.daemon.capped.lock().unwrap().clone();
    assert!(
        capped
            .iter()
            .any(|(_, cap)| *cap == Some(ferb_supervisor::flow::attempt::CHECK_CALLS)),
        "{capped:?}"
    );
    assert_eq!(ferb_supervisor::flow::attempt::CHECK_CALLS, 6);
}

/// No look at the place of the last action (code's failed, the check made none): the pass is can't tell at once,
/// never false, and the check is not run again.
// decision: ORCH-696
#[test]
fn without_any_look_the_pass_is_cant_tell_and_never_rerun() {
    let name = "one_step_task";
    let mut w = lean(name, |c| planned(c, json!([])));
    *w.daemon.code_look.lock().unwrap() = Some(Err("the tab is gone".into()));
    let _ = run_with(&mut w, name, |_| {});
    let (row, out) = root_outcome(&w);
    assert_eq!(row, "done_unverified", "{out}");
    let v = &out["verification"][0];
    assert_eq!(v["met"], Value::Null, "{v}");
    assert!(v["evidence"]
        .as_str()
        .unwrap()
        .starts_with("This check made no look of its own at tab t21 on www.trips.example"));
    assert_eq!(checker_calls(&w).len(), 1, "one check");
    assert_eq!(
        count(
            &w,
            "SELECT COUNT(*) FROM spans WHERE kind = 'verify.refused'"
        ),
        1
    );
    assert_eq!(
        count(
            &w,
            "SELECT COUNT(*) FROM spans WHERE kind = 'verify.look' AND error IS NOT NULL"
        ),
        1
    );
}

/// Code's look failed, the check looked itself: its pass stands.
// decision: ORCH-696
#[test]
fn when_codes_look_fails_the_checks_own_look_counts() {
    let name = "one_step_task";
    let mut w = lean(name, |c| planned(c, json!([])));
    *w.daemon.code_look.lock().unwrap() = Some(Err("the tab is gone".into()));
    *w.daemon.look.lock().unwrap() = Some(Arc::new(|db: &ferb_store::Db, k: usize| {
        let task: String = db
            .conn()
            .query_row(
                "SELECT id FROM tasks WHERE id LIKE '%.verify' ORDER BY rowid DESC LIMIT 1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        call(
            db,
            &format!("look-{k}"),
            &task,
            json!({"tool": "browser.snapshot", "kind": "read", "args": {"tab": "t21"}, "target": {"site": CLAIM_SITE}}),
            json!({"ok": true}),
        );
    }));
    let _ = run_with(&mut w, name, |_| {});
    let (row, out) = root_outcome(&w);
    assert_eq!(row, "done", "{out}");
    assert_eq!(
        count(
            &w,
            "SELECT COUNT(*) FROM spans WHERE kind = 'verify.refused'"
        ),
        0
    );
}

/// A check stopped at its cap (the gateway refused the calls past it and the run's wrong-call cap ended it) gives no
/// verdict: the work the worker reported done stands unconfirmed.
// decision: ORCH-696
#[test]
fn a_check_past_its_cap_ends_unconfirmed() {
    let name = "one_step_task";
    let mut w = lean(name, |c| planned(c, json!([])));
    *w.daemon.code_look.lock().unwrap() = page_look("Refund processed today.");
    w.daemon
        .refused_after_cap
        .store(ferb_worker::errors::WRONG_CALLS_CAP, Ordering::SeqCst);
    let _ = run_with(&mut w, name, |_| {});
    let (row, out) = root_outcome(&w);
    assert_eq!(row, "done_unverified", "{out}");
    assert_eq!(out["verification"][0]["met"], Value::Null, "{out}");
    assert_eq!(checker_calls(&w).len(), 1, "no second check");
}

/// One check, its finding goes to the answer's fix round (the owner's check never reruns).
// decision: ORCH-696
#[test]
fn the_checks_findings_reach_the_answer() {
    let name = "one_step_task";
    let finding = json!([{"quote": SENTENCE, "kind": "unsupported", "evidence": "No page read shows a date."}]);
    let mut w = lean(name, |c| planned(c, finding));
    *w.daemon.code_look.lock().unwrap() = page_look("Refund requested.");
    let _ = run_with(&mut w, name, |_| {});
    let found = journal(&w, "task.answer_findings");
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0]["findings"][0]["quote"], SENTENCE);
}

/// c252e651 (2026-11-21): on the Python recordings' path a check refused for no look is run again; the rerun's verdict
/// used to replace the first, dropping its finding, so the sentence reached the owner unchecked. Now the findings of
/// both runs reach the fix round.
// decision: ORCH-696
#[test]
fn a_reruns_verdict_keeps_the_first_runs_findings() {
    let name = "one_step_task";
    let finding = json!([{"quote": SENTENCE, "kind": "unsupported", "evidence": "No page read shows a date."}]);
    let mut w = world_with(name, |c| {
        planned(c, finding);
        let v = index(c, "worker.verify");
        let mut again = c[v].clone();
        let mut verdict = answer(&again);
        verdict["findings"] = json!([]);
        answered(&mut again, verdict);
        c.insert(v + 1, again);
    });
    *w.daemon.literal.lock().unwrap() = Some(Arc::new(long_read_then_clicks));
    // the second run looks at the tab; the first didn't
    *w.daemon.look.lock().unwrap() = Some(Arc::new(|db: &ferb_store::Db, k: usize| {
        if k < 1 {
            return;
        }
        let task: String = db
            .conn()
            .query_row(
                "SELECT id FROM tasks WHERE id LIKE '%.verify' ORDER BY rowid DESC LIMIT 1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        call(
            db,
            &format!("look-{k}"),
            &task,
            json!({"tool": "browser.snapshot", "kind": "read", "args": {"tab": "t21"}, "target": {"site": CLAIM_SITE}}),
            json!({"ok": true}),
        );
    }));
    let _ = run_with(&mut w, name, |_| {});
    assert_eq!(
        count(
            &w,
            "SELECT COUNT(*) FROM spans WHERE kind = 'verify.refused'"
        ),
        1
    );
    let found = journal(&w, "task.answer_findings");
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0]["findings"][0]["quote"], SENTENCE);
}

/// A not-done report after acting: each is routed by one model call with no tools (mode supervisor, no tool names),
/// never the full check; the 5-attempt check-in comes as before.
// decision: ORCH-697
#[test]
fn a_not_done_report_is_routed_by_one_call_without_tools() {
    let name = "check_in_after_five";
    let mut w = world_with(name, |_| {});
    w.daemon.lean.store(true, Ordering::SeqCst);
    let _ = run_with(&mut w, name, |_| {});
    let calls = checker_calls(&w);
    assert!(!calls.is_empty());
    assert!(
        calls
            .iter()
            .all(|(mode, tools)| mode == "supervisor" && tools == &json!([])),
        "{calls:?}"
    );
    assert_eq!(
        count(
            &w,
            "SELECT COUNT(*) FROM spans WHERE kind = 'verify.routed' AND error IS NULL"
        ),
        calls.len() as i64
    );
    assert!(checks_parts(&w).is_empty(), "no full check ran");
    assert!(
        !journal(&w, "task.check_in").is_empty(),
        "the check-in after five"
    );
}

/// ORCH-654's skip stands with the owner's check: a not-done report that did nothing ends without any checker call.
// decision: ORCH-654
// decision: ORCH-697
#[test]
fn a_not_done_report_that_did_nothing_still_skips_every_checker_call() {
    let name = "check_in_after_five";
    let mut w = world_with(name, |_| {});
    w.daemon.lean.store(true, Ordering::SeqCst);
    w.daemon.checks_every_report.store(false, Ordering::SeqCst);
    let _ = run_with(&mut w, name, |_| {});
    assert!(checker_calls(&w).is_empty());
    assert_eq!(
        count(
            &w,
            "SELECT COUNT(*) FROM spans WHERE kind = 'verify.skipped'"
        ),
        1
    );
    let (row, _) = root_outcome(&w);
    assert_eq!(row, "failed");
}

/// The check's model input (its instructions and context parts).
fn check_inputs(w: &World) -> Vec<Value> {
    w.daemon
        .calls
        .lock()
        .unwrap()
        .started
        .iter()
        .filter(|c| {
            c["task_id"]
                .as_str()
                .is_some_and(|t| t.ends_with(".verify"))
        })
        .cloned()
        .collect()
}

/// The owner's check opens with the recorded calls (before the task, the report and code's look), and the facts
/// that change by the minute leave its instructions for the end of its context, so a later check of the task can
/// reuse the cached prefix; a Python recording's replay keeps its order (lean off).
// decision: ORCH-719, ORCH-757
#[test]
fn the_check_opens_with_the_calls_and_ends_with_the_live_facts() {
    let name = "one_step_task";
    let mut w = lean(name, |c| planned(c, json!([])));
    *w.daemon.code_look.lock().unwrap() = page_look("Refund processed today.");
    let _ = run_with(&mut w, name, |_| {});
    let checks = check_inputs(&w);
    assert_eq!(checks.len(), 1);
    let parts: Vec<&str> = checks[0]["parts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p.as_str().unwrap())
        .collect();
    assert!(
        parts[0].starts_with(ferb_orchestrator::check_evidence::HEAD),
        "{}",
        parts[0]
    );
    let at = |needle: &str| parts.iter().position(|p| p.contains(needle)).unwrap();
    assert!(at("Refund requested.") < at("The worker's answer to the owner"));
    assert!(at("The worker's answer to the owner") < at("FERB's fresh look at tab t21"));
    let live = parts
        .iter()
        .position(|p| p.starts_with(ferb_supervisor::words::LIVE_FACTS))
        .expect("the live facts, in the context");
    assert!(live > at("FERB's fresh look at tab t21"));
    let facts = parts[live]
        .strip_prefix(ferb_supervisor::words::LIVE_FACTS)
        .unwrap()
        .trim();
    assert!(!facts.is_empty());
    let system = checks[0]["system"].as_str().unwrap();
    assert!(!system.contains(facts), "not in the check's instructions");
    // a recording's replay: the calls after the report, the live facts in the instructions
    let mut old = world_with(name, |c| planned(c, json!([])));
    *old.daemon.literal.lock().unwrap() = Some(Arc::new(long_read_then_clicks));
    let _ = run_with(&mut old, name, |_| {});
    let checks = check_inputs(&old);
    let first = checks[0]["parts"][0].as_str().unwrap();
    assert!(!first.starts_with(ferb_orchestrator::check_evidence::HEAD));
    assert!(!checks[0]["parts"].as_array().unwrap().iter().any(|p| p
        .as_str()
        .unwrap()
        .starts_with(ferb_supervisor::words::LIVE_FACTS)));
}

/// ORCH-738: the check accepts with a condition it couldn't confirm and names it in a few words: the answer ends with
/// the fixed opening and that bracket; code keeps it to one line without brackets of its own.
// decision: ORCH-738
#[test]
fn the_unconfirmed_line_names_what_the_check_couldnt_confirm() {
    let name = "one_step_task";
    let mut w = lean(name, |c| {
        planned(c, json!([]));
        let v = index(c, "worker.verify");
        answered(
            &mut c[v],
            json!({"results": [{"index": 0, "met": null, "applies": true,
                                "evidence": "The page shows the request, not its processing.", "how": null}],
                "next": "accept_unconfirmed", "findings": [], "missing": "", "instructions": "", "worker_type": null,
                "question": "", "reason": "The refund's processing isn't shown.", "gaps": [],
                "unconfirmed_short": "whether the\nrefund (TRIP-1) was processed."}),
        );
    });
    *w.daemon.code_look.lock().unwrap() = page_look("Refund requested.");
    let _ = run_with(&mut w, name, |_| {});
    let (row, out) = root_outcome(&w);
    assert_eq!(row, "done_unverified", "{out}");
    assert_eq!(
        out["answer"],
        format!(
            "{CLAIM_ANSWER}\n\nI couldn't confirm part of this (whether the refund TRIP-1 was processed)."
        ),
        "{out}"
    );
}
