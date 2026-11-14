//! ORCH-643 and ORCH-644: a plan's wait for another task to finish (live 2026-11-14, tree 15737cce waited on the
//! research task 153f1603 under an invented event name and never moved). Scripted on recorded control scenarios with
//! the planner's answers replaced: the awaited task is a row FERB recorded (`tasks`), its end is the event FERB
//! publishes when a tree ends (`task.finished`).

mod common;
mod support_control;

use serde_json::{json, Value};
use support_control::{run_with, world_with, World};

const AWAITED: &str = "t-research";

fn plan_answer(steps: Vec<Value>) -> String {
    json!({"kind": "task", "answers_question": false, "task": {
        "goal": "Make the invented sheet once the invented research is done", "name": "Sheet", "identity": "owner",
        "worker_type": "default", "targets": {"apps": [], "sites": [], "paths": [], "recipients": []},
        "success_conditions": [{"text": "The sheet exists", "owner_text": "", "layer": "outcome", "check": null}],
        "plan": {"steps": steps, "input_paths": [], "big_job": false, "time_estimate": "", "cost_estimate": "",
                 "standing_sends": []},
        "folders": [], "keep_tabs_open": false, "parts": [], "earlier_results": []},
        "control": null, "side_entries": null, "text": "On it."})
    .to_string()
}

fn wait_for_research() -> Value {
    json!({"id": "wait_research", "kind": "wait", "label": "", "goal": "Wait for the research",
           "depends_on": [], "inputs": [], "worker_type": "default", "conditions": [], "until": null,
           "event": "task.finished", "event_match": [{"field": "task_id", "value": AWAITED}], "question": "",
           "options": [], "conversation": null, "repeat": null, "earlier_results": []})
}

fn final_text(text: &str) -> Value {
    json!([
        {"kind": "usage", "data": {"input_tokens": 10, "output_tokens": 5, "cost_usd": 0.001}},
        {"kind": "final", "data": {"text": text}}
    ])
}

/// The awaited task as FERB recorded it (another tree of this chat, `status`).
fn awaited_row(w: &World, status: &str) {
    w.db.lock()
        .unwrap()
        .conn()
        .execute(
            "INSERT INTO tasks (id, tree_id, parent_id, request_id, goal, identity, role, scopes_json, status, \
             outcome_json, created_at, updated_at) VALUES (?1, ?1, NULL, 'r-research', \
             'Research the invented laptops', 'owner', 'worker', '[]', ?2, '{}', \
             '2026-01-01T08:00:00+00:00', '2026-01-01T08:00:00+00:00')",
            [AWAITED, status],
        )
        .unwrap();
}

