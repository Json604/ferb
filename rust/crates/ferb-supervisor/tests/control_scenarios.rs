//! Control and recovery replayed against the recorded Python daemon (rust/fixtures/daemon/chat_scenarios.json): every
//! frame each connection received, every model call (with what FERB delivered into a running call), the daemon's own
//! replies the scenario's calls got, and every store table except those another part owns. The harness is
//! tests/support_control (the forward path's replay harness, plus held and stalled model calls, the daemon's stop,
//! question and restart handlers and the bus).
//!
//! The scenarios that replay fully are asserted to; the others are asserted up to the point named in `PARTLY`, which
//! says whose path each needs.

mod common;
mod support_control;

use support_control::{run, world, Report};

fn replay(name: &str) -> Report {
    let mut w = world(name);
    let r = run(&mut w, name);
    eprintln!("{name}: {}", r.verdict());
    if !r.skipped.is_empty() {
        eprintln!(
            "  not compared (the daemon's own composition): {}",
            r.skipped.join(", ")
        );
    }
    eprintln!("  {} comparisons", r.checked);
    r
}

fn full(name: &str) {
    let r = replay(name);
    assert!(r.full(), "{name}: {}", r.verdict());
}

// decision: ORCH-579
#[test]
fn stop_by_chat() {
    full("stop_by_chat");
}

// decision: ORCH-638
#[test]
fn stop_plus_new_task() {
    full("stop_plus_new_task");
}

// decision: ORCH-508
#[test]
fn stop_by_task_stop() {
    full("stop_by_task_stop");
}

// decision: ORCH-308
#[test]
fn steer_running_task() {
    full("steer_running_task");
}

// decision: ORCH-307
#[test]
fn check_in_after_five() {
    full("check_in_after_five");
}

// decision: ORCH-323
#[test]
fn worker_question_answered() {
    full("worker_question_answered");
}

/// The global stop (`stop.engage`): the gateway's stop order (block, revoke every run token, the hooks: the
/// supervisor's `cancel_all`), the running attempt ends cancelled ("FERB was stopped") and the task failed; a chat
/// message while stopped is answered; `stop.lift`.
///
/// The one deliberate difference from Python (ENG-6, GATE-581): Python ends the task with the summary
/// "(from the task) FERB was stopped", marking FERB's own words as the task's; the port says "FERB was stopped". The
/// recording is compared with that one text changed, everywhere it appears (the outcome frame, the task row, the
/// span, the journal's `task.outcome`).
// decision: GATE-581
#[test]
fn global_stop_with_the_stop_words_as_ferbs_own() {
    let (_, sc) = support_control::scenario("global_stop");
    let recorded = serde_json::to_string(sc).unwrap();
    assert!(
        recorded.contains("(from the task) FERB was stopped"),
        "the Python recording marks the stop words as the task's"
    );
    let mut w = world("global_stop");
    w.fixes.push((
        "(from the task) FERB was stopped".into(),
        "FERB was stopped".into(),
    ));
    let r = run(&mut w, "global_stop");
    eprintln!("global_stop (GATE-581 fixed): {}", r.verdict());
    assert!(r.full(), "global_stop: {}", r.verdict());
}

/// The scenarios that replay up to a path another slice owns: each is asserted to match everything before the point
/// named here (the first difference, or the step the replay stops at), so a regression before it fails.
// decision: ORCH-308
#[test]
fn partly_up_to_the_paths_they_still_need() {
    let cases: [(&str, &str, &str); 9] = [
        // the resumed attempt's file write goes through the gateway's tool bridge (the forward path's join)
        ("pause_steer_resume", "frames", "chat line 9"),
        // the protected write that asks is a tool call through the gateway (the forward path's join)
        (
            "confirm_yes",
            "stopped",
            "a wait for an approval is pending",
        ),
        ("confirm_no", "stopped", "a wait for an approval is pending"),
        (
            "confirm_stop_pending",
            "stopped",
            "a wait for an approval is pending",
        ),
        // a shutdown suspends the tree: run_tree records no end for it, then resumes it by resume_id (tree.rs)
        ("restart_mid_task", "frames", "chat line 5"),
        ("restart_with_open_question", "frames", "chat line 17"),
        // the schedule is a side entry of the planner's answer (the forward path's `_side_entries`)
        ("schedule_created_and_fired", "frames", "app#2 line 1"),
        // the planner's task state shows the open check-in's card row (`_task_state`'s cards, turn.rs)
        (
            "steer_during_check_in",
            "calls",
            "call 12 (supervisor) parts: part 5",
        ),
        // two trees' step lines interleave as Python's threads scheduled them (its model answered from a thread)
        ("pause_plus_new_task", "frames", "app line 5"),
    ];
    for (name, kind, at) in cases {
        let r = replay(name);
        let first = match kind {
            "frames" => r.frames.first().cloned(),
            "calls" => r.calls.first().cloned(),
            _ => r.stopped.clone(),
        };
        assert!(
            first.as_deref().is_some_and(|f| f.starts_with(at)),
            "{name}: expected to match up to {at}, got {}",
            r.verdict()
        );
    }
}

/// The control fixtures made by rust/fixtures/supervisor/make_control.py: a stop, a pause or a correction at each
/// await the Python tests cover.
#[test]
fn control_fixtures() {
    let names: Vec<String> = support_control::control()["scenarios"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["name"].as_str().unwrap().to_string())
        .collect();
    let partly = ["steer_during_check_in", "pause_plus_new_task"];
    for name in names {
        let r = replay(&name);
        if !partly.contains(&name.as_str()) {
            assert!(r.full(), "{name}: {}", r.verdict());
        }
    }
}
