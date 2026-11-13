//! The authenticated local RPC transport. Port of `ferb/daemon/server.py`'s `Server`.
//!
//! The first request on a connection must be `auth.hello {role, token?}`; afterwards only the role's methods are
//! allowed. A unary handler returns a result; a stream handler returns a source of notifications. While an MCP
//! shim's call runs, its connection is watched: a shim that leaves (its runtime gave up the call) is a fact the
//! handler can read (`caller_left`), and its result goes nowhere. A runtime that drops a call without telling its
//! shim (Codex) has its adapter name the call (`Server::abandon`), which ends it the same way and closes the
//! shim's connection.
//!
//! One connection is one task and answers one request at a time, in order, as Python's does.

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::io;
use std::os::fd::{AsFd, BorrowedFd};
use std::os::unix::fs::PermissionsExt;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError, RwLock};
use std::task::{Context, Poll};
use std::time::Duration;

use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{watch, Notify};
use tokio::task::{JoinHandle, JoinSet};
use tokio::time::Instant;

use crate::app_peer::AuditToken;
use crate::auth::{self, AuthError, Role, RunGrant, RunTokens};
use crate::framing::{closing, FrameError, FrameReader, FrameWriter, Phase};
use crate::peer;
use crate::wire::{self, Id, Params, Reject, Request};

/// How long the notch waits for a reply (its socket read timeout, DaemonRPC.swift), and the CLI by default
/// (cli/client.py). A later reply reaches neither, unless the caller chose a longer wait (server.py:35-37).
pub const CLIENT_WAIT_S: f64 = 10.0;

/// Calls the notch waits longer for, in seconds (server.py:38-44).
pub const LONG_WAITS: &[(&str, f64)] = &[
    ("automation.request", 120.0),
    ("intro.suggest", 60.0),
    ("reports.build", 120.0),
    ("reports.diagnostic", 60.0),
    ("reports.send", 600.0),
    ("runtimes.check", 60.0),
    ("runtimes.check_calls", 600.0),
    ("runtimes.model", 90.0),
    ("runtimes.models", 30.0),
    ("setup.browser_pair", 90.0),
    ("setup.browsers_pair_all", 90.0),
    ("setup.permission_request", 120.0),
];

/// Calls that end when their caller leaves (server.py:45-47; DMN-403): an abandoned preview request stops its
/// capture, and an abandoned intro suggestion its model call.
pub const CANCEL_WHEN_LEFT: &[&str] = &["intro.suggest", "tasks.preview"];

/// Answered while the daemon is still starting (server.py:51-53; DMN-510). Every other call waits until the
/// start is done.
pub const ANSWERED_WHILE_STARTING: &[&str] = &["ping"];

/// The journal kinds the daemon records `on_internal` and `on_slow` under (server.py:49-50).
pub const INTERNAL_ERROR: &str = "daemon.internal_error";
pub const SLOW_CALL: &str = "daemon.slow_call";

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// What a connection proved about itself (server.py:69-75).
#[derive(Clone, Debug)]
pub struct Session {
    pub role: Role,
    /// An MCP shim's grant, re-checked before each of its calls.
    pub grant: Option<Arc<RunGrant>>,
    /// A CLI's process, as its socket reports it (L12).
    pub pid: Option<i32>,
    /// And its audit token (who it is, for FERB.app-only methods).
    pub token: Option<AuditToken>,
    /// Whether that process is FERB.app: checked once per connection, on first need.
    pub app: Option<bool>,
}

/// How a handler failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Failure {
    /// One of FERB's own errors (Python's `FerbError` and its subclasses): its words are for the caller.
    Refusal,
    /// Anything else: the caller learns only its type; the daemon records the rest (`on_internal`).
    Fault,
}

/// A handler's error. A unary call answers a refusal with `FAILED` and its message, a fault with `INTERNAL` and
/// only `type_name` (server.py:325-329); a stream answers both with `FAILED` and "`type_name`: `message`"
/// (server.py:432). `type_name` is what Python's `type(e).__name__` was ("Refused", "RuntimeError"): clients
/// see it, so a ported handler keeps the name.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{type_name}: {message}")]
pub struct HandlerError {
    pub failure: Failure,
    pub type_name: String,
    pub message: String,
}

impl HandlerError {
    pub fn refusal(type_name: &str, message: impl Into<String>) -> Self {
        HandlerError {
            failure: Failure::Refusal,
            type_name: type_name.to_owned(),
            message: message.into(),
        }
    }

    pub fn fault(type_name: &str, message: impl Into<String>) -> Self {
        HandlerError {
            failure: Failure::Fault,
            type_name: type_name.to_owned(),
            message: message.into(),
        }
    }
}

pub type HandlerResult = Result<Params, HandlerError>;

