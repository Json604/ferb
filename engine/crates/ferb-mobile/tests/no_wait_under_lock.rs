//! The core never waits on the Mac with its state locked (kb PHN-680, PHN-681). The owner's phone froze after the
//! unlock (2026-11-18): `sync` kept the state locked while it waited for `chat.entries.mark`, so every reader of the
//! core queued behind a network call (the app's main thread among them), and the link's one pump thread, which needs
//! the state to hand over a watch frame or a link event, could never reach that call's answer: the call ended only at
//! the door's 60 s backstop. Two checks: nothing of the core is locked during any call to the Mac, and a frame that
//! arrives ahead of a call's answer through the real door never stalls the sync.

mod common;

use std::collections::VecDeque;
use std::sync::mpsc;
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

use common::mac::FakeMac;
use common::phone::{online, Phone};
use ferb_mobile::door::{DoorTransport, LinkDoor};
use ferb_mobile::entry::ChatEntry;
use ferb_mobile::transport::{LinkFailure, TransferSink, Transport, WatchHandle, WatchSink};
use ferb_mobile::{Answer, Mobile};
use serde_json::{json, Value};

/// How long a reader of the core may wait while a call to the Mac is under way: it must not wait for the call at all
/// (a store read is milliseconds); the fake Mac's slow call takes [SLOW].
const READER_MAY_WAIT: Duration = Duration::from_millis(1500);
const SLOW: Duration = Duration::from_millis(2500);

fn entry(id: &str, seq: i64, text: &str) -> ChatEntry {
    ChatEntry::read(&json!({
        "id": id, "conversation_id": "C1", "pos": (1i64 << 32) + seq, "seq": seq, "rev": 1, "kind": "message",
        "state": "done", "open": false, "request_id": format!("r-{seq}"), "client_id": null, "submission_id": null,
        "created_at": "2026-01-02T10:00:00+00:00", "updated_at": "2026-01-02T10:00:00+00:00", "day": "2026-01-02",
        "body": {"text": text, "attachments": [], "hidden": []}, "deleted": false
    }))
    .expect("an entry")
}

/// A Mac that is slow to answer, and during every call asks, from another thread, what the app's screens ask of the
/// core (the last status, the link's state, the transcript, the outbox): a reader that has to wait for the call to end
/// names the call.
struct Probing {
    mac: FakeMac,
    mobile: OnceLock<Arc<Mobile>>,
    stuck: Mutex<Vec<String>>,
    asked: Mutex<Vec<String>>,
}

impl Probing {
    fn during(&self, what: &str) {
        self.asked.lock().unwrap().push(what.to_string());
        let Some(m) = self.mobile.get().cloned() else {
            return;
        };
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = m.last_status();
            let _ = m.link_state();
            let _ = m.is_locked();
            let _ = m.outbox();
            let _ = m.current_conversation();
            let _ = m.transcript("C1");
            let _ = tx.send(());
        });
        // the Mac takes its time: a reader that waits for this call waits longer than it may
        let began = Instant::now();
        let free = rx.recv_timeout(READER_MAY_WAIT).is_ok();
        if !free {
            self.stuck.lock().unwrap().push(what.to_string());
        }
        std::thread::sleep(
            SLOW.saturating_sub(began.elapsed())
                .min(Duration::from_millis(50)),
        );
    }
}

impl Transport for Probing {
    fn call(&self, method: &str, params: &str, proof: Option<&str>) -> Result<String, LinkFailure> {
        self.during(method);
        self.mac.call(method, params, proof)
    }
    fn send(&self, params: &str) -> Result<String, LinkFailure> {
        self.during("chat.send");
        self.mac.send(params)
    }
    fn watch(
        &self,
        params: &str,
        sink: Arc<dyn WatchSink>,
    ) -> Result<Box<dyn WatchHandle>, LinkFailure> {
        self.during("chat.entries.watch");
        self.mac.watch(params, sink)
    }
    fn get(
        &self,
        id: &str,
        params: &str,
        save_to: Option<&str>,
        append: bool,
        sink: Arc<dyn TransferSink>,
    ) -> Result<String, LinkFailure> {
        self.during("link.get");
        self.mac.get(id, params, save_to, append, sink)
    }
    fn put(
        &self,
        id: &str,
        path: &str,
        opts: &str,
        sink: Arc<dyn TransferSink>,
    ) -> Result<String, LinkFailure> {
        self.during("link.put");
        self.mac.put(id, path, opts, sink)
    }
}

/// Everything the app does that reaches the Mac, against a Mac that probes the core during each call: no reader ever
/// waits for a call to end.
// decision: PHN-681
#[test]
fn nothing_of_the_core_is_locked_while_a_call_to_the_mac_is_under_way() {
    let mac = FakeMac::with("C1", &[entry("m-1", 1, "one"), entry("m-2", 2, "two")]);
    let phone = Phone::new();
    let m = Arc::new(phone.open());
    let t = Arc::new(Probing {
        mac: mac.clone(),
        mobile: OnceLock::new(),
        stuck: Mutex::new(Vec::new()),
        asked: Mutex::new(Vec::new()),
    });
    let _ = t.mobile.set(m.clone());
    m.connect(t.clone()).unwrap();
    m.link_event(&online("direct"));
    // the sync the link's coming online starts, then the app's own
    phone.jobs.run();
    m.sync().unwrap();
    // a message, an answer, the status, the read mark, the lock and the unlock's sync again
    m.send("hello").unwrap();
    let _ = m.answer(
        "a-1",
        Answer::Approve {
            granted: true,
            choice: None,
        },
        None,
    );
    let _ = m.status();
    let _ = m.mark_read_latest("C1");
    let _ = m.load_older("C1");
    let _ = m.mac_info();
    let _ = m.tasks(10, None);
    let _ = m.settings();
    let _ = m.lock();
    let _ = m.sync();
    m.unlocked();
    m.sync().unwrap();
    phone.jobs.run();
    let asked = t.asked.lock().unwrap().clone();
    for needed in [
        "chat.entries.page",
        "chat.entries.changes",
        "chat.entries.watch",
        "chat.entries.mark",
        "chat.send",
        "chat.status",
        "link.lock",
    ] {
        assert!(
            asked.iter().any(|a| a == needed),
            "the run reached {needed}: {asked:?}"
        );
    }
    let stuck = t.stuck.lock().unwrap().clone();
    assert!(
        stuck.is_empty(),
        "a reader of the core waited for these calls to the Mac to end: {stuck:?}"
    );
}

