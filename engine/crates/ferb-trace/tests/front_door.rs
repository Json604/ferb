//! The front door's turn in the story (ORCH-735): its `model.turn` span holds the run's raw answer in `text` and what
//! it decided, in the planner's answer form, in `decided`; the Planner line reads `decided` when it is there, so a
//! hand-off reads "task — <goal>" as the planner's does. A planner's span (no `decided`) reads `text` as before.

use ferb_trace::compose;
use serde_json::{json, Value};

fn planner_line(output: Value, input: Value) -> String {
    let trace = json!({
        "task_id": "t1",
        "spans": [{
            "id": "s1", "kind": "task", "task_id": "t1", "input": {}, "output": {},
            "started_at": "2026-11-27T10:00:00+00:00", "ended_at": "2026-11-27T10:00:05+00:00",
            "children": [{
                "id": "s2", "kind": "model.turn", "task_id": null, "input": input, "output": output,
                "started_at": "2026-11-27T10:00:01+00:00", "ended_at": "2026-11-27T10:00:02+00:00",
                "children": []
            }]
        }],
        "outcome": {}
    });
    let composed = compose(&trace);
    let steps = composed["steps"].as_array().expect("steps");
    let planner = steps
        .iter()
        .find(|s| s["role"] == "Planner")
        .unwrap_or_else(|| panic!("a Planner line: {steps:?}"));
    planner["text"].as_str().unwrap().to_string()
}

// decision: ORCH-735
#[test]
fn a_front_door_hand_off_reads_as_the_task_it_started() {
    let decided =
        json!({"kind": "task", "text": "On it.", "task": {"goal": "Buy bread on the way home"}})
            .to_string();
    let line = planner_line(
        json!({"text": "Looking into it.", "calls": ["chat.dispatch_task"], "decided": decided}),
        json!({"role": "supervisor", "request_id": "r1", "repair": false, "front_door": true}),
    );
    assert_eq!(line, "task — Buy bread on the way home");
}

// decision: ORCH-735 (a plain reply: the kind and the reply's words, as the planner's line)
#[test]
fn a_front_door_reply_reads_as_a_reply() {
    let decided = json!({"kind": "reply", "text": "You bake on Sundays."}).to_string();
    let line = planner_line(
        json!({"text": "You bake on Sundays.", "calls": [], "decided": decided}),
        json!({"role": "supervisor", "request_id": "r1", "repair": false, "front_door": true}),
    );
    assert_eq!(line, "reply — You bake on Sundays.");
}

// decision: ORCH-735 (the planner's span has no `decided`: its text is read as before)
#[test]
fn a_planner_span_still_reads_its_text() {
    let text =
        json!({"kind": "task", "text": "On it.", "task": {"goal": "Water the plants"}}).to_string();
    let line = planner_line(
        json!({"text": text}),
        json!({"role": "supervisor", "request_id": "r1", "repair": false}),
    );
    assert_eq!(line, "task — Water the plants");
}
