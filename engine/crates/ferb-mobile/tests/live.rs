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

/// The owner's one-part research task of 2026-11-18 02:16: a plan with no card and no checklist, then the worker's
/// step lines on the stream. The phone drew nothing; FERB.app's work row says what the task does now
/// (`TaskItem.currentLine`): "Starting task…", then each newest line, then "Checking the result", gone at the answer.
// decision: PHN-725
#[test]
fn a_one_part_task_says_what_it_does_now() {
    let mut set = EntrySet::new();
    let mut live = LiveState::new();
    let message = entry(
        "p1",
        10,
        11,
        "message",
        "r-one",
        json!({"text": "search online and then tell", "attachments": [], "hidden": []}),
    );
    set.apply(message.clone());
    live.entry(&message);
    assert_eq!(
        live.view(&set.transcript(0))[0].current,
        None,
        "no task yet: the typing dots say it"
    );
    let plan = entry(
        "p2",
        11,
        12,
        "plan",
        "r-one",
        json!({"text": "I'll research it.", "plan_line": "I'll research it.",
        "lines": [], "name": "Research automation detection"}),
    );
    set.apply(plan.clone());
    live.entry(&plan);
    let v = live.view(&set.transcript(0));
    assert_eq!(v[0].current.as_deref(), Some("Starting task…"));
    assert!(v[0].card_line.is_none());
    assert!(live.frame(Some("r-one"), &json!({"type": "progress", "step_task_id": "t-phone.worker",
        "text": "searching for extension detection", "started_at": "2026-01-01T09:00:05+00:00", "task_id": "t-phone"})));
    assert_eq!(
        live.view(&set.transcript(0))[0].current.as_deref(),
        Some("Searching for extension detection")
    );
    live.frame(Some("r-one"), &json!({"type": "verification"}));
    assert_eq!(
        live.view(&set.transcript(0))[0].current.as_deref(),
        Some("Checking the result")
    );
    let t = set.transcript(0);
    let turn = t
        .items
        .iter()
        .find_map(|i| match i {
            ferb_mobile::fold::Item::Turn(x) => Some(x),
            _ => None,
        })
        .unwrap();
    assert!(
        turn.plan_at.is_some(),
        "the work row's time counts from the plan"
    );
    let answer = entry(
        "p3",
        12,
        13,
        "answer",
        "r-one",
        json!({"text": "Yes, sometimes.", "status": "done"}),
    );
    set.apply(answer.clone());
    live.entry(&answer);
    assert!(live.view(&set.transcript(0)).is_empty());
}

/// A running card's one status line, as FERB.app's (`CardStatusLine`): "0 of 2 done · 1 running · 1 waiting".
// decision: PHN-726
#[test]
fn a_running_card_has_the_macs_status_line() {
    let mut set = EntrySet::new();
    let mut live = LiveState::new();
    set.apply(entry(
        "p1",
        10,
        11,
        "message",
        "r-c",
        json!({"text": "both", "attachments": [], "hidden": []}),
    ));
    let card = json!({"request_id": "r-c", "task_id": "t-phone", "total": 2,
        "rows": [{"part": "a", "label": "A", "state": "working", "step": "s1"},
                 {"part": "b", "label": "B", "state": "working", "step": "s2"}]});
    set.apply(entry(
        "p2",
        11,
        12,
        "plan",
        "r-c",
        json!({"text": "Both.", "plan_line": "Both.", "lines": [], "card": card}),
    ));
    live.frame(Some("r-c"), &json!({"type": "step", "task_id": "t-phone", "step": "s1", "status": "running", "seq": "1"}));
    live.frame(Some("r-c"), &json!({"type": "step", "task_id": "t-phone", "step": "s2", "status": "waiting", "waits_on": ["s1"], "seq": "1"}));
    let v = live.view(&set.transcript(0));
    assert_eq!(
        v[0].card_line.as_deref(),
        Some("0 of 2 done · 1 running · 1 waiting")
    );
}

/// APP-895: under a card row still waiting, the plan steps it waits on that no row answers, each with its own line.
// decision: PHN-736
#[test]
fn a_waiting_row_shows_the_steps_it_waits_on() {
    let mut set = EntrySet::new();
    let mut live = LiveState::new();
    set.apply(entry(
        "p1",
        10,
        11,
        "message",
        "r-f",
        json!({"text": "draft it", "attachments": [], "hidden": []}),
    ));
    let card = json!({"request_id": "r-f", "task_id": "t-phone", "total": 1,
        "plan": [{"id": "s1", "label": "Read the notes", "step_task_id": "t-phone.s1"},
                 {"id": "s2", "label": "Find the address", "step_task_id": "t-phone.s2", "depends_on": ["s1"]},
                 {"id": "s3", "label": "Write the email", "step_task_id": "t-phone.s3", "depends_on": ["s2"]}],
        "rows": [{"part": "a", "label": "Draft the email", "state": "working", "step": "s3"}]});
    set.apply(entry(
        "p2",
        11,
        12,
        "plan",
        "r-f",
        json!({"text": "On it.", "plan_line": "On it.", "lines": [], "card": card}),
    ));
    live.frame(Some("r-f"), &json!({"type": "step", "task_id": "t-phone", "step": "s3", "status": "waiting", "waits_on": ["s2"], "seq": "1"}));
    live.frame(Some("r-f"), &json!({"type": "step", "task_id": "t-phone", "step": "s1", "status": "running", "seq": "1"}));
    live.frame(
        Some("r-f"),
        &json!({"type": "progress", "step_task_id": "t-phone.s1", "text": "reading the notes"}),
    );
    let v = live.view(&set.transcript(0));
    let f = &v[0].feeders;
    assert_eq!(
        f.iter().map(|x| x.step.as_str()).collect::<Vec<_>>(),
        ["s1", "s2"]
    );
    assert_eq!(f[0].part, "a");
    assert_eq!(f[0].line.kind, "running");
    assert_eq!(f[0].line.words.as_deref(), Some("Reading the notes"));
    assert_eq!(f[1].line.kind, "upcoming");
}
