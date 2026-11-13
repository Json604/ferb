//! The supervisor's paths no earlier recording reached, replayed against the recordings
//! rust/fixtures/supervisor/make_more.py made from the real Python daemon (more_scenarios.json): the running
//! summary's fold, several asks and their card, the checklist, memory and forget side entries, an answer routed in
//! chat, loops, the paused task's mention, a closed tab, findings on a reason, a runtime switch, an event wait, and
//! the rest named in that script. The harness is tests/replay.rs's (tests/replay/harness.rs): every frame,
//! every model call, every tool call's reply, the chat RPCs' replies and every table but those another part owns.

mod common;
#[path = "replay/harness.rs"]
mod harness;

use std::collections::BTreeSet;
use std::sync::{Arc, OnceLock};

use common::*;
use harness::*;
use serde_json::{json, Map, Value};

fn more() -> &'static Value {
    static M: OnceLock<Value> = OnceLock::new();
    M.get_or_init(|| read("rust/fixtures/supervisor/more_scenarios.json"))
}

/// A scenario's kept records (make_more.py `kept_records`), each role's instruction inputs put back.
fn records(name: &str) -> Vec<Value> {
    let inputs = &more()["inputs"];
    more()["records"][name]
        .as_array()
        .unwrap_or_else(|| panic!("no records for {name}"))
        .iter()
        .map(|r| {
            let mut r = resolve(r);
            if let Some(key) = r["inputs"]["inputs"].as_str() {
                r["inputs"] = resolve(&inputs[key]);
            }
            r
        })
        .collect()
}

/// The daemon's registered tools and the answer schemas, as tests/replay.rs builds them.
fn tools_and_schemas() -> (Vec<Tool>, Map<String, Value>) {
    let chat = read("rust/fixtures/daemon/chat_scenarios.json");
    let catalog = read("rust/fixtures/catalog/tools.json");
    let registered: BTreeSet<String> = chat["scenarios"]
        .as_array()
        .unwrap()
        .iter()
        .chain(more()["scenarios"].as_array().unwrap().iter())
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
    let tools = catalog["tools"]
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
    let schemas = answers["schemas"]
        .as_object()
        .unwrap()
        .iter()
        .filter_map(|(k, v)| v.get("core").map(|c| (k.clone(), c.clone())))
        .collect();
    (tools, schemas)
}

/// The scenario's folder number as the recorder numbered it (`/FX/NN/home`; a scenario that names no path: any).
fn folder_index(sc: &Value) -> usize {
    let text = sc.to_string();
    text.find("/FX/")
        .and_then(|at| text[at + 4..at + 6].parse::<usize>().ok())
        .map_or(0, |n| n - 1)
}

fn replay(name: &str) -> Report {
    let chat = read("rust/fixtures/daemon/chat_scenarios.json");
    let (_folder, run) = RunRoot::new(recorded_root_len(&chat));
    let run = Arc::new(run);
    let (tools, schemas) = tools_and_schemas();
    let sc = more()["scenarios"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["name"] == name)
        .unwrap_or_else(|| panic!("no scenario {name}"));
    let r = run_scenario(sc, &records(name), folder_index(sc), &run, &tools, &schemas);
    eprintln!("{name}: {} comparisons", r.checked);
    if let Some(stop) = &r.stopped {
        eprintln!("  stops at {stop}");
    }
    for f in r.frames.iter().take(3) {
        eprintln!("  frames: {f}");
    }
    for f in r.calls.iter().take(3) {
        eprintln!("  model calls: {f}");
    }
    for f in r.tables.iter().take(6) {
        eprintln!("  tables: {f}");
    }
    if !r.skipped.is_empty() {
        eprintln!("  not compared: {}", r.skipped.join(", "));
    }
    r
}

fn full(name: &str) {
    let r = replay(name);
    assert!(r.full(), "{name} does not replay fully");
}

// decision: ORCH-303
// decision: ORCH-326
#[test]
fn summary_fold() {
    full("summary_fold");
}

// decision: ORCH-319
// decision: ORCH-543
#[test]
fn several_asks() {
    full("several_asks");
}

// decision: ORCH-532
#[test]
fn plan_parts_checklist() {
    full("plan_parts_checklist");
}

#[test]
fn remember_then_forget() {
    full("remember_then_forget");
}

// decision: ORCH-537
#[test]
fn answer_in_chat() {
    full("answer_in_chat");
}

// decision: ORCH-626
#[test]
fn checklist_single() {
    full("checklist_single");
}

// decision: ORCH-322
#[test]
fn ask_step() {
    full("ask_step");
}

// decision: ORCH-322
#[test]
fn wait_step() {
    full("wait_step");
}

#[test]
fn route_question() {
    full("route_question");
}

// decision: ORCH-315
#[test]
fn checker_runtime_down() {
    full("checker_runtime_down");
}

// decision: ORCH-558
#[test]
fn last_call_empty() {
    full("last_call_empty");
}

// decision: ORCH-553
#[test]
fn unbacked_part() {
    full("unbacked_part");
}

// decision: ORCH-538
#[test]
fn correct_pending_check() {
    full("correct_pending_check");
}

// decision: ORCH-619
#[test]
fn earlier_result() {
    full("earlier_result");
}

// decision: ORCH-505
// decision: ORCH-322
#[test]
fn loop_steer_finish() {
    full("loop_steer_finish");
}

// decision: ORCH-323
// decision: ORCH-536
#[test]
fn loop_pause_stop() {
    full("loop_pause_stop");
}

// decision: ORCH-312
#[test]
fn mention_paused() {
    full("mention_paused");
}

// decision: ORCH-536
#[test]
fn owner_closed_tab() {
    full("owner_closed_tab");
}

// decision: ORCH-562
#[test]
fn reason_findings() {
    full("reason_findings");
}

// decision: ORCH-315
#[test]
fn runtime_switch_tap() {
    full("runtime_switch_tap");
}

// decision: ORCH-322
#[test]
fn event_wait() {
    full("event_wait");
}
