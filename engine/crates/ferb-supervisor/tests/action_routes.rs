//! ORCH-664..ORCH-673 through the real supervisor, on a recorded one-step scenario whose plan, report and check are
//! changed by the test (invented hosts, names and files): who decides a condition when code's facts and the checker
//! meet (the Mac's state, "nothing done", a receipt showing another file, the checker's fresh look), what the owner is
//! told, and what is recorded. The live cases they come from: task 0abecb83 (2026-11-16, work through a window),
//! d05e8354 (2026-11-17, a script button pressed with plain clicks), the owner's Drive example (2026-11-17). Each test
//! reads what FERB stored and what the scripted models were given.

mod common;
mod support_control;

use std::sync::Arc;

use serde_json::{json, Value};
use support_control::{run_with, world_with, World};

const CLAIM_SITE: &str = "www.trips.example";
const CLAIMED: &str = "The eligible price-drop refund of around 438 is claimed.";
const CLAIM_ANSWER: &str =
    "Claimed your 433 price-drop refund for booking TRIP-1. The site shows it as processed today.";

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

fn spans_of(w: &World, kind: &str) -> Vec<Value> {
    let db = w.db.lock().unwrap();
    let mut st = db
        .conn()
        .prepare("SELECT input_json FROM spans WHERE kind = ? ORDER BY rowid")
        .unwrap();
    let rows = st
        .query_map([kind], |r| r.get::<_, String>(0))
        .unwrap()
        .map(|r| serde_json::from_str(&r.unwrap()).unwrap())
        .collect();
    rows
}

/// One condition with a check, the worker's answer, and the check's verdict on it (its next, its met).
fn planned(calls: &mut [Value], check: Value, text: &str, report: &str, met: Value, next: &str) {
    let mut plan = answer(&calls[0]);
    let mut full = json!({"kind": null, "path": null, "text": null, "url_contains": null, "site": null,
                          "expected": null, "tool": null});
    for (k, v) in check.as_object().unwrap() {
        full[k] = v.clone();
    }
    plan["task"]["success_conditions"] =
        json!([{"text": text, "owner_text": text, "layer": "outcome", "check": full}]);
    answered(&mut calls[0], plan);
    let w = index(calls, "worker");
    let mut r = answer(&calls[w]);
    r["answer"] = json!(report);
    answered(&mut calls[w], r);
    let v = index(calls, "worker.verify");
    answered(
        &mut calls[v],
        json!({"results": [{"index": 0, "met": met, "applies": true,
                            "evidence": "A fresh page read of the site.", "how": null}],
            "next": next, "findings": [], "missing": "", "instructions": "", "worker_type": null,
            "question": "", "reason": "", "gaps": []}),
    );
}

/// The live Goibibo task's calls on the claim page, shaped as stored (invented host): reads, then three plain clicks
/// in tab t21 (one loaded a new document, one couldn't be read after it), and the worker's own snapshot.
fn plain_clicks(db: &ferb_store::Db, task: &str) {
    let id = |n: &str| format!("{task}-{n}");
    let click = |changed: Value, note: &str| json!({"ok": true, "after": {"read": "page", "changed": changed, "note": note}});
    let at = json!({"site": CLAIM_SITE, "page": "https://www.trips.example/mytrips"});
    call(
        db,
        &id("1"),
        task,
        json!({"tool": "browser.snapshot", "kind": "read", "args": {"tab": "t21"},
        "target": {"site": CLAIM_SITE}}),
        json!({"ok": true}),
        None,
    );
    call(
        db,
        &id("2"),
        task,
        json!({"tool": "browser.act", "kind": "click", "args": {"tab": "t21"}, "target": at}),
        click(json!(true), "the page loaded a new document"),
        None,
    );
    call(
        db,
        &id("3"),
        task,
        json!({"tool": "browser.act", "kind": "click", "args": {"tab": "t21"}, "target": at}),
        click(Value::Null, "nothing changed within 0.5 s"),
        None,
    );
    call(
        db,
        &id("4"),
        task,
        json!({"tool": "browser.act", "kind": "click", "args": {"tab": "t21"}, "target": at}),
        click(
            Value::Null,
            "unknown: the page couldn't be read after the act",
        ),
        None,
    );
    call(
        db,
        &id("5"),
        task,
        json!({"tool": "browser.snapshot", "kind": "read", "args": {"tab": "t21"},
        "target": {"site": CLAIM_SITE}}),
        json!({"ok": true}),
        None,
    );
}

