//! The chat-pane look's single call (kb HAND-699): the request holds only the call's own prompt and input, nothing of
//! FERB's context (no profile, memory, chat history or task tree).

mod support;

use ferb_learning::calls::{learning_call, learning_task, Made};
use serde_json::{json, Value};
use support::FakeRuntime;

// decision: HAND-699
#[tokio::test]
async fn a_single_call_carries_only_its_prompt_and_its_input() {
    let events = support::Events::default();
    let rt = FakeRuntime {
        answer: std::sync::Mutex::new(
            json!({"answer": {"chat": false, "message_list": null, "reply_box": null}}),
        ),
        events: events.clone(),
    };
    let made = Made {
        rt: Box::new(rt),
        limits: Some(support::limits(false)),
        model: None,
    };
    let task = learning_task(
        "chat-pane-1",
        "where a chat app's reply box is",
        "dev",
        "chat-pane",
    );
    let schema = json!({"type": "object", "properties": {"chat": {"type": "boolean"},
        "message_list": {"type": ["integer", "null"]}, "reply_box": {"type": ["integer", "null"]}},
        "required": ["chat", "message_list", "reply_box"]});
    let out = learning_call(
        &made,
        &task,
        "Find the chat.",
        "0 AXWindow @0,0 800x600",
        "window outline",
        &schema,
        "chat_pane",
        None,
        &|_, _| {},
    )
    .await
    .unwrap();
    assert!(out.contains("\"chat\""));
    let started: Vec<Value> = events
        .lock()
        .unwrap()
        .iter()
        .filter(|e| e[0] == "model.start")
        .map(|e| e[1].clone())
        .collect();
    assert_eq!(started.len(), 1);
    let parts = started[0]["parts"].to_string();
    assert!(parts.contains("0 AXWindow @0,0 800x600"), "{parts}");
    for other in ["profile", "memory", "history", "Tier", "task tree"] {
        assert!(!parts.contains(other), "{other} in {parts}");
    }
}
