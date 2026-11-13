//! The supervisor in the composition (`areas/compose.py` `Supervisor(...)`, main.py's start and shutdown): the
//! planner of ferb-supervisor built over this daemon's parts through its [`Daemon`] seam, its chat RPCs on the
//! server, its seams into group A (scheduled runs, the task store), its observer on the gateway, the global stop's
//! `runtimes` hook, and FERB's own questions inside a task.
//!
//! [`SupDaemon`] is the seam: the gateway's reads and run tokens, the request facts, the live task records, the bus a
//! chat turn subscribes to, the change records, the runtime factory, schedules from chat. What a real daemon computes
//! about the Mac and FERB (the environment, the facts in every role's instructions, the relevant activity, the desktop
//! observer, the model runtime) is [`Reads`]: [`RealReads`] here, the recorder's stand-ins in a test build.
//!
//! Also here: chat capture (`remember`, group A), the task questions in the gateway's one store
//! (`GatewayTaskQuestions`), the front window at send (L12, [`super::front`]), the desktop observer
//! ([`super::desktop`]), episode cards (`on_episode`), the literal checks ([`super::checks`]), the browser drivers'
//! tabs, a loop's rest, notices and probes.
//!
//! Not wired, each named where it sits: the standing permissions and the named destinations (no public store on the
//! gateway), memory's push and Tier 1 (`identity_context`, `memory_push`: empty), the intro's preview (not ported in
//! ferb-supervisor: [`PREVIEW_NOT_PORTED`]). The supervisor never asks for the front window yet (its chat RPC sends
//! `screen: false`, and its planner answers NotPorted for a turn that has one).

use std::collections::{BTreeSet, HashMap};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use ferb_context::assemble::ContextBlock;
use ferb_core::types::Identity;
use ferb_daemon::areas_b::core::Host;
use ferb_gateway::gateway::Gateway;
use ferb_gateway::questions::{Approval, ResolveError};
use ferb_orchestrator::models::Condition;
use ferb_orchestrator::ports::BoxFuture;
use ferb_orchestrator::verify::ConditionResult;
use ferb_rpc::auth::RunGrant;
use ferb_rpc::{HandlerError, RunTokens};
use ferb_supervisor::flow::daemon_seams::GatewayTaskQuestions;
use ferb_supervisor::flow::owner_wait::Questions;
use ferb_supervisor::flow::{
    BusSink, Change, Daemon, Environment, FrontSeen, LiveTask, Request, Supervisor, Unsubscribe,
};
use ferb_supervisor::planner::OpenQuestion;
use ferb_supervisor::prompts::SystemInputs;
use ferb_supervisor::worker_context::Activity;
use ferb_worker::ports::{Grant, ModelRuntime};
use serde_json::{json, Map, Value};

use crate::basics::{lock, Events, Journal, SharedDb, Stop};

/// What `intro.preview` answers: ferb-supervisor's `Supervisor::preview` (ORCH-500) is not ported.
pub const PREVIEW_NOT_PORTED: &str =
    "not available in this build: the intro's plan-only preview (supervisor.py `preview`) isn't ported yet";

// ---------------------------------------------------------------------------------------------------------------
// what a real daemon computes; a test build answers from recordings
// ---------------------------------------------------------------------------------------------------------------

/// The reads the supervisor makes of the Mac and of FERB's own state, and the runtime factory: what a recorder's
/// stand-ins replace in a test build (`Overrides::supervisor_reads`).
pub trait Reads: Send + Sync {
    /// `environment(words)`.
    fn environment<'a>(&'a self, words: &'a str) -> BoxFuture<'a, Environment>;
    /// `live_facts(last_runs=..., owner_state=...)`.
    fn live_facts(&self, last_runs: bool, owner_state: bool) -> String;
    /// `_system`'s inputs.
    fn system_inputs(&self, live: bool) -> SystemInputs;
    /// `availability().text`.
    fn availability(&self) -> String;
    /// `relevant_context(words)`.
    fn relevant_context(&self, words: &str) -> Activity;
    /// The desktop observer (`disturbance.context`, `start`, `_tree_finished`'s `finish`).
    fn disturbance_context(&self, tree_id: &str) -> String;
    fn on_task_start<'a>(&'a self, tree_id: &'a str) -> BoxFuture<'a, ()>;
    fn on_task_finish<'a>(&'a self, tree_id: &'a str) -> BoxFuture<'a, ()>;
    /// (bundle id, name, pid) of the apps running now.
    fn running_apps(&self) -> Vec<(String, String, i64)>;
    /// The daemon's shutdown (the desktop observer's summaries of the trees it still watches).
    fn shutdown(&self) {}
    /// `_worker_types()`: the default and each runtime available now.
    fn worker_types(&self) -> Vec<String>;
    /// areas/chat.py:355 `self.front_window is not None`: messages are captured at send. None: as this daemon's parts
    /// say (the Mac's parts on).
    fn captures_at_send(&self) -> Option<bool> {
        None
    }
    /// The front window a message got at send; None: this daemon's own capture ([`super::front`]).
    fn front_capture<'a>(
        &'a self,
        _request_id: &'a str,
    ) -> Option<BoxFuture<'a, Result<Option<ferb_supervisor::flow::FrontCapture>, String>>> {
        None
    }
    /// `memory_push(query, targets)`: the ranked memory blocks a worker is given (L36 §4).
    fn memory_push<'a>(
        &'a self,
        _query: &'a str,
        _targets: &'a str,
    ) -> BoxFuture<'a, Vec<ContextBlock>> {
        // the worker's memory search (`_learning_candidates` with memory_search.WORKER): not wired in this build
        Box::pin(async { Vec::new() })
    }
    /// `model_limits(make_runtime("worker:" + type, ...))`: a worker type's documented limits, without starting it.
    fn worker_limits(&self, worker_type: &str) -> Option<ferb_core::types::ModelLimits>;
    /// `make_runtime(role, mode=, tool_names=, schema=, effort=)`.
    fn make_runtime(
        &self,
        role: &str,
        mode: &str,
        tool_names: &[String],
        schema: &Value,
        effort: Option<&str>,
    ) -> Result<Arc<dyn ModelRuntime>, ferb_worker::Error>;
}

/// The real reads: the environment area's port over [`super::env_seams::EnvSeams`], group B's runtimes.
pub struct RealReads {
    pub host: Host,
    pub seams: Arc<super::env_seams::EnvSeams>,
    pub runtimes: Arc<OnceLock<ferb_daemon::areas_b::runtimes::Runtimes>>,
    pub tools: super::tools::Parts,
    pub native: bool,
}

impl RealReads {
    fn availability_now(&self) -> String {
        ferb_daemon::areas_b::environment::availability(self.seams.as_ref()).text
    }
}

