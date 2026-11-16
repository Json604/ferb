//! The adapter onto ferb-link's JSON door (`LinkClient::command`/`poll`): a call's result and error reach its caller,
//! `chat.send` returns at its ack, a watch's events reach its sink and its end ends it, the link's own events reach
//! the app, a refusal at the door comes back at once, and close ends every waiter.

use std::collections::VecDeque;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use ferb_mobile::door::{DoorTransport, LinkDoor};
use ferb_mobile::transport::{LinkFailure, TransferSink, Transport, WatchSink};
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
        match c["cmd"].as_str().unwrap_or_default() {
            // a fetch: progress, then its result; a hung one ("hang" in its path) waits for a cancel
            "get" if c["params"]["path"] != "hang" => {
                self.push(json!({"type": "progress", "id": id, "done": 5, "of": 10}));
                self.push(json!({"type": "progress", "id": id, "done": 10, "of": 10}));
                self.push(json!({"type": "result", "id": id, "result": {"received": 10, "saved_to": c["save_to"]}}));
                return json!({"ok": true}).to_string();
            }
            "put" => {
                self.push(json!({"type": "accepted", "id": id, "upload_id": "U-9", "offset": 0}));
                self.push(json!({"type": "result", "id": id, "result": {"upload_id": "U-9"}}));
                return json!({"ok": true}).to_string();
            }
            _ => {}
        }
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

#[derive(Default)]
struct Moves(Mutex<Vec<(u64, u64)>>, Mutex<Vec<String>>);
impl TransferSink for Moves {
    fn progress(&self, done: u64, of: u64) {
        self.0.lock().unwrap().push((done, of));
    }
    fn accepted(&self, upload_id: &str, _offset: u64) {
        self.1.lock().unwrap().push(upload_id.to_string());
    }
}

/// Transfers through the door: a fetch hears its progress until its result, an upload its go-ahead; a cancel ends a
/// transfer under way at once with `cancelled` (the door sends no result for an aborted command); the Mac's pushed
/// `status` reaches the app as one of the link's own events (T-1221).
// decision: PHN-477
// decision: PHN-126
// decision: PHN-511
// decision: PHN-513
#[test]
fn transfers_hear_their_progress_and_a_cancel_and_the_status_reaches_the_app() {
    let door = Arc::new(FakeDoor::default());
    let (tx, heard) = std::sync::mpsc::channel::<String>();
    let tx = Mutex::new(tx);
    let t = Arc::new(DoorTransport::start(
        door.clone(),
        Arc::new(move |e| {
            let _ = tx.lock().unwrap().send(e);
        }),
    ));
    let moves = Arc::new(Moves::default());
    let r: Value = serde_json::from_str(
        &t.get(
            "f1",
            r#"{"what":"path","path":"/a"}"#,
            Some("/tmp/x"),
            true,
            moves.clone(),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(r["received"], 10);
    assert_eq!(*moves.0.lock().unwrap(), [(5, 10), (10, 10)]);
    let get = door.commands.lock().unwrap()[0].clone();
    assert_eq!(
        (
            get["cmd"].clone(),
            get["save_to"].clone(),
            get["append"].clone()
        ),
        (json!("get"), json!("/tmp/x"), json!(true))
    );
    let up: Value = serde_json::from_str(
        &t.put(
            "u1",
            "/tmp/y",
            r#"{"name":"y","resume":null,"group":"G"}"#,
            moves.clone(),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(up["upload_id"], "U-9");
    assert_eq!(*moves.1.lock().unwrap(), ["U-9"]);
    let put = door.commands.lock().unwrap()[1].clone();
    assert_eq!(
        (put["name"].clone(), put["group"].clone(), put.get("resume")),
        (json!("y"), json!("G"), None)
    );
    // a hung fetch, cancelled from another thread
    let t2 = t.clone();
    let waiting = std::thread::spawn(move || {
        t2.get(
            "f2",
            r#"{"what":"path","path":"hang"}"#,
            Some("/tmp/z"),
            false,
            Arc::new(Moves::default()),
        )
    });
    std::thread::sleep(Duration::from_millis(100));
    t.cancel("f2");
    assert_eq!(waiting.join().unwrap().unwrap_err().code, "cancelled");
    assert!(door
        .commands
        .lock()
        .unwrap()
        .iter()
        .any(|c| c["cmd"] == "cancel" && c["target"] == "f2"));
    door.push(json!({"type": "status", "status": {"approvals": []}}));
    let event = heard.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(event.contains("\"status\""), "{event}");
}
