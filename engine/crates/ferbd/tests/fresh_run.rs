//! ORCH-719 item 4, ORCH-760: a worker run whose conversation passes the size (200k, or 70% of a smaller window) is
//! ended and the attempt goes on in a fresh run, with a hand-off note code builds from FERB's records (at most 1,500
//! o200k tokens). Through the composed `ferbd daemon` with the test stand-ins (tests/life, TEST-512): the scripted
//! run reports its size once its tool calls are made (`context_tokens`, as each runtime reports its own), its calls
//! go through the real gateway, the supervisor's attempt loop and its checker run as in FERB.app. Temp folders
//! only; no model.

#[path = "../../ferb-supervisor/tests/support_entries/mod.rs"]
mod entry_rules;
mod life;
mod support;

use std::time::Duration;

use life::*;
use serde_json::{json, Value};
use support::*;

const GOAL: &str = "List the home folder";
const PAST: i64 = 250_000;

/// `n` listings of the home folder, as the tool bridge makes them.
fn listings(n: usize) -> Vec<(&'static str, Value)> {
    (0..n).map(|_| ("fs.list", json!({"path": "~"}))).collect()
}

/// A worker call that makes `calls` and then reports this size, before its answer.
fn long_run(calls: &[(&str, Value)], size: i64, answer: &str) -> Value {
    let mut c = with_tools(worker_done(answer), calls);
    c["context_tokens"] = json!(size);
    c
}

/// The spans of this kind, oldest first: (task_id, input, output).
fn spans(life: &Life, kind: &str) -> Vec<(String, Value, Value)> {
    let db = rusqlite::Connection::open(life.run.home.join("ferb.db")).unwrap();
    db.busy_timeout(Duration::from_secs(5)).unwrap();
    let mut q = db
        .prepare(
            "SELECT coalesce(task_id, ''), input_json, coalesce(output_json, 'null') FROM spans WHERE kind = ?1 \
             ORDER BY rowid",
        )
        .unwrap();
    q.query_map([kind], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
        ))
    })
    .unwrap()
    .map(|r| {
        let (t, i, o) = r.unwrap();
        (
            t,
            serde_json::from_str(&i).unwrap_or(Value::Null),
            serde_json::from_str(&o).unwrap_or(Value::Null),
        )
    })
    .collect()
}

/// The worker runs started, in order, each with the parts of its context.
fn worker_runs(life: &Life) -> Vec<Vec<String>> {
    let made = life.made();
    life.started()
        .iter()
        .filter(|s| {
            made.get(s["n"].as_u64().unwrap_or(0) as usize)
                .is_some_and(|r| r == "worker:default")
        })
        .map(|s| {
            s["parts"]
                .as_array()
                .unwrap()
                .iter()
                .map(|p| p.as_str().unwrap_or("").to_string())
                .collect()
        })
        .collect()
}

/// The hand-off note among a run's parts: the part naming the goal and the earlier calls one per line.
fn note(parts: &[String]) -> Option<String> {
    parts
        .iter()
        .find(|p| p.contains(GOAL) && p.contains("fs.list at "))
        .cloned()
}

fn ask(life: &Life) -> Send {
    life.send("list my home folder")
}

fn done(life: &Life) {
    life.wait("the task is done", 60, |l| {
        l.root_status().as_deref() == Some("done")
    });
}