impl Reads for RealReads {
    fn environment<'a>(&'a self, words: &'a str) -> BoxFuture<'a, Environment> {
        let (host, seams, words) = (self.host.clone(), self.seams.clone(), words.to_string());
        Box::pin(async move {
            // its files are read off the loop (`_environment_files`)
            tokio::task::spawn_blocking(move || {
                ferb_daemon::areas_b::environment::environment(&host, seams.as_ref(), &words)
            })
            .await
            .ok()
            .and_then(Result::ok)
            .map(|e| Environment {
                stable: e.stable,
                live: e.live,
            })
            .unwrap_or_default()
        })
    }
    fn live_facts(&self, last_runs: bool, owner_state: bool) -> String {
        ferb_daemon::areas_b::environment::live_facts(self.seams.as_ref(), owner_state, last_runs)
    }
    fn system_inputs(&self, live: bool) -> SystemInputs {
        let skills = ferb_orchestrator::skills::Skills {
            directory: self.host.home.join("skills").to_string_lossy().into_owned(),
        }
        .catalog();
        let installed: BTreeSet<String> = self
            .tools
            .gateway()
            .map(|g| g.registry().names().into_iter().collect())
            .unwrap_or_default();
        let abilities = super::tools::context::abilities_catalog(self.tools.package(), &installed);
        SystemInputs {
            facts: ferb_daemon::areas_b::environment::facts(&self.host, self.seams.as_ref())
                .unwrap_or_default(),
            live_facts: if live {
                self.live_facts(false, true)
            } else {
                String::new()
            },
            capabilities: self.availability_now(),
            // Tier 1 (memory's core profile, `_core_profile_text`): not wired in this build
            identity_context: String::new(),
            skills_line: ferb_supervisor::prompts::catalog_line(&skills),
            abilities_line: ferb_supervisor::prompts::catalog_line(&abilities),
        }
    }
    fn availability(&self) -> String {
        self.availability_now()
    }
    fn worker_types(&self) -> Vec<String> {
        self.runtimes
            .get()
            .map(|r| r.worker_types())
            .unwrap_or_default()
    }
    fn worker_limits(&self, worker_type: &str) -> Option<ferb_core::types::ModelLimits> {
        let schema =
            ferb_context::schemas::worker_schema(ferb_context::schemas::WorkerFlags::default());
        let (build, _) = self
            .runtimes
            .get()?
            .make_runtime(
                &format!("worker:{worker_type}"),
                "worker",
                &[],
                &schema,
                None,
            )
            .ok()?;
        ferb_runtimes::catalog::limits(&build.name, Some(&build.model), &build.home).ok()
    }
    fn relevant_context(&self, words: &str) -> Activity {
        match ferb_daemon::areas_b::environment::relevant_context(self.seams.as_ref(), words) {
            Ok(r) => Activity::Labeled {
                text: r.text,
                origin: r.origin,
                payload: Some(r.data),
                refs: Some(r.refs),
            },
            Err(_) => Activity::Text(None),
        }
    }
    fn disturbance_context(&self, tree_id: &str) -> String {
        self.tools
            .sentinel()
            .map(|s| s.context(tree_id))
            .unwrap_or_default()
    }
    fn on_task_start<'a>(&'a self, tree_id: &'a str) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            if let Some(s) = self.tools.sentinel() {
                s.start(tree_id).await;
            }
        })
    }
    fn on_task_finish<'a>(&'a self, tree_id: &'a str) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            if let Some(s) = self.tools.sentinel() {
                s.finish(tree_id).await;
            }
        })
    }
    fn running_apps(&self) -> Vec<(String, String, i64)> {
        if !self.native {
            return Vec::new();
        }
        ferb_mac_hands::native::apps::running()
            .into_iter()
            .map(|a| (a.bundle_id, a.name, i64::from(a.pid)))
            .collect()
    }
    fn make_runtime(
        &self,
        role: &str,
        mode: &str,
        tool_names: &[String],
        schema: &Value,
        effort: Option<&str>,
    ) -> Result<Arc<dyn ModelRuntime>, ferb_worker::Error> {
        let rt = self.runtimes.get().ok_or_else(|| {
            ferb_worker::Error::from(ferb_core::errors::FerbError::RuntimeUnavailable(
                "FERB's model runtimes aren't set up yet".into(),
            ))
        })?;
        let (build, built) = rt.make_runtime(role, mode, tool_names, schema, effort)?;
        Ok(Arc::new(ferb_runtimes::worker::WorkerRuntime {
            adapter: built.adapter,
            limits: ferb_runtimes::catalog::limits(&build.name, Some(&build.model), &build.home),
            output_schema: (!build.schema.is_null()).then(|| build.schema.clone()),
        }))
    }
}

// ---------------------------------------------------------------------------------------------------------------
// the seam
// ---------------------------------------------------------------------------------------------------------------

/// Everything the supervisor reads from, or hands to, the rest of this daemon.
pub struct SupDaemon {
    pub me: std::sync::Weak<SupDaemon>,
    pub gateway: Arc<Gateway>,
    pub tokens: Arc<RunTokens>,
    pub tools: super::tools::Parts,
    pub db: SharedDb,
    pub clock: Arc<dyn ferb_core::interfaces::Clock>,
    pub ids: Arc<dyn ferb_core::interfaces::IdSource>,
    pub redactor: Arc<ferb_sanitize::redact::Redactor>,
    pub events: Arc<Events>,
    pub journal: Arc<Journal>,
    pub stop: Arc<Stop>,
    pub ledger: Arc<super::gateway::RealLedger>,
    /// The environment area's seams: the browser drivers' tabs a checker, a loop round and a task's end read.
    pub env: Arc<super::env_seams::EnvSeams>,
    pub reads: Arc<dyn Reads>,
    pub home: String,
    pub user_home: String,
    pub workspace: String,
    /// The Mac's zone (the daemon clock's): when a card was asked.
    pub zone: ferb_clock::zone::Zone,
    /// Group A's state (schedules from chat), once registered.
    pub a: Arc<OnceLock<ferb_daemon::shared::Local<ferb_daemon::areas_a::State>>>,
    /// Group B's runtimes (an account fact a call reported).
    pub runtimes: Arc<OnceLock<ferb_daemon::areas_b::runtimes::Runtimes>>,
    /// The supervisor, once built (the journal rows of `_question_record`).
    pub sup: OnceLock<Supervisor>,
    /// A loop notice's id (`uuid4().hex[:12]`; a test build: the recorder's counter).
    pub notice_hex: OnceLock<Arc<dyn Fn() -> String + Send + Sync>>,
    requests: Mutex<HashMap<String, Request>>,
    /// The planner's lookups: worker id -> the only tools its token offers (ORCH-516).
    only: Mutex<HashMap<String, Vec<String>>>,
    unattended: Mutex<BTreeSet<String>>,
    /// The bus a chat turn subscribes to: (subscriber, topics, sink).
    bus: Mutex<Vec<(usize, Vec<String>, BusSink)>>,
    subscribers: AtomicUsize,
    live_tasks: Mutex<HashMap<String, LiveTask>>,
    /// compose.py `_asked_kinds`, `_answered_ids`.
    asked_kinds: Mutex<HashMap<String, String>>,
    answered_ids: Mutex<BTreeSet<String>>,
}

/// What [`SupDaemon::new`] takes.
pub struct SupParts {
    pub gateway: Arc<Gateway>,
    pub tokens: Arc<RunTokens>,
    pub tools: super::tools::Parts,
    pub db: SharedDb,
    pub clock: Arc<dyn ferb_core::interfaces::Clock>,
    pub ids: Arc<dyn ferb_core::interfaces::IdSource>,
    pub redactor: Arc<ferb_sanitize::redact::Redactor>,
    pub events: Arc<Events>,
    pub journal: Arc<Journal>,
    pub stop: Arc<Stop>,
    pub ledger: Arc<super::gateway::RealLedger>,
    pub env: Arc<super::env_seams::EnvSeams>,
    pub reads: Arc<dyn Reads>,
    pub home: String,
    pub user_home: String,
    pub zone: ferb_clock::zone::Zone,
    pub a: Arc<OnceLock<ferb_daemon::shared::Local<ferb_daemon::areas_a::State>>>,
    pub runtimes: Arc<OnceLock<ferb_daemon::areas_b::runtimes::Runtimes>>,
}

impl SupDaemon {
    pub fn new(p: SupParts) -> Arc<SupDaemon> {
        let workspace = format!("{}/workspace", p.home);
        let d = Arc::new_cyclic(|me| SupDaemon {
            me: me.clone(),
            gateway: p.gateway,
            tokens: p.tokens,
            tools: p.tools,
            db: p.db,
            clock: p.clock,
            ids: p.ids,
            redactor: p.redactor,
            events: p.events,
            journal: p.journal,
            stop: p.stop,
            ledger: p.ledger,
            env: p.env,
            reads: p.reads,
            home: p.home,
            user_home: p.user_home,
            workspace,
            zone: p.zone,
            a: p.a,
            runtimes: p.runtimes,
            sup: OnceLock::new(),
            notice_hex: OnceLock::new(),
            requests: Mutex::default(),
            only: Mutex::default(),
            unattended: Mutex::default(),
            bus: Mutex::default(),
            subscribers: AtomicUsize::new(0),
            live_tasks: Mutex::default(),
            asked_kinds: Mutex::default(),
            answered_ids: Mutex::default(),
        });
        // the daemon's event consumers a chat turn and the questions need (compose.py `_wire_events`): the plans that
        // wait for an event, the bus a turn subscribes to, then `_question_record`
        {
            let me = Arc::downgrade(&d);
            d.events.events.on(
                None,
                Arc::new(move |topic: &str, data: Map<String, Value>| {
                    if let Some(sup) = me.upgrade().and_then(|d| d.sup.get().cloned()) {
                        sup.inner().notify_plan_event(topic, &Value::Object(data));
                    }
                }),
            );
            let me = Arc::downgrade(&d);
            d.events.events.on(
                None,
                Arc::new(move |topic: &str, data: Map<String, Value>| {
                    if let Some(d) = me.upgrade() {
                        d.bus_publish(topic, &ferb_sanitize::Py::from(&Value::Object(data)));
                    }
                }),
            );
            let me = Arc::downgrade(&d);
            d.events.events.on(
                Some(&["approval.requested", "approval.resolved"]),
                Arc::new(move |topic: &str, data: Map<String, Value>| {
                    if let Some(d) = me.upgrade() {
                        d.question_record(topic, &data);
                    }
                }),
            );
        }
        d
    }

