//! A stop's end is said once in the chat (owner 2026-11-22, ORCH-688: "didnt you said you fixed this redundant
//! response???"; live: the stop message's reply and the stopped task's own turn both showed "Stopped, as you asked.
//! This is where things stood: …" with the same card and sites chip, 13–17 ms apart). Each route through the real
//! supervisor with stand-in models (the recorded scenarios of engine/fixtures/daemon/chat_scenarios.json), and the chat
//! as the supervisor's own ChatLog kept it: exactly one entry says the end's words; the stopped task's turn shows its
//! ended state only (ORCH-689, APP-1113); the entries keep ENG-15's order rules. The restart's routes (its first turn,
//! a stop while it waits) are in tests/restart_by_follow_up.rs.

mod common;
mod support_control;
mod support_entries;
#[path = "common/told.rs"]
mod told;

use ferb_core::chat_entry::{AnswerBody, EntryState};
use ferb_orchestrator::answer_marks::STOPPED;
use serde_json::Value;
use support_control::{run_with, world, World};

/// The scenario run with the log's trace kept: the stored entries, and ENG-15's rules on them.
fn chat(
    w: &mut World,
    name: &str,
    edit: impl FnOnce(&mut Vec<Value>),
) -> Vec<ferb_core::chat_entry::Entry> {
    let items = told::traced(&w.sup);
    let r = run_with(w, name, edit);
    eprintln!("{name}: {}", r.verdict());
    let entries = told::stored(&w.sup, &w.db);
    let broken = support_entries::check(
        name,
        &support_entries::read(&items.lock().unwrap()),
        &entries,
    );
    assert!(broken.is_empty(), "{}", broken.join("\n"));
    entries
}

/// The stopped task's own turn: its end as its state only (cancelled), no words, lines, cards or files.
fn ended_without_words(entries: &[ferb_core::chat_entry::Entry], request: &str) {
    let own = told::answers_of(entries, request);
    assert_eq!(own.len(), 1, "the task's turn has its end: {own:?}");
    assert_eq!(own[0].state, EntryState::Cancelled);
    let body: AnswerBody = own[0].body_as().unwrap();
    assert_eq!(body.text, "", "{body:?}");
    assert_eq!(body.status, "cancelled");
    assert!(
        body.cards.is_none()
            && body.lines.is_none()
            && body.note.is_none()
            && body.artifacts.is_empty(),
        "{body:?}"
    );
}

/// "stop that" in chat while the task runs: its reply says the end once; the task's turn reads Cancelled.
// decision: ORCH-688, ORCH-689, APP-1113, ORCH-579
#[test]
fn a_stop_in_chat_is_said_once_under_the_stop_message() {
    let mut w = world("stop_by_chat");
    let entries = chat(&mut w, "stop_by_chat", |_| {});
    let requests = told::messages(&entries);
    assert_eq!(requests.len(), 2, "{requests:?}");
    let said = told::saying(&entries, STOPPED);
    assert_eq!(said.len(), 1, "one entry says the stop: {said:?}");
    assert_eq!(
        said[0].request_id.as_deref(),
        Some(requests[1].as_str()),
        "under the stop message"
    );
    ended_without_words(&entries, &requests[0]);
}

/// A stop and new work in one message (ORCH-638): the stop is said once, before the new task's plan line.
// decision: ORCH-688, ORCH-689, APP-1113, ORCH-638
#[test]
fn a_stop_beside_new_work_is_said_once() {
    let mut w = world("stop_plus_new_task");
    let entries = chat(&mut w, "stop_plus_new_task", |_| {});
    let requests = told::messages(&entries);
    let said = told::saying(&entries, STOPPED);
    assert_eq!(said.len(), 1, "one entry says the stop: {said:?}");
    assert_eq!(
        said[0].request_id.as_deref(),
        requests.last().map(String::as_str),
        "under the stop message"
    );
    ended_without_words(&entries, &requests[0]);
}

/// The Stop button (task.stop; the phone's Stop is the same call): no reply tells it, so the task's own turn says its
/// end, once.
// decision: ORCH-688, ORCH-579
#[test]
fn the_stop_button_is_said_once_by_the_tasks_own_end() {
    let mut w = world("stop_by_task_stop");
    let entries = chat(&mut w, "stop_by_task_stop", |_| {});
    let requests = told::messages(&entries);
    let said = told::saying(&entries, STOPPED);
    assert_eq!(said.len(), 1, "one entry says the stop: {said:?}");
    assert_eq!(
        said[0].request_id.as_deref(),
        Some(requests[0].as_str()),
        "on the task's own turn"
    );
}
