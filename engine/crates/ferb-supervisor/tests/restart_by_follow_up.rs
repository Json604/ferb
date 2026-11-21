//! The owner's follow-up that starts a running task again (live 2026-11-21, tree 111d1ed8: "do it faster, maybe do all
//! that at the same time??" while one step searched; the planner answered a retry with three parallel steps, and FERB
//! told the owner "Stopped, as you asked." and ended the new attempt failed in 170 ms, never running a step). Scripted
//! on the recorded `stop_by_chat` scenario, through the real supervisor with stand-in models: the first message's
//! one-step task runs (its worker held), a tool call of that worker is recorded, then the follow-up's planner turn
//! answers a retry with a plan of two parallel steps and one that waits on both.

mod common;
mod support_control;

use ferb_orchestrator::answer_marks::STOPPED;
use ferb_supervisor::words::{RESTARTED, RESTARTED_SAID, RESTART_QUEUED, STOPPED_BY_YOU};
use serde_json::{json, Value};
use support_control::{run_with, world_with, Report, World};

const TREE: &str = "id-000021";
const FOLLOW_UP: &str = "id-000031";
const PLANNER_SAID: &str = "I'm starting again so both sources are checked at the same time.";
const SEEN_PAGE: &str = "https://portal.invalid/assignments";
const SEEN_SPAN: &str = "span-seen-before-the-restart";

fn step(id: &str, depends_on: &[&str]) -> Value {
    json!({"id": id, "kind": "do", "label": format!("Check {id}"), "goal": format!("Check source {id}"),
           "depends_on": depends_on, "inputs": [], "worker_type": "default", "conditions": [], "until": null,
           "event": null, "event_match": [], "question": "", "options": [], "conversation": null, "repeat": null,
           "earlier_results": []})
}

/// A follow-up's planner answer: `action` on the task, with a plan of two parallel steps and one that waits on both
/// (`plan` true), or the one-step task again (`plan` false).
fn follow_up(action: &str, new_goal: bool, plan: bool) -> String {
    follow_up_on(TREE, action, new_goal, plan)
}

fn follow_up_on(tree: &str, action: &str, new_goal: bool, plan: bool) -> String {
    let plan = if plan {
        json!({"steps": [step("a", &[]), step("b", &[]), step("c", &["a", "b"])], "input_paths": [],
               "big_job": false, "time_estimate": "", "cost_estimate": "", "standing_sends": []})
    } else {
        Value::Null
    };
    json!({"kind": "task", "answers_question": false, "task": {
        "goal": "Wait for the invented build to finish, checking both sources at the same time", "name": "Build watch",
        "identity": "owner", "worker_type": "default",
        "targets": {"apps": [], "sites": [], "paths": [], "recipients": []},
        "success_conditions": [{"text": "The build's state is reported", "owner_text": "", "layer": "outcome",
                                "check": null}],
        "plan": plan, "folders": [], "keep_tabs_open": false, "parts": [], "earlier_results": []},
        "control": {"task_id": tree, "action": action, "new_goal": new_goal, "standing_sends": "unchanged",
                    "supersedes": []},
        "side_entries": null, "text": PLANNER_SAID})
    .to_string()
}

/// The calls the three-step plan makes: the two parallel steps (each its worker, then its check), then the step
/// that waits on both.
fn plan_calls() -> Vec<Value> {
    vec![
        worker("Source a: the build is still running."),
        worker("Source b: the build is still running."),
        check(),
        check(),
        worker("The build is still running, by both sources."),
        check(),
    ]
}

fn call(key: &str, text: &str) -> Value {
    json!({"key": key, "events": [
        {"kind": "usage", "data": {"input_tokens": 10, "output_tokens": 5, "cost_usd": 0.001}},
        {"kind": "final", "data": {"text": text}}]})
}

fn worker(answer: &str) -> Value {
    call(
        "step",
        &json!({"status": "done", "reason": "", "question": null, "answer": answer, "artifacts": [], "how": ""})
            .to_string(),
    )
}

fn check() -> Value {
    call(
        "step.verify",
        &json!({"results": [{"index": 0, "met": true, "applies": true,
                "evidence": "Seen in the worker's report.", "how": null}], "next": "accept", "findings": [],
                "missing": "", "instructions": "", "worker_type": null, "question": "", "reason": ""})
        .to_string(),
    )
}

/// The running one-step task (its worker held, one call of it recorded), then the follow-up the planner answers
/// with `answer`; the model calls after it are `then`.
fn while_running(answer: String, then: Vec<Value>) -> (World, Report) {
    let name = "stop_by_chat";
    let mut w = world_with(name, |calls| {
        let planner = calls
            .iter_mut()
            .filter(|c| c["key"] == "supervisor")
            .nth(1)
            .unwrap();
        planner["events"] = call("supervisor", &answer)["events"].clone();
        calls.extend(then);
    });
    let report = run_with(&mut w, name, |steps| {
        // a call the held worker made before the follow-up: what it had read
        let at = steps.iter().position(|st| st["do"] == "wait").unwrap();
        steps.insert(at + 1, seen_call());
    });
    eprintln!("scripted: {}", report.verdict());
    (w, report)
}

fn seen_call() -> Value {
    json!({"do": "plan_event", "sql": format!(
        "INSERT INTO spans (id, parent_id, task_id, kind, started_at, ended_at, input_json, output_json) \
         VALUES ('{SEEN_SPAN}', NULL, '{TREE}.worker', 'tool.call', '2026-01-01T09:00:00+00:00', \
         '2026-01-01T09:00:00+00:00', '{{\"tool\": \"browser.navigate\", \"target\": {{\"page\": \"{SEEN_PAGE}\"}}, \
         \"why\": \"open the assignments page\"}}', '{{\"ok\": true}}')")})
}

