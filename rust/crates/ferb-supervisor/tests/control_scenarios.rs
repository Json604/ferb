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

use serde_json::{json, Value};
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
    let cases: [(&str, &str, &str); 6] = [
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
        // (restart_mid_task and restart_with_open_question replay fully in tests/replay.rs, on the real gateway)
        // this harness has no daemon side (group A's schedules and timer loop): tests/replay.rs replays it fully
        ("schedule_created_and_fired", "frames", "app#2 line 1"),
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
    // (steer_during_check_in replays fully since the planner's task state shows the open check-in's card row and the
    // open questions block lists it: `_task_state`'s cards, `open_questions`)
    let partly = ["pause_plus_new_task"];
    for name in names {
        let r = replay(&name);
        if !partly.contains(&name.as_str()) {
            assert!(r.full(), "{name}: {}", r.verdict());
        }
    }
}

/// The journal's rows of `kind` after a run (their data).
fn rows_of(w: &support_control::World, kind: &str) -> Vec<Value> {
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

/// supervisor.py:4980 `_capture_after_outcome`: a capture the daemon refused because FERB is stopping (`Deferred`) is
/// journaled as deferred, and one whose hook raised as failed with its error; a task's outcome never waits for either.
#[test]
fn an_episode_capture_after_the_outcome_journals_how_it_ended() {
    let name = "stop_by_chat";
    let mut w = world(name);
    *w.daemon.episode_fault.lock().unwrap() = Some(ferb_supervisor::flow::EpisodeFault::Deferred);
    let _ = run(&mut w, name);
    let deferred = rows_of(&w, "memory.episode_deferred");
    assert!(!deferred.is_empty(), "no deferred capture journaled");
    assert!(deferred.iter().all(|d| d["task_id"].is_string()));

    let mut w = world(name);
    *w.daemon.episode_fault.lock().unwrap() = Some(ferb_supervisor::flow::EpisodeFault::Failed(
        "RuntimeError: the card store is closed".into(),
    ));
    let _ = run(&mut w, name);
    let failed = rows_of(&w, "memory.episode_hook_failed");
    assert!(!failed.is_empty(), "no failed capture journaled");
    assert_eq!(failed[0]["error"], "RuntimeError: the card store is closed");
    assert!(rows_of(&w, "memory.episode_deferred").is_empty());
}

/// supervisor.py:5010 `stop_episodes` (main.py:526): the shutdown cancels a capture still under way rather than
/// wait for it, and journals it as deferred (the hourly pass captures the card again).
#[test]
fn the_shutdown_cancels_a_pending_episode_capture_as_deferred() {
    let name = "stop_by_chat";
    let mut w = world(name);
    w.daemon
        .episode_hang
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let _ = run(&mut w, name);
    assert!(rows_of(&w, "memory.episode_deferred").is_empty());
    let mut w = world(name);
    w.daemon
        .episode_hang
        .store(true, std::sync::atomic::Ordering::SeqCst);
    w.daemon
        .shut_down_at_end
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let _ = run(&mut w, name);
    assert!(!rows_of(&w, "memory.episode_deferred").is_empty());
}

/// supervisor.py:5657 `_wait_cut` against `rust/fixtures/supervisor/wait_cut.json` (made by `make_wait_cut.py`
/// from the real method on the real journal): the latest `wait.ended` row of the tree, its time in the Mac's zone, its
/// line made printable; none for another tree's or no row. Each case through the supervisor on a scenario's store
/// (UTC), and the text for its own zone.
// decision: ORCH-633
#[test]
fn a_wait_a_restart_cut_is_told_as_python_tells_it() {
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/supervisor/wait_cut.json");
    let f: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    let cases = f["cases"].as_array().unwrap();
    for case in cases {
        let tree = case["tree"].as_str().unwrap();
        let rows: Vec<&Value> = case["rows"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|r| r[2]["tree"] == tree)
            .collect();
        let zone = ferb_clock::zone::Zone::named(case["zone"].as_str().unwrap()).unwrap();
        let text = rows
            .last()
            .and_then(|r| ferb_supervisor::lines::wait_cut(r[1].as_str().unwrap(), &r[2], &zone));
        assert_eq!(json!(text), case["text"], "{}", case["name"]);
        if case["zone"] != "UTC" {
            continue;
        }
        let w = world("stop_by_chat");
        {
            let db = w.db.lock().unwrap();
            for (i, r) in case["rows"].as_array().unwrap().iter().enumerate() {
                db.conn()
                    .execute(
                        "INSERT INTO journal (id, at, kind, data_json, origin) VALUES (?, ?, ?, ?, 'ferb')",
                        rusqlite::params![
                            format!("wc-{i}"),
                            r[1].as_str().unwrap(),
                            r[0].as_str().unwrap(),
                            ferb_py::json::dumps(&r[2])
                        ],
                    )
                    .unwrap();
            }
        }
        assert_eq!(
            json!(w.sup.wait_cut(tree)),
            case["text"],
            "{} (supervisor)",
            case["name"]
        );
    }
    println!("{} cases equal", cases.len());
}
