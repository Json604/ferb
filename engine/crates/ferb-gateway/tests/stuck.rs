//! ORCH-691 (owner, 2026-11-22: "8 failed actions or 4 min"): the gateway's count for one step (crate::stuck), on
//! structure only: failed or unchanged on-screen actions on one app, and the worker's own time without progress.

use ferb_gateway::stuck::{words, Done, Steps, FAILED_ON_ONE_APP, NO_PROGRESS_S};
use serde_json::{json, Map, Value};

fn args(v: Value) -> Map<String, Value> {
    v.as_object().unwrap().clone()
}

/// One finished call of `tool` (on screen into `app` when it isn't "").
#[allow(clippy::too_many_arguments)]
fn call(
    s: &mut Steps,
    worker: &str,
    tool: &str,
    app: &str,
    a: Value,
    content: Value,
    error: bool,
    now: f64,
) {
    s.started("task-1", worker, now);
    let a = args(a);
    let d = Done {
        tool,
        on_screen: !app.is_empty() && !(tool.contains("snapshot") || tool.contains("read")),
        observation: tool.contains("snapshot") || tool.contains("read"),
        app,
        app_label: "Google Chrome",
        args: &a,
        content: &content,
        is_error: error,
    };
    s.ended("task-1", worker, &d, now);
}

fn unchanged() -> Value {
    json!({"ok": true, "after": {"read": "ax", "changed": null}})
}

const CHROME: &str = "com.google.chrome";

fn fail(s: &mut Steps, i: u32, app: &str) {
    fail_as(s, "w1", i, app);
}

fn fail_as(s: &mut Steps, worker: &str, i: u32, app: &str) {
    let kinds = [
        (unchanged(), false),
        (
            json!({"ok": false, "detail": "AX focus request refused"}),
            false,
        ),
        (
            json!({"error": "the app changed since the screenshot"}),
            true,
        ),
        (json!({"performed": false}), false),
    ];
    let (c, e) = kinds[i as usize % kinds.len()].clone();
    call(
        s,
        worker,
        "mac.act",
        app,
        json!({"app": "pid:1"}),
        c,
        e,
        1.0,
    );
}

/// A long healthy task: a few failed or unchanged actions now and then, each run broken by progress on the app (a
/// visible change, or a new observation of it), never asks; failures in other apps don't count toward this one.
// decision: ORCH-691, ORCH-692
#[test]
fn failures_broken_by_progress_on_the_app_never_stop_a_long_task() {
    let mut s = Steps::default();
    // each round its own attempt (the in-a-row count is the step's)
    for round in 0..50u32 {
        let w = format!("w{round}");
        for i in 0..FAILED_ON_ONE_APP - 1 {
            fail_as(&mut s, &w, i, CHROME);
            fail_as(&mut s, &w, i, "com.apple.notes");
        }
        if round % 2 == 0 {
            call(
                &mut s,
                &w,
                "mac.act",
                CHROME,
                json!({}),
                json!({"ok": true, "after": {"read": "ax", "changed": true}}),
                false,
                1.0,
            );
        } else {
            call(
                &mut s,
                &w,
                "mac.snapshot",
                CHROME,
                json!({"app": "pid:1", "window": round}),
                json!({"elements": []}),
                false,
                1.0,
            );
        }
        call(
            &mut s,
            &w,
            "mac.snapshot",
            "com.apple.notes",
            json!({"app": "Notes", "window": round}),
            json!({"elements": []}),
            false,
            1.0,
        );
        assert_eq!(s.check(&w, 1.0), None, "round {round}");
    }
}

/// Eight failed or unchanged actions in a row on one app stop the attempt, once; neither an action nobody looked at
/// afterwards, the same observation again, nor progress in another app breaks the row.
// decision: ORCH-691, ORCH-692
#[test]
fn eight_failed_or_unchanged_actions_in_a_row_on_one_app_stop_the_attempt_once() {
    let mut s = Steps::default();
    call(
        &mut s,
        "w1",
        "mac.snapshot",
        CHROME,
        json!({"app": "pid:1"}),
        json!({"elements": []}),
        false,
        1.0,
    );
    for i in 0..FAILED_ON_ONE_APP - 1 {
        fail(&mut s, i, CHROME);
        call(
            &mut s,
            "w1",
            "mac.menu",
            CHROME,
            json!({}),
            json!({"pressed": "Profiles > X", "after": {"read": "none"}}),
            false,
            1.0,
        );
        call(
            &mut s,
            "w1",
            "mac.snapshot",
            CHROME,
            json!({"app": "pid:1"}),
            json!({"elements": []}),
            false,
            1.0,
        );
        call(
            &mut s,
            "w1",
            "mac.act",
            "com.apple.notes",
            json!({}),
            json!({"ok": true, "after": {"read": "ax", "changed": true}}),
            false,
            1.0,
        );
        assert_eq!(s.check("w1", 1.0), None, "{i}");
    }
    fail(&mut s, 0, CHROME);
    let why = s.check("w1", 1.0).expect("the eighth in a row stops it");
    assert_eq!(
        why,
        json!({"why": "failed_actions", "app": "Google Chrome", "count": 8})
    );
    assert_eq!(
        words(&why),
        "8 of FERB's actions in Google Chrome failed or changed nothing, so FERB stopped to ask you"
    );
    // said once, latched once for the supervisor; the step counts afresh
    assert_eq!(s.check("w1", 1.0), None);
    assert!(s.take_tripped("task-1").is_some());
    assert!(s.take_tripped("task-1").is_none());
    call(
        &mut s,
        "w2",
        "mac.act",
        CHROME,
        json!({}),
        unchanged(),
        false,
        2.0,
    );
    assert_eq!(s.check("w2", 2.0), None);
}