    fn approval(&self, id: &str) -> Option<Approval> {
        self.gateway.approval(id)
    }

    /// compose.py `_publish`: a task's question names its request (the event contract), then the events.
    pub fn publish_event(&self, topic: &str, data: Map<String, Value>) {
        let mut data = data;
        if topic == "approval.requested"
            && data.get("task_id").is_some_and(ferb_py::value::truthy)
            && !data.contains_key("request_id")
        {
            let id = data
                .get("approval_id")
                .map(ferb_py::value::py_str)
                .unwrap_or_default();
            if let Some(ap) = self.approval(&id).filter(|a| !a.request_id.is_empty()) {
                data.insert("request_id".into(), json!(ap.request_id));
            }
        }
        self.events.publish(topic, data);
    }

    fn bus_publish(&self, topic: &str, data: &ferb_sanitize::Py) {
        let sinks: Vec<BusSink> = lock(&self.bus)
            .iter()
            .filter(|(_, topics, _)| {
                topics.iter().any(|t| match t.strip_suffix('*') {
                    Some(prefix) => topic.starts_with(prefix),
                    None => t == topic,
                })
            })
            .map(|(_, _, sink)| sink.clone())
            .collect();
        for sink in sinks {
            sink(topic, data);
        }
    }

    /// compose.py `_question_record` (DMN-583): every question FERB asks, with its kind, and how it was answered.
    fn question_record(&self, topic: &str, data: &Map<String, Value>) {
        let id = data
            .get("approval_id")
            .map(ferb_py::value::py_str)
            .unwrap_or_default();
        let Some(ap) = self.approval(&id) else {
            return;
        };
        let first = |t: &str| -> String {
            t.lines()
                .map(str::trim)
                .find(|l| !l.is_empty())
                .unwrap_or("")
                .chars()
                .take(200)
                .collect()
        };
        let given = data
            .get("question_kind")
            .filter(|k| ferb_py::value::truthy(k))
            .map(ferb_py::value::py_str);
        let kind = given.unwrap_or_else(|| {
            if ["owner_login_code", "owner_admin_password", "owner_secret"]
                .contains(&ap.kind.as_str())
            {
                "secret".to_string()
            } else {
                // compose.py:23 `question_kind`
                match ap.capability.as_str() {
                    "ferb.confirm" => "confirm",
                    "ferb.retry" => "retry",
                    "conversation.approve" => "conversation_scope",
                    "plan.ask" | "owner.ask" => "task",
                    "downloads.delete" => "downloads",
                    "browsing.choose" => "browsing",
                    "loop.ask" => "loop",
                    "runtime.switch" => "runtime_switch",
                    // steering.py MEMORY_ASKS (a forget asked in chat)
                    "memory.forget" => "memory",
                    other => other,
                }
                .to_string()
            }
        });
        if topic == "approval.requested" {
            lock(&self.asked_kinds).insert(ap.id.clone(), kind.clone());
            let asked = first(if ap.summary.is_empty() {
                &ap.reason
            } else {
                &ap.summary
            });
            let task_id = match data.get("task_id").filter(|t| ferb_py::value::truthy(t)) {
                Some(t) => t.clone(),
                None if !ap.task_id.is_empty() => json!(ap.task_id),
                None => Value::Null,
            };
            let mut row = json!({"approval_id": ap.id, "kind": kind, "capability": ap.capability,
                "task_id": task_id, "request_id": ap.request_id});
            if !asked.is_empty() {
                row["question"] = json!(asked);
            }
            let exact = first(&ap.exact);
            if !exact.is_empty() {
                row["exact"] = json!(exact);
            }
            self.journal_row("question.asked", row);
            return;
        }
        if ap.state == ferb_gateway::questions::State::Pending
            || !lock(&self.answered_ids).insert(ap.id.clone())
        {
            return;
        }
        let kind = lock(&self.asked_kinds).remove(&ap.id).unwrap_or(kind);
        let answer = ap.answer.clone().unwrap_or_default();
        let choice = ap.choice.clone().unwrap_or_default();
        let secret = ["owner_login_code", "owner_admin_password", "owner_secret"]
            .contains(&ap.kind.as_str());
        let said = if ap.hidden.is_empty() && !secret {
            first(&answer)
        } else {
            String::new()
        };
        let mut row = json!({"approval_id": ap.id, "kind": kind, "state": ap.state.as_str(),
            "answer": if !choice.is_empty() { "option" } else if !answer.is_empty() { "words" } else { "none" }});
        if !choice.is_empty() {
            row["choice"] = json!(choice);
        }
        if !said.is_empty() && choice.is_empty() {
            row["said"] = json!(said);
        }
        self.journal_row("question.answered", row);
    }

    fn journal_row(&self, kind: &str, row: Value) {
        match self.sup.get() {
            Some(sup) => {
                sup.inner().journal(kind, row, Some("ferb"));
            }
            None => {
                let _ = self.journal.append(kind, row, Some("ferb"));
            }
        }
    }

    fn run_grant(&self, grant: &Grant) -> RunGrant {
        RunGrant {
            task_id: grant.task_id.clone(),
            worker_id: grant.worker_id.clone(),
            scopes: grant.scopes.clone(),
            identity: match grant.identity {
                Identity::Ferb => ferb_rpc::auth::Identity::Ferb,
                Identity::Owner => ferb_rpc::auth::Identity::Owner,
            },
            expires_at: f64::MAX,
            observe_only: grant.observe_only,
            tools: lock(&self.only)
                .get(&grant.worker_id)
                .map(|t| t.iter().cloned().collect())
                .unwrap_or_default(),
            max_calls: 0,
        }
    }

    /// What the chat RPCs read of the gateway.
    pub fn chat_gateway(self: &Arc<Self>) -> Arc<dyn ferb_supervisor::flow::chat_rpc::ChatGateway> {
        Arc::new(ChatGw(self.clone()))
    }
}

