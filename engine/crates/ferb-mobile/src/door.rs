//! The real link as a `Transport`: an adapter onto ferb-link's JSON door (`ffi::LinkClient`'s `command` and `poll`,
//! the same calls its C and JNI functions expose; ferb-link `src/ffi.rs`). This crate does not depend on ferb-link
//! (its tests need no iroh): slice S8 implements `LinkDoor` for `LinkClient` in two lines and hands it here.
//!
//! One pump thread polls the door and routes each event: a call's `result` or `error` to the caller waiting on it, a
//! stream's `event`s to its watch sink (or, for `chat.send`, the first `ack` to the sender), and the link's own events
//! (`state`, `bulk`, `relay_state`, `persist`, `prove`, and the Mac's pushed `status` and `notice`) to the app's
//! callback, which passes `state`, `bulk`, `relay_state` and `status` to `Mobile::link_event`. A transfer (`get`,
//! `put`) hears its `progress` (and an upload its `accepted`) until its `result` or `error`.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Value};

use crate::transport::{codes, LinkFailure, TransferSink, Transport, WatchHandle, WatchSink};

/// ferb-link's door: `command(json) -> {"ok":true}|{"error":{..}}` at once, `poll(wait_ms) -> event JSON or ""`.
pub trait LinkDoor: Send + Sync {
    fn command(&self, json: &str) -> String;
    fn poll(&self, wait_ms: u64) -> String;
}

/// How long a caller waits for its call's outcome at most, whatever the link does: the link's own deadline answers
/// first (`call_deadline_s` + 5 = 15 s); this is the backstop for a link that went silent, equal to the door's
/// longest poll (ferb-link `POLL_WAIT_MAX`).
pub const CALL_BACKSTOP: Duration = Duration::from_secs(60);
/// How long one poll of the pump waits (the pump checks for its own close in between).
pub const PUMP_POLL_MS: u64 = 500;
/// How long a transfer may go without a word (no progress, no result) before its caller stops waiting: the link's
/// own stall checks end a dead transfer first; the Mac may take a while to hash a very large file before its first
/// byte (the bridge waits up to 30 minutes for that), so this backstop is as long.
pub const TRANSFER_SILENCE: Duration = Duration::from_secs(1800);
/// The link's own events (not a call's) the app hears.
const LINK_EVENTS: [&str; 8] = [
    "state",
    "bulk",
    "relay_state",
    "persist",
    "prove",
    "status",
    "notice",
    "location_wanted",
];

enum Waiter {
    Call(mpsc::Sender<Result<Value, LinkFailure>>),
    /// `chat.send`: the first `ack` (or the refusal) goes to the sender; the rest of the stream is not read here.
    Send(Option<mpsc::Sender<Result<Value, LinkFailure>>>),
    Watch(Arc<dyn WatchSink>),
    /// A file or picture under way: its progress to the sink, its outcome to the caller.
    Transfer(
        mpsc::Sender<Result<Value, LinkFailure>>,
        Arc<dyn TransferSink>,
        Arc<AtomicU64>,
    ),
}

struct Inner {
    door: Arc<dyn LinkDoor>,
    waiters: Mutex<HashMap<String, Waiter>>,
    next: AtomicU64,
    closed: AtomicBool,
    on_event: Arc<dyn Fn(String) + Send + Sync>,
}

/// The link as the core's transport. `close` stops the pump; the app closes the `LinkClient` itself.
pub struct DoorTransport {
    inner: Arc<Inner>,
}

fn closed() -> LinkFailure {
    LinkFailure::new(codes::LINK_CLOSED, "This link was closed.")
}

impl DoorTransport {
    /// Starts the pump. `on_event` gets every event that is not a call's (the link's `state`, `bulk`, `relay_state`,
    /// `persist`, `prove`), as JSON text.
    pub fn start(
        door: Arc<dyn LinkDoor>,
        on_event: Arc<dyn Fn(String) + Send + Sync>,
    ) -> DoorTransport {
        let inner = Arc::new(Inner {
            door,
            waiters: Mutex::new(HashMap::new()),
            next: AtomicU64::new(1),
            closed: AtomicBool::new(false),
            on_event,
        });
        let pump = inner.clone();
        let _ = std::thread::Builder::new()
            .name("ferb-mobile-door".into())
            .spawn(move || {
                while !pump.closed.load(Ordering::SeqCst) {
                    let text = pump.door.poll(PUMP_POLL_MS);
                    if text.is_empty() {
                        continue;
                    }
                    pump.route(&text);
                }
            });
        DoorTransport { inner }
    }

    /// Stops the pump; every caller still waiting gets `link_closed`, every watch ends.
    pub fn close(&self) {
        if self.inner.closed.swap(true, Ordering::SeqCst) {
            return;
        }
        let waiters: Vec<Waiter> = self
            .inner
            .waiters
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .drain()
            .map(|(_, w)| w)
            .collect();
        for w in waiters {
            match w {
                Waiter::Call(tx) | Waiter::Send(Some(tx)) | Waiter::Transfer(tx, _, _) => {
                    let _ = tx.send(Err(closed()));
                }
                Waiter::Watch(sink) => sink.ended(Some(closed())),
                Waiter::Send(None) => {}
            }
        }
    }

