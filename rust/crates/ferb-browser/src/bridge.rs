//! The extension connection: a port of `ext_bridge.py`'s `Hub` and `Bridge` (and `_Conn`,
//! `_pass_through`, `_let_download_go`).
//!
//! The hub takes each native-host connection after its RPC hello (decision: HAND-223: only one a
//! browser FERB knows started), binds authenticated installs to profiles through [`Pairings`],
//! signs what the extension acts on (HAND-222), and journals every connection or message it
//! doesn't take with a reason code (HAND-406). A bridge is one paired profile: its connection
//! generation, the commands in flight by id, commands and replies too big for one message sent in
//! parts, and the commands a dropped connection left unknown, settled from the next hello's
//! `seen` / `owned` lists and never resent (21.15).
//!
//! Seams (another part's work, injected): [`Ledger`] (the orchestrator's intent ledger),
//! [`Publish`] (the bus), [`TabDriver`] (the browser tools' tab drivers), [`Hooks`] (the daemon's
//! trail, sound, downloads and refusal journal), [`BrowserApps`] and the browser process's app
//! (`AppOf`, the platform's libproc and Info.plist read), [`Random`] (command ids).
//!
//! Fixed against Python (named in `tests/transcripts.rs`): when a connection ends, only the
//! commands sent on it fail as unknown; Python fails every command in flight on the bridge, those
//! already sent on the profile's newer connection too.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use ferb_core::interfaces::BoxFuture;
use ferb_core::types::Identity;
use serde_json::{json, Map, Value};
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::{oneshot, watch};
use tokio::task::JoinHandle;

use crate::error::{Error, Result};
use crate::pairing::{token_hex, Binding, PairingKey, Pairings, Random};
use crate::protocol::{
    self, command, daemon_envelope, CmdResult, Event, ExtMessage, ExtType, Hello, TabEvent,
    MAX_MESSAGE, MIN_EXTENSION_VERSION, PROTOCOL_VERSION,
};
use crate::rules::{self, BrowserApps};

/// `CMD_TIMEOUT_S` (HAND-440).
pub const CMD_TIMEOUT_S: f64 = 30.0;
/// `KEEPALIVE_S`: while a task tab is open a ping keeps the MV3 worker awake (idle limit 30 s).
pub const KEEPALIVE_S: f64 = 15.0;
/// A command whose signed line is longer than this goes in `command_part`s of `PART_CHARS`.
pub const PART_LIMIT: usize = 500_000;
pub const PART_CHARS: usize = 100_000;

/// The bus (`publish(topic, data)`).
pub type Publish = Arc<dyn Fn(&str, Map<String, Value>) + Send + Sync>;

/// The orchestrator's intent ledger, as the bridge uses it (`ferb/orchestrator/ledger.py`).
pub trait Ledger: Send + Sync {
    /// The intent's state ("unknown", "done", ...); `None` when the ledger has no such intent.
    fn state(&self, intent_id: &str) -> Option<String>;
    fn reconcile(&self, intent_id: &str, observed: &str, evidence: Vec<String>);
}

/// The live task that drives a tab (`downloads.TaskTab`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TaskTab {
    pub task: String,
    pub tab: String,
    pub identity: String,
}

/// What a driver makes of a popup a task tab's page opened (`_TabDriver.adopt`).
#[derive(Clone, Debug, PartialEq)]
pub enum Adopted {
    /// The extension's record for it (`adopt_tab`).
    Record(Map<String, Value>),
    /// Past the task's tab cap: closed while blank (`popup_refused`).
    Refused(String),
}

/// What the hub asks of a browser tools' tab driver (`_TabDriver`; the browser tools part).
pub trait TabDriver: Send + Sync {
    fn identity(&self) -> Identity;
    fn task_tab(&self, br: &Arc<Bridge>, chrome_id: i64) -> Option<TaskTab>;
    fn holds(&self, br: &Arc<Bridge>, chrome_id: i64) -> bool;
    fn tab_closed(&self, br: &Arc<Bridge>, chrome_id: i64, by_page: bool);
    fn tab_taken(&self, br: &Arc<Bridge>, chrome_id: i64, how: &str);
    /// After a hello: the jobs that close or let go of tabs whose task ended (run after the welcome).
    fn readopt(
        &self,
        br: &Arc<Bridge>,
        listed: &[Map<String, Value>],
    ) -> Vec<BoxFuture<'static, ()>>;
    fn open_from(
        &self,
        br: Arc<Bridge>,
        opener: i64,
        url: String,
        window_id: Option<i64>,
    ) -> BoxFuture<'static, ()>;
    fn adopt(&self, br: &Arc<Bridge>, opener: i64, chrome_id: i64, url: &str) -> Option<Adopted>;
    fn download_tab_done(&self, br: Arc<Bridge>, chrome_id: i64) -> BoxFuture<'static, ()>;
    /// This driver's still-open tabs of a task tree and the opens in flight (HAND-234: the tab cap
    /// counts every driver's).
    fn open_count(&self, root: &str) -> i64;
}

type BridgeHook<T> = Arc<dyn Fn(&Arc<Bridge>, &T) + Send + Sync>;
/// A hook that may hand back a job to run in the background.
pub type TabClosedHook =
    Arc<dyn Fn(&Arc<Bridge>, i64) -> Option<BoxFuture<'static, ()>> + Send + Sync>;
/// Whether FERB owns a tab of a bridge.
pub type OwnedHook = Arc<dyn Fn(&Arc<Bridge>, i64) -> bool + Send + Sync>;

/// What the daemon wires into the hub (`Hub.on_*`, `paused`, `owned`, `record_refusal`). Each
/// defaults to Python's default.
#[derive(Clone)]
pub struct Hooks {
    /// An unpaired copy said hello: FERB can pair it (L25).
    pub on_unpaired: Arc<dyn Fn() + Send + Sync>,
    /// The activity trail: every tab event (while paused, only tabs FERB owns).
    pub on_tab: BridgeHook<TabEvent>,
    /// A tab closed: FERB's own resources tied to it can go. A returned job runs in the background.
    pub on_tab_closed: TabClosedHook,
    pub paused: Arc<dyn Fn() -> bool + Send + Sync>,
    pub owned: OwnedHook,
    /// HAND-207: a tab's sound changed.
    pub on_sound: BridgeHook<TabEvent>,
    /// A download the extension holds (D35); `None`: back to the browser untouched.
    pub on_download: Option<BridgeHook<Event>>,
    /// HAND-406: every connection or message FERB doesn't take (never a message's contents).
    pub record_refusal: Arc<dyn Fn(Map<String, Value>) + Send + Sync>,
}

impl Default for Hooks {
    fn default() -> Hooks {
        Hooks {
            on_unpaired: Arc::new(|| {}),
            on_tab: Arc::new(|_, _| {}),
            on_tab_closed: Arc::new(|_, _| None),
            paused: Arc::new(|| false),
            owned: Arc::new(|_, _| false),
            on_sound: Arc::new(|_, _| {}),
            on_download: None,
            record_refusal: Arc::new(|_| {}),
        }
    }
}

