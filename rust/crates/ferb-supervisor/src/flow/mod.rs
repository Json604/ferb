//! The supervisor's flows (supervisor.py:1097 `Supervisor`), the forward path: a chat message from its arrival to its
//! result. [`turn`] takes a message in and has the planner decide (`send`, `_turn`, `_intake`, `_decide`,
//! `_supervise`, `_route`, `_exit`, `_record_reply`, `_start`); [`tree`] runs a task (`run_tree`, `_execute`,
//! `_execute_plan`, `_finish_tree`, `_final`, the plan event and the step states); [`attempt`] is one task's or
//! step's attempt loop (`_worker`, `_judge`, `_next`, `verify`, `_verifier`, `_check`, the findings round, `_context`);
//! [`host`] is what the worker reads of the supervisor (ferb-worker's `WorkerHost`); [`chat`] is the daemon's
//! `chat.send` (ack first, the turn's events stamped, the task's result recorded).
//!
//! What another part owns comes in through [`Daemon`] (the gateway's reads, the environment, the desktop, the browser,
//! memory, schedules, the change records, the literal checks) and ferb-worker's runtime factory. The store is FERB's
//! SQLite database, written through ferb-orchestrator's stores as Python writes it.
//!
//! Not ported here (each answers [`crate::NotPorted`] by its Python name): control of running work (stop, pause,
//! steer, continue, retry), suspend and resume, restart recovery, scheduled runs, waits for the owner (a plan's ask
//! step, a checker's `ask`, the check-in after five attempts, a runtime switch), loops, delegated conversations, the
//! side entries (memory, forgets, reminders, schedules), the running summary's fold, the planner's lookups' last call,
//! the image-refused rerun and the intro preview.

pub mod attempt;
pub mod chat;
pub mod host;
pub mod store;
pub mod tree;
pub mod turn;

use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, Weak};

use ferb_clock::clock::ZonedClock;
use ferb_context::assemble::{Assembly, ContextBlock};
use ferb_context::prompts::PromptRoot;
use ferb_context::tokens::Tokenizer;
use ferb_core::interfaces::IdSource;
use ferb_core::types::{ModelImage, Outcome, Task};
use ferb_orchestrator::models::{Condition, TaskSpec};
use ferb_orchestrator::ports::{BoxFuture, Policy};
use ferb_orchestrator::steering::Steering;
use ferb_orchestrator::verify::ConditionResult;
use ferb_sanitize::Redactor;
use ferb_store::Db;
use ferb_worker::ports::{Grant, ModelRuntime};
use ferb_worker::{Runs, Worker};
use serde_json::{Map, Value};
use tokio::sync::mpsc;

use crate::NotPorted;

/// A turn's or a tree's stream of events (the daemon's `chat.send` stamps each as it queues it).
pub type Emit = mpsc::UnboundedSender<Value>;

/// A change record a step's checker reads (ferb/act/fs/changes.py `Change`, as `_changed_files` reads it).
#[derive(Clone, Debug, PartialEq)]
pub struct Change {
    pub path: String,
    /// "new", "modified", "removed", "renamed".
    pub what: String,
    pub source: Option<String>,
    /// FERB may read its content now (a record and a look), else a fact only.
    pub content: bool,
    /// A call of the step named it exactly.
    pub named: bool,
    /// `store.describe(change)`: its redacted diff as the checker sees it.
    pub text: String,
}

/// What a planner turn's environment read gave (`EnvironmentPort.environment(words)`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Environment {
    pub stable: String,
    pub live: String,
}