/// A method that answers once. The future is dropped (cancelled) only for `CANCEL_WHEN_LEFT` methods whose
/// caller left; every other call runs to its end even when nobody waits for it.
pub trait Unary: Send + Sync + 'static {
    fn call(&self, session: Session, params: Params) -> BoxFuture<'static, HandlerResult>;
}

impl<F, Fut> Unary for F
where
    F: Fn(Session, Params) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = HandlerResult> + Send + 'static,
{
    fn call(&self, session: Session, params: Params) -> BoxFuture<'static, HandlerResult> {
        Box::pin(self(session, params))
    }
}

/// The notifications of one stream call, in order. `None` ends the stream; an error ends it with that error.
/// It is dropped when the stream ends or its caller leaves (Python's `aclosing`).
pub trait EventStream: Send {
    fn next(&mut self) -> BoxFuture<'_, Option<HandlerResult>>;
}

/// A stream whose events come through a channel: the producer sends, and dropping the sender ends the stream.
impl EventStream for tokio::sync::mpsc::Receiver<HandlerResult> {
    fn next(&mut self) -> BoxFuture<'_, Option<HandlerResult>> {
        Box::pin(self.recv())
    }
}

/// A method that sends notifications until it ends or its caller leaves.
pub trait Stream: Send + Sync + 'static {
    fn open(&self, session: Session, params: Params) -> Box<dyn EventStream>;
}

impl<F> Stream for F
where
    F: Fn(Session, Params) -> Box<dyn EventStream> + Send + Sync + 'static,
{
    fn open(&self, session: Session, params: Params) -> Box<dyn EventStream> {
        self(session, params)
    }
}

/// A native host's connection after its hello: from here on it carries extension messages, not RPC
/// (server.py:247-249).
pub struct NativeConnection {
    pub reader: OwnedReadHalf,
    /// What the host sent right behind its hello, already read off the socket.
    pub buffered: Vec<u8>,
    pub writer: OwnedWriteHalf,
}

/// The extension message channel (the browser bridge). Its future is dropped when the server closes.
pub trait NativeChannel: Send + Sync + 'static {
    fn run(&self, connection: NativeConnection) -> BoxFuture<'static, ()>;
}

type ExtensionIds = dyn Fn() -> HashSet<String> + Send + Sync;
type IsApp = dyn Fn(Option<AuditToken>) -> bool + Send + Sync;
type PeerLookup = dyn Fn(BorrowedFd<'_>) -> PeerFacts + Send + Sync;

/// What the platform says about the process on the other end of a connection (server.py:91-103,
/// app_peer.py:27-33): each `None` when the socket can't say.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PeerFacts {
    pub pid: Option<i32>,
    pub token: Option<AuditToken>,
}
type OnInternal = dyn Fn(&str, &HandlerError) + Send + Sync;
type OnSlow = dyn Fn(&str, f64) + Send + Sync;
type OnNativeRefused = dyn Fn(&str, &str) + Send + Sync;

/// Whether the MCP shim call this code runs for has lost its caller (the connection closed mid-call, or the
/// runtime's adapter abandoned the call). Python keeps this in a context variable that tasks a handler starts
/// inherit; a tokio task does not inherit it, so a handler that spawns takes `caller_left_flag()` along.
pub fn caller_left() -> bool {
    CALLER_LEFT.try_with(|left| left.is_set()).unwrap_or(false)
}

/// The flag `caller_left()` reads, to carry into a spawned task. `None` outside an MCP shim's call.
pub fn caller_left_flag() -> Option<CallerLeft> {
    CALLER_LEFT
        .try_with(|left| CallerLeft(Arc::clone(left)))
        .ok()
}

/// See `caller_left_flag`.
#[derive(Clone)]
pub struct CallerLeft(Arc<Left>);

impl CallerLeft {
    pub fn is_set(&self) -> bool {
        self.0.is_set()
    }
}

tokio::task_local! {
    static CALLER_LEFT: Arc<Left>;
}

struct Left {
    flag: AtomicBool,
    abandoned: Notify,
}

impl Left {
    fn is_set(&self) -> bool {
        self.flag.load(Ordering::SeqCst)
    }

    fn set(&self) {
        self.flag.store(true, Ordering::SeqCst);
    }
}