/// The live case: the planner's retry with a new plan of parallel steps.
fn restarted() -> (World, Report) {
    while_running(follow_up("retry", false, true), plan_calls())
}

/// The owner stopped the task (the recorded "stop that"); then a third message the planner answers with `answer`,
/// the model calls after it being `then`.
fn after_the_stop(answer: String, then: Vec<Value>) -> (World, Report) {
    let name = "stop_by_chat";
    let mut w = world_with(name, |calls| {
        calls.push(call("supervisor", &answer));
        calls.extend(then);
    });
    {
        // the third planner turn reads what the second one read
        let mut q = w.daemon.supervise.lock().unwrap();
        let last = q.last().cloned().unwrap();
        q.push(last);
    }
    let report = run_with(&mut w, name, |steps| {
        let at = steps.iter().position(|st| st["do"] == "wait").unwrap();
        steps.insert(at + 1, seen_call());
        steps.extend([
            json!({"do": "connect", "conn": "third"}),
            json!({"do": "stream", "conn": "third", "lines": [{"send": json!({"jsonrpc": "2.0", "id": 3,
                "method": "chat.send", "params": {"session_id": "s1", "text": "check both at the same time"}})
                .to_string()}]}),
            json!({"do": "stream_end", "conn": "third", "lines": []}),
        ]);
    });
    eprintln!("scripted: {}", report.verdict());
    (w, report)
}

fn stream(r: &Report, conn: &str) -> Vec<Value> {
    r.streams
        .iter()
        .find(|(c, _)| c == conn)
        .map(|(_, lines)| lines.as_slice())
        .unwrap_or_else(|| panic!("no {conn} stream: {:?}", r.verdict()))
        .iter()
        .map(|l| serde_json::from_str::<Value>(l).unwrap()["params"].clone())
        .collect()
}

fn of_type(frames: &[Value], ty: &str) -> Vec<Value> {
    frames.iter().filter(|f| f["type"] == ty).cloned().collect()
}

fn journal(w: &World, kind: &str) -> Vec<Value> {
    let db = w.db.lock().unwrap();
    let mut st = db
        .conn()
        .prepare("SELECT data_json FROM journal WHERE kind = ? ORDER BY seq")
        .unwrap();
    let rows: Vec<Value> = st
        .query_map([kind], |r| r.get::<_, String>(0))
        .unwrap()
        .map(|r| serde_json::from_str(&r.unwrap()).unwrap())
        .collect();
    rows
}

fn plan_steps(w: &World) -> Vec<(String, String, Value)> {
    let db = w.db.lock().unwrap();
    let mut st = db
        .conn()
        .prepare("SELECT step_id, status, output FROM plan_steps WHERE task_id = ? ORDER BY rowid")
        .unwrap();
    let rows: Vec<(String, String, Value)> = st
        .query_map([TREE], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        })
        .unwrap()
        .map(|r| {
            let (a, b, c) = r.unwrap();
            (a, b, serde_json::from_str(&c).unwrap())
        })
        .collect();
    rows
}

/// The follow-up re-plans the running task into parallel steps; the old worker is stopped with the restart's reason,
/// not the owner's stop; the new steps run (a and b before c) and the task's end is the new plan's.
// decision: ORCH-682, ORCH-683
#[test]
fn a_follow_up_restart_runs_the_new_plan_and_ends_with_its_outcome() {
    let (w, _) = restarted();
    let started: Vec<String> = w
        .daemon
        .calls
        .lock()
        .unwrap()
        .started
        .iter()
        .map(|c| c["task_id"].as_str().unwrap_or("").to_string())
        .collect();
    let at = |id: &str| {
        started
            .iter()
            .position(|t| t == id)
            .unwrap_or_else(|| panic!("{id} never ran: {started:?}"))
    };
    let (a, b, c) = (
        at(&format!("{TREE}.step.a")),
        at(&format!("{TREE}.step.b")),
        at(&format!("{TREE}.step.c")),
    );
    assert!(
        a < c && b < c,
        "the parallel steps run before the one that waits on both: {started:?}"
    );
    let outcomes = journal(&w, "task.outcome");
    assert_eq!(
        outcomes.len(),
        2,
        "the stopped run and the new one: {outcomes:?}"
    );
    assert_eq!(outcomes[0]["status"], "cancelled");
    assert_eq!(
        outcomes[0]["summary"], RESTARTED,
        "the old run ends with the restart's own reason"
    );
    assert_eq!(
        outcomes[1]["status"], "done",
        "the task's end is the new plan's: {:?}",
        outcomes[1]
    );
    let subtasks = outcomes[1]["subtasks"].to_string();
    assert!(
        !subtasks.contains("stopped"),
        "the new run's outcome counts no cancelled step: {subtasks}"
    );
    let steps = plan_steps(&w);
    for id in ["a", "b", "c"] {
        assert!(
            steps.iter().any(|(k, s, _)| k == id && s == "done"),
            "{id} done: {steps:?}"
        );
    }
    let stored: String =
        w.db.lock()
            .unwrap()
            .conn()
            .query_row(
                "SELECT plan FROM task_plans WHERE task_id = ?",
                [TREE],
                |r| r.get(0),
            )
            .unwrap();
    assert!(
        stored.contains("\"id\": \"a\"") && !stored.contains("\"id\": \"worker\""),
        "the stored plan is the new one: {stored}"
    );
}

