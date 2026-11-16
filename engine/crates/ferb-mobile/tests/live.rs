//! The live state over the folded transcript (ENG-8): frames that only show while they happen, never stored, gone at
//! the turn's answer and on a restart of the watch. Driven by the fixtures' own recorded frames, and by the cases of
//! FERB.app's EntryWatchTests (a phone's turn: its planning line, ended by the plan; its answer ends everything).

mod common;

use std::collections::HashMap;

use common::{read, scenarios};
use ferb_mobile::entries::EntrySet;
use ferb_mobile::entry::ChatEntry;
use ferb_mobile::live::LiveState;
use serde_json::{json, Value};

/// The frames the engine passes as live state only (ferb-supervisor flow/entries.rs `LIVE_FRAMES`).
const LIVE: [&str; 11] = [
    "progress",
    "step",
    "subtask",
    "verification",
    "memory",
    "timer",
    "plan_delta",
    "plan_abort",
    "plan_end",
    "reply_delta",
    "reply_abort",
];

/// Every recorded timeline's live frames, in their order, through the reducer; each stream's stored entries arrive
/// at its end, and all of them at the last (a catch-up). Then nothing live is left for a turn that has its answer,
/// and live frames never change the transcript.
// decision: ENG-8
// decision: ENG-10
// decision: PHN-473
#[test]
fn the_recorded_frames_leave_nothing_live_once_a_turn_is_answered() {
    let (mut applied, mut steps_applied) = (0, 0);
    for file in ["scenarios.json", "engine_chat.json", "engine_more.json"] {
        for s in read(file)["scenarios"].as_array().unwrap() {
            let name = s["name"].as_str().unwrap();
            let entries = ChatEntry::list(&s["entries"]);
            let set = EntrySet::from_entries(entries.clone());
            let before = set.transcript(0);
            let mut state = LiveState::new();
            let mut stream_request: HashMap<i64, String> = HashMap::new();
            for step in s["timeline"].as_array().unwrap() {
                let stream = step["stream"].as_i64().unwrap_or(-1);
                match step["do"].as_str().unwrap_or_default() {
                    "send" => {
                        if let Some(r) = step["request_id"].as_str() {
                            stream_request.insert(stream, r.to_string());
                        }
                    }
                    "frame" => {
                        let frame = &step["frame"];
                        let ty = frame["type"].as_str().unwrap_or_default();
                        if !LIVE.contains(&ty) {
                            continue;
                        }
                        let request = frame["request_id"]
                            .as_str()
                            .map(str::to_string)
                            .or_else(|| stream_request.get(&stream).cloned());
                        if state.frame(request.as_deref(), frame) {
                            applied += 1;
                            if ty == "step" {
                                steps_applied += 1;
                            }
                        }
                    }
                    "end" => {
                        if let Some(r) = stream_request.get(&stream) {
                            for e in entries
                                .iter()
                                .filter(|e| e.request_id.as_deref() == Some(r))
                            {
                                state.entry(e);
                            }
                        }
                    }
                    "restart" => {
                        state.reset();
                    }
                    _ => {}
                }
            }
            for e in &entries {
                state.entry(e);
            }
            assert_eq!(
                set.transcript(0),
                before,
                "{file} {name}: live frames never change the transcript"
            );
            let answered: Vec<&ChatEntry> = entries
                .iter()
                .filter(|e| {
                    !e.deleted
                        && (e.kind == "answer" || e.kind == "error")
                        && !e.body.get("follows").is_some_and(Value::is_string)
                })
                .collect();
            for e in answered {
                let r = e.request_id.as_deref().unwrap_or_default();
                assert!(
                    state.turn(r).is_none(),
                    "{file} {name}: {r} has its answer, yet live state stays"
                );
            }
            // what is drawn live is only for turns still waiting for their answer
            for v in state.view(&before) {
                let turn = before
                    .items
                    .iter()
                    .find(|i| i.entry() == v.entry)
                    .expect("a live view names a turn of the transcript");
                let ferb_mobile::fold::Item::Turn(t) = turn else {
                    panic!("{file} {name}: a live view on a non-turn")
                };
                assert!(t.status == "working" || t.status == "needs_user");
            }
        }
    }
    assert!(
        applied > 500,
        "the recorded frames reach the reducer ({applied})"
    );
    assert!(steps_applied > 100, "step frames apply ({steps_applied})");
    let _ = scenarios;
}

fn entry(id: &str, pos: i64, seq: i64, kind: &str, request: &str, body: Value) -> ChatEntry {
    ChatEntry::read(&json!({
        "id": id, "conversation_id": "S1", "pos": pos, "seq": seq, "rev": 1, "kind": kind,
        "state": if kind == "message" { "working" } else { "done" }, "open": kind == "message",
        "request_id": request, "task_id": if kind == "message" { Value::Null } else { json!("t-phone") },
        "client_id": "phone-1", "answers": if kind == "answer" { json!("p1") } else { Value::Null },
        "placement": if kind == "answer" { json!("inline") } else { Value::Null },
        "created_at": "2026-01-01T09:00:00+00:00", "updated_at": "2026-01-01T09:00:00+00:00",
        "day": "2026-01-01", "body": body, "deleted": false,
    }))
    .unwrap()
}

