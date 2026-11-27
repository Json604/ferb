//! One Codex run on the pinned app-server's stdio JSON-RPC (codex_stream.py): `initialize`, `thread/start` or
//! `thread/resume`, `turn/start`, `turn/steer`; the stdout reader that multiplexes acknowledgements and
//! notifications, the stderr reader that checks the request trace's tool catalog, the item gate, and the clean-up
//! that kills and reaps the app-server, publishes a refreshed sign-in and removes the run's folder.

use std::collections::{BTreeSet, HashMap, VecDeque};
use std::fs;
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ferb_core::errors::FerbError;
use ferb_core::timing::{CallTimer, Steps};
use ferb_core::types::{ModelRequest, RunHandle, RuntimeEventKind as K, Task};
use serde_json::{json, Map, Value};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::JoinHandle;
use tokio::time::Instant;

use super::*;
use crate::error::{Raised, RuntimeError};
use crate::gen::{Closed, EventStream, Yielder};
use crate::process::{ProcControl, Spawn};
use crate::pyjson::{get, get_or_empty};
use crate::run::{event, RunControl, Started};

/// RUN-480: how long a plain-mode cancel waits for Codex to end the interrupted turn before the app-server is
/// killed (a clean-up wait, not a limit on the run).
pub const INTERRUPT_WAIT_S: f64 = 2.0;

/// The reader's limit for one line (Python's `limit=64 * 1024 * 1024`).
pub const LINE_LIMIT: usize = 64 * 1024 * 1024;

type Waiter = oneshot::Sender<Result<Value, RuntimeError>>;

struct Auth {
    source: PathBuf,
    copy: PathBuf,
    original: Vec<u8>,
}

struct State {
    closed: bool,
    cancelled: bool,
    turn_completed: bool,
    request_id: i64,
    waiters: HashMap<i64, Waiter>,
    seen: BTreeSet<String>,
    violation: Option<BTreeSet<String>>,
    violation_item: Option<Value>,
    catalog_observed: bool,
    catalog_unreadable: bool,
    thread_id: Option<Value>,
    turn_id: Option<Value>,
    timer: Option<CallTimer>,
    timings: VecDeque<Map<String, Value>>,
    /// RUN-477: each model call's usage report as the reader saw it, until the run's events carry it.
    usage_seen: VecDeque<Map<String, Value>>,
    context_tokens: Value,
    context_window: Value,
    watcher_crash: Option<Raised>,
    /// RUN-480: a plain-mode cancel is ending the turn (interrupt first); nothing after it is an error or an event.
    ending: bool,
    /// T-1343: the request-trace lines the stderr reader checked over the program's life (each holds a request's
    /// tool catalog), the count when this turn began, and this turn's model calls (`thread/tokenUsage/updated`).
    trace_lines: u64,
    turn_trace_start: u64,
    turn_calls: u64,
}

/// What belongs to one message in a program: today a run is one message, a kept program takes several (T-1343).
pub(super) struct TurnInfo {
    pub(super) rt: Arc<CodexRuntime>,
    pub(super) task_id: String,
    pub(super) run_id: String,
    pub(super) deadline: Option<Instant>,
}

/// A kept program's link to its keeper (T-1343).
pub(super) struct KeptLink {
    pub(super) settings: String,
    pub(super) back: crate::keep::Back,
}

/// One live Codex run: everything its readers, its consumer and its control share.
pub struct CodexRun {
    turn: Mutex<TurnInfo>,
    st: Mutex<State>,
    auth: Mutex<Option<Auth>>,
    proc: Mutex<Option<Arc<dyn ProcControl>>>,
    stdin: tokio::sync::Mutex<Option<Box<dyn AsyncWrite + Send + Unpin>>>,
    notes_tx: mpsc::UnboundedSender<Result<Value, RuntimeError>>,
    notes_rx: tokio::sync::Mutex<mpsc::UnboundedReceiver<Result<Value, RuntimeError>>>,
    checked: watch::Sender<bool>,
    reader: Mutex<Option<JoinHandle<()>>>,
    watcher: Mutex<Option<JoinHandle<()>>>,
    temporary: Mutex<Option<PathBuf>>,
    lock: Mutex<Option<fs::File>>,
    token_file: Mutex<Option<PathBuf>>,
    cleanup_done: tokio::sync::Mutex<bool>,
    context_role: Mutex<String>,
    /// Told when the turn completed or the transport closed (a plain-mode cancel waits for its interrupt).
    turn_end: tokio::sync::Notify,
    /// T-1343: a kept program (several messages, one at a time); None for today's one-message run.
    kept: Option<KeptLink>,
    /// T-1343: the message the program is on (a turn's control acts only while it is the current one).
    generation: std::sync::atomic::AtomicU64,
    /// Told when the stderr reader checked another request-trace line.
    trace_seen: tokio::sync::Notify,
}

pub(super) fn timer_now(rt: &CodexRuntime) -> ferb_core::timing::Now {
    let m = rt.monotonic.clone();
    Box::new(move || m())
}

fn eq(a: Option<&Value>, b: Option<&Value>) -> bool {
    ferb_py::value::py_eq(a.unwrap_or(&Value::Null), b.unwrap_or(&Value::Null))
}

/// A waiter key for a response's id, as Python's dict lookup would match it; `Err` for an unhashable id.
fn waiter_key(id: &Value) -> Result<Option<i64>, ()> {
    match id {
        Value::Bool(b) => Ok(Some(i64::from(*b))),
        Value::Number(n) => Ok(n.as_i64().or_else(|| {
            n.as_f64()
                .filter(|f| f.fract() == 0.0 && f.abs() < 9.0e18)
                .map(|f| f as i64)
        })),
        Value::Array(_) | Value::Object(_) => Err(()),
        _ => Ok(None),
    }
}

/// A hashable key for a JSON value as a Python dict key (`1`, `1.0` and `True` are one key).
fn hash_key(v: &Value) -> Result<String, Raised> {
    match v {
        Value::Null => Ok("None".into()),
        Value::Bool(b) => Ok(format!("n:{}", i64::from(*b))),
        Value::Number(n) => Ok(match n.as_f64() {
            Some(f) if f.fract() == 0.0 && n.is_f64() => format!("n:{}", f as i64),
            _ => format!("n:{n}"),
        }),
        Value::String(s) => Ok(format!("s:{s}")),
        other => Err(Raised::new(
            "TypeError",
            format!("unhashable type: '{}'", crate::error::py_type(other)),
        )),
    }
}

/// How one events loop ended.
enum Exit {
    Closed,
    Fail(RuntimeError),
}

impl From<Closed> for Exit {
    fn from(_: Closed) -> Self {
        Exit::Closed
    }
}

impl From<RuntimeError> for Exit {
    fn from(e: RuntimeError) -> Self {
        Exit::Fail(e)
    }
}

impl From<Raised> for Exit {
    fn from(e: Raised) -> Self {
        Exit::Fail(RuntimeError::Raised(e))
    }
}

impl CodexRun {
    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.st.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn turn_info(&self) -> std::sync::MutexGuard<'_, TurnInfo> {
        self.turn.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The settings of the message the program is on.
    pub(super) fn rt(&self) -> Arc<CodexRuntime> {
        self.turn_info().rt.clone()
    }

    fn task_id(&self) -> String {
        self.turn_info().task_id.clone()
    }

    fn deadline(&self) -> Option<Instant> {
        self.turn_info().deadline
    }

    fn is_checked(&self) -> bool {
        *self.checked.borrow()
    }

    fn set_checked(&self) {
        self.checked.send_replace(true);
    }

    async fn wait_checked(&self) {
        let mut rx = self.checked.subscribe();
        loop {
            if *rx.borrow_and_update() {
                return;
            }
            if rx.changed().await.is_err() {
                return;
            }
        }
    }

    /// codex_stream.py `_close_transport`.
    fn close_transport(&self, error: RuntimeError) {
        let waiters: Vec<Waiter> = {
            let mut st = self.lock();
            if st.closed {
                return;
            }
            st.closed = true;
            st.waiters.drain().map(|(_, w)| w).collect()
        };
        for w in waiters {
            let _ = w.send(Err(error.clone()));
        }
        let _ = self.notes_tx.send(Err(error));
        self.turn_end.notify_waiters();
    }

    /// codex_stream.py `_write`.
    async fn write(&self, message: &Value) -> Result<(), RuntimeError> {
        let mut stdin = self.stdin.lock().await;
        let Some(w) = stdin.as_mut() else {
            return Err(RuntimeError::protocol("Codex runtime input is unavailable"));
        };
        let line = format!("{}\n", ferb_py::json::dumps(message));
        w.write_all(line.as_bytes()).await?;
        w.flush().await?;
        Ok(())
    }