/// The wait step's row in plan_steps, and the newest tree's status (the awaited task's row is older).
fn wait_row(w: &World) -> (String, Value, String) {
    let db = w.db.lock().unwrap();
    let (status, output): (String, String) = db
        .conn()
        .query_row(
            "SELECT status, output FROM plan_steps WHERE step_id = 'wait_research'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    let tree: String = db
        .conn()
        .query_row(
            "SELECT status FROM tasks WHERE parent_id IS NULL AND id != ?1 ORDER BY rowid DESC LIMIT 1",
            [AWAITED],
            |r| r.get(0),
        )
        .unwrap();
    (status, serde_json::from_str(&output).unwrap(), tree)
}

/// One turn's plan with the steps a test gives after the chat.send (`stop_while_step_asks`'s first three steps: the
/// chat's connection, its hello, the message), then the task's end.
fn one_turn(w: &mut World, after: Vec<Value>) {
    let report = run_with(w, "stop_while_step_asks", |steps| {
        steps.truncate(3);
        steps.extend(after);
        steps.push(json!({"do": "wait", "until": "the task ended"}));
    });
    eprintln!("scripted: {}", report.verdict());
}

fn world_waiting(status: &str) -> World {
    let text = plan_answer(vec![wait_for_research()]);
    let w = world_with("stop_while_step_asks", |calls| {
        let planner = calls.iter_mut().find(|c| c["key"] == "supervisor").unwrap();
        planner["events"] = final_text(&text);
    });
    awaited_row(&w, status);
    w
}

// The awaited task ended before the wait began: the step goes on at once, saying where its answer is.
// decision: ORCH-643
#[test]
fn a_wait_registered_after_the_task_ended_continues_at_once() {
    let mut w = world_waiting("done");
    one_turn(&mut w, vec![]);
    let (status, output, _) = wait_row(&w);
    assert_eq!(status, "done", "{output}");
    assert_eq!(
        output["summary"],
        "\u{201c}Research the invented laptops\u{201d} finished (done). Its answer: chat_history task_id=t-research."
    );
    assert_eq!(
        output["event"],
        json!({"task_id": AWAITED, "status": "done"})
    );
}

// The awaited task is still running when the wait begins; its end (the row, then task.finished) wakes the wait.
// decision: ORCH-643
#[test]
fn a_wait_on_another_task_wakes_when_that_task_ends() {
    let mut w = world_waiting("running");
    one_turn(
        &mut w,
        vec![
            json!({"do": "wait", "until": "a plan step waits"}),
            json!({"do": "plan_event", "topic": "task.finished",
                   "sql": format!("UPDATE tasks SET status = 'done' WHERE id = '{AWAITED}'"),
                   "data": {"task_id": AWAITED, "status": "done"}}),
        ],
    );
    let (status, output, _) = wait_row(&w);
    assert_eq!(status, "done", "{output}");
    assert_eq!(output["event"]["status"], "done");
}

// The awaited task was stopped: the wait ends with that fact and the step after it won't run.
// decision: ORCH-643
#[test]
fn a_wait_on_a_cancelled_task_ends_with_that_fact() {
    let text = plan_answer(vec![
        wait_for_research(),
        json!({"id": "make_sheet", "kind": "do", "label": "", "goal": "Make the sheet",
               "depends_on": ["wait_research"], "inputs": [], "worker_type": "default", "conditions": [],
               "until": null, "event": null, "event_match": [], "question": "", "options": [],
               "conversation": null, "repeat": null, "earlier_results": []}),
    ]);
    let mut w = world_with("stop_while_step_asks", |calls| {
        let planner = calls.iter_mut().find(|c| c["key"] == "supervisor").unwrap();
        planner["events"] = final_text(&text);
    });
    awaited_row(&w, "running");
    one_turn(
        &mut w,
        vec![
            json!({"do": "wait", "until": "a plan step waits"}),
            json!({"do": "plan_event", "topic": "task.finished",
                   "sql": format!("UPDATE tasks SET status = 'cancelled' WHERE id = '{AWAITED}'"),
                   "data": {"task_id": AWAITED, "status": "cancelled"}}),
        ],
    );
    let (status, output, _) = wait_row(&w);
    assert_eq!(status, "failed", "{output}");
    assert_eq!(
        output["summary"],
        "\u{201c}Research the invented laptops\u{201d}, the task this step waited for, ended cancelled, so the \
         steps that needed it won't run."
    );
    let db = w.db.lock().unwrap();
    let sheet: String = db
        .conn()
        .query_row(
            "SELECT status FROM plan_steps WHERE step_id = 'make_sheet'",
            [],
            |r| r.get(0),
        )
        .unwrap_or_default();
    assert_ne!(sheet, "done", "the step that needed it didn't run");
}

// The live case: the owner asks about a task whose wait is stuck. The planner's task status facts say what the step
// waits for, since when and what can be done; the planner continues the task, and the step (whose awaited task has
// ended meanwhile, with no event) looks again and goes on.
// decision: ORCH-644
#[test]
fn asked_about_a_waiting_task_the_planner_has_the_facts_and_continue_carries_it_on() {
    let name = "steer_while_checking";
    let text = plan_answer(vec![wait_for_research()]);
    let mut w = world_with(name, |calls| {
        // the scripted model answers by call order: the plan, then the owner's question's turn
        calls[0]["events"] = final_text(&text);
        let second = json!({"kind": "reply", "answers_question": false, "task": null,
            "control": {"task_id": "id-000021", "action": "continue", "new_goal": false,
                        "standing_sends": "unchanged", "supersedes": []},
            "side_entries": null, "text": "It was waiting for the research; carrying on."});
        calls[1]["events"] = final_text(&second.to_string());
    });
    awaited_row(&w, "running");
    {
        // the facts of each planner turn this script makes (the recording's last, again)
        let mut q = w.daemon.supervise.lock().unwrap();
        let last = q.last().cloned().unwrap();
        eprintln!("recorded planner turns: {}", q.len());
        q.push(last);
    }
    let report = run_with(&mut w, name, |steps| {
        // the chat's message, then the wait parks; the research ends with no event (the live tree's state); the
        // owner asks on the app's connection
        let ask = steps
            .iter()
            .position(|st| st["do"] == "stream" && st["conn"] == "app")
            .unwrap();
        let mut asked = steps[ask].clone();
        let mut sent: Value =
            serde_json::from_str(asked["lines"][0]["send"].as_str().unwrap()).unwrap();
        sent["params"]["text"] = json!("is it stuck?");
        asked["lines"][0]["send"] = json!(sent.to_string());
        let app: Vec<Value> = steps
            .iter()
            .filter(|st| st["conn"] == "app" && (st["do"] == "connect" || st["do"] == "call"))
            .cloned()
            .collect();
        let mut out: Vec<Value> = steps[..3].to_vec();
        out.push(json!({"do": "wait", "until": "a plan step waits"}));
        out.push(json!({"do": "plan_event", "topic": "",
                        "sql": format!("UPDATE tasks SET status = 'done' WHERE id = '{AWAITED}'")}));
        out.extend(app);
        out.push(asked);
        out.push(json!({"do": "wait", "until": "the task ended"}));
        *steps = out;
    });
    eprintln!("{name} (scripted): {}", report.verdict());
    let calls = w.daemon.calls.lock().unwrap();
    assert_eq!(
        calls.made[1]["runtime_role"], "orchestrator",
        "the question's turn"
    );
    let started = calls
        .started
        .iter()
        .find(|c| c["n"] == json!(1))
        .expect("the planner's turn for the question started");
    let context = started["parts"].to_string();
    assert!(context.contains("waiting_steps"), "{context}");
    assert!(
        context.contains("Waiting for \u{201c}Research the invented laptops\u{201d} to finish"),
        "{context}"
    );
    assert!(context.contains("\\\"since\\\""), "{context}");
    assert!(
        context.contains("control continue on this task"),
        "{context}"
    );
    drop(calls);
    let (status, output, _) = wait_row(&w);
    assert_eq!(status, "done", "continue carried it on: {output}");
}

// The live plan's invented event name is refused at admission and told back with the names FERB publishes; the
// planner's repaired plan (the right shape) runs.
// decision: ORCH-642
#[test]
fn a_wait_on_an_event_ferb_never_publishes_is_refused_and_asked_again() {
    let mut invented = wait_for_research();
    invented["event"] = json!("task_completed");
    let first = plan_answer(vec![invented]);
    let repaired = plan_answer(vec![wait_for_research()]);
    let mut w = world_with("stop_while_step_asks", |calls| {
        calls[0]["events"] = final_text(&first);
        calls[1]["events"] = final_text(&repaired);
    });
    {
        let mut q = w.daemon.supervise.lock().unwrap();
        let last = q.last().cloned().unwrap();
        q.push(last);
    }
    awaited_row(&w, "done");
    one_turn(&mut w, vec![]);
    let calls = w.daemon.calls.lock().unwrap();
    assert_eq!(calls.made[1]["runtime_role"], "orchestrator", "asked again");
    let started = calls
        .started
        .iter()
        .find(|c| c["n"] == json!(1))
        .expect("the repair turn started");
    let context = started["parts"].to_string();
    assert!(
        context.contains("wait_research: FERB never publishes an event named 'task_completed'"),
        "{context}"
    );
    assert!(
        context.contains("task.finished (task_id, status: a task ended"),
        "{context}"
    );
    drop(calls);
    let db = w.db.lock().unwrap();
    let repairs: i64 = db
        .conn()
        .query_row(
            "SELECT COUNT(*) FROM journal WHERE kind = 'task.repair' AND data_json LIKE '%task_completed%'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(repairs, 1);
    drop(db);
    let (status, _, _) = wait_row(&w);
    assert_eq!(status, "done", "the repaired plan ran");
}

/// The task lines FERB.app's Tasks list shows (`task.progress` on the bus), in order.
fn progress_lines(w: &World) -> Vec<String> {
    w.daemon
        .published
        .lock()
        .unwrap()
        .iter()
        .filter(|(topic, _)| topic == "task.progress")
        .filter_map(|(_, d)| d["text"].as_str().map(str::to_string))
        .collect()
}

// Live 2026-11-14 23:34: a tree whose only live step was a wait showed its last finished step's words in the Tasks
// list. While only a wait is live, the task's own line is the wait's.
// decision: ORCH-647
#[test]
fn a_task_whose_only_live_step_is_a_wait_says_what_it_waits_for() {
    let mut w = world_waiting("running");
    one_turn(
        &mut w,
        vec![
            json!({"do": "wait", "until": "a plan step waits"}),
            json!({"do": "plan_event", "topic": "task.finished",
                   "sql": format!("UPDATE tasks SET status = 'done' WHERE id = '{AWAITED}'"),
                   "data": {"task_id": AWAITED, "status": "done"}}),
        ],
    );
    let lines = progress_lines(&w);
    assert!(
        lines
            .iter()
            .any(|l| l == "Waiting for \u{201c}Research the invented laptops\u{201d} to finish"),
        "{lines:?}"
    );
}

fn time_wait() -> Value {
    json!({"id": "wait_noon", "kind": "wait", "label": "", "goal": "Wait for the morning", "depends_on": [],
        "inputs": [], "worker_type": "default", "conditions": [], "until": "2030-01-01T09:00:00+00:00",
        "event": null, "event_match": [], "question": "", "options": [], "conversation": null, "repeat": null,
        "earlier_results": []})
}

/// The plan's steps run until `until` (the harness's condition), then the owner's stop; the task lines published.
fn lines_until_stopped(steps: Vec<Value>, until: &str) -> Vec<String> {
    let text = plan_answer(steps);
    let mut w = world_with("stop_while_step_asks", |calls| {
        let planner = calls.iter_mut().find(|c| c["key"] == "supervisor").unwrap();
        planner["events"] = final_text(&text);
    });
    let until = until.to_string();
    let report = run_with(&mut w, "stop_while_step_asks", |steps| {
        for st in steps.iter_mut() {
            if st["do"] == "wait" {
                st["until"] = json!(until);
            }
        }
        let at = steps.iter().position(|st| st["do"] == "close").unwrap();
        steps.insert(at + 1, json!({"do": "wait", "until": "the task ended"}));
    });
    eprintln!("scripted: {}", report.verdict());
    progress_lines(&w)
}

// A wait on a time says the time (a day ahead: with its day).
// decision: ORCH-647
#[test]
fn a_time_wait_says_its_time() {
    let lines = lines_until_stopped(vec![time_wait()], "a plan step waits");
    assert!(
        lines
            .iter()
            .any(|l| l.starts_with("Waiting until ") && l.contains(" Jan ")),
        "{lines:?}"
    );
}

// A wait beside a question to the owner says nothing: the task is asking, and the Tasks list says so.
// decision: ORCH-647
#[test]
fn a_wait_beside_an_open_question_stays_quiet() {
    let mut ask = time_wait();
    ask["id"] = json!("choose");
    ask["kind"] = json!("ask");
    ask["until"] = Value::Null;
    ask["question"] = json!("Which one?");
    ask["options"] = json!(["One", "Two"]);
    let lines = lines_until_stopped(vec![ask, time_wait()], "an approval is pending");
    assert!(!lines.iter().any(|l| l.starts_with("Waiting")), "{lines:?}");
}