/// Past the size after its 10 calls, the run ends and a fresh run of the same attempt gets the note: the goal and
/// what was done, under 1,500 tokens, the latest file with its record. It is not an attempt: one check, of the
/// fresh run's answer. The fresh run past the size again with no calls of its own is not ended (10 between).
// decision: ORCH-719
// decision: ORCH-760
#[test]
fn a_run_past_the_size_goes_on_fresh_with_a_note_of_what_it_did() {
    let tmp = root();
    let life = Life::start(
        tmp.path(),
        "01",
        Model::default()
            .planner(plan_task(GOAL, "On it."))
            .worker(long_run(&listings(10), PAST, "The first run's answer."))
            .worker(long_run(&[], PAST, "Listed it."))
            .checker(always(check_accept())),
    );
    let mut turn = ask(&life);
    done(&life);
    turn.end();
    let runs = worker_runs(&life);
    assert_eq!(runs.len(), 2, "one fresh run, and only one");
    assert!(note(&runs[0]).is_none(), "the first run has no note");
    let text = note(&runs[1]).expect("the fresh run's note");
    assert_eq!(text.matches("fs.list at ").count(), 10, "{text}");
    assert!(
        text.contains("span_id="),
        "the latest place's record: {text}"
    );
    let tokens = ferb_context::tokens::count_tokens(&text);
    assert!(tokens <= 1_500, "{tokens} tokens");
    assert_eq!(life.calls_of("checker"), 1, "one attempt, one check");
    let answers: Vec<String> = life.entries("answer").iter().map(said).collect();
    assert!(
        answers.iter().any(|a| a.contains("Listed it.")),
        "{answers:?}"
    );
    assert!(
        !answers.iter().any(|a| a.contains("first run's")),
        "the ended run's answer is never read: {answers:?}"
    );
    let fresh = spans(&life, "worker.fresh_run");
    assert_eq!(fresh.len(), 1);
    let (_, input, output) = &fresh[0];
    assert_eq!(input["context_tokens"], PAST);
    assert_eq!(input["threshold"], 200_000);
    assert_eq!(input["calls"], 10);
    assert_eq!(output["alarm"], false);
    let statuses: Vec<Value> = spans(&life, "worker")
        .iter()
        .filter(|(_, i, _)| i["role"] == "worker")
        .map(|(_, _, o)| o["status"].clone())
        .collect();
    assert_eq!(statuses, vec![json!("fresh_run"), json!("done")]);
    // `ferb trace` says it
    let tree = life
        .tasks()
        .into_iter()
        .find(|(id, _)| !id.contains('.'))
        .expect("a task")
        .0;
    let traced = life.call("trace.get", json!({"task_id": tree}));
    let story = ferb_trace::trace_output(&traced["result"], ferb_trace::Mode::Story);
    assert!(
        story.contains("ended for a fresh run at 250000 tokens"),
        "{story} {traced}"
    );
}

/// Nine calls past the size: the run goes on to its answer (at least 10 calls before a fresh run).
// decision: ORCH-760
#[test]
fn nine_calls_are_not_enough_for_a_fresh_run() {
    let tmp = root();
    let life = Life::start(
        tmp.path(),
        "01",
        Model::default()
            .planner(plan_task(GOAL, "On it."))
            .worker(long_run(&listings(9), PAST, "Listed it."))
            .checker(always(check_accept())),
    );
    let mut turn = ask(&life);
    done(&life);
    turn.end();
    assert_eq!(worker_runs(&life).len(), 1);
    assert!(spans(&life, "worker.fresh_run").is_empty());
}

/// While the run's write waits on FERB's safety question, the run reports past the size and is not ended; once the
/// owner answered and the write ran, the next report ends it for a fresh run.
// decision: ORCH-760
#[test]
fn no_fresh_run_while_a_question_is_open() {
    let tmp = root();
    let mut calls = listings(10);
    calls.push((
        "fs.write",
        json!({"path": "~/.zshrc", "content": "alias ll='ls -l'\n", "overwrite": true, "effect": "edit",
               "why": "add the alias"}),
    ));
    let mut first = long_run(&calls, PAST, "The first run's answer.");
    first["tools_background"] = json!(true);
    let usage = json!({"kind": "usage", "data": {"input_tokens": 10, "output_tokens": 5}});
    let final_ = first["events"][1].clone();
    first["events"] = json!([
        {"kind": "usage", "data": usage["data"], "hold": "asked"},
        {"kind": "usage", "data": usage["data"], "hold": "answered"},
        final_,
    ]);
    let life = Life::start(
        tmp.path(),
        "01",
        Model::default()
            .planner(plan_task(GOAL, "On it."))
            .worker(first)
            .worker(long_run(&[], PAST, "Listed it."))
            .checker(always(with_tools(
                check_accept(),
                &[("fs.read", json!({"path": "~/.zshrc"}))],
            ))),
    );
    let mut turn = ask(&life);
    life.wait("the question waits", 30, |l| !l.open_questions().is_empty());
    let reports = spans(&life, "model.usage").len();
    life.release("asked");
    life.wait("the run reported past the size", 30, |l| {
        spans(l, "model.usage").len() > reports
    });
    life.settle();
    assert!(
        spans(&life, "worker.fresh_run").is_empty(),
        "held while the question is open"
    );
    assert_eq!(worker_runs(&life).len(), 1);
    let asked = life.open_questions()[0]["approval_id"].clone();
    let yes = life.call(
        "chat.approve",
        json!({"approval_id": asked, "granted": true, "choice": 1}),
    );
    assert_eq!(yes["result"]["state"], "granted", "{yes}");
    life.wait("the write ran", 30, |l| {
        spans(l, "tool.call")
            .iter()
            .any(|(_, i, o)| i["tool"] == "fs.write" && !o.is_null())
    });
    life.settle();
    life.release("answered");
    done(&life);
    turn.end();
    assert_eq!(spans(&life, "worker.fresh_run").len(), 1);
    assert_eq!(worker_runs(&life).len(), 2);
}