/// The bundle id of the app whose process started the native host (the host's parent, the browser):
/// the platform's part (`_app_bundle`: libproc's path, then the outermost `.app`'s Info.plist,
/// HAND-438). Called on the connection's task: it must be quick.
pub type AppOf = Arc<dyn Fn(Option<i32>) -> Option<String> + Send + Sync>;

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

fn dumps(v: &Value) -> String {
    ferb_py::json::dumps(v)
}

static CONN_IDS: AtomicU64 = AtomicU64::new(1);

type Writer = tokio::sync::Mutex<Box<dyn AsyncWrite + Send + Unpin>>;

/// One native-host connection (`_Conn`).
pub struct Conn {
    id: u64,
    writer: Writer,
    /// The browser process behind this native host (L26: hear its tab's app).
    pub browser_pid: Option<i32>,
    /// That process's app bundle id (`peer_refusal`).
    pub app: Option<String>,
    state: Mutex<ConnState>,
}

#[derive(Default)]
struct ConnState {
    install_id: Option<String>,
    bridge: Option<Arc<Bridge>>,
    gen: i64,
    unpaired_install: Option<String>,
    /// Protocol 18: the hello's signing nonce for this connection.
    nonce: String,
    /// The pairing key the daemon signs with.
    key: Option<PairingKey>,
    /// The last counter the daemon signed on this connection.
    dseq: i64,
}

impl Conn {
    fn new(
        writer: Box<dyn AsyncWrite + Send + Unpin>,
        browser_pid: Option<i32>,
        app: Option<String>,
    ) -> Conn {
        Conn {
            id: CONN_IDS.fetch_add(1, Ordering::Relaxed),
            writer: tokio::sync::Mutex::new(writer),
            browser_pid,
            app,
            state: Mutex::default(),
        }
    }

    /// One line (the text and a newline), flushed.
    async fn write_line(&self, text: &str) -> std::io::Result<()> {
        let mut w = self.writer.lock().await;
        w.write_all(format!("{text}\n").as_bytes()).await?;
        w.flush().await
    }

    fn gen(&self) -> i64 {
        lock(&self.state).gen
    }

    fn bridge(&self) -> Option<Arc<Bridge>> {
        lock(&self.state).bridge.clone()
    }
}

enum Delivery {
    Result(Box<CmdResult>),
    /// The connection the command went on dropped (the message).
    Dropped(String),
}

struct Pending {
    tx: Option<oneshot::Sender<Delivery>>,
    conn: u64,
}

struct BridgeState {
    gen: i64,
    conn: Option<Arc<Conn>>,
    version: String,
    file_access: Option<bool>,
    pending: HashMap<String, Pending>,
    lost: BTreeSet<String>,
    tabs: BTreeMap<i64, (String, bool)>,
    navs: BTreeMap<i64, i64>,
    detached: BTreeMap<i64, String>,
    holds: i64,
    hellos: i64,
    window: Option<i64>,
    keepalive: Option<JoinHandle<()>>,
}

/// One paired profile's connection state (`Bridge`).
pub struct Bridge {
    pub binding: Binding,
    pub device_id: String,
    ledger: Arc<dyn Ledger>,
    random: Arc<dyn Random>,
    state: Mutex<BridgeState>,
    connected: watch::Sender<bool>,
}

/// What a bridge's state looks like now (for the tools, the daemon's listings and the tests).
#[derive(Clone, Debug, PartialEq)]
pub struct BridgeFacts {
    pub gen: i64,
    pub connected: bool,
    pub version: String,
    pub file_access: Option<bool>,
    pub lost: Vec<String>,
    /// Chrome tab id -> (host, active).
    pub tabs: BTreeMap<i64, (String, bool)>,
    /// Chrome tab id -> loads seen (stale-ref detection).
    pub navs: BTreeMap<i64, i64>,
    /// Chrome tab id -> why the debugger left.
    pub detached: BTreeMap<i64, String>,
    pub hellos: i64,
    pub holds: i64,
    pub window: Option<i64>,
}

impl Bridge {
    fn new(
        binding: Binding,
        ledger: Arc<dyn Ledger>,
        random: Arc<dyn Random>,
        device_id: String,
    ) -> Bridge {
        Bridge {
            binding,
            device_id,
            ledger,
            random,
            state: Mutex::new(BridgeState {
                gen: 0,
                conn: None,
                version: String::new(),
                file_access: None,
                pending: HashMap::new(),
                lost: BTreeSet::new(),
                tabs: BTreeMap::new(),
                navs: BTreeMap::new(),
                detached: BTreeMap::new(),
                holds: 0,
                hellos: 0,
                window: None,
                keepalive: None,
            }),
            connected: watch::channel(false).0,
        }
    }

    /// "browser/folder".
    pub fn name(&self) -> String {
        format!("{}/{}", self.binding.browser, self.binding.folder)
    }

    pub fn facts(&self) -> BridgeFacts {
        let s = lock(&self.state);
        BridgeFacts {
            gen: s.gen,
            connected: s.conn.is_some(),
            version: s.version.clone(),
            file_access: s.file_access,
            lost: s.lost.iter().cloned().collect(),
            tabs: s.tabs.clone(),
            navs: s.navs.clone(),
            detached: s.detached.clone(),
            hellos: s.hellos,
            holds: s.holds,
            window: s.window,
        }
    }

    pub fn gen(&self) -> i64 {
        lock(&self.state).gen
    }

    pub fn is_connected(&self) -> bool {
        lock(&self.state).conn.is_some()
    }

    /// The extension version the current connection's hello said ("" before any).
    pub fn version(&self) -> String {
        lock(&self.state).version.clone()
    }

    /// 0.19.9 (HAND-500): the hello's `file_access`, `None` from an older extension.
    pub fn file_access(&self) -> Option<bool> {
        lock(&self.state).file_access
    }

    pub fn set_file_access(&self, v: Option<bool>) {
        lock(&self.state).file_access = v;
    }

    /// The loads seen in this tab (`navs`).
    pub fn navs(&self, chrome_id: i64) -> i64 {
        lock(&self.state).navs.get(&chrome_id).copied().unwrap_or(0)
    }

    pub fn detached(&self, chrome_id: i64) -> Option<String> {
        lock(&self.state).detached.get(&chrome_id).cloned()
    }

    /// The window FERB chose for a tab here last (remembered).
    pub fn window(&self) -> Option<i64> {
        lock(&self.state).window
    }

    pub fn set_window(&self, w: Option<i64>) {
        lock(&self.state).window = w;
    }

    /// The browser process behind the current connection.
    pub fn browser_pid(&self) -> Option<i32> {
        lock(&self.state).conn.as_ref().and_then(|c| c.browser_pid)
    }

    /// Ids of commands in flight when a connection dropped (`lost`).
    pub fn lost(&self) -> Vec<String> {
        lock(&self.state).lost.iter().cloned().collect()
    }

    /// Waits until the bridge is connected or `wait` passes (`_TabDriver._came`).
    pub async fn came(&self, wait: Duration) -> bool {
        let mut rx = self.connected.subscribe();
        let done = tokio::time::timeout(wait, rx.wait_for(|c| *c)).await;
        done.is_ok_and(|r| r.is_ok()) || self.is_connected()
    }