/// Why the server did not start.
#[derive(Debug, thiserror::Error)]
pub enum StartError {
    /// Another daemon is already answering on the socket (`core.errors.AlreadyRunning`).
    #[error("another daemon is answering on {0}")]
    AlreadyRunning(String),
    #[error(transparent)]
    Io(#[from] io::Error),
}

/// A leftover socket file from a crash is removed; a live daemon behind it means we must not start
/// (server.py:111-123).
pub fn ensure_socket_free(path: &Path) -> Result<(), StartError> {
    if !path.exists() {
        return Ok(());
    }
    match std::os::unix::net::UnixStream::connect(path) {
        Ok(_) => Err(StartError::AlreadyRunning(path.display().to_string())),
        Err(e)
            if matches!(
                e.kind(),
                io::ErrorKind::ConnectionRefused | io::ErrorKind::NotFound
            ) =>
        {
            match std::fs::remove_file(path) {
                Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e.into()),
                _ => Ok(()),
            }
        }
        Err(e) => Err(e.into()),
    }
}

fn read<T: ?Sized>(lock: &RwLock<Arc<T>>) -> Arc<T> {
    Arc::clone(&lock.read().unwrap_or_else(PoisonError::into_inner))
}

type ShimCalls = HashMap<(String, String), Vec<Arc<Left>>>;

struct Inner {
    path: PathBuf,
    tokens: Arc<RunTokens>,
    owner_uid: u32,
    unary: RwLock<HashMap<String, Arc<dyn Unary>>>,
    streams: RwLock<HashMap<String, Arc<dyn Stream>>>,
    native: RwLock<Option<Arc<dyn NativeChannel>>>,
    extension_ids: RwLock<Arc<ExtensionIds>>,
    is_app: RwLock<Arc<IsApp>>,
    peer_lookup: RwLock<Arc<PeerLookup>>,
    on_internal: RwLock<Arc<OnInternal>>,
    on_slow: RwLock<Arc<OnSlow>>,
    on_native_refused: RwLock<Arc<OnNativeRefused>>,
    phase: watch::Sender<Phase>,
    /// (task id, the runtime's call id) -> each running MCP shim call so named, oldest first (a runtime may
    /// repeat a call id; each call can still be released) (server.py:137-139).
    shim_calls: Mutex<ShimCalls>,
    accepting: Mutex<Option<JoinHandle<()>>>,
}

/// The RPC server on one Unix socket. Cheap to clone; every clone is the same server.
#[derive(Clone)]
pub struct Server {
    inner: Arc<Inner>,
}

impl Server {
    /// A server for the socket at `path`, owned by this process's user. Nothing is opened until `start`.
    pub fn new(path: impl Into<PathBuf>, tokens: Arc<RunTokens>) -> Self {
        Self::with_owner_uid(path, tokens, peer::current_uid())
    }

    /// As `new`, for an owner other than this process's user (tests: server.py:127 `owner_uid`).
    pub fn with_owner_uid(
        path: impl Into<PathBuf>,
        tokens: Arc<RunTokens>,
        owner_uid: u32,
    ) -> Self {
        let (phase, _) = watch::channel(Phase {
            ready: true,
            closing: false,
        });
        Server {
            inner: Arc::new(Inner {
                path: path.into(),
                tokens,
                owner_uid,
                unary: RwLock::default(),
                streams: RwLock::default(),
                native: RwLock::new(None),
                extension_ids: RwLock::new(Arc::new(HashSet::new)),
                // No check in place: nobody is FERB.app (fails closed; server.py:158-161).
                is_app: RwLock::new(Arc::new(|_| false)),
                // No lookup in place: no pid and no audit token, so nobody is FERB.app either.
                peer_lookup: RwLock::new(Arc::new(|_| PeerFacts::default())),
                on_internal: RwLock::new(Arc::new(|_, _| {})),
                on_slow: RwLock::new(Arc::new(|_, _| {})),
                on_native_refused: RwLock::new(Arc::new(|_, _| {})),
                phase,
                shim_calls: Mutex::default(),
                accepting: Mutex::new(None),
            }),
        }
    }

    pub fn path(&self) -> &Path {
        &self.inner.path
    }

    pub fn tokens(&self) -> &Arc<RunTokens> {
        &self.inner.tokens
    }

    /// Registers (or replaces) the handler of a unary method. A method outside `auth::METHODS` is never
    /// reached: the role check refuses it first.
    pub fn register_unary(&self, method: &str, handler: impl Unary) {
        let mut unary = self
            .inner
            .unary
            .write()
            .unwrap_or_else(PoisonError::into_inner);
        unary.insert(method.to_owned(), Arc::new(handler));
    }

    /// Registers (or replaces) the handler of a stream method. A name registered both ways answers as unary
    /// (server.py:311, 338).
    pub fn register_stream(&self, method: &str, handler: impl Stream) {
        let mut streams = self
            .inner
            .streams
            .write()
            .unwrap_or_else(PoisonError::into_inner);
        streams.insert(method.to_owned(), Arc::new(handler));
    }

    /// The extension message channel; without one a native host's hello is refused with `UNSUPPORTED`.
    pub fn set_native(&self, channel: impl NativeChannel) {
        *self
            .inner
            .native
            .write()
            .unwrap_or_else(PoisonError::into_inner) = Some(Arc::new(channel));
    }