    /// codex_stream.py `_rpc`.
    async fn rpc(&self, method: &str, params: Value) -> Result<Map<String, Value>, RuntimeError> {
        let (ident, rx) = {
            let mut st = self.lock();
            if st.closed || st.cancelled {
                return Err(RuntimeError::protocol("Codex run is closed"));
            }
            st.request_id += 1;
            let ident = st.request_id;
            let (tx, rx) = oneshot::channel();
            st.waiters.insert(ident, tx);
            (ident, rx)
        };
        let out = async {
            self.write(&json!({"id": ident, "method": method, "params": params}))
                .await?;
            let obj = match rx.await {
                Ok(r) => r?,
                Err(_) => return Err(RuntimeError::protocol("Codex run closed")),
            };
            match obj.get("result") {
                Some(Value::Object(result)) if obj.get("error").is_none() => Ok(result.clone()),
                _ => Err(RuntimeError::protocol(codex_error(
                    &format!("Codex could not complete {method}"),
                    obj.get("error"),
                    &self.rt().redact,
                ))),
            }
        }
        .await;
        self.lock().waiters.remove(&ident);
        out
    }

    /// codex_stream.py `_save_refresh`: a refreshed login goes back only while the source is still the one copied.
    pub(super) fn save_refresh(&self) {
        let mut guard = self.auth.lock().unwrap_or_else(|e| e.into_inner());
        let Some(auth) = guard.as_mut() else { return };
        let Ok(refreshed) = fs::read(&auth.copy) else {
            return;
        };
        if refreshed == auth.original {
            return;
        }
        let parsed = std::str::from_utf8(&refreshed)
            .ok()
            .and_then(|t| ferb_py::json::loads(t).ok());
        if !matches!(parsed, Some(Value::Object(_))) {
            return;
        }
        if fs::read(&auth.source).ok().as_deref() != Some(auth.original.as_slice()) {
            return; // another run or login owns the newer credentials
        }
        let Some(parent) = auth.source.parent() else {
            return;
        };
        let Ok(mut tmp) = tempfile::Builder::new()
            .prefix(".ferb-auth-refresh-")
            .tempfile_in(parent)
        else {
            return;
        };
        use std::io::Write;
        if tmp.write_all(&refreshed).is_err() || tmp.flush().is_err() {
            return;
        }
        if fs::read(&auth.source).ok().as_deref() == Some(auth.original.as_slice())
            && tmp.persist(&auth.source).is_ok()
        {
            auth.original = refreshed;
        }
    }

    fn proc(&self) -> Option<Arc<dyn ProcControl>> {
        self.proc.lock().ok().and_then(|p| p.clone())
    }

    /// RUN-480: a plain-mode run's turn is ended in Codex first (`turn/interrupt`, measured on 0.166.0: the turn
    /// ends in about 5 ms with its usage reported), waiting at most [`INTERRUPT_WAIT_S`] for the turn to complete,
    /// so the usage Codex counted reaches the reader before the app-server is killed.
    pub(super) async fn interrupt(&self) {
        let (thread_id, turn_id) = {
            let mut st = self.lock();
            if st.closed || st.cancelled || st.turn_completed || st.turn_id.is_none() {
                return;
            }
            st.ending = true;
            (st.thread_id.clone(), st.turn_id.clone())
        };
        let ended = self.turn_end.notified();
        tokio::pin!(ended);
        ended.as_mut().enable();
        let wait = async {
            let params = json!({"threadId": thread_id, "turnId": turn_id});
            let _ = self.rpc("turn/interrupt", params).await;
            let done = {
                let st = self.lock();
                st.turn_completed || st.closed
            };
            if !done {
                ended.await;
            }
        };
        let _ = tokio::time::timeout(Duration::from_secs_f64(INTERRUPT_WAIT_S), wait).await;
    }

    /// codex_stream.py `cancel`: closed, the reader stopped, the app-server killed and reaped, the stderr reader
    /// stopped, a refreshed login published, the run's folder removed and its lock released. A plain-mode run's turn
    /// is interrupted first (RUN-480).
    pub async fn cancel(&self) {
        if self.rt().plain() {
            self.interrupt().await;
        }
        self.end_program().await;
    }

    /// [`CodexRun::cancel`] without the interrupt: the program ends with today's code.
    pub(super) async fn end_program(&self) {
        self.lock().cancelled = true;
        self.close_transport(RuntimeError::protocol("Codex run closed"));
        let mut done = self.cleanup_done.lock().await;
        if let Some(reader) = self.reader.lock().ok().and_then(|mut r| r.take()) {
            reader.abort();
            let _ = reader.await;
        }
        if let Some(proc) = self.proc() {
            if proc.returncode().is_none() {
                proc.kill();
                proc.wait().await;
            }
        }
        if let Some(watcher) = self.watcher.lock().ok().and_then(|mut w| w.take()) {
            watcher.abort();
            let _ = watcher.await;
        }
        if *done {
            return;
        }
        if let Some(root) = self.temporary.lock().ok().and_then(|mut t| t.take()) {
            self.save_refresh();
            let _ = fs::remove_dir_all(&root);
        }
        if let Some(lock) = self.lock.lock().ok().and_then(|mut l| l.take()) {
            drop(lock); // the folder is gone; the lock goes with it
        }
        if let Ok(mut t) = self.token_file.lock() {
            t.take();
        }
        *done = true;
    }

    /// codex_stream.py `steer` (worker mode only: `CodexRuntime.continue_`).
    pub async fn steer(&self, message: &str) -> Result<(), RuntimeError> {
        if self.rt().mode != Mode::Worker {
            return Err(RuntimeError::unsupported(
                "Only active Codex worker turns accept steering",
            ));
        }
        let (thread_id, turn_id) = {
            let st = self.lock();
            if st.closed || st.cancelled || st.turn_completed {
                return Err(RuntimeError::unsupported(
                    "Codex worker no longer has an active turn",
                ));
            }
            (st.thread_id.clone(), st.turn_id.clone())
        };
        let params = json!({"threadId": thread_id, "expectedTurnId": turn_id,
            "input": [{"type": "text", "text": message}]});
        let out = match self.rpc("turn/steer", params).await {
            Ok(result) => {
                let st = self.lock();
                if !eq(result.get("turnId"), st.turn_id.as_ref()) || st.turn_completed {
                    Err(RuntimeError::protocol(
                        "Codex worker completed before steering was acknowledged",
                    ))
                } else {
                    Ok(())
                }
            }
            Err(e) => Err(e),
        };
        match out {
            Ok(()) => Ok(()),
            Err(RuntimeError::Protocol { .. } | RuntimeError::Account { .. }) => Err(
                RuntimeError::unsupported("Codex worker did not acknowledge the correction"),
            ),
            Err(RuntimeError::Raised(r)) if is_os_error(&r) => Err(RuntimeError::unsupported(
                "Codex worker did not acknowledge the correction",
            )),
            Err(other) => Err(other),
        }
    }
}

fn is_os_error(r: &Raised) -> bool {
    matches!(
        r.class.as_str(),
        "OSError"
            | "BrokenPipeError"
            | "ConnectionResetError"
            | "FileNotFoundError"
            | "PermissionError"
            | "FileExistsError"
    )
}

/// Reads newline-ended lines, each at most LINE_LIMIT bytes before its newline (asyncio's StreamReader with
/// `limit`): a longer line is dropped whole and reported once.
struct Lines<R> {
    inner: R,
    buf: Vec<u8>,
    eof: bool,
}

enum Line {
    Text(Vec<u8>),
    TooLong,
    Eof,
}

impl<R: AsyncRead + Unpin> Lines<R> {
    fn new(inner: R) -> Self {
        Lines {
            inner,
            buf: Vec::new(),
            eof: false,
        }
    }

    async fn next(&mut self) -> std::io::Result<Line> {
        let mut skipping = false;
        loop {
            if let Some(i) = self.buf.iter().position(|b| *b == b'\n') {
                let line: Vec<u8> = self.buf.drain(..=i).collect();
                if skipping || i > LINE_LIMIT {
                    return Ok(Line::TooLong);
                }
                return Ok(Line::Text(line));
            }
            if self.buf.len() > LINE_LIMIT {
                self.buf.clear();
                skipping = true;
            }
            if self.eof {
                if self.buf.is_empty() {
                    return Ok(if skipping { Line::TooLong } else { Line::Eof });
                }
                return Ok(Line::Text(std::mem::take(&mut self.buf)));
            }
            let mut chunk = [0u8; 65536];
            let n = self.inner.read(&mut chunk).await?;
            if n == 0 {
                self.eof = true;
            } else {
                self.buf.extend_from_slice(&chunk[..n]);
            }
        }
    }
}

