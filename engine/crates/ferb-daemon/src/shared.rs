//! What more than one handler area uses. Port of `ferb/daemon/areas/shared.py` (constants and small helpers), plus
//! what the Rust handlers need that Python got from its one event loop:
//!
//! - [`LoopThread`] and [`Local`]: the daemon's state lives on one thread, as Python's lives on its one asyncio loop.
//!   The engines it holds (`ferb-learning`'s stores, the timers on one SQLite connection) are `Rc`-based and not
//!   `Send`; a handler runs on the server's threads and reaches them by sending a closure to that thread
//!   ([`Local::call`]). Jobs run in the order they arrive and interleave only at their awaits, as Python's handlers
//!   do.
//! - Parameter readers that say what Python's `params.get(...)` idioms do ([`text`], [`get`], [`truthy`]), and the
//!   refusal a handler answers with ([`refusal`]).
//!
//! Published API for both groups (stable): every `pub` item here.

use std::any::Any;
use std::cell::RefCell;
use std::collections::HashMap;
use std::future::Future;
use std::marker::PhantomData;
use std::path::Path;
use std::pin::Pin;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use ferb_core::interfaces::Clock;
use ferb_rpc::server::HandlerError;
use ferb_rpc::wire::Params;
use serde_json::{Map, Value};
use tokio::sync::{mpsc, oneshot};

/// `MACOS_TICK_S`.
pub const MACOS_TICK_S: f64 = 0.2;

/// The FERB window's Tasks page (L34 Phase E): rows per page, most per call.
pub const TASK_PAGE: i64 = 50;
pub const TASK_PAGE_MAX: i64 = 200;

/// An Edit answer that secret redaction hid whole (rule 14): never saved as the owner's words.
pub const EDIT_HIDDEN: &str =
    "Those words looked like a secret, so FERB hid them and saved nothing. Type how FERB \
                               should remember it, without the secret.";

/// A request that failed inside FERB (`_record_internal`): its message's first characters and its innermost frames.
pub const INTERNAL_MESSAGE_CHARS: usize = 300;
pub const INTERNAL_FRAMES: usize = 20;
pub const WITHHELD: &str = "(withheld: it came while a hidden answer was handled)";

/// A problem that keeps happening ([`Repeats`]): its first row at once, then one row per this many seconds.
pub const REPEAT_WINDOW_S: f64 = 600.0;

/// An exception nobody retrieved (the event loop's handler, `_loop_exception`).
pub const UNHANDLED: &str = "daemon.unhandled";

/// How long a kept permission check or browser profile discovery serves before a refresh ([`Fresh`]).
pub const FRESH_S: f64 = 10.0;

/// The redaction mark (`ferb.sanitize.redact.REDACTED`).
pub const REDACTED: &str = "[redacted]";

/// `_all_hidden`: words of which redaction left nothing but its marks.
pub fn all_hidden(text: &str) -> bool {
    !ferb_py::value::strip(text).is_empty()
        && ferb_py::value::strip(&text.replace(REDACTED, "")).is_empty()
}

/// `_code_path`: a traceback's file as a record shows it: under FERB's package from `ferb/`, else its file name alone
/// (never a path through the owner's home). `package` is the package's own folder.
pub fn code_path(filename: &Path, package: &Path) -> String {
    match filename.strip_prefix(package) {
        Ok(rel) => format!(
            "ferb/{}",
            rel.components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join("/")
        ),
        Err(_) => filename
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default(),
    }
}

// ---------- parameters ----------

/// `params.get(key)`: the value, `None` when absent.
pub fn get<'a>(params: &'a Params, key: &str) -> Option<&'a Value> {
    params.get(key)
}

/// Python's truth value of `params.get(key)` (absent is false).
pub fn truthy(params: &Params, key: &str) -> bool {
    params.get(key).is_some_and(ferb_py::value::truthy)
}

/// `str(params.get(key) or "")`: the text of a truthy value, else "".
pub fn text(params: &Params, key: &str) -> String {
    match params.get(key) {
        Some(v) if ferb_py::value::truthy(v) => ferb_py::value::py_str(v),
        _ => String::new(),
    }
}