/// The check's own look at the tab of the step's last action, recorded as its runtime starts (for every check run, or
/// only from run `from`).
fn looks_at(w: &World, tab: &str, site: &str, from: usize) {
    let (tab, site) = (tab.to_string(), site.to_string());
    *w.daemon.look.lock().unwrap() = Some(Arc::new(move |db: &ferb_store::Db, k: usize| {
        if k < from {
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
            json!({"tool": "browser.snapshot", "kind": "read", "args": {"tab": tab}, "target": {"site": site}}),
            json!({"ok": true}),
            None,
        );
    }));
}

/// The live case of 2026-11-16: code has no verdict for the window route; the check looked at Chrome after the last
/// action there and confirmed both conditions: the task is done and verified, the owner reads the worker's answer,
/// the Sent card stands beside it.
// decision: ORCH-664, ORCH-666, ORCH-694
#[test]
fn work_done_through_the_window_and_confirmed_by_the_check_ends_done() {
    let name = "one_step_task";
    let mut w = world_with(name, |c| the_live_plan(c));
    *w.daemon.literal.lock().unwrap() = Some(Arc::new(through_the_window));
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
            json!({"tool": "mac.snapshot", "kind": "read", "target": {"site": "com.google.Chrome"}}),
            json!({"ok": true}),
            None,
        );
    }));
    let _ = run_with(&mut w, name, |_| {});
    let (row, out) = root_outcome(&w);
    assert_eq!(row, "done", "{out}");
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
    assert_eq!(v[0]["by"], "checker", "{v}");
    assert_eq!(
        v[0]["code"]["by"], "receipt",
        "code's fact kept beside it: {v}"
    );
    assert!(out["cards"]
        .as_array()
        .unwrap()
        .iter()
        .any(|c| c["type"] == "send" && c["recipient"] == "Course grader"));
    assert!(
        checks_parts(&w).contains(&format!(
            "no action of FERB's own on {SITE} is recorded in this step"
        )),
        "the check read what code recorded, as a fact"
    );
}

/// The Goibibo case rebuilt from its trace (d05e8354): a script button pressed with plain clicks, the check's fresh
/// look at the same tab says the refund was processed. Code has no proof either way, so the page decides: the task
/// ends done, verified, with the worker's answer shown, and no line about forms.
// decision: ORCH-664, ORCH-665, ORCH-694, ORCH-672, ORCH-309
#[test]
fn a_script_button_confirmed_on_the_page_ends_done_with_the_answer() {
    let name = "one_step_task";
    let mut w = world_with(name, |c| {
        planned(
            c,
            json!({"kind": "submitted", "site": CLAIM_SITE}),
            CLAIMED,
            CLAIM_ANSWER,
            json!(true),
            "accept",
        )
    });
    *w.daemon.literal.lock().unwrap() = Some(Arc::new(plain_clicks));
    looks_at(&w, "t21", CLAIM_SITE, 0);
    let _ = run_with(&mut w, name, |_| {});
    let (row, out) = root_outcome(&w);
    assert_eq!(row, "done", "{out}");
    assert_eq!(out["verified"], true, "{out}");
    assert_eq!(out["answer"], CLAIM_ANSWER, "{out}");
    let v = &out["verification"][0];
    assert_eq!(
        (v["met"].clone(), v["verdict"].clone(), v["by"].clone()),
        (json!(true), json!("met"), json!("checker")),
        "{v}"
    );
    assert!(
        v["code"]["basis"]
            .as_str()
            .unwrap()
            .contains("FERB acted 3 time(s)"),
        "{v}"
    );
    assert!(!out.to_string().contains("was submitted"), "{out}");
    assert!(journal(&w, "task.verdict_disagreement").is_empty());
    assert!(
        spans_of(&w, "verify.refused").is_empty(),
        "the check had looked"
    );
}