impl Daemon for SupDaemon {
    fn tool_names(&self) -> Vec<String> {
        self.gateway.registry().names()
    }
    fn all_scopes(&self) -> BTreeSet<String> {
        self.gateway
            .registry()
            .tools()
            .iter()
            .flat_map(|t| t.scopes())
            .collect()
    }
    fn observes(&self, tool: &str) -> bool {
        self.gateway
            .registry()
            .get(tool)
            .is_some_and(|t| t.facts().observation)
    }
    fn reads_content(&self, tool: &str) -> bool {
        // `reads_content = True` (L39 M8): the two tools that read what a file says or shows
        matches!(tool, "fs.read" | "fs.look") && self.gateway.registry().get(tool).is_some()
    }
    fn tools_for(&self, grant: &Grant) -> Vec<Value> {
        super::rpc::tools_for(&self.gateway, &self.tools, &self.run_grant(grant))
    }
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
    ) -> String {
        if let Some(t) = tools {
            lock(&self.only).insert(worker_id.to_string(), t.to_vec());
        }
        self.tokens.issue(
            task_id,
            worker_id,
            ferb_rpc::auth::GrantSpec {
                scopes: scopes.clone(),
                identity: if identity == "ferb" {
                    ferb_rpc::auth::Identity::Ferb
                } else {
                    ferb_rpc::auth::Identity::Owner
                },
                ttl_s,
                observe_only,
                tools: tools
                    .map(|t| t.iter().cloned().collect())
                    .unwrap_or_default(),
                max_calls: max_calls.unwrap_or(0) as u64,
            },
        )
    }
    fn validate_token(&self, token: &str) -> Option<Grant> {
        let g = self.tokens.validate(token).ok()?;
        Some(Grant {
            task_id: g.task_id.clone(),
            worker_id: g.worker_id.clone(),
            scopes: g.scopes.clone(),
            identity: match g.identity {
                ferb_rpc::auth::Identity::Ferb => Identity::Ferb,
                ferb_rpc::auth::Identity::Owner => Identity::Owner,
            },
            observe_only: g.observe_only,
        })
    }
    fn revoke_worker(&self, worker_id: &str) {
        self.tokens.revoke_worker(worker_id);
        lock(&self.only).remove(worker_id);
    }
    fn forget_calls(&self, worker_id: &str) {
        self.gateway.forget_calls(worker_id)
    }
    fn calls_refused(&self, watch_id: &str) -> u64 {
        self.gateway.calls_refused(watch_id)
    }
    fn lookups_made(&self, holder: &str) -> usize {
        self.gateway.lookups_made(holder) as usize
    }
    fn initial_image_given(&self, task_id: &str) {
        self.gateway.initial_image_given(task_id)
    }
    fn unresolved(&self, root: &str) -> Vec<Map<String, Value>> {
        self.ledger
            .with(|l| l.unresolved(root))
            .unwrap_or_default()
            .into_iter()
            .filter_map(|v| v.as_object().cloned())
            .collect()
    }
    fn forget_tasks(&self, ids: &BTreeSet<String>) {
        self.gateway.forget_tasks(ids)
    }
    fn task_folders(&self, tree_id: &str, folders: Option<&[String]>) {
        self.gateway
            .set_task_folders(tree_id, folders.map(<[String]>::to_vec).unwrap_or_default())
    }
    fn record_named(&self, _words: &str, _request_id: &str) {
        // SQ6's named destinations: ferb-gateway keeps its destination store private (seam gap, reported)
    }
    fn standing_revoke(&self, _tree_id: &str) {
        // standing permissions: ferb-gateway keeps its standing store private (seam gap, reported)
    }
    fn standing_scopes(&self, _tree_id: &str) -> Vec<Value> {
        Vec::new()
    }
    fn record_request(&self, request: &Request) {
        let set = |v: &Vec<String>| v.iter().cloned().collect::<BTreeSet<String>>();
        self.gateway.record_request(ferb_policy::small::Request {
            request_id: request.request_id.clone(),
            words: request.words.clone(),
            targets: ferb_policy::small::Targets {
                apps: set(&request.targets[0]),
                sites: set(&request.targets[1]),
                paths: set(&request.targets[2]),
                recipients: set(&request.targets[3]),
            },
            payload: request.payload.clone(),
            asked_origins: request.asked_origins.clone(),
        });
        lock(&self.requests).insert(request.request_id.clone(), request.clone());
    }
    fn request(&self, request_id: &str) -> Request {
        lock(&self.requests)
            .get(request_id)
            .cloned()
            .unwrap_or(Request {
                request_id: request_id.into(),
                ..Request::default()
            })
    }
    fn forget_request(&self, request_id: &str) {
        self.gateway.forget_request(request_id);
        lock(&self.requests).remove(request_id);
    }
    fn unattended(&self, request_id: &str) -> bool {
        lock(&self.unattended).contains(request_id)
    }
    fn pending_questions(&self) -> Vec<OpenQuestion> {
        if let Some(sup) = self.sup.get() {
            return sup.inner().open_questions();
        }
        let zone = self.zone;
        let checks = &ferb_supervisor::words::FERB_CHECKS;
        self.gateway
            .pending()
            .into_iter()
            .map(|ap| {
                let routable = ferb_supervisor::planner::routable(&ap.kind, &ap.capability, checks);
                let asked_at = self
                    .journal
                    .of_kind("question.asked", "approval_id", &ap.id)
                    .last()
                    .and_then(|(at, _)| ferb_clock::py::parse(at));
                let mut row = ferb_supervisor::planner::card_row(
                    &ap.kind,
                    &ap.capability,
                    &ap.summary,
                    &ap.reason,
                    asked_at.as_ref(),
                    routable,
                    &zone,
                );
                row.remove("answered_in");
                let task_label = self.task_label(&ap.task_id);
                OpenQuestion {
                    id: ap.id,
                    kind: ap.kind,
                    capability: ap.capability,
                    reason: ap.reason,
                    options: ap.options,
                    task_label,
                    card_row: row,
                }
            })
            .collect()
    }
    fn environment<'a>(&'a self, words: &'a str) -> BoxFuture<'a, Environment> {
        self.reads.environment(words)
    }
    fn live_facts(&self, last_runs: bool, owner_state: bool) -> String {
        self.reads.live_facts(last_runs, owner_state)
    }
    fn system_inputs(&self, live: bool) -> SystemInputs {
        self.reads.system_inputs(live)
    }
    fn availability(&self) -> String {
        self.reads.availability()
    }
    fn worker_types(&self) -> Vec<String> {
        self.reads.worker_types()
    }
    fn signals(&self) -> (Option<ferb_clock::py::Time>, Option<ferb_clock::py::Time>) {
        // the owner-input tap and the trail's last event: neither runs in this build
        (None, None)
    }
    fn relevant_context(&self, words: &str) -> Activity {
        self.reads.relevant_context(words)
    }
    fn apps(&self) -> Vec<(String, String)> {
        let host = self.host();
        ferb_daemon::areas_b::desktop::installed_apps(&host).unwrap_or_default()
    }
    fn running_apps(&self) -> Vec<(String, String, i64)> {
        self.reads.running_apps()
    }
    fn shell_process(&self, id: &str) -> Option<Map<String, Value>> {
        let shells = self.tools.shells()?;
        let hit = shells.list().into_iter().find(|p| p.id == id)?;
        Some(crate::basics::object(
            json!({"command": hit.command, "started": hit.started, "owner_task": hit.owner_task,
                   "running": hit.running}),
        ))
    }
    fn runtime_account(&self, runtime: &str, account: &str) {
        if let Some(r) = self.runtimes.get() {
            let _ = r.runtime_account(runtime, account);
        }
    }
    fn on_violation(&self, runtime: &str, text: &str) {
        // runtimes.py `_runtime_violation`
        self.journal.note(
            "runtime.failed",
            json!({"runtime": runtime, "detail": text}),
        );
    }
    fn has_route(&self) -> Option<bool> {
        None
    }
    fn identity_pending(&self) -> String {
        String::new()
    }
    fn settle(&self) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            if let Some(a) = self.a.get() {
                let _ = a
                    .call(|st| async move {
                        ferb_daemon::areas_a::memory::settle_forgets(&st).await;
                    })
                    .await;
            }
        })
    }
    fn memory_push<'a>(
        &'a self,
        query: &'a str,
        targets: &'a str,
    ) -> BoxFuture<'a, Vec<ContextBlock>> {
        self.reads.memory_push(query, targets)
    }
    /// memory.py `_capture_learning_episode`: the finished task's episode card, on group A's loop (its reads off it).
    fn on_episode<'a>(&'a self, tree_id: &'a str) -> BoxFuture<'a, ()> {
        let a = self.a.get().cloned();
        let task = tree_id.to_string();
        Box::pin(async move {
            if let Some(a) = a {
                let _ = a
                    .call(move |st| async move {
                        ferb_daemon::areas_a::cards::capture_learning_episode(&st, &task).await
                    })
                    .await;
            }
        })
    }
    fn last_run_origins(&self) -> BTreeSet<String> {
        self.a
            .get()
            .and_then(|a| {
                a.call_blocking(|st| async move {
                    ferb_daemon::areas_a::schedules::last_run_origins(&st).unwrap_or_default()
                })
                .ok()
            })
            .unwrap_or_default()
    }
    fn on_task_start<'a>(&'a self, tree_id: &'a str) -> BoxFuture<'a, ()> {
        self.reads.on_task_start(tree_id)
    }
    fn on_task_finish<'a>(&'a self, tree_id: &'a str) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            // desktop.py `_tree_finished`: its run tokens end with it, the observer's summary, then its change
            // record's copies go
            self.tokens.revoke_tree(tree_id);
            self.reads.on_task_finish(tree_id).await;
            let stuck = self.tools.changes().forget(tree_id);
            if !stuck.is_empty() {
                self.journal.note(
                    "changes.cleanup_failed",
                    json!({"at": "tree", "paths": stuck.len()}),
                );
            }
        })
    }
    fn disturbance_context(&self, tree_id: &str) -> String {
        self.reads.disturbance_context(tree_id)
    }
    fn on_task_end<'a>(
        &'a self,
        _task_id: &'a str,
        _span_id: &'a str,
    ) -> BoxFuture<'a, Vec<Value>> {
        // desktop.py `_task_end`: hearing, recordings, launched apps and the browsers' tabs end with the task; none
        // of those is held for a task by this build's composition
        Box::pin(async { Vec::new() })
    }
    fn changes(&self, task_id: &str, tree_id: &str) -> Option<Result<Vec<Change>, String>> {
        let changes = self.tools.changes();
        let found = changes.changes(task_id, Some(tree_id));
        Some(Ok(found
            .iter()
            .map(|c| Change {
                path: c.path.clone(),
                what: c.what.clone(),
                source: c.source.clone(),
                content: c.content,
                named: c.named,
                text: changes.describe(c),
            })
            .collect()))
    }
    fn untrusted_roots(&self) -> Vec<String> {
        let downloads = std::path::Path::new(&self.workspace).join("downloads");
        vec![std::fs::canonicalize(&downloads)
            .unwrap_or(downloads)
            .to_string_lossy()
            .into_owned()]
    }
    fn browser_context(&self, task_id: &str) -> String {
        ferb_daemon::areas_b::environment::browser_context(&self.host(), self.env.as_ref(), task_id)
    }
    fn task_tabs(&self, task_id: &str) -> Vec<Value> {
        ferb_daemon::areas_b::environment::task_tabs(&self.host(), self.env.as_ref(), task_id)
    }
    fn keep_tabs_open(&self, task_id: &str) {
        ferb_daemon::areas_b::environment::keep_tabs_open(self.env.as_ref(), task_id)
    }
    /// notify.py `_loop_notice` (L31): a loop's back-off or pause notice in the owner's chat (`_tell_owner`: stored,
    /// journaled `chat.notice`, published; a banner while no FERB client shows it).
    fn loop_notice<'a>(
        &'a self,
        tree_id: &'a str,
        step_id: &'a str,
        text: &'a str,
        origin: &'a str,
    ) -> BoxFuture<'a, Result<(), String>> {
        let hex = self
            .notice_hex
            .get()
            .map(|f| f())
            .unwrap_or_else(|| ferb_runtimes::codex::token_hex(6));
        let ident = format!("loop-{step_id}-{hex}");
        let notifier = super::areas::NOTIFIER.get().cloned();
        Box::pin(async move {
            match notifier {
                Some(n) => {
                    n.tell_owner(
                        &ident,
                        &[format!("task:{tree_id}")],
                        text,
                        origin,
                        "loop",
                        tree_id,
                        &[],
                        &Value::Null,
                        None,
                    )
                    .await
                }
                None => Err("the owner's notices aren't built yet".into()),
            }
        })
    }
    /// `gateway.caps.get(tool)` as a probe's checks read it (`probes.refusal`): the registered tool's scopes,
    /// identities and facts; a browser verb's look-only facts (`moves_view`, `holds_tab`, `probe_keys`).
    fn probe_cap(&self, tool: &str) -> Option<Arc<dyn ferb_orchestrator::ports::CapFacts>> {
        let found = self.gateway.registry().get(tool).cloned()?;
        Some(Arc::new(ProbeCap::of(tool, found)))
    }
    /// `gateway.probe(RunGrant(task_id, worker_id, scopes, identity, inf, observe_only=True), tool, args, span)`.
    fn probe_read<'a>(
        &'a self,
        task_id: &'a str,
        worker_id: &'a str,
        scopes: &'a BTreeSet<String>,
        identity: &'a str,
        tool: &'a str,
        args: &'a Map<String, Value>,
        span: &'a str,
    ) -> BoxFuture<'a, Result<Value, String>> {
        let grant = RunGrant {
            task_id: task_id.to_string(),
            worker_id: worker_id.to_string(),
            scopes: scopes.clone(),
            identity: if identity == "ferb" {
                ferb_rpc::auth::Identity::Ferb
            } else {
                ferb_rpc::auth::Identity::Owner
            },
            expires_at: f64::INFINITY,
            observe_only: true,
            tools: BTreeSet::new(),
            max_calls: 0,
        };
        let (gw, tool, args, span) = (
            self.gateway.clone(),
            tool.to_string(),
            args.clone(),
            span.to_string(),
        );
        Box::pin(async move {
            // `Gateway::probe`'s future is not Send (its argument model's hooks are held across an await): it runs to
            // its end on a blocking thread of this runtime
            let handle = tokio::runtime::Handle::current();
            let read = tokio::task::spawn_blocking(move || {
                handle.block_on(gw.probe(&grant, &tool, &args, &span))
            })
            .await
            .map_err(|e| format!("the probe's read stopped: {e}"))?;
            match read {
                Ok(m) => Ok(Value::Object(m)),
                Err(ferb_gateway::gateway::CallError::Internal(m)) => Err(m),
                Err(ferb_gateway::gateway::CallError::Cancelled) => {
                    Err("the read was cancelled".into())
                }
            }
        })
    }
    /// `changes.reset(task_id)` (L39): a loop round's check reads only the files this round changed.
    fn changes_reset(&self, task_id: &str) -> Vec<String> {
        self.tools.changes().reset(task_id)
    }
    /// environment.py `_loop_rest` (L31): each browser driver gives the owner's held tab back between a loop's
    /// rounds, or takes it for the round.
    fn loop_rest<'a>(
        &'a self,
        task_id: &'a str,
        resting: bool,
    ) -> BoxFuture<'a, Result<(), String>> {
        Box::pin(async move {
            ferb_daemon::areas_b::environment::loop_rest(self.env.as_ref(), task_id, resting).await;
            Ok(())
        })
    }
    /// verify.py `Checks.code_checkable`: only literal predicates are code-checked (V6).
    fn code_checkable(&self, condition: &Condition, task_id: &str) -> bool {
        super::checks::checkable(&self.tools, condition, task_id)
    }
    /// verify.py `Checks.run`.
    fn code_check<'a>(
        &'a self,
        index: i64,
        condition: &'a Condition,
        task_id: &'a str,
    ) -> BoxFuture<'a, ConditionResult> {
        Box::pin(async move { super::checks::run(&self.tools, index, condition, task_id) })
    }
    fn declined(
        &self,
        _index: i64,
        _condition: &Condition,
        _task_id: &str,
    ) -> Option<ConditionResult> {
        None
    }
    fn sites_contacted(&self, tree_id: &str) -> Vec<String> {
        let db = lock(&self.db);
        ferb_policy::stores::sites_contacted(&db, tree_id).unwrap_or_default()
    }
    fn schedule_label(&self, schedule_id: &str) -> String {
        // supervisor.py:4456 `_schedule_label`: its name, else its instruction's first clause (ORCH-542)
        let db = lock(&self.db);
        db.conn()
            .query_row(
                "SELECT s.request, n.name FROM schedules s LEFT JOIN schedule_names n ON n.schedule_id = s.id \
                 WHERE s.id = ?",
                [schedule_id],
                |r| Ok((r.get::<_, Option<String>>(0)?, r.get::<_, Option<String>>(1)?)),
            )
            .map(|(request, name)| {
                ferb_orchestrator::names::shown_name(
                    name.as_deref().filter(|n| !n.is_empty()),
                    &request.unwrap_or_default(),
                )
            })
            .unwrap_or_else(|_| schedule_id.to_string())
    }
    fn left_running(&self, resources: &[Value]) -> Vec<String> {
        ferb_mac_hands::launched::left_running(resources)
    }
    fn ran_cards(&self, _tree_id: &str) -> Vec<Value> {
        Vec::new()
    }
    fn write_notes(&self, path: &str, text: &str) -> Result<(), String> {
        let p = std::path::Path::new(path);
        let notes = std::path::Path::new(&self.workspace);
        if !p.starts_with(notes) {
            return Err(format!("{path} is outside FERB's workspace"));
        }
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let tmp = p.with_extension("tmp-notes");
        std::fs::write(&tmp, text).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, p).map_err(|e| e.to_string())
    }
    /// L12 (desktop.py `_front_window`, supervisor.py `_screen_at_send`): the owner's front window as the message
    /// arrived, waited for at most `FRONT_WINDOW_WAIT_S`.
    fn front_window<'a>(&'a self, request_id: &'a str) -> BoxFuture<'a, Option<FrontSeen>> {
        Box::pin(async move {
            super::front::at_send(&self.tools, request_id)
                .await
                .map(|c| FrontSeen {
                    facts: c.window.facts,
                    image: c.image,
                    origin: c.window.origin,
                })
        })
    }
    fn captures_at_send(&self) -> bool {
        self.reads
            .captures_at_send()
            .unwrap_or_else(|| self.tools.mac().is_some())
    }
    fn front_window_capture<'a>(
        &'a self,
        request_id: &'a str,
    ) -> BoxFuture<'a, Result<Option<ferb_supervisor::flow::FrontCapture>, String>> {
        if let Some(recorded) = self.reads.front_capture(request_id) {
            return recorded;
        }
        Box::pin(async move { Ok(super::front::at_send(&self.tools, request_id).await) })
    }
    fn make_runtime(
        &self,
        role: &str,
        mode: &str,
        tool_names: &[String],
        schema: &Value,
        effort: Option<&str>,
    ) -> Result<Arc<dyn ModelRuntime>, ferb_worker::Error> {
        self.reads
            .make_runtime(role, mode, tool_names, schema, effort)
    }
    fn publish(&self, topic: &str, data: Value) {
        self.publish_event(topic, crate::basics::object(data));
    }
    fn cancel_tasks(&self, ids: &BTreeSet<String>) {
        self.gateway.cancel_tasks(ids)
    }
    fn interrupt_tasks(&self, ids: &BTreeSet<String>) {
        self.gateway.interrupt_tasks(ids)
    }
    fn in_flight(&self, ids: &BTreeSet<String>) -> bool {
        self.gateway.in_flight(ids)
    }
    fn stopped(&self) -> bool {
        self.stop.engaged()
    }
    fn questions(&self) -> Option<Arc<dyn Questions>> {
        // the supervisor's task questions live in the gateway's one store, as Python's `state.approvals`
        Some(Arc::new(GatewayTaskQuestions(self.gateway.clone())))
    }
    fn set_unattended(&self, request_id: &str) {
        lock(&self.unattended).insert(request_id.to_string());
        self.gateway.set_unattended(request_id, true);
    }
    fn reopen_noes(&self, tree_id: &str, request_id: &str, source: &str) {
        let _ = self.gateway.reopen_noes(tree_id, request_id, source);
    }
    fn schedules_chat(
        &self,
    ) -> Option<Arc<dyn ferb_supervisor::flow::schedules_chat::SchedulesChat>> {
        let local = self.a.get()?.clone();
        Some(Arc::new(
            ferb_supervisor::flow::daemon_seams::DaemonSchedules {
                local,
                home: self.user_home.clone(),
                workspace: self.workspace.clone(),
                blocking: false,
            },
        ))
    }
    fn mark_unknown_after_restart(&self) -> Vec<String> {
        self.ledger.mark_unknown_after_restart().unwrap_or_default()
    }
    fn task_record(&self, record: &LiveTask) {
        lock(&self.live_tasks).insert(record.task_id.clone(), record.clone());
        self.hold(record);
    }
    fn task_record_span(&self, task_id: &str, span_id: &str) {
        let rec = {
            let mut live = lock(&self.live_tasks);
            let Some(r) = live.get_mut(task_id) else {
                return;
            };
            r.span_id = span_id.to_string();
            r.clone()
        };
        self.hold(&rec);
    }
    fn task_record_removed(&self, task_id: &str) {
        lock(&self.live_tasks).remove(task_id);
        self.tools.release_task(&self.gateway, task_id);
    }
    fn subscribe(&self, topics: &[&str], sink: BusSink) -> Option<Unsubscribe> {
        let id = self.subscribers.fetch_add(1, Ordering::SeqCst);
        lock(&self.bus).push((id, topics.iter().map(|t| t.to_string()).collect(), sink));
        let me = self.me.clone();
        Some(Box::new(move || {
            if let Some(d) = me.upgrade() {
                lock(&d.bus).retain(|(i, _, _)| *i != id);
            }
        }))
    }
    fn approvals(&self) -> Vec<Approval> {
        self.gateway.pending()
    }
    fn approval(&self, id: &str) -> Option<Approval> {
        self.gateway.approval(id)
    }
    fn resolve_question(
        &self,
        id: &str,
        granted: bool,
        choice: Option<usize>,
        text: Option<&str>,
    ) -> Option<Approval> {
        ChatGw(self.me.upgrade()?)
            .resolve_on(id, granted, choice, text, &[])
            .ok()
    }
    fn supersede_question(&self, id: &str) -> bool {
        self.gateway.supersede(id).is_some()
    }
    fn worker_limits(&self, worker_type: &str) -> Option<ferb_core::types::ModelLimits> {
        self.reads.worker_limits(worker_type)
    }
    fn settle_identity(&self, why: &str) {
        let why = why.to_string();
        if let Some(a) = self.a.get() {
            let _ = a.call_blocking(move |st| async move {
                ferb_daemon::areas_a::memory::settle_core_profile(&st, &why)
            });
        }
    }
    fn forget_memory(&self, item_id: &str, request_id: &str) -> Option<String> {
        let (item, request) = (item_id.to_string(), request_id.to_string());
        let a = self.a.get()?;
        a.call_blocking(move |st| async move {
            ferb_daemon::areas_a::memory::chat_forget(&st, &item, &request)
                .unwrap_or_else(|e| format!("I couldn't forget that: {e}"))
        })
        .ok()
    }
    fn question_answered(&self, approval: &Approval) -> String {
        let (id, state, choice) = (
            approval.id.clone(),
            approval.state.as_str().to_string(),
            approval.choice.clone(),
        );
        let Some(a) = self.a.get() else {
            return String::new();
        };
        a.call_blocking(move |st| async move {
            ferb_daemon::areas_a::memory::question_answered(
                &st,
                &ferb_daemon::areas_a::memory::Answered {
                    approval_id: &id,
                    state: &state,
                    choice: choice.as_deref(),
                },
            )
            .unwrap_or_default()
        })
        .unwrap_or_default()
    }
    /// areas/memory.py `_remember` (chat capture), on group A's loop: ferb-daemon's `areas_a::remember`. An error
    /// Python would raise out of the turn (a store failure) is the item's refusal here, with its words.
    fn remember<'a>(
        &'a self,
        items: &'a [Value],
        words: &'a str,
        source: &'a str,
        sites: &'a [String],
        elsewhere: &'a [String],
        about: &'a str,
    ) -> BoxFuture<'a, Option<Vec<Value>>> {
        let a = self.a.get().cloned();
        let (items, words, source) = (items.to_vec(), words.to_string(), source.to_string());
        let (sites, elsewhere, about) = (sites.to_vec(), elsewhere.to_vec(), about.to_string());
        Box::pin(async move {
            let a = a?;
            a.call(move |st| async move {
                ferb_daemon::areas_a::remember::remember(
                    &st, &items, &words, &source, &sites, &elsewhere, &about,
                )
                .await
                .unwrap_or_else(|e| vec![json!({"refused": format!("{}: {e}", e.class())})])
            })
            .await
            .ok()
        })
    }
}