/// `str(params.get(key, default))`: the text of the value when present (falsy included), else `default`.
pub fn text_or(params: &Params, key: &str, default: &str) -> String {
    match params.get(key) {
        Some(v) => ferb_py::value::py_str(v),
        None => default.to_owned(),
    }
}

/// `params.get(key) is True`.
pub fn is_true(params: &Params, key: &str) -> bool {
    params.get(key) == Some(&Value::Bool(true))
}

/// Python's `repr()` of a decoded JSON value (`{item!r}` in an error's words).
pub fn py_repr(value: &Value) -> String {
    match value {
        Value::String(s) => ferb_clock::py::repr(s),
        Value::Array(items) => format!(
            "[{}]",
            items.iter().map(py_repr).collect::<Vec<_>>().join(", ")
        ),
        Value::Object(map) => format!(
            "{{{}}}",
            map.iter()
                .map(|(k, v)| format!("{}: {}", ferb_clock::py::repr(k), py_repr(v)))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        other => ferb_py::value::py_str(other),
    }
}

/// A FERB refusal (`FerbError` and its subclasses): its words reach the caller.
pub fn refusal(message: impl Into<String>) -> HandlerError {
    HandlerError::refusal("FerbError", message)
}

/// A refusal under its Python class name (`LearningRefused`, `BadSetting`, ...): the class shows on a stream.
pub fn refusal_as(class: &str, message: impl Into<String>) -> HandlerError {
    HandlerError::refusal(class, message)
}

/// An internal failure: the caller learns only its type name.
pub fn fault(class: &str, message: impl Into<String>) -> HandlerError {
    HandlerError::fault(class, message)
}

/// A result object.
pub fn object(value: Value) -> Params {
    match value {
        Value::Object(m) => m,
        _ => Map::new(),
    }
}

// ---------- the daemon's loop ----------

type Job = Box<dyn FnOnce() -> Pin<Box<dyn Future<Output = ()>>> + Send>;

/// The thread the daemon's state lives on: a current-thread runtime with a `LocalSet`, so the jobs it runs may hold
/// `Rc` state and spawn local tasks (Python's one asyncio loop). Cheap to clone; the thread ends when every handle is
/// dropped and its jobs are done.
#[derive(Clone)]
pub struct LoopThread {
    tx: mpsc::UnboundedSender<Job>,
    ids: Arc<AtomicU64>,
}

thread_local! {
    static SLOTS: RefCell<HashMap<u64, Rc<dyn Any>>> = RefCell::new(HashMap::new());
}

/// The loop is gone (its thread ended).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Gone;

impl std::fmt::Display for Gone {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the daemon's loop has stopped")
    }
}

impl std::error::Error for Gone {}

impl From<Gone> for HandlerError {
    fn from(_: Gone) -> HandlerError {
        HandlerError::fault("RuntimeError", "the daemon's loop has stopped")
    }
}

impl LoopThread {
    /// Start the loop's thread (named `name`).
    pub fn start(name: &str) -> LoopThread {
        let (tx, mut rx) = mpsc::unbounded_channel::<Job>();
        std::thread::Builder::new()
            .name(name.to_owned())
            .spawn(move || {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("the daemon loop's runtime");
                let local = tokio::task::LocalSet::new();
                local.block_on(&rt, async move {
                    while let Some(job) = rx.recv().await {
                        tokio::task::spawn_local(job());
                    }
                });
                // the last handle is gone: let what was spawned finish its current step, then drop the state
                rt.block_on(local);
                SLOTS.with(|s| s.borrow_mut().clear());
            })
            .expect("the daemon loop's thread");
        LoopThread {
            tx,
            ids: Arc::new(AtomicU64::new(1)),
        }
    }