/// Everything the supervisor reads from, or hands to, another part of FERB (supervisor.py's `__init__` arguments:
/// the gateway, `ExecutionState`, `RunTokens`, the ports). Each method names the Python call it stands for.
pub trait Daemon: Send + Sync {
    // --- the gateway (ferb/daemon/gateway.py) ---
    /// `gateway.caps`: the registered tool names, registry order.
    fn tool_names(&self) -> Vec<String>;
    /// `set().union(*(c.scopes for c in gateway.caps.values()))`.
    fn all_scopes(&self) -> BTreeSet<String>;
    /// `observes(gateway.caps.get(tool))`: whether a tool only observes.
    fn observes(&self, tool: &str) -> bool;
    /// `getattr(gateway.caps.get(tool), "reads_content", False)`.
    fn reads_content(&self, tool: &str) -> bool;
    /// `gateway.tools_for(grant)`: the rows `{name, description, input_schema}`.
    fn tools_for(&self, grant: &Grant) -> Vec<Value>;
    /// `tokens.issue(...)` (auth.py `RunTokens`); `tools`/`max_calls` for the planner's lookups.
    #[allow(clippy::too_many_arguments)] // auth.py's own arguments
    fn issue_token(
        &self,
        task_id: &str,
        worker_id: &str,
        scopes: &BTreeSet<String>,
        identity: &str,
        ttl_s: f64,
        observe_only: bool,
        tools: Option<&[String]>,
        max_calls: Option<usize>,
    ) -> String;
    /// `tokens.validate(token)`: its grant, or None (`AuthError`).
    fn validate_token(&self, token: &str) -> Option<Grant>;
    fn revoke_worker(&self, worker_id: &str);
    fn forget_calls(&self, worker_id: &str);
    fn calls_refused(&self, watch_id: &str) -> u64;
    fn lookups_made(&self, holder: &str) -> usize;
    fn initial_image_given(&self, task_id: &str);
    /// `gateway.ledger.unresolved(root)`.
    fn unresolved(&self, root: &str) -> Vec<Map<String, Value>>;
    /// `gateway.forget_tasks(ids)`.
    fn forget_tasks(&self, ids: &BTreeSet<String>);
    /// `gateway.task_folders[tree] = folders` (None: `pop`).
    fn task_folders(&self, tree_id: &str, folders: Option<&[String]>);
    /// `gateway.destinations.from_words(words)` recorded as named (SQ6).
    fn record_named(&self, words: &str, request_id: &str);
    /// `gateway.standing.revoke(tree_id)`.
    fn standing_revoke(&self, tree_id: &str);
    /// `gateway.standing.scopes(tree_id)` as dumped.
    fn standing_scopes(&self, tree_id: &str) -> Vec<Value>;
    /// `state.requests.record(Request(...))` (the gateway's `ExecutionState`).
    fn record_request(&self, request: &Request);
    /// `state.requests.get(request_id)`.
    fn request(&self, request_id: &str) -> Request;
    /// `state.forget_request(request_id)`.
    fn forget_request(&self, request_id: &str);
    /// `request_id in state.approvals.unattended`.
    fn unattended(&self, request_id: &str) -> bool;
    /// `state.approvals.pending()` as the planner's questions block reads them.
    fn pending_questions(&self) -> Vec<crate::planner::OpenQuestion>;

    // --- the environment (EnvironmentPort) and the facts every role reads ---
    /// `environment(words)`.
    fn environment<'a>(&'a self, words: &'a str) -> BoxFuture<'a, Environment>;
    /// `live_facts(last_runs=..., owner_state=...)` (the planner's).
    fn live_facts(&self, last_runs: bool, owner_state: bool) -> String;
    /// What `_system` reads as it assembles a role's instructions: `facts()`, `live_facts()` when `live`,
    /// `availability().text`, `identity_context()` and both catalogs.
    fn system_inputs(&self, live: bool) -> crate::prompts::SystemInputs;
    /// `availability().text` (the checker's report).
    fn availability(&self) -> String;
    fn worker_types(&self) -> Vec<String>;
    /// `signals()`: when the owner was last active, the last trail event.
    fn signals(&self) -> (Option<ferb_clock::py::Time>, Option<ferb_clock::py::Time>);
    /// `relevant_context(words)`.
    fn relevant_context(&self, words: &str) -> crate::worker_context::Activity;
    fn apps(&self) -> Vec<(String, String)>;
    fn running_apps(&self) -> Vec<(String, String, i64)>;
    fn shell_process(&self, id: &str) -> Option<Map<String, Value>>;
    fn runtime_account(&self, runtime: &str, account: &str);
    fn on_violation(&self, runtime: &str, text: &str);
    fn has_route(&self) -> Option<bool>;