/// The owner never sees "Stopped, as you asked." for a restart: the first message's turn ends with the restart's own
/// line, filed on that turn in place (told_in), and the follow-up's turn carries the plan line and the task's end.
// decision: ORCH-682, ORCH-683
#[test]
fn a_follow_up_restart_is_never_told_as_the_owners_stop() {
    let (w, r) = restarted();
    let first = stream(&r, "chat");
    let follow = stream(&r, "app");
    let everything: String = first
        .iter()
        .chain(follow.iter())
        .map(Value::to_string)
        .collect();
    assert!(
        !everything.contains(STOPPED) && !everything.contains(STOPPED_BY_YOU),
        "{everything}"
    );
    let old_end = of_type(&first, "outcome");
    assert_eq!(old_end.len(), 1, "{first:?}");
    assert_eq!(old_end[0]["answer"], RESTARTED_SAID);
    assert_eq!(old_end[0]["told_in"], FOLLOW_UP);
    let plans = of_type(&follow, "plan");
    assert_eq!(
        plans.len(),
        1,
        "one plan line in the follow-up's turn: {follow:?}"
    );
    assert_eq!(plans[0]["text"], PLANNER_SAID);
    assert!(
        of_type(&follow, "reply").is_empty(),
        "no separate reply beside the plan line: {follow:?}"
    );
    let end = of_type(&follow, "outcome");
    assert_eq!(end.len(), 1, "{follow:?}");
    assert_eq!(end[0]["status"], "done", "{:?}", end[0]);
    let answers = journal(&w, "task.answer");
    assert!(
        answers
            .iter()
            .all(|a| !a["text"].as_str().unwrap_or("").contains(STOPPED)),
        "{answers:?}"
    );
}

/// What the stopped worker had read reaches every new step as data: its call, with the span id to open it in full.
// decision: ORCH-683
#[test]
fn a_follow_up_restart_hands_the_stopped_runs_calls_to_the_new_steps() {
    let (w, _) = restarted();
    let steps = plan_steps(&w);
    let kept = steps
        .iter()
        .find(|(k, _, _)| k == "earlier plan: worker")
        .unwrap_or_else(|| panic!("the stopped step is kept as an earlier result: {steps:?}"));
    assert_eq!(kept.1, "cancelled");
    let calls = kept.2["stopped_run"]["calls"].to_string();
    assert!(
        calls.contains(SEEN_SPAN) && calls.contains(SEEN_PAGE),
        "{}",
        kept.2
    );
    let started = w.daemon.calls.lock().unwrap().started.clone();
    for id in ["a", "b"] {
        let call = started
            .iter()
            .find(|c| c["task_id"] == format!("{TREE}.step.{id}"))
            .unwrap();
        let parts = call["parts"].to_string();
        assert!(
            parts.contains("earlier_plan_results")
                && parts.contains(SEEN_SPAN)
                && parts.contains(SEEN_PAGE),
            "step {id} gets the stopped run's call: {parts}"
        );
    }
}

/// The task's outcomes in order (the journal's `task.outcome`), and each of the new plan's steps done.
fn ended_with_the_new_plan(w: &World, first: (&str, &str)) {
    let outcomes = journal(w, "task.outcome");
    assert_eq!(outcomes.len(), 2, "{outcomes:?}");
    assert_eq!(
        (
            outcomes[0]["status"].as_str().unwrap(),
            outcomes[0]["summary"].as_str().unwrap()
        ),
        first,
        "{:?}",
        outcomes[0]
    );
    assert_eq!(outcomes[1]["status"], "done", "{:?}", outcomes[1]);
    let steps = plan_steps(w);
    for id in ["a", "b", "c"] {
        assert!(
            steps.iter().any(|(k, s, _)| k == id && s == "done"),
            "{id} done: {steps:?}"
        );
    }
}

/// A correction (continue) whose plan has a different step graph than the running one is the planner's retry: the
/// task starts again with the new steps, through the same path, never "It keeps its current plan" with the added
/// work left undone.
// decision: ORCH-683
#[test]
fn a_correction_with_new_steps_starts_the_task_again_with_them() {
    let (w, r) = while_running(follow_up("continue", false, true), plan_calls());
    ended_with_the_new_plan(&w, ("cancelled", RESTARTED));
    let said: String = stream(&r, "app").iter().map(Value::to_string).collect();
    assert!(
        !said.contains(STOPPED) && !said.contains("keeps its current plan"),
        "{said}"
    );
}

/// A fresh attempt of the running one-step task without a new plan: its run is stopped for the restart, the one step
/// runs again with what the stopped run had read, and the task ends with the new run's outcome.
// decision: ORCH-683
#[test]
fn a_fresh_attempt_of_a_running_one_step_task_carries_what_it_read() {
    let (w, _) = while_running(
        follow_up("retry", false, false),
        vec![worker("The build is still running."), check()],
    );
    let outcomes = journal(&w, "task.outcome");
    assert_eq!(outcomes.len(), 2, "{outcomes:?}");
    assert_eq!(outcomes[0]["summary"], RESTARTED);
    assert_eq!(outcomes[1]["status"], "done", "{:?}", outcomes[1]);
    let started = w.daemon.calls.lock().unwrap().started.clone();
    let runs: Vec<&Value> = started
        .iter()
        .filter(|c| c["task_id"] == format!("{TREE}.worker"))
        .collect();
    assert_eq!(runs.len(), 2, "the one step ran again: {started:?}");
    let parts = runs[1]["parts"].to_string();
    assert!(
        parts.contains(SEEN_SPAN) && parts.contains(SEEN_PAGE),
        "the new run gets the stopped run's call: {parts}"
    );
}

