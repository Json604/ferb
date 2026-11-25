//! ORCH-725 (agent, for ORCH-719's part 8): the planner's context without what it repeated on every turn after its
//! cached part. No line of the conversation entries' ages (each entry shows its own date and time); the latest
//! result's block only when the conversation doesn't already show that result word for word with its id. Through the
//! real supervisor on the recorded stop_after_end scenario (its fourth call carried both, the result also shown whole
//! in the conversation) with stand-in models, temp store, injected ids and time; and the rule itself on invented blocks.

mod common;
mod support_control;

use std::sync::atomic::Ordering;

use ferb_context::assemble::ContextBlock;
use ferb_supervisor::history::Sourced;
use ferb_supervisor::planner::latest_unless_shown;
use ferb_supervisor::words::LATEST_RESULT;
use serde_json::Value;
use support_control::{run_with, world_with};

fn parts(call: &Value) -> Vec<String> {
    call["parts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p.as_str().unwrap_or_default().to_string())
        .collect()
}

// decision: ORCH-725
#[test]
fn a_result_the_conversation_shows_whole_is_not_repeated_and_no_ages_line_is_sent() {
    let name = "stop_after_end";
    let mut w = world_with(name, |_| {});
    w.daemon.bounded.store(true, Ordering::SeqCst);
    let _ = run_with(&mut w, name, |_| {});
    let started = w.daemon.calls.lock().unwrap().started.clone();
    let planners: Vec<Vec<String>> = started
        .iter()
        .map(parts)
        .filter(|p| p.iter().any(|t| t.starts_with("Layer 5 — conversation:")))
        .collect();
    assert!(planners.len() >= 2, "{started:?}");
    for p in &planners {
        assert!(
            !p.iter().any(|t| t.contains("Ages of the items above")),
            "no ages line: {p:?}"
        );
    }
    // the turn after the task: the result is in the conversation, whole and with its id, and nowhere else
    let last = planners.last().unwrap();
    let shown: Vec<&String> = last
        .iter()
        .filter(|t| t.contains("\"There are 3 files.\""))
        .collect();
    assert_eq!(shown.len(), 1, "{last:?}");
    assert!(shown[0].starts_with("[c") && shown[0].contains("Result id: "));
    assert!(!last.iter().any(|t| t.starts_with(LATEST_RESULT)));
    // the time is still told: now, and each entry's own
    assert!(last
        .iter()
        .any(|t| t.contains("[now Thursday 2026-01-01 09:00:00 UTC")));
    assert!(last
        .iter()
        .any(|t| t.starts_with("[c1] (Thu 01 Jan 09:00) Owner:")));
}

fn block(text: &str, payload: &str, refs: &[&str]) -> Sourced {
    Sourced {
        block: ContextBlock {
            payload: Some(payload.to_string()),
            source_refs: refs.iter().map(|r| r.to_string()).collect(),
            ..ContextBlock::new(text, "chat.history")
        },
        origins: Default::default(),
    }
}

// decision: ORCH-725
#[test]
fn the_latest_block_stays_unless_every_row_is_shown_whole_with_its_id() {
    let said = "First paragraph of the invented report.\n\nIts second paragraph.";
    let quoted = ferb_sanitize::untrusted::as_data(said);
    let latest = vec![block(
        &format!("{LATEST_RESULT}\nResult r-1 (…):\n{quoted}"),
        said,
        &["entry:r-1"],
    )];
    let entry = |text: &str| vec![block(text, "", &["entry:r-1"])];
    // the conversation's short form (the first paragraph) only: kept
    let short = entry("[c2] FERB (task t-1 result, done; short form): \"First paragraph of the invented report.\"\n[Result id: r-1]");
    assert_eq!(latest_unless_shown(latest.clone(), &short).len(), 1);
    // whole, but without its id (another entry quoting it): kept
    let other = entry(&format!(
        "[c2] FERB (reply): {said}\n[Full source: chat.history entry_id=x-9]"
    ));
    assert_eq!(latest_unless_shown(latest.clone(), &other).len(), 1);
    // whole and with its id, as data or as a reply's words: left out
    let whole = entry(&format!(
        "[c2] FERB (task t-1 result, done): {quoted}\n[Result id: r-1]"
    ));
    assert!(latest_unless_shown(latest.clone(), &whole).is_empty());
    let told = entry(&format!("[c3] FERB (reply, model-written, unchecked): {said}\n[Result id: r-1; task.evidence task_id=t-1]"));
    assert!(latest_unless_shown(latest.clone(), &told).is_empty());
    // a card: every row must be shown, or all stay
    let two = vec![
        latest[0].clone(),
        block("Result r-2 (…):\n\"Row two.\"", "Row two.", &["entry:r-2"]),
    ];
    assert_eq!(latest_unless_shown(two, &whole).len(), 2);
}