/// codex_stream.py `start` behind CodexRuntime.start (RUN-420: the start's steps on the runtime's clock).
pub async fn start(
    rt: &CodexRuntime,
    task: &Task,
    request: &ModelRequest,
    run_token: &str,
) -> Result<Started, RuntimeError> {
    let mut steps = Steps::new(timer_now(rt));
    check_input(&request.context())?;
    ensure_version(rt).await?;
    let version = steps.mark("version");
    let (run, held) = start_stream(rt, task, request, run_token).await?;
    let mut start_steps = Map::new();
    start_steps.insert("version".into(), json!(version));
    for (k, v) in held {
        start_steps.insert(k, v);
    }
    let mut state = Map::new();
    state.insert("app_server".into(), json!(true));
    state.insert("start_steps".into(), Value::Object(start_steps));
    let ctx_role = run.context_role();
    state.insert("context_role".into(), json!(ctx_role));
    if let Some(t) = run.token_file.lock().ok().and_then(|t| t.clone()) {
        state.insert("token_file".into(), json!(path_str(&t)));
    }
    let handle = RunHandle {
        id: run.turn_info().run_id.clone(),
        runtime: "codex".into(),
        task_id: task.id.clone(),
        state,
    };
    let control: Arc<CodexRun> = run.clone();
    let events = EventStream::spawn(move |y| events_body(run, None, y));
    Ok(Started {
        handle,
        events,
        control,
    })
}

impl CodexRun {
    fn context_role(&self) -> String {
        self.context_role
            .lock()
            .map(|r| r.clone())
            .unwrap_or_default()
    }
}

/// codex_exec.py `ensure_version` / `check_version`: once per binary per process; a mismatch is never remembered.
pub(super) async fn ensure_version(rt: &CodexRuntime) -> Result<(), RuntimeError> {
    let known = rt
        .host
        .caches
        .checked
        .lock()
        .map(|c| c.contains(&rt.binary))
        .unwrap_or(false);
    if known {
        return Ok(());
    }
    let mut spec = Spawn::new(
        vec![path_str(&rt.binary), "--version".into()],
        rt.host.environ.clone(),
    );
    spec.stdin = false;
    spec.stderr = false;
    let mut child = rt.host.spawner.spawn(spec)?;
    let mut out = Vec::new();
    if let Some(mut stdout) = child.stdout.take() {
        stdout.read_to_end(&mut out).await?;
    }
    child.control.wait().await;
    let text = std::str::from_utf8(&out).map_err(|_| {
        RuntimeError::Raised(Raised::new(
            "UnicodeDecodeError",
            "'utf-8' codec can't decode the version output",
        ))
    })?;
    let last = text.split_whitespace().last();
    if last != Some(CODEX_VERSION) {
        return Err(RuntimeError::unsupported(format!(
            "Codex {} is not the pinned {CODEX_VERSION}; rerun spike 1's tests before changing the pin",
            ferb_py::value::strip(text)
        )));
    }
    if let Ok(mut c) = rt.host.caches.checked.lock() {
        c.insert(rt.binary.clone());
    }
    Ok(())
}

fn mkdir_private(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    fs::DirBuilder::new().mode(0o700).create(path)
}

fn new_temporary(root: &Path) -> std::io::Result<PathBuf> {
    // tempfile.TemporaryDirectory(prefix=RUN_PREFIX): a fresh owner-only folder with a random name
    for _ in 0..100 {
        let path = root.join(format!("{RUN_PREFIX}{}", token_hex(4)));
        match mkdir_private(&path) {
            Ok(()) => return Ok(path),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::AlreadyExists,
        "no usable temporary folder name",
    ))
}

/// codex_stream.py `start`: the run's folder, the copied sign-in, the app-server, initialize, the thread (resumed
/// for a cached turn), the turn. Any failure cancels what was started.
async fn start_stream(
    rt: &CodexRuntime,
    task: &Task,
    request: &ModelRequest,
    run_token: &str,
) -> Result<(Arc<CodexRun>, Map<String, Value>), RuntimeError> {
    let mut steps = Steps::new(timer_now(rt));
    let deadline = rt
        .timeout_s
        .map(|s| Instant::now() + Duration::from_secs_f64(s.max(0.0)));
    let run = new_run(rt, &task.id, deadline, None)?;
    let token = (rt.mode == Mode::Worker || !run_token.is_empty()).then_some(run_token);
    let body = async {
        program_body(&run, rt, token, &mut steps).await?;
        turn_body(&run, rt, task, request, &mut steps).await
    };
    let body = async {
        match deadline {
            Some(d) => match tokio::time::timeout_at(d, body).await {
                Ok(r) => r,
                Err(_) => Err(RuntimeError::Raised(Raised::timeout())),
            },
            None => body.await,
        }
    };
    match body.await {
        Ok(()) => Ok((run, steps.steps.clone())),
        Err(e) => {
            run.cancel().await;
            Err(e)
        }
    }
}

/// The run's owner-only folder and its lock, and the run itself (not started yet). `kept`: a program kept across
/// messages (T-1343).
pub(super) fn new_run(
    rt: &CodexRuntime,
    task_id: &str,
    deadline: Option<Instant>,
    kept: Option<KeptLink>,
) -> Result<Arc<CodexRun>, RuntimeError> {
    let Some(home) = codex_home(&rt.ferb_home) else {
        return Err(RuntimeError::account_error("auth_required", None));
    };
    let root = new_temporary(&rt.host.temp_root)?;
    let lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode_private()
        .open(root.join(RUN_LOCK));
    let lock = match lock {
        Ok(l) => l,
        Err(e) => {
            let _ = fs::remove_dir_all(&root);
            return Err(e.into());
        }
    };
    let isolated = root.join("codex");
    let workdir = root.join("work");
    // SAFETY: flock on a descriptor we own; held while this run lives so a start's sweep leaves its folder.
    let locked = unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX) } == 0;
    if let Err(e) = (|| -> std::io::Result<()> {
        if !locked {
            return Err(std::io::Error::last_os_error());
        }
        mkdir_private(&isolated)?;
        mkdir_private(&workdir)
    })() {
        drop(lock);
        let _ = fs::remove_dir_all(&root);
        return Err(e.into());
    }
    let (notes_tx, notes_rx) = mpsc::unbounded_channel();
    let (checked, _) = watch::channel(false);
    Ok(Arc::new(CodexRun {
        turn: Mutex::new(TurnInfo {
            rt: Arc::new(rt.clone()),
            task_id: task_id.to_string(),
            run_id: format!("run-{}", (rt.host.token_hex)(6)),
            deadline,
        }),
        st: Mutex::new(State {
            closed: false,
            cancelled: false,
            turn_completed: false,
            request_id: 0,
            waiters: HashMap::new(),
            seen: BTreeSet::new(),
            violation: None,
            violation_item: None,
            catalog_observed: false,
            catalog_unreadable: false,
            thread_id: None,
            turn_id: None,
            timer: Some(CallTimer::new(timer_now(rt), "runtime")),
            timings: VecDeque::new(),
            usage_seen: VecDeque::new(),
            context_tokens: Value::Null,
            context_window: Value::Null,
            watcher_crash: None,
            ending: false,
            trace_lines: 0,
            turn_trace_start: 0,
            turn_calls: 0,
        }),
        auth: Mutex::new(Some(Auth {
            source: home.join("auth.json"),
            copy: isolated.join("auth.json"),
            original: Vec::new(),
        })),
        proc: Mutex::new(None),
        stdin: tokio::sync::Mutex::new(None),
        notes_tx,
        notes_rx: tokio::sync::Mutex::new(notes_rx),
        checked,
        reader: Mutex::new(None),
        watcher: Mutex::new(None),
        temporary: Mutex::new(Some(root)),
        lock: Mutex::new(Some(lock)),
        token_file: Mutex::new(None),
        cleanup_done: tokio::sync::Mutex::new(false),
        context_role: Mutex::new(String::new()),
        turn_end: tokio::sync::Notify::new(),
        kept,
        generation: std::sync::atomic::AtomicU64::new(0),
        trace_seen: tokio::sync::Notify::new(),
    }))
}

impl CodexRun {
    pub(super) fn root(&self) -> Option<PathBuf> {
        self.temporary.lock().ok().and_then(|t| t.clone())
    }

    /// T-1343: what a kept program was made for.
    pub(super) fn kept_settings(&self) -> &str {
        self.kept
            .as_ref()
            .map(|k| k.settings.as_str())
            .unwrap_or("")
    }

    pub(super) fn kept_back(&self) -> Option<crate::keep::Back> {
        self.kept.as_ref().map(|k| k.back.clone())
    }

    pub(super) fn generation(&self) -> u64 {
        self.generation.load(std::sync::atomic::Ordering::SeqCst)
    }

    pub(super) fn next_generation(&self) {
        self.generation
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }

