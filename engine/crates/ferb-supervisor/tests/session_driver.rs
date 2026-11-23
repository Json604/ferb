//! HAND-707, HAND-709, ORCH-705: the session driver's task as the supervisor runs it. Hand to FERB makes no planner or
//! checker call: each agent reply is exactly one fresh worker step, whose request carries no profile, memory, chat
//! history, schedules or screen, and whose token keeps the lookups that read what FERB knows (so "our decision" can be
//! looked up) while typing into the session stays code's. Recorded like any task: its rows, a span per step with the
//! worker run under it; never a chat entry, a task result or an episode. Take it back is the task's stop. Stand-in
//! models (a recorded scenario's world with the test's own calls), temp store, injected ids and time.

mod common;
mod support_control;

use ferb_context::assemble::ContextBlock;
use ferb_supervisor::flow::driver::{Decision, DriverStep, STEP_SPAN};
use serde_json::{json, Value};
use support_control::{world_with, World};

const PROFILE: &str = "About the owner (always on): INVENTED-PROFILE-LINE";
const NEWLY_ON: &str = "INVENTED-NEWLY-ALWAYS-ON";
const MEMORY: &str = "INVENTED-MEMORY-ITEM about the owner";
const SCHEDULES: &str = "The owner's schedules: 1\n- Schedule sc-INVENTED0001";
const MEANING: &str = "- When the owner says “INVENTED-PHRASE”, they mean: INVENTED-MEANING";
/// What the stand-in finds in the chat history: the decision the goal refers to.
const DECISION: &str = "INVENTED-DECISION: Postgres, one schema per tenant";
const GOAL: &str = "carry on with what we decided about the database";

/// One stand-in model call: a lookup of the chat history, then its decision.
fn call(decision: &str, text: &str, looks_up: bool) -> Value {
    let mut events = Vec::new();
    if looks_up {
        events.push(json!({"kind": "tool_call", "data": {"name": "chat_history", "offered": true,
                                                         "arguments": {"query": "database decision"}}}));
    }
    events.push(json!({"kind": "usage", "data": {"input_tokens": 10, "output_tokens": 5, "cost_usd": 0.001}}));
    events.push(json!({"kind": "final", "data": {"text": json!({"decision": decision, "text": text}).to_string()}}));
    json!({"events": events})
}

/// The recorded one_step_task world (its catalog, its role inputs) with what FERB knows of the owner seeded, and the
/// driver's own model calls from the first one on.
fn world(calls: Vec<Value>) -> World {
    let w = world_with("one_step_task", |_| {});
    *w.daemon.profile.lock().unwrap() = PROFILE.into();
    *w.daemon.newly_on.lock().unwrap() = NEWLY_ON.into();
    *w.daemon.memory.lock().unwrap() = vec![ContextBlock::new(MEMORY, "memory item")];
    *w.daemon.owner_state.lock().unwrap() = SCHEDULES.into();
    *w.daemon.meanings.lock().unwrap() = MEANING.into();
    *w.daemon.replaced.lock().unwrap() = Some((0, calls));
    w
}

fn step(round: u32, exchanges: &str, decided: &[&str]) -> DriverStep {
    DriverStep {
        round,
        goal: GOAL.into(),
        session: "Agent: Claude Code\nSession id: claude:abc\nFolder: /p/app\nApp: Terminal".into(),
        exchanges: exchanges.into(),
        asking: String::new(),
        decided: decided.iter().map(|d| d.to_string()).collect(),
    }
}

fn rt() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .start_paused(true)
        .build()
        .unwrap()
}

fn rows(w: &World, sql: &str) -> Vec<Vec<Value>> {
    let db = w.db.lock().unwrap();
    let mut st = db.conn().prepare(sql).unwrap();
    let n = st.column_count();
    st.query_map([], |r| {
        Ok((0..n)
            .map(|i| match r.get_ref(i).unwrap() {
                rusqlite::types::ValueRef::Null => Value::Null,
                rusqlite::types::ValueRef::Integer(x) => json!(x),
                rusqlite::types::ValueRef::Real(x) => json!(x),
                rusqlite::types::ValueRef::Text(t) => json!(String::from_utf8_lossy(t)),
                rusqlite::types::ValueRef::Blob(_) => Value::Null,
            })
            .collect())
    })
    .unwrap()
    .collect::<Result<_, _>>()
    .unwrap()
}