    /// `envelope`: a signed daemon message for this connection: the next counter, the hello's
    /// nonce, the key.
    fn envelope(conn: &Conn, mtype: &'static str, body: &Value) -> String {
        let mut s = lock(&conn.state);
        s.dseq += 1;
        let key = s
            .key
            .as_ref()
            .map(|k| k.expose().to_vec())
            .unwrap_or_default();
        daemon_envelope(&key, &s.nonce, s.dseq, mtype, body).wire()
    }

    /// One command (`Bridge.call`). A read (READS, or `dialog` without a decision) that fails,
    /// times out or loses its connection is a known failure (DriverError, HAND-224); an acting
    /// command whose outcome can't be known is `OutcomeUnknown` and is kept in `lost` until a
    /// hello settles it.
    pub fn call(
        self: &Arc<Self>,
        op: &str,
        args: Value,
        timeout: f64,
        cid: Option<String>,
    ) -> BoxFuture<'static, Result<Value>> {
        let br = Arc::clone(self);
        let op = op.to_string();
        Box::pin(async move { br.call_inner(op, args, timeout, cid).await })
    }

    async fn call_inner(
        self: Arc<Self>,
        op: String,
        args: Value,
        timeout: f64,
        cid: Option<String>,
    ) -> Result<Value> {
        let name = self.name();
        let Some(conn) = lock(&self.state).conn.clone() else {
            return Err(Error::driver(format!("{name} isn't connected")));
        };
        {
            let s = lock(&conn.state);
            if s.key.is_none() || s.nonce.is_empty() {
                return Err(Error::driver(format!(
                    "{name} can't be sent commands: its connection has no signing key yet"
                )));
            }
        }
        let read = protocol::is_read(&op)
            || (op == "dialog" && !args.get("decision").is_some_and(ferb_py::value::truthy));
        let cid = cid.unwrap_or_else(|| format!("c-{}", token_hex(self.random.as_ref(), 8)));
        let (tx, rx) = oneshot::channel();
        lock(&self.state).pending.insert(
            cid.clone(),
            Pending {
                tx: Some(tx),
                conn: conn.id,
            },
        );
        let mut guard = CallGuard {
            bridge: Arc::clone(&self),
            cid: cid.clone(),
            read,
            op: op.clone(),
            args: args.clone(),
            done: false,
        };
        let has_tab = args.get("tab_id").is_some();
        let wire = Self::envelope(
            &conn,
            "cmd",
            &command(
                &cid,
                conn.gen(),
                &op,
                args.clone(),
                (timeout * 1000.0) as i64,
            ),
        );
        let sent: Result<()> = async {
            if wire.len() > PART_LIMIT {
                // Native messaging has a per-message limit, not a deliverable limit. Only the fully assembled
                // original command can perform an action; chunks have separate transport ids.
                let total = wire.len();
                let mut offset = 0;
                while offset < total {
                    let end = (offset + PART_CHARS).min(total);
                    let part = json!({"id": cid, "offset": offset, "text": &wire[offset..end], "total_chars": total});
                    self.call("command_part", part, timeout, None).await?;
                    offset = end;
                }
                Ok(())
            } else {
                conn.write_line(&wire).await.map_err(|e| write_failed(&op, &name, read, &e))
            }
        }
        .await;
        let outcome = match sent {
            Err(e) => {
                guard.done = true;
                lock(&self.state).pending.remove(&cid);
                if e.unknown && read {
                    lock(&self.state).lost.remove(&cid);
                    return Err(Error::driver(format!(
                        "{op}: {name} disconnected; nothing changed, try it again"
                    )));
                }
                return Err(e);
            }
            Ok(()) => {
                let wait = Duration::from_secs_f64(timeout + 5.0);
                tokio::time::timeout(wait, async {
                    match rx.await {
                        Ok(d) => d,
                        // its pending entry was replaced by a newer command with the same id: never answered
                        Err(_) => std::future::pending().await,
                    }
                })
                .await
            }
        };
        guard.done = true;
        lock(&self.state).pending.remove(&cid);
        let r = match outcome {
            Err(_elapsed) => {
                if op == "pointer" {
                    self.cancel_pointer(&args, &cid).await;
                }
                if read && has_tab {
                    return Err(rules::not_responding(&op, timeout, None, None));
                }
                if read {
                    return Err(Error::driver(format!("{op} timed out in {name}")));
                }
                lock(&self.state).lost.insert(cid);
                return Err(Error::unknown(format!("{op} timed out in {name}")));
            }
            Ok(Delivery::Dropped(message)) => {
                if read {
                    lock(&self.state).lost.remove(&cid);
                    return Err(Error::driver(format!(
                        "{op}: {name} disconnected; nothing changed, try it again"
                    )));
                }
                return Err(Error::unknown(message));
            }
            Ok(Delivery::Result(r)) => *r,
        };
        let error = r.error.clone().unwrap_or_default();
        match r.status.as_str() {
            "ok" => {
                if let Some(transfer) = r.value.as_object().and_then(|v| v.get("result_parts")) {
                    return self.result_parts(transfer.clone(), timeout).await;
                }
                return Ok(r.value);
            }
            "stale" => return Err(Error::stale("the browser reconnected; take a new snapshot")),
            "duplicate" => {
                return Err(Error::driver(format!(
                    "{op} was already sent once; not repeating it"
                )))
            }
            _ => {}
        }
        if !read
            && (r.outcome.as_deref() == Some("unknown")
                || error == "timeout"
                || error.starts_with("detached:"))
        {
            lock(&self.state).lost.insert(cid);
            let shown = r.error.clone().unwrap_or_else(|| "None".into());
            return Err(Error::unknown(format!("{op}: {shown}; outcome unknown")));
        }
        if let Some(rest) = error.strip_prefix("debugger_refused:") {
            let mut facts = Map::new();
            facts.insert("reason".into(), "debugger_refused".into());
            return Err(Error::driver_with(ferb_py::value::strip(rest), facts));
        }
        if read && error == "timeout" && has_tab {
            return Err(rules::not_responding(
                &op,
                timeout,
                r.step.as_deref(),
                r.steps.clone().map(Value::Object).as_ref(),
            ));
        }
        if let Some(rest) = error.strip_prefix("dialog_pending:") {
            let mut facts = Map::new();
            facts.insert("reason".into(), "dialog_pending".into());
            if let Some(d) = r.dialog.clone().filter(|d| !d.is_empty()) {
                facts.insert("dialog".into(), Value::Object(d));
            }
            return Err(Error::driver_with(ferb_py::value::strip(rest), facts));
        }
        Err(Error::driver(if error.is_empty() {
            format!("{op} failed")
        } else {
            error
        }))
    }

    /// A reply too big for one message: its parts, read in order, joined and parsed.
    async fn result_parts(self: &Arc<Self>, transfer: Value, timeout: f64) -> Result<Value> {
        let malformed = || Error::driver("the browser's reply parts weren't in FERB's format");
        let id = transfer.get("id").cloned().ok_or_else(malformed)?;
        let mut pieces = String::new();
        let mut offset = json!(0);
        while !offset.is_null() {
            let part = self
                .call(
                    "result_part",
                    json!({"id": id, "offset": offset}),
                    timeout,
                    None,
                )
                .await?;
            pieces.push_str(
                part.get("text")
                    .and_then(Value::as_str)
                    .ok_or_else(malformed)?,
            );
            offset = part.get("next_offset").cloned().ok_or_else(malformed)?;
        }
        serde_json::from_str(&pieces).map_err(|_| malformed())
    }

    async fn cancel_pointer(self: &Arc<Self>, args: &Value, cid: &str) {
        let Some(tab) = args.get("tab_id").cloned() else {
            return;
        };
        // disconnected: the extension also cancels on disconnect
        let _ = self
            .call(
                "pointer_cancel",
                json!({"tab_id": tab, "command_id": cid}),
                2.0,
                None,
            )
            .await;
    }

    /// `attach`: this connection's hello was accepted.
    fn attach(self: &Arc<Self>, conn: &Arc<Conn>, hello: &Hello) {
        let old;
        {
            let mut s = lock(&self.state);
            s.hellos += 1;
            if s.conn.as_ref().is_some_and(|c| Arc::ptr_eq(c, conn)) {
                // a second hello on the same connection (right after pairing): same generation, in-flight commands stay
                lock(&conn.state).bridge = Some(Arc::clone(self));
                s.version = hello.version.clone();
                s.file_access = hello.file_access;
                drop(s);
                self.reconcile(hello);
                return;
            }
            s.gen += 1;
            {
                let mut c = lock(&conn.state);
                c.gen = s.gen;
                c.bridge = Some(Arc::clone(self));
            }
            old = s.conn.replace(Arc::clone(conn));
            s.version = hello.version.clone();
            s.file_access = hello.file_access;
            s.detached.clear();
        }
        if let Some(old) = old {
            self.drop_conn(&old);
        }
        self.reconcile(hello);
        self.connected.send_replace(true);
        self.keep();
    }

    /// `drop`: a connection ended (or was replaced). The commands sent on it are lost.
    fn drop_conn(&self, conn: &Arc<Conn>) {
        let mut s = lock(&self.state);
        if s.conn.as_ref().is_some_and(|c| Arc::ptr_eq(c, conn)) {
            s.conn = None;
            self.connected.send_replace(false);
            if let Some(k) = s.keepalive.take() {
                k.abort();
            }
        }
        let name = self.name();
        let ids: Vec<String> = s
            .pending
            .iter()
            .filter(|(_, p)| p.conn == conn.id)
            .map(|(k, _)| k.clone())
            .collect();
        for cid in ids {
            if let Some(mut p) = s.pending.remove(&cid) {
                if let Some(tx) = p.tx.take() {
                    s.lost.insert(cid.clone());
                    let _ = tx.send(Delivery::Dropped(format!(
                        "{name} disconnected mid-command; outcome unknown"
                    )));
                }
            }
        }
    }

    /// A task tab is open here: keep the extension's worker awake until the last one closes.
    pub fn hold(self: &Arc<Self>) {
        lock(&self.state).holds += 1;
        self.keep();
    }

    pub fn unhold(&self) {
        let mut s = lock(&self.state);
        s.holds = (s.holds - 1).max(0);
        if s.holds == 0 {
            if let Some(k) = s.keepalive.take() {
                k.abort();
            }
        }
    }

    fn keep(self: &Arc<Self>) {
        let mut s = lock(&self.state);
        if s.holds > 0
            && s.conn.is_some()
            && s.keepalive.as_ref().is_none_or(JoinHandle::is_finished)
        {
            let br = Arc::clone(self);
            s.keepalive = Some(tokio::spawn(async move {
                loop {
                    {
                        let s = lock(&br.state);
                        if s.holds <= 0 || s.conn.is_none() {
                            break;
                        }
                    }
                    match br.call("ping", json!({}), 5.0, None).await {
                        Ok(_) => {}
                        Err(e) if e.is_driver_error() => {} // a dropped connection: the next hello restarts the loop
                        Err(_) => break, // Python's loop dies on anything else (a stale reply)
                    }
                    tokio::time::sleep(Duration::from_secs_f64(KEEPALIVE_S)).await;
                }
            }));
        }
    }

    /// `_reconcile`: the lost commands the hello's `seen` / `owned` lists settle.
    fn reconcile(&self, hello: &Hello) {
        let mut seen: HashMap<String, Value> = HashMap::new();
        for s in &hello.seen {
            if let Some(Value::String(id)) = s.get("id") {
                seen.insert(id.clone(), s.get("status").cloned().unwrap_or(Value::Null));
            }
        }
        let owned: BTreeSet<String> = hello
            .owned
            .iter()
            .filter_map(|o| o.get("id").and_then(Value::as_str).map(String::from))
            .collect();
        let name = self.name();
        let lost: Vec<String> = lock(&self.state).lost.iter().cloned().collect();
        for cid in lost {
            match self.ledger.state(&cid) {
                None => {
                    lock(&self.state).lost.remove(&cid);
                    continue;
                }
                Some(state) if state != "unknown" => {
                    lock(&self.state).lost.remove(&cid);
                    continue;
                }
                Some(_) => {}
            }
            let status = seen.get(&cid).and_then(Value::as_str);
            if status == Some("done") || owned.contains(&cid) {
                self.ledger.reconcile(
                    &cid,
                    "done",
                    vec![format!("{name} reported it done after reconnecting")],
                );
            } else if status == Some("failed") {
                self.ledger.reconcile(
                    &cid,
                    "failed",
                    vec![format!("{name} reported failure after reconnecting")],
                );
            } else {
                continue; // a bounded/reloaded journal cannot prove that a missing command never ran
            }
            lock(&self.state).lost.remove(&cid);
        }
    }

    fn deliver(&self, conn: &Conn, r: CmdResult) {
        let mut s = lock(&self.state);
        if let Some(p) = s.pending.get_mut(&r.id) {
            if r.gen == conn.gen() {
                if let Some(tx) = p.tx.take() {
                    let _ = tx.send(Delivery::Result(Box::new(r)));
                }
            }
        }
    }

    fn current(&self, conn: &Arc<Conn>) -> bool {
        lock(&self.state)
            .conn
            .as_ref()
            .is_some_and(|c| Arc::ptr_eq(c, conn))
    }

    /// The owner's tabs FERB knows here (chrome id -> host, active).
    pub fn tabs(&self) -> BTreeMap<i64, (String, bool)> {
        lock(&self.state).tabs.clone()
    }

    fn forget_tab(&self, chrome_id: i64) {
        lock(&self.state).tabs.remove(&chrome_id);
    }
}