/// ORCH-671's rule as the Python recordings' path still runs it (ORCH-694's named difference; the owner's check, with
/// no rerun, is tests/lean_check.rs): the same task, but the check never looks at the tab after the last click: FERB
/// refuses its pass once with the reason, and when the second check doesn't look either, the condition is can't tell, never false: done but
/// unconfirmed, with ORCH-607's line and the worker's answer.
// decision: ORCH-694, ORCH-607, PRM-360
#[test]
fn a_pass_without_a_fresh_look_is_cant_tell_not_false() {
    let name = "one_step_task";
    let mut w = world_with(name, |c| {
        planned(
            c,
            json!({"kind": "submitted", "site": CLAIM_SITE}),
            CLAIMED,
            CLAIM_ANSWER,
            json!(true),
            "accept",
        );
        let v = index(c, "worker.verify");
        let again = c[v].clone();
        c.insert(v + 1, again);
    });
    *w.daemon.literal.lock().unwrap() = Some(Arc::new(plain_clicks));
    let _ = run_with(&mut w, name, |_| {});
    let (row, out) = root_outcome(&w);
    assert_eq!(row, "done_unverified", "{out}");
    assert_eq!(out["verification"][0]["met"], Value::Null, "{out}");
    assert!(out["verification"][0]["evidence"]
        .as_str()
        .unwrap()
        .starts_with("This check made no look of its own at tab t21 on www.trips.example"));
    assert_eq!(
        out["answer"],
        format!("{CLAIM_ANSWER}\n\nI couldn't confirm part of this."),
        "{out}"
    );
    assert_eq!(spans_of(&w, "verify.refused").len(), 2);
    assert!(checks_parts(&w)
        .contains("FERB refused this check's pass: the step's last action was on tab t21"));
}

/// ORCH-671's rule on the Python recordings' path (ORCH-694's named difference): refused once, the second check looks
/// at the tab: its pass stands.
// decision: ORCH-694
#[test]
fn a_check_that_looks_after_the_refusal_passes() {
    let name = "one_step_task";
    let mut w = world_with(name, |c| {
        planned(
            c,
            json!({"kind": "submitted", "site": CLAIM_SITE}),
            CLAIMED,
            CLAIM_ANSWER,
            json!(true),
            "accept",
        );
        let v = index(c, "worker.verify");
        let again = c[v].clone();
        c.insert(v + 1, again);
    });
    *w.daemon.literal.lock().unwrap() = Some(Arc::new(plain_clicks));
    looks_at(&w, "t21", CLAIM_SITE, 1);
    let _ = run_with(&mut w, name, |_| {});
    let (row, out) = root_outcome(&w);
    assert_eq!(row, "done", "{out}");
    assert_eq!(spans_of(&w, "verify.refused").len(), 1);
}

/// The owner's Drive example: the request names report.pdf on the Mac; the worker picked a file in the site's own
/// picker (plain clicks, no upload, nothing that can carry a file from the Mac) and submitted; the check sees a
/// report.pdf on the form. That is another file: never done and never a bare failure, but delivered with code's line
/// saying what happened instead (D3's default), the disagreement journaled. The worker was told the fact first.
// decision: ORCH-667, ORCH-668, ORCH-669, ORCH-672, ORCH-673, PRM-360
#[test]
fn a_same_named_file_from_the_site_s_picker_is_different_never_done() {
    let dir = tempfile::tempdir().unwrap();
    let report = dir.path().join("Desktop/report.pdf");
    std::fs::create_dir_all(report.parent().unwrap()).unwrap();
    std::fs::write(&report, b"the desktop copy").unwrap();
    let report = report.to_string_lossy().into_owned();
    let attached = "report.pdf from your Desktop is attached to the form";
    let name = "one_step_task";
    let check = json!({"kind": "uploaded", "path": report});
    let mut w = world_with(name, |c| {
        planned(
            c,
            check,
            attached,
            "Attached report.pdf and submitted the form.",
            json!(true),
            "accept",
        )
    });
    *w.daemon.literal.lock().unwrap() = Some(Arc::new(|db: &ferb_store::Db, task: &str| {
        let at = json!({"site": SITE});
        for n in ["1", "2", "3"] {
            call(
                db,
                &format!("{task}-{n}"),
                task,
                json!({"tool": "browser.act", "kind": "click", "args": {"tab": "t1"}, "target": at}),
                json!({"ok": true, "after": {"changed": true}}),
                None,
            );
        }
    }));
    looks_at(&w, "t1", SITE, 0);
    let _ = run_with(&mut w, name, |_| {});
    let (row, out) = root_outcome(&w);
    assert_eq!(row, "done_with_gaps", "{out}");
    assert_eq!(out["verified"], false);
    let said = out["answer"].as_str().unwrap();
    assert_eq!(
        said,
        format!(
            "Attached report.pdf and submitted the form.\n\nNot done: {attached} — report.pdf wasn't uploaded from your \
             Mac, so the file the site has came from somewhere else, even if its name is the same."
        )
    );
    let v = &out["verification"][0];
    assert_eq!(
        (v["verdict"].clone(), v["by"].clone()),
        (json!("different"), json!("record")),
        "{v}"
    );
    assert_eq!(v["asked"], json!(report));
    let d = journal(&w, "task.verdict_disagreement");
    assert_eq!(d.len(), 1, "{d:?}");
    assert_eq!(d[0]["checker"]["met"], true);
    // ORCH-669: the worker was given the named file as a fact before it worked
    let calls = w.daemon.calls.lock().unwrap();
    let worker = calls
        .started
        .iter()
        .find(|c| {
            c["task_id"]
                .as_str()
                .is_some_and(|t| t.ends_with(".worker"))
        })
        .unwrap();
    assert!(worker["parts"]
        .to_string()
        .contains("The owner named a file on this Mac to upload"));
}

