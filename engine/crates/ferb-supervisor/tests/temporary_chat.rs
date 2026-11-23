//! ORCH-699..702: the temporary chat. A `chat.send` with `fresh` runs with FERB's tools and safety unchanged but
//! with no prior context (no Tier 1 profile, nothing newly always on, no memory, no lookups or tools that read what FERB
//! knows), with the temporary chat's fact and the owner's `extra_context`; nothing of it is kept (no chat entries, no
//! task results, no episodes, its conversation rows forgotten when it ends) while the traces and audit rows stay.
//! Scripted on the recorded one_step_task scenario (planner, worker, checker; fake models, temp store, injected ids
//! and time).

mod common;
mod support_control;

use ferb_context::assemble::ContextBlock;
use ferb_supervisor::flow::owner_wait::{Ask, Questions};
use ferb_supervisor::flow::FrontSeen;
use serde_json::{json, Value};
use support_control::{run_with, world_with, World};

const PROFILE: &str = "About the owner (always on): INVENTED-PROFILE-LINE";
const NEWLY_ON: &str = "INVENTED-NEWLY-ALWAYS-ON";
const MEMORY: &str = "INVENTED-MEMORY-ITEM about the owner";
const EXTRA: &str = "INVENTED-EXTRA-CONTEXT for this chat";

/// The kept chat's history and FERB's own records of the owner, seeded before the run (owner 2026-11-23's live case:
/// a temporary chat's planner was given the kept chat's notices and posts, the schedules and reminders, and the
/// screen). Each is a distinct marker a built request must not carry when fresh.
const KEPT_MESSAGE: &str = "INVENTED-KEPT-CHAT-MESSAGE";
const KEPT_REPLY: &str = "INVENTED-KEPT-CHAT-REPLY";
const NOTICE: &str = "INVENTED-NOTICE-WITHOUT-CONVERSATION";
const NOTICE_ID: &str = "seednotice0001";
const REMINDER_ARRIVED: &str = "INVENTED-REMINDER-ARRIVED";
const TIMER_ID: &str = "seedtimer00001";
const SCHEDULED_RESULT: &str = "INVENTED-SCHEDULED-RUN-RESULT";
const POSTED_ID: &str = "seedposted0001";
const SCHEDULES: &str =
    "The owner's schedules: 1\n- Schedule sc-INVENTED0001 \u{201c}INVENTED-SCHEDULE\u{201d}";
const REMINDERS: &str = "The owner's pending reminders: 1\n- Reminder tm-INVENTED0002 \u{201c}INVENTED-REMINDER\u{201d}";
const MEANING: &str =
    "- When the owner says \u{201c}INVENTED-PHRASE\u{201d}, they mean: INVENTED-MEANING";
const WINDOW: &str = "INVENTED-FRONT-WINDOW-TITLE";
const SHOT: &str = "INVENTED-SCREENSHOT-EVIDENCE";
const KEPT_QUESTION: &str = "INVENTED-KEPT-CHAT-QUESTION";

/// Every marker of what FERB knew before a temporary chat.
const PRIOR: [&str; 14] = [
    KEPT_MESSAGE,
    KEPT_REPLY,
    NOTICE,
    NOTICE_ID,
    REMINDER_ARRIVED,
    TIMER_ID,
    SCHEDULED_RESULT,
    POSTED_ID,
    "sc-INVENTED0001",
    "tm-INVENTED0002",
    "INVENTED-MEANING",
    WINDOW,
    SHOT,
    KEPT_QUESTION,
];

fn world() -> World {
    seeded("one_step_task")
}

