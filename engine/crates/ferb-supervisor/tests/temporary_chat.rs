//! ORCH-699..702: the temporary chat. A `chat.send` with `fresh` runs with FERB's tools and safety unchanged but
//! with no prior context (no Tier 1 profile, nothing newly always on, no memory, no lookups or tools that read what FERB
//! knows), with the temporary chat's fact and the owner's `extra_context`; nothing of it is kept (no chat entries, no
//! task results, no episodes, its conversation rows forgotten when it ends) while the traces and audit rows stay.
//! Scripted on the recorded one_step_task scenario (planner, worker, checker; fake models, temp store, injected ids
//! and time).

mod common;
mod support_control;

use ferb_context::assemble::ContextBlock;
use serde_json::{json, Value};
use support_control::{run_with, world_with, World};

const PROFILE: &str = "About the owner (always on): INVENTED-PROFILE-LINE";
const NEWLY_ON: &str = "INVENTED-NEWLY-ALWAYS-ON";
const MEMORY: &str = "INVENTED-MEMORY-ITEM about the owner";
const EXTRA: &str = "INVENTED-EXTRA-CONTEXT for this chat";

fn world() -> World {
    let w = world_with("one_step_task", |_| {});
    *w.daemon.profile.lock().unwrap() = PROFILE.into();
    *w.daemon.newly_on.lock().unwrap() = NEWLY_ON.into();
    *w.daemon.memory.lock().unwrap() = vec![ContextBlock::new(MEMORY, "memory item")];
    w
}

/// Runs the scenario, its chat.send changed by `params`.
fn run(w: &mut World, params: Value) {
    let _ = run_with(w, "one_step_task", |steps| {
        for st in steps.iter_mut().filter(|st| st["do"] == "stream") {
            let mut sent: Value =
                serde_json::from_str(st["lines"][0]["send"].as_str().unwrap()).unwrap();
            for (k, v) in params.as_object().unwrap() {
                sent["params"][k] = v.clone();
            }
            st["lines"][0]["send"] = json!(sent.to_string());
        }
    });
    w.sup.chat_flush();
}

fn count(w: &World, sql: &str) -> i64 {
    w.db.lock()
        .unwrap()
        .conn()
        .query_row(sql, [], |r| r.get(0))
        .unwrap()
}

/// Every model call's (system, parts joined).
fn calls(w: &World) -> Vec<(String, String)> {
    w.daemon
        .calls
        .lock()
        .unwrap()
        .started
        .iter()
        .map(|c| {
            let parts: Vec<String> = c["parts"]
                .as_array()
                .unwrap()
                .iter()
                .filter_map(|p| p.as_str().map(str::to_string))
                .collect();
            (
                c["system"].as_str().unwrap_or("").to_string(),
                parts.join("\n"),
            )
        })
        .collect()
}

// decision: ORCH-699
#[test]
fn a_kept_turn_reads_the_profile_and_memory_the_temporary_one_does_not() {
    // the control: the same scenario, kept, reads all three (so the fresh test below can see their absence)
    let mut w = world();
    run(&mut w, json!({}));
    let made = calls(&w);
    assert!(made.len() >= 2, "planner and worker ran: {}", made.len());
    assert!(made.iter().any(|(s, _)| s.contains(PROFILE)), "the profile");
    assert!(made.iter().any(|(_, p)| p.contains(NEWLY_ON)), "newly on");
    assert!(made.iter().any(|(_, p)| p.contains(MEMORY)), "memory");
    assert!(!made
        .iter()
        .any(|(_, p)| p.contains(ferb_supervisor::flow::temporary::FRESH_FACT)));
}