    /// T-1343: the program can take a message: its app-server runs and its transport is open, nothing outside FERB
    /// was seen, and FERB's sign-in is still the one it copied (another run's refresh or a new sign-in ends it, so a
    /// kept program never goes on with a refresh token that was already used).
    pub(super) fn kept_alive(&self) -> bool {
        {
            let st = self.lock();
            if st.closed || st.cancelled || st.violation.is_some() || st.watcher_crash.is_some() {
                return false;
            }
        }
        if self.proc().is_none_or(|p| p.returncode().is_some()) {
            return false;
        }
        let auth = self.auth.lock().unwrap_or_else(|e| e.into_inner());
        let Some(auth) = auth.as_ref() else {
            return false;
        };
        fs::read(&auth.source).ok().as_deref() == Some(auth.original.as_slice())
    }

    /// T-1343: the message's turn ended in Codex (completed, or interrupted and completed), so nothing of it runs on.
    pub(super) fn turn_ended(&self) -> bool {
        self.lock().turn_completed
    }

    /// T-1343: the message's events end quietly (as a plain-mode cancel's do), and a loop waiting for Codex's next
    /// notification is woken to see it.
    pub(super) fn end_events(&self) {
        self.lock().ending = true;
        let _ = self
            .notes_tx
            .send(Ok(json!({"method": "ferb/messageEnded"})));
    }

    /// T-1343: `thread/unsubscribe` for the message's thread (measured on 0.166.0: the next resume of the same id then
    /// carries nothing of it); anything but "unsubscribed" is a failure.
    pub(super) async fn unsubscribe(&self) -> Result<(), RuntimeError> {
        let Some(thread) = self.lock().thread_id.clone() else {
            return Err(RuntimeError::protocol("Codex run has no thread"));
        };
        let out = self
            .rpc("thread/unsubscribe", json!({"threadId": thread}))
            .await?;
        if out.get("status").and_then(Value::as_str) != Some("unsubscribed") {
            return Err(RuntimeError::protocol(
                "Codex did not unsubscribe the message's thread",
            ));
        }
        let mut st = self.lock();
        st.thread_id = None;
        st.turn_id = None;
        Ok(())
    }

    /// T-1343: the program's token file holds `token` (blank between messages), replaced in one step so its tool
    /// bridge never reads half a token.
    pub(super) fn put_token(&self, token: &str) -> std::io::Result<()> {
        let Some(path) = self.token_file.lock().ok().and_then(|t| t.clone()) else {
            return Ok(());
        };
        let next = path.with_file_name(".run-token.next");
        private_write(&next, token.as_bytes())?;
        fs::rename(&next, &path)
    }

    pub(super) fn context_role_now(&self) -> String {
        self.context_role()
    }

    pub(super) fn token_path(&self) -> Option<PathBuf> {
        self.token_file.lock().ok().and_then(|t| t.clone())
    }

    pub(super) fn run_id_now(&self) -> String {
        self.turn_info().run_id.clone()
    }

    pub(super) fn has_token_file(&self) -> bool {
        self.token_file.lock().ok().is_some_and(|t| t.is_some())
    }

    /// T-1343: the program takes its next message: that message's settings and limit, a new run id, and a fresh
    /// turn state (nothing of the last message's thread, timing, usage or catalog), the token written last. Waits
    /// (at most 2 s) for the last message's events to have ended.
    pub(super) async fn arm(
        &self,
        rt: &CodexRuntime,
        task_id: &str,
        deadline: Option<Instant>,
        token: &str,
    ) -> Result<(), RuntimeError> {
        let Ok(mut rx) = tokio::time::timeout(Duration::from_secs(2), self.notes_rx.lock()).await
        else {
            return Err(RuntimeError::protocol(
                "Codex's previous message did not end",
            ));
        };
        while let Ok(item) = rx.try_recv() {
            if item.is_err() {
                return Err(RuntimeError::protocol("Codex run closed"));
            }
        }
        drop(rx);
        {
            let mut t = self.turn_info();
            t.rt = Arc::new(rt.clone());
            t.task_id = task_id.to_string();
            t.run_id = format!("run-{}", (rt.host.token_hex)(6));
            t.deadline = deadline;
        }
        {
            let mut st = self.lock();
            if st.closed || st.cancelled {
                return Err(RuntimeError::protocol("Codex run is closed"));
            }
            st.turn_completed = false;
            st.seen.clear();
            st.catalog_observed = false;
            st.catalog_unreadable = false;
            st.thread_id = None;
            st.turn_id = None;
            st.timer = Some(CallTimer::new(timer_now(rt), "runtime"));
            st.timings.clear();
            st.usage_seen.clear();
            st.context_tokens = Value::Null;
            st.context_window = Value::Null;
            st.ending = false;
            st.turn_trace_start = st.trace_lines;
            st.turn_calls = 0;
        }
        let reads_trace = self.watcher.lock().ok().is_some_and(|w| w.is_some());
        self.checked.send_replace(!reads_trace);
        if let Ok(mut r) = self.context_role.lock() {
            r.clear();
        }
        self.put_token(token)?;
        Ok(())
    }
}

/// The program's half of a start: the token file (`token`: the run token, or None when the run gets no tools; a
/// kept program's file starts blank), the copied sign-in, the app-server and its readers, `initialize`.
pub(super) async fn program_body(
    run: &Arc<CodexRun>,
    rt: &CodexRuntime,
    token: Option<&str>,
    steps: &mut Steps,
) -> Result<(), RuntimeError> {
    let root = run
        .root()
        .ok_or_else(|| RuntimeError::protocol("Codex run is closed"))?;
    let isolated = root.join("codex");
    let workdir = root.join("work");
    let mut token_file = None;
    if let Some(token) = token {
        let path = root.join("run-token");
        private_write(&path, token.as_bytes())?;
        *run.token_file.lock().unwrap_or_else(|e| e.into_inner()) = Some(path.clone());
        token_file = Some(path);
    }
    let (source, copy) = {
        let a = run.auth.lock().unwrap_or_else(|e| e.into_inner());
        let a = a.as_ref().expect("set when the run was made");
        (a.source.clone(), a.copy.clone())
    };
    let original = fs::read(&source)?;
    if !chatgpt_login(&original) {
        return Err(RuntimeError::account_error("auth_required", None));
    }
    if let Some(a) = run.auth.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
        a.original = original.clone();
    }
    private_write(&copy, &original)?;
    let user_home = rt.ferb_home.join(CODEX_USER);
    {
        use std::os::unix::fs::DirBuilderExt;
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&user_home)?;
    }
    steps.mark("files");
    let mut env = rt.host.environ.clone();
    for k in [
        "OPENAI_API_KEY",
        "CODEX_API_KEY",
        "OPENAI_BASE_URL",
        "CODEX_ACCESS_TOKEN",
        "CODEX_AUTHAPI_BASE_URL",
        "CODEX_AGENT_IDENTITY_AUTHAPI_BASE_URL",
    ] {
        env.remove(k);
    }
    env.insert("HOME".into(), path_str(&user_home));
    env.insert("CODEX_HOME".into(), path_str(&isolated));
    env.insert("RUST_LOG".into(), "trace".into());
    let catalog = locked_catalog(&rt.ferb_home, &rt.host.caches);
    let argv = command(rt, catalog.as_deref(), token_file.as_deref())?;
    let mut spec = Spawn::new(argv, env);
    spec.cwd = Some(workdir.clone());
    let mut child = rt.host.spawner.spawn(spec)?;
    *run.proc.lock().unwrap_or_else(|e| e.into_inner()) = Some(child.control.clone());
    *run.stdin.lock().await = child.stdin.take();
    if let Some(stderr) = child.stderr.take() {
        let w = run.clone();
        *run.watcher.lock().unwrap_or_else(|e| e.into_inner()) =
            Some(tokio::spawn(watch_catalog(w, stderr)));
    } else {
        run.set_checked();
    }
    if let Some(stdout) = child.stdout.take() {
        let r = run.clone();
        *run.reader.lock().unwrap_or_else(|e| e.into_inner()) =
            Some(tokio::spawn(dispatch(r, stdout)));
    }
    steps.mark("spawn");
    run.rpc(
        "initialize",
        json!({"clientInfo": {"name": "ferb", "title": "FERB", "version": "0.27"},
            "capabilities": {"experimentalApi": true}}),
    )
    .await?;
    run.write(&json!({"method": "initialized"})).await?;
    steps.mark("initialize");
    Ok(())
}