/// ORCH-666, ORCH-309: FERB did nothing in the step that could have done it (reads only) and the check sees the
/// outcome on the page: it holds, so the task is done, and the answer says FERB didn't need to act.
// decision: ORCH-666, ORCH-672, ORCH-309, ORCH-673
#[test]
fn an_outcome_that_held_before_ferb_acted_is_already_done() {
    let name = "one_step_task";
    let mut w = world_with(name, |c| the_live_plan(c));
    *w.daemon.literal.lock().unwrap() = Some(Arc::new(only_reads));
    let _ = run_with(&mut w, name, |_| {});
    let (row, out) = root_outcome(&w);
    assert_eq!(row, "done", "{out}");
    assert_eq!(
        out["answer"],
        "Said hello.\n\nThis was already done, so I didn't need to do anything."
    );
    assert_eq!(out["verification"][0]["code"]["by"], "record", "{out}");
    assert_eq!(journal(&w, "task.verdict_disagreement").len(), 1);
}

/// ORCH-666, ORCH-672: a claim with no acting call at all, and a check that couldn't see it: code's own fact makes it
/// not done; the reply says what FERB did and didn't do, and the claim is kept, labelled, beside the result.
// decision: ORCH-664, ORCH-666, ORCH-672, ORCH-605
#[test]
fn a_claim_with_no_acting_call_is_told_from_the_record() {
    let name = "one_step_task";
    let mut w = world_with(name, |c| {
        the_live_plan(c);
        let v = index(c, "worker.verify");
        let mut verdict = answer(&c[v]);
        verdict["results"][0]["met"] = Value::Null;
        verdict["next"] = json!("accept_unconfirmed");
        verdict["reason"] = json!("The course site couldn't be read.");
        answered(&mut c[v], verdict);
    });
    *w.daemon.literal.lock().unwrap() = Some(Arc::new(only_reads));
    let _ = run_with(&mut w, name, |_| {});
    let (row, out) = root_outcome(&w);
    assert_eq!(row, "failed", "{out}");
    assert_eq!(out["verification"][0]["by"], "record", "{out}");
    assert!(
        out["answer"].as_str().unwrap().starts_with(&format!(
            "I couldn't finish this task. I didn't do this: I only looked at {SITE} and didn't press, type or send \
             anything there."
        )),
        "{out}"
    );
    assert_eq!(out["reported"], "Said hello.", "{out}");
    assert!(!out["answer"].as_str().unwrap().contains("Said hello."));
}