impl SupDaemon {
    fn hold(&self, rec: &LiveTask) {
        let Some(started) = ferb_clock::py::parse(&rec.started_at) else {
            return;
        };
        self.tools.hold_task(
            &self.gateway,
            ferb_gateway::gateway::TaskRecord {
                task_id: rec.task_id.clone(),
                request_id: rec.request_id.clone(),
                started_at: started,
                deadline: rec.deadline.as_deref().and_then(ferb_clock::py::parse),
                span_id: rec.span_id.clone(),
                tree_id: rec.tree_id.clone(),
            },
        );
    }

    fn host(&self) -> Host {
        Host {
            home: self.home.clone().into(),
            config: self.tools.config(),
            redactor: self.redactor.clone(),
            journal: self.journal.clone(),
            bus: self.events.clone(),
            clock: self.clock.clone(),
        }
    }

    /// ORCH-539 `_question_task`: the short label of the running task a question belongs to.
    fn task_label(&self, task_id: &str) -> String {
        if task_id.is_empty() {
            return String::new();
        }
        let Some(sup) = self.sup.get() else {
            return String::new();
        };
        let root = self
            .tools
            .task(task_id)
            .and_then(|t| t.tree_id)
            .unwrap_or_else(|| task_id.split('.').next().unwrap_or(task_id).to_string());
        let tree = sup.inner().live().tree_of.get(&root).cloned();
        tree.and_then(|t| t.lock().spec.as_ref().map(|s| s.goal.clone()))
            .map(|g| ferb_orchestrator::names::short_label(&g, 60))
            .unwrap_or_default()
    }

