//! The gateway (`ferb/daemon/gateway.py`): the one path every model tool call takes, on the Mac's store.
//!
//! One pipeline per call, in Python's order (the stage order is the contract): admission (`_admit`) → the call built
//! (`_build`: the agent's notes taken out, arguments validated, precheck, `unavailable`, owner.ask's own path, the
//! capability's description, the worker's declaration that only adds, the tool's facts, P19's identity, the locks)
//! → boundaries that need nobody (`_boundaries`) → every reason to ask (`_needs`) → the one step that finds the
//! owner's earlier answers and asks once for the rest (`_authorize`) → GATE-580's press re-check → the grants →
//! the ledger row before the effect → the call under its locks → its outcome, redacted once → the reply.
//!
//! In memory, as in Python, and lost with the process: pending questions, the routine 30-minute window, payment
//! approvals and their changes, `_undelivered`, `_amount_asked`, `_terminal_arms`, the lookups' counts and answers,
//! the tree locks and what was read per tree. Durable: spans, confirmations, known destinations, standing
//! permissions and the ledger.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::future::Future;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use ferb_core::interfaces::{Clock, IdSource};
use ferb_core::types::{
    ActionFacts, ActionSpec, EscapeGrant, Identity, PathCheck, Target, TerminalGrant,
    TerminalInput, ToolContext,
};
use ferb_core::when::{self, DateTime};
use ferb_policy::confirm::{self, Need, Values};
use ferb_policy::payments::{self, PayFacts, Payments};
use ferb_policy::protection::{Protection, FERB_APP, FERB_SERVICE, FERB_STATE, NEVER_LIFTED};
use ferb_policy::provenance::{self, ArgSources, Text};
use ferb_policy::rules::{self, ActionRequest, Kind, REVERSIBLE};
use ferb_policy::small::{Request, Requests};
use ferb_policy::stores::{Confirmation, Confirmations, KnownDestinations, StandingPermissions};
use ferb_policy::{carried, destinations as dest};
use ferb_rpc::auth::RunGrant;
use ferb_sanitize::origin;
use ferb_sanitize::terminal::printable;
use ferb_sanitize::{Py, PyStr, Redactor};
use ferb_store::spans::{tree_where, Finish, PayloadOutput, Span, SpanStore};
use ferb_store::Db;
use serde_json::{json, Map, Value};
use tokio::sync::Notify;

use crate::admit;
use crate::args::{self, Destination};
use crate::listing::{acts, identity as grant_identity, observes};
use crate::pay_picture::{PayPictures, NOT_CAPTURED};
use crate::questions::{card_flags, card_parts, Approval, Approvals, Asker, ResolveError, State};
use crate::reply::{self, Reply};
use crate::seams::{
    CallHooks, Describe, DuplicateUnresolved, Ledger, NoObserver, Observer, QuestionRouter,
    Recovery, Snapshots, Usage,
};
use crate::tool::{Args, Registry, Tool, ToolError, ToolOutput};
use crate::words::*;

mod hidden;
mod own;
pub use hidden::{HiddenError, SecretAsk};
pub use own::{OwnEvent, OwnQuestion};

// ---------------------------------------------------------------------------------------------------------------
// FERB's own words, marked (Python's `Redacted`)
// ---------------------------------------------------------------------------------------------------------------

/// Strings FERB's code built from already-redacted values, or the redactor already returned (Python's `Redacted`,
/// `redact.py:50`; GATE-572): a sink that redacts (the span store, the daemon's events) keeps them as they are.
/// Python marks the string object; here a sink is told which texts are marked, and a string of the same text in
/// the same record is kept too (it is the redactor's own output, or FERB's own line, either way).
#[derive(Clone, Debug, Default)]
pub struct Marks(HashSet<String>);

impl Marks {
    pub fn of(texts: &[&str]) -> Marks {
        let mut m = Marks::default();
        for t in texts {
            m.add(t);
        }
        m
    }

    /// Every string in `v` (the redactor's whole output).
    pub fn of_value(v: &Value) -> Marks {
        let mut m = Marks::default();
        m.add_value(v);
        m
    }

    pub fn add(&mut self, text: &str) {
        if !text.is_empty() {
            self.0.insert(text.to_string());
        }
    }

    pub fn add_value(&mut self, v: &Value) {
        match v {
            Value::String(s) => self.add(s),
            Value::Array(a) => a.iter().for_each(|x| self.add_value(x)),
            Value::Object(o) => o.values().for_each(|x| self.add_value(x)),
            _ => {}
        }
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    fn contains(&self, text: &str) -> bool {
        self.0.contains(text)
    }
}

/// `v` as a Python value, each string `marked` names carrying `template`'s mark.
fn mark_py(v: &Value, marked: &Marks, template: &PyStr) -> Py {
    match v {
        Value::String(s) if marked.contains(s) => {
            let mut p = template.clone();
            p.text = s.clone();
            Py::Str(p)
        }
        Value::Array(a) => Py::List(a.iter().map(|x| mark_py(x, marked, template)).collect()),
        Value::Object(o) => Py::Dict(
            o.iter()
                .map(|(k, x)| (PyStr::new(k.as_str()), mark_py(x, marked, template)))
                .collect(),
        ),
        other => Py::from(other),
    }
}

/// A Python value back to JSON, as `json.loads(json.dumps(v))` reads it.
pub fn py_value(p: &Py) -> Value {
    p.to_json(false)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or(Value::Null)
}

// ---------------------------------------------------------------------------------------------------------------
// errors and cancellation
// ---------------------------------------------------------------------------------------------------------------

/// Why a call returned no reply: it was cancelled (a stop, a pause, its caller gave up: Python's
/// `CancelledError`), or something below the gateway failed (Python raises out of `call`; the RPC answers an
/// internal error).
#[derive(Debug, Clone, PartialEq)]
pub enum CallError {
    Cancelled,
    Internal(String),
}

impl From<ferb_store::Error> for CallError {
    fn from(e: ferb_store::Error) -> CallError {
        CallError::Internal(format!("store: {e}"))
    }
}

impl From<rusqlite::Error> for CallError {
    fn from(e: rusqlite::Error) -> CallError {
        CallError::Internal(format!("store: {e}"))
    }
}

type R<T> = Result<T, CallError>;

/// A call's cancellation (Python cancels the call's asyncio task): `cancel_tasks`, `interrupt_tasks` and a caller
/// that gives up trigger it; the call sees it at its next wait.
#[derive(Clone, Default)]
pub struct Cancel(Arc<(AtomicBool, Notify)>);

impl Cancel {
    pub fn new() -> Cancel {
        Cancel::default()
    }

    pub fn cancel(&self) {
        self.0 .0.store(true, Ordering::SeqCst);
        self.0 .1.notify_waiters();
    }

    pub fn is_cancelled(&self) -> bool {
        self.0 .0.load(Ordering::SeqCst)
    }

    async fn cancelled(&self) {
        loop {
            let n = self.0 .1.notified();
            tokio::pin!(n);
            n.as_mut().enable();
            if self.is_cancelled() {
                return;
            }
            n.await;
        }
    }

    /// `fut`, unless the call is cancelled first (or already).
    pub async fn guard<T>(&self, fut: impl Future<Output = T>) -> R<T> {
        if self.is_cancelled() {
            return Err(CallError::Cancelled);
        }
        tokio::select! {
            biased;
            () = self.cancelled() => Err(CallError::Cancelled),
            v = fut => Ok(v),
        }
    }
}

/// A tool's run with its call-facts record open on every poll (`call_facts.recording()`); the record stays with
/// the caller, so what was noted survives the run being dropped by a cancel (`_measured`'s `finally`).
struct Recorded<F> {
    record: ferb_core::call_facts::Record,
    run: F,
}

impl<F: Future + Unpin> Future for Recorded<F> {
    type Output = F::Output;

    fn poll(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<F::Output> {
        let this = &mut *self;
        let record = this.record.clone();
        record.enter(|| std::pin::Pin::new(&mut this.run).poll(cx))
    }
}

// ---------------------------------------------------------------------------------------------------------------
// what the gateway is given
// ---------------------------------------------------------------------------------------------------------------

/// The task a run token works for (`TaskRecord`): what the gateway reads of it.
#[derive(Debug, Clone)]
pub struct TaskRecord {
    pub task_id: String,
    pub request_id: String,
    pub started_at: DateTime,
    pub deadline: Option<DateTime>,
    /// The task's root span: tool spans hang under it.
    pub span_id: String,
    /// The request's root task (None: the task is its own tree).
    pub tree_id: Option<String>,
}

/// FERB's file roots (`FsRoots`) and the protection lists (rule 10).
pub struct FsRoots {
    pub workspace: String,
    pub owner_roots: Vec<String>,
    pub untrusted_roots: Vec<String>,
    /// The owner's home folder (`Path.home()`).
    pub home: String,
    pub protection: Arc<SyncProtection>,
}

/// The protection lists, shared across threads.
pub struct SyncProtection(pub Protection<'static>);

// SAFETY: `Protection<'static>` holds only owned data and a `&'static dyn Files`; the gateway builds it with the
// stateless `PosixFiles` (each call goes straight to libc), which is safe to share between threads. Nothing else
// in it has interior mutability.
unsafe impl Send for SyncProtection {}
// SAFETY: as above: read-only data and a stateless file-call table.
unsafe impl Sync for SyncProtection {}

/// The stateless file calls the protection lists use.
pub static POSIX_FILES: ferb_policy::ppath::PosixFiles = ferb_policy::ppath::PosixFiles;

/// One id counter for every store (Python's `SequentialIds` / `UUIDs`, shared by spans, ledger and confirmations).
pub trait Ids: IdSource {}
impl<T: IdSource> Ids for T {}

/// Where a journaled problem goes: (kind, data).
pub type Sink = Box<dyn Fn(&str, Map<String, Value>) + Send + Sync>;
/// Where an event goes: (topic, data). Strings FERB's code already redacted or built carry the redactor's mark
/// (Python's `Redacted`, GATE-572), so the daemon's one redaction of the event keeps them as they are.
pub type EventSink = Box<dyn Fn(&str, Py) + Send + Sync>;
/// A profile's name or account by (browser, folder).
pub type Names = Box<dyn Fn(&str, &str) -> String + Send + Sync>;
/// Trail attribution of a path FERB touched: (path, task).
pub type PathNote = Box<dyn Fn(&str, &str) + Send + Sync>;
/// The task of a shell process while it runs.
pub type ProcTask = Box<dyn Fn(&str) -> Option<String> + Send + Sync>;
/// GATE-539: whether a tree made a path: (root, real path).
pub type MadeHere = Box<dyn Fn(&str, &str) -> bool + Send + Sync>;
/// The owner's exclusion list for hosts.
pub type HostTest = Box<dyn Fn(&str) -> bool + Send + Sync>;

/// Everything the gateway uses from the rest of FERB (`GatewayDeps`): every safety store is required.
pub struct Deps {
    pub db: Arc<Mutex<Db>>,
    pub clock: Arc<dyn Clock>,
    pub ids: Arc<dyn IdSource>,
    pub redactor: Arc<Redactor>,
    pub ledger: Arc<dyn Ledger>,
    pub publish: EventSink,
    pub roots: FsRoots,
    pub device_id: String,
    pub recovery: Arc<dyn Recovery>,
    pub snapshots: Option<Arc<dyn Snapshots>>,
    pub usage: Arc<dyn Usage>,
    pub apps: Arc<dyn confirm::Apps + Send + Sync>,
    pub describe: Arc<dyn Describe>,
    pub journal_problem: Sink,
    pub profile_name: Names,
    pub profile_account: Names,
    pub owner_contacts: Box<dyn Fn() -> Vec<String> + Send + Sync>,
    pub pay_same_site_forms: Box<dyn Fn() -> bool + Send + Sync>,
    pub on_path: PathNote,
    pub proc_task: ProcTask,
    pub route_question: Option<Arc<dyn QuestionRouter>>,
    pub hooks: Arc<dyn CallHooks>,
    /// GATE-539 (L39's change records): whether this task tree made `path` (None: no change records).
    pub made_in_tree: Option<MadeHere>,
    /// The owner's exclusion list (`Redactor.excluded_host` by default).
    pub excluded_host: Option<HostTest>,
    /// The owner's saved secrets (`OwnerSecrets(protection.ferb_home)` on the signed helper); None: a hidden input
    /// is refused ("secret storage protection is unavailable", as Python without protection lists).
    pub owner_secrets: Option<Arc<ferb_identity::owner_secrets::OwnerSecrets>>,
    /// Which apps are web browsers and how a browser reads an address (`is_browser`, `urls.parse`): a secret is
    /// never bound to a whole browser (D12).
    pub places: Arc<dyn ferb_identity::owner_secrets::Places>,
    /// L36 §6: the memory sources a saved username may still come from (`username_sources()`); None: any.
    pub username_holds: Option<HostTest>,
}

/// A file read in chunks, joined: (first rowid, labels, prepared text).
type Joined = (i64, BTreeSet<String>, Text);
/// A field's refusals by (place, key).
type RefusalAt = ((String, String), Refusal);
/// What S1 does before a call: (snapshot facts, the copy's record, the change record's counts).
type Before = (Map<String, Value>, Option<String>, Map<String, Value>);

/// `OutsideTexts`: what a tree read, prepared once.
#[derive(Default)]
struct OutsideTexts {
    floor: i64,
    seen: HashSet<i64>,
    texts: Vec<(String, Text)>,
    payload_floor: i64,
    payload_seen: HashSet<i64>,
    payloads: BTreeMap<i64, (BTreeSet<String>, Text)>,
    file_chunks: Vec<(String, BTreeMap<i64, PayloadOutput>)>,
    joined_files: Vec<(String, Vec<Joined>)>,
    pasted: Option<Vec<(String, Text)>>,
    owners: BTreeMap<i64, Vec<(BTreeSet<String>, Text)>>,
    pages: HashMap<i64, String>,
    said: BTreeMap<i64, String>,
}

#[derive(Default)]
struct Inner {
    tasks: HashMap<String, TaskRecord>,
    requests: Requests,
    approvals: Approvals,
    stopped: bool,
    cancelled: HashSet<String>,
    in_flight: Vec<(u64, String, Cancel, Left)>,
    waiting: HashMap<String, usize>,
    undelivered: HashMap<(String, String, String), (String, Reply)>,
    calls_made: HashMap<String, u64>,
    refused_over: HashMap<String, u64>,
    lookup_answers: HashMap<(String, String, String), Reply>,
    last_act: HashMap<String, (String, Option<Map<String, Value>>)>,
    allow_until: Option<DateTime>,
    terminal_arms: HashMap<String, String>,
    amount_asked: HashMap<String, HashSet<(String, Option<String>)>>,
    outside: HashMap<String, OutsideTexts>,
    pictures: PayPictures,
    task_folders: HashMap<String, Vec<String>>,
    confirming: HashMap<String, Arc<tokio::sync::Mutex<()>>>,
    /// GATE-562: per tree, each field's refusals by (place, key).
    refusals: HashMap<String, Vec<RefusalAt>>,
}

/// `refused_fills.Refusal`: one field's refusals in a task tree.
#[derive(Debug, Clone, Default)]
pub struct Refusal {
    pub what: String,
    pub place: String,
    pub count: u64,
    pub asked_after: u64,
    pub words: String,
    judged: BTreeSet<String>,
}

/// `refused_fills.SEALED_BROWSER`.
const SEALED_BROWSER: [&str; 4] = [
    "browser.secret",
    "ferb_browser.secret",
    "browser.login_code",
    "ferb_browser.login_code",
];
/// `ASKS_AFTER_REFUSAL` (owner GATE-563: "Twice").
pub const ASKS_AFTER_REFUSAL: u64 = 2;
/// `WORDS_SHOWN`.
const WORDS_SHOWN: usize = 300;

/// `field_key(tool, kind)`.
pub fn field_key(tool: &str, kind: Option<&str>) -> String {
    if tool.ends_with("login_code") {
        "login_code".into()
    } else {
        format!("secret:{}", kind.unwrap_or(""))
    }
}

/// `what_of(key)`.
pub fn what_of(key: &str) -> &'static str {
    let kind = key.split_once(':').map(|(_, k)| k).unwrap_or("");
    if key == "login_code" || kind == "one_time_code" {
        "login code"
    } else if kind == "password" || kind == "new_password" {
        "password"
    } else if kind.starts_with("card_") {
        "card detail"
    } else {
        "secret"
    }
}

/// `_page(url)`: (host lowercased, path without its last "/").
fn page_of(url: &str) -> (String, String) {
    match ferb_policy::urls::parse(url) {
        Some(p) if !p.host.is_empty() => {
            let path = if p.path.is_empty() {
                "/".to_string()
            } else {
                p.path.clone()
            };
            let trimmed = path.trim_end_matches('/');
            (
                p.host.to_lowercase(),
                if trimmed.is_empty() {
                    "/".into()
                } else {
                    trimmed.to_string()
                },
            )
        }
        _ => (String::new(), String::new()),
    }
}

/// `Answer` (`gateway.py:221`): the owner's answer to FERB's own question.
#[derive(Debug, Clone, Default)]
pub struct Answer {
    pub granted: bool,
    pub ids: Vec<String>,
    pub error: String,
    pub next: String,
    pub declined: String,
}

/// `Covered` (`gateway.py:236`): how a call stands with the owner.
#[derive(Debug, Clone, Default)]
struct Covered {
    ids: Vec<String>,
    rows: Vec<(String, String)>,
    spent: Vec<String>,
    refused: Option<Map<String, Value>>,
    standing: Vec<String>,
    started: bool,
    declined: String,
    wide: Vec<String>,
}

impl Covered {
    fn row(&self, key: &str) -> Option<&str> {
        self.rows
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }

    fn set_row(&mut self, key: &str, cid: &str) {
        match self.rows.iter_mut().find(|(k, _)| k == key) {
            Some(r) => r.1 = cid.to_string(),
            None => self.rows.push((key.to_string(), cid.to_string())),
        }
    }
}

/// P19's identity of a call: (ledger key, payload hash), the refusal of a path no file can have, or none.
#[derive(Debug, Clone)]
enum Ident {
    None,
    Refused(String),
    Key(String, String),
}

/// One file the call names, with the protection rules that close it: (path, access, rules).
type Found = Vec<(String, String, Vec<String>)>;

/// `Call` (`gateway.py:568`): one call as the pipeline sees it.
struct Call {
    grant: RunGrant,
    rec: TaskRecord,
    tool: Arc<dyn Tool>,
    name: String,
    args: Args,
    arguments: Map<String, Value>,
    why: String,
    effect: String,
    spec: ActionSpec,
    checked: ActionSpec,
    action: ActionRequest,
    real: Option<String>,
    ident: Ident,
    lock_keys: Vec<String>,
    armed: Option<String>,
    fact_targets: Vec<(String, Target)>,
    extra: Map<String, Value>,
    lifts: Vec<((String, String), Need)>,
    found: Found,
    escape: Option<(String, String, String, String)>,
    escape_need: Option<Need>,
    terminal: Option<TerminalInput>,
    terminal_need: Option<Need>,
    pay: Option<PayFacts>,
    pay_cid: Option<String>,
    pay_key: Option<String>,
    pay_values: Values,
    cancel: Cancel,
    left: Left,
}

/// Whether the call's caller has gone (`server.caller_left()`).
pub type Left = Arc<dyn Fn() -> bool + Send + Sync>;

impl Call {
    fn root(&self) -> String {
        self.rec
            .tree_id
            .clone()
            .unwrap_or_else(|| self.grant.task_id.clone())
    }

    fn facts(&self) -> Option<&ActionFacts> {
        self.spec.facts.as_ref()
    }

    fn kinds(&self) -> BTreeSet<String> {
        let mut k: BTreeSet<String> = [self.spec.kind.clone()].into_iter().collect();
        k.extend(rules::declared_kinds(&self.checked));
        k.extend(self.fact_targets.iter().map(|(k, _)| k.clone()));
        k
    }

    fn consequential(&self) -> bool {
        !REVERSIBLE.contains(&self.spec.kind.as_str())
            || !rules::declared_kinds(&self.checked).is_empty()
            || !self.fact_targets.is_empty()
    }
}

fn obj(v: Value) -> Map<String, Value> {
    match v {
        Value::Object(m) => m,
        _ => Map::new(),
    }
}

fn set1(o: &str) -> BTreeSet<String> {
    [o.to_string()].into_iter().collect()
}

fn ferb_origin() -> BTreeSet<String> {
    set1(origin::FERB)
}

fn core_identity(grant: &RunGrant) -> Identity {
    match grant.identity {
        ferb_rpc::auth::Identity::Owner => Identity::Owner,
        ferb_rpc::auth::Identity::Ferb => Identity::Ferb,
    }
}

/// `ferb_store`'s clock and ids over the gateway's.
pub struct StoreTime(pub Arc<dyn Clock>);
impl ferb_store::spans::Clock for StoreTime {
    fn now(&self) -> ferb_py::time::DateTime {
        when::to_py(&self.0.now()).expect("the clock's time is representable")
    }
}
pub struct StoreIds(pub Arc<dyn IdSource>);
impl ferb_store::spans::IdSource for StoreIds {
    fn new_id(&self) -> String {
        self.0.new_id()
    }
}

static NEXT_JOB: AtomicU64 = AtomicU64::new(1);

/// The gateway.
pub struct Gateway {
    registry: Mutex<Registry>,
    d: Deps,
    inner: Mutex<Inner>,
    payments: Mutex<Payments>,
    answers: Notify,
    observer: Mutex<Arc<dyn Observer>>,
    locks: crate::locks::ResourceLocks,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Gateway {
    /// `Gateway(...)`: the registry's tools and everything in `deps`. The approvals' ids come from `question_ids`
    /// (Python: `"ap-" + secrets.token_hex(6)`).
    pub fn new(
        registry: Registry,
        deps: Deps,
        question_ids: impl FnMut() -> String + Send + Sync + 'static,
    ) -> Gateway {
        let clock = deps.clock.clone();
        Gateway {
            registry: Mutex::new(registry),
            payments: Mutex::new(Payments::new(Box::new(move || clock.now()))),
            d: deps,
            inner: Mutex::new(Inner {
                approvals: Approvals::new(question_ids),
                ..Inner::default()
            }),
            answers: Notify::new(),
            observer: Mutex::new(Arc::new(NoObserver)),
            locks: crate::locks::ResourceLocks::new(),
        }
    }

    /// `attach`: the orchestrator's observer and conversation guard, given once.
    pub fn attach(&self, observer: Arc<dyn Observer>) {
        *lock(&self.observer) = observer;
    }

    fn observer(&self) -> Arc<dyn Observer> {
        lock(&self.observer).clone()
    }

    fn hooks(&self) -> &dyn CallHooks {
        self.d.hooks.as_ref()
    }

    /// The resource locks every call takes (`self.locks`).
    pub fn locks(&self) -> &crate::locks::ResourceLocks {
        &self.locks
    }

    /// `locks.try_hold(keys, holder, during_reads)` (call_hooks.py:80, DMN-403/406): a bystander (the live preview)
    /// takes every key only when each is free and nobody waits for it, else none at once; never queued behind a
    /// call. Checked and taken under one lock of the lock table, so no call slips in between the look and the take.
    pub fn try_hold(
        &self,
        keys: &[String],
        holder: &str,
        during_reads: bool,
    ) -> Result<Option<crate::locks::Held>, String> {
        self.locks.try_hold(keys, holder, during_reads)
    }

    // ---------- the state the daemon and the orchestrator change ----------

    /// The registry (tools are added and removed as browsers attach).
    pub fn registry(&self) -> MutexGuard<'_, Registry> {
        lock(&self.registry)
    }

    pub fn add_task(&self, rec: TaskRecord) {
        lock(&self.inner).tasks.insert(rec.task_id.clone(), rec);
    }

    pub fn remove_task(&self, task_id: &str) {
        lock(&self.inner).tasks.remove(task_id);
    }

    pub fn record_request(&self, r: Request) {
        lock(&self.inner).requests.record(r);
    }

    /// The emergency stop's switch (`state.stopped`).
    pub fn set_stopped(&self, on: bool) {
        lock(&self.inner).stopped = on;
    }

    pub fn set_task_folders(&self, root: &str, folders: Vec<String>) {
        lock(&self.inner)
            .task_folders
            .insert(root.to_string(), folders);
    }

    /// A request nobody is at chat for (a scheduled run).
    pub fn set_unattended(&self, request_id: &str, on: bool) {
        let mut i = lock(&self.inner);
        if on {
            i.approvals.unattended.insert(request_id.to_string());
        } else {
            i.approvals.unattended.remove(request_id);
        }
    }

    /// The pending questions, as they stand.
    pub fn pending(&self) -> Vec<Approval> {
        lock(&self.inner)
            .approvals
            .pending()
            .into_iter()
            .cloned()
            .collect()
    }

    /// Every question still kept, oldest first (open, or answered and not yet forgotten).
    pub fn approvals(&self) -> Vec<Approval> {
        lock(&self.inner)
            .approvals
            .all()
            .into_iter()
            .cloned()
            .collect()
    }

    pub fn approval(&self, id: &str) -> Option<Approval> {
        lock(&self.inner).approvals.get(id).cloned()
    }