/// A write to the native host failed. Python lets the `ConnectionError` through; here a read is a
/// known failure and an acting command (which may have reached the browser) is unknown.
fn write_failed(op: &str, name: &str, read: bool, e: &std::io::Error) -> Error {
    let message = format!("{op}: the connection to {name} failed ({e})");
    if read {
        Error::driver(message)
    } else {
        Error::unknown(message)
    }
}

/// Removes a call's pending entry however the call ends; a call dropped mid-flight (cancelled)
/// leaves an acting command lost, and a pointer gesture cancelled (`except CancelledError`).
struct CallGuard {
    bridge: Arc<Bridge>,
    cid: String,
    read: bool,
    op: String,
    args: Value,
    done: bool,
}

impl Drop for CallGuard {
    fn drop(&mut self) {
        if self.done {
            return;
        }
        let mut s = lock(&self.bridge.state);
        s.pending.remove(&self.cid);
        if !self.read {
            s.lost.insert(self.cid.clone());
        }
        drop(s);
        if self.op == "pointer" {
            if let Ok(rt) = tokio::runtime::Handle::try_current() {
                let br = Arc::clone(&self.bridge);
                let (args, cid) = (self.args.clone(), self.cid.clone());
                rt.spawn(async move { br.cancel_pointer(&args, &cid).await });
            }
        }
    }
}

