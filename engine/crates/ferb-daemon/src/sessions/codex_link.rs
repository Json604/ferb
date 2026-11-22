//! FERB's connection to Codex's own daemon (`~/.codex/app-server-control/app-server-control.sock`, JSON-RPC over a
//! WebSocket): one connection while sessions are on, tried again at most every [`RETRY_S`] while Codex isn't there.
//! A reader thread takes the daemon's messages: answers go to the call that waits for them, everything else to a
//! second thread that updates the sessions (so an update may itself ask the daemon without waiting on itself).

use std::collections::HashMap;
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

use ferb_sessions::{codex, ws};
use serde_json::{json, Value};

use super::{lock, Sessions};

/// How often FERB looks for Codex's daemon again while it isn't there.
pub const RETRY_S: u64 = 10;
/// How long a call waits for Codex's answer.
pub const CALL_S: u64 = 10;
/// The largest message FERB reads from Codex (a long thread read whole).
pub const MAX_MESSAGE: usize = 64 * 1024 * 1024;

type Pending = Arc<Mutex<HashMap<u64, mpsc::Sender<Result<Value, String>>>>>;

struct Conn {
    writer: Arc<Mutex<UnixStream>>,
    pending: Pending,
    next: Arc<AtomicU64>,
    alive: Arc<AtomicBool>,
}

struct Inner {
    socket: PathBuf,
    version: String,
    conn: Mutex<Option<Conn>>,
    last_try: Mutex<Option<Instant>>,
}

/// The link (cheap to clone).
#[derive(Clone)]
pub struct CodexLink(Arc<Inner>);

impl CodexLink {
    pub fn new(socket: PathBuf, version: String) -> CodexLink {
        CodexLink(Arc::new(Inner {
            socket,
            version,
            conn: Mutex::new(None),
            last_try: Mutex::new(None),
        }))
    }

    pub fn connected(&self) -> bool {
        lock(&self.0.conn)
            .as_ref()
            .is_some_and(|c| c.alive.load(Ordering::SeqCst))
    }

    /// Ends the connection (sessions turned off).
    pub fn stop(&self) {
        if let Some(c) = lock(&self.0.conn).take() {
            c.alive.store(false, Ordering::SeqCst);
            let _ = lock(&c.writer).shutdown(std::net::Shutdown::Both);
        }
    }

    /// Connects when Codex's daemon is there and FERB isn't connected; lists its threads.
    pub fn ensure(&self, sessions: &Sessions) {
        if self.connected() {
            return;
        }
        {
            let mut last = lock(&self.0.last_try);
            if last.is_some_and(|t| t.elapsed() < Duration::from_secs(RETRY_S)) {
                return;
            }
            *last = Some(Instant::now());
        }
        if !self.0.socket.exists() {
            return;
        }
        match self.connect(sessions) {
            Ok(()) => {
                for t in self.loaded() {
                    sessions.note_codex(&t, true);
                }
            }
            Err(e) => (sessions.seams.journal)("sessions.codex_unreachable", json!({"error": e})),
        }
    }

    fn connect(&self, sessions: &Sessions) -> Result<(), String> {
        let mut s = UnixStream::connect(&self.0.socket).map_err(|e| e.to_string())?;
        s.set_read_timeout(Some(Duration::from_secs(CALL_S)))
            .map_err(|e| e.to_string())?;
        s.write_all(ws::handshake(&ws::new_key()).as_bytes())
            .map_err(|e| e.to_string())?;
        let mut head = Vec::new();
        let mut b = [0u8; 1];
        while !head.ends_with(b"\r\n\r\n") {
            if head.len() > 16 * 1024 || s.read(&mut b).map_err(|e| e.to_string())? == 0 {
                return Err("Codex's daemon closed the connection".into());
            }
            head.push(b[0]);
        }
        if !ws::accepted(&String::from_utf8_lossy(&head)) {
            return Err("Codex's daemon didn't accept the connection".into());
        }
        s.set_read_timeout(None).map_err(|e| e.to_string())?;
        let reader = s.try_clone().map_err(|e| e.to_string())?;
        let conn = Conn {
            writer: Arc::new(Mutex::new(s)),
            pending: Arc::default(),
            next: Arc::new(AtomicU64::new(1)),
            alive: Arc::new(AtomicBool::new(true)),
        };
        let (events_tx, events_rx) = mpsc::channel::<codex::Event>();
        let (pending, alive, writer) = (
            conn.pending.clone(),
            conn.alive.clone(),
            conn.writer.clone(),
        );
        std::thread::Builder::new()
            .name("ferb-codex-read".into())
            .spawn(move || read_loop(reader, pending, alive, writer, events_tx))
            .map_err(|e| e.to_string())?;
        let me = sessions.clone();
        let handle = tokio::runtime::Handle::try_current().ok();
        std::thread::Builder::new()
            .name("ferb-codex-events".into())
            .spawn(move || {
                let _g = handle.as_ref().map(|h| h.enter());
                for ev in events_rx {
                    me.on_codex(ev);
                }
            })
            .map_err(|e| e.to_string())?;
        *lock(&self.0.conn) = Some(conn);
        self.call("initialize", json!({"clientInfo": {"name": "ferb", "version": self.0.version}, "capabilities": {"experimentalApi": true}}))?;
        self.write(&codex::initialized())?;
        Ok(())
    }