    fn id(&self, what: &str) -> String {
        format!(
            "m-{what}-{}",
            self.inner.next.fetch_add(1, Ordering::SeqCst)
        )
    }

    /// Hands a call to the door; the door's immediate refusal comes back as the failure.
    fn issue(
        &self,
        id: &str,
        method: &str,
        params: Value,
        proof: Option<Value>,
        waiter: Waiter,
    ) -> Result<(), LinkFailure> {
        let mut cmd = json!({"cmd": "call", "id": id, "method": method, "params": params});
        if let Some(p) = proof {
            cmd["proof"] = p;
        }
        self.issue_command(id, cmd, waiter)
    }

    /// Hands any command with an id to the door (a call, a `get`, a `put`).
    fn issue_command(&self, id: &str, cmd: Value, waiter: Waiter) -> Result<(), LinkFailure> {
        if self.inner.closed.load(Ordering::SeqCst) {
            return Err(closed());
        }
        self.inner.lock().insert(id.to_string(), waiter);
        let reply: Value =
            serde_json::from_str(&self.inner.door.command(&cmd.to_string())).unwrap_or(Value::Null);
        if let Some(e) = reply.get("error") {
            self.inner.lock().remove(id);
            return Err(LinkFailure::from_json(e));
        }
        Ok(())
    }

    fn wait(
        &self,
        id: &str,
        rx: mpsc::Receiver<Result<Value, LinkFailure>>,
    ) -> Result<Value, LinkFailure> {
        match rx.recv_timeout(CALL_BACKSTOP) {
            Ok(r) => r,
            Err(_) => {
                self.inner.lock().remove(id);
                Err(LinkFailure::new(
                    codes::MAC_UNREACHABLE,
                    "FERB's Mac did not answer.",
                ))
            }
        }
    }
}

impl Drop for DoorTransport {
    fn drop(&mut self) {
        self.close();
    }
}

impl Inner {
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Waiter>> {
        self.waiters.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn route(&self, text: &str) {
        let Ok(ev) = serde_json::from_str::<Value>(text) else {
            return;
        };
        let id = ev["id"].as_str().map(str::to_string);
        let ty = ev["type"].as_str().unwrap_or_default();
        let Some(id) = id.filter(|i| self.lock().contains_key(i)) else {
            if LINK_EVENTS.contains(&ty) {
                (self.on_event)(text.to_string());
            }
            return;
        };
        match ty {
            "event" => {
                let mut waiters = self.lock();
                match waiters.get_mut(&id) {
                    Some(Waiter::Watch(sink)) => {
                        let sink = sink.clone();
                        drop(waiters);
                        sink.frame(ev["event"].to_string());
                    }
                    Some(Waiter::Send(tx)) if ev["event"]["type"] == "ack" => {
                        if let Some(tx) = tx.take() {
                            let _ = tx.send(Ok(ev["event"].clone()));
                        }
                    }
                    _ => {}
                }
            }
            "progress" | "accepted" => {
                let waiters = self.lock();
                if let Some(Waiter::Transfer(_, sink, last)) = waiters.get(&id) {
                    let (sink, last) = (sink.clone(), last.clone());
                    drop(waiters);
                    last.store(now_s(), Ordering::SeqCst);
                    if ty == "progress" {
                        sink.progress(
                            ev["done"].as_u64().unwrap_or(0),
                            ev["of"].as_u64().unwrap_or(0),
                        );
                    } else {
                        sink.accepted(
                            ev["upload_id"].as_str().unwrap_or_default(),
                            ev["offset"].as_u64().unwrap_or(0),
                        );
                    }
                }
            }
            "result" | "error" => {
                let Some(w) = self.lock().remove(&id) else {
                    return;
                };
                let outcome = if ty == "result" {
                    Ok(ev["result"].clone())
                } else {
                    Err(LinkFailure::from_json(&ev["error"]))
                };
                match w {
                    Waiter::Call(tx) => {
                        let _ = tx.send(outcome);
                    }
                    Waiter::Send(Some(tx)) => {
                        // the stream ended before any ack: a refusal, or a stream with no acceptance
                        let _ = tx.send(match outcome {
                            Err(f) => Err(f),
                            Ok(_) => Err(LinkFailure::new(
                                codes::FERB_ERROR,
                                "FERB did not accept the message.",
                            )),
                        });
                    }
                    Waiter::Send(None) => {}
                    Waiter::Watch(sink) => sink.ended(outcome.err()),
                    Waiter::Transfer(tx, _, _) => {
                        let _ = tx.send(outcome);
                    }
                }
            }
            _ => {}
        }
    }
}

struct DoorWatch {
    inner: Arc<Inner>,
    id: String,
}

impl WatchHandle for DoorWatch {
    /// Stops delivering this watch's frames. (ferb-link's `cancel` command stops transfers, not calls: the stream
    /// itself ends when the link's connection does; slice S8 may add call cancellation to the door.)
    fn close(&self) {
        let _ = self.inner.lock().remove(&self.id);
        let _ = self
            .inner
            .door
            .command(&json!({"cmd": "cancel", "target": self.id}).to_string());
    }
}

impl Transport for DoorTransport {
    fn call(
        &self,
        method: &str,
        params_json: &str,
        proof_json: Option<&str>,
    ) -> Result<String, LinkFailure> {
        let id = self.id("call");
        let (tx, rx) = mpsc::channel();
        let params: Value = serde_json::from_str(params_json).unwrap_or(json!({}));
        let proof = proof_json.and_then(|p| serde_json::from_str(p).ok());
        self.issue(&id, method, params, proof, Waiter::Call(tx))?;
        self.wait(&id, rx).map(|v| v.to_string())
    }