    /// The owner's answer (`Approvals.resolve`); the waiting call wakes.
    pub fn resolve(
        &self,
        id: &str,
        granted: bool,
        choice: Option<usize>,
        text: Option<&str>,
    ) -> Result<Approval, ResolveError> {
        // a hidden input's answer goes straight into a zeroizing secret its call takes (rule 7)
        let out = lock(&self.inner)
            .approvals
            .resolve_sealed(id, granted, choice, text)
            .cloned();
        self.answers.notify_waiters();
        out
    }

    /// `Approvals.supersede`: a correction set the question aside (not a No).
    pub fn supersede(&self, id: &str) -> Option<Approval> {
        let out = lock(&self.inner).approvals.supersede(id).cloned();
        self.answers.notify_waiters();
        out
    }

    /// `forget_request`: the request's facts and questions go (its task ended).
    pub fn forget_request(&self, request_id: &str) {
        let mut i = lock(&self.inner);
        i.requests.forget(request_id);
        i.approvals.forget_request(request_id);
        drop(i);
        self.answers.notify_waiters();
    }

    /// GATE-579: FERB.app's one fetch of a payment question's picture.
    pub fn take_picture(&self, approval_id: &str) -> Option<Vec<u8>> {
        lock(&self.inner).pictures.take(approval_id)
    }

    pub fn has_picture(&self, approval_id: &str) -> bool {
        lock(&self.inner).pictures.has(approval_id)
    }

    /// The owner's "don't ask for 30 min", if it runs now.
    pub fn allow_until(&self) -> Option<DateTime> {
        lock(&self.inner).allow_until
    }

    // ---------- the stores ----------