    fn write(&self, text: &str) -> Result<(), String> {
        let c = lock(&self.0.conn);
        let c = c.as_ref().ok_or("FERB isn't connected to Codex")?;
        let mut w = lock(&c.writer);
        w.write_all(&ws::frame(ws::TEXT, text.as_bytes()))
            .map_err(|e| e.to_string())
    }

    /// One request, answered within [`CALL_S`].
    pub fn call(&self, method: &str, params: Value) -> Result<Value, String> {
        let (id, rx) = {
            let c = lock(&self.0.conn);
            let c = c.as_ref().ok_or("FERB isn't connected to Codex")?;
            let id = c.next.fetch_add(1, Ordering::SeqCst);
            let (tx, rx) = mpsc::channel();
            lock(&c.pending).insert(id, tx);
            (id, rx)
        };
        self.write(&codex::request(id, method, params))?;
        rx.recv_timeout(Duration::from_secs(CALL_S))
            .map_err(|_| "Codex didn't answer in time".to_string())?
    }

    /// The loaded threads, read with their items.
    pub fn loaded(&self) -> Vec<codex::Thread> {
        let Ok(list) = self.call("thread/loaded/list", json!({})) else {
            return Vec::new();
        };
        list.get("data")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .filter_map(|id| self.read_now(id))
            .collect()
    }

    /// One thread, read now (blocking).
    pub fn read_now(&self, thread: &str) -> Option<codex::Thread> {
        if !self.connected() {
            return None;
        }
        let r = self
            .call(
                "thread/read",
                json!({"threadId": thread, "includeTurns": true}),
            )
            .ok()?;
        codex::thread(r.get("thread")?)
    }

    /// The owner's words as a turn: a new turn when the thread is idle, else steered into the running one.
    pub async fn send(&self, thread: &str, text: &str, turn: &str) -> Result<(), String> {
        let (me, thread, text, turn) = (
            self.clone(),
            thread.to_owned(),
            text.to_owned(),
            turn.to_owned(),
        );
        tokio::task::spawn_blocking(move || {
            let input = codex::text_input(&text);
            if turn.is_empty() {
                me.call("turn/start", json!({"threadId": thread, "input": input}))
            } else {
                me.call(
                    "turn/steer",
                    json!({"threadId": thread, "input": input, "expectedTurnId": turn}),
                )
            }
            .map(|_| ())
        })
        .await
        .map_err(|e| e.to_string())?
    }

    /// Codex's own interrupt.
    pub async fn interrupt(&self, thread: &str, turn: &str) -> Result<(), String> {
        let (me, thread, turn) = (self.clone(), thread.to_owned(), turn.to_owned());
        tokio::task::spawn_blocking(move || {
            me.call(
                "turn/interrupt",
                json!({"threadId": thread, "turnId": turn}),
            )
            .map(|_| ())
        })
        .await
        .map_err(|e| e.to_string())?
    }

    /// The owner's answer to one of Codex's requests.
    pub fn respond(&self, id: &Value, result: Value) {
        let _ = self.write(&codex::respond(id, result));
    }
}

fn read_loop(
    mut s: UnixStream,
    pending: Pending,
    alive: Arc<AtomicBool>,
    writer: Arc<Mutex<UnixStream>>,
    events: mpsc::Sender<codex::Event>,
) {
    let mut r = ws::Reader::new(MAX_MESSAGE);
    let mut buf = vec![0u8; 64 * 1024];
    'outer: while alive.load(Ordering::SeqCst) {
        let n = match s.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        r.push(&buf[..n]);
        loop {
            match r.next_message() {
                Ok(None) => break,
                Ok(Some(ws::Message::Ping(p))) => {
                    let _ = lock(&writer).write_all(&ws::frame(ws::PONG, &p));
                }
                Ok(Some(ws::Message::Close)) | Err(_) => break 'outer,
                Ok(Some(ws::Message::Text(t))) => match codex::event(&t) {
                    codex::Event::Answer { id, result } => {
                        if let Some(tx) = lock(&pending).remove(&id) {
                            let _ = tx.send(result);
                        }
                    }
                    ev => {
                        let _ = events.send(ev);
                    }
                },
            }
        }
    }
    alive.store(false, Ordering::SeqCst);
    lock(&pending).clear();
}