/// The scenario's world with a test profile, newly-always-on line and memory, and the kept chat's history, the
/// owner's schedules and reminders, a fact from memory, a screen captured at send and an open question of a kept task.
fn seeded(name: &str) -> World {
    let w = world_with(name, |_| {});
    *w.daemon.profile.lock().unwrap() = PROFILE.into();
    *w.daemon.newly_on.lock().unwrap() = NEWLY_ON.into();
    *w.daemon.memory.lock().unwrap() = vec![ContextBlock::new(MEMORY, "memory item")];
    *w.daemon.owner_state.lock().unwrap() = format!("{SCHEDULES}\n{REMINDERS}");
    *w.daemon.meanings.lock().unwrap() = MEANING.into();
    *w.daemon.capture.lock().unwrap() = Some(FrontSeen {
        facts:
            json!({"app": "TextEdit", "bundle_id": "com.apple.TextEdit", "window_title": WINDOW})
                .as_object()
                .unwrap()
                .clone(),
        image: Some(ferb_core::types::ModelImage::new(vec![1, 2, 3], SHOT)),
        origin: "external:unlabeled".into(),
    });
    {
        let db = w.db.lock().unwrap();
        let at = "2025-12-31T08:00:00+00:00";
        let rows = [
            (
                "seedmessage001",
                "chat.message",
                json!({"session_id": "s1", "request_id": "seedrequest01", "text": KEPT_MESSAGE}),
                "owner",
            ),
            (
                "seedreply00001",
                "chat.reply",
                json!({"session_id": "s1", "request_id": "seedrequest01", "text": KEPT_REPLY}),
                "ferb",
            ),
            (
                NOTICE_ID,
                "chat.notice",
                json!({"text": NOTICE, "quotes": []}),
                "ferb",
            ),
            (
                TIMER_ID,
                "timer.delivered",
                json!({"kind": "reminder", "message": REMINDER_ARRIVED}),
                "owner",
            ),
            (
                POSTED_ID,
                "schedule.posted",
                json!({"name": "INVENTED-SCHEDULE", "status": "done", "text": SCHEDULED_RESULT}),
                "ferb",
            ),
        ];
        for (id, kind, data, origin) in rows {
            ferb_supervisor::flow::store::journal_row(db.conn(), id, at, kind, &data, Some(origin))
                .unwrap();
        }
    }
    let _ = w.daemon.questions.lock().unwrap().ask(&Ask {
        capability: "owner.ask".into(),
        task_id: "seedtask00001.worker".into(),
        request_id: "seedrequest01".into(),
        question: KEPT_QUESTION.into(),
        options: vec!["Yes".into(), "No".into()],
        ..Ask::default()
    });
    w
}

/// Runs the scenario, its chat.send changed by `params`.
fn run(w: &mut World, params: Value) {
    run_named(w, "one_step_task", params)
}