    // --- memory (MemoryPort) ---
    fn identity_pending(&self) -> String;
    fn settle(&self) -> BoxFuture<'_, ()>;
    /// `memory_push(query, targets)`: the ranked blocks a worker is given.
    fn memory_push<'a>(
        &'a self,
        query: &'a str,
        targets: &'a str,
    ) -> BoxFuture<'a, Vec<ContextBlock>>;
    fn on_episode<'a>(&'a self, tree_id: &'a str) -> BoxFuture<'a, ()>;

    // --- schedules (SchedulePort) ---
    fn last_run_origins(&self) -> BTreeSet<String>;

    // --- the desktop (DesktopPort) and the resources a task holds ---
    fn on_task_start<'a>(&'a self, tree_id: &'a str) -> BoxFuture<'a, ()>;
    fn on_task_finish<'a>(&'a self, tree_id: &'a str) -> BoxFuture<'a, ()>;
    fn disturbance_context(&self, tree_id: &str) -> String;
    /// `on_task_end(tid)` watched by the disturbance sentinel (`_end_task`): what the release closed or left.
    fn on_task_end<'a>(&'a self, task_id: &'a str, span_id: &'a str) -> BoxFuture<'a, Vec<Value>>;
    /// `changes.changes(task, tree)` described (`_changed_files`); None when there is no change store.
    fn changes(&self, task_id: &str, tree_id: &str) -> Option<Result<Vec<Change>, String>>;
    /// The untrusted roots (downloads), resolved.
    fn untrusted_roots(&self) -> Vec<String>;

    // --- the browser (BrowserPort) ---
    fn browser_context(&self, task_id: &str) -> String;
    fn task_tabs(&self, task_id: &str) -> Vec<Value>;
    fn keep_tabs_open(&self, task_id: &str);

    // --- the literal checks (verify.py `Checks`) ---
    fn code_checkable(&self, condition: &Condition, task_id: &str) -> bool;
    fn code_check<'a>(
        &'a self,
        index: i64,
        condition: &'a Condition,
        task_id: &'a str,
    ) -> BoxFuture<'a, ConditionResult>;
    fn declined(&self, index: i64, condition: &Condition, task_id: &str)
        -> Option<ConditionResult>;

    // --- records other parts own ---
    /// policy/sites.py `sites_contacted(tree)`.
    fn sites_contacted(&self, tree_id: &str) -> Vec<String>;
    /// `_schedule_label(schedule_id)`: its name, else its instruction's first clause; the id when unknown.
    fn schedule_label(&self, schedule_id: &str) -> String;
    /// launched.py `left_running(records)`: code's line for each app a task launched and left running.
    fn left_running(&self, resources: &[Value]) -> Vec<String>;
    /// cards.py `payments(tree)` + `sends(tree)`.
    fn ran_cards(&self, tree_id: &str) -> Vec<Value>;
    /// A plan step's notes file (`atomic_text` under the workspace's task-notes folder), its protection checked.
    fn write_notes(&self, path: &str, text: &str) -> Result<(), String>;
    /// The front window captured at send (L12), once awaited.
    fn front_window<'a>(&'a self, request_id: &'a str) -> BoxFuture<'a, Option<FrontSeen>>;

    // --- the runtime factory ---
    /// `make_runtime(role, mode=..., tool_names=..., schema=..., effort=...)`.
    fn make_runtime(
        &self,
        role: &str,
        mode: &str,
        tool_names: &[String],
        schema: &Value,
        effort: Option<&str>,
    ) -> Result<Arc<dyn ModelRuntime>, ferb_worker::Error>;

    // --- the bus ---
    fn publish(&self, topic: &str, data: Value);
}

/// What `_screen_at_send` gave a message: its front window facts and its redacted screenshot.
#[derive(Clone, Debug, PartialEq)]
pub struct FrontSeen {
    pub facts: Map<String, Value>,
    pub image: Option<ModelImage>,
    pub origin: String,
}

/// tree.py / state.py `Request`: what the gateway knows of a request.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Request {
    pub request_id: String,
    pub words: String,
    /// (apps, sites, paths, recipients).
    pub targets: [Vec<String>; 4],
    pub payload: String,
    pub asked_origins: BTreeSet<String>,
}