// decision: HAND-707
// decision: HAND-709
// decision: ORCH-705
#[test]
fn a_drive_is_one_fresh_worker_step_per_reply_with_its_lookups_and_no_planner_or_checker() {
    let w = world(vec![
        call("send", &format!("Go ahead with {DECISION}."), true),
        call("done", "The tenant schema is in and its tests pass.", false),
    ]);
    let rt = rt();
    let task = rt.block_on(w.sup.drive_begin("claude:abc", GOAL, "Terminal"));
    assert!(w.sup.inner().driver_task(&task));
    assert!(!w.sup.drive_stopped(&task));
    let first = rt
        .block_on(w.sup.drive_step(
            &task,
            &step(1, "Claude Code: Which database setup should I use?", &[]),
        ))
        .unwrap();
    // the stand-in looked the decision up in the chat history and used it
    assert_eq!(first, Decision::Send(format!("Go ahead with {DECISION}.")));
    let second = rt
        .block_on(w.sup.drive_step(
            &task,
            &step(
                2,
                "Claude Code: Done, tests pass.",
                &["Round 1: sent “Go ahead…”"],
            ),
        ))
        .unwrap();
    assert_eq!(
        second,
        Decision::Done("The tenant schema is in and its tests pass.".into())
    );
    rt.block_on(
        w.sup
            .drive_end(&task, "done", "The tenant schema is in and its tests pass."),
    );
    assert!(w.sup.drive_stopped(&task), "ended");

    // no planner and no checker: one worker run per step
    let made = w.daemon.calls.lock().unwrap().made.clone();
    let roles: Vec<&str> = made
        .iter()
        .map(|m| m["runtime_role"].as_str().unwrap())
        .collect();
    assert_eq!(roles, ["worker:default", "worker:default"], "{made:?}");
    // its token keeps the lookups that read what FERB knows
    for m in &made {
        let tools: Vec<&str> = m["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t.as_str().unwrap())
            .collect();
        for lookup in [
            "chat.history",
            "memory.read",
            "memory.learned",
            "trail.query",
        ] {
            assert!(tools.contains(&lookup), "{lookup}: {tools:?}");
        }
        assert!(
            tools.iter().any(|t| t.starts_with("fs.")),
            "the usual tools"
        );
        for typed in ferb_supervisor::flow::temporary::DRIVER_HIDDEN_TOOLS {
            assert!(
                !tools.contains(&typed),
                "{typed}: code types, never the step"
            );
        }
    }
    let only = w.daemon.only.lock().unwrap().clone();
    assert_eq!(only.len(), 2, "each step's token names its tools");
    // nothing pushed: no profile, newly always on, memory, owner state or meanings; every role read as fresh
    let started = w.daemon.calls.lock().unwrap().started.clone();
    assert_eq!(started.len(), 2);
    for (n, c) in started.iter().enumerate() {
        let system = c["system"].as_str().unwrap();
        let parts: String = c["parts"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            system.contains("keeping one of the owner's sessions"),
            "call {n}: its own prompt"
        );
        assert!(
            !system.contains("You are FERB's worker"),
            "call {n}: not the worker's"
        );
        for marker in [PROFILE, "sc-INVENTED0001", "INVENTED-MEANING"] {
            assert!(!system.contains(marker), "call {n}: {marker}");
        }
        for marker in [
            NEWLY_ON,
            MEMORY,
            "sc-INVENTED0001",
            ferb_supervisor::flow::temporary::FRESH_FACT,
        ] {
            assert!(!parts.contains(marker), "call {n}: {marker}");
        }
        assert!(parts.contains(GOAL), "call {n}: the goal");
        assert!(
            parts.contains("Session id: claude:abc"),
            "call {n}: the session"
        );
        assert_eq!(c["images"], 0, "call {n}: no screen");
    }
    let s0: String = started[0]["parts"].to_string();
    assert!(s0.contains("Which database setup should I use?") && s0.contains("<untrusted-data"));
    let s1: String = started[1]["parts"].to_string();
    assert!(s1.contains("Round 1: sent"), "what it decided before");
    assert!(w.daemon.fresh_reads.lock().unwrap().iter().all(|f| *f));

    // recorded like any task: its rows, a span per step with the worker run under it
    let tasks = rows(&w, "SELECT id, request_id, status FROM tasks ORDER BY id");
    assert_eq!(tasks.len(), 2, "{tasks:?}");
    assert!(tasks
        .iter()
        .all(|t| t[1].as_str().unwrap().starts_with("driver-") && t[2] == "done"));
    let steps = rows(&w, &format!("SELECT id, input_json, output_json FROM spans WHERE kind = '{STEP_SPAN}' ORDER BY rowid"));
    assert_eq!(steps.len(), 2);
    let out: Value = serde_json::from_str(steps[0][2].as_str().unwrap()).unwrap();
    assert_eq!(out["decision"], "send");
    for st in &steps {
        let under = rows(
            &w,
            &format!(
                "SELECT COUNT(*) FROM spans WHERE kind = 'worker' AND parent_id = '{}'",
                st[0].as_str().unwrap()
            ),
        );
        assert_eq!(under[0][0], 1, "the worker run under its step");
    }
    // never the chat's or the Tasks page's, never an episode
    for table in ["chat_entries", "task_results", "episodes"] {
        assert_eq!(
            rows(&w, &format!("SELECT COUNT(*) FROM {table}"))[0][0],
            0,
            "{table}"
        );
    }
    assert!(w.daemon.episodes.lock().unwrap().is_empty());
    let published = w.daemon.published.lock().unwrap().clone();
    assert!(!published
        .iter()
        .any(|(t, _)| t == "task.created" || t == "task.result" || t == "task.finished"));
    drop(rt);
    w.sup.chat_flush();
}

/// Take it back is the task's own stop: the drive reads it as stopped, and a step it would run refuses.
// decision: HAND-709
#[test]
fn take_it_back_is_the_task_s_stop() {
    let w = world(vec![call("send", "next", false)]);
    let rt = rt();
    let task = rt.block_on(w.sup.drive_begin("claude:abc", GOAL, ""));
    // not a chat task: no client adopts it as one
    assert!(w.sup.running_trees().iter().any(|(t, _)| *t == task));
    let stopped = w
        .sup
        .task_stop(json!({"task_ids": [task]}).as_object().unwrap())
        .unwrap();
    assert_eq!(stopped["stopped"], json!([task]));
    assert!(w.sup.drive_stopped(&task));
    assert!(rt
        .block_on(w.sup.drive_step(&task, &step(1, "", &[])))
        .is_err());
    assert!(
        w.daemon.calls.lock().unwrap().made.is_empty(),
        "no model call once stopped"
    );
    rt.block_on(w.sup.drive_end(&task, "cancelled", "You took it back."));
    let tasks = rows(&w, "SELECT status FROM tasks");
    assert!(tasks.iter().all(|t| t[0] == "cancelled"), "{tasks:?}");
    assert!(!w.sup.running_trees().iter().any(|(t, _)| *t == task));
}