    /// FERB's extension ids, asked at each native host's hello.
    pub fn set_extension_ids(&self, ids: impl Fn() -> HashSet<String> + Send + Sync + 'static) {
        *self
            .inner
            .extension_ids
            .write()
            .unwrap_or_else(PoisonError::into_inner) = Arc::new(ids);
    }

    /// Whether a cli connection's process is FERB.app (its audit token -> bool; `AppPeers::into_check`): the
    /// gate of FERB.app-only methods. Until set, nobody is. It may block (it asks the Security framework), so
    /// it runs on the blocking pool.
    pub fn set_is_app(&self, check: impl Fn(Option<AuditToken>) -> bool + Send + Sync + 'static) {
        *self
            .inner
            .is_app
            .write()
            .unwrap_or_else(PoisonError::into_inner) = Arc::new(check);
    }

    /// How a connection's process and audit token are read from its socket: the platform's part
    /// (`ferb_platform_macos::peer::facts` on the Mac). Until set, a session has neither.
    pub fn set_peer_lookup(
        &self,
        lookup: impl Fn(BorrowedFd<'_>) -> PeerFacts + Send + Sync + 'static,
    ) {
        *self
            .inner
            .peer_lookup
            .write()
            .unwrap_or_else(PoisonError::into_inner) = Arc::new(lookup);
    }

    /// Told a handler's fault (its method and the error; the reply names only its type), for the daemon to
    /// record (L36). A recording hook never changes a reply or ends a connection.
    pub fn on_internal(&self, hook: impl Fn(&str, &HandlerError) + Send + Sync + 'static) {
        *self
            .inner
            .on_internal
            .write()
            .unwrap_or_else(PoisonError::into_inner) = Arc::new(hook);
    }

    /// Told an owner-side call that took as long as its caller waits, or longer (its method and seconds).
    pub fn on_slow(&self, hook: impl Fn(&str, f64) + Send + Sync + 'static) {
        *self
            .inner
            .on_slow
            .write()
            .unwrap_or_else(PoisonError::into_inner) = Arc::new(hook);
    }

    /// HAND-406: told a native host's refused hello (why, and the origin it gave, at most 120 characters): the
    /// extension never gets as far as a hello, and the host only exits, so the daemon records it.
    pub fn on_native_refused(&self, hook: impl Fn(&str, &str) + Send + Sync + 'static) {
        *self
            .inner
            .on_native_refused
            .write()
            .unwrap_or_else(PoisonError::into_inner) = Arc::new(hook);
    }

    /// DMN-510: the daemon serves before its slow start-up work. Until `set_ready` only
    /// `ANSWERED_WHILE_STARTING` is answered and every other call waits. A server nobody marks as starting
    /// answers everything.
    pub fn set_starting(&self) {
        self.inner.phase.send_modify(|phase| phase.ready = false);
    }

    /// The start is done: waiting calls are answered.
    pub fn set_ready(&self) {
        self.inner.phase.send_modify(|phase| phase.ready = true);
    }

    pub fn is_ready(&self) -> bool {
        self.inner.phase.borrow().ready
    }

    /// The runtime of `task_id` gave up its call `call_id` without telling the shim (Codex sends no
    /// cancellation): the call sees `caller_left()`, as if the shim had left, and the shim's connection is
    /// closed, so its waiting thread ends. Only that task's own call, the oldest still running under that id
    /// (one per report); false when no such call is running (server.py:173-186).
    pub fn abandon(&self, task_id: &str, call_id: &str) -> bool {
        let mut calls = self
            .inner
            .shim_calls
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let key = (task_id.to_owned(), call_id.to_owned());
        let Some(running) = calls.get_mut(&key) else {
            return false;
        };
        if running.is_empty() {
            return false;
        }
        let left = running.remove(0);
        if running.is_empty() {
            calls.remove(&key);
        }
        left.set();
        left.abandoned.notify_one();
        true
    }

    /// Makes the socket's folder (0700) and the socket (0600) and starts accepting (server.py:188-206). The
    /// socket file is made 0600 between bind and listen, before anyone can connect, and never through the
    /// umask, which is process-wide.
    pub async fn start(&self) -> Result<(), StartError> {
        let path = &self.inner.path;
        if let Some(folder) = path.parent() {
            std::fs::create_dir_all(folder)?;
            std::fs::set_permissions(folder, std::fs::Permissions::from_mode(0o700))?;
        }
        ensure_socket_free(path)?;
        let socket = socket2::Socket::new(socket2::Domain::UNIX, socket2::Type::STREAM, None)?;
        socket.bind(&socket2::SockAddr::unix(path)?)?;
        let listening = (|| {
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
            socket.listen(100)?; // asyncio's default backlog
            socket.set_nonblocking(true)?;
            UnixListener::from_std(socket.into())
        })();
        let listener = match listening {
            Ok(listener) => listener,
            Err(e) => {
                let _ = std::fs::remove_file(path); // only the file this start made
                return Err(e.into());
            }
        };
        let accepting = tokio::spawn(accept(Arc::clone(&self.inner), listener));
        *self
            .inner
            .accepting
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(accepting);
        Ok(())
    }

    /// Stops accepting, closes the open connections (e.g. a running `ferb logs`), waits for them and removes
    /// the socket file (server.py:208-218). Calls waiting for a start that won't finish end without an answer.
    /// A unary call under way is not cancelled; its connection ends when it returns.
    pub async fn close(&self) {
        self.inner.phase.send_modify(|phase| {
            phase.closing = true;
            phase.ready = true;
        });
        let accepting = self
            .inner
            .accepting
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        if let Some(accepting) = accepting {
            let _ = accepting.await;
            // only its own socket: never another daemon's (a failed start)
            let _ = std::fs::remove_file(&self.inner.path);
        }
    }
}

async fn accept(inner: Arc<Inner>, listener: UnixListener) {
    let mut phase = inner.phase.subscribe();
    let mut connections = JoinSet::new();
    loop {
        tokio::select! {
            biased;
            _ = closing(&mut phase) => break,
            accepted = listener.accept() => match accepted {
                Ok((stream, _)) => {
                    connections.spawn(connection(Arc::clone(&inner), stream));
                }
                // out of descriptors and the like: don't spin
                Err(_) => tokio::time::sleep(Duration::from_millis(50)).await,
            },
            Some(_) = connections.join_next(), if !connections.is_empty() => {}
        }
    }
    drop(listener);
    while connections.join_next().await.is_some() {}
}

/// What a step did to the connection.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Flow {
    Serve,
    End,
}

