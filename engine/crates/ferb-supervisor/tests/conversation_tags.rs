//! Front door step 8 (kb/plans/front-door.md "The card and the library", "Conversation lines"): the conversation
//! lines lose what the model does not need. `[cN]` stays and is the entry's id; no 32-hex id is in a line; an origin
//! list becomes "has outside content" (or nothing); a lookup that names an entry, a result or its task by its tag gets
//! the id it stands for in that turn. Code keeps every origin as before: only the text changes (the block's origins
//! are unchanged). A Python recording's replay keeps today's lines. Through the real supervisor on the recorded
//! plain_reply scenario with stand-in models (temp store, injected ids and time).

mod common;
mod support_control;

use std::sync::atomic::Ordering;

use serde_json::{json, Value};
use support_control::{run_with, world_with, World};

const MSG: &str = "0123456789abcdef0123456789abcd01";
const ANSWER: &str = "0123456789abcdef0123456789abcd02";
const TASK: &str = "0123456789abcdef0123456789abcd03";
const REPLY: &str = "0123456789abcdef0123456789abcd04";

fn row(w: &World, id: &str, at: &str, kind: &str, data: Value, made: &str) {
    let db = w.db.lock().unwrap();
    ferb_supervisor::flow::store::journal_row(db.conn(), id, at, kind, &data, Some(made)).unwrap();
}

/// The chat `s1`: the owner's message, FERB's reply, and a task's result that read a web page.
fn seed(w: &World) {
    row(
        w,
        MSG,
        "2026-01-01T08:00:00+00:00",
        "chat.message",
        json!({"session_id": "s1", "request_id": "seedrequest1", "text": "INVENTED-ASK find the invented shop's hours"}),
        "owner",
    );
    row(
        w,
        REPLY,
        "2026-01-01T08:00:10+00:00",
        "chat.reply",
        json!({"session_id": "s1", "request_id": "seedrequest1", "text": "INVENTED-ACK Looking it up."}),
        "ferb,inferred",
    );
    row(
        w,
        ANSWER,
        "2026-01-01T08:01:00+00:00",
        "task.answer",
        json!({"task_id": TASK, "request_id": "seedrequest1", "status": "done",
               "text": "INVENTED-RESULT The invented shop opens at nine."}),
        "external:web:shop.example,ferb,inferred",
    );
    // the task is this chat's (continuity reads its task_context row)
    let db = w.db.lock().unwrap();
    db.conn()
        .execute("INSERT OR IGNORE INTO conversations(id) VALUES ('s1')", [])
        .unwrap();
    db.conn()
        .execute(
            "INSERT INTO tasks (id, tree_id, parent_id, request_id, goal, identity, role, scopes_json, status, \
             created_at, updated_at) VALUES (?, ?, NULL, 'seedrequest1', 'Find the invented shop hours', 'owner', \
             'mixed', '[]', 'done', '2026-01-01T08:00:20+00:00', '2026-01-01T08:01:00+00:00')",
            rusqlite::params![TASK, TASK],
        )
        .unwrap();
    db.conn()
        .execute(
            "INSERT INTO task_context (task_id, conversation_id, spec_json, owner_words) VALUES (?, 's1', '{}', '')",
            rusqlite::params![TASK],
        )
        .unwrap();
}

/// The recorded plain_reply with its planner call's events replaced by `events`.
fn world(bounded: bool, front: bool, events: Option<Value>) -> World {
    let mut w = world_with("plain_reply", move |calls| {
        if let Some(events) = events {
            for c in calls.iter_mut().filter(|c| c["key"] == "supervisor") {
                c["events"] = events.clone();
            }
        }
    });
    w.daemon.bounded.store(bounded, Ordering::SeqCst);
    w.daemon.front.store(front, Ordering::SeqCst);
    seed(&w);
    let _ = run_with(&mut w, "plain_reply", |_| {});
    w.sup.chat_flush();
    w
}

fn first_planner_parts(w: &World) -> Vec<String> {
    let calls = w.daemon.calls.lock().unwrap();
    calls
        .started
        .iter()
        .find(|c| calls.made[c["n"].as_u64().unwrap() as usize]["runtime_role"] == "orchestrator")
        .expect("a planner call")["parts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p.as_str().unwrap_or_default().to_string())
        .collect()
}

// decision: ORCH-726, ORCH-746
#[test]
fn the_lines_hold_no_32_hex_id_and_say_only_that_an_entry_has_outside_content() {
    let w = world(true, false, None);
    let parts = first_planner_parts(&w);
    let all = parts.join("\n");
    for id in [MSG, ANSWER, TASK, REPLY] {
        assert!(!all.contains(id), "{id} is in the planner's input:\n{all}");
    }
    let lines: Vec<&String> = parts.iter().filter(|p| p.starts_with("[c")).collect();
    assert_eq!(lines.len(), 3, "{lines:#?}");
    assert!(
        lines[0].starts_with("[c1] (Thu 01 Jan 08:00) Owner: INVENTED-ASK"),
        "{}",
        lines[0]
    );
    assert!(
        lines[2]
            .starts_with("[c3] (Thu 01 Jan 08:01) FERB (task result, done; has outside content): "),
        "{}",
        lines[2]
    );
    assert!(
        !all.contains("origin external")
            && !all.contains("Full source:")
            && !all.contains("Result id:"),
        "{all}"
    );
    assert!(
        all.contains(ferb_supervisor::words::TAGS_ARE_IDS.trim_start()),
        "{all}"
    );
}

// decision: ORCH-726, ORCH-746
#[test]
fn a_python_recordings_replay_keeps_todays_lines() {
    let w = world(false, false, None);
    let all = first_planner_parts(&w).join("\n");
    assert!(
        all.contains(&format!("chat.history entry_id={MSG}")),
        "{all}"
    );
    assert!(
        all.contains(&format!(
            "Result id: {ANSWER}; task.evidence task_id={TASK}"
        )),
        "{all}"
    );
    assert!(all.contains("origin external"), "{all}");
}

// decision: ORCH-726, ORCH-746
#[test]
fn a_lookup_names_an_entry_a_result_or_its_task_by_its_tag() {
    let tagged =
        |value: &str, named: &str| json!({"kind": "tagged", "value": value, "named": named});
    let events = json!([
        tagged("c1", "entry"),
        tagged("[c3]", "result"),
        tagged("C3", "task"),
        tagged("c2", "task"),
        tagged("c99", "entry"),
        tagged("not a tag", "entry"),
        {"kind": "text_delta", "data": {"text": "Hello."}},
        {"kind": "usage", "data": {"input_tokens": 10, "output_tokens": 5}},
        {"kind": "final", "data": {"text": "Hello."}}
    ]);
    let w = world(true, true, Some(events));
    let got = w.daemon.tagged_results.lock().unwrap().clone();
    assert_eq!(
        got,
        vec![
            Some(MSG.to_string()),
            Some(ANSWER.to_string()),
            Some(TASK.to_string()),
            None,
            None,
            None
        ]
    );
    // once the turn is over no tag names anything (a tag is the id of what that turn was shown)
    assert_eq!(
        w.sup.inner().tagged(
            "id-000015.lookups",
            "c1",
            ferb_supervisor::flow::library::Named::Entry
        ),
        None
    );
}