    /// Run `job` on the loop (fire and forget).
    pub fn spawn<F, Fut>(&self, job: F) -> Result<(), Gone>
    where
        F: FnOnce() -> Fut + Send + 'static,
        Fut: Future<Output = ()> + 'static,
    {
        self.tx
            .send(Box::new(move || Box::pin(job())))
            .map_err(|_| Gone)
    }

    /// Run `job` on the loop and wait for its answer.
    pub async fn run<R, F, Fut>(&self, job: F) -> Result<R, Gone>
    where
        R: Send + 'static,
        F: FnOnce() -> Fut + Send + 'static,
        Fut: Future<Output = R> + 'static,
    {
        let (done, answer) = oneshot::channel();
        self.spawn(move || async move {
            let _ = done.send(job().await);
        })?;
        answer.await.map_err(|_| Gone)
    }

    /// As [`LoopThread::run`], from a thread that isn't async (start-up, tests).
    pub fn run_blocking<R, F, Fut>(&self, job: F) -> Result<R, Gone>
    where
        R: Send + 'static,
        F: FnOnce() -> Fut + Send + 'static,
        Fut: Future<Output = R> + 'static,
    {
        let (done, answer) = std::sync::mpsc::channel();
        self.spawn(move || async move {
            let _ = done.send(job().await);
        })?;
        answer.recv().map_err(|_| Gone)
    }

    /// Build a state on the loop (`make` runs there) and keep it there; the handle reaches it.
    pub fn install<S, F>(&self, make: F) -> Result<Local<S>, Gone>
    where
        S: 'static,
        F: FnOnce() -> S + Send + 'static,
    {
        let id = self.ids.fetch_add(1, Ordering::SeqCst);
        self.run_blocking(move || async move {
            let state: Rc<dyn Any> = Rc::new(make());
            SLOTS.with(|s| s.borrow_mut().insert(id, state));
        })?;
        Ok(Local {
            thread: self.clone(),
            id,
            _state: PhantomData,
        })
    }

    /// As [`LoopThread::install`], for a state `make` builds as an `Rc` (one that keeps a weak handle to itself) and
    /// may fail to build.
    pub fn try_install_rc<S, F>(&self, make: F) -> Result<Local<S>, String>
    where
        S: 'static,
        F: FnOnce() -> Result<Rc<S>, String> + Send + 'static,
    {
        let id = self.ids.fetch_add(1, Ordering::SeqCst);
        self.run_blocking(move || async move {
            let state: Rc<dyn Any> = make()?;
            SLOTS.with(|s| s.borrow_mut().insert(id, state));
            Ok::<(), String>(())
        })
        .map_err(|e| e.to_string())??;
        Ok(Local {
            thread: self.clone(),
            id,
            _state: PhantomData,
        })
    }
}

/// A handle to one state kept on a [`LoopThread`]. Cheap to clone, `Send` and `Sync` whatever `S` is: `S` itself
/// never leaves its thread.
pub struct Local<S> {
    thread: LoopThread,
    id: u64,
    _state: PhantomData<fn() -> S>,
}

impl<S> Clone for Local<S> {
    fn clone(&self) -> Self {
        Local {
            thread: self.thread.clone(),
            id: self.id,
            _state: PhantomData,
        }
    }
}