struct HubState {
    bridges: Vec<((String, String), Arc<Bridge>)>,
    /// install_id -> why (HAND-406).
    refused: Vec<(String, String)>,
    /// install_id -> browser: installed, connected, not paired.
    unpaired: Vec<(String, String)>,
    /// install_id -> the version its hello said (APP-892).
    unpaired_version: Vec<(String, String)>,
    drivers: Vec<Arc<dyn TabDriver>>,
}

fn set<K: PartialEq, V>(v: &mut Vec<(K, V)>, k: K, val: V) {
    match v.iter_mut().find(|(x, _)| *x == k) {
        Some(e) => e.1 = val,
        None => v.push((k, val)),
    }
}

fn take<K: PartialEq, V>(v: &mut Vec<(K, V)>, k: &K) -> Option<V> {
    let at = v.iter().position(|(x, _)| x == k)?;
    Some(v.remove(at).1)
}

/// Why a connection ends where Python lets an exception end `Hub.serve`.
#[derive(Debug)]
struct End;

/// `Hub`: binds authenticated installs to profiles and runs every native-host connection.
pub struct Hub {
    pub pairings: Arc<Pairings>,
    ledger: Arc<dyn Ledger>,
    publish: Publish,
    device_id: Arc<dyn Fn() -> String + Send + Sync>,
    random: Arc<dyn Random>,
    apps: Arc<dyn BrowserApps>,
    app_of: AppOf,
    hooks: Mutex<Hooks>,
    state: Mutex<HubState>,
}