/// A follow-up to a task the owner stopped, re-planned as a rewrite (continue, new goal) into steps where the task had
/// one: the new steps run (the stored one-step plan was kept before, and the new run failed at once on its cancelled
/// record).
// decision: ORCH-683
#[test]
fn a_rewrite_of_a_stopped_one_step_task_runs_its_new_steps() {
    let (w, _) = after_the_stop(follow_up("continue", true, true), plan_calls());
    ended_with_the_new_plan(&w, ("cancelled", STOPPED_BY_YOU));
}

/// The same with the planner's retry of the stopped task.
// decision: ORCH-683
#[test]
fn a_retry_of_a_stopped_one_step_task_with_new_steps_runs_them() {
    let (w, _) = after_the_stop(follow_up("retry", false, true), plan_calls());
    ended_with_the_new_plan(&w, ("cancelled", STOPPED_BY_YOU));
}

/// A correction (continue, same result) of the stopped task that splits it into steps: the new steps run, not the
/// saved one-step plan again.
// decision: ORCH-683
#[test]
fn a_correction_of_a_stopped_task_into_steps_runs_them() {
    let (w, _) = after_the_stop(follow_up("continue", false, true), plan_calls());
    ended_with_the_new_plan(&w, ("cancelled", STOPPED_BY_YOU));
}

// ---- a restart while a step waits on a question (ORCH-684) ----

const ASKED: &str = "ap-a00000000001";