fn flow(written: io::Result<()>) -> Flow {
    // a reply's write failing is the caller leaving (server.py:253-254)
    if written.is_ok() {
        Flow::Serve
    } else {
        Flow::End
    }
}

/// A recording hook never changes a reply or ends a connection (server.py:167-171).
fn tell(hook: impl FnOnce()) {
    let _ = catch_unwind(AssertUnwindSafe(hook));
}

// server.py:222-257
async fn connection(inner: Arc<Inner>, stream: UnixStream) {
    let Ok(uid) = peer::peer_uid(&stream) else {
        return;
    };
    let PeerFacts { pid, token } = read(&inner.peer_lookup)(stream.as_fd());
    let (reading, writing) = stream.into_split();
    let mut reader = FrameReader::new(reading, inner.phase.subscribe());
    let mut writer = FrameWriter::new(writing, inner.phase.subscribe());
    let mut session: Option<Session> = None;
    loop {
        let line = match reader.read_line().await {
            Ok(Some(line)) => line,
            Ok(None) | Err(FrameError::Io(_)) => return,
            Err(FrameError::TooLarge) => {
                let message = format!("message exceeds {} bytes", wire::MAX_MESSAGE);
                let _ = writer
                    .send(&wire::error_line(&Id::Null, wire::TOO_LARGE, &message))
                    .await;
                return;
            }
        };
        let rejected = match wire::parse_request(&line) {
            Ok(req) => match session.as_mut() {
                Some(session) => {
                    if dispatch(&inner, session, req, &mut reader, &mut writer).await == Flow::End {
                        return;
                    }
                    continue;
                }
                None => {
                    let Some(greeted) = hello(&inner, &req, uid, pid, token, &mut writer).await
                    else {
                        return;
                    };
                    if greeted.role == Role::NativeHost {
                        let native = inner
                            .native
                            .read()
                            .unwrap_or_else(PoisonError::into_inner)
                            .clone();
                        if let Some(native) = native {
                            // the rest of the connection is extension messages
                            let (reader, buffered) = reader.into_parts();
                            let connection = NativeConnection {
                                reader,
                                buffered,
                                writer: writer.into_half(),
                            };
                            let mut phase = inner.phase.subscribe();
                            tokio::select! {
                                _ = native.run(connection) => {}
                                _ = closing(&mut phase) => {}
                            }
                            return;
                        }
                    }
                    session = Some(greeted);
                    continue;
                }
            },
            Err(Reject::NotText) => return,
            Err(Reject::Parse) => wire::error_line(&Id::Null, wire::PARSE_ERROR, "invalid JSON"),
            Err(Reject::Invalid(message)) => {
                wire::error_line(&Id::Null, wire::INVALID_REQUEST, message)
            }
        };
        if writer.send(&rejected).await.is_err() {
            return;
        }
    }
}