impl Hub {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        pairings: Arc<Pairings>,
        ledger: Arc<dyn Ledger>,
        publish: Publish,
        device_id: Arc<dyn Fn() -> String + Send + Sync>,
        random: Arc<dyn Random>,
        apps: Arc<dyn BrowserApps>,
        app_of: AppOf,
    ) -> Arc<Hub> {
        Arc::new(Hub {
            pairings,
            ledger,
            publish,
            device_id,
            random,
            apps,
            app_of,
            hooks: Mutex::new(Hooks::default()),
            state: Mutex::new(HubState {
                bridges: vec![],
                refused: vec![],
                unpaired: vec![],
                unpaired_version: vec![],
                drivers: vec![],
            }),
        })
    }

    pub fn set_hooks(&self, hooks: Hooks) {
        *lock(&self.hooks) = hooks;
    }

    pub fn hooks(&self) -> Hooks {
        lock(&self.hooks).clone()
    }

    /// A browser FERB pairs with, by key (`profiles.BROWSERS`).
    pub fn browser_info(&self, key: &str) -> Option<rules::BrowserInfo> {
        self.apps.browser(key)
    }

    /// The browser drivers register here: whose tab is it.
    pub fn add_driver(&self, driver: Arc<dyn TabDriver>) {
        lock(&self.state).drivers.push(driver);
    }

    pub(crate) fn drivers(&self) -> Vec<Arc<dyn TabDriver>> {
        lock(&self.state).drivers.clone()
    }

    pub fn publish(&self, topic: &str, data: Map<String, Value>) {
        (self.publish)(topic, data);
    }

    /// The bridges made so far, in the order they were made.
    pub fn bridges(&self) -> Vec<Arc<Bridge>> {
        lock(&self.state)
            .bridges
            .iter()
            .map(|(_, b)| Arc::clone(b))
            .collect()
    }

    pub fn existing(&self, browser: &str, folder: &str) -> Option<Arc<Bridge>> {
        let key = (browser.to_string(), folder.to_string());
        lock(&self.state)
            .bridges
            .iter()
            .find(|(k, _)| *k == key)
            .map(|(_, b)| Arc::clone(b))
    }

    /// install_id -> why, for this run (HAND-406).
    pub fn refused(&self, install_id: &str) -> Option<String> {
        lock(&self.state)
            .refused
            .iter()
            .find(|(k, _)| k == install_id)
            .map(|(_, v)| v.clone())
    }

    pub fn refusals(&self) -> Vec<(String, String)> {
        lock(&self.state).refused.clone()
    }

    /// install_id -> browser of the unpaired copies connected now.
    pub fn unpaired(&self) -> Vec<(String, String)> {
        lock(&self.state).unpaired.clone()
    }

    /// APP-892: each browser with an unpaired FERB copy connected now, and that copy's version (""
    /// when it said none, or when there are several there and they differ).
    pub fn unpaired_copies(&self) -> BTreeMap<String, String> {
        let s = lock(&self.state);
        let mut out: BTreeMap<String, String> = BTreeMap::new();
        for (install, browser) in &s.unpaired {
            let version = s
                .unpaired_version
                .iter()
                .find(|(k, _)| k == install)
                .map(|(_, v)| v.clone())
                .unwrap_or_default();
            let same = out.get(browser).is_none_or(|v| *v == version);
            out.insert(browser.clone(), if same { version } else { String::new() });
        }
        out
    }

    /// `note_refusal`: one refused connection or message (HAND-406). A paired install's refusal
    /// is also kept for this run (`refused`).
    fn note_refusal(
        &self,
        reason: &str,
        why: &str,
        install_id: Option<&str>,
        app: Option<&str>,
        facts: Map<String, Value>,
    ) {
        let binding = install_id
            .filter(|i| !i.is_empty())
            .and_then(|i| self.pairings.binding(i).ok().flatten());
        if let Some(b) = &binding {
            set(
                &mut lock(&self.state).refused,
                b.install_id.clone(),
                why.to_string(),
            );
        }
        let mut out = Map::new();
        out.insert("reason".into(), reason.into());
        out.insert("why".into(), why.into());
        out.extend(facts);
        if let Some(b) = &binding {
            out.insert(
                "profile".into(),
                format!("{}/{}", b.browser, b.folder).into(),
            );
        }
        if let Some(app) = app.filter(|a| !a.is_empty()) {
            out.insert("app".into(), app.into());
        }
        let record = Arc::clone(&lock(&self.hooks).record_refusal);
        record(out);
    }

    /// `problem`: why this paired profile's extension can't be used, as far as FERB knows: its
    /// refusal in this run, else the version it last reported when FERB needs a newer one.
    pub fn problem(&self, binding: &Binding) -> Option<String> {
        if let Some(why) = self.refused(&binding.install_id) {
            return Some(why);
        }
        let version = self
            .pairings
            .version(&binding.install_id)
            .unwrap_or_default();
        // A version Python's int() refuses ("²") raises there; here it counts as too old.
        (!version.is_empty()
            && !protocol::version_ok(&version, MIN_EXTENSION_VERSION).unwrap_or(false))
        .then(|| rules::too_old(&version))
    }

    /// The live task that drives this tab now, in any driver.
    pub fn task_tab(&self, br: &Arc<Bridge>, chrome_id: i64) -> Option<TaskTab> {
        self.drivers()
            .iter()
            .find_map(|d| d.task_tab(br, chrome_id))
    }

    /// A running task still holds this tab.
    pub fn holds(&self, br: &Arc<Bridge>, chrome_id: i64) -> bool {
        self.drivers().iter().any(|d| d.holds(br, chrome_id))
    }

    pub fn tab_removed(&self, br: &Arc<Bridge>, chrome_id: i64, by_page: bool) {
        for d in self.drivers() {
            d.tab_closed(br, chrome_id, by_page);
        }
    }

    fn spawn(&self, job: BoxFuture<'static, ()>) {
        tokio::spawn(job);
    }

    fn spawn_call(&self, br: &Arc<Bridge>, op: &str, args: Value) {
        let job = br.call(op, args, 10.0, None);
        self.spawn(Box::pin(async move {
            let _ = job.await; // best effort: a dropped connection ends it anyway
        }));
    }

    /// After a reconnect or a restart: let go of every tab the extension still holds downloads for
    /// that isn't a live task tab here. Never closes a tab.
    fn disarm_unheld(&self, br: &Arc<Bridge>, armed: &[i64]) {
        for tab in armed {
            if self.task_tab(br, *tab).is_none() && !self.holds(br, *tab) {
                self.spawn_call(br, "disarm", json!({"tab_id": tab}));
            }
        }
    }

    /// `_open`: a task tab's link that opens a new tab.
    async fn open(self: Arc<Self>, br: Arc<Bridge>, ev: Event) -> Result<()> {
        if let Some(open_id) = ev.open_id.as_ref().filter(|o| !o.is_empty()) {
            let seen = br
                .call("open_seen", json!({"open_id": open_id}), 10.0, None)
                .await?;
            if seen.get("seen") != Some(&Value::Bool(true)) {
                return Ok(());
            }
        }
        let url = ev.url.clone().unwrap_or_default();
        for d in self.drivers() {
            if d.task_tab(&br, ev.tab_id).is_some() {
                d.open_from(Arc::clone(&br), ev.tab_id, url, ev.window_id)
                    .await;
                return Ok(());
            }
        }
        // a loop's held tab between rounds: the owner's link opens as theirs, and the loop keeps holding the tab
        let disarm = !self.holds(&br, ev.tab_id);
        pass_through(&br, ev.tab_id, &url, ev.background != Some(true), disarm).await
    }

    /// `_on_popup`.
    fn on_popup(&self, br: &Arc<Bridge>, ev: &Event) {
        for d in self.drivers() {
            let Some(opener) = ev.opener_tab_id else {
                continue;
            };
            if d.task_tab(br, opener).is_none() {
                continue;
            }
            match d.adopt(br, opener, ev.tab_id, ev.url.as_deref().unwrap_or("")) {
                Some(Adopted::Refused(_)) => {
                    self.spawn_call(br, "popup_refused", json!({"tab_id": ev.tab_id}))
                }
                Some(Adopted::Record(record)) => {
                    let (br, tab) = (Arc::clone(br), ev.tab_id);
                    self.spawn(Box::pin(async move {
                        // recorded (and grouped) first: a reconnect re-adopts it
                        let _ = br
                            .call("adopt_tab", Value::Object(record), 10.0, None)
                            .await;
                        let _ = br
                            .call("popup_ready", json!({"tab_id": tab}), 10.0, None)
                            .await;
                    }));
                }
                None => self.spawn_call(br, "disarm", json!({"tab_id": ev.tab_id})),
            }
            return;
        }
        self.spawn_call(br, "disarm", json!({"tab_id": ev.tab_id}));
    }

    /// On pause: drop what's known about tabs FERB doesn't own.
    pub fn forget_owner_tabs(&self) {
        let owned = Arc::clone(&lock(&self.hooks).owned);
        for br in self.bridges() {
            for tab in br.tabs().keys() {
                if !owned(&br, *tab) {
                    br.forget_tab(*tab);
                }
            }
        }
    }

    /// `bridge(browser, folder)`: the paired profile's bridge (a new one when its binding changed).
    pub fn bridge(&self, browser: &str, folder: &str) -> Result<Option<Arc<Bridge>>> {
        let b = self
            .pairings
            .for_profile(browser, folder)
            .map_err(|e| Error::driver(e.to_string()))?;
        let Some(b) = b else { return Ok(None) };
        let key = (browser.to_string(), folder.to_string());
        let mut s = lock(&self.state);
        if let Some((_, br)) = s.bridges.iter().find(|(k, _)| *k == key) {
            if br.binding == b {
                return Ok(Some(Arc::clone(br)));
            }
        }
        let br = Arc::new(Bridge::new(
            b,
            Arc::clone(&self.ledger),
            Arc::clone(&self.random),
            (self.device_id)(),
        ));
        set(&mut s.bridges, key, Arc::clone(&br));
        Ok(Some(br))
    }

    /// `serve`: one native-host connection, after its RPC hello: newline-delimited extension
    /// messages both ways. `browser_pid` is the native host's parent (the platform reads it).
    pub async fn serve<R, W>(self: Arc<Self>, reader: R, writer: W, browser_pid: Option<i32>)
    where
        R: AsyncBufRead + Unpin + Send,
        W: AsyncWrite + Unpin + Send + 'static,
    {
        // Python reads it in a worker thread; it is one libproc call and one small plist read, so it is read here
        let app = (self.app_of)(browser_pid);
        let conn = Arc::new(Conn::new(Box::new(writer), browser_pid, app));
        if let Some(why) = rules::peer_refusal(self.apps.as_ref(), conn.app.as_deref(), None) {
            let mut data = Map::new();
            data.insert("why".into(), why.clone().into());
            self.publish("browser.connection_refused", data);
            self.note_refusal("peer", &why, None, conn.app.as_deref(), Map::new());
            let _ = conn
                .write_line(&dumps(&json!({"type": "refused", "reason": why})))
                .await;
            return;
        }
        let mut reader = reader;
        while let Some(line) = read_line(&mut reader).await {
            let raw: Option<Value> = serde_json::from_slice(&line).ok();
            match raw
                .as_ref()
                .filter(|v| v.is_object())
                .and_then(protocol::parse)
            {
                Some(m) => {
                    if Arc::clone(&self).handle(&conn, m).await.is_err() {
                        break;
                    }
                }
                None => {
                    // dropped, nothing acts; recorded (never its contents)
                    let install = lock(&conn.state).install_id.clone();
                    self.note_refusal(
                        "malformed",
                        "a message from the FERB extension wasn't in FERB's format, so FERB dropped it",
                        install.as_deref(),
                        conn.app.as_deref(),
                        Map::new(),
                    );
                }
            }
        }
        let (unpaired, bridge) = {
            let s = lock(&conn.state);
            (s.unpaired_install.clone(), s.bridge.clone())
        };
        if let Some(install) = unpaired {
            let mut s = lock(&self.state);
            take(&mut s.unpaired, &install);
            take(&mut s.unpaired_version, &install);
        }
        if let Some(br) = bridge {
            // fixed against Python: only the profile's current connection ending disconnects it; Python also
            // published `browser.disconnected` when an older connection ended under a newer one
            let current = br.current(&conn);
            br.drop_conn(&conn);
            if current {
                let mut data = Map::new();
                data.insert("profile".into(), br.name().into());
                self.publish("browser.disconnected", data);
            }
        }
    }

    async fn write(conn: &Conn, v: &Value) -> std::result::Result<(), End> {
        conn.write_line(&dumps(v)).await.map_err(|_| End)
    }

    /// `_handle`: one well-formed message.
    async fn handle(
        self: Arc<Self>,
        conn: &Arc<Conn>,
        m: ExtMessage,
    ) -> std::result::Result<(), End> {
        let v = self.pairings.handle(&m).map_err(|_| End)?;
        let app = conn.app.as_deref();
        if !v.accepted {
            if m.mtype == ExtType::Hello && (v.reason == "bad_mac" || v.reason == "replay") {
                let why = format!(
                    "extension pairing failed ({}); pair this profile again (browser_pair, or Settings → Browsers)",
                    v.reason
                );
                self.note_refusal(&v.reason, &why, Some(&m.install_id), app, Map::new());
                Self::write(conn, &json!({"type": "refused", "reason": why})).await?;
            } else if matches!(m.mtype, ExtType::Hello | ExtType::Pair) {
                // a pairing that didn't take (unknown or expired token)
                let why = format!(
                    "the FERB extension's {} was refused ({})",
                    m.mtype.as_str(),
                    v.reason
                );
                self.note_refusal(&v.reason, &why, Some(&m.install_id), app, Map::new());
            }
            return Ok(()); // unpaired / bad MAC / replay: dropped, nothing acts
        }
        if let Some(reply) = &v.reply {
            // just paired: hand the key back on this connection
            {
                let mut s = lock(&self.state);
                take(&mut s.unpaired, &m.install_id);
                take(&mut s.unpaired_version, &m.install_id);
            }
            conn.write_line(&reply.wire()).await.map_err(|_| End)?;
            if let Some(b) = &v.binding {
                let mut data = Map::new();
                data.insert("browser".into(), b.browser.clone().into());
                data.insert("folder".into(), b.folder.clone().into());
                data.insert("identity".into(), b.identity.as_str().into());
                self.publish("browser.profile_paired", data);
            }
            return Ok(());
        }
        let Some(binding) = v.binding.clone() else {
            // unpaired hello: nothing acts; FERB can tell the owner
            if m.mtype == ExtType::Hello {
                let said = m.body().unwrap_or_default();
                let b = rules::text_or_empty(said.get("browser"));
                if b == "chrome" || b == "brave" {
                    let version =
                        ferb_py::value::prefix(&rules::text_or_empty(said.get("version")), 32)
                            .to_string();
                    {
                        let mut s = lock(&self.state);
                        set(&mut s.unpaired, m.install_id.clone(), b);
                        set(&mut s.unpaired_version, m.install_id.clone(), version);
                    }
                    lock(&conn.state).unpaired_install = Some(m.install_id.clone());
                    let hook = Arc::clone(&lock(&self.hooks).on_unpaired);
                    hook();
                }
                Self::write(conn, &json!({"type": "pairing_required"})).await?;
            }
            return Ok(());
        };
        lock(&conn.state).install_id = Some(m.install_id.clone());
        let Ok(body) = m.body() else { return Ok(()) };
        if m.mtype == ExtType::Hello {
            return self.hello(conn, &m, &binding, &body).await;
        }
        let Some(br) = conn.bridge() else {
            return Ok(());
        };
        if !br.current(conn) {
            return Ok(()); // results from an old connection are ignored
        }
        match m.mtype {
            ExtType::Result => {
                if let Ok(r) = CmdResult::validate(&body) {
                    br.deliver(conn, r);
                }
            }
            ExtType::Event => {
                if let Ok(ev) = Event::validate(&body) {
                    self.event(&br, ev);
                }
            }
            ExtType::TabEvent => {
                if let Ok(e) = TabEvent::validate(&body) {
                    self.tab_event(&br, e)?;
                }
            }
            _ => {}
        }
        Ok(())
    }

    async fn hello(
        self: Arc<Self>,
        conn: &Arc<Conn>,
        m: &ExtMessage,
        binding: &Binding,
        body: &Map<String, Value>,
    ) -> std::result::Result<(), End> {
        let app = conn.app.as_deref();
        let Ok(hello) = Hello::validate(body) else {
            return Ok(());
        };
        // authenticated: kept, accepted or not (HAND-406)
        self.pairings
            .record_version(&m.install_id, &hello.version)
            .map_err(|_| End)?;
        // `or` as Python's: the version is read only when the protocol is FERB's. Fixed against Python: a
        // version Python's int() refuses ("0.19.²") is refused as too old; Python's ValueError ended the connection
        if hello.protocol != PROTOCOL_VERSION
            || !protocol::version_ok(&hello.version, MIN_EXTENSION_VERSION).unwrap_or(false)
        {
            let (reason, why) = rules::version_refusal(&hello.version, hello.protocol);
            let mut facts = Map::new();
            facts.insert(
                "version".into(),
                ferb_py::value::prefix(&hello.version, 32).into(),
            );
            facts.insert("protocol".into(), hello.protocol.into());
            self.note_refusal(reason, &why, Some(&m.install_id), app, facts);
            return Self::write(
                conn,
                &json!({"type": "refused", "reason": why, "required_protocol": PROTOCOL_VERSION,
                        "minimum_version": MIN_EXTENSION_VERSION}),
            )
            .await;
        }
        let key = self.pairings.key(&m.install_id).map_err(|_| End)?;
        let (reason, refusal) = if hello.nonce.is_empty() || key.is_none() {
            (
                "no_nonce",
                Some(
                    "the FERB extension sent no signing nonce: reload the FERB extension"
                        .to_string(),
                ),
            )
        } else {
            (
                "peer",
                rules::peer_refusal(self.apps.as_ref(), app, Some(&binding.browser)),
            )
        };
        if let Some(why) = refusal {
            self.note_refusal(reason, &why, Some(&m.install_id), app, Map::new());
            return Self::write(conn, &json!({"type": "refused", "reason": why})).await;
        }
        take(&mut lock(&self.state).refused, &m.install_id);
        let Some(br) = self
            .bridge(&binding.browser, &binding.folder)
            .map_err(|_| End)?
        else {
            return Ok(());
        };
        {
            let mut c = lock(&conn.state);
            c.nonce = hello.nonce.clone();
            c.key = key;
            c.dseq = 0;
        }
        br.attach(conn, &hello);
        // identity: in FERB's own profile every download is FERB's (D35)
        let welcome = Bridge::envelope(
            conn,
            "welcome",
            &json!({"gen": br.gen(), "identity": br.binding.identity.as_str()}),
        );
        conn.write_line(&welcome).await.map_err(|_| End)?;
        let mut data = Map::new();
        data.insert("profile".into(), br.name().into());
        data.insert("version".into(), hello.version.clone().into());
        data.insert("gen".into(), br.gen().into());
        self.publish("browser.connected", data);
        for d in self.drivers() {
            if d.identity() == br.binding.identity {
                for settle in d.readopt(&br, &hello.tabs) {
                    self.spawn(settle);
                }
            }
        }
        self.disarm_unheld(&br, &hello.armed);
        Ok(())
    }

    fn event(self: &Arc<Self>, br: &Arc<Bridge>, ev: Event) {
        let name = br.name();
        match ev.event.as_str() {
            "detached" => {
                let reason = ev
                    .reason
                    .clone()
                    .filter(|r| !r.is_empty())
                    .unwrap_or_else(|| "unknown".into());
                lock(&br.state).detached.insert(ev.tab_id, reason.clone());
                let mut data = Map::new();
                data.insert("profile".into(), name.into());
                data.insert("tab_id".into(), ev.tab_id.into());
                data.insert("reason".into(), reason.into());
                data.insert(
                    "by_owner".into(),
                    (ev.reason.as_deref() == Some("canceled_by_user")).into(),
                );
                self.publish("browser.debugger_detached", data);
            }
            "file_chooser" => {
                let mut data = Map::new();
                data.insert("profile".into(), name.into());
                data.extend(ev.dump_set());
                self.publish("browser.file_chooser", data);
            }
            "download" => {
                let hook = lock(&self.hooks).on_download.clone();
                match hook {
                    Some(h) => h(br, &ev), // its parts come back on this connection: never awaited
                    None => let_download_go(br, &ev),
                }
                if ev.blank_tab == Some(true)
                    && ev.download_id.as_ref().is_some_and(|d| !d.is_empty())
                {
                    for d in self.drivers() {
                        if d.task_tab(br, ev.tab_id).is_some() {
                            self.spawn(d.download_tab_done(Arc::clone(br), ev.tab_id));
                        }
                    }
                }
            }
            "open" => {
                let hub = Arc::clone(self);
                let br = Arc::clone(br);
                self.spawn(Box::pin(async move {
                    let _ = hub.open(br, ev).await;
                }));
            }
            "popup" => self.on_popup(br, &ev),
            "taken" => {
                // the owner moved a FERB tab out of its group: theirs now
                for d in self.drivers() {
                    d.tab_taken(br, ev.tab_id, ev.reason.as_deref().unwrap_or(""));
                }
            }
            _ => {
                let mut data = Map::new();
                data.insert("profile".into(), name.into());
                data.insert("tab_id".into(), ev.tab_id.into());
                data.insert(
                    "type".into(),
                    ev.r#type.clone().map_or(Value::Null, Value::from),
                );
                data.insert(
                    "message".into(),
                    ev.message.clone().unwrap_or_default().into(),
                );
                data.insert(
                    "handled".into(),
                    ev.handled.clone().map_or(Value::Null, Value::from),
                );
                self.publish("browser.dialog", data);
            }
        }
    }

    fn tab_event(&self, br: &Arc<Bridge>, e: TabEvent) -> std::result::Result<(), End> {
        let hooks = self.hooks();
        if e.event == "removed" {
            self.tab_removed(br, e.tab_id, e.closed_by_page == Some(true));
            if let Some(job) = (hooks.on_tab_closed)(br, e.tab_id) {
                self.spawn(job);
            }
        }
        if (hooks.paused)() && !(hooks.owned)(br, e.tab_id) {
            return Ok(()); // paused: the owner's tabs aren't watched at all
        }
        if e.event == "sound" {
            (hooks.on_sound)(br, &e);
            return Ok(());
        }
        (hooks.on_tab)(br, &e);
        let mut s = lock(&br.state);
        if e.event == "removed" {
            s.tabs.remove(&e.tab_id);
        } else {
            let host = match e.url.as_deref().filter(|u| !u.is_empty()) {
                Some(url) => crate::url::hostname(url)
                    .map_err(|_| End)?
                    .unwrap_or_default(),
                None => s
                    .tabs
                    .get(&e.tab_id)
                    .map(|t| t.0.clone())
                    .unwrap_or_default(),
            };
            s.tabs.insert(e.tab_id, (host, e.active == Some(true)));
            if e.event == "updated" && e.same_document != Some(true) {
                // HAND-492: a route change keeps ids
                *s.navs.entry(e.tab_id).or_insert(0) += 1;
            }
        }
        Ok(())
    }
}