// ---- through the real door ------------------------------------------------------------------------------------------

/// The link's door as the Mac answers it, with what the owner's link did on 2026-11-18: the watch's first frame and a
/// pushed status reach the pump before the answer to the call the sync is waiting on.
#[derive(Default)]
struct StormDoor {
    events: Mutex<VecDeque<String>>,
    ready: Condvar,
    watch: Mutex<Option<Value>>,
    marks: Mutex<u32>,
}

impl StormDoor {
    fn push(&self, v: Value) {
        self.events.lock().unwrap().push_back(v.to_string());
        self.ready.notify_all();
    }
}

impl LinkDoor for StormDoor {
    fn command(&self, text: &str) -> String {
        let c: Value = serde_json::from_str(text).unwrap();
        let id = c["id"].clone();
        if c["cmd"] != "call" {
            return json!({"ok": true}).to_string();
        }
        let one = entry("m-1", 1, "one").to_json();
        match c["method"].as_str().unwrap_or_default() {
            "chat.entries.page" => self.push(json!({"type": "result", "id": id, "result":
                {"conversation": "C1", "epoch": "E1", "head": 1, "entries": [one], "more": false, "before_pos": null}})),
            "chat.entries.changes" => self.push(json!({"type": "result", "id": id, "result":
                {"conversation": "C1", "epoch": "E1", "head": 1, "entries": [], "more": false, "next_seq": 1}})),
            "chat.entries.watch" => *self.watch.lock().unwrap() = Some(id),
            "chat.entries.mark" => {
                *self.marks.lock().unwrap() += 1;
                // ahead of the mark's answer: the watch's first frame, a live frame, the Mac's pushed status and a
                // change of path (each needs the core's state on the pump's thread)
                let watch = self.watch.lock().unwrap().clone();
                if let Some(w) = watch {
                    self.push(json!({"type": "event", "id": w, "event": {"type": "caught_up", "epoch": "E1", "head": 1}}));
                    self.push(json!({"type": "event", "id": w, "event":
                        {"type": "live", "request_id": "r-9", "frame": {"type": "status", "text": "Working"}}}));
                }
                self.push(json!({"type": "status", "status":
                    {"approvals": [], "running": [], "results": [], "stopped": false}}));
                self.push(json!({"type": "bulk", "allowed": false, "tier": "shared_relay"}));
                self.push(json!({"type": "result", "id": id, "result": {"marks": {}}}));
            }
            _ => self.push(json!({"type": "result", "id": id, "result": {}})),
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

/// The freeze itself, through the real pump: the Mac's frames and its pushed status arrive ahead of the answer to
/// `chat.entries.mark`. Before the fix the sync kept the state locked across that call, the pump waited for the state
/// on the first frame, and the call's answer behind it was never read: the sync (and every reader behind it) hung for
/// the door's whole backstop, 60 s. Now the sync ends at once and a reader is answered during it.
// decision: PHN-680
// decision: PHN-681
#[test]
fn a_frame_ahead_of_a_calls_answer_never_stalls_the_sync_or_a_reader() {
    let phone = Phone::new();
    let m = Arc::new(phone.open());
    let door = Arc::new(StormDoor::default());
    let weak = Arc::downgrade(&m);
    let t = Arc::new(DoorTransport::start(
        door.clone(),
        Arc::new(move |text: String| {
            // as the phone library does (ferb-mobile-ffi link.rs `forward`): the link's events to the core, on the pump
            if let Some(m) = weak.upgrade() {
                m.link_event(&text);
            }
        }),
    ));
    m.connect(t.clone()).unwrap();
    m.link_event(&online("direct"));
    phone.jobs.0.lock().unwrap().clear();
    let reader = {
        let (m, door) = (m.clone(), door.clone());
        std::thread::spawn(move || {
            // once the mark is on its way, what the app's main thread asked on the owner's phone
            let began = Instant::now();
            while *door.marks.lock().unwrap() == 0 && began.elapsed() < Duration::from_secs(10) {
                std::thread::sleep(Duration::from_millis(5));
            }
            let asked = Instant::now();
            let _ = m.last_status();
            asked.elapsed()
        })
    };
    let began = Instant::now();
    m.sync().unwrap();
    let took = began.elapsed();
    let reader_waited = reader.join().unwrap();
    assert!(
        took < Duration::from_secs(5),
        "the sync waited {took:?} (the door's backstop is 60 s): the pump could not reach the call's answer"
    );
    assert!(
        reader_waited < Duration::from_secs(1),
        "a reader of the last status waited {reader_waited:?} behind the sync"
    );
    assert_eq!(*door.marks.lock().unwrap(), 1);
    assert!(m.link_state().watching);
    t.close();
}