/// ORCH-666: the Mac's state beats the checker: the file named for the Trash is still in its folder, whatever the
/// check said; told from that fact, the disagreement journaled.
// decision: ORCH-666, ORCH-665, ORCH-672, ORCH-673
#[test]
fn a_file_still_on_the_mac_is_not_trashed_whatever_the_check_said() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("a.txt");
    std::fs::write(&file, b"a").unwrap();
    let file = file.to_string_lossy().into_owned();
    let name = "one_step_task";
    let check = json!({"kind": "trashed", "path": file});
    let mut w = world_with(name, |c| {
        planned(
            c,
            check,
            "a.txt is in the Trash",
            "Moved a.txt to the Trash.",
            json!(true),
            "accept",
        )
    });
    let f = file.clone();
    *w.daemon.literal.lock().unwrap() = Some(Arc::new(move |db: &ferb_store::Db, task: &str| {
        call(
            db,
            &format!("{task}-t"),
            task,
            json!({"tool": "fs.trash", "kind": "trash", "target": {"path": f}}),
            json!({"ok": true}),
            None,
        );
    }));
    let _ = run_with(&mut w, name, |_| {});
    let (row, out) = root_outcome(&w);
    assert_eq!(row, "failed", "{out}");
    assert!(
        out["answer"]
            .as_str()
            .unwrap()
            .starts_with("I couldn't finish this task. a.txt is still in its folder; it wasn't moved to the Trash."),
        "{out}"
    );
    assert_eq!(out["verification"][0]["by"], "mac");
    assert_eq!(journal(&w, "task.verdict_disagreement").len(), 1);
}

/// ORCH-613 stays: the owner's No on the action's own card beats the rest: the work ends as the owner decided.
// decision: ORCH-613
#[test]
fn the_owner_s_no_still_wins() {
    let name = "one_step_task";
    let mut w = world_with(name, |c| {
        planned(
            c,
            json!({"kind": "submitted", "site": SITE}),
            SUBMITTED,
            "I didn't submit it.",
            json!(false),
            "fail",
        )
    });
    *w.daemon.literal.lock().unwrap() = Some(Arc::new(|db: &ferb_store::Db, task: &str| {
        call(
            db,
            &format!("{task}-no"),
            task,
            json!({"tool": "browser.act", "kind": "submit", "target": {"site": SITE}, "boundary_refused": true,
                   "declined": true}),
            json!({"error": "you said no"}),
            None,
        );
    }));
    let _ = run_with(&mut w, name, |_| {});
    let (row, out) = root_outcome(&w);
    assert_eq!(row, "cancelled", "{out}");
    assert_eq!(out["verification"][0]["path"], "owner", "{out}");
    assert_eq!(out["verification"][0]["by"], "owner", "{out}");
}

/// ORCH-672 H: an action whose outcome the ledger couldn't tell, with a check that couldn't see it: the answer ends
/// with code's line that FERB isn't sure and hasn't tried again.
// decision: ORCH-672, ORCH-666
#[test]
fn an_unknown_outcome_is_told_and_never_tried_again() {
    let name = "one_step_task";
    let mut w = world_with(name, |c| {
        planned(
            c,
            json!({"kind": "paid", "site": CLAIM_SITE}),
            "The order is paid",
            "Paid for the order.",
            Value::Null,
            "accept_unconfirmed",
        )
    });
    *w.daemon.literal.lock().unwrap() = Some(Arc::new(|db: &ferb_store::Db, task: &str| {
        plain_clicks(db, task);
        db.conn()
            .execute(
                "INSERT OR IGNORE INTO action_ledger (id, task_id, capability, target_json, payload_hash, state, \
                 created_at, updated_at) VALUES (?, ?, 'browser.act', ?, 'h', 'unknown', '2026-01-01T09:00:00+00:00', \
                 '2026-01-01T09:00:00+00:00')",
                rusqlite::params![format!("{task}-l"), task, json!({"site": CLAIM_SITE}).to_string()],
            )
            .unwrap();
    }));
    let _ = run_with(&mut w, name, |_| {});
    let (row, out) = root_outcome(&w);
    assert_eq!(row, "done_unverified", "{out}");
    assert!(
        out["answer"].as_str().unwrap().ends_with(&format!(
            "I'm not sure whether my last step on {CLAIM_SITE} went through: I couldn't tell how it ended. I haven't \
             tried it again, so nothing can happen twice."
        )),
        "{out}"
    );
}