/// The first request of a connection (server.py:259-288). `None`: refused, the connection ends.
async fn hello(
    inner: &Inner,
    req: &Request,
    uid: u32,
    pid: Option<i32>,
    token: Option<AuditToken>,
    writer: &mut FrameWriter,
) -> Option<Session> {
    if req.method != "auth.hello" {
        let _ = writer
            .send(&wire::error_line(
                &req.id,
                wire::UNAUTHORIZED,
                "authenticate first (auth.hello)",
            ))
            .await;
        return None;
    }
    let refuse = |code: i64, why: &str| {
        // HAND-406 (server.py:163-165): only a caller that named the native_host role is recorded
        if req.params.get("role").and_then(|role| role.as_str()) == Some(Role::NativeHost.as_str())
        {
            let origin = req
                .params
                .get("origin")
                .and_then(|origin| origin.as_str())
                .unwrap_or("");
            let origin: String = origin.chars().take(120).collect();
            let hook = read(&inner.on_native_refused);
            tell(|| hook(why, &origin));
        }
        wire::error_line(&req.id, code, why)
    };
    let greeted: Result<Session, Vec<u8>> = match wire::parse_hello(&req.params) {
        Err(message) => Err(wire::error_line(&req.id, wire::INVALID_REQUEST, message)),
        Ok(hello) => match hello.role {
            Role::Cli => auth::check_cli_peer(uid, inner.owner_uid)
                .map(|()| Session {
                    role: Role::Cli,
                    grant: None,
                    pid,
                    token,
                    app: None,
                })
                .map_err(|AuthError(why)| refuse(wire::UNAUTHORIZED, &why)),
            Role::McpShim => inner
                .tokens
                .validate(hello.token.as_deref().unwrap_or(""))
                .map(|grant| Session {
                    role: Role::McpShim,
                    grant: Some(grant),
                    pid: None,
                    token: None,
                    app: None,
                })
                .map_err(|AuthError(why)| refuse(wire::UNAUTHORIZED, &why)),
            Role::NativeHost => {
                if inner
                    .native
                    .read()
                    .unwrap_or_else(PoisonError::into_inner)
                    .is_none()
                {
                    Err(refuse(
                        wire::UNSUPPORTED,
                        "FERB's browser bridge isn't running",
                    ))
                } else {
                    let ids = read(&inner.extension_ids)();
                    auth::check_native_host(
                        uid,
                        inner.owner_uid,
                        hello.origin.as_deref().unwrap_or(""),
                        &ids,
                    )
                    .map(|()| Session {
                        role: Role::NativeHost,
                        grant: None,
                        pid: None,
                        token: None,
                        app: None,
                    })
                    .map_err(|AuthError(why)| refuse(wire::UNAUTHORIZED, &why))
                }
            }
        },
    };
    match greeted {
        Ok(session) => {
            let mut result = Params::new();
            result.insert("role".to_owned(), session.role.as_str().into());
            writer
                .send(&wire::result_line(&req.id, &result))
                .await
                .ok()?;
            Some(session)
        }
        Err(refusal) => {
            let _ = writer.send(&refusal).await;
            None
        }
    }
}