/// The text of a recorded call's final answer, parsed.
fn final_of(call: &Value) -> Value {
    let text = call["events"]
        .as_array()
        .unwrap()
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

/// The scenario `name`, its calls changed by `edit` and its steps by `steps`, run until a question of the task waits;
/// then the owner's follow-up, which the planner answers with a retry and the three-step plan.
fn retry_while_asked(
    name: &str,
    edit: impl FnOnce(&mut Vec<Value>),
    steps: impl FnOnce(&mut Vec<Value>),
) -> (World, Report) {
    let mut calls = vec![call("supervisor", &follow_up("retry", false, true))];
    calls.extend(plan_calls());
    follow_up_while_asked(name, edit, steps, calls)
}

/// The same with the follow-up's planner answer and the calls after it given (`calls`).
fn follow_up_while_asked(
    name: &str,
    edit: impl FnOnce(&mut Vec<Value>),
    steps: impl FnOnce(&mut Vec<Value>),
    calls: Vec<Value>,
) -> (World, Report) {
    let mut w = world_with(name, edit);
    {
        let mut q = w.daemon.supervise.lock().unwrap();
        let last = q.last().cloned().unwrap();
        q.push(last);
    }
    let report = run_with(&mut w, name, |all| {
        steps(all);
        let at = all
            .iter()
            .position(|st| st["do"] == "wait" && st["until"] == "an approval is pending")
            .expect("a question that waits");
        let conn = all
            .iter()
            .find(|st| st["do"] == "stream")
            .map(|st| st["conn"].clone())
            .unwrap();
        all.truncate(at + 1);
        all.extend([
            json!({"do": "calls_from_here", "calls": calls}),
            json!({"do": "connect", "conn": "follow"}),
            json!({"do": "stream", "conn": "follow", "lines": [{"send": json!({"jsonrpc": "2.0", "id": 7,
                "method": "chat.send", "params": {"session_id": "s1", "text": "do it differently, both at once"}})
                .to_string()}]}),
            json!({"do": "stream_end", "conn": "follow", "lines": []}),
            json!({"do": "stream_end", "conn": conn, "lines": []}),
        ]);
    });
    eprintln!("{name}: {}", report.verdict());
    (w, report)
}

/// The question was set aside, never read as the owner's No; the old run ended for the restart and the new plan ran;
/// no stream says the owner stopped it.
fn restarted_past_the_question(w: &World, r: &Report) {
    let published = w.daemon.published.lock().unwrap().clone();
    let resolved: Vec<&Value> = published
        .iter()
        .filter(|(t, d)| t == "approval.resolved" && d["approval_id"] == ASKED)
        .map(|(_, d)| d)
        .collect();
    assert!(
        !resolved.is_empty() && resolved.iter().all(|d| d["state"] == "superseded"),
        "the question is set aside, never a No: {resolved:?}"
    );
    for kind in [
        "plan.after_gaps",
        "task.rabbit_hole_answered",
        "task.check_in_answered",
    ] {
        let read = journal(w, kind);
        assert!(
            read.iter().all(|r| r["answer"] == "stopped"),
            "{kind}: no owner's answer was read from the restart: {read:?}"
        );
    }
    let records = format!("{:?}{:?}", journal(w, "task.outcome"), plan_steps(w));
    assert!(
        !records.contains("you said not to go on"),
        "a stop is never told as the owner's No: {records}"
    );
    let outcomes = journal(w, "task.outcome");
    assert_eq!(outcomes.len(), 2, "{outcomes:?}");
    assert_eq!(outcomes[0]["status"], "cancelled", "{:?}", outcomes[0]);
    assert_eq!(outcomes[0]["summary"], RESTARTED);
    assert_eq!(outcomes[1]["status"], "done", "{:?}", outcomes[1]);
    let steps = plan_steps(w);
    for id in ["a", "b", "c"] {
        assert!(
            steps.iter().any(|(k, s, _)| k == id && s == "done"),
            "{id} done: {steps:?}"
        );
    }
    let said: String = r
        .streams
        .iter()
        .flat_map(|(_, lines)| lines.iter().cloned())
        .collect();
    assert!(
        !said.contains(STOPPED) && !said.contains(STOPPED_BY_YOU),
        "{said}"
    );
}

/// A worker's own question (owner.ask) waits; the owner's follow-up re-plans.
// decision: ORCH-683, ORCH-684
#[test]
fn a_restart_while_a_workers_question_waits() {
    let (w, r) = retry_while_asked("worker_question_answered", |_| {}, |_| {});
    restarted_past_the_question(&w, &r);
}

/// FERB's check-in after five attempts waits.
// decision: ORCH-683, ORCH-684
#[test]
fn a_restart_while_the_check_in_waits() {
    let (w, r) = retry_while_asked("check_in_after_five", |_| {}, |_| {});
    restarted_past_the_question(&w, &r);
}

/// "Keep trying, or go on with what I have?" waits (ORCH-679: a search left after a retry).
// decision: ORCH-683, ORCH-684, ORCH-679
#[test]
fn a_restart_while_keep_trying_waits() {
    let (w, r) = retry_while_asked(
        "check_in_after_five",
        |calls| {
            for c in calls.iter_mut().filter(|c| c["key"] == "worker.verify") {
                answered(
                    c,
                    json!({"results": [{"index": 0, "met": null, "applies": true,
                            "evidence": "Seen in the worker's report.", "how": "Not visible from here.",
                            "left": {"kind": "search", "reached": "some"}}],
                        "next": "retry", "findings": [], "missing": "The table isn't booked.",
                        "instructions": "Try another booking site.", "worker_type": null, "question": "",
                        "reason": "", "gaps": [],
                        "progress": {"found": "Two places were found.", "missing": "No free table yet."}}),
                );
            }
        },
        |_| {},
    );
    assert_eq!(
        journal(&w, "task.rabbit_hole_asked").len(),
        1,
        "the keep-trying question was the one waiting"
    );
    restarted_past_the_question(&w, &r);
}

/// A plan's own ask step waits.
// decision: ORCH-683, ORCH-684
#[test]
fn a_restart_while_a_plans_ask_step_waits() {
    let (w, r) = retry_while_asked(
        "stop_while_step_asks",
        |calls| {
            let planner = calls.iter_mut().find(|c| c["key"] == "supervisor").unwrap();
            let mut plan = final_of(planner);
            let mut ask = step("choose", &[]);
            ask["kind"] = json!("ask");
            ask["question"] = json!("Which one?");
            ask["options"] = json!(["One", "Two"]);
            plan["task"]["plan"]["steps"] = json!([ask, step("after", &["choose"])]);
            answered(planner, plan);
        },
        |_| {},
    );
    restarted_past_the_question(&w, &r);
}

/// multi_step_plan with its first step delivered with a gap and a submit step after it (ORCH-653's question).
fn gapped_then_submit(calls: &mut [Value]) {
    {
        let mut plan = final_of(&calls[0]);
        plan["task"]["success_conditions"][0]["check"] = json!({"kind": "submitted", "path": null,
                "text": null, "url_contains": null, "site": "forms.example", "expected": null, "tool": null});
        plan["task"]["plan"]["steps"][0]["conditions"] = json!([
            "Three temperatures are collected",
            "Each temperature comes from the city's official weather office"
        ]);
        answered(&mut calls[0], plan);
        let v = calls
            .iter()
            .position(|c| c["key"] == "step.collect.verify")
            .unwrap();
        answered(
            &mut calls[v],
            json!({"results": [
                        {"index": 0, "met": true, "applies": true, "evidence": "Three were read.", "how": null},
                        {"index": 1, "met": null, "applies": true, "evidence": "The cities are invented.",
                         "how": null}],
                    "next": "accept_with_gaps", "findings": [], "missing": "", "instructions": "",
                    "worker_type": null, "question": "", "reason": "",
                    "gaps": [{"index": 1, "reason": "the cities are invented, so no office exists"}]}),
        );
    }
}

fn ask_after_stream(steps: &mut Vec<Value>) {
    let at = steps.iter().position(|s| s["do"] == "stream").unwrap();
    steps.insert(
        at + 1,
        json!({"do": "wait", "until": "an approval is pending"}),
    );
}

/// The after-gaps question (ORCH-653) waits before a submit step.
// decision: ORCH-683, ORCH-684, ORCH-653
#[test]
fn a_restart_while_the_after_gaps_question_waits() {
    let (w, r) = retry_while_asked(
        "multi_step_plan",
        |c| gapped_then_submit(c),
        ask_after_stream,
    );
    restarted_past_the_question(&w, &r);
}

/// The owner's stop while the after-gaps question waits: the step not started is the stop's, never "you said not to
/// go on" (a later continue's worker reads that step's record as its previous attempt).
// decision: ORCH-684, ORCH-653
#[test]
fn a_stop_while_the_after_gaps_question_waits_is_never_his_no() {
    let stop = json!({"kind": "reply", "answers_question": false, "task": null,
        "control": {"task_id": TREE, "action": "stop", "new_goal": false, "standing_sends": "unchanged",
                    "supersedes": []},
        "side_entries": null, "text": ""})
    .to_string();
    let (w, _) = follow_up_while_asked(
        "multi_step_plan",
        |c| gapped_then_submit(c),
        ask_after_stream,
        vec![call("supervisor", &stop)],
    );
    let steps = plan_steps(&w);
    let write = steps
        .iter()
        .find(|(k, _, _)| k == "write")
        .unwrap_or_else(|| panic!("{steps:?}"));
    assert_eq!(write.1, "cancelled", "{steps:?}");
    assert_eq!(write.2["summary"], STOPPED_BY_YOU, "{steps:?}");
}

// ---- two follow-ups inside a stop's settling window (ORCH-684) ----

/// The first follow-up restarts the running task, but its stopped run's tool call is still in flight past the 10 s
/// the retry waits, so it says it will start again and lets the chat go; a second follow-up (a correction) arrives
/// then. One resume per tree: the second says it waits, then steers the new run; the task runs once more, not twice.
// decision: ORCH-684
#[test]
fn a_second_follow_up_during_a_restart_waits_for_it_and_the_tree_resumes_once() {
    let name = "stop_by_chat";
    let correction = json!({"kind": "task", "answers_question": false, "task": {
        "goal": "Wait for the invented build to finish, checking both sources at the same time", "name": "Build watch",
        "identity": "owner", "worker_type": "default",
        "targets": {"apps": [], "sites": [], "paths": [], "recipients": []},
        "success_conditions": [{"text": "The build's state is reported", "owner_text": "", "layer": "outcome",
                                "check": null}],
        "plan": null, "folders": [], "keep_tabs_open": false, "parts": [], "earlier_results": []},
        "control": {"task_id": TREE, "action": "continue", "new_goal": false, "standing_sends": "unchanged",
                    "supersedes": []},
        "side_entries": null, "text": "I'll keep the build's last state in the answer too."})
    .to_string();
    let mut w = world_with(name, |calls| {
        let planner = calls
            .iter_mut()
            .filter(|c| c["key"] == "supervisor")
            .nth(1)
            .unwrap();
        planner["events"] = call("supervisor", &follow_up("retry", false, true))["events"].clone();
        calls.push(call("supervisor", &correction));
        calls.extend(plan_calls());
    });
    {
        let mut q = w.daemon.supervise.lock().unwrap();
        let last = q.last().cloned().unwrap();
        q.push(last);
    }
    let r = run_with(&mut w, name, |steps| {
        let at = steps.iter().position(|st| st["do"] == "wait").unwrap();
        steps.insert(at + 1, json!({"do": "in_flight", "on": true}));
        let at = steps
            .iter()
            .position(|st| st["do"] == "stream" && st["conn"] == "app")
            .unwrap();
        let tail: Vec<Value> = steps.split_off(at + 1);
        steps.extend([
            json!({"do": "wait", "until": "model calls made: 3"}),
            // past the 10 s the retry waits for its stop: it has said it will start again and let the chat go
            json!({"do": "sleep", "s": 15}),
            json!({"do": "connect", "conn": "second"}),
            json!({"do": "stream", "conn": "second", "lines": [{"send": json!({"jsonrpc": "2.0", "id": 9,
                "method": "chat.send", "params": {"session_id": "s1", "text": "and keep its last state"}})
                .to_string()}]}),
            json!({"do": "wait", "until": "model calls made: 4"}),
            json!({"do": "in_flight", "on": false}),
            json!({"do": "stream_end", "conn": "second", "lines": []}),
        ]);
        steps.extend(tail.into_iter().filter(|st| st["do"] != "close"));
    });
    eprintln!("scripted: {}", r.verdict());
    let runs: i64 =
        w.db.lock()
            .unwrap()
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM spans WHERE kind = 'task' AND task_id = ?",
                [TREE],
                |r| r.get(0),
            )
            .unwrap();
    assert_eq!(
        runs,
        2,
        "the first run and the one restart, never a second resume of the same tree: {:?} {:?}",
        journal(&w, "task.outcome"),
        r.streams
    );
    let second = stream(&r, "second");
    let replies: Vec<String> = of_type(&second, "reply")
        .iter()
        .map(|f| f["text"].as_str().unwrap_or("").to_string())
        .collect();
    assert_eq!(
        replies.first().map(String::as_str),
        Some(RESTART_QUEUED),
        "the second follow-up says it waits for the restart: {second:?}"
    );
    let outcomes = journal(&w, "task.outcome");
    assert_eq!(
        outcomes.last().map(|o| o["status"].clone()),
        Some(json!("done")),
        "{outcomes:?}"
    );
    let steps = plan_steps(&w);
    for id in ["a", "b", "c"] {
        assert!(
            steps.iter().any(|(k, s, _)| k == id && s == "done"),
            "{id} done: {steps:?}"
        );
    }
}