impl<S: 'static> Local<S> {
    /// The thread it lives on.
    pub fn thread(&self) -> &LoopThread {
        &self.thread
    }

    /// The state, from a job already on its thread (`None` elsewhere or once dropped).
    pub fn here(&self) -> Option<Rc<S>> {
        let id = self.id;
        SLOTS
            .with(|s| s.borrow().get(&id).cloned())?
            .downcast()
            .ok()
    }

    /// Run `f` with the state on its thread and wait for the answer.
    pub async fn call<R, F, Fut>(&self, f: F) -> Result<R, Gone>
    where
        R: Send + 'static,
        F: FnOnce(Rc<S>) -> Fut + Send + 'static,
        Fut: Future<Output = R> + 'static,
    {
        let me = self.clone();
        let out = self
            .thread
            .run(move || async move {
                match me.here() {
                    Some(state) => Some(f(state).await),
                    None => None,
                }
            })
            .await?;
        out.ok_or(Gone)
    }

    /// As [`Local::call`], from a thread that isn't async.
    pub fn call_blocking<R, F, Fut>(&self, f: F) -> Result<R, Gone>
    where
        R: Send + 'static,
        F: FnOnce(Rc<S>) -> Fut + Send + 'static,
        Fut: Future<Output = R> + 'static,
    {
        let me = self.clone();
        let out = self.thread.run_blocking(move || async move {
            match me.here() {
                Some(state) => Some(f(state).await),
                None => None,
            }
        })?;
        out.ok_or(Gone)
    }

    /// Run `f` with the state on its thread without waiting for it (Python's `_spawn`).
    pub fn spawn<F, Fut>(&self, f: F) -> Result<(), Gone>
    where
        F: FnOnce(Rc<S>) -> Fut + Send + 'static,
        Fut: Future<Output = ()> + 'static,
    {
        let me = self.clone();
        self.thread.spawn(move || async move {
            if let Some(state) = me.here() {
                f(state).await;
            }
        })
    }
}

// ---------- a problem that keeps happening ----------

struct Window {
    kind: String,
    until: ferb_clock::py::Time,
    repeats: u64,
    latest: Map<String, Value>,
    generation: u64,
}

type Write = dyn Fn(&str, Map<String, Value>) + Send + Sync;

/// `_Repeats`: the journal rows of a problem that keeps happening, bounded. The first occurrence of a key is written at
/// once; repeats in the next [`REPEAT_WINDOW_S`] are counted and written as one row (the latest occurrence's, with
/// `repeats: N`) when the window ends, or at shutdown ([`Repeats::flush`]).
#[derive(Clone)]
pub struct Repeats {
    write: Arc<Write>,
    clock: Arc<dyn Clock>,
    held: Arc<Mutex<HashMap<Vec<String>, Window>>>,
    timers: Arc<Mutex<Vec<tokio::task::JoinHandle<()>>>>,
    generation: Arc<AtomicU64>,
}

impl Repeats {
    pub fn new(
        write: impl Fn(&str, Map<String, Value>) + Send + Sync + 'static,
        clock: Arc<dyn Clock>,
    ) -> Repeats {
        Repeats {
            write: Arc::new(write),
            clock,
            held: Arc::default(),
            timers: Arc::default(),
            generation: Arc::default(),
        }
    }

    /// Record one occurrence. Needs a tokio runtime for the window's end.
    pub fn record(&self, kind: &str, key: &[&str], data: Map<String, Value>) {
        let key: Vec<String> = key.iter().map(|k| (*k).to_owned()).collect();
        let now = self.clock.now();
        let mut held = self.held.lock().expect("repeats");
        if let Some(w) = held.get_mut(&key) {
            if now < w.until {
                w.repeats += 1;
                w.latest = data;
                if w.repeats == 1 {
                    let (me, key, until, generation) =
                        (self.clone(), key.clone(), w.until, w.generation);
                    let timer = tokio::spawn(async move {
                        me.clock.sleep_until(until).await;
                        me.end_if(&key, generation);
                    });
                    self.timers.lock().expect("repeats").push(timer);
                }
                return;
            }
        }
        let ended = held.remove(&key);
        drop(held);
        if let Some(w) = ended {
            self.write_end(w);
        }
        (self.write)(kind, data);
        let until = ferb_clock::py::plus_us(&now, (REPEAT_WINDOW_S * 1e6) as i64).unwrap_or(now);
        self.held.lock().expect("repeats").insert(
            key,
            Window {
                kind: kind.to_owned(),
                until,
                repeats: 0,
                latest: Map::new(),
                generation: self.generation.fetch_add(1, Ordering::SeqCst),
            },
        );
    }

    fn end_if(&self, key: &[String], generation: u64) {
        let ended = {
            let mut held = self.held.lock().expect("repeats");
            match held.get(key) {
                Some(w) if w.generation == generation => held.remove(key),
                _ => None,
            }
        };
        if let Some(w) = ended {
            self.write_end(w);
        }
    }

