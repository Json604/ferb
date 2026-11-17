//! SENSE-424, PRM-362: a message a paired phone sends (the phone gate names its client `phone:<link id>`) is recorded
//! with where it was sent from, by the phone's name as the Mac knows it, and the planner reads that fact on the new
//! message's heading. Scripted on the recorded plain_reply scenario (fake model, no provider).

mod common;
mod support_control;

use serde_json::{json, Value};
use support_control::{run_with, world_with};

// decision: SENSE-424
// decision: PRM-362
#[test]
fn a_phones_message_is_recorded_and_read_as_sent_from_that_phone() {
    let name = "plain_reply";
    let mut w = world_with(name, |_| {});
    ferb_store::phones::add(
        &w.db.lock().unwrap(),
        &ferb_store::phones::NewPhone {
            endpoint: "e0e0e0".into(),
            name: "Invented Phone".into(),
            app: "android".into(),
            ..Default::default()
        },
        "2026-11-17T22:00:00+05:30",
        5,
    )
    .unwrap()
    .unwrap();
    let _ = run_with(&mut w, name, |steps| {
        for st in steps.iter_mut().filter(|st| st["do"] == "stream") {
            let mut sent: Value =
                serde_json::from_str(st["lines"][0]["send"].as_str().unwrap()).unwrap();
            sent["params"]["client_id"] = json!("phone:e0e0e0");
            st["lines"][0]["send"] = json!(sent.to_string());
        }
    });
    let from = "the phone \u{201c}Invented Phone\u{201d}, in FERB's phone app";
    let rows: Vec<Value> = {
        let db = w.db.lock().unwrap();
        let mut st = db
            .conn()
            .prepare("SELECT data_json FROM journal WHERE kind = 'chat.message' ORDER BY seq")
            .unwrap();
        let rows = st
            .query_map([], |r| r.get::<_, String>(0))
            .unwrap()
            .map(|r| serde_json::from_str(&r.unwrap()).unwrap())
            .collect();
        rows
    };
    let sent = rows.last().expect("the message's row");
    assert_eq!(sent["sent_from"], from, "{sent}");
    let keys: Vec<&String> = sent.as_object().unwrap().keys().collect();
    assert_eq!(keys[..4], ["session_id", "request_id", "sent_from", "text"]);
    assert!(
        !sent.to_string().contains("e0e0e0"),
        "never the phone's link id"
    );
    let calls = w.daemon.calls.lock().unwrap();
    let parts: Vec<String> = calls
        .started
        .iter()
        .flat_map(|c| c["parts"].as_array().cloned().unwrap_or_default())
        .filter_map(|p| p.as_str().map(str::to_string))
        .collect();
    let heading = format!("Layer 5 \u{2014} owner's complete new message (sent from {from}):\n");
    assert!(
        parts.iter().any(|p| p.contains(&heading)),
        "the planner reads where the message came from"
    );
    assert!(!parts.concat().contains("e0e0e0"));
}