    /// supervisor.py:5671 `corrections`: how many corrections the owner steered into this task's tree so far.
    pub fn corrections(&self, task_id: &str) -> i64 {
        let Some(sup) = self.sup.get() else { return 0 };
        let root = self
            .tools
            .task(task_id)
            .and_then(|t| t.tree_id)
            .unwrap_or_else(|| task_id.split('.').next().unwrap_or(task_id).to_string());
        let tree = sup.inner().live().tree_of.get(&root).cloned();
        tree.and_then(|t| t.steering.try_lock().ok().map(|s| s.messages.len() as i64))
            .unwrap_or(0)
    }
}

/// The supervisor's side of a worker's `wait.for` (schedules.py `_wait_status`, `supervisor.corrections`).
pub struct Waits(pub Arc<SupDaemon>);

impl super::tools::WaitHooks for Waits {
    fn status(
        &self,
        worker: &str,
        task: &str,
        until: Option<&ferb_clock::py::Time>,
        line: Option<&str>,
    ) {
        if let Some(sup) = self.0.sup.get() {
            sup.wait_status(worker, task, until, line);
        }
    }
    fn steered(&self, task: &str) -> i64 {
        self.0.corrections(task)
    }
    fn sleeping(&self, worker: &str, task: &str, until: Option<&ferb_clock::py::Time>, why: &str) {
        if let Some(sup) = self.0.sup.get() {
            match until {
                // `activity.waiting(worker, True)` (wait_status with no time: its task_sleeping(None) finds no sleep
                // yet and does nothing), then `task_sleeping(task, at, why)`: one status line
                Some(at) => {
                    sup.wait_status(worker, task, None, Some(""));
                    sup.task_sleeping(task, Some(at), why, "");
                }
                // awake: `activity.waiting(worker, False)`, `task_sleeping(task, None, "")`
                None => sup.wait_status(worker, task, None, None),
            }
        }
    }
}

