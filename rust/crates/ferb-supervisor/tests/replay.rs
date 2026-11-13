//! The forward path replayed against the 28 chat scenarios recorded from the real Python daemon
//! (rust/fixtures/daemon/chat_scenarios.json): every frame the app received on each connection (bytes and order), every
//! model call FERB made (its runtime, tools, answer schema, instructions and context parts, byte for byte) and the store
//! tables at the end.
//!
//! Inputs are what other parts gave the Python supervisor: the scripted model answers (each call's recorded events),
//! the reads the supervisor made of other parts (its environment, live facts, the facts every role's instructions
//! carry, the relevant-activity search, the desktop observations; rust/fixtures/supervisor/scenarios.json recorded them
//! at the call), the gateway's tool listing (rust/fixtures/catalog/tools.json, the daemon's registered tools) and the
//! rows the desktop observer wrote (its `disturbance.*` spans, written through the desktop seam when it is called).
//! The store is a real FERB store in a temporary folder, migrated from ferb/store/migrations, holding the rows the
//! daemon's start wrote before the scenario's first message.
//!
//! Compared after the run: every frame line, every model call, every tool call's reply through the bridge, the chat
//! RPCs' replies (`chat.status`, `chat.approve`, `chat.answer`, `task.stop`) and every table except those another part
//! owns (`learning_*`: the episode card the memory part captures after an outcome; `perception_floor`,
//! `recall_state`, `trail_changes`: the daemon's start).
//!
//! The daemon's side is real where it is the supervisor's partner: ferb-gateway's `Gateway` (its pipeline, spans,
//! ledger rows, questions and events) with the change record (ferb-fs) behind its hooks, scripted tools answering with
//! the recorded tool results, the bus a chat turn subscribes to, and compose.py's event consumers a chat scenario
//! reaches (`_publish`'s request id on a task's question, `_question_record`). The daemon's other RPC areas, its
//! notifications and the timer loop's delivery are passed over and named in the report; a restart isn't replayed.
//! FERB hashes real paths into a few records and this run's folder can't be the recorder's, so 64-hex hashes are
//! compared as `<sha256>`.

mod common;
#[path = "replay/harness.rs"]
mod harness;

use common::*;
use harness::*;
use serde_json::{json, Map, Value};
use std::collections::BTreeSet;
use std::sync::Arc;

/// The scenarios that replay fully: the plain reply, the clarifying question and its answer, the one-step task, a
/// task's tool calls through the real gateway (a file written and read back, the change record its checker reads or
/// skips, a failed call and the retry, a wait in code while the clock moves), the two-step plan, the plan sent back for
/// repair, a protected file's confirmation answered Yes, No or left pending under a stop, the checker's rejection, its
/// findings with the fix turn, a plan step's question answered in chat, the check-in after five attempts, pause and
/// steer and resume, a correction delivered into a running call, the stops (by task.stop, by chat, with a new task), the
/// merged message and the attachment with chat.send's refusals. Control and recovery's paths are the other port's
/// (control.rs, owner_wait.rs), run here through the real gateway.
const FULL: [&str; 28] = [
    "plain_reply",
    "clarify_then_answer",
    "one_step_task",
    "one_step_with_tools",
    "checker_skips_change_records",
    "multi_step_plan",
    "confirm_yes",
    "confirm_no",
    "confirm_stop_pending",
    "pause_steer_resume",
    "steer_running_task",
    "stop_by_task_stop",
    "stop_by_chat",
    "stop_plus_new_task",
    "second_message_merged",
    "wait_for_time",
    "tool_failure_then_retry",
    "plan_repair",
    "checker_rejects",
    "checker_findings_fix",
    "worker_question_answered",
    "check_in_after_five",
    "attachment_and_bad_params",
    "restart_mid_task",
    "restart_with_open_question",
    "timer_set_and_fired",
    "schedule_created_and_fired",
    "global_stop",
];