// ---- a scheduled run restarted by the owner's follow-up (ORCH-685) ----

/// A schedule's run is under way (its worker held); the owner's follow-up re-plans it. The old run returns to the
/// daemon marked restarted (neither counted nor posted); the new run keeps the schedule (its facts reach the new
/// steps) and its end goes through the schedule's bookkeeping, not as a plain chat result.
// decision: ORCH-685
#[test]
fn a_restarted_scheduled_run_stays_its_schedules_run() {
    let name = "stop_by_chat";
    let mut w = world_with(name, |calls| {
        calls.truncate(2);
    });
    let sched_step = json!({"do": "run_scheduled", "id": "sch-1", "request": "Wait for the invented build to finish",
                            "name": "Build watch"});
    let rule = support_control::test_schedule(&sched_step).when_said();
    let mut calls = vec![call(
        "supervisor",
        &follow_up_on("@TREE@", "retry", false, true),
    )];
    calls.extend(plan_calls());
    let r = run_with(&mut w, name, |steps| {
        steps.clear();
        steps.extend([
            sched_step.clone(),
            json!({"do": "wait", "until": "model calls made: 2"}),
            json!({"do": "calls_from_here", "calls": calls}),
            json!({"do": "connect", "conn": "app"}),
            json!({"do": "stream", "conn": "app", "lines": [{"send": json!({"jsonrpc": "2.0", "id": 2,
                "method": "chat.send", "params": {"session_id": "s1", "text": "check both at the same time"}})
                .to_string()}]}),
            json!({"do": "stream_end", "conn": "app", "lines": []}),
            json!({"do": "wait", "until": "the scheduled run ended"}),
        ]);
    });
    eprintln!("scripted: {}", r.verdict());
    let old = w.daemon.scheduled.lock().unwrap().clone();
    assert_eq!(old.len(), 1, "{old:?}");
    assert_eq!(
        old[0]["restarted"], true,
        "the old run is neither counted nor posted: {:?}",
        old[0]
    );
    let resumed = w.daemon.resumed.lock().unwrap().clone();
    assert_eq!(
        resumed.len(),
        1,
        "the new run's end goes to its schedule: {resumed:?}"
    );
    assert_eq!(resumed[0]["schedule"]["id"], "sch-1");
    assert_eq!(resumed[0]["status"], "done", "{:?}", resumed[0]);
    let follow = stream(&r, "app");
    assert!(
        of_type(&follow, "outcome").is_empty(),
        "its end is the schedule's post, not a second chat result: {follow:?}"
    );
    let tree = resumed[0]["task_id"].as_str().unwrap().to_string();
    let started = w.daemon.calls.lock().unwrap().started.clone();
    let step_a = started
        .iter()
        .find(|c| c["task_id"] == format!("{tree}.step.a"))
        .unwrap_or_else(|| panic!("step a ran: {started:?}"));
    assert!(
        step_a["parts"].to_string().contains(&rule),
        "the schedule's facts reach the new steps"
    );
}