    fn write_end(&self, w: Window) {
        if w.repeats > 0 {
            let mut data = w.latest;
            data.insert("repeats".into(), Value::from(w.repeats));
            (self.write)(&w.kind, data);
        }
    }

    /// At shutdown: every open window's count is written now.
    pub fn flush(&self) {
        for t in self.timers.lock().expect("repeats").drain(..) {
            t.abort();
        }
        let windows: Vec<Window> = self
            .held
            .lock()
            .expect("repeats")
            .drain()
            .map(|(_, w)| w)
            .collect();
        for w in windows {
            self.write_end(w);
        }
    }
}

// ---------- values a slow call reads ----------

type Read = Arc<dyn Fn() -> Value + Send + Sync>;

/// `_Fresh`: values read by a slow call (a TCC check, browser profile files), each kept for `ttl_s`. A read after that
/// gets the kept value at once and refreshes it in a worker thread; only the first read (nothing kept yet) waits.
/// Keyed by a name the caller gives (Python keys by the call itself) and its `salt`.
#[derive(Clone)]
pub struct Fresh {
    pub ttl_s: f64,
    clock: Arc<dyn Clock>,
    held: Arc<Mutex<HashMap<String, (f64, Value)>>>,
    refreshing: Arc<Mutex<std::collections::HashSet<String>>>,
}

impl Fresh {
    pub fn new(ttl_s: f64, clock: Arc<dyn Clock>) -> Fresh {
        Fresh {
            ttl_s,
            clock,
            held: Arc::default(),
            refreshing: Arc::default(),
        }
    }

    /// The kept value of `read` under `key`/`salt`, refreshed in the background once older than `ttl_s`.
    pub fn get(&self, key: &str, salt: &str, read: Read) -> Value {
        let key = format!("{key}\u{0}{salt}");
        let now = self.clock.monotonic();
        let held = self.held.lock().expect("fresh").get(&key).cloned();
        let Some((at, value)) = held else {
            let value = read();
            self.held
                .lock()
                .expect("fresh")
                .insert(key, (now, value.clone()));
            return value;
        };
        if now - at > self.ttl_s && self.refreshing.lock().expect("fresh").insert(key.clone()) {
            match tokio::runtime::Handle::try_current() {
                Ok(rt) => {
                    let me = self.clone();
                    rt.spawn_blocking(move || {
                        let value = read();
                        let at = me.clock.monotonic();
                        me.held
                            .lock()
                            .expect("fresh")
                            .insert(key.clone(), (at, value));
                        me.refreshing.lock().expect("fresh").remove(&key);
                    });
                }
                Err(_) => {
                    let value = read();
                    self.held
                        .lock()
                        .expect("fresh")
                        .insert(key.clone(), (now, value.clone()));
                    self.refreshing.lock().expect("fresh").remove(&key);
                    return value;
                }
            }
        }
        value
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hidden_words_are_only_marks() {
        assert!(all_hidden("[redacted] [redacted]"));
        assert!(!all_hidden("  "));
        assert!(!all_hidden("[redacted] cat"));
    }

    #[test]
    fn text_reads_as_python_does() {
        let p = object(serde_json::json!({"a": 0, "b": "x", "c": null, "d": 1.5, "e": false}));
        assert_eq!(text(&p, "a"), "");
        assert_eq!(text(&p, "b"), "x");
        assert_eq!(text(&p, "c"), "");
        assert_eq!(text(&p, "d"), "1.5");
        assert_eq!(text_or(&p, "e", "y"), "False");
        assert_eq!(text_or(&p, "z", "y"), "y");
    }

    #[test]
    fn local_state_stays_on_its_thread() {
        let lp = LoopThread::start("test-loop");
        let local = lp.install(|| std::cell::Cell::new(1)).unwrap();
        let n = local
            .call_blocking(|s| async move {
                s.set(s.get() + 1);
                s.get()
            })
            .unwrap();
        assert_eq!(n, 2);
    }
}