/// Answer one request; `Flow::End` when the caller has left (the connection ends) (server.py:290-342).
async fn dispatch(
    inner: &Arc<Inner>,
    session: &mut Session,
    req: Request,
    reader: &mut FrameReader,
    writer: &mut FrameWriter,
) -> Flow {
    let role = session.role;
    if role == Role::McpShim {
        // re-check on every call: revoked/expired tokens stop at once
        let checked = match &session.grant {
            None => Err(AuthError("mcp_shim session without a run token".to_owned())),
            Some(grant) => inner.tokens.recheck(grant),
        };
        match checked {
            Ok(grant) => session.grant = Some(grant),
            Err(AuthError(why)) => {
                return flow(
                    writer
                        .send(&wire::error_line(&req.id, wire::UNAUTHORIZED, &why))
                        .await,
                );
            }
        }
    }
    if !auth::allowed(role, &req.method) {
        let why = format!("{} not allowed for {}", req.method, role.as_str());
        return flow(
            writer
                .send(&wire::error_line(&req.id, wire::UNAUTHORIZED, &why))
                .await,
        );
    }
    if auth::app_only(&req.method) && !is_app(inner, session).await {
        let why = format!("{} is for FERB.app only", req.method);
        return flow(
            writer
                .send(&wire::error_line(&req.id, wire::APP_ONLY_REFUSED, &why))
                .await,
        );
    }
    if !inner.phase.borrow().ready && !ANSWERED_WHILE_STARTING.contains(&req.method.as_str()) {
        let mut phase = inner.phase.subscribe();
        let waited = phase.wait_for(|p| p.ready).await.map(|p| *p);
        // the start failed or the daemon is stopping: nothing to answer with
        if !matches!(waited, Ok(Phase { closing: false, .. })) {
            return Flow::End;
        }
    }
    let unary = inner
        .unary
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .get(&req.method)
        .cloned();
    if let Some(handler) = unary {
        let started = Instant::now();
        let call = guarded(Box::pin({
            let (session, params) = (session.clone(), req.params.clone());
            async move { handler.call(session, params).await }
        }));
        let outcome = if role == Role::McpShim {
            watched(inner, session, &req, call, reader, writer).await
        } else if CANCEL_WHEN_LEFT.contains(&req.method.as_str()) {
            while_caller_stays(call, reader).await
        } else {
            Some(call.await)
        };
        let answered = match outcome {
            None => Flow::End,
            Some(Ok(result)) => flow(writer.send(&wire::result_line(&req.id, &result)).await),
            Some(Err(error)) if error.failure == Failure::Refusal => flow(
                writer
                    .send(&wire::error_line(&req.id, wire::FAILED, &error.message))
                    .await,
            ),
            // failures are values, never a crashed connection
            Some(Err(error)) => {
                let hook = read(&inner.on_internal);
                tell(|| hook(&req.method, &error));
                flow(
                    writer
                        .send(&wire::error_line(&req.id, wire::INTERNAL, &error.type_name))
                        .await,
                )
            }
        };
        // an MCP shim waits for its call however long it runs; the notch and the CLI stop waiting
        if role != Role::McpShim {
            let seconds = started.elapsed().as_secs_f64();
            let limit = LONG_WAITS
                .iter()
                .find(|(method, _)| *method == req.method)
                .map_or(CLIENT_WAIT_S, |w| w.1);
            if seconds >= limit {
                let hook = read(&inner.on_slow);
                tell(|| hook(&req.method, seconds));
            }
        }
        return answered;
    }
    let stream = inner
        .streams
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .get(&req.method)
        .cloned();
    if let Some(handler) = stream {
        return stream_call(inner, handler, session, &req, reader, writer).await;
    }
    let why = format!("no method {}", req.method);
    flow(
        writer
            .send(&wire::error_line(&req.id, wire::METHOD_NOT_FOUND, &why))
            .await,
    )
}

/// Whether this connection's process is FERB.app (auth's `app_only`), checked once per connection
/// (server.py:344-351). A check that fails in any way is a no.
async fn is_app(inner: &Inner, session: &mut Session) -> bool {
    if let Some(known) = session.app {
        return known;
    }
    let verdict = if session.role == Role::Cli {
        let (check, token) = (read(&inner.is_app), session.token);
        tokio::task::spawn_blocking(move || check(token))
            .await
            .unwrap_or(false)
    } else {
        false
    };
    session.app = Some(verdict);
    verdict
}

/// A handler's future with a panic turned into a fault, as Python turns any exception into one.
struct Guarded<T>(BoxFuture<'static, Result<T, HandlerError>>);

fn guarded<T>(call: BoxFuture<'static, Result<T, HandlerError>>) -> Guarded<T> {
    Guarded(call)
}

impl<T> Future for Guarded<T> {
    type Output = Result<T, HandlerError>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        match catch_unwind(AssertUnwindSafe(|| self.0.as_mut().poll(cx))) {
            Ok(polled) => polled,
            Err(_) => Poll::Ready(Err(HandlerError::fault("Panic", "the handler panicked"))),
        }
    }
}

/// A call that ends when its caller leaves (`CANCEL_WHEN_LEFT`): the connection's end of input, seen without
/// reading anything a caller sends after the call, cancels it. `None`: the caller left (server.py:353-370).
/// Python looks every 0.1 s (`LEFT_POLL_S`); this sees the end as it happens.
async fn while_caller_stays(
    call: Guarded<Params>,
    reader: &mut FrameReader,
) -> Option<HandlerResult> {
    tokio::select! {
        biased;
        result = call => Some(result),
        _ = reader.at_eof() => None, // the call's future is dropped here: cancelled
    }
}