/// The message's half of a start: the thread (the role's template resumed for a cached turn, else a new ephemeral
/// thread) and the turn.
pub(super) async fn turn_body(
    run: &Arc<CodexRun>,
    rt: &CodexRuntime,
    task: &Task,
    request: &ModelRequest,
    steps: &mut Steps,
) -> Result<(), RuntimeError> {
    let root = run
        .root()
        .ok_or_else(|| RuntimeError::protocol("Codex run is closed"))?;
    let isolated = root.join("codex");
    let workdir = root.join("work");
    let mut resumed: Option<String> = None;
    let mut developer = false;
    if rt.cache_role.is_some() && rt.mode == Mode::Supervisor {
        let refused = cache_refusal(rt, task, request);
        if !refused.is_empty() {
            if let Some(note) = &rt.on_cache_skipped {
                note(&task.id, &refused);
            }
        }
        resumed = resume_cached(run, request, &isolated, &workdir, refused.is_empty()).await;
        developer = resumed.is_some() && refused.is_empty();
    } else if rt.cache_role.is_some() {
        // a worker-mode run (the checker) resumes its role's template only for the cache key: its whole
        // context stays in the user turn, never the developer role (RUN-416's rule for any refused turn)
        resumed = resume_cached(run, request, &isolated, &workdir, false).await;
    }
    if let Ok(mut r) = run.context_role.lock() {
        *r = if developer { "developer" } else { "user" }.into();
    }
    match &resumed {
        None => {
            let thread = run
                .rpc("thread/start", start_params(rt, request, &workdir))
                .await?;
            let thread = Value::Object(thread);
            let fits = isolated_thread(&thread, rt)? && {
                let t = get_or_empty(&thread, "thread")?;
                get(&t, "ephemeral")?.is_some_and(ferb_py::value::truthy)
            };
            if !fits {
                return Err(RuntimeError::protocol(
                    "Codex runtime configuration did not match FERB isolation",
                ));
            }
            let id = thread
                .get("thread")
                .and_then(|t| t.get("id"))
                .cloned()
                .ok_or_else(|| Raised::key_error("id"))?;
            run.lock().thread_id = Some(id);
        }
        Some(id) => run.lock().thread_id = Some(json!(id)),
    }
    steps.mark(if resumed.is_some() {
        "thread_resume"
    } else {
        "thread_start"
    });
    if let Some(t) = run.lock().timer.as_mut() {
        t.begin();
    }
    let thread_id = run.lock().thread_id.clone().unwrap_or(Value::Null);
    let turn = run
        .rpc(
            "turn/start",
            turn_params(rt, request, &thread_id, developer),
        )
        .await?;
    let turn_id = turn
        .get("turn")
        .ok_or_else(|| Raised::key_error("turn"))?
        .get("id")
        .cloned()
        .ok_or_else(|| Raised::key_error("id"))?;
    run.lock().turn_id = Some(turn_id);
    steps.mark("turn_start");
    Ok(())
}

trait PrivateOpen {
    fn mode_private(&mut self) -> &mut Self;
}

impl PrivateOpen for fs::OpenOptions {
    fn mode_private(&mut self) -> &mut Self {
        use std::os::unix::fs::OpenOptionsExt;
        self.mode(0o600)
    }
}

/// codex_stream.py `_isolated`: no approval, read-only without network, FERB's model on FERB's provider entry, no
/// instruction files.
fn isolated_thread(thread: &Value, rt: &CodexRuntime) -> Result<bool, Raised> {
    if !eq(thread.get("approvalPolicy"), Some(&json!("never"))) {
        return Ok(false);
    }
    let sandbox = match thread.get("sandbox") {
        Some(v) if ferb_py::value::truthy(v) => v.clone(),
        _ => json!({}),
    };
    if !eq(get(&sandbox, "type")?, Some(&json!("readOnly"))) {
        return Ok(false);
    }
    let network = get(&sandbox, "networkAccess")?
        .cloned()
        .unwrap_or(json!(false));
    Ok(!ferb_py::value::truthy(&network)
        && eq(thread.get("model"), Some(&json!(rt.model)))
        && eq(thread.get("modelProvider"), Some(&json!(PROVIDER_ID)))
        && !thread
            .get("instructionSources")
            .is_some_and(ferb_py::value::truthy))
}

/// codex_stream.py `_start_params` (RUN-410).
fn start_params(rt: &CodexRuntime, request: &ModelRequest, workdir: &Path) -> Value {
    json!({"model": rt.model, "modelProvider": PROVIDER_ID, "cwd": path_str(workdir),
        "approvalPolicy": "never", "sandbox": "read-only", "baseInstructions": request.system,
        "developerInstructions": "", "ephemeral": true, "dynamicTools": [], "environments": [],
        "selectedCapabilityRoots": [], "allowProviderModelFallback": false, "experimentalRawEvents": false})
}

/// codex_stream.py `_cache_refusal`.
fn cache_refusal(rt: &CodexRuntime, task: &Task, request: &ModelRequest) -> String {
    if !(0 < request.cacheable && request.cacheable < request.parts.len()) {
        return format!(
            "{} of {} context parts are stable",
            request.cacheable,
            request.parts.len()
        );
    }
    rt.cache_refusal
        .as_ref()
        .map(|f| f(task))
        .unwrap_or_default()
}

/// codex_stream.py `_resume_params`.
fn resume_params(
    rt: &CodexRuntime,
    request: &ModelRequest,
    workdir: &Path,
    thread_id: &str,
    path: &Path,
    developer: bool,
) -> Value {
    let dev = if developer {
        let mut parts = vec![CONTEXT_FRAMING.to_string()];
        parts.push(request.parts[..request.cacheable.min(request.parts.len())].join("\n\n"));
        parts.join("\n\n")
    } else {
        String::new()
    };
    json!({"threadId": thread_id, "path": path_str(path), "excludeTurns": true, "model": rt.model,
        "modelProvider": PROVIDER_ID, "cwd": path_str(workdir), "approvalPolicy": "never",
        "sandbox": "read-only", "baseInstructions": request.system, "developerInstructions": dev})
}

/// codex_stream.py `_turn_params` (L12: images after the text).
fn turn_params(
    rt: &CodexRuntime,
    request: &ModelRequest,
    thread_id: &Value,
    developer: bool,
) -> Value {
    let text = if developer {
        request.parts[request.cacheable.min(request.parts.len())..].join("\n\n")
    } else {
        request.context()
    };
    let mut input = vec![json!({"type": "text", "text": text})];
    for image in &request.images {
        input.push(json!({"type": "image", "url": image.data_url()}));
    }
    let mut m = Map::new();
    m.insert("threadId".into(), thread_id.clone());
    m.insert("input".into(), Value::Array(input));
    if let Some(e) = rt.sent_effort() {
        m.insert("effort".into(), json!(e));
    }
    m.insert(
        "outputSchema".into(),
        rt.output_schema.clone().unwrap_or(Value::Null),
    );
    m.insert("environments".into(), json!([]));
    Value::Object(m)
}

/// codex_stream.py `_resume_cached` (P20, RUN-416): the template thread's id, or None and `on_cache_fallback` told
/// why, when the turn takes the ephemeral path instead. Each run writes its template into its own CODEX_HOME (a fresh
/// owner-only folder), where Codex also keeps the thread's writer lock: runs of one role resuming the same id at once
/// share no file. The resumed thread is not ephemeral: Codex appends the run's turn to that template, which goes
/// with the run's folder.
async fn resume_cached(
    run: &Arc<CodexRun>,
    request: &ModelRequest,
    codex_home: &Path,
    workdir: &Path,
    developer: bool,
) -> Option<String> {
    let rt = run.rt();
    let rt = &*rt;
    let role = rt.cache_role.as_deref().unwrap_or_default();
    let thread_id = cache_thread_id(role, &rt.model, &rt.cache_scope);
    // the effort the turn sends (a supervisor run always sends one), none when it sends none
    let effort = rt.sent_effort();
    let unmatched = format!("the resumed {role} thread did not match FERB's isolation");
    let why = match write_template(
        codex_home,
        workdir,
        &thread_id,
        &rt.model,
        &request.system,
        effort.as_deref(),
    ) {
        Err(e) => format!(
            "the {role} template could not be written ({})",
            crate::error::os_error(&e).class
        ),
        Ok(path) => {
            let params = resume_params(rt, request, workdir, &thread_id, &path, developer);
            match run.rpc("thread/resume", params).await {
                Err(e @ RuntimeError::Protocol { .. }) | Err(e @ RuntimeError::Account { .. }) => {
                    let s = e.to_string();
                    if s.is_empty() {
                        e.class()
                    } else {
                        s
                    }
                }
                Err(RuntimeError::Raised(r)) if is_os_error(&r) => {
                    if r.message.is_empty() {
                        r.class
                    } else {
                        r.message
                    }
                }
                Err(_) => unmatched,
                Ok(thread) => {
                    let thread = Value::Object(thread);
                    let same_id = thread
                        .get("thread")
                        .and_then(|t| t.get("id"))
                        .is_some_and(|id| eq(Some(id), Some(&json!(thread_id))));
                    if isolated_thread(&thread, rt).unwrap_or(false) && same_id {
                        return Some(thread_id);
                    }
                    unmatched
                }
            }
        }
    };
    if let Some(note) = &rt.on_cache_fallback {
        note(&run.task_id(), &why);
    }
    None
}