/// One line as `StreamReader.readline` gives it under the daemon's limit: `None` at the end, or
/// when a line is longer than `MAX_MESSAGE` (Python's `ValueError` ends the connection). A last
/// line without a newline still counts.
async fn read_line<R: AsyncBufRead + Unpin>(reader: &mut R) -> Option<Vec<u8>> {
    let mut line = Vec::new();
    loop {
        let buf = reader.fill_buf().await.ok()?;
        if buf.is_empty() {
            return (!line.is_empty()).then_some(line);
        }
        match buf.iter().position(|b| *b == b'\n') {
            Some(at) => {
                line.extend_from_slice(&buf[..at]);
                reader.consume(at + 1);
                return (line.len() <= MAX_MESSAGE).then_some(line);
            }
            None => {
                let n = buf.len();
                line.extend_from_slice(buf);
                reader.consume(n);
                if line.len() > MAX_MESSAGE {
                    return None;
                }
            }
        }
    }
}

/// `_pass_through`: the owner's own new-tab click caught in a tab no task drives now: opened as
/// they asked, the tab disarmed unless a task still holds it.
async fn pass_through(
    br: &Arc<Bridge>,
    tab_id: i64,
    url: &str,
    active: bool,
    disarm: bool,
) -> Result<()> {
    let opened = br
        .call(
            "open_plain",
            json!({"tab_id": tab_id, "url": url, "active": active}),
            10.0,
            None,
        )
        .await;
    if disarm {
        br.call("disarm", json!({"tab_id": tab_id}), 10.0, None)
            .await?;
    }
    opened.map(|_| ())
}

