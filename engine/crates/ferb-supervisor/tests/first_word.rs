//! ORCH-732 (front door step 1): each planner turn records how long the owner waited for FERB's first word: the ms
//! from the turn's intake (its `planner.prepare` span's start) to the first reply or plan text frame sent to the
//! client, as `first_text_ms` in the timing of the `model.turn` call that sent it. A turn that streams no text records
//! nothing. Only with the bounded planner (the Python recordings' replays keep their `model.turn` bytes). Scripted on
//! the recorded plain_reply scenario through the real supervisor with stand-in models (temp store, injected ids and an
//! injected clock the stand-in model moves on: no provider).

mod common;
mod support_control;

use std::sync::atomic::Ordering;

use serde_json::{json, Value};
use support_control::{run_with, world_with, World};

fn answer(text: &str) -> String {
    json!({"kind": "reply", "answers_question": false, "task": null, "control": null,
           "side_entries": null, "text": text})
    .to_string()
}

/// A planner call that takes `before` seconds to its first words, streams its whole answer, and ends `after` seconds
/// later; `stream` false: the answer comes only whole, at the end.
fn planner(text: &str, before: f64, after: f64, stream: bool) -> Value {
    let said = answer(text);
    let mut events = vec![json!({"kind": "advance", "seconds": before})];
    if stream {
        events.push(json!({"kind": "text_delta", "data": {"text": said}}));
    }
    events.push(json!({"kind": "advance", "seconds": after}));
    events.push(json!({"kind": "usage", "data": {"input_tokens": 10, "output_tokens": 5}}));
    events.push(json!({"kind": "final", "data": {"text": said}}));
    json!({"key": "supervisor", "events": events})
}

/// One message on the recorded scenario's chat, answered by `call`; the planner calls' recorded timing.
fn one_turn(w: &mut World, call: Value) -> Vec<Value> {
    let _ = run_with(w, "plain_reply", |steps| {
        let at = steps.iter().position(|st| st["do"] == "stream").unwrap();
        steps.insert(at, json!({"do": "calls_from_here", "calls": [call]}));
    });
    let db = w.db.lock().unwrap();
    let mut st = db
        .conn()
        .prepare("SELECT output_json FROM spans WHERE kind = 'model.turn' ORDER BY rowid")
        .unwrap();
    st.query_map([], |r| r.get::<_, Option<String>>(0))
        .unwrap()
        .map(|o| serde_json::from_str::<Value>(&o.unwrap().unwrap_or_default()).unwrap())
        .map(|o| o["timing"].clone())
        .collect()
}

fn streamed(w: &World) -> bool {
    w.daemon.calls.lock().unwrap().started.len() == 1
}

// decision: ORCH-732
#[test]
fn a_streamed_reply_records_the_ms_from_intake_to_its_first_words() {
    let mut w = world_with("plain_reply", |calls| calls.clear());
    w.daemon.bounded.store(true, Ordering::SeqCst);
    let timings = one_turn(&mut w, planner("Here it is.", 1.25, 2.0, true));
    assert!(streamed(&w));
    assert_eq!(timings.len(), 1, "{timings:?}");
    // the clock stood still from intake to the call; the model's first words came 1.25 s into it
    assert_eq!(timings[0]["first_text_ms"], json!(1250), "{}", timings[0]);
    // the call itself ran on to its end: the first word is its own fact, not the call's length
    assert_eq!(timings[0]["steps_ms"]["model_call"], json!(3250));
}

// decision: ORCH-732
#[test]
fn a_turn_that_streams_no_text_records_nothing() {
    let mut w = world_with("plain_reply", |calls| calls.clear());
    w.daemon.bounded.store(true, Ordering::SeqCst);
    let timings = one_turn(&mut w, planner("Here it is.", 1.25, 2.0, false));
    assert_eq!(timings.len(), 1, "{timings:?}");
    assert!(timings[0].get("first_text_ms").is_none(), "{}", timings[0]);
}

// decision: ORCH-732 (a replay of a Python recording keeps its model.turn bytes)
#[test]
fn a_replay_of_a_python_recording_records_no_first_word() {
    let mut w = world_with("plain_reply", |calls| calls.clear());
    let timings = one_turn(&mut w, planner("Here it is.", 1.25, 2.0, true));
    assert_eq!(timings.len(), 1, "{timings:?}");
    assert!(timings[0].get("first_text_ms").is_none(), "{}", timings[0]);
}
