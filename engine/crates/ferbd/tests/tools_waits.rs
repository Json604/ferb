//! `wait.for` and `time.sleep_until` through the real binary: a run token's calls over the shim's socket path, the
//! sleep's durable timer on the store, the fake clock moved through the control socket.

mod shim;
mod support;

use std::time::{Duration, Instant};

use serde_json::{json, Value};
use shim::{started, Shim};
use support::*;

fn until(what: &str, mut f: impl FnMut() -> bool) {
    let end = Instant::now() + Duration::from_secs(30);
    while !f() {
        assert!(Instant::now() < end, "{what}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn sleeps(run: &Run) -> Vec<Value> {
    run.table("timers")
        .as_array()
        .unwrap()
        .iter()
        .filter(|r| r[2] == "sleep")
        .cloned()
        .collect()
}

#[test]
fn sleep_until_wakes_when_its_timer_fires() {
    let tmp = root();
    let (run, shim) = started(tmp.path(), "01");
    let mut shim = shim;
    let call = std::thread::spawn(move || shim.call("time.sleep_until", json!({"in_minutes": 5})));
    until("the sleep's timer", || sleeps(&run).len() == 1);
    // not due yet: the timer loop leaves it
    run.control(json!({"do": "timers_tick"}));
    std::thread::sleep(Duration::from_millis(1500));
    assert!(!call.is_finished());
    run.control(json!({"do": "clock", "advance_s": 301.0}));
    run.control(json!({"do": "timers_tick"}));
    let out = call.join().unwrap();
    assert_eq!(out["is_error"], false, "{out}");
    assert_eq!(
        out["content"]["woke_at"], "2026-01-01T09:05:01+00:00",
        "{out}"
    );
    // a time past the task's own limit is refused before any timer
    let token = run.control(
        json!({"do": "issue", "task_id": "task-2", "worker_id": "w-2",
                                   "identity": "owner", "scopes": ["time"]}),
    );
    run.control(json!({"do": "hold_task", "task_id": "task-2", "deadline_s": 60.0}));
    let mut short = Shim::open(&run, token.as_str().unwrap());
    let late = short.call("time.sleep_until", json!({"in_minutes": 5}));
    assert_eq!(late["is_error"], true, "{late}");
    assert!(late["content"]["error"]
        .as_str()
        .unwrap()
        .contains("past this task's time limit"));
}

#[test]
fn wait_for_a_command_and_a_time() {
    let tmp = root();
    let (run, mut shim) = started(tmp.path(), "01");
    // a command FERB's shell runs in its sandbox; the wait wakes when it exits
    let started = shim.call(
        "shell.start",
        json!({"command": "sleep 1; echo finished", "background": true}),
    );
    assert_eq!(started["is_error"], false, "{started}");
    let id = started["content"]["id"].as_str().unwrap().to_string();
    let woke = shim.call(
        "wait.for",
        json!({"any_of": [{"kind": "process", "id": id}], "limit_minutes": 1}),
    );
    assert_eq!(woke["is_error"], false, "{woke}");
    assert_eq!(woke["content"]["woke"], "process", "{woke}");
    assert!(woke.to_string().contains("finished"), "{woke}");
    // a time two minutes ahead on the daemon's clock: it wakes once the clock passes it
    let mut later = shim;
    let call = std::thread::spawn(move || {
        later.call(
            "wait.for",
            json!({"any_of": [{"kind": "time", "in_minutes": 2}], "limit_minutes": 5}),
        )
    });
    std::thread::sleep(Duration::from_millis(1500));
    assert!(!call.is_finished());
    run.control(json!({"do": "clock", "advance_s": 121.0}));
    let out = call.join().unwrap();
    assert_eq!(out["content"]["woke"], "time", "{out}");
}