/// What the daemon builds the supervisor from.
pub struct Deps {
    pub db: Arc<Mutex<Db>>,
    pub clock: Arc<dyn ZonedClock>,
    pub ids: Arc<dyn IdSource>,
    pub redactor: Arc<Redactor>,
    pub policy: Arc<dyn Policy>,
    pub daemon: Arc<dyn Daemon>,
    pub prompts: PromptRoot,
    pub tokenizer: Tokenizer,
    pub device_id: String,
    pub workspace: String,
    pub home: String,
    /// The untrusted-data wrapper (`wrap_untrusted`, its id injected in tests).
    pub wrap: Arc<dyn Fn(&str) -> String + Send + Sync>,
    /// `"chat-" + token_hex(6)`'s hex (continuity's new chat ids).
    pub new_chat: Arc<dyn Fn() -> String + Send + Sync>,
    pub stall_s: f64,
    pub heartbeat_poll_s: f64,
}

/// One planning turn (supervisor.py:511 `_Turn`), shared while it runs (`_turns`).
#[derive(Default)]
pub struct Turn {
    pub request_id: String,
    pub session: String,
    pub words: String,
    pub payload: String,
    pub own_words: String,
    pub typed: String,
    pub typed_lines: String,
    pub for_model: String,
    pub untrusted: bool,
    /// Its chat.message rows (id, origin, request id), the lead's first.
    pub entries: Vec<store::Entry>,
    pub carried: Vec<store::Entry>,
    pub clarified: String,
    pub open: Option<Start>,
    pub joined: bool,
    pub asked_outside: BTreeSet<String>,
    pub input_paths: Vec<String>,
    pub attached: bool,
    pub message_origin: BTreeSet<String>,
    pub environment: Environment,
    pub screen: Option<FrontSeen>,
    pub turn_origins: BTreeSet<String>,
    /// (labels, text, values) it saw (GATE-519).
    pub shown: Vec<(BTreeSet<String>, String, Option<Value>)>,
    pub exposures: Vec<Map<String, Value>>,
    pub looked: i64,
    pub assembly: Option<Assembly>,
    pub planner_sources: String,
    pub intake_receipts: Vec<String>,
    pub side_lines: Vec<String>,
    pub side_cards: Vec<Value>,
    pub acknowledged: String,
    pub said_first: String,
    pub said_first_cards: Vec<Value>,
    pub decided: Option<f64>,
    pub preview: bool,
    /// The planner's pre-call span, open from intake to its first call (ORCH-525).
    pub prepare: Option<turn::Prepare>,
}

impl Turn {
    /// `request_ids`: the turn's request and its messages'.
    pub fn request_ids(&self) -> BTreeSet<String> {
        let mut out: BTreeSet<String> = self.entries.iter().map(|e| e.request_id.clone()).collect();
        out.insert(self.request_id.clone());
        out
    }
}

/// What a chat turn continues (`_Start`): FERB's open question and the request it asked about.
#[derive(Clone, Debug, Default)]
pub struct Start {
    pub pending: String,
    pub asked: String,
    pub payload: String,
    pub asked_outside: BTreeSet<String>,
    pub carried: Vec<store::Entry>,
    pub paths: Vec<String>,
}

/// One task tree or plan step as its attempt loop runs it (supervisor.py:892 `_Tree`): the fields the forward path
/// reads and writes.
#[derive(Default)]
pub struct TreeState {
    pub spec: Option<TaskSpec>,
    pub outcome: Option<Outcome>,
    pub verification: Vec<ConditionResult>,
    pub stop: String,
    pub prior_task: Option<String>,
    pub reports: Vec<Map<String, Value>>,
    pub resources: Vec<Value>,
    pub cleanup_errors: Vec<String>,
    pub intake_receipts: Vec<String>,
    pub context_sources: HashMap<String, String>,
    pub owner_question: String,
    pub stop_reason: String,
    pub stop_by_code: bool,
    pub stands_unconfirmed: bool,
    pub kept_contradicted: bool,
    pub declined: bool,
    pub check_told: String,
    pub checker_judged: BTreeSet<i64>,
    pub check_problem: String,
    pub owner_answer: String,
    pub reason: String,
    pub worker_question: String,
    pub how: String,
    pub confirmed: HashMap<i64, ConditionResult>,
    pub unconfirmed: String,
    pub runtime: String,
    pub runtime_switch: Option<Value>,
    pub input_notes: Vec<Value>,
    pub conversation_proposals: Vec<Value>,
    pub part_updates: Vec<Value>,
    pub answer_settled: bool,
    pub answer_unmarked: String,
    pub watch: bool,
    pub watch_met: Option<bool>,
    pub step_context: String,
    pub parent_goal: String,
    pub required_inputs: Vec<String>,
    pub accepting_updates: bool,
    pub work_done: bool,
    pub side_lines: Vec<String>,
    pub side_cards: Vec<Value>,
    pub said_first: String,
    pub said_first_cards: Vec<Value>,
    pub start_block: Option<ContextBlock>,
    pub start_image: Option<ModelImage>,
    pub planner_reads: bool,
    pub unattended_scopes: Option<Vec<Value>>,
    pub schedule_id: Option<String>,
    pub schedule_facts: Map<String, Value>,
    /// The root's: each step's latest live state (ORCH-531).
    pub step_states: Map<String, Value>,
    pub attempt: usize,
    /// The root's: each answer step's tree and each step's tree, as they ran here (ORCH-319, ORCH-627).
    pub answers: Vec<(String, Arc<Tree>)>,
    pub ran: Vec<(String, Arc<Tree>)>,
    pub answering: Option<Arc<Tree>>,
    /// A single task's one step: the outcome its attempt loop returned (`_execute_plan`'s `ran`).
    pub single_ran: Option<Outcome>,
}