/// Runs a recorded scenario, its every chat.send changed by `params`.
fn run_named(w: &mut World, name: &str, params: Value) {
    let _ = run_with(w, name, |steps| {
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

/// Every model call's (system, parts joined, images attached).
fn built(w: &World) -> Vec<(String, String, u64)> {
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
                c["images"].as_u64().unwrap_or(0),
            )
        })
        .collect()
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

/// Each planner call's parts joined, in order (the runtimes made for the orchestrator role).
fn planner_parts(w: &World) -> Vec<String> {
    let calls = w.daemon.calls.lock().unwrap();
    calls
        .started
        .iter()
        .filter(|c| {
            let n = c["n"].as_u64().unwrap() as usize;
            calls.made[n]["runtime_role"] == "orchestrator"
        })
        .map(|c| {
            c["parts"]
                .as_array()
                .unwrap()
                .iter()
                .filter_map(|p| p.as_str())
                .collect::<Vec<_>>()
                .join("\n")
        })
        .collect()
}

// decision: ORCH-699
#[test]
fn a_kept_turn_reads_the_kept_chat_the_owner_state_and_the_screen() {
    // the control for the test below: the same seeds reach a kept turn, so their absence there is the switch
    let mut w = world();
    run(&mut w, json!({}));
    let made = built(&w);
    let planner = &planner_parts(&w)[0];
    for m in [
        KEPT_MESSAGE,
        KEPT_REPLY,
        NOTICE,
        NOTICE_ID,
        REMINDER_ARRIVED,
        TIMER_ID,
        SCHEDULED_RESULT,
        POSTED_ID,
        "sc-INVENTED0001",
        "tm-INVENTED0002",
        WINDOW,
        KEPT_QUESTION,
    ] {
        assert!(planner.contains(m), "the kept planner reads {m}");
    }
    assert!(made[0].2 >= 1, "the planner is given the screenshot");
    assert!(
        made.iter().any(|(s, _, _)| s.contains("INVENTED-MEANING")),
        "the fact from memory"
    );
    assert!(
        made.iter()
            .any(|(s, _, _)| s.contains("sc-INVENTED0001") && s.contains("tm-INVENTED0002")),
        "a worker's instructions carry the schedules and reminders"
    );
    assert!(w.daemon.fresh_reads.lock().unwrap().iter().all(|f| !f));
}

// decision: ORCH-699
#[test]
fn a_fresh_turn_reads_no_kept_chat_owner_state_or_screen() {
    let mut w = world();
    run(&mut w, json!({"fresh": true}));
    let made = built(&w);
    assert!(
        made.len() >= 3,
        "planner, worker and checker: {}",
        made.len()
    );
    for (n, (system, parts, images)) in made.iter().enumerate() {
        for m in PRIOR {
            assert!(!system.contains(m), "call {n}'s instructions carry {m}");
            assert!(!parts.contains(m), "call {n}'s context carries {m}");
        }
        assert_eq!(*images, 0, "call {n}: no screenshot");
    }
    let planner = &planner_parts(&w)[0];
    assert!(planner.contains("say hello"), "the owner's words");
    assert!(
        planner.contains("Layer 1"),
        "this Mac's and the browsers' facts stay: {planner}"
    );
    let reads = w.daemon.fresh_reads.lock().unwrap().clone();
    assert!(
        !reads.is_empty() && reads.iter().all(|f| *f),
        "every role's instructions are read fresh: {reads:?}"
    );
}

// decision: ORCH-699
#[test]
fn a_temporary_chat_reads_its_own_earlier_turn_and_nothing_kept() {
    let mut w = seeded("clarify_then_answer");
    run_named(&mut w, "clarify_then_answer", json!({"fresh": true}));
    let planners = planner_parts(&w);
    assert!(planners.len() >= 2, "two turns: {}", planners.len());
    for (n, p) in planners.iter().enumerate() {
        for m in PRIOR {
            assert!(!p.contains(m), "planner call {n} carries {m}");
        }
    }
    // its own earlier turn is its conversation (Layer 5), as for any chat
    let second = &planners[1];
    let from = second
        .find("Layer 5 \u{2014} conversation:")
        .expect("a conversation");
    let to = from + second[from..].find("Layer 2").unwrap();
    let conversation = &second[from..to];
    assert!(
        conversation.contains("Owner: find me a table tonight")
            && conversation.contains("FERB: Which restaurant, and for what time?"),
        "the second turn reads the temporary chat's first: {conversation}"
    );
}

struct NoReads;

impl ferb_supervisor::history::TimelineReads for NoReads {
    fn outcome_json(&self, _task_id: &str) -> Option<String> {
        None
    }
    fn notice_sources(&self, _notice_id: &str) -> Vec<String> {
        Vec::new()
    }
}

// decision: ORCH-699
#[test]
fn a_temporary_conversation_holds_only_its_own_rows() {
    // the conversation layer every reader builds (the planner, the fold): a notice with no conversation and FERB's own
    // posts (a scheduled run's result, a reminder) are the kept chat's, never a temporary chat's
    use ferb_supervisor::history::{timeline, Timeline};
    use ferb_supervisor::seams::JournalRow;
    let row = |id: &str, kind: &str, data: Value| {
        JournalRow::from_json(
            &json!({"id": id, "at": "2026-01-01T08:00:00+00:00", "kind": kind,
                                      "data": data, "origin": "ferb"}),
        )
        .unwrap()
    };
    let rows = vec![
        row(
            "k1",
            "chat.message",
            json!({"session_id": "s1", "request_id": "r1", "text": KEPT_MESSAGE}),
        ),
        row("n1", "chat.notice", json!({"text": NOTICE, "quotes": []})),
        row(
            "t1",
            "timer.delivered",
            json!({"kind": "reminder", "message": REMINDER_ARRIVED}),
        ),
        row(
            "p1",
            "schedule.posted",
            json!({"name": "x", "status": "done", "text": SCHEDULED_RESULT}),
        ),
        row(
            "m1",
            "chat.message",
            json!({"session_id": "temporary:abc", "request_id": "temporary-r2", "text": "my earlier temporary words"}),
        ),
    ];
    let zone = ferb_clock::zone::Zone::utc();
    let read = |conversation: &str| -> Vec<String> {
        let (entries, _) = timeline(&Timeline {
            current: &Default::default(),
            conversation: Some(conversation),
            rows: &rows,
            task_ids: Some(&Default::default()),
            covered: &Default::default(),
            reads: &NoReads,
            zone: &zone,
        });
        entries.into_iter().map(|e| e.text).collect()
    };
    let temporary = read("temporary:abc").join("\n");
    assert!(
        temporary.contains("my earlier temporary words"),
        "{temporary}"
    );
    for m in [KEPT_MESSAGE, NOTICE, REMINDER_ARRIVED, SCHEDULED_RESULT] {
        assert!(!temporary.contains(m), "{m}: {temporary}");
    }
    let kept = read("s1").join("\n");
    for m in [KEPT_MESSAGE, NOTICE, REMINDER_ARRIVED, SCHEDULED_RESULT] {
        assert!(kept.contains(m), "the kept chat reads {m}: {kept}");
    }
    assert!(!kept.contains("my earlier temporary words"));
    // the conversation store's rule is the same one (a temporary chat's tasks never reach the schedules' runs)
    assert!(ferb_orchestrator::continuity::reserved("temporary:abc"));
    assert!(!ferb_orchestrator::continuity::reserved("s1"));
}