/// codex_stream.py `_dispatch`: the only stdout reader.
async fn dispatch(run: Arc<CodexRun>, stdout: Box<dyn AsyncRead + Send + Unpin>) {
    let mut lines = Lines::new(stdout);
    let failure = loop {
        let raw = match lines.next().await {
            Ok(Line::Text(l)) => l,
            Ok(Line::Eof) => {
                break RuntimeError::protocol(
                    "Codex runtime disconnected before its turn completed",
                )
            }
            Ok(Line::TooLong) | Err(_) => {
                break RuntimeError::protocol("Codex protocol transport closed or failed")
            }
        };
        let obj = match std::str::from_utf8(&raw)
            .ok()
            .and_then(|t| ferb_py::json::loads(t).ok())
        {
            Some(Value::Object(m)) => m,
            _ => {
                break RuntimeError::protocol("Codex runtime returned an invalid protocol message")
            }
        };
        if obj.contains_key("id") && obj.contains_key("method") {
            // a server-to-client request: FERB answers none; recorded as the run's violation, naming it
            let method = ferb_py::value::py_str(obj.get("method").unwrap_or(&Value::Null));
            {
                let mut st = run.lock();
                st.violation = Some([format!("client request {method}")].into_iter().collect());
                st.violation_item = Some(json!({"kind": "serverRequest", "method": method}));
            }
            let reply = json!({"id": obj.get("id"), "error": {"code": -32601,
                "message": "FERB exposes no client tools"}});
            if run.write(&reply).await.is_err() {
                break RuntimeError::protocol("Codex protocol transport closed or failed");
            }
            break RuntimeError::protocol(format!(
                "Codex runtime requested a disallowed client action ({method})"
            ));
        }
        if let Some(id) = obj.get("id") {
            match waiter_key(id) {
                Err(()) => {
                    break RuntimeError::protocol("Codex protocol transport closed or failed")
                }
                Ok(Some(k)) => {
                    let w = run.lock().waiters.remove(&k);
                    if let Some(w) = w {
                        let _ = w.send(Ok(Value::Object(obj)));
                    }
                }
                Ok(None) => {}
            }
            continue;
        }
        let obj = Value::Object(obj);
        if obj.get("method").and_then(Value::as_str) == Some("turn/completed") {
            let params = obj.get("params").cloned().unwrap_or_else(|| json!({}));
            let matched = (|| -> Result<bool, Raised> {
                let st = run.lock();
                if !eq(get(&params, "threadId")?, st.thread_id.as_ref()) {
                    return Ok(false);
                }
                let turn = get_or_empty(&params, "turn")?;
                Ok(eq(get(&turn, "id")?, st.turn_id.as_ref()))
            })();
            match matched {
                Err(_) => {
                    break RuntimeError::protocol("Codex protocol transport closed or failed")
                }
                Ok(true) => {
                    let waiters: Vec<Waiter> = {
                        let mut st = run.lock();
                        st.turn_completed = true;
                        st.waiters.drain().map(|(_, w)| w).collect()
                    };
                    for w in waiters {
                        let _ = w.send(Err(RuntimeError::protocol(
                            "Codex turn completed before RPC acknowledgement",
                        )));
                    }
                    run.turn_end.notify_waiters();
                }
                Ok(false) => {}
            }
        }
        observe(&run, &obj);
        let _ = run.notes_tx.send(Ok(obj));
    };
    run.close_transport(failure);
}

/// codex_stream.py `_observe` (RUN-421): each model call's timing from the arrival of its notifications.
fn observe(run: &CodexRun, obj: &Value) {
    let empty_params = json!({});
    let params = match obj.get("params") {
        Some(p) if ferb_py::value::truthy(p) => p,
        _ => &empty_params,
    };
    let mut st = run.lock();
    let Value::Object(params) = params else {
        return;
    };
    if st.timer.is_none() || !eq(params.get("threadId"), st.thread_id.as_ref()) {
        return;
    }
    // T-1343: a kept program's earlier message on the same cached thread id is not this one's
    if let (Some(turn), Some(ours)) = (params.get("turnId"), st.turn_id.as_ref()) {
        if !eq(Some(turn), Some(ours)) {
            return;
        }
    }
    let method = match obj.get("method") {
        Some(m) if ferb_py::value::truthy(m) => ferb_py::value::py_str(m),
        _ => String::new(),
    };
    let empty = Map::new();
    let item = match params.get("item") {
        Some(Value::Object(m)) => m,
        _ => &empty,
    };
    let kind = item.get("type");
    let st = &mut *st;
    let timer = st.timer.as_mut().expect("checked above");
    if method == "thread/tokenUsage/updated" {
        let t = timer.done();
        st.timings.push_back(t);
        st.turn_calls += 1;
        let last = params
            .get("tokenUsage")
            .and_then(|u| u.get("last"))
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        st.usage_seen.push_back(last);
    } else if method == "item/completed" && kind == Some(&json!("mcpToolCall")) {
        timer.handed_back();
    } else if method.starts_with("item/") && kind != Some(&json!("userMessage")) {
        timer.output();
    }
}

/// codex_stream.py `_watch_catalog`: the only stderr reader; it reads to the end whatever happens.
async fn watch_catalog(run: Arc<CodexRun>, stderr: Box<dyn AsyncRead + Send + Unpin>) {
    let mut lines = Lines::new(stderr);
    loop {
        let raw = match lines.next().await {
            Ok(Line::Text(l)) => l,
            Ok(Line::Eof) => break,
            Ok(Line::TooLong) => {
                // it may have held the tool array: the catalog counts as unobserved, and reading goes on
                {
                    let mut st = run.lock();
                    st.catalog_unreadable = true;
                    st.catalog_observed = false;
                }
                run.set_checked();
                continue;
            }
            Err(_) => {
                run.lock().catalog_observed = false;
                break;
            }
        };
        if run.lock().violation.is_some() {
            continue; // the run is ending; the pipe is still emptied
        }
        let text = String::from_utf8_lossy(&raw).into_owned();
        let Some(marker) = text.find("\"tools\":[") else {
            continue;
        };
        match crate::pyjson::raw_decode(&text, marker + "\"tools\":".len()) {
            Some((Value::Array(_), _)) => {}
            _ => continue,
        }
        let seen: BTreeSet<String> = match crate::tooltrace::tools_in(&text) {
            Ok(names) => names
                .into_iter()
                .map(|n| {
                    n.strip_prefix("functions.")
                        .map(str::to_string)
                        .unwrap_or(n)
                })
                .collect(),
            Err(r) => {
                run.lock().watcher_crash = Some(r);
                break;
            }
        };
        if let Some(t) = run.lock().timer.as_mut() {
            t.sent(); // Codex's request trace: the provider request goes out now (RUN-421)
        }
        // Codex refreshed its sign-in (if it had to) before this request: published now (RUN-422)
        run.save_refresh();
        let kill = {
            let mut st = run.lock();
            st.trace_lines += 1;
            st.seen.extend(seen.iter().cloned());
            if !st.catalog_unreadable {
                st.catalog_observed = true;
            }
            let extra: BTreeSet<String> = seen.difference(&run.rt().allowed).cloned().collect();
            if extra.is_empty() {
                false
            } else {
                st.violation_item = Some(json!({"kind": "toolCatalog", "tools": extra}));
                st.violation = Some(extra);
                true
            }
        };
        if kill {
            if let Some(p) = run.proc() {
                if p.returncode().is_none() {
                    p.kill();
                }
            }
        }
        run.set_checked();
        run.trace_seen.notify_waiters();
    }
    run.set_checked();
    run.trace_seen.notify_waiters();
}

/// T-1343: how long a kept program's turn waits, once Codex said it completed, for the stderr reader to have checked
/// the request trace of each of the turn's model calls. Codex writes a call's trace before the call goes out, so this
/// is only the reader catching up; past it the turn ends as today's does (the app-server killed and its stderr read to
/// the end) and the program is not kept.
pub const KEPT_TRACE_WAIT_S: f64 = 2.0;

/// T-1343: every model call of the program's current turn had its request trace checked (at least one line, and at
/// least one per call), with no extra tool found; false past [`KEPT_TRACE_WAIT_S`].
pub(super) async fn traced(run: &CodexRun) -> bool {
    let wait = async {
        loop {
            let seen = run.trace_seen.notified();
            tokio::pin!(seen);
            seen.as_mut().enable();
            {
                let st = run.lock();
                if st.violation.is_some() || st.watcher_crash.is_some() {
                    return false;
                }
                if st.trace_lines - st.turn_trace_start >= st.turn_calls.max(1) {
                    return true;
                }
            }
            seen.await;
        }
    };
    tokio::time::timeout(Duration::from_secs_f64(KEPT_TRACE_WAIT_S), wait)
        .await
        .unwrap_or(false)
}