#[test]
fn the_recorded_chat_scenarios() {
    let chat = read("rust/fixtures/daemon/chat_scenarios.json");
    let (_folder, run) = RunRoot::new(recorded_root_len(&chat));
    let run = Arc::new(run);
    let catalog = read("rust/fixtures/catalog/tools.json");
    // the daemon's registry: the tools its owner workers were offered (every one but those a missing grant hides)
    let registered: BTreeSet<String> = chat["scenarios"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|s| s["model_calls"].as_array().unwrap().iter())
        .filter(|m| m["role_played"] != "planner")
        .flat_map(|m| strs(&resolve(&m["tools"])))
        .chain(
            [
                "about.read",
                "chat.history",
                "memory.read",
                "task.evidence",
                "trail.days",
                "trail.query",
            ]
            .map(String::from),
        )
        .collect();
    let tools: Vec<Tool> = catalog["tools"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|t| registered.contains(t["name"].as_str().unwrap()))
        .map(|t| Tool {
            name: s(&t["name"]),
            row: json!({"name": t["name"], "description": t["description"], "input_schema": t["input_schema"]}),
            scopes: strs(&t["scopes"]),
            identities: strs(&t["identities"]),
            observation: t["facts"]["observation"].as_bool().unwrap_or(false),
        })
        .collect();
    let answers = read("rust/fixtures/catalog/answer_schemas.json");
    let schemas: Map<String, Value> = answers["schemas"]
        .as_object()
        .unwrap()
        .iter()
        .filter_map(|(k, v)| v.get("core").map(|c| (k.clone(), c.clone())))
        .collect();
    let supervisor = scenarios();
    let mut lines = Vec::new();
    let mut failures = Vec::new();
    let mut checked = 0;
    let only = std::env::var("REPLAY").ok();
    for (i, sc) in chat["scenarios"].as_array().unwrap().iter().enumerate() {
        let name = s(&sc["name"]);
        if only.as_ref().is_some_and(|o| *o != name) {
            continue;
        }
        let records = &supervisor.iter().find(|(n, _)| *n == name).unwrap().1;
        let r = run_scenario(sc, records, i, &run, &tools, &schemas);
        checked += r.checked;
        let verdict = if r.full() {
            "replays fully".to_string()
        } else {
            let mut why = Vec::new();
            if let Some(stop) = &r.stopped {
                why.push(format!("stops at {stop}"));
            }
            if !r.skipped.is_empty() {
                why.push(format!(
                    "the daemon's own lines not compared ({})",
                    r.skipped.join(", ")
                ));
            }
            why.extend(r.frames.iter().take(1).map(|f| format!("frames: {f}")));
            why.extend(r.calls.iter().take(1).map(|f| format!("model calls: {f}")));
            why.extend(r.tables.iter().take(2).map(|f| format!("tables: {f}")));
            format!("replays partly — {}", why.join("; "))
        };
        lines.push(format!("{name}: {verdict}"));
        if FULL.contains(&name.as_str()) && !r.full() {
            failures.push(format!("{name}: {verdict}"));
        }
    }
    eprintln!("{}", lines.join("\n"));
    eprintln!("{checked} comparisons");
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}

/// The one deliberate difference from Python on the planner's answer (ENG-6): a malformed legacy answer whose validator
/// raised TypeError went past Python's `except ValidationError` and failed the turn; here it is repaired like any
/// invalid answer, under the exception's class.
#[test]
fn a_legacy_answer_python_raised_on_is_repaired() {
    for legacy in [
        r#"{"kind": "task", "text": "x", "task": {"goal": "g", "name": "n", "identity": "owner", "worker_type": "default",
            "targets": {"apps": [], "sites": [], "paths": [], "recipients": []}, "success_conditions": [],
            "plan": {"steps": [{"id": "a", "kind": "map", "goal": "g", "inputs": 5}]}}}"#,
        r#"{"kind": "reply", "text": "x", "schedule": {"request": "r"}, "schedules": 7}"#,
    ] {
        let e = ferb_orchestrator::validate::supervisor_outcome_validate_json(legacy).unwrap_err();
        assert_eq!(
            e.raised().map(|r| r.exception),
            Some("TypeError"),
            "Python raises TypeError on {legacy}"
        );
        assert_eq!(ferb_supervisor::flow::turn::repair_class(&e), "TypeError");
    }
}