/// Six fresh runs in one attempt: the sixth raises the alarm (journaled once, marked on its record), every note stays
/// within 1,500 tokens however many calls came before, and the attempt still ends with one check.
// decision: ORCH-760
#[test]
fn past_five_fresh_runs_in_one_attempt_raise_the_alarm() {
    let tmp = root();
    let mut model = Model::default().planner(plan_task(GOAL, "On it."));
    for _ in 0..6 {
        model = model.worker(long_run(&listings(10), PAST, "Not this one."));
    }
    let life = Life::start(
        tmp.path(),
        "01",
        model
            .worker(worker_done("Listed it."))
            .checker(always(check_accept())),
    );
    let mut turn = ask(&life);
    done(&life);
    turn.end();
    let runs = worker_runs(&life);
    assert_eq!(runs.len(), 7);
    for (i, parts) in runs.iter().enumerate().skip(1) {
        let text = note(parts).unwrap_or_else(|| panic!("run {i}'s note"));
        let tokens = ferb_context::tokens::count_tokens(&text);
        assert!(tokens <= 1_500, "run {i}: {tokens} tokens");
    }
    let alarms: Vec<Value> = spans(&life, "worker.fresh_run")
        .iter()
        .map(|(_, _, o)| o["alarm"].clone())
        .collect();
    assert_eq!(alarms, [vec![json!(false); 5], vec![json!(true)]].concat());
    let journaled = life.journal("worker.fresh_run_alarm");
    assert_eq!(journaled.len(), 1);
    assert_eq!(journaled[0]["fresh_runs"], 6);
    assert_eq!(life.calls_of("checker"), 1);
    // `ferb stats` names them
    let mut out = Vec::new();
    let now = ferbd::stats::Now {
        utc: ferb_py::time::DateTime::from_isoformat("2026-11-25T12:00:00+00:00").unwrap(),
        local: ferb_py::time::DateTime::from_isoformat("2026-11-25T17:30:00").unwrap(),
    };
    let range = Some(("2000-01-01".to_string(), "2100-12-31".to_string()));
    ferbd::stats::stats_in(&life.run.home, &mut out, 7, range, now);
    let stats = String::from_utf8(out).unwrap();
    assert!(stats.contains("6 fresh runs in 1 attempts"), "{stats}");
    assert!(
        stats.contains("past 5 in one attempt (the alarm): 1 attempts"),
        "{stats}"
    );
}

/// The switch off (FERB.app until a real replay): the same run goes on past the size to its own answer, as before.
// decision: ORCH-760
#[test]
fn with_the_switch_off_a_run_past_the_size_goes_on() {
    let tmp = root();
    let life = Life::start(
        tmp.path(),
        "01",
        Model::default()
            .script("fresh_run", json!(false))
            .planner(plan_task(GOAL, "On it."))
            .worker(long_run(&listings(10), PAST, "Listed it."))
            .checker(always(check_accept())),
    );
    let mut turn = ask(&life);
    done(&life);
    turn.end();
    assert_eq!(worker_runs(&life).len(), 1);
    assert!(spans(&life, "worker.fresh_run").is_empty());
    let answers: Vec<String> = life.entries("answer").iter().map(said).collect();
    assert!(
        answers.iter().any(|a| a.contains("Listed it.")),
        "{answers:?}"
    );
}