/// The chat RPCs' view of the gateway and the supervisor's own questions.
struct ChatGw(Arc<SupDaemon>);

impl ChatGw {
    /// `state.approvals.resolve(...)` on whichever store holds the question, then its hidden kinds.
    fn resolve_on(
        &self,
        id: &str,
        granted: bool,
        choice: Option<usize>,
        text: Option<&str>,
        hidden: &[String],
    ) -> Result<Approval, ResolveError> {
        let ap = self.0.gateway.resolve(id, granted, choice, text)?;
        if !hidden.is_empty() {
            self.0.gateway.set_hidden(id, hidden);
        }
        Ok(ap)
    }
}

impl ferb_supervisor::flow::chat_rpc::ChatGateway for ChatGw {
    fn pending(&self) -> Vec<Approval> {
        self.0.gateway.pending()
    }
    fn approval(&self, id: &str) -> Option<Approval> {
        self.0.approval(id)
    }
    fn resolve(
        &self,
        id: &str,
        granted: bool,
        choice: Option<usize>,
        text: Option<&str>,
        hidden: &[String],
    ) -> Result<Approval, ResolveError> {
        self.resolve_on(id, granted, choice, text, hidden)
    }
    fn tools_for(&self, grant: &RunGrant) -> Vec<Value> {
        super::rpc::tools_for(&self.0.gateway, &self.0.tools, grant)
    }
    fn call<'a>(
        &'a self,
        grant: &'a RunGrant,
        name: &'a str,
        arguments: &'a Map<String, Value>,
    ) -> BoxFuture<'a, Result<Value, String>> {
        Box::pin(async move {
            self.0
                .tools
                .call(
                    self.0.gateway.clone(),
                    Arc::new(grant.clone()),
                    name.to_string(),
                    arguments.clone(),
                    Arc::new(|| false),
                )
                .await
                .map(|r| r.to_json())
                .map_err(|e| match e {
                    ferb_gateway::gateway::CallError::Cancelled => "CancelledError".to_string(),
                    ferb_gateway::gateway::CallError::Internal(_) => "RuntimeError".to_string(),
                })
        })
    }
    fn stopped(&self) -> bool {
        self.0.stop.engaged()
    }
}

/// Group B's runtimes seam: a task stopped on a usage limit is offered another runtime (`runtime_offer`), and the
/// owner's pick resumes it there (`answer_runtime_switch`).
pub struct Switch(pub Supervisor);

impl ferb_daemon::areas_b::runtimes::Supervisor for Switch {
    fn runtime_offer(&self, task_id: &str) -> Option<Value> {
        self.0.inner().runtime_offer(task_id)
    }

    fn answer_runtime_switch(&self, task_id: &str, runtime: &str) -> Result<Value, HandlerError> {
        self.0
            .inner()
            .answer_runtime_switch(task_id, runtime)
            .map_err(|m| HandlerError::refusal("FerbError", m))
    }
}

/// The gateway's question router (`route_question`, compose.py's `questions`), bound once the supervisor exists: a
/// worker's question goes to the orchestrator first (ORCH-616).
#[derive(Default)]
pub struct Router(pub OnceLock<Supervisor>);

impl ferb_gateway::seams::QuestionRouter for Router {
    fn route<'a>(
        &'a self,
        task_id: &'a str,
        question: &'a str,
        options: &'a [String],
    ) -> BoxFuture<'a, Result<Map<String, Value>, String>> {
        match self.0.get() {
            Some(sup) => {
                ferb_gateway::seams::QuestionRouter::route(sup, task_id, question, options)
            }
            None => Box::pin(async { Err("FERB's planner isn't started yet".to_string()) }),
        }
    }
}

/// The supervisor as built for the daemon: the planner, its seam and its chat RPCs.
pub struct Built {
    pub supervisor: Supervisor,
    pub daemon: Arc<SupDaemon>,
    pub rpc: Arc<ferb_supervisor::flow::chat_rpc::ChatRpc>,
}