/// A tree (or a plan step's member tree): its ids, its state and the corrections it shares with its root.
pub struct Tree {
    pub id: String,
    pub worker: Task,
    pub step_id: String,
    pub asks_in_place: bool,
    pub st: Mutex<TreeState>,
    pub steering: Arc<tokio::sync::Mutex<Steering>>,
    pub result_revision: Mutex<usize>,
}

impl Tree {
    pub fn lock(&self) -> MutexGuard<'_, TreeState> {
        self.st.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn spec(&self) -> TaskSpec {
        self.lock().spec.clone().expect("a tree's spec")
    }
}

/// The supervisor's live state (`_lines`, `_turns`, `_tree_of`, `_step_seq`, the sessions).
#[derive(Default)]
pub struct Live {
    pub lines: HashMap<String, turn::Line>,
    pub turns: HashMap<String, Arc<Mutex<Turn>>>,
    pub tree_of: HashMap<String, Arc<Tree>>,
    pub step_seq: String,
    /// Live task records (tree.py `TaskTree._tasks`): task id -> (tree id, span id).
    pub records: HashMap<String, (String, String)>,
    /// `continuity.lend`: a planner's lookups task -> the chat it reads.
    pub lent: HashMap<String, String>,
}

/// The supervisor. Cheap to clone; every flow runs on a shared [`Inner`].
#[derive(Clone)]
pub struct Supervisor {
    pub(crate) inner: Arc<Inner>,
}

pub struct Inner {
    pub deps: Deps,
    pub runs: Arc<Runs>,
    pub(crate) worker: OnceLock<Arc<Worker>>,
    pub(crate) live: Mutex<Live>,
    pub(crate) me: Weak<Inner>,
    /// The workers' liveness (guards.py `Activity`, `self.activity`).
    pub(crate) activity: Arc<ferb_orchestrator::guards::Activity>,
}

impl Supervisor {
    pub fn new(deps: Deps) -> Supervisor {
        let clock = deps.clock.clone();
        let inner = Arc::new_cyclic(|me| Inner {
            activity: Arc::new(ferb_orchestrator::guards::Activity::new(move || {
                clock.monotonic()
            })),
            deps,
            runs: Arc::new(Runs::new()),
            worker: OnceLock::new(),
            live: Mutex::new(Live::default()),
            me: me.clone(),
        });
        let host = Arc::new(host::Host {
            sup: Arc::downgrade(&inner),
            clock: inner.deps.clock.clone(),
            policy: inner.deps.policy.clone(),
            activity: inner.activity.clone(),
        });
        let config = ferb_worker::WorkerConfig {
            heartbeat_poll_s: inner.deps.heartbeat_poll_s,
            stall_s: inner.deps.stall_s,
            tokenizer: inner.deps.tokenizer,
            prompts: inner.deps.prompts.clone(),
            approach: crate::prompts::APPROACH.to_string(),
        };
        let worker = Arc::new(Worker::new(host, inner.runs.clone(), config));
        let _ = inner.worker.set(worker);
        Supervisor { inner }
    }

    pub fn inner(&self) -> &Arc<Inner> {
        &self.inner
    }

