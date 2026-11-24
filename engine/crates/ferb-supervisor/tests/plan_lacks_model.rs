//! RUN-474 (owner, 2026-11-24, T-1045: "Do now"): when the ChatGPT plan doesn't include FERB's model, FERB offers
//! another service he's already signed in to, with one tap, as its own question; his tap points that role at it the
//! way Settings → Thinking does and the message or task is tried again once. With no other service signed in, no
//! question: the plain line says how to get one. Run on the one-step task (one_step_task) with a planner or worker
//! call that fails for the plan put in front of its recorded one (invented: no recording has it).

mod common;
mod support_control;

use serde_json::{json, Value};
use support_control::{run_with, world_with, World};

const NAME: &str = "one_step_task";

/// A call of `key` that fails as Codex fails when the plan lacks the model (RUN-417's words, RUN-474's account).
fn plan_error(calls: &[Value], key: &str) -> Value {
    let mut call = calls.iter().find(|c| c["key"] == key).unwrap().clone();
    call["events"] = json!([{"kind": "error", "data": {
        "message": "Your ChatGPT plan doesn't include gpt-test right now: renew the plan, or pick another service in \
                    Settings → Thinking: model not found (modelNotFound)",
        "account": ferb_worker::errors::PLAN_LACKS_MODEL}}]);
    call
}

/// The recorded calls with a failing call of `key` in front of the recorded one.
fn failing_first(key: &'static str) -> impl FnOnce(&mut Vec<Value>) {
    move |calls: &mut Vec<Value>| {
        let at = calls.iter().position(|c| c["key"] == key).unwrap();
        let failing = plan_error(calls, key);
        calls.insert(at, failing);
    }
}

/// After the message is sent: the owner taps option `choice` (from 1) of the first question.
fn tap(choice: u64) -> impl FnOnce(&mut Vec<Value>) {
    move |steps: &mut Vec<Value>| {
        let at = steps.iter().position(|s| s["do"] == "stream").unwrap();
        steps.insert(
            at + 1,
            json!({"do": "wait", "until": "an approval is pending"}),
        );
        steps.insert(
            at + 2,
            json!({"do": "call", "conn": "app", "lines": [{"send": ferb_py::json::dumps(&json!({"jsonrpc": "2.0",
                "id": 90, "method": "chat.approve",
                "params": {"approval_id": "ap-a00000000001", "granted": true, "choice": choice}}))}]}),
        );
    }
}