// ---- a restarted scheduled run keeps the schedule's standing permissions (ORCH-686) ----

/// The scheduled run's standing send (its schedule's grant) and recorded scope hold for the run the owner's follow-up
/// started again: the grant is never revoked between the two runs, the new steps run with it, and the schedule's
/// recorded identity is applied again; it ends with the task.
// decision: ORCH-686
#[test]
fn a_restarted_scheduled_run_keeps_its_schedules_standing_send_and_scope() {
    let name = "stop_by_chat";
    let mut w = world_with(name, |calls| {
        calls.truncate(2);
    });
    let sched_step = json!({"do": "run_scheduled", "id": "sch-1", "request": "Wait for the invented build to finish",
        "name": "Build watch", "targets": {
            "standing_sends": [{"capability": "email.send", "identity": "owner", "account": null,
                                "site": "mail.example", "recipients": ["team@example.invalid"], "content": null}],
            "run_scope": {"folders": [], "identity": "ferb", "worker_type": "default"}}});
    let mut calls = vec![call(
        "supervisor",
        &follow_up_on("@TREE@", "retry", false, true),
    )];
    calls.extend(plan_calls());
    let _ = run_with(&mut w, name, |steps| {
        steps.clear();
        steps.extend([
            sched_step.clone(),
            json!({"do": "wait", "until": "model calls made: 2"}),
            json!({"do": "calls_from_here", "calls": calls}),
            json!({"do": "connect", "conn": "app"}),
            json!({"do": "stream", "conn": "app", "lines": [{"send": json!({"jsonrpc": "2.0", "id": 2,
                "method": "chat.send", "params": {"session_id": "s1", "text": "check both at the same time"}})
                .to_string()}]}),
            json!({"do": "stream_end", "conn": "app", "lines": []}),
            json!({"do": "wait", "until": "the scheduled run ended"}),
        ]);
    });
    let resumed = w.daemon.resumed.lock().unwrap().clone();
    assert_eq!(resumed.len(), 1, "{resumed:?}");
    let tree = resumed[0]["task_id"].as_str().unwrap().to_string();
    let log = w.daemon.standing.lock().unwrap().1.clone();
    assert_eq!(
        log,
        vec![
            format!("grant {tree} schedule:sch-1"),
            format!("revoke {tree}")
        ],
        "granted once for the schedule's run, kept across the restart, ended with the task"
    );
    let started = w.daemon.calls.lock().unwrap().started.clone();
    for id in ["a", "b", "c"] {
        let c = started
            .iter()
            .find(|c| c["task_id"] == format!("{tree}.step.{id}"))
            .unwrap_or_else(|| panic!("{id} ran: {started:?}"));
        assert_eq!(
            c["standing"], true,
            "step {id} runs with the schedule's grant"
        );
    }
    let applied = journal(&w, "schedule.scope_applied");
    assert_eq!(
        applied.len(),
        2,
        "the recorded scope holds for the run and its restart: {applied:?}"
    );
    assert!(applied.iter().all(|a| a["recorded"]["identity"] == "ferb"));
}

// ---- the owner's stop while a restart waits (ORCH-684) ----

fn stop_answer() -> String {
    json!({"kind": "reply", "answers_question": false, "task": null,
        "control": {"task_id": TREE, "action": "stop", "new_goal": false, "standing_sends": "unchanged",
                    "supersedes": []},
        "side_entries": null, "text": ""})
    .to_string()
}

fn task_stop_call() -> Value {
    json!({"do": "call", "conn": "app", "lines": [{"send": json!({"jsonrpc": "2.0", "id": 50,
        "method": "task.stop", "params": {"task_ids": [TREE]}}).to_string()}]})
}