    fn send(&self, params_json: &str) -> Result<String, LinkFailure> {
        let id = self.id("send");
        let (tx, rx) = mpsc::channel();
        let params: Value = serde_json::from_str(params_json).unwrap_or(json!({}));
        self.issue(&id, "chat.send", params, None, Waiter::Send(Some(tx)))?;
        self.wait(&id, rx).map(|v| v.to_string())
    }

    fn watch(
        &self,
        params_json: &str,
        sink: Arc<dyn WatchSink>,
    ) -> Result<Box<dyn WatchHandle>, LinkFailure> {
        let id = self.id("watch");
        let params: Value = serde_json::from_str(params_json).unwrap_or(json!({}));
        self.issue(&id, "chat.entries.watch", params, None, Waiter::Watch(sink))?;
        Ok(Box::new(DoorWatch {
            inner: self.inner.clone(),
            id,
        }))
    }

    fn get(
        &self,
        id: &str,
        params_json: &str,
        save_to: Option<&str>,
        append: bool,
        sink: Arc<dyn TransferSink>,
    ) -> Result<String, LinkFailure> {
        let params: Value = serde_json::from_str(params_json).unwrap_or(json!({}));
        let mut cmd = json!({"cmd": "get", "id": id, "params": params});
        if let Some(p) = save_to {
            cmd["save_to"] = json!(p);
            cmd["append"] = json!(append);
        }
        self.transfer(id, cmd, sink)
    }

    fn put(
        &self,
        id: &str,
        path: &str,
        opts_json: &str,
        sink: Arc<dyn TransferSink>,
    ) -> Result<String, LinkFailure> {
        let mut cmd = json!({"cmd": "put", "id": id, "path": path});
        if let Ok(Value::Object(opts)) = serde_json::from_str::<Value>(opts_json) {
            for k in ["name", "resume", "group", "rel"] {
                if let Some(v) = opts.get(k).filter(|v| !v.is_null()) {
                    cmd[k] = v.clone();
                }
            }
        }
        self.transfer(id, cmd, sink)
    }

    /// Stops a transfer (or a call) under way: the door aborts it (no result follows for its id), so its caller is
    /// answered here with `cancelled`.
    fn cancel(&self, id: &str) {
        let w = self.inner.lock().remove(id);
        let _ = self
            .inner
            .door
            .command(&json!({"cmd": "cancel", "target": id}).to_string());
        let stopped = || LinkFailure::new(codes::CANCELLED, "Stopped on the phone.");
        match w {
            Some(Waiter::Call(tx))
            | Some(Waiter::Send(Some(tx)))
            | Some(Waiter::Transfer(tx, _, _)) => {
                let _ = tx.send(Err(stopped()));
            }
            Some(Waiter::Watch(sink)) => sink.ended(None),
            _ => {}
        }
    }
}

/// Seconds on a monotonic clock of this process (a transfer's last word).
fn now_s() -> u64 {
    static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    START
        .get_or_init(std::time::Instant::now)
        .elapsed()
        .as_secs()
}

impl DoorTransport {
    /// A transfer: its outcome, waited for as long as it keeps saying something (`TRANSFER_SILENCE`).
    fn transfer(
        &self,
        id: &str,
        cmd: Value,
        sink: Arc<dyn TransferSink>,
    ) -> Result<String, LinkFailure> {
        let (tx, rx) = mpsc::channel();
        let last = Arc::new(AtomicU64::new(now_s()));
        self.issue_command(id, cmd, Waiter::Transfer(tx, sink, last.clone()))?;
        loop {
            match rx.recv_timeout(Duration::from_secs(5)) {
                Ok(r) => return r.map(|v| v.to_string()),
                Err(mpsc::RecvTimeoutError::Disconnected) => return Err(closed()),
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    if now_s().saturating_sub(last.load(Ordering::SeqCst))
                        > TRANSFER_SILENCE.as_secs()
                    {
                        self.cancel(id);
                        return Err(LinkFailure::new(
                            codes::MAC_UNREACHABLE,
                            "FERB's Mac stopped sending.",
                        ));
                    }
                }
            }
        }
    }
}