/// codex_stream.py `_finish_trace`: the app-server reaped and its stderr read to the end before the final result.
async fn finish_trace(run: &CodexRun) -> Result<(), RuntimeError> {
    if let Some(p) = run.proc() {
        if p.returncode().is_none() {
            p.kill();
        }
        p.wait().await;
    }
    let watcher = run.watcher.lock().ok().and_then(|mut w| w.take());
    if let Some(w) = watcher {
        let _ = w.await;
    }
    if let Some(crash) = run.lock().watcher_crash.take() {
        return Err(RuntimeError::Raised(crash));
    }
    Ok(())
}

/// codex_stream.py `_check_catalog`.
async fn check_catalog(run: &CodexRun) -> Result<(), RuntimeError> {
    run.wait_checked().await;
    let st = run.lock();
    if st.violation.is_some() {
        return Err(RuntimeError::protocol(
            "Codex runtime exposed tools outside FERB containment",
        ));
    }
    if !st.catalog_observed {
        return Err(RuntimeError::protocol(
            "Codex runtime tool catalog was not observed",
        ));
    }
    Ok(())
}

const PASSING_KINDS: [&str; 5] = [
    "agentMessage",
    "userMessage",
    "reasoning",
    "plan",
    "contextCompaction",
];

fn in_set(v: Option<&Value>, set: &[&str]) -> Result<bool, Raised> {
    match v {
        Some(Value::Array(_)) | Some(Value::Object(_)) => Err(Raised::new(
            "TypeError",
            format!(
                "unhashable type: '{}'",
                crate::error::py_type(v.unwrap_or(&Value::Null))
            ),
        )),
        Some(Value::String(s)) => Ok(set.contains(&s.as_str())),
        _ => Ok(false),
    }
}

/// codex_stream.py `events`.
pub(super) async fn events_body(
    run: Arc<CodexRun>,
    lease: Option<Arc<super::kept::CodexLease>>,
    mut y: Yielder,
) -> Result<(), RuntimeError> {
    let deadline = run.deadline();
    let main = event_loop(&run, &mut y);
    let out = match deadline {
        Some(d) => match tokio::time::timeout_at(d, main).await {
            Ok(r) => r,
            Err(_) => Err(Exit::Fail(RuntimeError::Raised(Raised::timeout()))),
        },
        None => main.await,
    };
    let result = match out {
        Ok(()) | Err(Exit::Closed) => Ok(()),
        Err(Exit::Fail(e)) => {
            let caught = matches!(
                e,
                RuntimeError::Protocol { .. } | RuntimeError::Account { .. }
            ) || matches!(&e, RuntimeError::Raised(r) if r.class == "TimeoutError");
            if caught {
                let (violation, item, cancelled) = {
                    let st = run.lock();
                    (
                        st.violation.clone(),
                        st.violation_item.clone(),
                        st.cancelled || st.ending,
                    )
                };
                let ev = if let Some(v) = violation {
                    let mut data = json!({"unexpected_tools": v});
                    if let Some(item) = item {
                        data["item"] = item;
                    }
                    Some(event(K::Violation, data))
                } else if !cancelled {
                    let message = match &e {
                        RuntimeError::Raised(_) => "Codex runtime timed out".to_string(),
                        other => other.to_string(),
                    };
                    let mut data = json!({"message": message});
                    if let RuntimeError::Protocol {
                        overflow, empty, ..
                    } = &e
                    {
                        if *overflow {
                            data["overflow"] = json!(true);
                        }
                        if !e.account().is_empty() {
                            data["account"] = json!(e.account());
                        }
                        if *empty {
                            data["empty"] = json!(true);
                        }
                    } else if !e.account().is_empty() {
                        data["account"] = json!(e.account());
                    }
                    Some(event(K::Error, data))
                } else {
                    None
                };
                if let Some(ev) = ev {
                    let _ = y.emit(ev).await;
                }
                Ok(())
            } else {
                Err(e)
            }
        }
    };
    match &lease {
        // T-1343: a kept program's message ends; the program goes back to its keeper when it can take another
        Some(l) => l.release(false).await,
        None => run.cancel().await,
    }
    result
}