fn world(edit: impl FnOnce(&mut Vec<Value>), types: &[&str]) -> World {
    let w = world_with(NAME, edit);
    *w.daemon.types.lock().unwrap() = Some(types.iter().map(|t| t.to_string()).collect());
    w
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

fn questions(w: &World) -> Vec<Value> {
    w.daemon
        .published
        .lock()
        .unwrap()
        .iter()
        .filter(|(topic, _)| topic == "approval.requested")
        .map(|(_, data)| data.clone())
        .collect()
}

fn calls_of(w: &World, role: &str) -> usize {
    w.daemon
        .calls
        .lock()
        .unwrap()
        .made
        .iter()
        .filter(|c| {
            c["runtime_role"]
                .as_str()
                .is_some_and(|r| r.starts_with(role))
        })
        .count()
}

/// Every frame of the message's stream, as sent.
fn frames(report: &support_control::Report) -> Vec<Value> {
    report
        .streams
        .iter()
        .flat_map(|(_, lines)| lines.iter())
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .map(|l| l["params"].clone())
        .collect()
}

// decision: RUN-474, RUN-475, ORCH-315
#[test]
fn the_plan_lacking_the_model_says_so_plainly() {
    use ferb_worker::errors::{account_problem, offers_switch, ACCOUNT_PROBLEMS, PLAN_LACKS_MODEL};
    assert!(offers_switch(PLAN_LACKS_MODEL) && offers_switch("usage_limit") && !offers_switch(""));
    // Settings → Thinking keeps only the two refusals it always kept (RUN-424)
    assert!(!ACCOUNT_PROBLEMS.contains(&PLAN_LACKS_MODEL));
    assert_eq!(
        account_problem("codex", PLAN_LACKS_MODEL),
        "Your ChatGPT plan doesn't include the model FERB uses with Codex, so I couldn't go on with this."
    );
}

/// A message whose thinking stops on the plan: FERB asks which signed-in service to use, offering only those; the tap
/// points the planner's role at it and the message is thought again once, then goes on as recorded.
// decision: RUN-474, RUN-475, ORCH-315
#[test]
fn a_chat_message_offers_the_signed_in_services_and_a_tap_retries_once() {
    let mut w = world(failing_first("supervisor"), &["default", "fake", "claude"]);
    // the retried planner turn reads what the first one read
    for q in [&w.daemon.supervise, &w.daemon.systems] {
        let mut q = q.lock().unwrap();
        let first = q.first().cloned().unwrap();
        q.insert(0, first);
    }
    let report = run_with(&mut w, NAME, tap(1));
    let asked = questions(&w);
    assert_eq!(asked.len(), 1, "{asked:?}");
    let q = &asked[0];
    assert_eq!(q["capability"], "runtime.switch");
    assert_eq!(q["options"], json!(["Use Claude", "Not now"]));
    assert_eq!(q["runtime_switch"]["choices"], json!(["claude"]));
    assert_eq!(q["runtime_switch"]["role"], "orchestrator");
    assert_eq!(q["task_id"], Value::Null);
    assert!(q["reason"]
        .as_str()
        .unwrap()
        .starts_with("Your ChatGPT plan doesn't include the model FERB uses with fake"));
    assert_eq!(
        *w.daemon.switched.lock().unwrap(),
        vec![("claude".to_string(), "orchestrator".to_string())]
    );
    assert_eq!(journal(&w, "runtime.switched").len(), 1);
    // the planner twice (the failure, the retry), then the recorded worker and check
    assert_eq!(calls_of(&w, "orchestrator"), 2);
    assert_eq!(calls_of(&w, "worker"), 1);
    let sent = frames(&report);
    assert!(
        !sent.iter().any(|f| f["status"] == "failed"),
        "the retried message went on: {sent:?}"
    );
    assert!(journal(&w, "chat.failed").is_empty());
}

/// "Not now": nothing switches, nothing is retried, and the reply says what happened.
// decision: RUN-474, RUN-475, ORCH-315
#[test]
fn not_now_ends_the_message_with_the_plain_line() {
    let mut w = world(failing_first("supervisor"), &["default", "fake", "claude"]);
    let report = run_with(&mut w, NAME, tap(2));
    assert!(w.daemon.switched.lock().unwrap().is_empty());
    assert_eq!(calls_of(&w, "orchestrator"), 1);
    let sent = frames(&report);
    let reply = sent.iter().find(|f| f["type"] == "reply").expect("a reply");
    assert_eq!(reply["status"], "failed");
    assert_eq!(
        reply["text"],
        "Your ChatGPT plan doesn't include the model FERB uses with fake, so I couldn't go on with this."
    );
}

/// No other service signed in: no question; the plain line says how to sign in to one.
// decision: RUN-474, RUN-475
#[test]
fn with_no_other_service_signed_in_nothing_is_asked() {
    let mut w = world(failing_first("supervisor"), &["default", "fake"]);
    let report = run_with(&mut w, NAME, |_| {});
    assert!(questions(&w).is_empty());
    assert!(w.daemon.switched.lock().unwrap().is_empty());
    let sent = frames(&report);
    let reply = sent.iter().find(|f| f["type"] == "reply").expect("a reply");
    assert_eq!(reply["status"], "failed");
    assert_eq!(
        reply["text"],
        "Your ChatGPT plan doesn't include the model FERB uses with fake, so I couldn't go on with this. No other \
         service is signed in for FERB. To use another, sign in to Claude or add an API key in Settings → Thinking."
    );
    assert!(reply.get("runtime_switch").is_none());
}

/// A task's worker stops on the plan: the step's own question offers the signed-in services; the tap points the
/// worker's role at the pick and the step runs again with it.
// decision: RUN-474, RUN-475, ORCH-315
#[test]
fn a_task_step_offers_the_signed_in_services_and_a_tap_retries_it() {
    let mut w = world(failing_first("worker"), &["default", "fake", "claude"]);
    let _ = run_with(&mut w, NAME, tap(1));
    let asked = questions(&w);
    assert_eq!(asked.len(), 1, "{asked:?}");
    let q = &asked[0];
    assert_eq!(q["options"], json!(["Use Claude", "Stop"]));
    assert_eq!(q["runtime_switch"]["choices"], json!(["claude"]));
    assert_eq!(q["runtime_switch"]["problem"], "model_not_in_plan");
    assert_eq!(
        q["summary"],
        "Your ChatGPT plan doesn't include FERB's model. Switch service?"
    );
    assert_eq!(
        *w.daemon.switched.lock().unwrap(),
        vec![("claude".to_string(), "worker".to_string())]
    );
    assert_eq!(
        calls_of(&w, "worker"),
        2,
        "the failed attempt, then one more"
    );
    let switched = journal(&w, "runtime.switched");
    assert_eq!(switched.len(), 1);
    assert_eq!(switched[0]["runtime"], "claude");
}

/// A task's worker with no other service signed in: the step ends with the plain line, nothing asked.
// decision: RUN-474, RUN-475
#[test]
fn a_task_step_with_no_other_service_ends_plainly() {
    let mut w = world(failing_first("worker"), &["default", "fake"]);
    let _ = run_with(&mut w, NAME, |_| {});
    assert!(questions(&w).is_empty());
    assert_eq!(calls_of(&w, "worker"), 1);
    let db = w.db.lock().unwrap();
    let outcome: String = db
        .conn()
        .query_row(
            "SELECT outcome_json FROM tasks WHERE parent_id IS NULL ORDER BY rowid DESC LIMIT 1",
            [],
            |r| r.get(0),
        )
        .unwrap_or_default();
    let summary = serde_json::from_str::<Value>(&outcome).unwrap()["summary"]
        .as_str()
        .unwrap_or("")
        .to_string();
    assert!(
        summary.ends_with(
            "To use another, sign in to Claude or add an API key in Settings → Thinking."
        ),
        "{summary}"
    );
}
