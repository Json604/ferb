//! The fresh run's hand-off note (ORCH-719 item 4, ORCH-760) on invented calls: at most the owner's 1,500 o200k tokens
//! however long the attempt was, the newest calls kept one per line and the oldest counted by tool, the latest page,
//! app window and file with the span id that opens each, everything read from the records inside one wrapper.

use ferb_orchestrator::check_evidence::Call;
use ferb_orchestrator::hand_off::{note, HandOff};
use serde_json::{json, Value};

fn call(n: usize, tool: &str, args: Value, output: Value) -> Call {
    Call {
        run: 1,
        run_began: "2026-01-01T09:00:00+00:00".into(),
        row: json!({"id": format!("span-{n:04}"), "kind": "tool.call", "task_id": "t1",
                    "started_at": "2026-01-01T09:00:00+00:00", "ended_at": "2026-01-01T09:00:01+00:00",
                    "input": {"tool": tool, "args": args}, "output": output, "error": null, "origin": null}),
    }
}

fn calls(n: usize) -> Vec<Call> {
    (0..n)
        .map(|i| match i % 4 {
            0 => call(
                i,
                "browser.snapshot",
                json!({"tab": "t1"}),
                json!({"tab": "t1", "url": format!("https://shop.example/item/{i}"), "title": "Item page"}),
            ),
            1 => call(
                i,
                "browser.click",
                json!({"tab": "t1", "element": format!("e{i}"), "why": "open the next listing in the results"}),
                json!({"tab": "t1", "ok": true}),
            ),
            2 => call(
                i,
                "mac.snapshot",
                json!({"app": "com.example.Notes"}),
                json!({"bundle_id": "com.example.Notes", "window": "Shopping list"}),
            ),
            _ => call(
                i,
                "fs.write",
                json!({"path": format!("/tmp/fx/notes-{i}.txt"), "content": "a line of the list"}),
                json!({"path": format!("/tmp/fx/notes-{i}.txt")}),
            ),
        })
        .collect()
}

fn wrap(text: &str) -> String {
    format!("<data>\n{text}\n</data>")
}

fn observes(tool: &str) -> bool {
    tool.ends_with(".snapshot")
}

fn count(text: &str) -> usize {
    ferb_context::tokens::count_tokens(text)
}

fn hand_off<'a>(calls: &'a [Call], conditions: &'a [String]) -> HandOff<'a> {
    HandOff {
        goal: "Put the three cheapest items on the shopping list",
        conditions,
        calls,
        unsettled: &[],
        last_words: Some("The second item is out of stock."),
    }
}

// decision: ORCH-719
// decision: ORCH-760
#[test]
fn a_long_attempt_fits_in_the_note_newest_kept() {
    let conditions = vec!["The list holds three items".to_string()];
    let many = calls(400);
    let text = note(
        &hand_off(&many, &conditions),
        &observes,
        &wrap,
        &count,
        1_500,
    );
    assert!(count(&text) <= 1_500, "{} tokens", count(&text));
    assert!(text.contains("Put the three cheapest items"));
    assert!(text.contains("The list holds three items"));
    assert!(text.contains("400. "), "the newest call is listed: {text}");
    assert!(!text.contains("\n1. "), "the oldest gave way");
    assert!(text.contains("not listed one by one: "), "{text}");
    assert!(text.contains("browser.click ×"));
    // the latest of each place, with its record: the newest page (call 396), app window (398) and file (399)
    assert!(text.contains("https://shop.example/item/396"), "{text}");
    assert!(text.contains("span_id=span-0396"));
    assert!(text.contains("span_id=span-0398"));
    assert!(text.contains("/tmp/fx/notes-399.txt (span_id=span-0399)"));
    assert!(text.contains("The second item is out of stock."));
    // what came from the records is inside the wrapper; the goal is FERB's own, outside it
    let inside = &text[text.find("<data>").unwrap()..];
    assert!(inside.contains("span-0396") && !inside.contains("Put the three cheapest"));
}

// decision: ORCH-760
#[test]
fn a_short_attempt_lists_every_call() {
    let few = calls(6);
    let text = note(&hand_off(&few, &[]), &observes, &wrap, &count, 1_500);
    for n in 1..=6 {
        assert!(text.contains(&format!("\n{n}. ")), "call {n}: {text}");
    }
    assert!(!text.contains("not listed one by one"));
}

// decision: ORCH-760
#[test]
fn an_overlong_goal_is_cut_to_fit() {
    let goal = "word ".repeat(5_000);
    let few = calls(20);
    let h = HandOff {
        goal: &goal,
        ..hand_off(&few, &[])
    };
    let text = note(&h, &observes, &wrap, &count, 1_500);
    assert!(count(&text) <= 1_500, "{} tokens", count(&text));
}