/// `_let_download_go`: no download handler: the held response goes back to the browser untouched.
pub fn let_download_go(br: &Arc<Bridge>, ev: &Event) {
    if ev.uncaptured == Some(true) {
        return;
    }
    let Some(id) = ev.download_id.clone().filter(|d| !d.is_empty()) else {
        return;
    };
    let job = br.call(
        "download_part",
        json!({"download_id": id, "release": true}),
        10.0,
        None,
    );
    tokio::spawn(async move {
        let _ = job.await;
    });
}

/// The daemon's socket hands a native host's connection over after its hello (ferb-rpc's
/// `NativeChannel`). `browser_pid` reads the native host's parent from the socket (`_browser_pid`:
/// LOCAL_PEERPID, then libproc's parent pid): the platform's part.
#[cfg(unix)]
pub mod native {
    use std::os::fd::{AsFd, BorrowedFd};
    use std::sync::Arc;

    use ferb_core::interfaces::BoxFuture;
    use ferb_rpc::{NativeChannel, NativeConnection};
    use tokio::io::{AsyncReadExt, BufReader};

    use super::Hub;

    pub type BrowserPid = Arc<dyn Fn(BorrowedFd<'_>) -> Option<i32> + Send + Sync>;

    pub struct Channel {
        pub hub: Arc<Hub>,
        pub browser_pid: BrowserPid,
    }

    impl NativeChannel for Channel {
        fn run(&self, connection: NativeConnection) -> BoxFuture<'static, ()> {
            let pid = (self.browser_pid)(connection.reader.as_ref().as_fd());
            let reader =
                BufReader::new(std::io::Cursor::new(connection.buffered).chain(connection.reader));
            Box::pin(Arc::clone(&self.hub).serve(reader, connection.writer, pid))
        }
    }
}