async fn event_loop(run: &Arc<CodexRun>, y: &mut Yielder) -> Result<(), Exit> {
    let rt = run.rt();
    let rt = &*rt;
    let plain = rt.plain();
    let mut phases: HashMap<String, Value> = HashMap::new();
    let mut final_text = String::new();
    let mut pending: Vec<String> = Vec::new();
    let mut rx = run.notes_rx.lock().await;
    loop {
        {
            let st = run.lock();
            if st.cancelled || st.ending {
                return Ok(());
            }
        }
        if !pending.is_empty() && run.is_checked() {
            check_catalog(run).await?;
            for text in pending.drain(..) {
                y.emit(event(K::TextDelta, json!({"text": text}))).await?;
            }
        }
        let item = if !pending.is_empty() && !run.is_checked() {
            tokio::select! {
                item = rx.recv() => item,
                _ = run.wait_checked() => continue,
            }
        } else {
            rx.recv().await
        };
        if run.lock().ending {
            // RUN-480: what arrives while a plain-mode cancel ends the turn is not told (its usage stays for
            // `unreported_usage`)
            return Ok(());
        }
        let obj = match item {
            Some(Ok(v)) => v,
            Some(Err(e)) => return Err(Exit::Fail(e)),
            None => return Err(Exit::Fail(RuntimeError::protocol("Codex run closed"))),
        };
        let method = obj.get("method").cloned().unwrap_or(Value::Null);
        let method = method.as_str().unwrap_or_default().to_string();
        let params = obj.get("params").cloned().unwrap_or_else(|| json!({}));
        let (thread_id, turn_id) = {
            let st = run.lock();
            (st.thread_id.clone(), st.turn_id.clone())
        };
        if !eq(get(&params, "threadId")?, thread_id.as_ref()) {
            continue;
        }
        if method == "turn/completed" {
            let turn = get_or_empty(&params, "turn")?;
            if !eq(get(&turn, "id")?, turn_id.as_ref()) {
                continue;
            }
            run.lock().turn_completed = true;
            // T-1343: a kept program is not killed to read its trace to the end: the turn waits until each of its
            // model calls' request traces was checked, and falls back to today's end past that wait
            if run.kept.is_some() && traced(run).await {
                if let Some(crash) = run.lock().watcher_crash.take() {
                    return Err(Exit::Fail(RuntimeError::Raised(crash)));
                }
            } else {
                finish_trace(run).await?;
            }
            check_catalog(run).await?;
            let status = get(&turn, "status")?.cloned();
            let error = get(&turn, "error")?.cloned();
            let error_set = error.as_ref().is_some_and(ferb_py::value::truthy);
            let completed = status.as_ref().and_then(Value::as_str) == Some("completed");
            if !completed || error_set || final_text.is_empty() {
                let mut failure = codex_failure(
                    "Codex runtime did not complete its final answer",
                    error.as_ref(),
                    Some(rt),
                );
                if let RuntimeError::Protocol { empty, .. } = &mut failure {
                    *empty = completed && !error_set;
                }
                return Err(Exit::Fail(failure));
            }
            for text in pending.drain(..) {
                y.emit(event(K::TextDelta, json!({"text": text}))).await?;
            }
            let seen: Vec<String> = run.lock().seen.iter().cloned().collect();
            y.emit(event(
                K::Final,
                json!({"text": final_text, "returncode": 0, "tools_seen": seen}),
            ))
            .await?;
            return Ok(());
        }
        if !eq(get(&params, "turnId")?, turn_id.as_ref()) {
            continue;
        }
        let mut progress = true;
        if method == "item/started" || method == "item/completed" {
            let item = get_or_empty(&params, "item")?;
            let kind = get(&item, "type")?.cloned();
            let kind_is = |k: &str| kind.as_ref().and_then(Value::as_str) == Some(k);
            let server = get(&item, "server")?.cloned();
            let tool = get(&item, "tool")?.cloned();
            let mcp_allowed = kind_is("mcpToolCall")
                && server.as_ref().and_then(Value::as_str) == Some("ferb")
                && rt.allowed.contains(&format!(
                    "mcp__ferb.{}",
                    ferb_py::value::py_str(tool.as_ref().unwrap_or(&Value::Null))
                ));
            let listing = kind_is("mcpToolCall")
                && in_set(server.as_ref(), &RESOURCE_LIST_SERVERS)?
                && in_set(tool.as_ref(), &RESOURCE_LISTS)?;
            if !in_set(kind.as_ref(), &PASSING_KINDS)? && !mcp_allowed && !listing {
                let tool_set = tool.as_ref().is_some_and(ferb_py::value::truthy);
                let name = if tool_set {
                    format!(
                        "{}.{}",
                        ferb_py::value::py_str(server.as_ref().unwrap_or(&Value::Null)),
                        ferb_py::value::py_str(tool.as_ref().unwrap_or(&Value::Null))
                    )
                } else {
                    ferb_py::value::py_str(kind.as_ref().unwrap_or(&Value::Null))
                };
                {
                    let mut st = run.lock();
                    st.violation = Some([name].into_iter().collect());
                    st.violation_item = Some(json!({
                        "kind": ferb_py::value::py_str(kind.as_ref().unwrap_or(&Value::Null)),
                        "tool": tool, "server": server}));
                }
                return Err(Exit::Fail(RuntimeError::protocol(
                    "Codex runtime attempted a disallowed action",
                )));
            }
            if mcp_allowed && plain {
                // RUN-480: the call started, then completed; text written before it is told first
                if !pending.is_empty() {
                    check_catalog(run).await?;
                    for text in pending.drain(..) {
                        y.emit(event(K::TextDelta, json!({"text": text}))).await?;
                    }
                }
                let sent = tool.clone().unwrap_or(Value::Null);
                let name = tool
                    .as_ref()
                    .and_then(Value::as_str)
                    .and_then(|t| crate::run::capability(&rt.tool_names, t));
                let arguments = get(&item, "arguments")?.cloned().unwrap_or(Value::Null);
                let status = get(&item, "status")?.cloned();
                let completed = status.as_ref().and_then(Value::as_str) == Some("completed");
                let data = if method == "item/started" {
                    crate::run::tool_phase(sent, name, "started", arguments, None)
                } else {
                    // a failed call carries no result (null): only an object's isError is read
                    let is_error = item
                        .get("result")
                        .and_then(|r| r.get("isError"))
                        .is_some_and(ferb_py::value::truthy);
                    crate::run::tool_phase(
                        sent,
                        name,
                        "completed",
                        arguments,
                        Some(!completed || is_error),
                    )
                };
                if method == "item/completed" && !completed {
                    if let Some(note) = &rt.on_abandoned {
                        let id = get(&item, "id")?.cloned().unwrap_or(Value::Null);
                        note(
                            &run.task_id(),
                            &format!(
                                "{}/{}",
                                ferb_py::value::py_str(thread_id.as_ref().unwrap_or(&Value::Null)),
                                ferb_py::value::py_str(&id)
                            ),
                        );
                    }
                }
                y.emit(event(K::ToolCall, data)).await?;
                progress = false;
            } else if mcp_allowed && method == "item/completed" {
                let status = get(&item, "status")?.cloned();
                if status.as_ref().and_then(Value::as_str) != Some("completed") {
                    if let Some(note) = &rt.on_abandoned {
                        let id = get(&item, "id")?.cloned().unwrap_or(Value::Null);
                        note(
                            &run.task_id(),
                            &format!(
                                "{}/{}",
                                ferb_py::value::py_str(thread_id.as_ref().unwrap_or(&Value::Null)),
                                ferb_py::value::py_str(&id)
                            ),
                        );
                    }
                }
                y.emit(event(
                    K::ToolCall,
                    json!({"tool": tool, "status": status,
                        "arguments": get(&item, "arguments")?.cloned()}),
                ))
                .await?;
                progress = false;
            }
            if kind_is("contextCompaction") && method == "item/completed" {
                let (before, window) = {
                    let st = run.lock();
                    (st.context_tokens.clone(), st.context_window.clone())
                };
                y.emit(event(
                    K::Compacted,
                    json!({"context_tokens_before": before, "context_window": window}),
                ))
                .await?;
                progress = false;
            }
            if kind_is("agentMessage") {
                let id = item.get("id").ok_or_else(|| Raised::key_error("id"))?;
                let phase = get(&item, "phase")?.cloned().unwrap_or(Value::Null);
                phases.insert(hash_key(id)?, phase.clone());
                let text = || ferb_py::value::py_str(item.get("text").unwrap_or(&json!("")));
                // RUN-480: a plain-mode run's answer is its last message, whatever its phase
                if method == "item/completed"
                    && (plain || phase.is_null() || phase.as_str() == Some("final_answer"))
                {
                    final_text = text();
                }
                if rt.mode == Mode::Worker && method == "item/completed" {
                    y.emit(event(K::Message, json!({"text": text()}))).await?;
                    progress = false;
                }
            }
        } else if method == "item/agentMessage/delta" && {
            let key = hash_key(get(&params, "itemId")?.unwrap_or(&Value::Null))?;
            // RUN-480: a plain-mode run streams every message, whatever its phase
            plain || phases.get(&key).and_then(Value::as_str) == Some("final_answer")
        } {
            if rt.output_schema.is_some() || plain {
                if let Some(Value::String(d)) = get(&params, "delta")? {
                    pending.push(d.clone());
                }
            }
        } else if method == "thread/tokenUsage/updated" {
            let token_usage = get_or_empty(&params, "tokenUsage")?;
            let usage = get_or_empty(&token_usage, "last")?;
            let window = get(&token_usage, "modelContextWindow")?.cloned();
            let mut mapped = Map::new();
            for (to, from) in [
                ("input_tokens", "inputTokens"),
                ("cached_input_tokens", "cachedInputTokens"),
                ("output_tokens", "outputTokens"),
                ("reasoning_output_tokens", "reasoningOutputTokens"),
            ] {
                mapped.insert(to.into(), get(&usage, from)?.cloned().unwrap_or(json!(0)));
            }
            let timing = {
                let mut st = run.lock();
                st.context_tokens = get(&usage, "inputTokens")?.cloned().unwrap_or(Value::Null);
                st.context_window = window.unwrap_or(Value::Null);
                st.usage_seen.pop_front();
                st.timings.pop_front()
            };
            let usage = crate::usage::from_codex(&rt.model, &mapped, &(rt.host.today)())?;
            let mut data = usage.as_dict();
            if let Some(t) = timing {
                data.insert("timing".into(), Value::Object(t));
            }
            y.emit(event(K::Usage, Value::Object(data))).await?;
            progress = false;
        } else if method == "error" {
            let will_retry = get(&params, "willRetry")?.cloned();
            let error = get(&params, "error")?.cloned();
            if will_retry == Some(Value::Bool(true)) {
                // Codex retries it itself and the turn goes on (v2 ErrorNotification)
                y.emit(event(
                    K::Retrying,
                    json!({"message": codex_error("Codex is retrying after an error", error.as_ref(), &rt.redact)}),
                ))
                .await?;
                progress = false;
            } else {
                return Err(Exit::Fail(codex_failure(
                    "Codex runtime reported an error",
                    error.as_ref(),
                    Some(rt),
                )));
            }
        }
        if progress {
            y.emit(event(K::Progress, json!({}))).await?;
        }
    }
}

impl RunControl for CodexRun {
    fn steer<'a>(
        &'a self,
        message: &'a str,
    ) -> ferb_core::interfaces::BoxFuture<'a, Result<(), RuntimeError>> {
        Box::pin(CodexRun::steer(self, message))
    }

    fn cancel(&self) -> ferb_core::interfaces::BoxFuture<'_, ()> {
        Box::pin(CodexRun::cancel(self))
    }

    /// RUN-477: the calls Codex reported that the run's events never carried (the run stopped first): one record each.
    fn unreported_usage(&self) -> Vec<Map<String, Value>> {
        let left: Vec<Map<String, Value>> = self.lock().usage_seen.drain(..).collect();
        let rt = self.rt();
        let today = (rt.host.today)();
        left.iter()
            .filter_map(|last| {
                let mut mapped = Map::new();
                for (to, from) in [
                    ("input_tokens", "inputTokens"),
                    ("cached_input_tokens", "cachedInputTokens"),
                    ("output_tokens", "outputTokens"),
                    ("reasoning_output_tokens", "reasoningOutputTokens"),
                ] {
                    mapped.insert(to.into(), last.get(from).cloned().unwrap_or(json!(0)));
                }
                let usage = crate::usage::from_codex(&rt.model, &mapped, &today).ok()?;
                let mut data = usage.as_dict();
                data.insert("recorded_at_end".into(), json!(true));
                Some(data)
            })
            .collect()
    }
}

impl Drop for CodexRun {
    fn drop(&mut self) {
        // a run nobody cancelled: its process and folder still go (Python's generator finalizer would do the same)
        if let Ok(p) = self.proc.get_mut() {
            if let Some(p) = p.take() {
                p.kill();
            }
        }
        if let Ok(t) = self.temporary.get_mut() {
            if let Some(root) = t.take() {
                let _ = fs::remove_dir_all(root);
            }
        }
    }
}

/// The Codex adapter behind the crate's common shape.
impl crate::run::Adapter for CodexRuntime {
    fn name(&self) -> &str {
        "codex"
    }

    fn start<'a>(
        &'a self,
        task: &'a Task,
        request: &'a ModelRequest,
        run_token: &'a str,
    ) -> ferb_core::interfaces::BoxFuture<'a, Result<Started, RuntimeError>> {
        Box::pin(start(self, task, request, run_token))
    }
}

#[allow(dead_code)]
fn _assert_ferb(e: FerbError) -> RuntimeError {
    e.into()
}