/// An MCP shim's call, watching its connection (a shim sends one call and then only closes it). When the shim
/// leaves first, or its runtime's adapter abandons the call, the call is not cancelled (an action already under
/// way finishes and is recorded); it can see `caller_left()`, and its result goes nowhere: `None`
/// (server.py:372-406).
async fn watched(
    inner: &Arc<Inner>,
    session: &Session,
    req: &Request,
    call: Guarded<Params>,
    reader: &mut FrameReader,
    writer: &mut FrameWriter,
) -> Option<HandlerResult> {
    let left = Arc::new(Left {
        flag: AtomicBool::new(false),
        abandoned: Notify::new(),
    });
    let key = match (
        &session.grant,
        req.params.get("call_id").and_then(|id| id.as_str()),
    ) {
        (Some(grant), Some(call_id)) => Some((grant.task_id.clone(), call_id.to_owned())),
        _ => None,
    };
    let named = Named {
        inner,
        key,
        left: &left,
    };
    named.add();
    let mut call = std::pin::pin!(CALLER_LEFT.scope(Arc::clone(&left), call));
    let finished = tokio::select! {
        biased;
        result = &mut call => Some(result),
        _ = left.abandoned.notified() => {
            writer.shutdown().await; // the shim's waiting thread reads the end and stops
            None
        }
        _ = reader.gone() => None,
    };
    drop(named);
    match finished {
        Some(result) if !left.is_set() => Some(result),
        Some(_) => None,
        None => {
            left.set();
            let _ = call.await;
            None
        }
    }
}

/// One running shim call in `Inner::shim_calls`, for as long as `Server::abandon` may name it.
struct Named<'a> {
    inner: &'a Inner,
    key: Option<(String, String)>,
    left: &'a Arc<Left>,
}

impl Named<'_> {
    fn add(&self) {
        if let Some(key) = &self.key {
            let mut calls = self
                .inner
                .shim_calls
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            calls
                .entry(key.clone())
                .or_default()
                .push(Arc::clone(self.left));
        }
    }
}

impl Drop for Named<'_> {
    fn drop(&mut self) {
        if let Some(key) = &self.key {
            let mut calls = self
                .inner
                .shim_calls
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            if let Some(running) = calls.get_mut(key) {
                running.retain(|other| !Arc::ptr_eq(other, self.left));
                if running.is_empty() {
                    calls.remove(key);
                }
            }
        }
    }
}

/// Send notifications until the stream ends or the client disconnects (server.py:408-436). A client that sends
/// anything while its stream runs ends the stream too, without a last frame, exactly as its leaving does.
async fn stream_call(
    inner: &Inner,
    handler: Arc<dyn Stream>,
    session: &Session,
    req: &Request,
    reader: &mut FrameReader,
    writer: &mut FrameWriter,
) -> Flow {
    let failed = |error: &HandlerError| {
        if error.failure == Failure::Fault {
            let hook = read(&inner.on_internal);
            tell(|| hook(&req.method, error));
        }
        wire::error_line(
            &req.id,
            wire::FAILED,
            &format!("{}: {}", error.type_name, error.message),
        )
    };
    let (opening_session, params) = (session.clone(), req.params.clone());
    let mut events = match catch_unwind(AssertUnwindSafe(|| handler.open(opening_session, params)))
    {
        Ok(events) => events,
        Err(_) => {
            let error = HandlerError::fault("Panic", "the handler panicked");
            return flow(writer.send(&failed(&error)).await);
        }
    };
    loop {
        let next = tokio::select! {
            biased;
            next = PanicToFault(events.next()) => next,
            _ = reader.gone() => return Flow::Serve, // `events` is dropped: the stream is closed
        };
        match next {
            Some(Ok(item)) => {
                if writer
                    .send(&wire::event_line(&req.method, &item))
                    .await
                    .is_err()
                {
                    // writing to this client failed: it left (server.py:416-417, 429)
                    return Flow::Serve;
                }
            }
            Some(Err(error)) => return flow(writer.send(&failed(&error)).await),
            None => return flow(writer.send(&wire::ended_line(&req.id)).await),
        }
    }
}

/// `EventStream::next` with a panic turned into a fault.
struct PanicToFault<'a>(BoxFuture<'a, Option<HandlerResult>>);

impl Future for PanicToFault<'_> {
    type Output = Option<HandlerResult>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        match catch_unwind(AssertUnwindSafe(|| self.0.as_mut().poll(cx))) {
            Ok(polled) => polled,
            Err(_) => Poll::Ready(Some(Err(HandlerError::fault(
                "Panic",
                "the handler panicked",
            )))),
        }
    }
}

/// The daemon's `ping` (`ferb/daemon/areas/rpc.py:190-192`): `{pid, role, now, stopped}` in that order. `now`
/// is the clock's local time in ISO form and `stopped` the global stop; both belong to other ports, so they are
/// asked here.
pub fn ping(
    now: impl Fn() -> String + Send + Sync + 'static,
    stopped: impl Fn() -> bool + Send + Sync + 'static,
) -> impl Unary {
    move |session: Session, _params: Params| {
        let mut result = Params::new();
        result.insert("pid".to_owned(), std::process::id().into());
        result.insert("role".to_owned(), session.role.as_str().into());
        result.insert("now".to_owned(), now().into());
        result.insert("stopped".to_owned(), stopped().into());
        std::future::ready(Ok(result))
    }
}