// decision: ORCH-691, ORCH-692
#[test]
fn four_minutes_of_the_workers_own_time_without_progress_stop_the_attempt() {
    let mut s = Steps::default();
    call(
        &mut s,
        "w1",
        "browser.read",
        "",
        json!({"tab": 1}),
        json!({"text": "a"}),
        false,
        0.0,
    );
    // the same observation again is no progress; time goes by while the worker thinks
    call(
        &mut s,
        "w1",
        "mac.snapshot",
        "",
        json!({"app": "pid:1"}),
        json!({"elements": []}),
        false,
        100.0,
    );
    call(
        &mut s,
        "w1",
        "mac.snapshot",
        "",
        json!({"app": "pid:1"}),
        json!({"elements": []}),
        false,
        200.0,
    );
    assert_eq!(s.check("w1", 239.0), None);
    // a new observation is progress
    call(
        &mut s,
        "w1",
        "browser.read",
        "",
        json!({"tab": 2}),
        json!({"text": "b"}),
        false,
        239.0,
    );
    assert_eq!(s.check("w1", 239.0 + NO_PROGRESS_S - 1.0), None);
    // time inside a call (a wait) doesn't count
    s.started("task-1", "w1", 239.0 + NO_PROGRESS_S - 1.0);
    assert_eq!(s.check("w1", 2000.0), None);
    let a = args(json!({"seconds": 1000}));
    let content = json!({"waited": true});
    s.ended(
        "task-1",
        "w1",
        &Done {
            tool: "wait.for",
            on_screen: false,
            observation: true,
            app: "",
            app_label: "",
            args: &a,
            content: &content,
            is_error: false,
        },
        2000.0,
    );
    assert_eq!(s.check("w1", 2000.5), None);
    // the time between two attempts (the check) doesn't count either
    call(
        &mut s,
        "w2",
        "mac.snapshot",
        "",
        json!({"app": "pid:1"}),
        json!({"elements": []}),
        false,
        5000.0,
    );
    assert_eq!(s.check("w2", 5000.0), None);
    let why = s
        .check("w2", 5000.0 + NO_PROGRESS_S)
        .expect("four minutes of its own time");
    assert_eq!(why["why"], "no_progress");
    assert_eq!(
        words(&why),
        "4 minutes went by on this step without progress, so FERB stopped to ask you"
    );
    // an acting call that went through, and an on-screen change, are progress; "keep trying" counts afresh
    call(
        &mut s,
        "w2",
        "browser.act",
        "",
        json!({"tab": 2}),
        json!({"done": true}),
        false,
        6000.0,
    );
    assert_eq!(s.check("w2", 6000.0 + NO_PROGRESS_S - 1.0), None);
    s.keep_trying("task-1");
    assert!(s.take_tripped("task-1").is_none());
}

/// ORCH-706 (owner 2026-11-23: "anyways remove taht", retiring ORCH-693's trial): task a843aa54's second attempt,
/// shaped as it ran (calls into Chrome with a visible change or a new snapshot every few calls, failures in between),
/// carried on to 500 calls into windows and pages in one attempt: nothing stops it for its length; only ORCH-691's
/// limits (8 in a row on one app, 4 minutes without progress) can.
// decision: ORCH-706, ORCH-691
#[test]
fn a_long_attempt_past_forty_window_calls_is_never_stopped_for_its_length() {
    let mut s = Steps::default();
    for n in 1..=500u32 {
        if n % 10 == 0 {
            call(
                &mut s,
                "w1",
                "fs.read",
                "",
                json!({"path": format!("/x/{n}")}),
                json!({"text": "x"}),
                false,
                n as f64,
            );
        }
        match n % 4 {
            0 => call(
                &mut s,
                "w1",
                "mac.act",
                CHROME,
                json!({"app": "pid:1"}),
                json!({"ok": true, "after": {"read": "ax", "changed": true}}),
                false,
                n as f64,
            ),
            1 => call(
                &mut s,
                "w1",
                "mac.snapshot",
                CHROME,
                json!({"app": "pid:1", "window": n}),
                json!({"elements": []}),
                false,
                n as f64,
            ),
            2 => call(
                &mut s,
                "w1",
                "browser.act",
                "",
                json!({"tab": 1, "n": n}),
                json!({"done": true}),
                false,
                n as f64,
            ),
            // a failed or unchanged action on the app, at this call's time
            _ => call(
                &mut s,
                "w1",
                "mac.act",
                CHROME,
                json!({"app": "pid:1"}),
                unchanged(),
                false,
                n as f64,
            ),
        }
        assert_eq!(s.check("w1", n as f64), None, "call {n}");
    }
    assert!(s.take_tripped("task-1").is_none());
}