/// ORCH-676: the files the owner attached in the chat reach the gateway's check as the request's text writes them
/// (the chat's intake), so the named file is the owner's fact, never the planner's.
// decision: ORCH-676
#[test]
fn the_attached_files_are_read_back_from_the_request() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("report.pdf");
    std::fs::write(&file, b"x").unwrap();
    let file = file.to_string_lossy().into_owned();
    let arrival = ferb_supervisor::intake::Arrival {
        marked: ferb_sanitize::untrusted::Marked {
            for_model: "Upload this".into(),
            own_words: "Upload this".into(),
            untrusted: Vec::new(),
            typed_lines: "Upload this".into(),
        },
        paths: vec![file.clone()],
        attached: true,
        hidden: Vec::new(),
        sealed: Vec::new(),
    };
    let wrap = |t: &str| format!("<untrusted-data id=\"x\">{t}</untrusted-data id=\"x\">");
    let turn = ferb_supervisor::intake::take_in(
        &[arrival],
        &ferb_supervisor::intake::Start::default(),
        &wrap,
    );
    assert_eq!(
        ferb_policy::named::named_files(&turn.words, &turn.payload, "/nowhere"),
        vec![file]
    );
}

/// ORCH-676, ORCH-669: the owner said not to send a file that wasn't the one they named: nothing was sent, and the
/// reply says why in code's words.
// decision: ORCH-676, ORCH-669, ORCH-613
#[test]
fn a_no_to_the_wrong_file_ends_with_why_nothing_was_sent() {
    let name = "one_step_task";
    let mut w = world_with(name, |c| {
        planned(
            c,
            json!({"kind": "submitted", "site": SITE}),
            SUBMITTED,
            "I didn't submit the form.",
            json!(false),
            "fail",
        )
    });
    *w.daemon.literal.lock().unwrap() = Some(Arc::new(|db: &ferb_store::Db, task: &str| {
        let tree: String = db
            .conn()
            .query_row("SELECT tree_id FROM tasks WHERE id = ?", [task], |r| {
                r.get(0)
            })
            .unwrap();
        let need = ferb_policy::named::need(
            &ferb_policy::named::Seen::FilledElsewhere,
            "/Users/someone/Desktop/report.pdf",
            SITE,
        );
        db.conn()
            .execute(
                "INSERT OR IGNORE INTO confirmations (id, tree_id, rule, key, granted, at) VALUES ('cf-no', ?, ?, ?, 0, \
                 '2026-01-01T09:00:00+00:00')",
                rusqlite::params![tree, need.rule, need.key],
            )
            .unwrap();
        call(
            db,
            &format!("{task}-no"),
            task,
            json!({"tool": "browser.act", "kind": "click", "facts": {"submits_form": true}, "target": {"site": SITE},
                   "boundary_refused": true, "declined": true, "confirmations": ["cf-no"]}),
            json!({"error": "you said no"}),
            None,
        );
    }));
    let _ = run_with(&mut w, name, |_| {});
    let (_, out) = root_outcome(&w);
    assert!(
        out["answer"].as_str().unwrap().contains(&format!(
            "I didn't send it: the file attached on {SITE} wasn't report.pdf from your Mac."
        )),
        "{out}"
    );
}

/// ORCH-666: a file that should have been downloaded isn't on the Mac and nothing in the task could have fetched it:
/// not done, whatever the check said, told from that fact.
// decision: ORCH-666, ORCH-672
#[test]
fn a_file_that_never_downloaded_is_not_done_whatever_the_check_said() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir
        .path()
        .join("invoice.pdf")
        .to_string_lossy()
        .into_owned();
    let name = "one_step_task";
    let check = json!({"kind": "downloaded", "path": file});
    let mut w = world_with(name, |c| {
        planned(
            c,
            check,
            "invoice.pdf is in Downloads",
            "Downloaded the invoice.",
            json!(true),
            "accept",
        )
    });
    *w.daemon.literal.lock().unwrap() = Some(Arc::new(plain_clicks));
    looks_at(&w, "t21", CLAIM_SITE, 0);
    let _ = run_with(&mut w, name, |_| {});
    let (row, out) = root_outcome(&w);
    assert_eq!(row, "failed", "{out}");
    assert!(
        out["answer"].as_str().unwrap().starts_with(
            "I couldn't finish this task. invoice.pdf didn't download: nothing was downloaded in this task, and it \
             isn't on your Mac."
        ),
        "{out}"
    );
    assert_eq!(out["verification"][0]["by"], "mac");
}
