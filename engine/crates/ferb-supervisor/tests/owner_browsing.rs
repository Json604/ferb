//! ORCH-648: a task the planner gave the owner's identity that will browse (its task names sites) goes through FERB's
//! one browsing question when the owner hasn't picked a profile, and browses in the pick when they have, unless the
//! owner's words name one of their profiles. Live 2026-11-15 02:29 (fresh install, no pick): the plan asked "Which
//! Chrome profile should I use for the Google Sheet?" as its own step, the answer was used for that plan only, and
//! the next task at 02:39 asked again. Scripted on the recorded `stop_while_step_asks` scenario with the planner's
//! answer replaced (a plan that only waits, so no worker runs), stopped by the owner while it waits.

mod common;
mod support_control;

use serde_json::{json, Map, Value};
use support_control::{run_with, world_with, World};

fn plan_answer(identity: &str, sites: &[&str]) -> String {
    let wait = json!({"id": "wait_noon", "kind": "wait", "label": "", "goal": "Wait for the morning",
        "depends_on": [], "inputs": [], "worker_type": "default", "conditions": [],
        "until": "2030-01-01T09:00:00+00:00", "event": null, "event_match": [], "question": "", "options": [],
        "conversation": null, "repeat": null, "earlier_results": []});
    json!({"kind": "task", "answers_question": false, "task": {
        "goal": "Put the invented list in a sheet", "name": "Sheet", "identity": identity,
        "worker_type": "default", "targets": {"apps": [], "sites": sites, "paths": [], "recipients": []},
        "success_conditions": [{"text": "The sheet exists", "owner_text": "", "layer": "outcome", "check": null}],
        "plan": {"steps": [wait], "input_paths": [], "big_job": false, "time_estimate": "", "cost_estimate": "",
                 "standing_sends": []},
        "folders": [], "keep_tabs_open": false, "parts": [], "earlier_results": []},
        "control": null, "side_entries": null, "text": "On it."})
    .to_string()
}

/// The planner answers with `identity` and `sites`; the browser area answers `browsing` as given; the owner's
/// profiles are `profiles`. The run ends with the owner's stop while the plan waits.
fn run(identity: &str, sites: &[&str], browsing: Vec<Value>, profiles: &[&str]) -> World {
    let text = plan_answer(identity, sites);
    let mut w = world_with("stop_while_step_asks", |calls| {
        let planner = calls.iter_mut().find(|c| c["key"] == "supervisor").unwrap();
        planner["events"] = json!([
            {"kind": "usage", "data": {"input_tokens": 10, "output_tokens": 5, "cost_usd": 0.001}},
            {"kind": "final", "data": {"text": text}}
        ]);
    });
    *w.daemon.browsing.lock().unwrap() = browsing
        .into_iter()
        .map(|v| v.as_object().cloned().unwrap_or_else(Map::new))
        .collect();
    *w.daemon.profile_names.lock().unwrap() = profiles.iter().map(|p| p.to_string()).collect();
    let report = run_with(&mut w, "stop_while_step_asks", |steps| {
        for st in steps.iter_mut() {
            if st["do"] == "wait" {
                st["until"] = json!("a plan step waits");
            }
        }
        let at = steps.iter().position(|st| st["do"] == "close").unwrap();
        steps.insert(at + 1, json!({"do": "wait", "until": "the task ended"}));
    });
    eprintln!("scripted: {}", report.verdict());
    w
}

fn browses_as_owner(w: &World) -> bool {
    let db = w.db.lock().unwrap();
    db.conn()
        .query_row(
            "SELECT count(*) FROM journal WHERE kind = 'task.browses_as_owner'",
            [],
            |r| r.get::<_, i64>(0),
        )
        .unwrap()
        > 0
}

fn asked_while_here(w: &World) -> Vec<bool> {
    w.daemon
        .browsing_calls
        .lock()
        .unwrap()
        .iter()
        .map(|(attended, _)| *attended)
        .collect()
}

// No pick yet: FERB's one browsing question is raised (with the owner here) before the plan runs, and the answer
// (read again once the question is settled) puts the task in the picked profile.
// decision: ORCH-648
#[test]
fn a_task_as_the_owner_that_browses_asks_the_one_browsing_question_without_a_pick() {
    let w = run(
        "owner",
        &["docs.google.com"],
        vec![
            json!({"ask": "ap-browsing"}),
            json!({"as_owner": true, "label": "Chrome — Work"}),
        ],
        &[],
    );
    assert_eq!(
        asked_while_here(&w),
        [true, false],
        "asked with the owner here, then read once more"
    );
    assert!(browses_as_owner(&w), "the task browses in the pick");
}

// With a pick the task browses there by default (HAND-644: a call may still name another of the owner's profiles).
// decision: ORCH-648
#[test]
fn a_task_as_the_owner_that_browses_uses_the_pick() {
    let w = run(
        "owner",
        &["docs.google.com"],
        vec![json!({"as_owner": true, "label": "Chrome — Work"})],
        &[],
    );
    assert_eq!(asked_while_here(&w), [true]);
    assert!(browses_as_owner(&w));
}

// The owner's words name one of their profiles ("downloads" is a profile's name here): the task browses where
// their words say; nothing is asked and the pick isn't imposed.
// decision: ORCH-648
#[test]
fn a_profile_named_in_the_owner_s_words_is_left_to_them() {
    let w = run(
        "owner",
        &["docs.google.com"],
        vec![json!({"as_owner": true, "label": "Chrome — Work"})],
        &["Downloads"],
    );
    assert!(asked_while_here(&w).is_empty(), "nothing asked");
    assert!(!browses_as_owner(&w));
}

// A task as the owner that names no site (a file, a Mac app) never meets the browsing question.
// decision: ORCH-648
#[test]
fn a_task_as_the_owner_that_names_no_site_is_not_asked() {
    let w = run("owner", &[], vec![json!({"ask": "ap-browsing"})], &[]);
    assert!(asked_while_here(&w).is_empty());
    assert!(!browses_as_owner(&w));
}