/// EntryWatchTests' phone turn: the typing dots after the message, the planning line, the plan's card with its
/// steps' live states, the line ended by the plan, everything ended by the answer.
// decision: ENG-8
// decision: APP-916
// decision: PHN-473
#[test]
fn a_turns_live_state_runs_from_its_message_to_its_answer() {
    let mut set = EntrySet::new();
    let mut live = LiveState::new();
    let message = entry(
        "p1",
        10,
        11,
        "message",
        "r-phone",
        json!({"text": "check the train times", "attachments": [], "hidden": []}),
    );
    set.apply(message.clone());
    live.entry(&message);
    let v = live.view(&set.transcript(0));
    assert_eq!(v.len(), 1);
    assert!(
        v[0].typing,
        "acknowledged, nothing said yet: the typing dots"
    );

    assert!(live.frame(
        Some("r-phone"),
        &json!({"type": "progress", "planning": true, "text": "Looking at the timetable",
                "at": "2026-01-01T09:00:01+00:00"})
    ));
    let v = live.view(&set.transcript(0));
    assert_eq!(v[0].line.as_ref().unwrap().text, "Looking at the timetable");

    let card = json!({"request_id": "r-phone", "task_id": "t-phone", "total": 2,
        "rows": [{"part": "a", "label": "Morning trains", "state": "working", "step": "s1"},
                 {"part": "b", "label": "Evening trains", "state": "working", "step": "s2"}]});
    let plan = entry(
        "p2",
        11,
        12,
        "plan",
        "r-phone",
        json!({"text": "I'll look up both.", "plan_line": "I'll look up both.", "lines": [], "card": card}),
    );
    set.apply(plan.clone());
    live.entry(&plan);
    let v = live.view(&set.transcript(0));
    assert!(v[0].line.is_none(), "the plan ends the live line");
    assert!(!v[0].typing);
    assert_eq!(v[0].rows[0].words.as_deref(), Some("Working"));

    live.frame(
        Some("r-phone"),
        &json!({"type": "step", "task_id": "t-phone", "step": "s1", "status": "running", "seq": "1"}),
    );
    live.frame(
        Some("r-phone"),
        &json!({"type": "step", "task_id": "t-phone", "step": "s2", "status": "waiting",
                "waits_on": ["s1"], "seq": "1"}),
    );
    // an older state never replaces a newer one
    live.frame(
        Some("r-phone"),
        &json!({"type": "step", "task_id": "t-phone", "step": "s1", "status": "starting", "seq": "0"}),
    );
    let v = live.view(&set.transcript(0));
    assert_eq!(v[0].rows[0].words.as_deref(), Some("Working"));
    assert!(v[0].rows[0].spinner);
    assert_eq!(
        v[0].rows[1].words.as_deref(),
        Some("Waiting for Morning trains")
    );
    assert_eq!(v[0].state, "running");

    let answer = entry(
        "p3",
        12,
        13,
        "answer",
        "r-phone",
        json!({"text": "Trains every 20 minutes.", "status": "done"}),
    );
    set.apply(answer.clone());
    assert!(live.entry(&answer));
    assert!(
        live.view(&set.transcript(0)).is_empty(),
        "answered: nothing live"
    );
    assert!(live.turn("r-phone").is_none());
}

/// A restart of the watch drops every live thing; the next frame starts it again.
// decision: ENG-8
// decision: PHN-473
#[test]
fn a_restarted_watch_has_nothing_live_until_the_next_frame() {
    let mut live = LiveState::new();
    live.frame(
        Some("r1"),
        &json!({"type": "plan_delta", "text": "Looking"}),
    );
    live.frame(
        Some("r1"),
        &json!({"type": "subtask", "summary": "Opened the page"}),
    );
    assert_eq!(live.turn("r1").unwrap().streaming_plan, "Looking");
    assert!(live.reset());
    assert!(live.turn("r1").is_none());
    live.frame(Some("r1"), &json!({"type": "verification"}));
    assert_eq!(
        live.turn("r1").unwrap().activity.as_deref(),
        Some("Checking the result")
    );
}

/// A withdrawn plan stream shows the typing dots again; a frame that shows nothing leaves no state behind.
// decision: ENG-8
// decision: PHN-473
#[test]
fn an_aborted_plan_stream_goes_back_to_the_dots() {
    let mut live = LiveState::new();
    live.frame(Some("r1"), &json!({"type": "plan_delta", "text": "Book"}));
    live.frame(Some("r1"), &json!({"type": "plan_abort"}));
    assert_eq!(live.turn("r1").unwrap().streaming_plan, "");
    assert!(!live.frame(Some("r2"), &json!({"type": "subtask", "state": "started"})));
    assert!(live.turn("r2").is_none());
}