/// What [`build`] takes besides the seam.
pub struct BuildParts {
    pub db: SharedDb,
    pub clock: Arc<dyn ferb_clock::clock::ZonedClock>,
    pub ids: Arc<dyn ferb_core::interfaces::IdSource>,
    pub redactor: Arc<ferb_sanitize::redact::Redactor>,
    /// FERB's package folder, resolved (`PromptRoot`).
    pub prompts: ferb_context::prompts::PromptRoot,
    pub device_id: String,
    pub home: String,
    pub user_home: String,
    /// The untrusted-data wrapper's id (None: random, as `wrap_untrusted`).
    pub wrap_id: Option<String>,
    /// `"chat-" + token_hex(6)`'s hex (None: random).
    pub new_chat: Option<Arc<dyn Fn() -> String + Send + Sync>>,
    /// A loop notice's id, `uuid4().hex[:12]` (None: random).
    pub notice_hex: Option<Arc<dyn Fn() -> String + Send + Sync>>,
    pub shutting_down: Arc<dyn Fn() -> bool + Send + Sync>,
    /// The token count (o200k; a test build: the recorder's byte ceiling).
    pub tokenizer: ferb_context::tokens::Tokenizer,
}

/// `Supervisor(...)` (compose.py:130) with `gateway.attach(_Watch(self))` and the chat RPCs.
pub fn build(daemon: Arc<SupDaemon>, p: BuildParts) -> Built {
    let wrap: Arc<dyn Fn(&str) -> String + Send + Sync> = match p.wrap_id {
        Some(id) => {
            Arc::new(move |t: &str| ferb_sanitize::untrusted::wrap_untrusted_with_id(t, &id))
        }
        None => Arc::new(|t: &str| {
            ferb_sanitize::untrusted::wrap_untrusted(t).unwrap_or_else(|_| {
                ferb_sanitize::untrusted::wrap_untrusted_with_id(t, "000000000000")
            })
        }),
    };
    let new_chat = p
        .new_chat
        .unwrap_or_else(|| Arc::new(|| ferb_runtimes::codex::token_hex(6)));
    let supervisor = Supervisor::new(ferb_supervisor::flow::Deps {
        db: p.db,
        clock: p.clock,
        ids: p.ids,
        redactor: p.redactor,
        policy: Arc::new(super::gateway::PolicyWords),
        daemon: daemon.clone(),
        prompts: p.prompts,
        tokenizer: p.tokenizer,
        device_id: p.device_id,
        workspace: format!("{}/workspace", p.home),
        home: p.user_home,
        wrap,
        new_chat,
        stall_s: 180.0,
        heartbeat_poll_s: 5.0,
    });
    if let Some(f) = p.notice_hex {
        let _ = daemon.notice_hex.set(f);
    }
    daemon.gateway.attach(supervisor.watch());
    let _ = daemon.sup.set(supervisor.clone());
    daemon.tools.set_supervisor(supervisor.clone());
    daemon.tools.set_wait_hooks(Arc::new(Waits(daemon.clone())));
    let rpc =
        ferb_supervisor::flow::chat_rpc::ChatRpc::new(ferb_supervisor::flow::chat_rpc::ChatDeps {
            supervisor: supervisor.clone(),
            gateway: daemon.chat_gateway(),
            persona: None,
            prepare_persona: Arc::new(|| {}),
            // `attention_notices.unseen()`: the notices no FERB client has shown yet (a loop's pause, an always-on fact)
            notices: Some({
                let d = daemon.clone();
                Arc::new(move || {
                    let db = lock(&d.db);
                    let r = d.redactor.clone();
                    let notices = ferb_orchestrator::attention::AttentionNotices {
                        db: &db,
                        now: Box::new(String::new),
                        redact: Arc::new(move |v: &Value| r.value(v)),
                    };
                    let unseen = notices.unseen().unwrap_or_default();
                    drop(notices);
                    unseen
                })
            }),
            conversations: {
                let d = daemon.clone();
                Arc::new(move || {
                    let db = lock(&d.db);
                    let r = d.redactor.clone();
                    ferb_orchestrator::conversations::ConversationStore::new(
                        &db,
                        Arc::new(String::new),
                        Arc::new(move |v: &Value| r.value(v)),
                        Arc::new(|_: &str, _: Value| {}),
                    )
                    .list(None)
                    .unwrap_or_default()
                    .into_iter()
                    .map(Value::Object)
                    .collect()
                })
            },
            sensing_paused: Arc::new(|| false),
            shutting_down: p.shutting_down,
            question_answered: Arc::new(|_ap| String::new()),
            ask_input_monitoring: Arc::new(|| {}),
            click_through: None,
        });
    Built {
        supervisor,
        daemon,
        rpc,
    }
}

/// One registered tool's facts as a loop's probe checks read them (probes.py `refusal`, `cap.Input.model_validate`).
pub struct ProbeCap {
    tool: Arc<dyn ferb_gateway::tool::Tool>,
    verb: Option<&'static ferb_browser::tools::Verb>,
}

impl ProbeCap {
    fn of(name: &str, tool: Arc<dyn ferb_gateway::tool::Tool>) -> ProbeCap {
        let verb = name
            .strip_prefix("browser.")
            .or_else(|| name.strip_prefix("ferb_browser."))
            .and_then(ferb_browser::tools::verb);
        ProbeCap { tool, verb }
    }
}

impl ferb_orchestrator::ports::CapFacts for ProbeCap {
    fn scopes(&self) -> Vec<String> {
        self.tool.scopes().into_iter().collect()
    }
    fn identities(&self) -> Vec<String> {
        self.tool.identities().into_iter().collect()
    }
    fn observation(&self) -> bool {
        self.tool.facts().observation
    }
    fn probe_safe(&self) -> bool {
        self.tool.facts().probe_safe
    }
    fn moves_view(&self) -> bool {
        self.verb.is_some_and(|v| v.moves_view)
    }
    fn on_screen(&self) -> bool {
        self.tool.facts().on_screen
    }
    fn holds_tab(&self) -> bool {
        self.verb.is_some_and(|v| v.holds_tab)
    }
    fn probe_keys(&self) -> Vec<String> {
        self.verb
            .map(|v| v.probe_keys.iter().map(|k| k.to_string()).collect())
            .unwrap_or_default()
    }
    /// `cap.Input.model_validate(args)`: the fields the arguments set (`exclude_unset`), or `str(e)`.
    fn validate(&self, args: &Value) -> Result<Map<String, Value>, String> {
        let mut schema = self.tool.input_schema();
        if !schema.is_object() {
            schema = json!({});
        }
        let hooks = self.tool.hooks();
        let mut model = ferb_gateway::pydantic::Model::of(&schema);
        model.hooks = hooks.as_deref();
        match model.validate(args) {
            Ok(mut fields) => {
                let given = args.as_object();
                fields.retain(|k, _| given.is_some_and(|g| g.contains_key(k)));
                Ok(fields)
            }
            Err(details) => Err(validation_text(model.class_name, &details)),
        }
    }
}

/// pydantic's `str(ValidationError)` of these details.
fn validation_text(title: &str, details: &[ferb_gateway::pydantic::Detail]) -> String {
    let n = details.len();
    let mut out = format!(
        "{n} validation error{} for {title}",
        if n == 1 { "" } else { "s" }
    );
    for d in details {
        if !d.loc.is_empty() {
            let loc: Vec<String> = d
                .loc
                .iter()
                .map(|l| match l {
                    ferb_gateway::pydantic::Loc::Key(k) => k.clone(),
                    ferb_gateway::pydantic::Loc::Index(i) => i.to_string(),
                })
                .collect();
            out.push_str(&format!("\n{}", loc.join(".")));
        }
        let repr = ferb_daemon::shared::py_repr(&d.input);
        let repr = if repr.len() <= 50 {
            repr
        } else {
            let (mut head, mut tail) = (25, repr.len() - 24);
            while !repr.is_char_boundary(head) {
                head -= 1;
            }
            while !repr.is_char_boundary(tail) {
                tail += 1;
            }
            format!("{}...{}", &repr[..head], &repr[tail..])
        };
        let kind = match &d.input {
            Value::Null => "NoneType",
            Value::Bool(_) => "bool",
            Value::Number(n) if n.is_f64() => "float",
            Value::Number(_) => "int",
            Value::String(_) => "str",
            Value::Array(_) => "list",
            Value::Object(_) => "dict",
        };
        out.push_str(&format!(
            "\n  {} [type={}, input_value={repr}, input_type={kind}]\n    For further information visit \
             https://errors.pydantic.dev/2.13/v/{}",
            d.msg, d.kind, d.kind
        ));
    }
    out
}