// decision: ORCH-699
// decision: ORCH-701
#[test]
fn a_fresh_turn_gets_no_prior_context_and_keeps_its_tools() {
    let mut w = world();
    run(&mut w, json!({"fresh": true, "extra_context": EXTRA}));
    let made = calls(&w);
    let roles: Vec<String> = w
        .daemon
        .calls
        .lock()
        .unwrap()
        .made
        .iter()
        .map(|m| m["runtime_role"].as_str().unwrap_or("").to_string())
        .collect();
    assert!(
        roles.iter().any(|r| r == "orchestrator") && roles.iter().any(|r| r.starts_with("worker")),
        "the planner and a worker ran: {roles:?}"
    );
    assert!(
        roles.iter().any(|r| r.starts_with("checker")),
        "and its checker: {roles:?}"
    );
    assert_eq!(made.len(), roles.len(), "every runtime made was called");
    for (n, (system, parts)) in made.iter().enumerate() {
        assert!(!system.contains(PROFILE), "call {n}: no Tier 1 profile");
        assert!(
            !parts.contains(NEWLY_ON),
            "call {n}: nothing newly always on"
        );
        assert!(!parts.contains(MEMORY), "call {n}: no memory");
        assert!(
            parts.contains(ferb_supervisor::flow::temporary::FRESH_FACT),
            "call {n}: the temporary chat's fact"
        );
        assert!(parts.contains(EXTRA), "call {n}: the owner's extra context");
        assert!(parts.contains("say hello"), "call {n}: the owner's words");
    }
    // the planner keeps only the lookups about FERB and this Mac
    let lookups: Vec<String> = w.daemon.calls.lock().unwrap().made[0]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t.as_str().unwrap().to_string())
        .collect();
    for t in &lookups {
        assert!(
            ferb_supervisor::flow::temporary::FRESH_PLANNER_TOOLS.contains(&t.as_str()),
            "{t}"
        );
    }
    // the workers' tokens name every tool but the ones that read what FERB knows (the gateway refuses the rest)
    let only = w.daemon.only.lock().unwrap().clone();
    let worker_lists: Vec<&Vec<String>> = only.values().filter(|l| l.len() > 5).collect();
    assert!(!worker_lists.is_empty(), "a worker's token lists its tools");
    for list in worker_lists {
        for hidden in ferb_supervisor::flow::temporary::HIDDEN_TOOLS {
            assert!(!list.iter().any(|t| t == hidden), "{hidden}");
        }
        assert!(
            list.iter().any(|t| t.starts_with("fs.")),
            "the normal tools stay"
        );
    }
}

