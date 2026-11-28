//! `ferb trace`'s timing (`--json` and `--full`) shows a chat turn's first word (ORCH-732: `first_text_ms` of the
//! `model.turn` span that sent it) and, under each model call, the requests FERB's Claude relay recorded (RUN-479:
//! the tools called, the results carried back with their error flags, the stop reason, the times). A record without
//! them reads as before, so every Python-recorded trace keeps its bytes (tests/parity.rs).

use ferb_trace::{breakdown, timing_lines};
use serde_json::{json, Value};

fn forest(turn_timing: Value, usage: Value) -> Vec<Value> {
    vec![json!({
        "id": "turn", "kind": "model.turn", "task_id": null,
        "input": {"role": "supervisor", "request_id": "r1", "front_door": true},
        "output": {"timing": turn_timing},
        "started_at": "2026-11-27T10:00:01+00:00", "ended_at": "2026-11-27T10:00:05+00:00",
        "children": [{
            "id": "u1", "kind": "model.usage", "task_id": null, "input": {"runtime": "claude"},
            "output": usage,
            "started_at": "2026-11-27T10:00:05+00:00", "ended_at": "2026-11-27T10:00:05+00:00",
            "children": []
        }]
    })]
}

fn turn_timing() -> Value {
    json!({"steps_ms": {"ferb_before": 8, "model_call": 4000, "ferb_after": 1}, "total_ms": 4010,
           "call": {"steps_ms": {"prepare": 50, "start": 2, "stream": 3900, "close": 0}, "total_ms": 3960,
                    "model_calls": 1}, "first_text_ms": 4648})
}

fn usage() -> Value {
    json!({"input_tokens": 10, "timing": {"by": "runtime", "ttft_ms": 1681, "generate_ms": 2200, "tail_ms": 0,
                                          "total_ms": 3881},
           "requests": 3, "steps_dropped": 2, "steps": [
        {"called": ["mcp__ferb__chat_dispatch_task"], "results": [], "stop": "tool_use",
         "first_byte_ms": 1607, "done_ms": 3499},
        {"called": ["mcp__ferb__chat_dispatch_task"],
         "results": [{"tool": "mcp__ferb__chat_dispatch_task", "error": true}], "stop": "tool_use",
         "first_byte_ms": 1545, "done_ms": 2585},
        {"called": [], "results": [{"tool": "mcp__ferb__chat_dispatch_task", "error": false}], "status": 529,
         "done_ms": 12}]})
}

// decision: ORCH-755
#[test]
fn the_json_carries_the_first_word_and_the_relay_requests() {
    let got = breakdown(&forest(turn_timing(), usage()));
    assert_eq!(got.len(), 1);
    assert_eq!(got[0]["first_text_ms"], json!(4648));
    let call = &got[0]["model_calls"][0];
    assert_eq!(call["relay_steps"], usage()["steps"]);
    assert_eq!(call["relay_steps_dropped"], json!(2));
}

// decision: ORCH-755
#[test]
fn the_full_trace_shows_the_first_word_and_each_request_under_its_call() {
    let lines = timing_lines(&breakdown(&forest(turn_timing(), usage())));
    let text = lines.join("\n");
    let header = lines
        .iter()
        .position(|l| l.starts_with("Planner turn"))
        .expect("a planner turn");
    assert_eq!(
        lines[header + 1],
        "  First word shown 4.65 s after its intake",
        "{text}"
    );
    let call = lines
        .iter()
        .position(|l| l.starts_with("  Model call 1 "))
        .expect("the model call");
    assert_eq!(
        lines[call + 1..call + 5],
        [
            "    (2 earlier requests not kept)",
            "    Request 1: called mcp__ferb__chat_dispatch_task; stop tool_use; first byte 1.61 s; done 3.50 s",
            "    Request 2: called mcp__ferb__chat_dispatch_task; carried results of \
             mcp__ferb__chat_dispatch_task (error); stop tool_use; first byte 1.54 s; done 2.58 s",
            "    Request 3: no tool called; carried results of mcp__ferb__chat_dispatch_task; status 529; done 12 ms",
        ],
        "{text}"
    );
}

// decision: ORCH-755 (a record without them reads as before: no key, no line)
#[test]
fn a_record_without_them_reads_as_before() {
    let mut timing = turn_timing();
    timing.as_object_mut().unwrap().remove("first_text_ms");
    let mut u = usage();
    for k in ["steps", "steps_dropped"] {
        u.as_object_mut().unwrap().remove(k);
    }
    let got = breakdown(&forest(timing, u));
    assert!(got[0].get("first_text_ms").is_none());
    assert!(got[0]["model_calls"][0].get("relay_steps").is_none());
    let text = timing_lines(&got).join("\n");
    assert!(
        !text.contains("First word") && !text.contains("Request 1"),
        "{text}"
    );
}