    fn spans<T>(&self, f: impl FnOnce(&SpanStore<'_>) -> T) -> T {
        self.spans_marked(Marks::default(), f)
    }

    /// The span store, its redaction keeping `marked` strings as they are (Python's `Redacted`, GATE-572).
    fn spans_marked<T>(&self, marked: Marks, f: impl FnOnce(&SpanStore<'_>) -> T) -> T {
        let db = lock(&self.d.db);
        let redactor = self.d.redactor.clone();
        let template = self.mark_str("");
        let store = SpanStore::new(
            &db,
            Box::new(StoreTime(self.d.clock.clone())),
            Box::new(StoreIds(self.d.ids.clone())),
            Box::new(move |v: &Value| {
                if marked.is_empty() {
                    redactor.value(v)
                } else {
                    py_value(&redactor.py(&mark_py(v, &marked, &template)))
                }
            }),
            Box::new(|kind: &str| REVERSIBLE.contains(&kind)),
        );
        f(&store)
    }

    fn confirmations<T>(&self, f: impl FnOnce(&Confirmations<'_>) -> T) -> T {
        let db = lock(&self.d.db);
        let c = Confirmations {
            db: &db,
            clock: self.d.clock.as_ref(),
            ids: self.d.ids.as_ref(),
            redact: self.d.redactor.as_ref(),
        };
        f(&c)
    }

    fn destinations<T>(&self, f: impl FnOnce(&KnownDestinations<'_>) -> T) -> T {
        let db = lock(&self.d.db);
        let k = KnownDestinations {
            db: &db,
            clock: self.d.clock.as_ref(),
            ids: self.d.ids.as_ref(),
            redact: self.d.redactor.as_ref(),
        };
        f(&k)
    }

    fn find(&self, root: &str, key: &str, reopened: &[String]) -> R<Option<Confirmation>> {
        Ok(self.confirmations(|c| c.find(root, key, reopened))?)
    }

    fn reopened(&self, root: &str) -> R<Vec<String>> {
        Ok(self
            .spans(|s| s.reopened_answers(root))?
            .into_iter()
            .collect())
    }

    fn redact(&self, v: &Value) -> Value {
        self.d.redactor.value(v)
    }

    fn publish(&self, topic: &str, data: Map<String, Value>) {
        self.publish_marked(topic, data, &Marks::default())
    }

    /// An event whose `marked` strings FERB's code built or already redacted (GATE-572): the daemon's one redaction
    /// of the event keeps them as they are, as Python's `Redacted` mark does.
    fn publish_marked(&self, topic: &str, data: Map<String, Value>, marked: &Marks) {
        (self.d.publish)(topic, self.py_marked(&Value::Object(data), marked))
    }

    /// A string carrying this redactor's mark now (`Redactor.marked()(text)`).
    fn mark_str(&self, text: &str) -> PyStr {
        let mut p = self.d.redactor.text_marked(&PyStr::new(""));
        p.text = text.to_string();
        p
    }

    /// `v` as Python holds it, each string in `marked` carrying the redactor's mark.
    fn py_marked(&self, v: &Value, marked: &Marks) -> Py {
        mark_py(v, marked, &self.mark_str(""))
    }

    /// `self.redact(v)` where `marked` strings are kept as they are.
    fn redact_marked(&self, v: &Value, marked: &Marks) -> Value {
        py_value(&self.d.redactor.py(&self.py_marked(v, marked)))
    }

    fn excluded(&self, host: &str) -> bool {
        match &self.d.excluded_host {
            Some(f) => f(host),
            None => self.d.redactor.excluded_host(host),
        }
    }

    fn now(&self) -> DateTime {
        self.d.clock.now()
    }

    fn task(&self, task_id: &str) -> Option<TaskRecord> {
        lock(&self.inner).tasks.get(task_id).cloned()
    }

    fn request(&self, request_id: &str) -> Request {
        lock(&self.inner).requests.get(request_id)
    }

    /// `tasks.tree_of`: the live record's tree, else the stored row's.
    fn tree_of(&self, task_id: &str) -> Option<String> {
        if let Some(r) = self.task(task_id) {
            return Some(r.tree_id.unwrap_or(r.task_id));
        }
        let db = lock(&self.d.db);
        db.conn()
            .query_row("SELECT tree_id FROM tasks WHERE id = ?", [task_id], |r| {
                r.get::<_, Option<String>>(0)
            })
            .ok()
            .flatten()
    }

    // ---------- replies ----------

    /// `_replier(rec)`'s reply.
    fn reply(
        &self,
        rec: Option<&TaskRecord>,
        content: Map<String, Value>,
        is_error: bool,
        origins: Option<&BTreeSet<String>>,
        redacted: bool,
    ) -> Reply {
        let now = self.now();
        reply::reply(
            &self.d.redactor,
            &now,
            &reply::zone_name(&now),
            rec.map(|r| &r.started_at),
            rec.and_then(|r| r.deadline.as_ref()),
            Value::Object(content),
            is_error,
            origins,
            redacted,
        )
    }

    // ---------- the call ----------

    /// `call(grant, name, arguments)`: one tool call, start to end.
    pub async fn call(
        &self,
        grant: &RunGrant,
        name: &str,
        arguments: &Map<String, Value>,
    ) -> R<Reply> {
        self.call_as(grant, name, arguments, Cancel::new(), Arc::new(|| false))
            .await
    }

    /// `call`, with the caller's own cancellation (the runtime gave up the call) and whether its caller has gone
    /// (`server.caller_left()`).
    pub async fn call_as(
        &self,
        grant: &RunGrant,
        name: &str,
        arguments: &Map<String, Value>,
        cancel: Cancel,
        caller_left: Left,
    ) -> R<Reply> {
        // ORCH-557: a planner lookup repeated with the same arguments gets its earlier answer back
        let same = (grant.max_calls > 0).then(|| {
            (
                grant.worker_id.clone(),
                name.to_string(),
                sorted_dumps(&Value::Object(arguments.clone())),
            )
        });
        if let Some(key) = &same {
            let mut i = lock(&self.inner);
            let made = i.calls_made.get(&grant.worker_id).copied().unwrap_or(0);
            if made < grant.max_calls {
                if let Some(earlier) = i.lookup_answers.get(key).cloned() {
                    i.calls_made.insert(grant.worker_id.clone(), made + 1);
                    let mut content = Map::new();
                    content.insert("same_as_your_earlier_call".into(), Value::Bool(true));
                    content.insert("note".into(), Value::String(SAME_LOOKUP.into()));
                    content.insert("result".into(), earlier.content.clone());
                    return Ok(Reply {
                        content: Value::Object(content),
                        ..earlier
                    });
                }
            }
        }
        let job = NEXT_JOB.fetch_add(1, Ordering::SeqCst);
        lock(&self.inner).in_flight.push((
            job,
            grant.task_id.clone(),
            cancel.clone(),
            caller_left.clone(),
        ));
        let r = self
            .inner_call(grant, name, arguments, cancel, caller_left)
            .await;
        lock(&self.inner).in_flight.retain(|(j, _, _, _)| *j != job);
        let r = r?;
        let tool = self.registry().get(name).cloned();
        if let Some(t) = tool {
            if acts(t.as_ref()) {
                let failed = r.is_error.then(|| failed_act(name, &r.content));
                lock(&self.inner)
                    .last_act
                    .insert(grant.worker_id.clone(), (grant.task_id.clone(), failed));
            }
        }
        if let Some(key) = same {
            if !r.is_error {
                lock(&self.inner).lookup_answers.insert(key, r.clone());
            }
        }
        Ok(r)
    }

    /// `in_flight(task_ids)`.
    pub fn in_flight(&self, task_ids: &BTreeSet<String>) -> bool {
        lock(&self.inner)
            .in_flight
            .iter()
            .any(|(_, t, c, _)| task_ids.contains(t) && !c.is_cancelled())
    }

    /// `cancel_tasks(ids)`: a stop of these tasks: their calls in flight end and they may not call again. An empty
    /// list stops nothing (DMN-1: never a global stop).
    pub fn cancel_tasks(&self, task_ids: &BTreeSet<String>) {
        self.hooks().stopped(task_ids);
        let mut i = lock(&self.inner);
        i.cancelled.extend(task_ids.iter().cloned());
        for (_, t, c, _) in &i.in_flight {
            if task_ids.contains(t) {
                c.cancel();
            }
        }
    }

    /// `interrupt_tasks(ids)` (ORCH-308): their calls in flight end; they may call again once they resume.
    pub fn interrupt_tasks(&self, task_ids: &BTreeSet<String>) {
        let i = lock(&self.inner);
        for (_, t, c, _) in &i.in_flight {
            if task_ids.contains(t) {
                c.cancel();
            }
        }
    }

    /// `forget_tasks(ids)`: a task tree ended; what the gateway kept for it goes.
    pub fn forget_tasks(&self, task_ids: &BTreeSet<String>) {
        self.hooks().forget(task_ids);
        let ids: Vec<String> = task_ids.iter().cloned().collect();
        lock(&self.payments).forget(&ids);
        let mut i = lock(&self.inner);
        i.pictures.forget(&ids);
        for t in task_ids {
            i.cancelled.remove(t);
            i.outside.remove(t);
            i.confirming.remove(t);
            i.amount_asked.remove(t);
        }
        for t in task_ids {
            i.refusals.remove(t);
        }
        i.terminal_arms
            .retain(|k, _| !task_ids.contains(k.split('.').next().unwrap_or(k)));
        i.undelivered.retain(|k, _| !task_ids.contains(&k.0));
        i.last_act.retain(|_, (task, _)| !task_ids.contains(task));
    }

    /// `initial_image_given(task)` (L12, D26 clarified): the worker was handed the owner's front window at send.
    pub fn initial_image_given(&self, task_id: &str) {
        self.hooks().initial_image_given(task_id);
    }

    /// `failed_act(worker)` (ORCH-639): this run's latest acting call when it ended with an error; taken once.
    pub fn failed_act(&self, worker_id: &str) -> Option<Map<String, Value>> {
        lock(&self.inner)
            .last_act
            .remove(worker_id)
            .and_then(|(_, f)| f)
    }

    pub fn lookups_made(&self, worker_id: &str) -> u64 {
        lock(&self.inner)
            .calls_made
            .get(worker_id)
            .copied()
            .unwrap_or(0)
    }

    pub fn calls_refused(&self, worker_id: &str) -> u64 {
        lock(&self.inner)
            .refused_over
            .get(worker_id)
            .copied()
            .unwrap_or(0)
    }

    /// `forget_calls(worker)`: a run ended (its token revoked).
    pub fn forget_calls(&self, worker_id: &str) {
        let mut i = lock(&self.inner);
        i.calls_made.remove(worker_id);
        i.refused_over.remove(worker_id);
        i.last_act.remove(worker_id);
        i.lookup_answers.retain(|k, _| k.0 != worker_id);
    }

    async fn inner_call(
        &self,
        grant: &RunGrant,
        name: &str,
        arguments: &Map<String, Value>,
        cancel: Cancel,
        caller_left: Left,
    ) -> R<Reply> {
        let rec = self.task(&grant.task_id);
        let tool = self.registry().get(name).cloned();
        // _admit
        let (stopped, made) = {
            let i = lock(&self.inner);
            (
                i.stopped || i.cancelled.contains(&grant.task_id),
                i.calls_made.get(&grant.worker_id).copied().unwrap_or(0),
            )
        };
        if let Err(refusal) =
            admit::admit(grant, tool.as_deref(), name, stopped, rec.is_some(), made)
        {
            if let (Some(reason), Some(rec)) = (&refusal.span_reason, &rec) {
                self.refused_span(grant, rec, name, reason)?;
            }
            if refusal.over_cap {
                *lock(&self.inner)
                    .refused_over
                    .entry(grant.worker_id.clone())
                    .or_insert(0) += 1;
            }
            let o = ferb_origin();
            return Ok(self.reply(
                rec.as_ref(),
                refusal.content,
                true,
                refusal.ferb_origin.then_some(&o),
                false,
            ));
        }
        if grant.max_calls > 0 {
            *lock(&self.inner)
                .calls_made
                .entry(grant.worker_id.clone())
                .or_insert(0) += 1;
        }
        let (rec, tool) = (rec.expect("admitted"), tool.expect("admitted"));
        let mut c = match self
            .build(
                grant,
                &rec,
                tool,
                name,
                arguments,
                cancel.clone(),
                caller_left.clone(),
            )
            .await?
        {
            Ok(c) => c,
            Err(reply) => return Ok(reply),
        };
        if let Some(r) = self.boundaries(&mut c)? {
            return Ok(r);
        }
        let needs = match self.needs(&mut c).await? {
            Ok(n) => n,
            Err(r) => return Ok(r),
        };
        let covered = self.authorize(&mut c, needs.clone()).await;
        let covered = match covered {
            Ok(cv) => self.pay_pressed(&mut c, cv).await,
            Err(e) => Err(e),
        };
        let mut covered = covered?;
        let out = self.after_authorize(&mut c, &mut covered).await;
        if !covered.spent.is_empty() && !covered.started {
            for cid in &covered.spent {
                self.confirmations(|s| s.release(cid))?;
            }
        }
        out
    }

    async fn after_authorize(&self, c: &mut Call, covered: &mut Covered) -> R<Reply> {
        if c.consequential() {
            c.extra.insert("confirmations".into(), json!(covered.ids));
        }
        if !covered.standing.is_empty() && covered.refused.is_none() {
            c.extra.insert(
                "standing".into(),
                json!({"task": c.root(), "rules": covered.standing}),
            );
        }
        if let Some(refused) = covered.refused.clone() {
            if !covered.declined.is_empty() {
                c.extra
                    .insert("declined".into(), Value::String(covered.declined.clone()));
            }
            return self.refuse_call(c, refused, None);
        }
        let (lifted, escape, terminal) = match self.grants(c, covered).await? {
            Ok(g) => g,
            Err(refused) => return self.refuse_call(c, refused, None),
        };
        let enter = terminal.as_ref().is_some_and(|t| t.input.enter);
        let cid = terminal.as_ref().map(|t| t.confirmation_id.clone());
        let result = self.run(c, covered, lifted, escape, terminal).await?;
        if enter && !result.is_error {
            if let Some(cid) = cid {
                lock(&self.inner)
                    .terminal_arms
                    .insert(c.grant.task_id.clone(), cid);
            }
        }
        Ok(result)
    }

    /// `_refused`: a small tool.refused span (a tool that doesn't exist or isn't the worker's).
    fn refused_span(&self, grant: &RunGrant, rec: &TaskRecord, name: &str, reason: &str) -> R<()> {
        self.spans(|s| -> ferb_store::Result<()> {
            let mut span = s.start(
                "tool.refused",
                &json!({"tool": clip(name, 100), "worker_id": grant.worker_id}),
                Some(&grant.task_id),
                Some(&rec.span_id),
                Some(&self.d.device_id),
            )?;
            s.finish(
                &mut span,
                Finish {
                    error: Some(reason),
                    ..Finish::default()
                },
            )
        })?;
        Ok(())
    }
}

/// `json.dumps(arguments, sort_keys=True, default=str)`: the lookup's key.
fn sorted_dumps(v: &Value) -> String {
    ferb_py::json::dumps_with(
        v,
        ferb_py::json::Style {
            ascii: true,
            sort_keys: true,
            indent: None,
        },
    )
}

// ---------------------------------------------------------------------------------------------------------------
// _build and _boundaries
// ---------------------------------------------------------------------------------------------------------------

/// `hashed_args(cap, args)`: the validated arguments as P19's hash binds them.
pub fn hashed_args(tool: &dyn Tool, args: &Args) -> Args {
    let f = tool.facts();
    let dropped: HashSet<&String> = f.target_args.iter().chain(f.method_args.iter()).collect();
    let hashed: Args = args
        .iter()
        .filter(|(k, _)| !dropped.contains(k))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    tool.ledger_args(args, &hashed).unwrap_or(hashed)
}

impl Gateway {
    #[allow(clippy::too_many_arguments)]
    async fn build(
        &self,
        grant: &RunGrant,
        rec: &TaskRecord,
        tool: Arc<dyn Tool>,
        name: &str,
        arguments: &Map<String, Value>,
        cancel: Cancel,
        left: Left,
    ) -> R<Result<Call, Reply>> {
        let armed = if observes(tool.as_ref()) {
            None
        } else {
            lock(&self.inner).terminal_arms.remove(&grant.task_id)
        };
        let checked = match args::check(tool.as_ref(), arguments) {
            Ok(c) => c,
            Err(content) => return Ok(Err(self.reply(Some(rec), content, true, None, false))),
        };
        let o = ferb_origin();
        if let Some(refusal) = tool.precheck(&checked.args) {
            self.refused_span(grant, rec, name, &refusal)?;
            return Ok(Err(self.reply(
                Some(rec),
                obj(json!({"error": refusal})),
                true,
                Some(&o),
                false,
            )));
        }
        if let Some(fact) = cancel.guard(tool.unavailable(&checked.args)).await? {
            let why = fact
                .get("detail")
                .filter(|v| ferb_py::value::truthy(v))
                .or_else(|| fact.get("code").filter(|v| ferb_py::value::truthy(v)))
                .map(ferb_py::value::py_str)
                .unwrap_or_else(|| "unavailable".into());
            self.refused_span(grant, rec, name, &why)?;
            return Ok(Err(self.reply(Some(rec), fact, true, Some(&o), false)));
        }
        if name == OWNER_ASK {
            let answer = self
                .ask_owner(grant, rec, &checked.args, &cancel, &left)
                .await?;
            let is_error = answer.get("error").is_some_and(ferb_py::value::truthy);
            let origins = ask_origins(&answer);
            return Ok(Err(self.reply(
                Some(rec),
                answer,
                is_error,
                Some(&origins),
                false,
            )));
        }
        // a `describe` that raises leaves `Gateway.call` (Python's exception; the RPC answers an internal error)
        let mut spec = tool
            .try_describe(&checked.args)
            .map_err(|e| CallError::Internal(format!("{}: {}", e.class, e.message)))?;
        if let Some(p) = spec.target.path.clone() {
            spec.target.path = Some(
                ferb_fs::files::normalize_path(&p, &self.d.roots.workspace, &self.d.roots.home)
                    .unwrap_or(p),
            );
        }
        let reaches: &Destination = &checked.reaches;
        let declared = rules::declare(
            &spec,
            &checked.effect,
            &reaches.recipients,
            reaches.account.as_deref(),
            reaches.amount,
            reaches.item.as_deref(),
            reaches.currency.as_deref(),
        );
        let real = self.real_path(tool.as_ref(), &checked.args, &spec);
        let action = ActionRequest {
            capability: name.to_string(),
            kind: spec.kind.clone(),
            identity: core_identity(grant),
            scopes: tool.scopes(),
            task_id: grant.task_id.clone(),
            worker_id: grant.worker_id.clone(),
            request_id: rec.request_id.clone(),
            task_scopes: grant.scopes.clone(),
            target: spec.target.clone(),
            content: spec.content.clone(),
            effect: checked.effect.clone(),
            why: checked.why.clone(),
        };
        let ident = if REVERSIBLE.contains(&spec.kind.as_str()) {
            Ident::None
        } else {
            self.identity(tool.as_ref(), name, &action, &checked.args, real.as_deref())
        };
        let mut lock_keys = tool.locks(&checked.args);
        if name.starts_with("fs.") {
            if let Some(r) = &real {
                lock_keys = vec![format!("file:{r}")];
            }
        }
        let mut c = Call {
            grant: grant.clone(),
            rec: rec.clone(),
            tool: tool.clone(),
            name: name.to_string(),
            args: checked.args.clone(),
            arguments: checked.arguments.clone(),
            why: checked.why.clone(),
            effect: checked.effect.clone(),
            spec,
            checked: declared,
            action,
            real,
            ident,
            lock_keys,
            armed,
            fact_targets: Vec::new(),
            extra: Map::new(),
            lifts: Vec::new(),
            found: Vec::new(),
            escape: None,
            escape_need: None,
            terminal: None,
            terminal_need: None,
            pay: None,
            pay_cid: None,
            pay_key: None,
            pay_values: Values::new(),
            cancel,
            left,
        };
        c.fact_targets = self.fact_targets(&c);
        let declared = rules::declared_kinds(&c.checked);
        if !declared.is_empty() {
            let v = if declared.len() == 1 {
                Value::String(declared.iter().next().cloned().unwrap_or_default())
            } else {
                json!(declared.iter().collect::<Vec<_>>())
            };
            c.extra.insert("declared".into(), v);
        }
        if checked.noted.is_some() {
            c.extra.insert(
                "destination".into(),
                Value::Object(checked.reaches.dump_set()),
            );
        }
        if let Some(f) = c.facts() {
            let recorded = f.recorded();
            if !recorded.is_empty() {
                c.extra.insert("facts".into(), Value::Object(recorded));
            }
        }
        if !c.fact_targets.is_empty() {
            let kinds: BTreeSet<&String> = c.fact_targets.iter().map(|(k, _)| k).collect();
            c.extra.insert("fact_kinds".into(), json!(kinds));
        }
        if SEALED_FILLS.contains(&name)
            && c.facts()
                .is_some_and(|f| f.field_kinds.iter().any(|k| k.as_str() == "card_number"))
        {
            let entry = c
                .arguments
                .get("secret")
                .filter(|v| ferb_py::value::truthy(v))
                .or_else(|| c.arguments.get("remember"));
            if let Some(Value::String(e)) = entry {
                let e = ferb_py::value::strip(e);
                if !e.is_empty() {
                    c.extra
                        .insert("card_entry".into(), Value::String(e.to_string()));
                }
            }
        }
        Ok(Ok(c))
    }

    /// `_real_path`: the file a call really acts on (symlinks followed, then `..`); None when it can't be told.
    fn real_path(&self, tool: &dyn Tool, args: &Args, spec: &ActionSpec) -> Option<String> {
        spec.target.path.as_ref().filter(|p| !p.is_empty())?;
        if let Some(r) = tool.real_path(args, &self.d.roots.workspace) {
            return r.ok().map(|p| p.to_string_lossy().into_owned());
        }
        let names = tool.facts().target_args;
        let key = if names.iter().any(|n| n == "path") {
            "path".to_string()
        } else {
            names.first()?.clone()
        };
        let given = args.get(&key)?.as_str()?;
        ferb_fs::files::real_path(given, &self.d.roots.workspace, &self.d.roots.home).ok()
    }

    /// `_identity` / `ledger_identity`: a consequential call's (ledger key, payload hash).
    fn identity(
        &self,
        tool: &dyn Tool,
        name: &str,
        action: &ActionRequest,
        args: &Args,
        real: Option<&str>,
    ) -> Ident {
        let f = tool.facts();
        let key = f.ledger_key.clone().unwrap_or_else(|| name.to_string());
        let mut target = action.target.clone();
        for field in &f.noted_targets {
            match field.as_str() {
                "contact" => target.contact = None,
                "account" => target.account = None,
                "site" => target.site = None,
                "path" => target.path = None,
                "page" => target.page = None,
                "item" => target.item = None,
                "amount" => target.amount = None,
                "currency" => target.currency = None,
                "recipients" => target.recipients = Vec::new(),
                _ => {}
            }
        }
        if let Some(p) = target.path.clone().filter(|p| !p.is_empty()) {
            let path = match real {
                Some(r) => Ok(r.to_string()),
                None => ferb_fs::pypath::resolve(&p).map_err(|_| ()),
            };
            match path {
                Ok(path) if path.contains('\0') => {
                    return Ident::Refused(
                        "that path can't name a file (it contains a NUL character); nothing ran"
                            .into(),
                    );
                }
                Ok(path) => target.path = Some(path),
                Err(()) => {}
            }
        }
        let a = ActionRequest {
            capability: key.clone(),
            target,
            ..action.clone()
        };
        Ident::Key(key, rules::payload_hash(&a, Some(&hashed_args(tool, args))))
    }

    /// `_fact_targets` (GATE-202): the kinds the tool's structural facts add, each with what it reaches.
    fn fact_targets(&self, c: &Call) -> Vec<(String, Target)> {
        let Some(f) = c.facts() else {
            return Vec::new();
        };
        let page = Some(host_of(f.page_url.as_deref())).filter(|h| !h.is_empty());
        let kinds: BTreeSet<&str> = f.field_kinds.iter().map(|k| k.as_str()).collect();
        let mut out = Vec::new();
        if f.submits_form == Some(true) {
            let card = kinds.iter().any(|k| ferb_identity_card_kind(k));
            if card || self.pays_on_site(f, page.as_deref()) {
                out.push((
                    Kind::PAY.to_string(),
                    Target {
                        site: page.clone(),
                        ..Target::default()
                    },
                ));
            }
            if kinds.contains("password") && kinds.contains("new_password") {
                out.push((
                    Kind::ACCOUNT_SECURITY.to_string(),
                    Target {
                        site: page.clone(),
                        ..Target::default()
                    },
                ));
            }
            let action_host = f
                .form_action_host
                .clone()
                .unwrap_or_default()
                .to_lowercase();
            let site = if action_host.is_empty() {
                page.clone()
            } else {
                Some(action_host)
            };
            out.push((
                Kind::SUBMIT.to_string(),
                Target {
                    site,
                    ..Target::default()
                },
            ));
        }
        if c.spec.kind != Kind::TRASH {
            for p in &f.deletes {
                let path =
                    ferb_fs::files::normalize_path(p, &self.d.roots.workspace, &self.d.roots.home)
                        .unwrap_or_else(|_| p.clone());
                out.push((
                    Kind::DELETE_PERMANENT.to_string(),
                    Target {
                        path: Some(path),
                        ..Target::default()
                    },
                ));
            }
        }
        out
    }

    /// `_pays_on_site` (GATE-582).
    fn pays_on_site(&self, f: &ActionFacts, page: Option<&str>) -> bool {
        let Some(page) = page else { return false };
        if f.form_method.as_deref().unwrap_or("").to_uppercase() != "POST"
            || !(self.d.pay_same_site_forms)()
        {
            return false;
        }
        let action = f
            .form_action_host
            .as_deref()
            .filter(|h| !h.is_empty())
            .unwrap_or(page);
        if confirm::registrable(action) != confirm::registrable(page) {
            return false;
        }
        f.page_amounts
            .iter()
            .any(|a| payments::amount_key(Some(a)).is_some())
    }

    fn boundaries(&self, c: &mut Call) -> R<Option<Reply>> {
        let o = ferb_origin();
        let mut guarded = c.spec.clone();
        guarded.effect_kind = c.checked.effect_kind.clone();
        if let Some(refusal) =
            self.observer()
                .conversation_guard(&c.grant, &c.name, c.tool.as_ref(), &guarded)
        {
            self.refused_span(&c.grant, &c.rec, &c.name, &refusal)?;
            return Ok(Some(self.reply(
                Some(&c.rec),
                obj(json!({"error": refusal})),
                true,
                Some(&o),
                false,
            )));
        }
        let (found, never) = self.file_rules(c.tool.as_ref(), &c.name, &c.args);
        if let Some((reason, error)) = never {
            self.refused_span(&c.grant, &c.rec, &c.name, &reason)?;
            return Ok(Some(self.reply(
                Some(&c.rec),
                obj(json!({"error": error})),
                true,
                Some(&o),
                false,
            )));
        }
        c.found = found;
        if self.card_typed(c) {
            self.refused_span(&c.grant, &c.rec, &c.name, "card details typed in the clear")?;
            return Ok(Some(self.reply(
                Some(&c.rec),
                obj(json!({"error": CARD_TYPED, "next": "Use browser.secret on the card field."})),
                true,
                Some(&o),
                false,
            )));
        }
        if (c.spec.kind == Kind::NAVIGATE || c.spec.kind == Kind::OPEN_TAB)
            && c.arguments.get("url").is_some_and(ferb_py::value::truthy)
            && dest::normalize("host", c.spec.target.site.as_deref().unwrap_or("")).is_empty()
        {
            return self
                .refuse_call(c, obj(json!({"error": NO_SITE, "next": "Use a plain http(s) address with a site name."})), None)
                .map(Some);
        }
        if (c.name == "shell.run" || c.name == "shell.start")
            && c.args.get("escape").is_some_and(ferb_py::value::truthy)
        {
            let rule = ferb_py::value::py_str(&c.args["escape"]);
            let command = c
                .args
                .get("command")
                .map(ferb_py::value::py_str)
                .unwrap_or_default();
            let protection = &self.d.roots.protection.0;
            if !protection.liftable(&rule) {
                let e = format!("{rule} is never lifted: FERB's own files and network protections remain enforced");
                return self
                    .refuse_call(c, obj(json!({"error": e})), None)
                    .map(Some);
            }
            let cwd_given = c
                .args
                .get("cwd")
                .filter(|v| ferb_py::value::truthy(v))
                .map(ferb_py::value::py_str)
                .unwrap_or_else(|| self.d.roots.workspace.clone());
            let cwd =
                ferb_fs::files::real_path(&cwd_given, &self.d.roots.workspace, &self.d.roots.home)
                    .unwrap_or(cwd_given);
            if !ferb_fs::pypath::is_dir(&cwd) {
                return self
                    .refuse_call(
                        c,
                        obj(json!({"error": "the escape command's working folder does not exist"})),
                        None,
                    )
                    .map(Some);
            }
            let kind = protection
                .rules()
                .into_iter()
                .find(|r| r["id"] == Value::String(rule.clone()))
                .and_then(|r| r["kind"].as_str().map(str::to_string))
                .unwrap_or_default();
            c.escape = Some((rule, command, cwd, kind));
        }
        if let Ident::Key(key, digest) = &c.ident {
            let earlier = lock(&self.inner).undelivered.remove(&(
                c.grant.task_id.clone(),
                key.clone(),
                digest.clone(),
            ));
            if let Some((intent_id, out)) = earlier {
                return self.earlier_result(c, &intent_id, out).map(Some);
            }
        }
        let repeat = match &c.ident {
            Ident::None => None,
            Ident::Refused(e) => Some(Err(e.clone())),
            Ident::Key(key, digest) => self.d.ledger.refusal(&c.grant.task_id, key, digest).map(Ok),
        };
        if let Some(repeat) = repeat {
            let mut span = self.call_span(c, &[("boundary_refused", Value::Bool(true))])?;
            return self
                .ledger_refused(&mut span, &c.name, &c.rec, repeat)
                .map(Some);
        }
        Ok(None)
    }

    /// `_card_typed` (GATE-206): text typed into a card field, or a payment card's number typed anywhere.
    fn card_typed(&self, c: &Call) -> bool {
        if c.spec.kind != Kind::FILL_FORM && c.spec.kind != Kind::MAC_INPUT {
            return false;
        }
        let Some(content) = &c.spec.content else {
            return false;
        };
        if let Some(f) = c.facts() {
            if f.field_kinds
                .iter()
                .any(|k| ferb_identity_card_kind(k.as_str()))
                && !ferb_py::value::strip(content).is_empty()
            {
                return true;
            }
        }
        matches!(self.d.redactor.hit(content), Ok(Some("payment card")))
    }

    /// `_file_rules` (rule 10, GATE-203): each declared file with the rules that close it, or the refusal.
    fn file_rules(
        &self,
        tool: &dyn Tool,
        name: &str,
        args: &Args,
    ) -> (Found, Option<(String, String)>) {
        let protection = &self.d.roots.protection.0;
        let Some(declared) = tool.file_paths(args) else {
            if tool.scopes().iter().any(|s| s.starts_with("fs:")) {
                return (
                    Vec::new(),
                    Some((
                        "no file_paths declared".into(),
                        fill(FS_UNDECLARED, &[("name", name)]),
                    )),
                );
            }
            return (Vec::new(), None);
        };
        let mut found = Vec::new();
        for (given, access) in declared {
            let path =
                ferb_fs::files::owner_path(&given, &self.d.roots.workspace, &self.d.roots.home)
                    .unwrap_or(given);
            let rules = protection.fs_rules(&path, &access);
            if rules
                .first()
                .is_some_and(|r| NEVER_LIFTED.contains(&r.as_str()))
            {
                return (Vec::new(), Some(self.closed(name, &path, &access, &rules)));
            }
            if !rules.is_empty() {
                found.push((path, access, rules));
            }
        }
        (found, None)
    }

    /// `_closed`: (the trace's reason, the worker's error) for a file the lists close.
    fn closed(&self, name: &str, path: &str, _access: &str, rules: &[String]) -> (String, String) {
        let protection = &self.d.roots.protection.0;
        let rule = rules[0].as_str();
        let short = ferb_fs::recovery::short(path, &self.d.roots.home);
        if rule == FERB_STATE {
            return (
                "FERB's own state".into(),
                fill(FS_FERB_STATE, &[("name", name), ("path", &short)]),
            );
        }
        if rule == FERB_APP {
            return (
                "FERB's own code".into(),
                fill(FS_FERB_APP, &[("name", name), ("path", &short)]),
            );
        }
        if rule == FERB_SERVICE {
            return (
                "FERB's own service".into(),
                fill(FS_FERB_SERVICE, &[("name", name), ("path", &short)]),
            );
        }
        let store = protection
            .entry(rule)
            .map(|e| e.shown.clone())
            .unwrap_or_else(|| rule.to_string());
        let text = if rule.starts_with("read:") {
            FS_NEVER_READ
        } else {
            FS_NEVER_WRITE
        };
        (
            format!("a protected folder ({rule})"),
            fill(text, &[("name", name), ("path", &short), ("store", &store)]),
        )
    }

    /// `_path_closed`: the check a tool runs on the file it really opened (rule 10).
    fn path_closed(&self, name: &str, lifted: &BTreeSet<(String, String)>) -> PathCheck {
        let protection = self.d.roots.protection.clone();
        let name = name.to_string();
        let lifted = lifted.clone();
        let home = self.d.roots.home.clone();
        PathCheck(Arc::new(move |path: &std::path::Path, access: &str| {
            let p = path.to_string_lossy();
            let rules = protection.0.fs_rules(&p, access);
            if rules.is_empty() {
                return None;
            }
            let rules: Vec<String> = if NEVER_LIFTED.contains(&rules[0].as_str()) {
                rules
            } else {
                let left: Vec<String> = rules
                    .into_iter()
                    .filter(|r| !lifted.contains(&(r.clone(), lift_how(access).to_string())))
                    .collect();
                if left.is_empty() {
                    return None;
                }
                left
            };
            Some(closed_text(&protection.0, &name, &p, &rules, &home))
        }))
    }
}

fn closed_text(
    protection: &Protection<'_>,
    name: &str,
    path: &str,
    rules: &[String],
    home: &str,
) -> String {
    let rule = rules[0].as_str();
    let short = ferb_fs::recovery::short(path, home);
    match rule {
        FERB_STATE => fill(FS_FERB_STATE, &[("name", name), ("path", &short)]),
        FERB_APP => fill(FS_FERB_APP, &[("name", name), ("path", &short)]),
        FERB_SERVICE => fill(FS_FERB_SERVICE, &[("name", name), ("path", &short)]),
        _ => {
            let store = protection
                .entry(rule)
                .map(|e| e.shown.clone())
                .unwrap_or_else(|| rule.to_string());
            let text = if rule.starts_with("read:") {
                FS_NEVER_READ
            } else {
                FS_NEVER_WRITE
            };
            fill(text, &[("name", name), ("path", &short), ("store", &store)])
        }
    }
}

/// `CARD_KINDS` (`ferb/identity/owner_secrets.py`): the card fields.
fn ferb_identity_card_kind(kind: &str) -> bool {
    matches!(kind, "card_number" | "card_csc" | "card_expiry")
}

// ---------------------------------------------------------------------------------------------------------------
// _needs: every reason to ask, from facts
// ---------------------------------------------------------------------------------------------------------------

/// `lift_need` (GATE-203, GATE-200): a read covers every read of that folder in the tree; a change this one call.
fn lift_need(protection: &Protection<'_>, rule: &str, how: &str, call: &str) -> Need {
    let shown = protection
        .entry(rule)
        .map(|e| e.shown.clone())
        .unwrap_or_else(|| rule.to_string());
    if how == "read" {
        return Need::new(
            &format!("escape:{rule}"),
            confirm::read_lift_key(rule),
            Values::of(&[
                (
                    "text",
                    &format!("let FERB read {shown} for this task (rule {rule})"),
                ),
                ("lift", "read"),
                ("path", &shown),
            ]),
        );
    }
    let mut n = Need::new(
        &format!("escape:{rule}"),
        format!(
            "lift:change:{rule}:{}",
            ferb_policy::small::escape_key(rule, call, "")
        ),
        Values::of(&[
            (
                "text",
                &format!("let FERB change {shown} once (rule {rule})"),
            ),
            ("lift", "write"),
            ("path", &shown),
        ]),
    );
    n.always = true;
    n.spent_on_ask = true;
    n
}

fn sources_json(s: &ArgSources) -> Value {
    Value::Object(s.iter().map(|(k, v)| (k.clone(), json!(v))).collect())
}

impl Gateway {
    async fn needs(&self, c: &mut Call) -> R<Result<Vec<Need>, Reply>> {
        let mut needs: Vec<Need> = self.lift_needs(c);
        if let Some(url) = self.url_sink_need(c)? {
            needs.push(url);
        }
        let foreground = confirm::foreground_need(&c.name, &c.arguments, self.d.apps.as_ref());
        if c.consequential() || foreground.is_some() {
            let sources = self.arg_sources(&c.checked.target, &c.rec, &c.grant)?;
            c.extra.insert("arg_sources".into(), sources_json(&sources));
        }
        if c.consequential() {
            match self.floor_needs(c).await? {
                Ok(floor) => needs.extend(floor),
                Err(refused) => return Ok(Err(self.refuse_call(c, refused, None)?)),
            }
        }
        if let Some(f) = foreground {
            needs.push(f);
            if let Some(page) = c.cancel.guard(c.tool.browser_page(&c.args)).await? {
                let app = c
                    .arguments
                    .get("app")
                    .filter(|v| ferb_py::value::truthy(v))
                    .map(ferb_py::value::py_str)
                    .unwrap_or_else(|| "the browser".into());
                needs.push(confirm::browser_page_need(
                    &app,
                    &page,
                    confirm::input_verb(&c.name, &c.arguments),
                ));
            }
        }
        if c.name == "browser.pair" {
            if let Some(pair) = self.pair_need(c)? {
                needs.push(pair);
            }
        }
        if c.escape.is_some() {
            needs.push(self.escape_need(c));
        }
        match self.terminal_need(c).await? {
            Err(refused) => return Ok(Err(self.refuse_call(c, refused, None)?)),
            Ok(Some(t)) => needs.push(t),
            Ok(None) => {}
        }
        let mut unique: Vec<Need> = Vec::new();
        for n in needs {
            if !unique.iter().any(|u| u.key == n.key) {
                unique.push(n);
            }
        }
        Ok(Ok(unique))
    }

    fn lift_needs(&self, c: &mut Call) -> Vec<Need> {
        let protection = &self.d.roots.protection.0;
        for (path, access, rules) in c.found.clone() {
            for rule in rules {
                let how = lift_how(&access).to_string();
                if !c
                    .lifts
                    .iter()
                    .any(|(k, _)| *k == (rule.clone(), how.clone()))
                {
                    let need = lift_need(protection, &rule, &how, &format!("{} {path}", c.name));
                    c.lifts.push(((rule, how), need));
                }
            }
        }
        c.lifts.iter().map(|(_, n)| n.clone()).collect()
    }

    /// `_outside_read(root)`: the outside content this tree read, sorted (S3, S4, L4).
    fn outside_read(&self, root: &str) -> R<Vec<String>> {
        let read = self.spans(|s| -> ferb_store::Result<BTreeSet<String>> {
            let mut r = s.tool_origins(root)?;
            r.extend(s.outside_exposure_origins(root)?);
            r.extend(s.request_origins(root)?);
            Ok(r)
        })?;
        Ok(read
            .into_iter()
            .filter(|o| origin::is_external(o))
            .collect())
    }

    fn owner_answers(&self, root: &str) -> R<Vec<String>> {
        Ok(self.spans(|s| s.owner_answers(root))?)
    }

    /// `_host_named(host, root)` (M4, GATE-537).
    fn host_named(&self, host_norm: &str, root: &str) -> R<bool> {
        let rec = self.task(root);
        let request = rec.map(|r| self.request(&r.request_id));
        let words = request
            .as_ref()
            .map(|r| r.words.clone())
            .unwrap_or_default();
        let text = format!("{words}\n{}", self.owner_answers(root)?.join("\n"));
        let sites: HashSet<String> = request
            .map(|r| {
                r.targets
                    .sites
                    .iter()
                    .filter(|h| !ferb_py::value::strip(h).is_empty())
                    .map(|h| {
                        let after = h.rsplit("://").next().unwrap_or(h);
                        let host = after.split('/').next().unwrap_or(after);
                        let n = dest::normalize("host", host);
                        n.strip_prefix("www.").map(str::to_string).unwrap_or(n)
                    })
                    .collect()
            })
            .unwrap_or_default();
        let named: HashSet<String> = dest::from_words(&text)
            .into_iter()
            .filter(|(k, _)| k == "host")
            .map(|(_, h)| dest::normalize("host", &h))
            .collect();
        Ok(named.contains(host_norm)
            || sites.contains(host_norm.strip_prefix("www.").unwrap_or(host_norm)))
    }

    /// `_read_texts(root)`: the tree's tool results so far, prepared once (only results not seen before).
    fn read_texts(&self, root: &str) -> R<Vec<(String, Text)>> {
        let (floor, seen) = {
            let mut i = lock(&self.inner);
            let k = i.outside.entry(root.to_string()).or_default();
            (k.floor, k.seen.clone())
        };
        let (running, outputs) = self.spans(|s| -> ferb_store::Result<_> {
            Ok((
                s.first_open_tool_call(root, floor, true)?,
                s.tool_outputs(root, floor, true)?,
            ))
        })?;
        let mut i = lock(&self.inner);
        let known = i.outside.entry(root.to_string()).or_default();
        let mut seen = seen;
        for (rowid, origins, text) in outputs {
            if seen.contains(&rowid) {
                continue;
            }
            seen.insert(rowid);
            let external: Vec<&String> =
                origins.iter().filter(|o| origin::is_external(o)).collect();
            if !external.is_empty() {
                let prepared = provenance::prepare(&text);
                for o in external {
                    known.texts.push((o.clone(), prepared.clone()));
                }
            } else if origins.contains(origin::PRIVATE)
                && origins
                    .iter()
                    .all(|o| o == origin::PRIVATE || o == origin::OWNER)
            {
                known
                    .texts
                    .push((origin::PRIVATE.to_string(), provenance::prepare(&text)));
            }
        }
        known.floor = match running {
            Some(r) => r - 1,
            None => seen.iter().copied().max().unwrap_or(known.floor),
        };
        let f = known.floor;
        known.seen = seen.into_iter().filter(|r| *r > f).collect();
        Ok(known.texts.clone())
    }

    /// `_arg_sources(spec, rec, grant)`: where each of the call's targets came from (S3).
    fn arg_sources(&self, target: &Target, rec: &TaskRecord, grant: &RunGrant) -> R<ArgSources> {
        let root = rec.tree_id.clone().unwrap_or_else(|| grant.task_id.clone());
        let request = self.request(&rec.request_id);
        {
            let mut i = lock(&self.inner);
            let known = i.outside.entry(root.clone()).or_default();
            if known.pasted.is_none() {
                let mut pasted: Vec<(String, Text)> =
                    ferb_sanitize::untrusted::pasted_parts(&request.payload)
                        .into_iter()
                        .map(|p| (origin::external("paste", ""), provenance::prepare(&p)))
                        .collect();
                for q in ferb_sanitize::untrusted::earlier_questions(&request.payload) {
                    for o in &request.asked_origins {
                        if origin::is_external(o) {
                            pasted.push((o.clone(), provenance::prepare(&q)));
                        }
                    }
                }
                known.pasted = Some(pasted);
            }
        }
        let mut external = self.read_texts(&root)?;
        external.extend(
            lock(&self.inner)
                .outside
                .get(&root)
                .and_then(|k| k.pasted.clone())
                .unwrap_or_default(),
        );
        let answers = provenance::prepare(&self.owner_answers(&root)?.join("\n"));
        Ok(provenance::arg_sources_prepared(
            target,
            &provenance::prepare(&request.words),
            &external,
            Some(&answers),
            &self.d.roots.home,
        ))
    }

    /// `_sink_texts(root)`: the tree's records as the sink reads them: the owner's data and what was outside only.
    #[allow(clippy::type_complexity)]
    fn sink_texts(
        &self,
        root: &str,
    ) -> R<(
        Vec<(i64, Text)>,
        Vec<(i64, Text)>,
        HashMap<usize, HashSet<String>>,
    )> {
        let (floor, seen) = {
            let mut i = lock(&self.inner);
            let k = i.outside.entry(root.to_string()).or_default();
            (k.payload_floor, k.payload_seen.clone())
        };
        let (running, results) = self.spans(|s| -> ferb_store::Result<_> {
            Ok((
                s.first_open_tool_call(root, floor, true)?,
                s.payload_outputs(root, floor, Some(&seen))?,
            ))
        })?;
        let mut i = lock(&self.inner);
        let known = i.outside.entry(root.to_string()).or_default();
        let mut changed: Vec<String> = Vec::new();
        for result in results {
            known.payload_seen.insert(result.rowid);
            if !result.text.is_empty() {
                let text = confirm::with_forms(provenance::prepare(&result.text));
                known
                    .payloads
                    .insert(result.rowid, (result.origins.clone(), text));
                if !result.page.is_empty() {
                    known.pages.insert(result.rowid, result.page.clone());
                }
                if let Some(segments) = &result.segments {
                    let segs: Vec<carried::Segment> =
                        segments.iter().map(carried::Segment::from_json).collect();
                    known
                        .owners
                        .insert(result.rowid, owner_parts(&result.text, &segs));
                    known
                        .said
                        .insert(result.rowid, carried::owner_said(&result.text, &segs));
                }
            }
            if !result.path.is_empty() {
                match known
                    .file_chunks
                    .iter_mut()
                    .find(|(p, _)| *p == result.path)
                {
                    Some((_, m)) => {
                        m.insert(result.rowid, result.clone());
                    }
                    None => known.file_chunks.push((
                        result.path.clone(),
                        [(result.rowid, result.clone())].into_iter().collect(),
                    )),
                }
                if !changed.contains(&result.path) {
                    changed.push(result.path.clone());
                }
            }
        }
        known.payload_floor = match running {
            Some(r) => r - 1,
            None => known
                .payload_seen
                .iter()
                .copied()
                .max()
                .unwrap_or(known.payload_floor),
        };
        let f = known.payload_floor;
        known.payload_seen.retain(|r| *r > f);
        for path in changed {
            let chunks: Vec<PayloadOutput> = known
                .file_chunks
                .iter()
                .find(|(p, _)| *p == path)
                .map(|(_, m)| m.values().cloned().collect())
                .unwrap_or_default();
            let mut joined: Vec<(i64, BTreeSet<String>, Text)> = Vec::new();
            let mut chain: Vec<PayloadOutput> = Vec::new();
            let finish =
                |chain: &Vec<PayloadOutput>, joined: &mut Vec<(i64, BTreeSet<String>, Text)>| {
                    if chain.len() > 1 {
                        let labels: BTreeSet<String> = chain
                            .iter()
                            .flat_map(|p| p.origins.iter().cloned())
                            .collect();
                        let text: String = chain.iter().map(|p| p.text.as_str()).collect();
                        joined.push((chain[0].rowid, labels, provenance::prepare(&text)));
                    }
                };
            for part in chunks {
                let starts = part.offset.as_i64() == Some(0) || part.offset == json!(0.0);
                if !chain.is_empty()
                    && (starts
                        || !ferb_py::value::py_eq(
                            &chain[chain.len() - 1].next_offset,
                            &part.offset,
                        ))
                {
                    finish(&chain, &mut joined);
                    chain.clear();
                }
                chain.push(part);
            }
            finish(&chain, &mut joined);
            match known.joined_files.iter_mut().find(|(p, _)| *p == path) {
                Some((_, j)) => *j = joined,
                None => known.joined_files.push((path, joined)),
            }
        }
        let mut results: Vec<(i64, BTreeSet<String>, Text)> = known
            .payloads
            .iter()
            .map(|(r, (l, t))| (*r, l.clone(), t.clone()))
            .collect();
        for (_, j) in &known.joined_files {
            results.extend(j.iter().cloned());
        }
        results.sort_by_key(|r| r.0);
        let mut private = Vec::new();
        let mut sites: HashMap<usize, HashSet<String>> = HashMap::new();
        for (rowid, labels, text) in &results {
            if let Some(parts) = known.owners.get(rowid) {
                for (on, p) in parts.iter().filter(|(_, p)| !p.folded.is_empty()) {
                    if !on.is_empty() {
                        sites.insert(private.len(), on.iter().cloned().collect());
                    }
                    private.push((*rowid, p.clone()));
                }
            } else if labels.contains(origin::PRIVATE) {
                private.push((*rowid, text.clone()));
            }
        }
        let outside_only = results
            .iter()
            .filter(|(_, l, _)| {
                !l.contains(origin::PRIVATE) && l.iter().any(|o| origin::is_external(o))
            })
            .map(|(r, _, t)| (*r, t.clone()))
            .collect();
        Ok((private, outside_only, sites))
    }

    /// `_sink_owner_text`: this request's words, the owner's answers and their earlier messages (GATE-534).
    fn sink_owner_text(&self, rec: &TaskRecord, root: &str) -> R<Text> {
        let request = self.request(&rec.request_id);
        let said: Vec<String> = lock(&self.inner)
            .outside
            .get(root)
            .map(|k| k.said.values().cloned().collect())
            .unwrap_or_default();
        let mut parts = vec![request.words.clone()];
        parts.extend(self.owner_answers(root)?);
        parts.extend(said);
        Ok(provenance::prepare(&parts.join("\n")))
    }

    /// `_sink_origins`: each record's labels by span order.
    fn sink_origins(&self, root: &str) -> HashMap<i64, HashSet<String>> {
        let i = lock(&self.inner);
        let Some(known) = i.outside.get(root) else {
            return HashMap::new();
        };
        let mut found: HashMap<i64, HashSet<String>> = known
            .payloads
            .iter()
            .map(|(r, (l, _))| (*r, l.iter().cloned().collect()))
            .collect();
        for (_, j) in &known.joined_files {
            for (r, l, _) in j {
                found.insert(*r, l.iter().cloned().collect());
            }
        }
        found
    }

    /// `_read_as(root, rowid, plain)` (GATE-507): where this task read a record, in the owner's terms.
    fn read_as(&self, root: &str, rowid: i64, plain: bool) -> R<String> {
        let path = lock(&self.inner).outside.get(root).and_then(|k| {
            k.file_chunks
                .iter()
                .find(|(_, m)| m.contains_key(&rowid))
                .map(|(p, _)| p.clone())
        });
        if let Some(path) = path {
            let name = ferb_fs::pypath::name(&path);
            return Ok(if plain {
                format!(
                    "your file {}",
                    if name.is_empty() { path.clone() } else { name }
                )
            } else {
                format!("your file {path}")
            });
        }
        let row: Option<(String, Option<String>, Option<String>)> = {
            let db = lock(&self.d.db);
            db.conn()
                .query_row(
                    "SELECT kind, json_extract(input_json, '$.tool') AS tool, origin FROM spans WHERE rowid = ?",
                    [rowid],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )
                .ok()
        };
        let Some((kind, tool, origins)) = row else {
            return Ok("your own data".into());
        };
        if kind != "tool.call" {
            return Ok(if plain {
                "your chats or FERB's memory".into()
            } else {
                "the context FERB gave the model (your conversation, memory or earlier results)"
                    .into()
            });
        }
        let sites: BTreeSet<String> = origin::split(origins.as_deref())
            .into_iter()
            .filter(|o| o.starts_with("external:web:"))
            .map(|o| o.splitn(3, ':').nth(2).unwrap_or("").to_string())
            .collect();
        let sites: Vec<String> = sites.into_iter().collect();
        let two = &sites[..sites.len().min(2)];
        if plain {
            return Ok(if sites.is_empty() {
                "your own data".into()
            } else {
                format!("your signed-in {} page", two.join(" and "))
            });
        }
        let on = if sites.is_empty() {
            String::new()
        } else {
            format!(" on {}", two.join(", "))
        };
        let tool = tool
            .filter(|t| !t.is_empty())
            .unwrap_or_else(|| "a tool".into());
        Ok(format!("the result of {tool}{on}"))
    }

    /// `_url_sink_need` (S4, SQ6, Q8): a browser address carrying the owner's data to a host not named or approved.
    fn url_sink_need(&self, c: &Call) -> R<Option<Need>> {
        let url = c
            .arguments
            .get("url")
            .filter(|v| ferb_py::value::truthy(v))
            .map(ferb_py::value::py_str)
            .unwrap_or_default();
        if (c.spec.kind != Kind::NAVIGATE && c.spec.kind != Kind::OPEN_TAB) || url.is_empty() {
            return Ok(None);
        }
        let host = dest::normalize("host", c.spec.target.site.as_deref().unwrap_or(""));
        let root = c.root();
        let key = confirm::destination_key(&host);
        let reopened = self.reopened(&root)?;
        if let Some(row) = self.find(&root, &key, &reopened)? {
            return Ok(if row.granted {
                None
            } else {
                Some(Need::new(
                    confirm::SINK_DESTINATION,
                    key,
                    Values::of(&[("text", &format!("reaching {host}"))]),
                ))
            });
        }
        if let Ok(address) = host.parse::<std::net::IpAddr>() {
            let loopback = match address {
                std::net::IpAddr::V4(a) => a.is_loopback(),
                std::net::IpAddr::V6(a) => {
                    a.is_loopback() || a.to_ipv4_mapped().is_some_and(|m| m.is_loopback())
                }
            };
            if loopback {
                return Ok(None);
            }
        }
        let outside_labels = self.outside_read(&root)?;
        if outside_labels.is_empty() {
            return Ok(None);
        }
        if self.destinations(|d| d.approved("host", &host))? || self.host_named(&host, &root)? {
            return Ok(None);
        }
        let (private, outside_only, sites) = self.sink_texts(&root)?;
        let owner_text = self.sink_owner_text(&c.rec, &root)?;
        let origins = self.sink_origins(&root);
        let pages = lock(&self.inner)
            .outside
            .get(&root)
            .map(|k| k.pages.clone())
            .unwrap_or_default();
        let values = confirm::url_private_values(
            &url,
            &private,
            &outside_only,
            &owner_text,
            Some(&origins),
            Some(&sites),
            Some(&pages),
        );
        let Some(first) = values.first() else {
            return Ok(None);
        };
        let shown_url = if url.chars().count() <= 300 {
            url.clone()
        } else {
            format!("{}…", ferb_py::value::prefix(&url, 299))
        };
        let mut n = Need::new(
            "sink:private_url",
            key,
            Values::of(&[
                ("destination", &host),
                ("value", &first.piece),
                ("matched", &first.matched),
                ("url", &shown_url),
                ("source", &self.read_as(&root, first.first_read, false)?),
                (
                    "source_plain",
                    &self.read_as(&root, first.first_read, true)?,
                ),
                (
                    "read",
                    &confirm::shown_origins(outside_labels.iter().map(String::as_str)),
                ),
            ]),
        );
        n.destinations = vec![("host".into(), host)];
        Ok(Some(n))
    }

    /// `_visited_outside(root)` (GATE-557).
    fn visited_outside(&self, root: &str) -> R<Vec<String>> {
        let (w, p) = tree_where(root);
        let db = lock(&self.d.db);
        let mut st = db.conn().prepare(&format!(
            "SELECT origin FROM spans WHERE kind = 'tool.call' AND origin IS NOT NULL AND {w}"
        ))?;
        let rows: Vec<String> = st
            .query_map(rusqlite::params_from_iter(p.iter()), |r| r.get(0))?
            .collect::<Result<_, _>>()?;
        let mut found = BTreeSet::new();
        for labels in rows {
            let outside: BTreeSet<String> = origin::split(Some(&labels))
                .into_iter()
                .filter(|o| origin::is_external(o))
                .collect();
            if outside.len() == 1 {
                found.extend(outside);
            }
        }
        Ok(found.into_iter().collect())
    }

    /// `_found_via(root, host)` (GATE-566).
    fn found_via(&self, root: &str, host: &str) -> String {
        let i = lock(&self.inner);
        let Some(known) = i.outside.get(root) else {
            return String::new();
        };
        let bare = host.strip_prefix("www.").unwrap_or(host);
        for (labels, text) in known.payloads.values() {
            let prefix = format!("{}:web:", origin::EXTERNAL);
            let sites: Vec<String> = labels
                .iter()
                .filter(|o| o.starts_with(&prefix) && o.matches(':').count() >= 2)
                .map(|o| o.splitn(3, ':').nth(2).unwrap_or("").to_string())
                .collect();
            if sites.len() == 1
                && sites[0].strip_prefix("www.").unwrap_or(&sites[0]) != bare
                && text.folded.contains(bare)
            {
                return format!("on a {} page", sites[0]);
            }
        }
        String::new()
    }

    /// `_submit_sink` (S4 a, GATE-215, GATE-557): a submitted form carrying the owner's data to a new site.
    fn submit_sink(&self, c: &Call, target: &Target, outside: &[String]) -> R<Option<Need>> {
        let host = dest::normalize("host", target.site.as_deref().unwrap_or(""));
        let tab = c
            .args
            .get("tab")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        if host.is_empty() || outside.is_empty() || tab.is_empty() {
            return Ok(None);
        }
        let root = c.root();
        if self.destinations(|d| d.approved("host", &host))? || self.host_named(&host, &root)? {
            return Ok(None);
        }
        let mut typed = self.spans(|s| s.typed_in_tab(&c.grant.task_id, &tab))?;
        if let (Some(content), true) = (&c.spec.content, c.spec.kind == Kind::FILL_FORM) {
            typed.push(content.clone());
        }
        if typed.is_empty() {
            return Ok(None);
        }
        let (private, outside_only, _) = self.sink_texts(&root)?;
        let owner_text = self.sink_owner_text(&c.rec, &root)?;
        let mut values =
            confirm::text_private_values(&typed.join("\n"), &private, &outside_only, &owner_text);
        if !values.is_empty() {
            let own = (self.d.owner_contacts)();
            values.retain(|v| !confirm::own_contact(&v.piece, &own));
        }
        if values.is_empty() {
            return Ok(None);
        }
        let shown = &values[..values.len().min(SINK_VALUES_SHOWN)];
        let mut sent = Vec::new();
        let mut plain: Vec<String> = Vec::new();
        for v in shown {
            sent.push((v.piece.clone(), self.read_as(&root, v.first_read, false)?));
            let p = self.read_as(&root, v.first_read, true)?;
            if !plain.contains(&p) {
                plain.push(p);
            }
        }
        let control = c
            .facts()
            .and_then(|f| f.control_label.clone())
            .map(|l| ferb_py::value::strip(&l).to_string())
            .unwrap_or_default();
        let visited = self.visited_outside(&root)?;
        let more = if values.len() > sent.len() {
            (values.len() - sent.len()).to_string()
        } else {
            String::new()
        };
        let mut n = Need::new(
            "sink:new_destination",
            confirm::key(&[json!("sink:new_destination"), json!(host)]),
            Values::of(&[
                ("destination", &host),
                ("value", &sent[0].0),
                ("source", &sent[0].1),
                ("sent", &confirm::shown_sent(&sent, values.len())),
                ("control", &control),
                ("form", "yes"),
                (
                    "read",
                    &confirm::shown_origins(visited.iter().map(String::as_str)),
                ),
                (
                    "values",
                    &sent
                        .iter()
                        .map(|(v, _)| v.as_str())
                        .collect::<Vec<_>>()
                        .join("\n"),
                ),
                ("more", &more),
                ("sources_plain", &plain.join(" and ")),
                ("found_via", &self.found_via(&root, &host)),
            ]),
        );
        n.reaches = Some(target.clone());
        Ok(Some(n))
    }

    /// `_floor_needs` (D23 floor, D33, S4, SQ6, L39): `confirm.decide` over the call's kinds and its facts' kinds.
    async fn floor_needs(&self, c: &mut Call) -> R<Result<Vec<Need>, Map<String, Value>>> {
        let root = c.root();
        let read = self.spans(|s| -> ferb_store::Result<BTreeSet<String>> {
            let mut r = s.tool_origins(&root)?;
            r.extend(s.exposure_origins(&root)?);
            Ok(r)
        })?;
        let outside = self.outside_read(&root)?;
        let known = |k: &str, v: &str| self.destinations(|d| d.known(k, v)).unwrap_or(false);
        let approved = |k: &str, v: &str| self.destinations(|d| d.approved(k, v)).unwrap_or(false);
        let own = || (self.d.owner_contacts)();
        let mut target = c.checked.target.clone();
        target.amount = c.spec.target.amount;
        target.currency = c.spec.target.currency.clone();
        let sources: ArgSources = arg_sources_back(c.extra.get("arg_sources"));
        let declared: Vec<String> = rules::declared_kinds(&c.checked).into_iter().collect();
        let declared_refs: Vec<&str> = declared.iter().map(String::as_str).collect();
        // The policy's facts hold plain `&dyn Fn` lookups (not `Sync`): they live only in this block, never across
        // the counting await below, so the call's future stays `Send`.
        let (mut needs, deletes) = {
            let facts = confirm::Facts {
                read_private: read.contains(origin::PRIVATE),
                read_external: !outside.is_empty(),
                known: &known,
                approved: Some(&approved),
                outside_read: &outside,
                place: c.spec.place.as_deref(),
                content: c.spec.content.as_deref(),
                own: &own,
            };
            let mut needs =
                confirm::decide(&c.spec.kind, &declared_refs, &target, &sources, &facts);
            let mut kinds_all: BTreeSet<String> = declared.iter().cloned().collect();
            kinds_all.insert(c.spec.kind.clone());
            let mut deletes: Vec<(String, ArgSources)> = Vec::new();
            if let Some(p) = target.path.clone().filter(|p| !p.is_empty()) {
                if kinds_all.contains(Kind::DELETE_PERMANENT) {
                    deletes.push((p, sources.clone()));
                }
            }
            for (kind, fact_target) in c.fact_targets.clone() {
                if kind == Kind::SUBMIT {
                    if let Some(sink) = self.submit_sink(c, &fact_target, &outside)? {
                        needs.push(sink);
                    }
                    continue;
                }
                let fact_sources = self.arg_sources(&fact_target, &c.rec, &c.grant)?;
                let f2 = confirm::Facts {
                    content: None,
                    ..facts_copy(&facts)
                };
                needs.extend(confirm::decide(
                    &kind,
                    &[],
                    &fact_target,
                    &fact_sources,
                    &f2,
                ));
                if kind == Kind::DELETE_PERMANENT {
                    if let Some(p) = fact_target.path.clone() {
                        deletes.push((p, fact_sources));
                    }
                }
            }
            (needs, deletes)
        };
        for (path, path_sources) in deletes {
            let real =
                ferb_fs::files::real_path(&path, &self.d.roots.workspace, &self.d.roots.home)
                    .unwrap_or(path.clone());
            if ferb_fs::pypath::is_dir(&real) {
                let closed = self.path_closed(&c.name, &BTreeSet::new());
                let counted = c.cancel.guard(self.d.usage.count(&real, closed)).await?;
                if !counted.get("complete").is_some_and(ferb_py::value::truthy) {
                    return Ok(Err(obj(
                        json!({"error": "bulk deletion could not be fully enumerated; inspect the exact target first"}),
                    )));
                }
                if !confirm::owner_named(provenance::get(&path_sources, "path"))
                    && !self.made_here(&root, &path)
                {
                    match confirm::bulk_delete(&path, &counted) {
                        Ok(n) => needs.push(n),
                        Err(m) => {
                            return Err(CallError::Internal(format!(
                                "bulk_delete: KeyError {:?}",
                                m.0
                            )))
                        }
                    }
                }
            }
        }
        if self.d.made_in_tree.is_some() {
            needs.retain(|n| {
                !(n.rule == "floor:delete_permanent"
                    && n.values
                        .get("path")
                        .is_some_and(|p| !p.is_empty() && self.made_here(&root, p)))
            });
        }
        if let Some(f) = c.facts().cloned() {
            if let Some(amount) = f.page_amount.filter(|a| !a.is_empty()) {
                for n in needs.iter_mut() {
                    if n.rule == confirm::ALWAYS_PAY {
                        n.values.set("amount", &amount);
                        n.values
                            .set("merchant", f.page_merchant.as_deref().unwrap_or(""));
                    }
                }
            }
        }
        self.payment(c, needs)
    }
}

/// The facts struct again, for the facts' own decisions (the same reads; content is the call's only).
fn facts_copy<'a>(f: &confirm::Facts<'a>) -> confirm::Facts<'a> {
    confirm::Facts {
        read_private: f.read_private,
        read_external: f.read_external,
        known: f.known,
        approved: f.approved,
        outside_read: f.outside_read,
        place: f.place,
        content: f.content,
        own: f.own,
    }
}

/// The recorded arg_sources back as ArgSources.
fn arg_sources_back(v: Option<&Value>) -> ArgSources {
    v.and_then(Value::as_object)
        .map(|m| {
            m.iter()
                .map(|(k, v)| {
                    (
                        k.clone(),
                        v.as_array()
                            .map(|a| a.iter().map(ferb_py::value::py_str).collect())
                            .unwrap_or_default(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// `_owner_parts`: a stored record of FERB's conversation as the sink's owner-data parts, prepared.
fn owner_parts(payload: &str, segments: &[carried::Segment]) -> Vec<(BTreeSet<String>, Text)> {
    let (main, by_site) = carried::sink_parts(payload, segments);
    let mut out = vec![(
        BTreeSet::new(),
        confirm::with_forms(provenance::prepare(&main)),
    )];
    for (sites, text) in by_site {
        out.push((sites, confirm::with_forms(provenance::prepare(&text))));
    }
    out
}

impl Gateway {
    /// `_made_here` (GATE-539).
    fn made_here(&self, root: &str, path: &str) -> bool {
        let Some(made) = &self.d.made_in_tree else {
            return false;
        };
        match ferb_fs::files::real_path(path, &self.d.roots.workspace, &self.d.roots.home) {
            Ok(real) => made(root, &real),
            Err(_) => false,
        }
    }

    /// `_card_entry(root, chain)` (ORCH-321): the saved card entry that filled a card field in this tab chain.
    fn card_entry(&self, root: &str, chain: &str) -> R<Option<String>> {
        let (w, p) = tree_where(root);
        let db = lock(&self.d.db);
        let mut st = db.conn().prepare(&format!(
            "SELECT input_json FROM spans WHERE kind = 'tool.call' AND ended_at IS NOT NULL AND error IS NULL AND \
             json_extract(input_json, '$.card_entry') IS NOT NULL AND {w} ORDER BY started_at DESC, rowid DESC"
        ))?;
        let rows: Vec<Option<String>> = st
            .query_map(rusqlite::params_from_iter(p.iter()), |r| r.get(0))?
            .collect::<Result<_, _>>()?;
        for data in rows {
            let call: Value =
                serde_json::from_str(data.as_deref().unwrap_or("{}")).unwrap_or(Value::Null);
            let same = call.get("facts").and_then(|f| f.get("tab_chain"))
                == Some(&Value::String(chain.to_string()));
            if same
                && !call
                    .get("boundary_refused")
                    .is_some_and(ferb_py::value::truthy)
            {
                return Ok(Some(ferb_py::value::py_str(&call["card_entry"])));
            }
        }
        Ok(None)
    }

    /// `_payment` (GATE-204): a payment in a task's tab is asked once per payment; outside a tab each run asks.
    fn payment(&self, c: &mut Call, needs: Vec<Need>) -> R<Result<Vec<Need>, Map<String, Value>>> {
        let pays: Vec<Need> = needs
            .iter()
            .filter(|n| n.rule == confirm::ALWAYS_PAY)
            .cloned()
            .collect();
        if pays.is_empty() {
            return Ok(Ok(needs));
        }
        let stated = confirm::shown_amount(
            c.checked.target.amount,
            c.checked.target.currency.as_deref(),
        );
        let f = c.facts().cloned();
        if let Some(chain) = f
            .as_ref()
            .and_then(|f| f.tab_chain.clone())
            .filter(|t| !t.is_empty())
        {
            let f = f.as_ref().expect("checked");
            let kinds: BTreeSet<&str> = f.field_kinds.iter().map(|k| k.as_str()).collect();
            let submits = f.submits_form == Some(true);
            c.pay = Some(PayFacts {
                chain: chain.clone(),
                host: host_of(f.page_url.as_deref()),
                frame_host: f
                    .frame_host
                    .clone()
                    .map(|h| h.to_lowercase())
                    .filter(|h| !h.is_empty()),
                amount: f.page_amount.clone(),
                merchant: f.page_merchant.clone(),
                submits_card: submits && kinds.iter().any(|k| ferb_identity_card_kind(k)),
                submits,
                shown: f.page_amounts.clone(),
                ..PayFacts::default()
            });
            if let Some(entry) = self.card_entry(&c.root(), &chain)? {
                c.extra.insert("card_entry".into(), Value::String(entry));
            }
        } else if c.name.starts_with("mac.")
            && c.spec.target.site.as_deref().is_some_and(|s| !s.is_empty())
        {
            let window = window_of(&Value::Object(hashed_args(c.tool.as_ref(), &c.args)));
            match window {
                Some(window) => {
                    let site = c.spec.target.site.clone().unwrap_or_default();
                    let shown = pays[0].values.get("amount").map(str::to_string);
                    let amount = shown.filter(|s| !s.is_empty() && s != confirm::UNREAD_AMOUNT);
                    c.pay = Some(PayFacts {
                        chain: format!("mac:{site}:{window}"),
                        host: site,
                        amount,
                        ..PayFacts::default()
                    });
                }
                None => return Ok(self.pay_each_run(c, needs, &stated)),
            }
        } else {
            return Ok(self.pay_each_run(c, needs, &stated));
        }
        let rest: Vec<Need> = needs
            .iter()
            .filter(|n| n.rule != confirm::ALWAYS_PAY)
            .cloned()
            .collect();
        let page = payments::page_key(f.as_ref().and_then(|f| f.page_url.as_deref()))
            .ok()
            .flatten();
        let control = f
            .as_ref()
            .and_then(|f| f.control_label.clone())
            .map(|l| ferb_py::value::strip(&l).to_string())
            .filter(|l| !l.is_empty());
        if let Some(pay) = c.pay.as_mut() {
            pay.declared = Some(stated.clone()).filter(|s| !s.is_empty());
            pay.page = page;
            pay.control = if f.is_some() { control } else { None };
        }
        let mut values = pays[0].values.clone();
        if !stated.is_empty() {
            values.set("declared", &stated);
        }
        c.pay_values = values;
        let check =
            lock(&self.payments).check(&c.root(), c.pay.as_ref().expect("set"), &c.pay_values);
        if let Some(a) = check.approval {
            c.pay_cid = Some(a.cid.clone());
            c.extra.insert(
                "payment".into(),
                json!({"covered_by": a.cid, "scope": c.pay.as_ref().map(|p| p.chain.clone())}),
            );
            return Ok(Ok(rest));
        }
        let need = check.need.expect("a check without an approval asks");
        if let Some(back) = self.amount_unstated(c, std::slice::from_ref(&need))? {
            return Ok(Err(back));
        }
        c.pay_key = Some(need.key.clone());
        let mut out = rest;
        out.push(need);
        Ok(Ok(out))
    }

    /// No tab and no window to scope it: each run asks (SQ1).
    fn pay_each_run(
        &self,
        c: &mut Call,
        needs: Vec<Need>,
        stated: &str,
    ) -> Result<Vec<Need>, Map<String, Value>> {
        let on_page: Vec<String> = c
            .facts()
            .map(|f| f.page_amounts.clone())
            .unwrap_or_default();
        let needs: Vec<Need> = needs
            .into_iter()
            .map(|mut n| {
                if n.rule == confirm::ALWAYS_PAY {
                    let read = read_amount(&n.values);
                    let add = payments::stated(
                        Some(&read).filter(|r| !r.is_empty()).map(String::as_str),
                        Some(stated).filter(|s| !s.is_empty()),
                        &on_page,
                    );
                    for (k, v) in &add.0 {
                        n.values.set(k, v);
                    }
                }
                n
            })
            .collect();
        let pays: Vec<Need> = needs
            .iter()
            .filter(|n| n.rule == confirm::ALWAYS_PAY)
            .cloned()
            .collect();
        match self.amount_unstated(c, &pays) {
            Ok(Some(back)) => Err(back),
            _ => Ok(needs),
        }
    }

    /// `_amount_unstated` (GATE-583, GATE-584): a payment that would show no amount goes back to the worker once.
    fn amount_unstated(&self, c: &mut Call, pays: &[Need]) -> R<Option<Map<String, Value>>> {
        let unread: Vec<&Need> = pays
            .iter()
            .filter(|n| {
                read_amount(&n.values).is_empty() && confirm::ai_amount(&n.values).is_empty()
            })
            .collect();
        if unread.is_empty() {
            return Ok(None);
        }
        let root = c.root();
        let reopened = self.reopened(&root)?;
        for n in &unread {
            if let Some(row) = self.find(&root, &n.key, &reopened)? {
                if !row.granted {
                    return Ok(None);
                }
            }
        }
        let page = payments::page_key(c.facts().and_then(|f| f.page_url.as_deref()))
            .ok()
            .flatten();
        let mut i = lock(&self.inner);
        let seen = i.amount_asked.entry(root).or_default();
        let fresh: Vec<(String, Option<String>)> = unread
            .iter()
            .map(|n| (n.key.clone(), page.clone()))
            .filter(|k| !seen.contains(k))
            .collect();
        if fresh.is_empty() {
            return Ok(None);
        }
        seen.extend(fresh);
        drop(i);
        c.extra.insert("amount_asked".into(), Value::Bool(true));
        Ok(Some(obj(
            json!({"error": PAY_AMOUNT_UNSTATED, "next": PAY_AMOUNT_NEXT}),
        )))
    }

    /// `_pair_need`: the owner's word pairs a profile (a profile they named is already authorized).
    fn pair_need(&self, c: &Call) -> R<Option<Need>> {
        let profile = c
            .arguments
            .get("profile")
            .filter(|v| ferb_py::value::truthy(v))
            .map(ferb_py::value::py_str)
            .unwrap_or_default();
        let (browser, folder) = profile.split_once('/').unwrap_or((&profile, ""));
        let shown = (self.d.profile_name)(browser, folder);
        let request = self.request(&c.rec.request_id);
        let said = provenance::sources(
            &shown,
            &request.words,
            &[],
            &[],
            "",
            &self.owner_answers(&c.root())?.join("\n"),
        );
        if confirm::owner_named(&said) {
            return Ok(None);
        }
        Ok(Some(confirm::pair_need(
            &shown,
            browser,
            folder,
            &(self.d.profile_account)(browser, folder),
        )))
    }

    /// `_escape_need` (D31): a read escape is one Yes per folder per tree; any other lift this exact command once.
    fn escape_need(&self, c: &mut Call) -> Need {
        let (rule, command, cwd, kind) = c.escape.clone().expect("an escape");
        let protection = &self.d.roots.protection.0;
        let need = if kind == "read" {
            lift_need(protection, &rule, "read", &command)
        } else {
            let shown = protection
                .rules()
                .into_iter()
                .find(|r| r["id"] == Value::String(rule.clone()))
                .and_then(|r| r["path"].as_str().map(str::to_string))
                .unwrap_or_default();
            let mut n = Need::new(
                &format!("escape:{rule}"),
                ferb_policy::small::escape_key(&rule, &command, &cwd),
                Values::of(&[
                    (
                        "text",
                        &format!("this exact command needs rule {rule} lifted once: {shown}"),
                    ),
                    ("lift", &kind),
                    ("path", &shown),
                    ("command", &command),
                ]),
            );
            n.always = true;
            n.spent_on_ask = true;
            n
        };
        c.escape_need = Some(need.clone());
        need
    }

    /// `_terminal_need` (D31, GATE-585/586): typing into any terminal asks first; a secret is never typed there.
    async fn terminal_need(&self, c: &mut Call) -> R<Result<Option<Need>, Map<String, Value>>> {
        let task = c.grant.task_id.clone();
        let Some(mut inp) = c
            .cancel
            .guard(c.tool.terminal_input(&c.args, &task))
            .await?
        else {
            return Ok(Ok(None));
        };
        if inp.covered_by.is_some() && inp.covered_by != c.armed {
            inp.covered_by = None;
        }
        c.terminal = Some(inp.clone());
        if let Some(cid) = &inp.covered_by {
            c.extra.insert(
                "terminal".into(),
                json!({"confirmation": cid, "covers": "return"}),
            );
            return Ok(Ok(None));
        }
        if let Some(field) = self.secret_field(&inp) {
            return Ok(Err(obj(
                json!({"error": fill(TERMINAL_SECRET, &[("field", &field)])}),
            )));
        }
        let mut need = ferb_policy::terminal::need(&inp);
        need.fresh = true;
        need.spent_on_ask = true;
        c.terminal_need = Some(need.clone());
        Ok(Ok(Some(need)))
    }

    /// `_secret_field`: the first field of a terminal input that holds a secret, by its name for the owner.
    fn secret_field(&self, inp: &TerminalInput) -> Option<String> {
        let mut fields: Vec<(String, Option<String>)> = vec![
            (
                "the text".into(),
                if inp.paste { None } else { inp.text.clone() },
            ),
            (
                "the clipboard's text".into(),
                if inp.paste { inp.text.clone() } else { None },
            ),
            ("the editor's selection".into(), inp.selection.clone()),
            ("the active file's name".into(), inp.file.clone()),
            ("the script".into(), inp.script.clone()),
            ("the keys".into(), inp.keys.clone()),
            ("the window's title".into(), Some(inp.window.clone())),
            ("the terminal's label".into(), Some(inp.label.clone())),
            ("the menu item".into(), inp.menu.clone()),
            ("the control".into(), inp.control.clone()),
        ];
        for (i, (_, _, text)) in inp.files.iter().enumerate() {
            fields.push((format!("the file {}", i + 1), Some(text.clone())));
        }
        for (i, (path, _, _)) in inp.files.iter().enumerate() {
            fields.push((format!("the path of file {}", i + 1), Some(path.clone())));
        }
        for (name, raw) in fields {
            if let Some(raw) = raw.filter(|r| !r.is_empty()) {
                // a redactor that can't read the text counts as a hit (fails closed)
                if !matches!(self.d.redactor.hit(&raw), Ok(None)) {
                    return Some(name);
                }
            }
        }
        None
    }
}

// ---------------------------------------------------------------------------------------------------------------
// _authorize and FERB's own question
// ---------------------------------------------------------------------------------------------------------------

/// `_squash(text, keep_spaces)`.
fn squash(text: &str, keep_spaces: bool) -> String {
    if keep_spaces {
        text.chars()
            .map(|c| {
                if ferb_py::value::is_py_space(c) && c != ' ' {
                    ' '
                } else {
                    c
                }
            })
            .collect()
    } else {
        py_split_join(text)
    }
}

impl Gateway {
    /// `value(text)` (GATE-572): one value redacted by itself.
    pub fn value(&self, text: &str) -> String {
        let r = self.redact(&json!({"v": text}));
        r.get("v").map(ferb_py::value::py_str).unwrap_or_default()
    }

    /// `built(text)`: FERB's line from its fixed words and already-redacted values: one line, printable.
    pub fn built(&self, text: &str, keep_spaces: bool) -> String {
        squash(&printable(text), keep_spaces)
    }

    fn redacted_needs(&self, needs: &[Need]) -> Vec<Need> {
        needs
            .iter()
            .map(|n| {
                let mut n = n.clone();
                n.values = Values(
                    n.values
                        .0
                        .iter()
                        .map(|(k, v)| (k.clone(), self.value(v)))
                        .collect(),
                );
                n
            })
            .collect()
    }

    fn ferb_line(&self, needs: &[Need], call: &str, why: &str, typed: &str) -> R<String> {
        let keep = needs.iter().any(|n| n.rule == confirm::TERMINAL_INPUT);
        let call = self.value(&py_split_join(&printable(call)));
        let why = if ferb_py::value::strip(why).is_empty() {
            String::new()
        } else {
            quoted(&self.value(why), REASON_CHARS)
        };
        let typed = if ferb_py::value::strip(typed).is_empty() {
            String::new()
        } else {
            quoted(&self.value(typed), REASON_CHARS)
        };
        let text = confirm::line(&self.redacted_needs(needs), &call, &why, &typed)
            .map_err(|m| CallError::Internal(format!("KeyError: {:?}", m.0)))?;
        Ok(self.built(&text, keep))
    }

    fn summary_line(&self, needs: &[Need], typed: &str, terminal: bool) -> String {
        match confirm::summary(&self.redacted_needs(needs), typed) {
            Ok(t) => self.built(&t, terminal),
            Err(_) => "FERB needs your OK to go ahead with this step.".into(),
        }
    }

    /// `_set_card` (GATE-513, GATE-566).
    fn set_card(&self, ap: &mut Approval, needs: &[Need], typed: &str, terminal: bool) {
        match confirm::card(&self.redacted_needs(needs), typed, self.d.apps.as_ref()) {
            Ok(made) => {
                ap.summary = self.built(&made.question, false);
                ap.detail = self.built(&made.detail, false);
                ap.exact = made
                    .exact
                    .split('\n')
                    .filter(|l| !ferb_py::value::strip(l).is_empty())
                    .map(|l| self.built(l, terminal))
                    .collect::<Vec<_>>()
                    .join("\n");
                ap.labels = if made.yes.is_empty() {
                    Vec::new()
                } else {
                    vec![
                        self.built(&made.yes, false),
                        self.built(if made.no.is_empty() { NO } else { &made.no }, false),
                    ]
                };
            }
            Err(e) => {
                (self.d.journal_problem)(
                    "confirm.card_failed",
                    obj(
                        json!({"rules": needs.iter().map(|n| n.rule.clone()).collect::<Vec<_>>(), "error": format!("KeyError ({:?})", e.0)}),
                    ),
                );
                ap.summary = self.summary_line(needs, typed, terminal);
                ap.detail = String::new();
                ap.exact = String::new();
                ap.labels = Vec::new();
            }
        }
        if ap.summary.is_empty() {
            ap.summary = self.summary_line(needs, typed, terminal);
        }
    }

    fn tree_lock(&self, root: &str) -> Arc<tokio::sync::Mutex<()>> {
        lock(&self.inner)
            .confirming
            .entry(root.to_string())
            .or_default()
            .clone()
    }

    /// `_typed_into` (L3): the text this worker last typed into the page a send-declared input acts on. Display only.
    fn typed_into(
        &self,
        grant: &RunGrant,
        name: &str,
        arguments: &Map<String, Value>,
    ) -> R<String> {
        match arguments
            .get("tab")
            .and_then(Value::as_str)
            .filter(|t| !t.is_empty())
        {
            Some(tab) => Ok(self.spans(|s| s.last_typed(&grant.task_id, name, tab))?),
            None => Ok(String::new()),
        }
    }

    /// `_authorize`: under the tree's lock, each need's latest answer; a standing scope; an earlier Yes; the rest
    /// asked at once in one question.
    async fn authorize(&self, c: &mut Call, needs: Vec<Need>) -> R<Covered> {
        let mut out = Covered::default();
        if let Some(cid) = &c.pay_cid {
            out.ids.push(cid.clone());
        }
        if needs.is_empty() {
            return Ok(out);
        }
        let root = c.root();
        let redacted_args = self.redact(&Value::Object(c.arguments.clone()));
        let call = call_excerpt(
            &c.name,
            redacted_args.as_object().unwrap_or(&Map::new()),
            None,
        );
        let send_class = c
            .kinds()
            .iter()
            .any(|k| confirm::SEND_CLASS.contains(&k.as_str()));
        let typed = if send_class {
            self.typed_into(&c.grant, &c.name, &c.arguments)?
        } else {
            String::new()
        };
        let tree = self.tree_lock(&root);
        let _held = c.cancel.guard(tree.lock_owned()).await?;
        let result = self
            .authorize_held(c, &needs, &mut out, &root, &call, &typed)
            .await;
        if result.is_err() {
            for cid in &out.spent {
                let _ = self.confirmations(|s| s.release(cid));
            }
            out.spent.clear();
        }
        result?;
        if out.refused.is_some() {
            for cid in &out.spent {
                self.confirmations(|s| s.release(cid))?;
            }
            out.spent.clear();
        }
        Ok(out)
    }

    async fn authorize_held(
        &self,
        c: &mut Call,
        needs: &[Need],
        out: &mut Covered,
        root: &str,
        call: &str,
        typed: &str,
    ) -> R<()> {
        let mut ask: Vec<Need> = Vec::new();
        let mut approved: Option<Vec<String>> = None;
        let reopened = self.reopened(root)?;
        for need in needs {
            let row = self.find(root, &need.key, &reopened)?;
            if let Some(r) = &row {
                if !r.granted {
                    out.ids.push(r.id.clone());
                    out.refused = Some(obj(json!({
                        "error": fill(ALREADY_NO, &[("line", &self.ferb_line(needs, call, &c.why, typed)?)]),
                        "next": OWNER_DECIDED_NEXT
                    })));
                    out.declined = "no".into();
                    continue;
                }
            }
            if row.is_none() && STANDING_RULES.contains(&need.rule.as_str()) {
                let covers = {
                    let db = lock(&self.d.db);
                    StandingPermissions { db: &db }.covers(
                        root,
                        &c.name,
                        grant_identity(&c.grant),
                        &c.checked,
                        need.reaches.as_ref(),
                    )?
                };
                if covers {
                    out.standing.push(need.rule.clone());
                    continue;
                }
            }
            if row.is_none() || need.fresh {
                if !need.grants.is_empty() {
                    if let Some(wide) = self.find(root, &need.grants, &[])? {
                        if wide.granted {
                            out.ids.push(wide.id.clone());
                            out.set_row(&need.key, &wide.id);
                            out.wide.push(wide.id.clone());
                            continue;
                        }
                    }
                }
                ask.push(need.clone());
                continue;
            }
            let row = row.expect("checked");
            if row.used && need.always {
                if approved.is_none() {
                    approved = Some(self.approved_retry_rows(&c.grant, &c.ident)?);
                }
                let mut again = None;
                for cid in approved.as_ref().expect("set") {
                    if self.confirmations(|s| s.covers(cid, &need.key))? {
                        again = Some(cid.clone());
                        break;
                    }
                }
                match again {
                    None => ask.push(need.clone()),
                    Some(cid) => {
                        out.ids.push(cid.clone());
                        out.set_row(&need.key, &cid);
                    }
                }
                continue;
            }
            out.ids.push(row.id.clone());
            out.set_row(&need.key, &row.id);
            if need.always {
                self.confirmations(|s| s.use_(&row.id))?;
                if !need.spent_on_ask {
                    out.spent.push(row.id.clone());
                }
            }
        }
        if out.refused.is_none() && !ask.is_empty() {
            let picture = c.pay.is_some() && ask.iter().any(|n| Some(&n.key) == c.pay_key.as_ref());
            let grant = c.grant.clone();
            let rec = c.rec.clone();
            let answer = self
                .ask_confirmation(
                    &grant,
                    &rec,
                    &ask,
                    None,
                    call,
                    &c.why,
                    typed,
                    picture.then_some(&*c),
                    &c.cancel,
                    &c.left,
                )
                .await?;
            out.ids.extend(answer.ids.iter().cloned());
            if answer.granted {
                for (need, cid) in ask.iter().zip(answer.ids.iter()) {
                    out.set_row(&need.key, cid);
                    if need.always || need.spent_on_ask {
                        self.confirmations(|s| s.use_(cid))?;
                        if !need.spent_on_ask {
                            out.spent.push(cid.clone());
                        }
                    }
                    if let (Some(pay), true) =
                        (c.pay.as_ref(), Some(&need.key) == c.pay_key.as_ref())
                    {
                        lock(&self.payments).approve(root, pay, cid);
                        c.pay_cid = Some(cid.clone());
                    }
                }
            } else {
                let mut refused = obj(json!({"error": answer.error}));
                if !answer.next.is_empty() {
                    refused.insert("next".into(), Value::String(answer.next.clone()));
                }
                out.refused = Some(refused);
                out.declined = answer.declined.clone();
            }
        }
        Ok(())
    }

    /// `_approved_retry_rows` (SQ1 with P19): the rows the approved attempt used, which its identical retry may use
    /// once more.
    fn approved_retry_rows(&self, grant: &RunGrant, ident: &Ident) -> R<Vec<String>> {
        let Ident::Key(key, digest) = ident else {
            return Ok(Vec::new());
        };
        let Some(approved) = self.d.ledger.approved_retry(&grant.task_id, key, digest) else {
            return Ok(Vec::new());
        };
        let Some(row) = self.d.ledger.get(&approved) else {
            return Ok(Vec::new());
        };
        for r in &row.evidence {
            let found = self
                .spans(|s| s.tool_call(&row.task_id, r, ferb_store::spans::Evidence::default()))?;
            if let Some(Value::Array(ids)) = found
                .as_ref()
                .and_then(|f| f.get("input"))
                .and_then(|i| i.get("confirmations"))
            {
                return Ok(ids.iter().map(ferb_py::value::py_str).collect());
            }
        }
        Ok(Vec::new())
    }

    /// `_known_from` (SQ6, GATE-205): the destinations a Yes approved stay known for good.
    fn known_from(&self, needs: &[Need], rec: &TaskRecord, span_id: &str) -> R<()> {
        let mut seen: Vec<(String, String)> = Vec::new();
        for n in needs {
            for d in &n.destinations {
                if !seen.contains(d) {
                    seen.push(d.clone());
                }
            }
        }
        for (kind, value) in seen {
            self.destinations(|d| {
                d.record(
                    &kind,
                    &value,
                    "confirmed",
                    Some(&rec.request_id),
                    Some(span_id),
                )
            })?;
        }
        Ok(())
    }

    fn start_span(
        &self,
        kind: &str,
        input: &Value,
        task_id: &str,
        parent: Option<&str>,
    ) -> R<Span> {
        self.start_span_m(kind, input, task_id, parent, Marks::default())
    }

    /// `start_span` whose input holds FERB's own `marked` words (GATE-572).
    fn start_span_m(
        &self,
        kind: &str,
        input: &Value,
        task_id: &str,
        parent: Option<&str>,
        marked: Marks,
    ) -> R<Span> {
        Ok(self.spans_marked(marked, |s| {
            s.start(kind, input, Some(task_id), parent, Some(&self.d.device_id))
        })?)
    }

    /// `ask_confirmation`: FERB's own question to the owner, in chat with Yes / No (D33).
    #[allow(clippy::too_many_arguments)]
    async fn ask_confirmation(
        &self,
        grant: &RunGrant,
        rec: &TaskRecord,
        needs: &[Need],
        span: Option<&str>,
        call: &str,
        why: &str,
        typed: &str,
        picture: Option<&Call>,
        cancel: &Cancel,
        left: &Left,
    ) -> R<Answer> {
        let root = rec.tree_id.clone().unwrap_or_else(|| grant.task_id.clone());
        let text = self.ferb_line(needs, call, why, typed)?;
        let routine = needs.iter().all(confirm::routine);
        let mut options = vec![YES.to_string(), NO.to_string()];
        if routine {
            options.push(ALLOW_WINDOW.to_string());
        }
        let confirm_list: Vec<Value> = needs
            .iter()
            .map(|n| json!({"rule": n.rule, "key": n.key}))
            .collect();
        let mut input = obj(
            json!({"tool": OWNER_ASK, "args": {"question": text, "options": options, "confirm": confirm_list}, "ferb_question": true}),
        );
        if let Some(s) = span {
            input.insert("for_span".into(), Value::String(s.to_string()));
        }
        let mut q_span = self.start_span_m(
            "tool.call",
            &Value::Object(input),
            &grant.task_id,
            Some(&rec.span_id),
            Marks::of(&[&text]),
        )?;
        self.publish(
            "tool.called",
            obj(json!({"tool": OWNER_ASK, "task_id": grant.task_id, "worker_id": grant.worker_id})),
        );
        self.observer()
            .tool_started(grant, OWNER_ASK, &Map::new(), "owner_question", None);
        let unattended = lock(&self.inner)
            .approvals
            .unattended
            .contains(&rec.request_id);
        if unattended {
            let declined = obj(json!({"declined": true, "unattended": true}));
            self.finish_call(
                &mut q_span,
                OWNER_ASK,
                &declined,
                false,
                Some(&declined),
                None,
                Some(&ask_origins(&declined)),
            )?;
            return Ok(Answer {
                error: fill(UNATTENDED, &[("line", &text)]),
                next: NEEDS_OWNER_NEXT.into(),
                ..Answer::default()
            });
        }
        if cancel
            .guard(self.observer().correction_pending(grant))
            .await?
        {
            let first = obj(json!({"declined": true, "state": "correction pending"}));
            self.finish_call(
                &mut q_span,
                OWNER_ASK,
                &first,
                false,
                Some(&first),
                None,
                Some(&ask_origins(&first)),
            )?;
            return Ok(Answer {
                error: fill(CORRECTION_FIRST, &[("line", &text)]),
                next: CORRECTION_FIRST_NEXT.into(),
                ..Answer::default()
            });
        }
        let window = lock(&self.inner).allow_until;
        if routine && window.is_some_and(|until| self.now() < until) {
            let mut ids = Vec::new();
            for n in needs {
                ids.push(
                    self.confirmations(|s| {
                        s.record(&root, &n.rule, &n.key, true, Some(&q_span.id))
                    })?,
                );
            }
            self.known_from(needs, rec, &q_span.id)?;
            let given = obj(
                json!({"answer": YES, "confirmed": ids, "allowed_until": when::isoformat(&window.expect("checked"))}),
            );
            self.finish_call(
                &mut q_span,
                OWNER_ASK,
                &given,
                false,
                Some(&given),
                None,
                Some(&ask_origins(&given)),
            )?;
            return Ok(Answer {
                granted: true,
                ids,
                ..Answer::default()
            });
        }
        let shot = match picture {
            Some(c) => self.pay_picture(c).await?,
            None => None,
        };
        let asker = Asker {
            capability: CONFIRM_ASK,
            task_id: &grant.task_id,
            request_id: &rec.request_id,
        };
        let (ap_id, new) = self.question(&asker, &text, &options, &text);
        if new {
            let shown_typed = if ferb_py::value::strip(typed).is_empty() {
                String::new()
            } else {
                quoted(&self.value(typed), REASON_CHARS)
            };
            let terminal = needs.iter().any(|n| n.rule == confirm::TERMINAL_INPUT);
            let mut ap = self.approval(&ap_id).expect("just asked");
            ap.corrects = true;
            self.set_card(&mut ap, needs, &shown_typed, terminal);
            if !ap.labels.is_empty() && routine {
                ap.labels.push(ALLOW_WINDOW_LABEL.into());
                ap.note = ALLOW_WINDOW_NOTE.into();
            }
            let why_q = if ferb_py::value::strip(why).is_empty() {
                String::new()
            } else {
                quoted(&self.value(why), REASON_CHARS)
            };
            ap.why = self.built(&confirm::reason_line(&why_q), false);
            if picture.is_some() {
                if let Some(bytes) = &shot {
                    if !bytes.is_empty() {
                        lock(&self.inner)
                            .pictures
                            .hold(&ap.id, &root, bytes.clone());
                    }
                }
                ap.picture = if shot.as_ref().is_some_and(|b| !b.is_empty()) {
                    "shown".into()
                } else {
                    NOT_CAPTURED.into()
                };
            } else if needs.iter().any(|n| n.rule == confirm::ALWAYS_PAY) {
                ap.picture = NOT_CAPTURED.into();
            }
            let mut event = obj(json!({
                "approval_id": ap.id, "capability": CONFIRM_ASK, "kind": "owner_question", "reason": text,
                "options": options, "task_id": grant.task_id, "summary": ap.summary, "why": ap.why
            }));
            event.extend(card_parts(&ap));
            event.extend(card_flags(&ap));
            // FERB's own built words keep their mark (`built`); the exact part is joined from them (unmarked)
            let mut built = Marks::of(&[&text, &ap.summary, &ap.why, &ap.detail]);
            for l in ap.labels.iter().take(2) {
                built.add(l);
            }
            self.store_approval(ap);
            self.publish_marked("approval.requested", event, &built);
        } else {
            self.with_approval(&ap_id, |ap| ap.corrects = true);
        }
        self.observer().tool_call(grant, false);
        let state = self.wait_answer(grant, &ap_id, cancel, left).await;
        self.observer().tool_call(grant, true);
        lock(&self.inner).pictures.drop_picture(&ap_id);
        let state = match state {
            Ok(s) => s,
            Err(e) => {
                let stopped = obj(json!({"declined": true, "state": "cancelled"}));
                self.finish_call(
                    &mut q_span,
                    OWNER_ASK,
                    &stopped,
                    false,
                    Some(&stopped),
                    None,
                    Some(&ask_origins(&stopped)),
                )?;
                return Err(e);
            }
        };
        let choice = self.approval(&ap_id).and_then(|a| a.choice);
        let yes = state == "granted"
            && (choice.as_deref() == Some(YES) || choice.as_deref() == Some(ALLOW_WINDOW));
        if state == "granted" && choice.as_deref() == Some(ALLOW_WINDOW) && routine {
            let until = self.now() + chrono::Duration::seconds(confirm::ALLOW_WINDOW_S as i64);
            lock(&self.inner).allow_until = Some(until);
            (self.d.journal_problem)(
                "confirm.allow_window",
                obj(json!({"until": when::isoformat(&until)})),
            );
        }
        if state == "superseded" {
            let aside = obj(json!({"declined": true, "state": state}));
            self.finish_call(
                &mut q_span,
                OWNER_ASK,
                &aside,
                false,
                Some(&aside),
                None,
                Some(&ask_origins(&aside)),
            )?;
            return Ok(Answer {
                error: fill(SUPERSEDED, &[("line", &text)]),
                next: SUPERSEDED_NEXT.into(),
                declined: "words".into(),
                ..Answer::default()
            });
        }
        if !yes && !((state == "granted" && choice.as_deref() == Some(NO)) || state == "denied") {
            let shown_state = if state != "granted" {
                state.clone()
            } else {
                "neither Yes nor No".into()
            };
            let unclear = obj(json!({"declined": true, "state": shown_state}));
            self.finish_call(
                &mut q_span,
                OWNER_ASK,
                &unclear,
                false,
                Some(&unclear),
                None,
                Some(&ask_origins(&unclear)),
            )?;
            return Ok(Answer {
                error: fill(NO_ANSWER, &[("line", &text)]),
                ..Answer::default()
            });
        }
        let mut ids = Vec::new();
        for n in needs {
            ids.push(
                self.confirmations(|s| s.record(&root, &n.rule, &n.key, yes, Some(&q_span.id)))?,
            );
        }
        if yes {
            self.known_from(needs, rec, &q_span.id)?;
            let mut wider: Vec<String> = Vec::new();
            for n in needs {
                if !n.grants.is_empty() && !wider.contains(&n.grants) {
                    wider.push(n.grants.clone());
                }
            }
            for w in wider {
                self.confirmations(|s| {
                    s.record(&root, confirm::TERMINAL_APP, &w, true, Some(&q_span.id))
                })?;
            }
        }
        let mut given = obj(json!({"answer": if yes { YES } else { NO }}));
        given.insert(if yes { "confirmed" } else { "refused" }.into(), json!(ids));
        if state == "denied" {
            given.insert("skipped".into(), Value::Bool(true));
        }
        self.finish_call(
            &mut q_span,
            OWNER_ASK,
            &given,
            false,
            Some(&given),
            None,
            Some(&ask_origins(&given)),
        )?;
        Ok(Answer {
            granted: yes,
            ids,
            error: if yes {
                String::new()
            } else {
                fill(SAID_NO, &[("line", &text)])
            },
            next: if yes {
                String::new()
            } else {
                OWNER_DECIDED_NEXT.into()
            },
            declined: if yes { String::new() } else { "no".into() },
        })
    }

    fn store_approval(&self, ap: Approval) {
        let mut i = lock(&self.inner);
        if let Some(slot) = i.approvals.get_mut(&ap.id) {
            *slot = ap;
        }
    }

    fn with_approval(&self, id: &str, f: impl FnOnce(&mut Approval)) {
        if let Some(ap) = lock(&self.inner).approvals.get_mut(id) {
            f(ap);
        }
    }

    /// `_reusable`: the question this task asked with this capability and key that a dropped call left untaken.
    fn reusable(
        &self,
        request_id: &str,
        task_id: &str,
        capability: &str,
        key: &str,
    ) -> Option<Approval> {
        let i = lock(&self.inner);
        i.approvals
            .of_request(request_id)
            .into_iter()
            .find(|ap| {
                ap.task_id == task_id
                    && ap.capability == capability
                    && ap.key == key
                    && !ap.taken
                    && matches!(ap.state, State::Pending | State::Granted | State::Denied)
                    && i.waiting.get(&ap.id).copied().unwrap_or(0) == 0
            })
            .cloned()
    }

    /// `_question`: the question a dropped call of the same request left untaken, else a new one (true: new).
    fn question(
        &self,
        asker: &Asker<'_>,
        text: &str,
        options: &[String],
        key: &str,
    ) -> (String, bool) {
        if let Some(ap) = self.reusable(asker.request_id, asker.task_id, asker.capability, key) {
            return (ap.id, false);
        }
        let mut i = lock(&self.inner);
        let ap = i.approvals.question(asker, text, options, None);
        ap.key = key.to_string();
        (ap.id.clone(), true)
    }

    fn task_stopped(&self, task_id: &str) -> bool {
        let i = lock(&self.inner);
        i.stopped || i.cancelled.contains(task_id)
    }

    /// `_wait_answer`: the question's state once answered; "gone" when it closed with its task; CALLER_LEFT when the
    /// call lost its caller first. Cancelled (a stop): a really stopped task's question closes as superseded.
    async fn wait_answer(
        &self,
        grant: &RunGrant,
        id: &str,
        cancel: &Cancel,
        left: &Left,
    ) -> R<String> {
        let observer = self.observer();
        observer.waiting(&grant.worker_id, true);
        *lock(&self.inner).waiting.entry(id.to_string()).or_insert(0) += 1;
        let out = loop {
            let n = self.answers.notified();
            tokio::pin!(n);
            n.as_mut().enable();
            {
                let mut i = lock(&self.inner);
                match i.approvals.get_mut(id) {
                    None => break Ok("gone".to_string()),
                    Some(_) if left() => break Ok(CALLER_LEFT.to_string()),
                    Some(ap) if ap.state != State::Pending => {
                        ap.taken = true;
                        break Ok(ap.state.as_str().to_string());
                    }
                    Some(_) => {}
                }
            }
            let waited = cancel
                .guard(async {
                    tokio::select! {
                        () = &mut n => {},
                        () = tokio::time::sleep(std::time::Duration::from_millis(200)) => {},
                    }
                })
                .await;
            if let Err(e) = waited {
                if self.task_stopped(&grant.task_id)
                    && lock(&self.inner).approvals.supersede(id).is_some()
                {
                    self.answers.notify_waiters();
                    self.publish(
                        "approval.resolved",
                        obj(json!({"approval_id": id, "state": "superseded"})),
                    );
                }
                break Err(e);
            }
        };
        {
            let mut i = lock(&self.inner);
            if let Some(n) = i.waiting.get_mut(id) {
                *n -= 1;
                if *n == 0 {
                    i.waiting.remove(id);
                }
            }
        }
        observer.waiting(&grant.worker_id, false);
        out
    }

    /// GATE-579: the payment page's one picture for the owner, taken under the call's locks (as a reading call).
    async fn pay_picture(&self, c: &Call) -> R<Option<Vec<u8>>> {
        let take = async {
            let held = self
                .locks
                .hold(&c.lock_keys, &format!("picture:{}", c.root()), false)
                .await
                .ok()?;
            let shot = c.tool.pay_picture(&c.args, &c.grant.task_id).await;
            drop(held);
            shot.filter(|b| !b.is_empty())
        };
        let limited = tokio::time::timeout(
            std::time::Duration::from_secs_f64(crate::pay_picture::PICTURE_LIMIT_S),
            take,
        );
        Ok(c.cancel.guard(limited).await?.ok().flatten())
    }
}

// ---------------------------------------------------------------------------------------------------------------
// owner.ask and P19's retry question
// ---------------------------------------------------------------------------------------------------------------

impl Gateway {
    /// `_ask_owner`: the worker's question goes to chat (or a retry question, FERB's own); the call waits.
    async fn ask_owner(
        &self,
        grant: &RunGrant,
        rec: &TaskRecord,
        args: &Args,
        cancel: &Cancel,
        left: &Left,
    ) -> R<Map<String, Value>> {
        let mut question = args
            .get("question")
            .map(ferb_py::value::py_str)
            .unwrap_or_default();
        let asked = question.clone();
        let mut options: Vec<String> = args
            .get("options")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .map(ferb_py::value::py_str)
                    .filter(|o| !ferb_py::value::strip(o).is_empty())
                    .collect()
            })
            .unwrap_or_default();
        let mut retry = ferb_py::value::strip(
            &args
                .get("retry_action")
                .filter(|v| ferb_py::value::truthy(v))
                .map(ferb_py::value::py_str)
                .unwrap_or_default(),
        )
        .to_string();
        let mut key = ferb_py::json::dumps_utf8(&json!({"question": question, "options": options}));
        let worker_options = options.clone();
        let reused = if retry.is_empty() {
            self.reusable(&rec.request_id, &grant.task_id, OWNER_ASK, &key)
        } else {
            None
        };
        let mut asked_about: Vec<String> = Vec::new();
        let mut attempts: Vec<LedgerRow> = Vec::new();
        let mut retry_said = String::new();
        if let Some(r) = &reused {
            question = r.reason.clone();
            options = r.options.clone();
        } else if !retry.is_empty() {
            let (row, all) = match self.retry_row(grant, rec, &retry) {
                Ok(found) => found,
                Err(why) => return Ok(obj(json!({"error": why}))),
            };
            retry = row.id.clone();
            asked_about = all.iter().map(|a| a.id.clone()).collect();
            let (tool, call, action) = self.earlier_call(&row)?;
            let line = retry_line(&row, &all, &call, &tool);
            let ferb_line = printable(&self.value(&line));
            retry_said = self.built(&retry_question(&self.value(&action)), false);
            question = format!(
                "{ferb_line}\n\nWhat the task found: \"{}\".",
                quoted(&self.value(&question), REASON_CHARS)
            );
            options = vec![RETRY_YES.into(), RETRY_NO.into()];
            key =
                ferb_py::json::dumps_utf8(&json!({"retry_question": question, "options": options}));
            attempts = all;
        } else if let Some(router) = &self.d.route_question {
            let observer = self.observer();
            observer.waiting(&grant.worker_id, true);
            let decision = cancel
                .guard(router.route(&grant.task_id, &question, &options))
                .await;
            observer.waiting(&grant.worker_id, false);
            let decision = match decision? {
                Ok(d) => d,
                Err(_) => {
                    return Ok(obj(
                        json!({"error": "The orchestrator could not resolve the question. Report this capability gap."}),
                    ))
                }
            };
            if !decision
                .get("ask_owner")
                .is_some_and(ferb_py::value::truthy)
            {
                return Ok(obj(
                    json!({"answer": decision.get("answer").cloned().unwrap_or(Value::Null), "answered_by": "orchestrator"}),
                ));
            }
            if let Some(q) = decision
                .get("question")
                .filter(|q| ferb_py::value::truthy(q))
            {
                question = ferb_py::value::py_str(q);
            }
        }
        let given_options = if reused.is_some() {
            worker_options.clone()
        } else {
            options.clone()
        };
        let from_task = retry.is_empty() && reused.as_ref().is_none_or(|r| r.from_task);
        if retry.is_empty() && reused.is_none() {
            question = confirm::task_question(&printable(&question));
            options = options
                .iter()
                .map(|o| confirm::from_task(&py_split_join(&printable(o))))
                .collect();
        }
        let mut args_in = obj(json!({"question": question, "options": options}));
        if !retry.is_empty() {
            args_in.insert("retry_action".into(), Value::String(retry.clone()));
        }
        // a retry question is FERB's own line, marked as built (`_marked`); a task's question is its words
        let own = if retry.is_empty() {
            Marks::default()
        } else {
            Marks::of(&[&question])
        };
        let mut span = self.start_span_m(
            "tool.call",
            &json!({"tool": OWNER_ASK, "args": args_in}),
            &grant.task_id,
            Some(&rec.span_id),
            own.clone(),
        )?;
        self.publish(
            "tool.called",
            obj(json!({"tool": OWNER_ASK, "task_id": grant.task_id, "worker_id": grant.worker_id})),
        );
        self.observer()
            .tool_started(grant, OWNER_ASK, &Map::new(), "owner_question", None);
        let unattended = lock(&self.inner)
            .approvals
            .unattended
            .contains(&rec.request_id);
        if unattended {
            let declined = obj(json!({"declined": true, "unattended": true}));
            self.finish_call(
                &mut span,
                OWNER_ASK,
                &declined,
                false,
                Some(&declined),
                None,
                Some(&ask_origins(&declined)),
            )?;
            return Ok(obj(
                json!({"declined": true, "reason": "the owner isn't here (a scheduled run): decide yourself or report what's missing"}),
            ));
        }
        if reused.is_none()
            && cancel
                .guard(self.observer().correction_pending(grant))
                .await?
        {
            let first = obj(json!({"declined": true, "state": "correction pending"}));
            self.finish_call(
                &mut span,
                OWNER_ASK,
                &first,
                false,
                Some(&first),
                None,
                Some(&ask_origins(&first)),
            )?;
            return Ok(obj(
                json!({"declined": true, "error": fill(CORRECTION_FIRST, &[("line", "your question")]), "next": CORRECTION_FIRST_NEXT}),
            ));
        }
        let capability = if retry.is_empty() {
            OWNER_ASK
        } else {
            RETRY_ASK
        };
        let (ap_id, new) = match &reused {
            Some(r) => (r.id.clone(), false),
            None => {
                let asker = Asker {
                    capability,
                    task_id: &grant.task_id,
                    request_id: &rec.request_id,
                };
                self.question(&asker, &question, &options, &key)
            }
        };
        if !retry.is_empty() {
            if new {
                let about = asked_about.clone();
                self.with_approval(&ap_id, |ap| ap.about = about);
            }
            asked_about = self.approval(&ap_id).map(|a| a.about).unwrap_or_default();
        }
        if new {
            let mut ap = self.approval(&ap_id).expect("just asked");
            ap.summary = if retry.is_empty() {
                String::new()
            } else {
                retry_said.clone()
            };
            ap.from_task = from_task;
            ap.corrects = !retry.is_empty();
            if !retry.is_empty() {
                ap.detail = self.built(&retry_detail(&attempts), false);
                ap.why = if ferb_py::value::strip(&asked).is_empty() {
                    String::new()
                } else {
                    self.built(
                        &confirm::reason_line(&quoted(&self.value(&asked), REASON_CHARS)),
                        false,
                    )
                };
                ap.labels = RETRY_LABELS.iter().map(|s| s.to_string()).collect();
            }
            let mut event = obj(
                json!({"approval_id": ap.id, "capability": capability, "kind": "owner_question", "reason": question, "options": options, "task_id": grant.task_id}),
            );
            if !ap.summary.is_empty() {
                event.insert("summary".into(), Value::String(ap.summary.clone()));
            }
            if from_task {
                event.insert("from_task".into(), Value::Bool(true));
            }
            if !ap.why.is_empty() {
                event.insert("why".into(), Value::String(ap.why.clone()));
            }
            event.extend(card_parts(&ap));
            event.extend(card_flags(&ap));
            let mut built = own.clone();
            if !retry.is_empty() {
                for t in [&ap.summary, &ap.detail, &ap.why] {
                    built.add(t);
                }
            }
            self.store_approval(ap);
            self.publish_marked("approval.requested", event, &built);
        }
        self.observer().tool_call(grant, false);
        let answer = self.wait_answer(grant, &ap_id, cancel, left).await;
        self.observer().tool_call(grant, true);
        let answer = answer?;
        if answer != "granted" {
            let refused = obj(json!({"declined": true, "state": answer}));
            self.finish_call(
                &mut span,
                OWNER_ASK,
                &refused,
                false,
                Some(&refused),
                None,
                Some(&ask_origins(&refused)),
            )?;
            return Ok(obj(json!({"declined": true})));
        }
        let ap = self.approval(&ap_id).expect("answered");
        let mut text = ap
            .choice
            .clone()
            .filter(|c| !c.is_empty())
            .or_else(|| ap.answer.clone().filter(|a| !a.is_empty()))
            .unwrap_or_default();
        if let Some(choice) = &ap.choice {
            if let Some(i) = options.iter().position(|o| o == choice) {
                text = given_options.get(i).cloned().unwrap_or(text);
            }
        }
        for (kind, value) in dest::from_words(&text) {
            self.destinations(|d| {
                d.record(
                    &kind,
                    &value,
                    "answered",
                    Some(&rec.request_id),
                    Some(&span.id),
                )
            })?;
        }
        let mut given = obj(json!({"answer": text}));
        if !ap.hidden.is_empty() {
            let kinds: Vec<&str> = ap.hidden.iter().map(String::as_str).collect();
            given.insert(
                "ferb_note".into(),
                Value::String(ferb_sanitize::formats::hidden_note(&kinds, "answer")),
            );
        }
        if !retry.is_empty() {
            let settled = if ap.choice.as_deref() == Some(RETRY_YES) {
                self.settle_retry(&retry, &span.id, &asked_about)
            } else {
                obj(json!({"retry_approved": false}))
            };
            given.extend(settled);
        }
        self.finish_call(
            &mut span,
            OWNER_ASK,
            &given,
            false,
            Some(&given),
            None,
            Some(&ask_origins(&given)),
        )?;
        Ok(given)
    }

    /// `_retry_row`: the latest unknown attempt at the action `ref` names in this tree, and every attempt.
    fn retry_row(
        &self,
        grant: &RunGrant,
        rec: &TaskRecord,
        r: &str,
    ) -> Result<(LedgerRow, Vec<LedgerRow>), String> {
        let row = self.d.ledger.get(r);
        let tree = rec.tree_id.clone().unwrap_or_else(|| grant.task_id.clone());
        let tree = tree.split('.').next().unwrap_or(&tree).to_string();
        let Some(row) =
            row.filter(|row| row.task_id == tree || row.task_id.starts_with(&format!("{tree}.")))
        else {
            return Err("retry_action doesn't name an earlier attempt in this task; use the value FERB's refusal returned".into());
        };
        let attempts = self.d.ledger.attempts(r);
        if attempts
            .iter()
            .any(|a| a.state == "intended" || a.state == "executing")
        {
            return Err(
                "that action is still running; wait for its result before asking about a retry"
                    .into(),
            );
        }
        let unknown: Vec<&LedgerRow> = attempts.iter().filter(|a| a.state == "unknown").collect();
        let Some(latest) = unknown.last() else {
            return Err(format!(
                "that earlier attempt is no longer unknown: it {}; there is nothing to approve",
                ended(&row.state)
            ));
        };
        if latest.retry_approved.is_some() && latest.retried_by.is_none() {
            return Err(
                "the owner already approved one more attempt at this action; it can run now".into(),
            );
        }
        Ok(((*latest).clone(), attempts.clone()))
    }

    /// `_earlier_call`: the tool the attempt really called, what its call did, and the action in plain words.
    fn earlier_call(&self, row: &LedgerRow) -> R<(String, String, String)> {
        let site = row
            .target
            .get("site")
            .filter(|v| ferb_py::value::truthy(v))
            .map(ferb_py::value::py_str);
        for r in &row.evidence {
            let found = self
                .spans(|s| s.tool_call(&row.task_id, r, ferb_store::spans::Evidence::default()))?;
            let Some(found) = found else { continue };
            let input = found.get("input").cloned().unwrap_or(Value::Null);
            let tool = input
                .get("tool")
                .filter(|v| ferb_py::value::truthy(v))
                .map(ferb_py::value::py_str)
                .unwrap_or_default();
            let key = self
                .registry()
                .get(&tool)
                .map(|t| t.facts().ledger_key.unwrap_or_else(|| tool.clone()))
                .unwrap_or_else(|| tool.clone());
            if key == row.capability {
                let args = input
                    .get("args")
                    .and_then(Value::as_object)
                    .cloned()
                    .unwrap_or_default();
                let kind = input
                    .get("kind")
                    .filter(|v| ferb_py::value::truthy(v))
                    .map(ferb_py::value::py_str)
                    .unwrap_or_default();
                let app = tool.starts_with("mac.").then(|| {
                    let given = site.clone().filter(|s| !s.is_empty()).unwrap_or_else(|| {
                        args.get("app")
                            .filter(|v| ferb_py::value::truthy(v))
                            .map(ferb_py::value::py_str)
                            .unwrap_or_default()
                    });
                    self.d.describe.app_name(&given)
                });
                let said =
                    self.d
                        .describe
                        .describe(&tool, &args, &kind, site.as_deref(), app.as_deref());
                let error = found.get("error").and_then(Value::as_str);
                return Ok((tool.clone(), call_excerpt(&tool, &args, error), said));
            }
        }
        let path = row
            .target
            .get("path")
            .filter(|v| ferb_py::value::truthy(v))
            .map(ferb_py::value::py_str)
            .unwrap_or_default();
        let said = self.d.describe.describe(
            &row.capability,
            &obj(json!({"path": path})),
            "",
            site.as_deref(),
            None,
        );
        Ok((String::new(), String::new(), said))
    }

    /// `_settle_retry`: the owner chose Yes: one more attempt may run, if the question still names every attempt.
    fn settle_retry(&self, r: &str, ask_span: &str, asked_about: &[String]) -> Map<String, Value> {
        let now: Vec<String> = self
            .d
            .ledger
            .attempts(r)
            .into_iter()
            .map(|a| a.id)
            .collect();
        if now != asked_about {
            return obj(json!({"retry_approved": false, "why_not": RETRY_STALE}));
        }
        if self.d.ledger.approve_retry(r, ask_span).is_err() {
            let state = self
                .d
                .ledger
                .get(r)
                .map(|row| row.state)
                .unwrap_or_default();
            return obj(json!({"retry_approved": state == "failed", "earlier_attempt": state}));
        }
        obj(json!({"retry_approved": true}))
    }

    /// `reopen_noes` (L11 round 4): the owner's own new message carried the task on; each latest No is reopened.
    pub fn reopen_noes(&self, tree_id: &str, request_id: &str, source_ref: &str) -> R<Vec<String>> {
        let ids = self.confirmations(|c| c.noes(tree_id))?;
        if !ids.is_empty() {
            let mut span = self.spans(|s| {
                s.start(
                    "confirmations.reopened",
                    &json!({"request_id": request_id, "source_ref": source_ref, "reopened": ids}),
                    Some(tree_id),
                    None,
                    None,
                )
            })?;
            let output = json!({"reopened": ids.len()});
            self.spans(|s| {
                s.finish(
                    &mut span,
                    Finish {
                        output: Some(&output),
                        origin: Some(origin::OWNER),
                        ..Finish::default()
                    },
                )
            })?;
        }
        Ok(ids)
    }
}

// ---------------------------------------------------------------------------------------------------------------
// GATE-580, the grants, and running it
// ---------------------------------------------------------------------------------------------------------------

/// `aftercheck.facts_of` and `after_of` (V2): the acting tool's per-action facts as one after block.
fn after_block(content: &Map<String, Value>) -> Value {
    let facts = match content.get("facts") {
        Some(Value::Object(m)) => m,
        _ => content,
    };
    let note = facts
        .get("note")
        .map(ferb_py::value::py_str)
        .unwrap_or_default();
    let value = facts.get("value").cloned().unwrap_or(Value::Null);
    if facts.contains_key("value_matches") || facts.get("method") == Some(&json!("ax_value")) {
        let matched = match facts.get("value_matches") {
            Some(Value::Null) | None => facts.get("accepted").cloned().unwrap_or(Value::Null),
            Some(m) => m.clone(),
        };
        if matched.is_null() {
            return json!({"read": "none", "changed": null, "value": value, "note": note});
        }
        return json!({"read": "value", "changed": ferb_py::value::truthy(&matched), "value": value, "note": note});
    }
    if let Some(observed) = facts.get("observed_change") {
        let page = facts.get("observed").is_some_and(Value::is_string);
        let changed = if ferb_py::value::truthy(observed) {
            json!(true)
        } else {
            Value::Null
        };
        let note = if page && note.is_empty() {
            facts["observed"].as_str().unwrap_or("").to_string()
        } else {
            note
        };
        return json!({"read": if page { "page" } else { "ax" }, "changed": changed, "value": value, "note": note});
    }
    if let Some(Value::Bool(b)) = facts.get("bytes_match") {
        return json!({"read": "value", "changed": b, "value": null, "note": note});
    }
    json!({"read": "none", "changed": null, "value": null, "note": note})
}

/// The tool's content redacted, its own measured numbers kept (`ToolOutput::measured`). Only a JSON number at a
/// measured place is kept: it is code's own measurement (a size, a time, an inode), never text read from outside.
fn redact_measured(
    redactor: &Redactor,
    content: &Map<String, Value>,
    measured: &[String],
) -> Map<String, Value> {
    let mut held: Vec<(String, Value)> = Vec::new();
    let mut work = Value::Object(content.clone());
    for p in measured {
        if let Some(slot) = work.pointer_mut(p) {
            if slot.is_number() {
                held.push((p.clone(), std::mem::replace(slot, Value::Null)));
            }
        }
    }
    let mut clean = redactor.value(&work);
    for (p, v) in held {
        if let Some(slot) = clean.pointer_mut(&p) {
            *slot = v;
        }
    }
    obj(clean)
}

impl Gateway {
    /// `_pay_pressed` (GATE-580): a covered press is compared just before it goes with the page the owner was asked
    /// about; changed, the owner is asked again (or told, when already asked about that change).
    async fn pay_pressed(&self, c: &mut Call, mut covered: Covered) -> R<Covered> {
        if covered.refused.is_some() || c.pay.is_none() || c.pay_cid.is_none() {
            return Ok(covered);
        }
        let root = c.root();
        let pay = c.pay.clone().expect("checked");
        let a = lock(&self.payments).active(&root, &pay.chain);
        let Some(a) =
            a.filter(|a| Some(&a.cid) == c.pay_cid.as_ref() && payments::rechecks(a, &pay))
        else {
            return Ok(covered);
        };
        let here = c
            .pay_key
            .as_ref()
            .is_some_and(|k| covered.row(k) == c.pay_cid.as_deref());
        let result: R<Option<Covered>> = async {
            let read = async {
                let held = self.locks.hold(&c.lock_keys, &format!("press:{root}"), false).await.ok()?;
                let now = c.tool.pay_press(&c.args, &c.grant.task_id, a.declared.as_deref()).await;
                drop(held);
                now
            };
            let now = c.cancel.guard(tokio::time::timeout(std::time::Duration::from_secs(10), read)).await?;
            let now = match now {
                Ok(n) => n,
                Err(_) => Some(payments::Pressed { url: None, amounts: None }),
            };
            let change = payments::pressed_change(&a, &pay, now.as_ref(), here).ok().flatten();
            let (Some(change), Some(now)) = (change, now) else { return Ok(None) };
            c.extra.insert("payment_changed".into(), json!({"what": change, "approved": a.cid}));
            let what = if change == "page" {
                PAY_CHANGE_PAGE.to_string()
            } else {
                fill(PAY_CHANGE_AMOUNT, &[("amount", a.declared.as_deref().unwrap_or(""))])
            };
            let mut amounts = now.amounts.clone().unwrap_or_default();
            amounts.sort();
            let state = vec![json!(change), json!(now.url), json!(amounts)];
            if !lock(&self.payments).first_change(&root, &pay.chain, &state) {
                let mut cv = covered.clone();
                cv.refused = Some(obj(json!({"error": fill(PAY_CHANGED_AGAIN, &[("what", &what)]), "next": PAY_CHANGED_NEXT})));
                return Ok(Some(cv));
            }
            lock(&self.payments).set_aside(&root, &pay.chain);
            let mut p = pay.clone();
            p.shown = now.amounts.clone().unwrap_or_default();
            c.pay = Some(p.clone());
            let check = lock(&self.payments).check(&root, &p, &c.pay_values);
            let need = check.need.expect("set aside: it asks");
            c.pay_cid = None;
            c.pay_key = Some(need.key.clone());
            let again = Box::pin(self.authorize(c, vec![need])).await?;
            let mut cv = covered.clone();
            cv.ids.extend(again.ids);
            for (k, v) in again.rows {
                cv.set_row(&k, &v);
            }
            cv.spent.extend(again.spent);
            cv.refused = again.refused;
            Ok(Some(cv))
        }
        .await;
        match result {
            Ok(Some(cv)) => Ok(cv),
            Ok(None) => Ok(covered),
            Err(e) => {
                for cid in &covered.spent {
                    let _ = self.confirmations(|s| s.release(cid));
                }
                covered.spent.clear();
                Err(e)
            }
        }
    }

    fn granted_row(&self, c: &Call, key: &str) -> R<Option<String>> {
        let root = c.root();
        let reopened = self.reopened(&root)?;
        Ok(self
            .find(&root, key, &reopened)?
            .filter(|r| r.granted)
            .map(|r| r.id))
    }

    /// `_grants`: what the owner's answers let this call do: lifted rules, the escape's grant, the terminal's.
    #[allow(clippy::type_complexity)]
    async fn grants(
        &self,
        c: &mut Call,
        covered: &Covered,
    ) -> R<
        Result<
            (
                BTreeSet<(String, String)>,
                Option<EscapeGrant>,
                Option<TerminalGrant>,
            ),
            Map<String, Value>,
        >,
    > {
        let lifted: BTreeSet<(String, String)> = c.lifts.iter().map(|(k, _)| k.clone()).collect();
        let mut escape = None;
        if let (Some((rule, command, cwd, _)), Some(need)) =
            (c.escape.clone(), c.escape_need.clone())
        {
            let cid = match covered.row(&need.key) {
                Some(cid) => Some(cid.to_string()),
                None => self.granted_row(c, &need.key)?,
            };
            // the Mac password for an admin command, asked only after the Yes (hidden input): it goes only to the
            // command (the escape grant), never to a span, a reply or the model
            let secret = if rule == "admin" {
                let given = self
                    .admin_password(&c.grant, &c.rec, &command, &c.cancel, &c.left)
                    .await?;
                match given {
                    Some(s) => Some(s),
                    None => {
                        return Ok(Err(obj(
                            json!({"error": "the approved admin command was cancelled without a password"}),
                        )))
                    }
                }
            } else {
                None
            };
            escape = Some(EscapeGrant {
                rule,
                confirmation_id: cid.unwrap_or_default(),
                command,
                cwd,
                secret,
            });
        }
        let mut terminal = None;
        if let Some(inp) = c.terminal.clone() {
            if let Some(cid) = inp.covered_by.clone() {
                terminal = Some(TerminalGrant {
                    input: inp,
                    confirmation_id: cid,
                });
            } else if let Some(need) = &c.terminal_need {
                if let Some(cid) = covered.row(&need.key).map(str::to_string) {
                    let mut rec = obj(json!({"confirmation": cid}));
                    if covered.wide.contains(&cid) {
                        rec.insert("covers".into(), Value::String("app".into()));
                    }
                    c.extra.insert("terminal".into(), Value::Object(rec));
                    terminal = Some(TerminalGrant {
                        input: inp,
                        confirmation_id: cid,
                    });
                }
            }
        }
        if !lifted.is_empty() {
            let mut shown: Vec<String> = lifted
                .iter()
                .map(|(rule, how)| format!("{how}:{rule}"))
                .collect();
            shown.sort();
            c.extra.insert("lifted".into(), json!(shown));
        }
        Ok(Ok((lifted, escape, terminal)))
    }

    /// `_call_span(c, **more)`.
    fn call_span(&self, c: &Call, more: &[(&str, Value)]) -> R<Span> {
        let mut input = obj(
            json!({"tool": c.name, "args": c.arguments, "effect": c.effect, "kind": c.spec.kind, "target": target_dict(&c.checked.target)}),
        );
        if !c.why.is_empty() {
            input.insert("why".into(), Value::String(c.why.clone()));
        }
        for (k, v) in &c.extra {
            input.insert(k.clone(), v.clone());
        }
        for (k, v) in more {
            input.insert((*k).to_string(), v.clone());
        }
        self.start_span(
            "tool.call",
            &Value::Object(input),
            &c.grant.task_id,
            Some(&c.rec.span_id),
        )
    }

    /// `_refuse_call`: a call that doesn't run: its span says so, nothing ran.
    fn refuse_call(
        &self,
        c: &Call,
        refused: Map<String, Value>,
        origins: Option<BTreeSet<String>>,
    ) -> R<Reply> {
        let mut span = self.call_span(c, &[("boundary_refused", Value::Bool(true))])?;
        let error = refused
            .get("error")
            .map(ferb_py::value::py_str)
            .unwrap_or_default();
        self.finish_call(&mut span, &c.name, &refused, true, None, Some(&error), None)?;
        let o = origins.unwrap_or_else(ferb_origin);
        Ok(self.reply(Some(&c.rec), refused, true, Some(&o), false))
    }

    /// `_finish_call`: the span finished with its one-line summary; `tool.finished` published.
    #[allow(clippy::too_many_arguments)]
    fn finish_call(
        &self,
        span: &mut Span,
        name: &str,
        content: &Map<String, Value>,
        is_error: bool,
        output: Option<&Map<String, Value>>,
        error: Option<&str>,
        origins: Option<&BTreeSet<String>>,
    ) -> R<()> {
        self.finish_call_m(
            span,
            name,
            content,
            is_error,
            output,
            error,
            origins,
            &Marks::default(),
        )
    }

    /// `_finish_call` where `marked` are strings FERB already redacted (Python's `Redacted`: a tool's redacted
    /// content, an error redacted by itself): the record keeps them as they are.
    #[allow(clippy::too_many_arguments)]
    fn finish_call_m(
        &self,
        span: &mut Span,
        name: &str,
        content: &Map<String, Value>,
        is_error: bool,
        output: Option<&Map<String, Value>>,
        error: Option<&str>,
        origins: Option<&BTreeSet<String>>,
        marked: &Marks,
    ) -> R<()> {
        let args = span
            .input
            .get("args")
            .filter(|a| ferb_py::value::truthy(a))
            .cloned()
            .unwrap_or_else(|| json!({}));
        let why = span
            .input
            .get("why")
            .filter(|w| ferb_py::value::truthy(w))
            .map(ferb_py::value::py_str)
            .unwrap_or_default();
        let summary = ferb_trace::lines::summarize(
            name,
            &args,
            &self.redact_marked(&Value::Object(content.clone()), marked),
            is_error,
        );
        let joined = origins.map(|o| origin::join(o.iter().map(String::as_str)));
        let out_v = output.map(|o| Value::Object(o.clone()));
        self.spans_marked(marked.clone(), |s| {
            s.finish(
                span,
                Finish {
                    output: out_v.as_ref(),
                    error,
                    cost_usd: None,
                    origin: joined.as_deref(),
                    summary: Some(&summary),
                },
            )
        })?;
        let seconds = span.ended_at.as_ref().map(|e| {
            let at = |t: &ferb_py::time::DateTime| when::from_py(t).ok();
            let d = match (at(e), at(&span.started_at)) {
                (Some(a), Some(b)) => (a - b).num_microseconds().unwrap_or(0) as f64 / 1e6,
                _ => 0.0,
            };
            (d * 10.0).round_ties_even() / 10.0
        });
        let red = |k: &str, v: &str| {
            self.redact(&json!({k: v}))
                .get(k)
                .cloned()
                .unwrap_or(Value::Null)
        };
        let red_args = self.redact(&args);
        let shown = shown_args(red_args.as_object().unwrap_or(&Map::new()));
        // every value here is the redactor's own output (Python's `Redacted`), kept as it is by the daemon's events
        let (summary_r, error_r, why_r) = (
            red("s", &summary),
            error.map(|e| red("e", e)),
            red("w", &why),
        );
        let mut texts = Marks::of_value(&red_args);
        for v in [Some(&summary_r), error_r.as_ref(), Some(&why_r)]
            .into_iter()
            .flatten()
        {
            texts.add_value(v);
        }
        self.publish_marked(
            "tool.finished",
            obj(json!({
                "tool": name, "task_id": span.task_id, "summary": summary_r,
                "seconds": seconds, "error": error_r, "args": shown, "why": why_r
            })),
            &texts,
        );
        Ok(())
    }

    fn observed(
        &self,
        name: &str,
        spec: &ActionSpec,
        action: &ActionRequest,
        grant: &RunGrant,
        status: &str,
    ) {
        self.publish(
            "action.observed",
            obj(json!({"what": name, "where": target_dict(&spec.target), "actor": "ferb", "task_id": grant.task_id, "status": status, "why": action.why})),
        );
    }

    /// `_ledger_refused` (P19): a call the ledger refuses, recorded and answered one way.
    fn ledger_refused(
        &self,
        span: &mut Span,
        name: &str,
        rec: &TaskRecord,
        e: Result<DuplicateUnresolved, String>,
    ) -> R<Reply> {
        let o = ferb_origin();
        match e {
            Err(refused) => {
                self.finish_call(
                    span,
                    name,
                    &obj(json!({"error": refused})),
                    true,
                    None,
                    Some(&refused),
                    None,
                )?;
                Ok(self.reply(
                    Some(rec),
                    obj(json!({"error": refused})),
                    true,
                    Some(&o),
                    false,
                ))
            }
            Ok(dup) => {
                let running = dup.state != "unknown";
                let error = if running {
                    SAME_ACTION_RUNNING
                } else {
                    UNKNOWN_NOT_REPEATED
                };
                self.finish_call(
                    span,
                    name,
                    &obj(json!({"error": error})),
                    true,
                    Some(&obj(json!({"unresolved": dup.intent_id}))),
                    Some(error),
                    None,
                )?;
                let mut content = obj(json!({"error": error}));
                if running {
                    content.insert("next".into(), Value::String(RUNNING_NEXT.into()));
                } else {
                    content.insert("retry_action".into(), Value::String(dup.intent_id.clone()));
                    content.insert("next".into(), Value::String(RETRY_NEXT.into()));
                }
                Ok(self.reply(Some(rec), content, true, Some(&o), false))
            }
        }
    }

    /// `_earlier_result`: the identical action already ran in a call whose caller had gone: that result.
    fn earlier_result(&self, c: &Call, intent_id: &str, out: Reply) -> R<Reply> {
        let mut span = self.start_span(
            "tool.call",
            &json!({"tool": c.name, "args": c.arguments, "kind": c.spec.kind, "target": target_dict(&c.spec.target), "earlier_attempt": intent_id}),
            &c.grant.task_id,
            Some(&c.rec.span_id),
        )?;
        let mut content = obj(out.content.clone());
        content.insert("completed_in_an_earlier_attempt".into(), Value::Bool(true));
        content.insert("note".into(), Value::String(EARLIER_ATTEMPT.into()));
        self.finish_call(
            &mut span,
            &c.name,
            &content,
            out.is_error,
            Some(&content),
            None,
            None,
        )?;
        Ok(Reply {
            content: Value::Object(content),
            ..out
        })
    }

    /// `_run` / `_held`: the authorized call's span, the live line and shadow advice, then the call under its locks.
    async fn run(
        &self,
        c: &mut Call,
        covered: &mut Covered,
        lifted: BTreeSet<(String, String)>,
        escape: Option<EscapeGrant>,
        terminal: Option<TerminalGrant>,
    ) -> R<Reply> {
        let mut span = self.call_span(c, &[])?;
        self.publish("tool.called", obj(json!({"tool": c.name, "task_id": c.grant.task_id, "worker_id": c.grant.worker_id, "effect": c.effect})));
        self.observer().tool_started(
            &c.grant,
            &c.name,
            &c.arguments,
            &c.spec.kind,
            c.spec.target.site.as_deref(),
        );
        if c.consequential() {
            let words = self.request(&c.rec.request_id).words;
            self.hooks().shadow(
                &c.name,
                &c.spec,
                &target_dict(&c.spec.target),
                &covered.ids,
                &c.grant.task_id,
                &words,
                &span.id,
            );
        }
        let observer = self.observer();
        observer.tool_call(&c.grant, false);
        let out = async {
            let held = c
                .cancel
                .guard(
                    self.locks
                        .hold(&c.lock_keys, &c.grant.worker_id, observes(c.tool.as_ref())),
                )
                .await?;
            let _held = held.map_err(CallError::Internal)?;
            let preview_root = c
                .rec
                .tree_id
                .clone()
                .unwrap_or_else(|| c.grant.task_id.split('.').next().unwrap_or("").to_string());
            self.hooks().preview(
                c.tool.as_ref(),
                &c.args,
                &c.grant,
                &preview_root,
                &c.lock_keys,
                None,
            );
            let point_root = c.root();
            c.cancel
                .guard(
                    self.hooks()
                        .point(c.tool.as_ref(), &c.args, &c.grant, &point_root),
                )
                .await?;
            c.cancel
                .guard(self.hooks().screen_before(
                    &c.grant,
                    &span.id,
                    &c.name,
                    &c.arguments,
                    c.tool.as_ref(),
                ))
                .await?;
            let result = self
                .execute(c, &mut span, covered, lifted, escape, terminal)
                .await;
            if let Ok(r) = &result {
                if !r.is_error {
                    if let Value::Object(content) = &r.content {
                        self.hooks().preview(
                            c.tool.as_ref(),
                            &c.args,
                            &c.grant,
                            &preview_root,
                            &c.lock_keys,
                            Some(content),
                        );
                    }
                }
            }
            let _ = self.hooks().screen_after(&span.id).await;
            result
        }
        .await;
        observer.tool_call(&c.grant, true);
        if let Err(e) = &out {
            if span.ended_at.is_none() {
                let why = match e {
                    CallError::Cancelled => "cancelled".to_string(),
                    CallError::Internal(_) => format!("{STAYED_OPEN}: Exception"),
                };
                let _ = self.finish_call(
                    &mut span,
                    &c.name,
                    &obj(json!({"error": why})),
                    true,
                    None,
                    Some(&why),
                    None,
                );
            }
        }
        out
    }

    fn mark(&self, intent: &Option<String>, state: &str, evidence: &[String]) {
        if let Some(id) = intent {
            let _ = self.d.ledger.mark(id, state, evidence);
        }
    }

    async fn execute(
        &self,
        c: &mut Call,
        span: &mut Span,
        covered: &mut Covered,
        lifted: BTreeSet<(String, String)>,
        escape: Option<EscapeGrant>,
        terminal: Option<TerminalGrant>,
    ) -> R<Reply> {
        let name = c.name.clone();
        let mut intent: Option<String> = None;
        let mut key_digest: Option<(String, String)> = None;
        if !REVERSIBLE.contains(&c.spec.kind.as_str()) {
            match c.ident.clone() {
                Ident::None => {
                    return self.ledger_refused(
                        span,
                        &name,
                        &c.rec,
                        Err("FERB couldn't tell what this does".into()),
                    )
                }
                Ident::Refused(e) => return self.ledger_refused(span, &name, &c.rec, Err(e)),
                Ident::Key(key, digest) => match self.d.ledger.intend(
                    &c.grant.task_id,
                    &key,
                    &target_dict(&c.spec.target),
                    &digest,
                ) {
                    Ok(id) => {
                        let _ =
                            self.d
                                .ledger
                                .mark(&id, "executing", std::slice::from_ref(&span.id));
                        intent = Some(id);
                        key_digest = Some((key, digest));
                    }
                    Err(dup) => return self.ledger_refused(span, &name, &c.rec, Ok(dup)),
                },
            }
        }
        let root = c.root();
        let folders = lock(&self.inner)
            .task_folders
            .get(&root)
            .cloned()
            .unwrap_or_default();
        let identities = c.tool.identities();
        let identity = if identities.len() == 1 {
            if identities.contains("ferb") {
                Identity::Ferb
            } else {
                Identity::Owner
            }
        } else {
            core_identity(&c.grant)
        };
        let ctx = ToolContext {
            task_id: c.grant.task_id.clone(),
            worker_id: c.grant.worker_id.clone(),
            request_id: c.rec.request_id.clone(),
            identity,
            scopes: c.grant.scopes.clone(),
            device_id: self.d.device_id.clone(),
            span_id: Some(span.id.clone()),
            workspace: self.d.roots.workspace.clone(),
            owner_roots: self.d.roots.owner_roots.clone(),
            untrusted_roots: self.d.roots.untrusted_roots.clone(),
            intent_id: intent.clone(),
            observe_only: c.grant.observe_only,
            probe: false,
            path_closed: Some(self.path_closed(&name, &lifted)),
            escape,
            terminal,
            folders,
        };
        if let Some(p) = &c.spec.target.path {
            (self.d.on_path)(c.real.as_deref().unwrap_or(p), &c.grant.task_id);
        }
        // S1: the snapshot and the file's copy come first
        let before: R<Result<Before, String>> = async {
            let mut snap = Map::new();
            if c.spec.kind == Kind::DELETE_PERMANENT
                || c.spec.kind == Kind::RUN_SHELL
                || c.fact_targets
                    .iter()
                    .any(|(k, _)| k == Kind::DELETE_PERMANENT)
            {
                snap = self.snapshot_before(c, &span.id).await?;
            }
            let kept = match self.keep_copy(c).await? {
                Ok(k) => k,
                Err(why) => return Ok(Err(why)),
            };
            let watched = c
                .cancel
                .guard(self.hooks().watch_changes(
                    c.tool.as_ref(),
                    &c.args,
                    &c.spec,
                    &c.grant,
                    &root,
                ))
                .await?;
            Ok(Ok((snap, kept, watched)))
        }
        .await;
        let (snap, kept, watched) = match before {
            Err(CallError::Cancelled) => {
                self.mark(&intent, "failed", &["cancelled before it ran".into()]);
                let e = "cancelled before it ran";
                self.finish_call(
                    span,
                    &name,
                    &obj(json!({"error": e})),
                    true,
                    None,
                    Some(e),
                    None,
                )?;
                return Err(CallError::Cancelled);
            }
            Err(e) => return Err(e),
            Ok(Err(why)) => {
                let error = fill(NO_COPY, &[("name", &name), ("why", &why)]);
                self.mark(&intent, "failed", std::slice::from_ref(&error));
                self.finish_call(
                    span,
                    &name,
                    &obj(json!({"error": error})),
                    true,
                    None,
                    Some(&error),
                    None,
                )?;
                return Ok(self.reply(
                    Some(&c.rec),
                    obj(json!({"error": error})),
                    true,
                    Some(&ferb_origin()),
                    false,
                ));
            }
            Ok(Ok(x)) => x,
        };
        let mut kept_out = Map::new();
        if let Some(k) = &kept {
            kept_out.insert("recoverable".into(), Value::String(k.clone()));
        }
        kept_out.extend(snap.clone());
        let kept_out = (!kept_out.is_empty()).then_some(kept_out);
        let mut guarded = c.spec.clone();
        guarded.effect_kind = None;
        if let Some(refusal) =
            self.observer()
                .conversation_guard(&c.grant, &name, c.tool.as_ref(), &c.spec)
        {
            self.mark(
                &intent,
                "failed",
                &["conversation paused before input".into()],
            );
            self.finish_call(
                span,
                &name,
                &obj(json!({"error": refusal})),
                true,
                None,
                Some(&refusal),
                None,
            )?;
            return Ok(self.reply(
                Some(&c.rec),
                obj(json!({"error": refusal})),
                true,
                Some(&ferb_origin()),
                false,
            ));
        }
        if let Some(f) = c.facts().cloned() {
            if let Some(chain) = f.tab_chain.clone().filter(|t| !t.is_empty()) {
                let host = f
                    .frame_host
                    .clone()
                    .map(|h| h.to_lowercase())
                    .filter(|h| !h.is_empty())
                    .unwrap_or_else(|| host_of(f.page_url.as_deref()));
                lock(&self.payments).observe(
                    &root,
                    Some(&chain),
                    Some(&host),
                    f.page_amount.as_deref(),
                    f.page_merchant.as_deref(),
                );
            }
        }
        covered.started = true;
        // `_measured`: what the tool's code measured meanwhile goes on its span whatever the outcome, a cancel's too
        let record = ferb_core::call_facts::Record::default();
        let run = Recorded {
            record: record.clone(),
            run: c.tool.run(&c.args, &ctx),
        };
        let ran = c.cancel.guard(run).await;
        let measured = record.facts();
        let (result, measured) = match ran {
            Err(CallError::Cancelled) => {
                if !measured.is_empty() {
                    self.spans(|s| s.update_input(&span.id, &Value::Object(measured)))?;
                }
                self.mark(
                    &intent,
                    "unknown",
                    &["cancelled while input may have been in flight".into()],
                );
                self.observed(&name, &c.spec, &c.action, &c.grant, "unknown");
                self.spans(|s| s.update_input(&span.id, &json!({"outcome": "unknown"})))?;
                let e = "cancelled; outcome unknown";
                self.finish_call(
                    span,
                    &name,
                    &obj(json!({"error": e})),
                    true,
                    kept_out.as_ref(),
                    Some(e),
                    None,
                )?;
                return Err(CallError::Cancelled);
            }
            Err(e) => return Err(e),
            Ok(r) => (r, measured),
        };
        if !measured.is_empty() {
            self.spans(|s| s.update_input(&span.id, &Value::Object(measured)))?;
        }
        let out = match result {
            Ok(out) => out,
            Err(e) => {
                return self
                    .failed_run(c, span, covered, &intent, kept_out, e)
                    .await
            }
        };
        let screen = self.hooks().screen_after(&span.id).await;
        self.mark(
            &intent,
            if out.result.is_error {
                "failed"
            } else {
                "done"
            },
            &[],
        );
        if out.result.is_error {
            covered.started = false;
        }
        if let Some(pay) = &c.pay {
            lock(&self.payments).ran(&root, pay, !out.result.is_error);
        }
        let mut raw = out.result.content.clone();
        if !REVERSIBLE.contains(&c.spec.kind.as_str()) {
            raw.insert("after".into(), after_block(&raw));
        }
        raw.extend(snap.clone());
        raw.extend(self.hooks().told(&screen));
        let mut out = out;
        let changed = self.after_change(c, &out, kept.as_deref()).await;
        raw.extend(changed);
        let early_origins = without_excluded_hosts(
            &result_origins(
                &out.result.read_sources,
                c.tool.facts().origin.as_deref(),
                &c.spec.kind,
            ),
            &|h| self.excluded(h),
        );
        let ran_is_error = out.result.is_error;
        let cancelled_after = |raw: &Map<String, Value>, span: &mut Span| -> R<Reply> {
            // stopped while the image hooks ran: the call ran, its span still ends with its result (redacted)
            let done = redact_measured(&self.d.redactor, raw, &[]);
            let err = if ran_is_error {
                done.get("error").map(ferb_py::value::py_str)
            } else {
                None
            };
            self.finish_call_m(
                span,
                &name,
                &done,
                ran_is_error,
                Some(&done),
                err.as_deref(),
                Some(&early_origins),
                &Marks::of_value(&Value::Object(done.clone())),
            )?;
            Err(CallError::Cancelled)
        };
        if !out.result.is_error && out.result.image.is_none() {
            let shot = c
                .cancel
                .guard(self.hooks().initial_image(
                    &name,
                    c.tool.as_ref(),
                    &c.args,
                    &c.spec,
                    &c.grant,
                    out,
                ))
                .await;
            match shot {
                Ok(o) => {
                    out = o;
                    raw.extend(out.result.content.clone());
                }
                Err(_) => return cancelled_after(&raw, span),
            }
        }
        if out.result.image.is_some() && !out.result.is_error {
            self.hooks().image_provided(&c.grant.task_id);
            match c.cancel.guard(self.hooks().redact_image(out)).await {
                Ok((o2, withheld)) => {
                    out = o2;
                    raw.extend(withheld);
                }
                Err(_) => return cancelled_after(&raw, span),
            }
        }
        let origins = without_excluded_hosts(
            &result_origins(
                &out.result.read_sources,
                c.tool.facts().origin.as_deref(),
                &c.spec.kind,
            ),
            &|h| self.excluded(h),
        );
        let mut content = redact_measured(&self.d.redactor, &raw, &out.measured);
        if !out.result.is_error && content.get("elements").is_some_and(Value::is_array) {
            content = self.refused_fill(c, span, content)?;
        }
        let mut output = content.clone();
        // the redacted content is the redactor's own output (Python's `Redacted`): the record keeps it as it is
        let marked = Marks::of_value(&Value::Object(content.clone()));
        if (out.result.image.is_some() || raw.contains_key("image_withheld"))
            && !out.result.is_error
        {
            let kept_image = c
                .cancel
                .guard(self.hooks().keep_image(
                    &name,
                    c.spec.target.site.as_deref(),
                    &c.grant,
                    &span.id,
                    &content,
                    out.result.image.as_deref(),
                    &origins,
                ))
                .await;
            let Ok(kept_image) = kept_image else {
                // stopped while the image was kept: the call ran, its span ends
                let mut o = content.clone();
                o.insert("evidence_error".into(), Value::String("cancelled".into()));
                let err = if out.result.is_error {
                    content.get("error").map(ferb_py::value::py_str)
                } else {
                    None
                };
                self.finish_call_m(
                    span,
                    &name,
                    &content,
                    out.result.is_error,
                    Some(&o),
                    err.as_deref(),
                    Some(&origins),
                    &marked,
                )?;
                return Err(CallError::Cancelled);
            };
            output.extend(kept_image);
        }
        if let Some(f) = &out.result.facts {
            let measured = f.recorded();
            if !measured.is_empty() {
                output.insert("facts".into(), Value::Object(measured));
            }
        }
        if let Some(t) = &out.result.timing {
            if !t.is_empty() {
                output.insert("timing".into(), Value::Object(t.clone()));
            }
        }
        let mut recorded = output.clone();
        if !watched.is_empty() {
            recorded.insert("change_record".into(), Value::Object(watched));
        }
        if !screen.is_empty() {
            recorded.insert("screen".into(), Value::Object(screen.clone()));
        }
        let err_text = if out.result.is_error {
            content.get("error").map(ferb_py::value::py_str)
        } else {
            None
        };
        self.finish_call_m(
            span,
            &name,
            &content,
            out.result.is_error,
            Some(&recorded),
            err_text.as_deref(),
            Some(&origins),
            &marked,
        )?;
        self.observed(
            &name,
            &c.spec,
            &c.action,
            &c.grant,
            if out.result.is_error {
                "failed"
            } else {
                "done"
            },
        );
        let footer = footer_origins(&out.result.read_sources, &origins);
        let mut reply = self.reply(
            Some(&c.rec),
            content,
            out.result.is_error,
            Some(&footer),
            true,
        );
        if let (Some(image), false) = (&out.result.image, out.result.is_error) {
            reply.image = Some(base64(image));
        }
        if let (Some(id), Some((key, digest))) = (&intent, &key_digest) {
            if !out.result.is_error && (c.left)() && self.task(&c.grant.task_id).is_some() {
                lock(&self.inner).undelivered.insert(
                    (c.grant.task_id.clone(), key.clone(), digest.clone()),
                    (id.clone(), reply.clone()),
                );
            }
        }
        Ok(reply)
    }

    async fn failed_run(
        &self,
        c: &mut Call,
        span: &mut Span,
        covered: &mut Covered,
        intent: &Option<String>,
        kept_out: Option<Map<String, Value>>,
        e: ToolError,
    ) -> R<Reply> {
        let name = c.name.clone();
        let detail = {
            let r = self.redact(&json!({"error": format!("{}: {}", e.class, e.message)}));
            r.get("error")
                .map(ferb_py::value::py_str)
                .unwrap_or_default()
        };
        let screen = self.hooks().screen_after(&span.id).await;
        let mut recorded = kept_out.clone().unwrap_or_default();
        if !screen.is_empty() {
            recorded.insert("screen".into(), Value::Object(screen.clone()));
        }
        let recorded = (!recorded.is_empty()).then_some(recorded);
        let mut content = obj(json!({}));
        if e.before_acting {
            self.mark(intent, "failed", std::slice::from_ref(&detail));
            covered.started = false;
            self.observed(&name, &c.spec, &c.action, &c.grant, "failed");
            self.finish_call_m(
                span,
                &name,
                &obj(json!({"error": detail})),
                true,
                recorded.as_ref(),
                Some(&detail),
                None,
                &Marks::of(&[&detail]),
            )?;
            content.insert(
                "error".into(),
                Value::String(format!("{name} failed: {detail}")),
            );
        } else {
            self.mark(intent, "unknown", std::slice::from_ref(&detail));
            self.observed(&name, &c.spec, &c.action, &c.grant, "unknown");
            self.spans(|s| s.update_input(&span.id, &json!({"outcome": "unknown"})))?;
            self.finish_call_m(
                span,
                &name,
                &obj(json!({"error": detail})),
                true,
                recorded.as_ref(),
                Some(&detail),
                None,
                &Marks::of(&[&detail]),
            )?;
            content.insert(
                "error".into(),
                Value::String(format!("{name} failed: {detail}; outcome unknown")),
            );
        }
        content.extend(kept_out.unwrap_or_default());
        content.extend(self.hooks().told(&screen));
        Ok(self.reply(Some(&c.rec), content, true, None, false))
    }

    /// `_snapshot_before` (S1): a local snapshot before a permanent delete or a shell command, on its own span.
    async fn snapshot_before(&self, c: &Call, before: &str) -> R<Map<String, Value>> {
        let Some(snapshots) = self.d.snapshots.clone() else {
            return Ok(Map::new());
        };
        let mut span = self.start_span(
            "recovery.snapshot",
            &json!({"before": c.name, "before_span": before}),
            &c.grant.task_id,
            Some(&c.rec.span_id),
        )?;
        let out = c
            .cancel
            .guard(snapshots.request(&c.grant.task_id, &format!("before {}", c.name)))
            .await;
        let out = match out {
            Ok(o) => o,
            Err(e) => {
                self.spans(|s| {
                    s.finish(
                        &mut span,
                        Finish {
                            error: Some("cancelled"),
                            ..Finish::default()
                        },
                    )
                })?;
                return Err(e);
            }
        };
        let err = out.get("error").map(ferb_py::value::py_str);
        let output = (!out.contains_key("error")).then(|| Value::Object(out.clone()));
        self.spans(|s| {
            s.finish(
                &mut span,
                Finish {
                    output: output.as_ref(),
                    error: err.as_deref(),
                    ..Finish::default()
                },
            )
        })?;
        Ok(snapshot_facts(&out))
    }

    /// `_keep_copy` (S1): before a call changes an existing file, a copy of the file it really writes. `Err(why)`:
    /// no copy could be made, so the call must not run.
    async fn keep_copy(&self, c: &Call) -> R<Result<Option<String>, String>> {
        let facts = c.tool.facts();
        if facts.creates_new_only {
            return Ok(Ok(None));
        }
        if c.spec.kind == Kind::TRASH {
            let Some(real) = &c.real else {
                return Ok(Ok(None));
            };
            if ferb_fs::pypath::is_symlink(real) || !ferb_fs::pypath::is_file(real) {
                return Ok(Ok(None));
            }
            return c
                .cancel
                .guard(
                    self.d
                        .recovery
                        .preserve(real, "trash copy", &c.grant.task_id),
                )
                .await;
        }
        if c.spec.kind != Kind::WRITE_FILE {
            return Ok(Ok(None));
        }
        let reason = match c.tool.recovery_reason(&c.args) {
            Some(r) => r,
            None => Some(c.name.clone()),
        };
        let Some(reason) = reason.filter(|r| !r.is_empty()) else {
            return Ok(Ok(None));
        };
        let Some(real) = &c.real else {
            return Ok(Err("FERB can't tell which file this tool writes".into()));
        };
        c.cancel
            .guard(self.d.recovery.preserve(real, &reason, &c.grant.task_id))
            .await
    }

    /// `_after_change` (S1): what a worker and the trace see about the copy and the Trash record. Never fails.
    async fn after_change(
        &self,
        c: &Call,
        out: &ToolOutput,
        kept: Option<&str>,
    ) -> Map<String, Value> {
        let r = &self.d.recovery;
        let content = &out.result.content;
        let is_error = out.result.is_error;
        let trash = c.spec.kind == Kind::TRASH;
        let attempt: Result<Map<String, Value>, String> = async {
            if let (Some(k), true) = (kept, trash) {
                if is_error && !content.get("trashed_to").is_some_and(ferb_py::value::truthy) {
                    if let Some(real) = &c.real {
                        r.discard_if_unchanged(k, real).await?;
                    }
                    return Ok(Map::new());
                }
                let mut m = obj(json!({"backup": k}));
                m.extend(Box::pin(self.after_change(c, out, None)).await);
                return Ok(m);
            }
            if let (Some(k), Some(real)) = (kept, &c.real) {
                if is_error {
                    let dropped = r.discard_if_unchanged(k, real).await?;
                    return Ok(if dropped { Map::new() } else { obj(json!({"recoverable": k})) });
                }
                r.mark_written(k, real).await?;
                return Ok(obj(json!({"recoverable": k})));
            }
            if trash && (!is_error || content.get("trashed_to").is_some_and(ferb_py::value::truthy)) {
                let where_ = content.get("path").filter(|v| ferb_py::value::truthy(v)).map(ferb_py::value::py_str);
                let to = content.get("trashed_to").filter(|v| ferb_py::value::truthy(v)).map(ferb_py::value::py_str);
                let Some(original) = where_.or_else(|| c.real.clone()) else {
                    return Ok(obj(json!({"recovery_error": "FERB couldn't tell which item it moved to the Trash"})));
                };
                let int = |k: &str| content.get(k).and_then(Value::as_i64);
                let verified = !is_error && int("device").is_some() && int("inode").is_some();
                let id = r.record_trash(
                    &original,
                    to.as_deref(),
                    if verified { int("device") } else { None },
                    if verified { int("inode") } else { None },
                    if verified { "trash" } else { "trash-unverified" },
                    &c.grant.task_id,
                )?;
                return Ok(obj(json!({"recoverable": id})));
            }
            Ok(Map::new())
        }
        .await;
        match attempt {
            Ok(m) => m,
            Err(e) => {
                let mut m = Map::new();
                if let Some(k) = kept {
                    m.insert("recoverable".into(), Value::String(k.to_string()));
                }
                m.insert("recovery_error".into(), Value::String(e));
                m
            }
        }
    }
}

fn base64(bytes: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            T[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            T[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

// ---------------------------------------------------------------------------------------------------------------
// a loop's probe, and the shell proxy's destinations
// ---------------------------------------------------------------------------------------------------------------

impl Gateway {
    /// `probe(grant, name, arguments, span_id)` (L31): one read by a loop's armed probe: the same admission as a call
    /// (FERB not stopped, the task running, the tool in its scopes and identity, an observation tool marked
    /// probe-safe, valid arguments, its precheck and the protection lists with only earlier Yeses) but no span, event
    /// or question of its own. A tool's failure is a returned error.
    pub async fn probe(
        &self,
        grant: &RunGrant,
        name: &str,
        arguments: &Map<String, Value>,
        span_id: &str,
    ) -> R<Map<String, Value>> {
        let cancel = Cancel::new();
        let job = NEXT_JOB.fetch_add(1, Ordering::SeqCst);
        lock(&self.inner).in_flight.push((
            job,
            grant.task_id.clone(),
            cancel.clone(),
            Arc::new(|| false),
        ));
        let out = self
            .probe_inner(grant, name, arguments, span_id, &cancel)
            .await;
        lock(&self.inner).in_flight.retain(|(j, _, _, _)| *j != job);
        out
    }

    async fn probe_inner(
        &self,
        grant: &RunGrant,
        name: &str,
        arguments: &Map<String, Value>,
        span_id: &str,
        cancel: &Cancel,
    ) -> R<Map<String, Value>> {
        let refused = |error: String| -> Map<String, Value> {
            let content = self.redact(&json!({"error": error}));
            obj(json!({"content": content, "is_error": true, "origins": [origin::FERB]}))
        };
        let rec = self.task(&grant.task_id);
        let stopped = {
            let i = lock(&self.inner);
            i.stopped || i.cancelled.contains(&grant.task_id)
        };
        let Some(rec) = rec.filter(|_| !stopped) else {
            return Ok(refused("task stopped or not running".into()));
        };
        let tool = self.registry().get(name).cloned();
        let Some(tool) = tool.filter(|t| {
            t.scopes().is_subset(&grant.scopes) && t.identities().contains(grant_identity(grant))
        }) else {
            return Ok(refused(format!(
                "tool {name} is not available to this loop"
            )));
        };
        let facts = tool.facts();
        if !facts.observation || !facts.probe_safe {
            return Ok(refused(format!(
                "{name} can't be a probe: a probe must only read, move nothing and return at once"
            )));
        }
        let mut model_schema = tool.input_schema();
        if !model_schema.is_object() {
            model_schema = json!({});
        }
        // the argument model and its hooks (not `Send`) end before the first await, so the probe's future is `Send`
        let checked = {
            let hooks = tool.hooks();
            let mut model = crate::pydantic::Model::of(&model_schema);
            model.hooks = hooks.as_deref();
            model.validate(&Value::Object(arguments.clone()))
        };
        let args = match checked {
            Ok(a) => a,
            Err(details) => {
                let reprs: Vec<String> = details
                    .iter()
                    .map(crate::pydantic::Detail::py_repr)
                    .collect();
                return Ok(refused(format!(
                    "invalid arguments: [{}]",
                    reprs.join(", ")
                )));
            }
        };
        if let Some(r) = tool.precheck(&args) {
            return Ok(refused(r));
        }
        let root = rec.tree_id.clone().unwrap_or_else(|| grant.task_id.clone());
        let lifted = self.reads_lifted(&root)?;
        let (found, never) = self.file_rules(tool.as_ref(), name, &args);
        if let Some((reason, error)) = never {
            return Ok(refused(if reason.starts_with("a protected") {
                fill(FS_PROBE_LIFT, &[("error", &error)])
            } else {
                error
            }));
        }
        for (path, access, rules) in found {
            let left: Vec<String> = rules
                .into_iter()
                .filter(|r| !lifted.contains(&(r.clone(), lift_how(&access).to_string())))
                .collect();
            if !left.is_empty() {
                let (reason, error) = self.closed(name, &path, &access, &left);
                return Ok(refused(if reason.starts_with("a protected") {
                    fill(FS_PROBE_LIFT, &[("error", &error)])
                } else {
                    error
                }));
            }
        }
        let spec = match tool.try_describe(&args) {
            Ok(spec) => spec,
            Err(e) => {
                return Ok(refused(format!(
                    "{name} failed: {}: {}",
                    e.class, e.message
                )))
            }
        };
        let identities = tool.identities();
        let identity = if identities.len() == 1 {
            if identities.contains("ferb") {
                Identity::Ferb
            } else {
                Identity::Owner
            }
        } else {
            core_identity(grant)
        };
        let ctx = ToolContext {
            task_id: grant.task_id.clone(),
            worker_id: grant.worker_id.clone(),
            request_id: rec.request_id.clone(),
            identity,
            scopes: grant.scopes.clone(),
            device_id: self.d.device_id.clone(),
            span_id: Some(span_id.to_string()),
            workspace: self.d.roots.workspace.clone(),
            owner_roots: self.d.roots.owner_roots.clone(),
            untrusted_roots: self.d.roots.untrusted_roots.clone(),
            intent_id: None,
            observe_only: true,
            probe: true,
            path_closed: Some(self.path_closed(name, &lifted)),
            escape: None,
            terminal: None,
            folders: Vec::new(),
        };
        let keys = tool.locks(&args);
        let held = cancel
            .guard(self.locks.hold(&keys, &grant.worker_id, false))
            .await?
            .map_err(CallError::Internal)?;
        let ran = cancel.guard(tool.run(&args, &ctx)).await?;
        drop(held);
        match ran {
            Ok(out) => {
                let origins = without_excluded_hosts(
                    &result_origins(
                        &out.result.read_sources,
                        facts.origin.as_deref(),
                        &spec.kind,
                    ),
                    &|h| self.excluded(h),
                );
                let content = redact_measured(&self.d.redactor, &out.result.content, &out.measured);
                Ok(obj(
                    json!({"content": content, "is_error": out.result.is_error, "origins": origins}),
                ))
            }
            Err(e) => Ok(refused(format!(
                "{name} failed: {}: {}",
                e.class, e.message
            ))),
        }
    }

    /// `_reads_lifted(root)` (GATE-200): the protected folders the owner let this tree read.
    fn reads_lifted(&self, root: &str) -> R<BTreeSet<(String, String)>> {
        let reopened = self.reopened(root)?;
        let ids: Vec<String> = self
            .d
            .roots
            .protection
            .0
            .reads
            .iter()
            .map(|e| e.id.clone())
            .collect();
        let mut out = BTreeSet::new();
        for id in ids {
            if self
                .find(root, &confirm::read_lift_key(&id), &reopened)?
                .is_some_and(|r| r.granted)
            {
                out.insert((id, "read".to_string()));
            }
        }
        Ok(out)
    }

    /// `destination_allowed(proc_id, host, port, local_approved)` (L2): any remote host for a shell process FERB
    /// tracks; the owner's No to a host in this tree stays final; a local address needs the proxy's own check.
    pub fn destination_allowed(
        &self,
        proc_id: &str,
        host: &str,
        _port: u16,
        local_approved: bool,
    ) -> R<(bool, String)> {
        let norm = dest::normalize("host", host);
        let root = (self.d.proc_task)(proc_id).and_then(|t| self.tree_of(&t));
        let Some(root) = root else {
            return Ok((
                false,
                "FERB couldn't tell which task this command belongs to".into(),
            ));
        };
        if let Some(row) = self.find(&root, &confirm::destination_key(&norm), &[])? {
            if !row.granted {
                return Ok((false, DEST_REFUSED_NO.into()));
            }
        }
        if local_approved {
            return Ok((true, String::new()));
        }
        let local = match norm.parse::<std::net::IpAddr>() {
            Ok(a) => a.is_loopback(),
            Err(_) => norm == "localhost" || norm.ends_with(".localhost"),
        };
        if local {
            return Ok((
                false,
                "local services need the proxy's executable-pinned permission".into(),
            ));
        }
        Ok((true, String::new()))
    }
}

// ---------------------------------------------------------------------------------------------------------------
// GATE-562: a page that asked again for a value FERB filled through the sealed path (`refused_fills.py`)
// ---------------------------------------------------------------------------------------------------------------

impl Gateway {
    /// `_refused_fill`: this read of a tab shows the page asking again for the field a sealed fill filled before a
    /// submit; the owner is told at once and the worker gets the fact.
    fn refused_fill(
        &self,
        c: &Call,
        span: &Span,
        content: Map<String, Value>,
    ) -> R<Map<String, Value>> {
        let Some(tab) = c
            .args
            .get("tab")
            .and_then(Value::as_str)
            .filter(|t| !t.is_empty())
        else {
            return Ok(content);
        };
        let rowid: Option<i64> = {
            let db = lock(&self.d.db);
            db.conn()
                .query_row("SELECT rowid FROM spans WHERE id = ?", [&span.id], |r| {
                    r.get(0)
                })
                .ok()
        };
        let Some(rowid) = rowid else {
            return Ok(content);
        };
        let Some(refusal) = self.refusal_check(&c.root(), tab, &content, rowid)? else {
            return Ok(content);
        };
        let left = ASKS_AFTER_REFUSAL.saturating_sub(refusal.asked_after);
        self.publish(
            "secret.refused",
            obj(json!({"task_id": c.root(), "place": refusal.place, "what": refusal.what, "count": refusal.count, "words": refusal.words, "span_id": span.id})),
        );
        let mut out = content;
        out.insert(
            "sealed_fill_refused".into(),
            json!({
                "fact": format!("The page asked again for the {} the owner gave (the same field, empty, after the submit). FERB told the owner.", refusal.what),
                "page_says": refusal.words, "times": refusal.count, "owner_asks_left": left
            }),
        );
        Ok(out)
    }

    /// `RefusedFills.check(root, tab, read, before_rowid)`.
    fn refusal_check(
        &self,
        root: &str,
        tab: &str,
        read: &Map<String, Value>,
        before: i64,
    ) -> R<Option<Refusal>> {
        let (Some(url), Some(elements)) = (
            read.get("url").and_then(Value::as_str),
            read.get("elements").and_then(Value::as_array),
        ) else {
            return Ok(None);
        };
        let (w, p) = tree_where(root);
        let fill: Option<(i64, String, String)> = {
            let db = lock(&self.d.db);
            let sql = format!(
                "SELECT rowid, id, input_json FROM spans WHERE kind = 'tool.call' AND rowid < ? AND ended_at IS NOT NULL \
                 AND error IS NULL AND json_extract(output_json, '$.ok') = 1 AND json_extract(input_json, '$.tool') IN \
                 (?,?,?,?) AND json_extract(input_json, '$.args.tab') = ? AND {w} ORDER BY rowid DESC LIMIT 1"
            );
            db.conn()
                .query_row(
                    &sql,
                    rusqlite::params![
                        before,
                        SEALED_BROWSER[0],
                        SEALED_BROWSER[1],
                        SEALED_BROWSER[2],
                        SEALED_BROWSER[3],
                        tab,
                        p[0],
                        p[1],
                        p[2]
                    ],
                    |r| {
                        Ok((
                            r.get(0)?,
                            r.get(1)?,
                            r.get::<_, Option<String>>(2)?
                                .unwrap_or_else(|| "{}".into()),
                        ))
                    },
                )
                .ok()
        };
        let Some((fill_rowid, fill_id, input)) = fill else {
            return Ok(None);
        };
        let call: Value = serde_json::from_str(&input).unwrap_or(json!({}));
        let facts = call.get("facts").cloned().unwrap_or(json!({}));
        let kinds: Vec<String> = facts
            .get("field_kinds")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        let key = field_key(
            call.get("tool").and_then(Value::as_str).unwrap_or(""),
            kinds.first().map(String::as_str),
        );
        let (place, path) = page_of(facts.get("page_url").and_then(Value::as_str).unwrap_or(""));
        if place.is_empty() || page_of(url) != (place.clone(), path) {
            return Ok(None);
        }
        let submitted = {
            let db = lock(&self.d.db);
            let sql = format!(
                "SELECT 1 FROM spans WHERE kind = 'tool.call' AND rowid > ? AND rowid < ? AND \
                 json_extract(input_json, '$.args.tab') = ? AND (json_extract(input_json, '$.facts.submits_form') = 1 \
                 OR json_extract(output_json, '$.facts.submits_form') = 1 OR \
                 json_extract(output_json, '$.submitted[0].method') IS NOT NULL) AND {w} LIMIT 1"
            );
            db.conn()
                .query_row(
                    &sql,
                    rusqlite::params![fill_rowid, before, tab, p[0], p[1], p[2]],
                    |_| Ok(()),
                )
                .is_ok()
        };
        if !submitted {
            return Ok(None);
        }
        let label = facts
            .get("control_label")
            .filter(|v| ferb_py::value::truthy(v))
            .map(ferb_py::value::py_str)
            .unwrap_or_default();
        let asks_again = elements.iter().any(|e| {
            e.get("sensitive").is_some_and(ferb_py::value::truthy)
                && e.get("filled") == Some(&Value::Bool(false))
                && match kinds.first() {
                    Some(k) => e.get("field") == Some(&Value::String(k.clone())),
                    None => {
                        !label.is_empty() && e.get("label") == Some(&Value::String(label.clone()))
                    }
                }
        });
        if !asks_again {
            return Ok(None);
        }
        let words = self.new_words(
            root,
            tab,
            fill_rowid,
            read.get("text")
                .map(ferb_py::value::py_str)
                .unwrap_or_default()
                .as_str(),
        )?;
        let bare = place.strip_prefix("www.").unwrap_or(&place).to_string();
        let mut i = lock(&self.inner);
        let tree = i.refusals.entry(root.to_string()).or_default();
        let at = match tree
            .iter()
            .position(|(k, _)| *k == (bare.clone(), key.clone()))
        {
            Some(at) => at,
            None => {
                tree.push((
                    (bare, key.clone()),
                    Refusal {
                        what: what_of(&key).into(),
                        place: place.clone(),
                        ..Refusal::default()
                    },
                ));
                tree.len() - 1
            }
        };
        let refusal = &mut tree[at].1;
        if refusal.judged.contains(&fill_id) {
            return Ok(None);
        }
        refusal.judged.insert(fill_id);
        refusal.count += 1;
        refusal.words = words;
        Ok(Some(refusal.clone()))
    }

    /// `_new_words`: the page's lines that weren't on it in the tab's last read before the fill.
    fn new_words(&self, root: &str, tab: &str, fill_rowid: i64, text: &str) -> R<String> {
        let (w, p) = tree_where(root);
        let before: String = {
            let db = lock(&self.d.db);
            let sql = format!(
                "SELECT output_json FROM spans WHERE kind = 'tool.call' AND rowid < ? AND \
                 json_extract(input_json, '$.args.tab') = ? AND json_extract(output_json, '$.elements') IS NOT NULL AND \
                 {w} ORDER BY rowid DESC LIMIT 1"
            );
            let row: Option<Option<String>> = db
                .conn()
                .query_row(
                    &sql,
                    rusqlite::params![fill_rowid, tab, p[0], p[1], p[2]],
                    |r| r.get(0),
                )
                .ok();
            row.flatten()
                .and_then(|o| serde_json::from_str::<Value>(&o).ok())
                .and_then(|v| {
                    v.get("text")
                        .filter(|t| ferb_py::value::truthy(t))
                        .map(ferb_py::value::py_str)
                })
                .unwrap_or_default()
        };
        let seen: HashSet<String> = py_lines(&before).iter().map(|l| py_split_join(l)).collect();
        let new: Vec<String> = py_lines(text)
            .iter()
            .map(|l| py_split_join(l))
            .filter(|l| !l.is_empty() && !seen.contains(l))
            .collect();
        let words = new.join(" ");
        Ok(if words.chars().count() <= WORDS_SHOWN {
            words
        } else {
            format!(
                "{}…",
                ferb_py::value::prefix(&words, WORDS_SHOWN - 1).trim_end()
            )
        })
    }

    /// `RefusedFills.get(root, place, key)`: for the hidden-input question's count (GATE-563).
    pub fn refusal(&self, root: &str, place: &str, key: &str) -> Option<Refusal> {
        let place = place.to_lowercase();
        let bare = place.strip_prefix("www.").unwrap_or(&place).to_string();
        lock(&self.inner)
            .refusals
            .get(root)?
            .iter()
            .find(|(k, _)| *k == (bare.clone(), key.to_string()))
            .map(|(_, r)| r.clone())
    }
}

/// Python's `str.splitlines()`.
fn py_lines(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '\r' => {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                out.push(std::mem::take(&mut cur));
            }
            '\n' | '\u{0b}' | '\u{0c}' | '\u{1c}' | '\u{1d}' | '\u{1e}' | '\u{85}' | '\u{2028}'
            | '\u{2029}' => {
                out.push(std::mem::take(&mut cur));
            }
            other => cur.push(other),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}