// decision: ORCH-699
// decision: ORCH-702
#[test]
fn a_fresh_turn_keeps_nothing_but_the_traces_and_audit() {
    let mut w = world();
    run(&mut w, json!({"fresh": true}));
    let inner = w.sup.inner().clone();
    // held in memory: its message and answer, as entries
    let snap = inner.temporary_snapshot();
    assert_eq!(snap["on"], true, "{snap}");
    let conversation = snap["conversation"].as_str().unwrap().to_string();
    assert!(conversation.starts_with("temporary:"));
    let entries = snap["entries"].as_array().unwrap();
    assert!(
        entries
            .iter()
            .any(|e| e["kind"] == "message" && e["body"]["text"] == "say hello"),
        "{snap}"
    );
    assert!(entries.len() >= 2, "the message and what FERB said: {snap}");
    assert!(inner.temporary_status().is_some());
    // never written: chat entries, submissions, task results, episodes; no task.result on the bus
    assert_eq!(count(&w, "SELECT COUNT(*) FROM chat_entries"), 0);
    assert_eq!(count(&w, "SELECT COUNT(*) FROM chat_submissions"), 0);
    let tasks = count(
        &w,
        "SELECT COUNT(*) FROM tasks WHERE request_id LIKE 'temporary-%'",
    );
    assert!(tasks >= 1, "its task ran (and its record stays)");
    assert_eq!(count(&w, "SELECT COUNT(*) FROM task_results"), 0);
    assert_eq!(count(&w, "SELECT COUNT(*) FROM episodes"), 0);
    assert!(
        w.daemon.episodes.lock().unwrap().is_empty(),
        "no episode capture"
    );
    let published = w.daemon.published.lock().unwrap().clone();
    assert!(!published.iter().any(|(t, _)| t == "task.result"));
    assert!(published
        .iter()
        .any(|(t, d)| t == "chat.temporary" && d["on"] == true));
    // the traces stay
    let spans = count(
        &w,
        "SELECT COUNT(*) FROM spans WHERE task_id IN (SELECT id FROM tasks WHERE request_id LIKE 'temporary-%')",
    );
    assert!(spans >= 1, "its traces");
    // the Tasks page never lists it
    let page = ferb_store::task_results::TaskResults::new(&w.db.lock().unwrap())
        .page("9999", 50)
        .unwrap();
    assert!(page.is_empty(), "{page:?}");
    // the icon off: its conversation goes from memory and the store; traces, tasks and audit rows stay
    let audit_before = count(&w, "SELECT COUNT(*) FROM journal WHERE kind = 'chat.ack'");
    assert_eq!(inner.temporary_end(), json!({"on": false}));
    assert_eq!(inner.temporary_snapshot(), json!({"on": false}));
    assert!(inner.temporary_status().is_none());
    let content = ferb_store::data_view::CONTENT_KINDS
        .iter()
        .map(|k| format!("'{k}'"))
        .collect::<Vec<_>>()
        .join(",");
    assert_eq!(
        count(
            &w,
            &format!(
                "SELECT COUNT(*) FROM journal WHERE kind IN ({content}) AND (json_extract(data_json, '$.request_id') \
                 LIKE 'temporary-%' OR json_extract(data_json, '$.session_id') LIKE 'temporary:%' OR \
                 json_extract(data_json, '$.task_id') IN (SELECT id FROM tasks WHERE request_id LIKE 'temporary-%'))"
            )
        ),
        0,
        "its conversation rows are forgotten"
    );
    assert_eq!(
        count(
            &w,
            "SELECT COUNT(*) FROM conversations WHERE id LIKE 'temporary:%'"
        ),
        0
    );
    assert_eq!(
        count(
            &w,
            "SELECT COUNT(*) FROM task_context WHERE conversation_id LIKE 'temporary:%'"
        ),
        0
    );
    assert_eq!(
        count(&w, "SELECT COUNT(*) FROM journal WHERE kind = 'chat.ack'"),
        audit_before,
        "the journal's other rows stay"
    );
    assert_eq!(
        count(
            &w,
            "SELECT COUNT(*) FROM tasks WHERE request_id LIKE 'temporary-%'"
        ),
        tasks
    );
    assert!(count(&w, "SELECT COUNT(*) FROM spans") >= spans);
    let published = w.daemon.published.lock().unwrap().clone();
    assert!(published
        .iter()
        .any(|(t, d)| t == "chat.temporary" && d["on"] == false));
}

// decision: ORCH-700
#[test]
fn the_switch_is_held_until_the_icon_and_a_message_starts_it() {
    let w = world();
    let inner = w.sup.inner().clone();
    assert_eq!(inner.temporary_snapshot(), json!({"on": false}));
    let on = inner.temporary_start(None);
    assert_eq!(on["on"], true);
    let id = on["id"].clone();
    // on again (a second client, a relaunch reading it): the same chat
    let again = inner.temporary_start(Some("later"));
    assert_eq!(again["id"], id);
    assert_eq!(again["extra_context"], "later");
    assert_eq!(inner.temporary_snapshot()["id"], id);
    assert_eq!(inner.temporary_end(), json!({"on": false}));
    // a new one is a new chat
    let next = inner.temporary_start(None);
    assert_ne!(next["id"], id);
    assert!(next.get("extra_context").is_none());
    assert!(next["rev"].as_u64().unwrap() > again["rev"].as_u64().unwrap());
    inner.temporary_end();
}

// decision: ORCH-699
#[test]
fn fresh_and_extra_context_are_checked_and_a_temporary_session_is_never_named() {
    let w = world();
    let refused = |params: Value| {
        w.sup
            .chat_send_params(params.as_object().unwrap())
            .err()
            .map(|r| r.0)
    };
    assert_eq!(
        refused(json!({"text": "hi", "fresh": "yes"})).as_deref(),
        Some("fresh must be true or false")
    );
    assert_eq!(
        refused(json!({"text": "hi", "fresh": true, "extra_context": 3})).as_deref(),
        Some("extra_context must be text")
    );
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .start_paused(true)
        .build()
        .unwrap();
    rt.block_on(async {
        assert_eq!(
            refused(json!({"text": "hi", "session_id": "temporary:abc"})).as_deref(),
            Some("session_id names one of FERB's own sessions, not a chat")
        );
    });
}
