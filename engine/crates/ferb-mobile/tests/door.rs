//! The adapter onto ferb-link's JSON door (`LinkClient::command`/`poll`): a call's result and error reach its caller,
//! `chat.send` returns at its ack, a watch's events reach its sink and its end ends it, the link's own events reach
//! the app, a refusal at the door comes back at once, and close ends every waiter.

use std::collections::VecDeque;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use ferb_mobile::door::{DoorTransport, LinkDoor};
use ferb_mobile::transport::{LinkFailure, Transport, WatchSink};
use serde_json::{json, Value};

/// A door that answers each command by a script keyed on the method.
#[derive(Default)]
struct FakeDoor {
    events: Mutex<VecDeque<String>>,
    ready: Condvar,
    commands: Mutex<Vec<Value>>,
}

impl FakeDoor {
    fn push(&self, v: Value) {
        self.events.lock().unwrap().push_back(v.to_string());
        self.ready.notify_all();
    }
}

impl LinkDoor for FakeDoor {
    fn command(&self, json: &str) -> String {
        let c: Value = serde_json::from_str(json).unwrap();
        self.commands.lock().unwrap().push(c.clone());
        let id = c["id"].clone();
        match c["method"].as_str().unwrap_or_default() {
            "chat.entries.page" => {
                self.push(json!({"type": "result", "id": id, "result": {"entries": []}}))
            }
            "task.stop" => self.push(json!({"type": "error", "id": id,
                "error": {"code": "method_not_allowed", "message": "no"}})),
            "chat.send" => {
                self.push(json!({"type": "event", "id": id, "event": {"type": "ack", "request_id": "r1"}}));
                self.push(json!({"type": "event", "id": id, "event": {"type": "plan"}}));
                self.push(json!({"type": "result", "id": id, "result": {}}));
            }
            "chat.entries.watch" => {
                self.push(
                    json!({"type": "event", "id": id, "event": {"type": "caught_up", "head": 3}}),
                );
                self.push(json!({"type": "result", "id": id, "result": {}}));
            }
            "chat.answer" => {
                return json!({"error": {"code": "link_closed", "message": "closed"}}).to_string();
            }
            _ => {}
        }
        json!({"ok": true}).to_string()
    }

    fn poll(&self, wait_ms: u64) -> String {
        let q = self.events.lock().unwrap();
        let (mut q, _) = self
            .ready
            .wait_timeout_while(q, Duration::from_millis(wait_ms), |q| q.is_empty())
            .unwrap();
        q.pop_front().unwrap_or_default()
    }
}

#[derive(Default)]
struct Frames(
    Mutex<Vec<String>>,
    Mutex<Option<Option<LinkFailure>>>,
    Condvar,
);
impl WatchSink for Frames {
    fn frame(&self, frame_json: String) {
        self.0.lock().unwrap().push(frame_json);
    }
    fn ended(&self, failure: Option<LinkFailure>) {
        *self.1.lock().unwrap() = Some(failure);
        self.2.notify_all();
    }
}

// decision: PHN-446
// decision: PHN-461
// decision: PHN-477
#[test]
fn the_door_routes_calls_streams_and_the_links_events() {
    let door = Arc::new(FakeDoor::default());
    let (tx, heard) = std::sync::mpsc::channel::<String>();
    let tx = Mutex::new(tx);
    let t = DoorTransport::start(
        door.clone(),
        Arc::new(move |e| {
            let _ = tx.lock().unwrap().send(e);
        }),
    );
    let r: Value =
        serde_json::from_str(&t.call("chat.entries.page", "{\"limit\":1}", None).unwrap()).unwrap();
    assert_eq!(r["entries"], json!([]));
    assert_eq!(
        t.call("task.stop", "{}", None).unwrap_err().code,
        "method_not_allowed"
    );
    assert_eq!(
        t.call(
            "chat.answer",
            "{}",
            Some("{\"payload\":\"p\",\"sig\":\"s\"}")
        )
        .unwrap_err()
        .code,
        "link_closed"
    );
    let proofed = door
        .commands
        .lock()
        .unwrap()
        .iter()
        .find(|c| c["method"] == "chat.answer")
        .cloned()
        .unwrap();
    assert_eq!(proofed["proof"]["sig"], "s");
    let ack: Value = serde_json::from_str(&t.send("{\"text\":\"hi\"}").unwrap()).unwrap();
    assert_eq!(ack["request_id"], "r1");
    let sink = Arc::new(Frames::default());
    let _w = t.watch("{}", sink.clone()).unwrap();
    let ended = sink.1.lock().unwrap();
    let ended = sink
        .2
        .wait_timeout_while(ended, Duration::from_secs(5), |e| e.is_none())
        .unwrap()
        .0;
    assert_eq!(*ended, Some(None), "the stream's end ends the watch");
    drop(ended);
    assert_eq!(sink.0.lock().unwrap().len(), 1);
    door.push(json!({"type": "bulk", "allowed": true, "tier": "direct"}));
    let event = heard.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(
        event.contains("\"bulk\""),
        "the link's own events reach the app"
    );
    t.close();
    assert_eq!(
        t.call("chat.entries.page", "{}", None).unwrap_err().code,
        "link_closed"
    );
}