    /// supervisor.py:2422 `preview`: the intro's plan-only turn (ORCH-500).
    pub fn preview(&self) -> Result<(), NotPorted> {
        Err(NotPorted("Supervisor.preview"))
    }

    /// supervisor.py:1629 `run_scheduled`.
    pub fn run_scheduled(&self) -> Result<(), NotPorted> {
        Err(NotPorted("Supervisor.run_scheduled"))
    }

    /// supervisor.py:2324 `resume_plans`: restart recovery.
    pub fn resume_plans(&self) -> Result<(), NotPorted> {
        Err(NotPorted("Supervisor.resume_plans"))
    }

    /// supervisor.py:1760 `stop_tree`.
    pub fn stop_tree(&self, _tree_id: &str) -> Result<bool, NotPorted> {
        Err(NotPorted("Supervisor.stop_tree"))
    }

    /// supervisor.py:1810 `pause_tree`.
    pub fn pause_tree(&self, _tree_id: &str) -> Result<(), NotPorted> {
        Err(NotPorted("Supervisor.pause_tree"))
    }

    /// supervisor.py:6745 `correct_from_question`.
    pub fn correct_from_question(&self) -> Result<(), NotPorted> {
        Err(NotPorted("Supervisor.correct_from_question"))
    }

    /// supervisor.py:7054 `route_question`.
    pub fn route_question(&self) -> Result<(), NotPorted> {
        Err(NotPorted("Supervisor.route_question"))
    }

    /// supervisor.py:2279 `cancel_all`, :2291 `suspend_plans`.
    pub fn suspend_plans(&self) -> Result<(), NotPorted> {
        Err(NotPorted("Supervisor.suspend_plans"))
    }
}

/// An error a flow ends with, as Python's exception would have ended it.
#[derive(Clone, Debug, PartialEq)]
pub enum Failure {
    /// A path not ported yet.
    NotPorted(NotPorted),
    /// An exception by its class name and words (`type(e).__name__`, `str(e)`).
    Raised { class: String, message: String },
    /// The worker's failure classes (the planner's call).
    Worker(ferb_worker::Error),
    /// `PlanRejected(problems)`.
    PlanRejected(Vec<String>),
    /// The flow was cancelled (a newer message, a stop).
    Cancelled,
}

impl Failure {
    pub fn raised(class: &str, message: impl Into<String>) -> Failure {
        Failure::Raised {
            class: class.to_string(),
            message: message.into(),
        }
    }

    pub fn class(&self) -> String {
        match self {
            Failure::NotPorted(_) => "NotImplementedError".into(),
            Failure::Raised { class, .. } => class.clone(),
            Failure::Worker(e) => e.class(),
            Failure::PlanRejected(_) => "PlanRejected".into(),
            Failure::Cancelled => "CancelledError".into(),
        }
    }

    /// `str(exc)`.
    pub fn message(&self) -> String {
        match self {
            Failure::NotPorted(n) => n.to_string(),
            Failure::Raised { message, .. } => message.clone(),
            Failure::Worker(e) => e.to_string(),
            Failure::PlanRejected(p) => plan_rejected(p),
            Failure::Cancelled => String::new(),
        }
    }
}

impl From<NotPorted> for Failure {
    fn from(n: NotPorted) -> Failure {
        Failure::NotPorted(n)
    }
}

impl From<ferb_orchestrator::sql::Error> for Failure {
    fn from(e: ferb_orchestrator::sql::Error) -> Failure {
        let class = match &e {
            ferb_orchestrator::sql::Error::Value(_) => "ValueError",
            ferb_orchestrator::sql::Error::Key(_) => "KeyError",
            _ => "OperationalError",
        };
        Failure::raised(class, e.to_string())
    }
}

impl From<ferb_store::Error> for Failure {
    fn from(e: ferb_store::Error) -> Failure {
        Failure::raised("OperationalError", e.to_string())
    }
}

impl From<rusqlite::Error> for Failure {
    fn from(e: rusqlite::Error) -> Failure {
        Failure::raised("OperationalError", e.to_string())
    }
}

/// errors.py `PlanRejected`'s words: which checks the plan still failed.
fn plan_rejected(problems: &[String]) -> String {
    format!(
        "The planner's answer still had problems after one repair: {}",
        problems.join("; ")
    )
}

pub type Flow<T> = Result<T, Failure>;