/// The running one-step task, its stopped run's tool call held in flight, then the follow-up's retry; `then` runs
/// once that retry is under way (its model call made), after `wait_s` seconds; the call is let go at the end.
fn stop_during_restart(wait_s: f64, then: Vec<Value>, extra_calls: Vec<Value>) -> (World, Report) {
    let name = "stop_by_chat";
    let mut w = world_with(name, |calls| {
        let planner = calls
            .iter_mut()
            .filter(|c| c["key"] == "supervisor")
            .nth(1)
            .unwrap();
        planner["events"] = call("supervisor", &follow_up("retry", false, true))["events"].clone();
        calls.extend(extra_calls);
        calls.extend(plan_calls());
    });
    {
        let mut q = w.daemon.supervise.lock().unwrap();
        let last = q.last().cloned().unwrap();
        q.push(last);
    }
    let r = run_with(&mut w, name, |steps| {
        let at = steps.iter().position(|st| st["do"] == "wait").unwrap();
        steps.insert(at + 1, json!({"do": "in_flight", "on": true}));
        let at = steps
            .iter()
            .position(|st| st["do"] == "stream" && st["conn"] == "app")
            .unwrap();
        let tail: Vec<Value> = steps.split_off(at + 1);
        steps.push(json!({"do": "wait", "until": "model calls made: 3"}));
        steps.push(json!({"do": "sleep", "s": wait_s}));
        steps.extend(then);
        steps.push(json!({"do": "in_flight", "on": false}));
        steps.extend(tail.into_iter().filter(|st| st["do"] != "close"));
    });
    eprintln!("scripted: {}", r.verdict());
    (w, r)
}

fn replies(frames: &[Value]) -> Vec<String> {
    frames
        .iter()
        .filter(|f| f["type"] == "reply" || f["type"] == "outcome")
        .map(|f| {
            f["answer"]
                .as_str()
                .or(f["text"].as_str())
                .unwrap_or("")
                .to_string()
        })
        .collect()
}

/// Nothing ran again, nothing claims it did, and the chat's last word is the owner's stop.
fn stopped_not_restarted(w: &World, r: &Report) {
    let runs: i64 =
        w.db.lock()
            .unwrap()
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM spans WHERE kind = 'task' AND task_id = ?",
                [TREE],
                |r| r.get(0),
            )
            .unwrap();
    assert_eq!(runs, 1, "the restart was called off");
    let said: String = r
        .streams
        .iter()
        .flat_map(|(_, l)| l.iter().cloned())
        .collect();
    assert!(
        !said.contains("carries on")
            && !said.contains("\"type\": \"plan\", \"text\": \"I'm starting"),
        "nothing claims the task went on: {said}"
    );
}

/// The Stop button inside the retry's first wait (10 s) for its stopped run, whose tool call is still in flight: the
/// retry never says it will start again; its turn says only the stop, and the first turn's line stays true.
// decision: ORCH-684
#[test]
fn the_stop_button_inside_the_restarts_first_wait() {
    let (w, r) = stop_during_restart(1.0, vec![task_stop_call()], vec![]);
    stopped_not_restarted(&w, &r);
    let first = replies(&stream(&r, "chat"));
    assert_eq!(first.last().map(String::as_str), Some(RESTARTED_SAID));
    let follow = replies(&stream(&r, "app"));
    assert_eq!(
        follow,
        vec![STOPPED.to_string()],
        "one true line: {follow:?}"
    );
}

/// The Stop button after the stopped run has ended but its tool call is still in flight (the retry already said it
/// would start again): the first turn's line stays true ("Stopped here to start again…" never claims it carried on),
/// and the retry's turn ends with the stop.
// decision: ORCH-684
#[test]
fn the_stop_button_after_the_stopped_run_ended_with_its_call_in_flight() {
    let (w, r) = stop_during_restart(15.0, vec![task_stop_call()], vec![]);
    stopped_not_restarted(&w, &r);
    let first = replies(&stream(&r, "chat"));
    assert_eq!(first.last().map(String::as_str), Some(RESTARTED_SAID));
    let follow = replies(&stream(&r, "app"));
    assert_eq!(
        follow.last().map(String::as_str),
        Some(STOPPED),
        "the retry's turn ends with the stop: {follow:?}"
    );
}

/// The owner's "stop" in chat after the stopped run ended, its call in flight: that message's reply says the stop and
/// is the chat's last word; the waiting retry adds nothing.
// decision: ORCH-684
#[test]
fn a_stop_in_chat_after_the_stopped_run_ended_with_its_call_in_flight() {
    let (w, r) = stop_during_restart(
        15.0,
        vec![
            json!({"do": "connect", "conn": "second"}),
            json!({"do": "stream", "conn": "second", "lines": [{"send": json!({"jsonrpc": "2.0", "id": 9,
                "method": "chat.send", "params": {"session_id": "s1", "text": "no, stop it"}}).to_string()}]}),
            json!({"do": "wait", "until": "model calls made: 4"}),
            json!({"do": "stream_end", "conn": "second", "lines": []}),
        ],
        vec![call("supervisor", &stop_answer())],
    );
    stopped_not_restarted(&w, &r);
    let second = replies(&stream(&r, "second"));
    assert_eq!(second, vec![STOPPED.to_string()], "{second:?}");
    let follow = replies(&stream(&r, "app"));
    assert!(
        !follow.iter().any(|t| t == STOPPED),
        "the retry's turn doesn't say it again: {follow:?}"
    );
}
