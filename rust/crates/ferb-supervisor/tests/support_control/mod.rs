//! The harness the control and recovery scenarios replay on (copied and adapted from tests/replay.rs, which is the
//! forward path's): a real FERB store in a temporary folder, the supervisor with a scripted runtime and a recording
//! daemon seam, walking a recorded scenario of rust/fixtures/daemon/chat_scenarios.json step by step.
//!
//! What this adds to the forward path's harness:
//! - the scripted runtime holds a call the recorder held (`Gate`: answered only at `release_model`), never answers a
//!   STALL call until FERB cancels it, and records what FERB delivered into a running call (`continued_with`);
//! - the daemon's own handlers the scenarios call, as areas/chat.py and areas/rpc.py run them: `task.stop`
//!   (`Supervisor::task_stop`), `stop.engage` / `stop.lift` (the gateway's stop order: block, revoke every run token,
//!   the hooks "runtimes" = `cancel_all` and "identity_browser"), `chat.approve` / `chat.answer`
//!   (`_resolve_approval` on the gateway's real question store, ferb-gateway `questions`);
//! - the bus: what the supervisor publishes reaches the chat stream's subscription (`Daemon::subscribe`);
//! - a restart: the daemon's shutdown (`Supervisor::shut_down`) and a new supervisor on the same store, ids under
//!   `r1`.
//!
//! `chat.status`, `results.ack`, `tasks.list`, `schedules.list` and the notifications stream are the daemon's
//! composition of many parts: their lines are not compared here (each is listed as skipped in the report).

#![allow(dead_code)]

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use crate::common::{read, repo, root, s, scenarios, strs, WRAP_ID};
use ferb_clock::clock::FakeClock;
use ferb_clock::zone::Zone;
use ferb_context::assemble::ContextBlock;
use ferb_context::tokens::Tokenizer;
use ferb_core::errors::FerbError;
use ferb_core::types::{
    ModelLimits, ModelRequest, RunHandle, RuntimeEvent, RuntimeEventKind, Task,
};
use ferb_gateway::questions::{Approvals, Asker};
use ferb_orchestrator::models::Condition;
use ferb_orchestrator::ports::{BoxFuture, Policy};
use ferb_orchestrator::steering::ContinueError;
use ferb_orchestrator::verify::ConditionResult;
use ferb_store::Db;
use ferb_supervisor::flow::owner_wait::{Ask, Asked, Questions};
use ferb_supervisor::flow::{
    BusSink, Change, Daemon, Deps, Environment, FrontSeen, Request, Supervisor, Unsubscribe,
};
use ferb_supervisor::planner::OpenQuestion;
use ferb_supervisor::prompts::SystemInputs;
use ferb_supervisor::worker_context::Activity;
use ferb_worker::ports::{EventSource, Grant, ModelRuntime, RunControl, StartedRun};
use serde_json::{json, Map, Value};
use tokio::sync::Notify;

// ---------------------------------------------------------------------------------------------------------------
// Reading the fixture (as tests/replay.rs)
// ---------------------------------------------------------------------------------------------------------------

/// Every recorded text of 2,048 characters or more: the daemon's, the supervisor's and the control scenarios'.
fn daemon_bodies() -> &'static HashMap<String, String> {
    static B: std::sync::OnceLock<HashMap<String, String>> = std::sync::OnceLock::new();
    B.get_or_init(|| {
        let mut out = HashMap::new();
        for rel in [
            "rust/fixtures/daemon/bodies.json",
            "rust/fixtures/supervisor/bodies.json",
            "rust/fixtures/supervisor/control_bodies.json",
        ] {
            for (k, v) in read(rel).as_object().unwrap() {
                out.insert(k.clone(), v.as_str().unwrap().to_string());
            }
        }
        out
    })
}

/// A value with every `{"sha256": …}` stand-in replaced by its text (any of the three bodies files).
pub fn resolve(v: &Value) -> Value {
    match v {
        Value::Object(m) if m.len() == 1 && m.contains_key("sha256") => {
            json!(daemon_bodies()[m["sha256"].as_str().unwrap()])
        }
        Value::Object(m) => Value::Object(m.iter().map(|(k, v)| (k.clone(), resolve(v))).collect()),
        Value::Array(a) => Value::Array(a.iter().map(resolve).collect()),
        other => other.clone(),
    }
}

/// rust/fixtures/supervisor/control_scenarios.json (made by make_control.py).
pub fn control() -> &'static Value {
    static C: std::sync::OnceLock<Value> = std::sync::OnceLock::new();
    C.get_or_init(|| read("rust/fixtures/supervisor/control_scenarios.json"))
}

/// A catalog stored once by make.py or make_control.py (`{"catalog": sha}`).
fn catalog(v: &Value) -> Vec<Value> {
    let key = s(&v["catalog"]);
    if key.is_empty() {
        return Vec::new();
    }
    let in_control = &control()["catalogs"][&key];
    if !in_control.is_null() {
        return resolve(in_control).as_array().expect("a catalog").clone();
    }
    crate::common::catalog(v)
}

fn expand(text: &str) -> String {
    let mut out = text.to_string();
    while let Some(at) = out.find("\"@@body:") {
        let rest = &out[at + 8..];
        let end = rest.find("@@\"").expect("a body token's end");
        let token = &rest[..end];
        let (sha, form) = token.split_once(':').expect("a body token");
        let body = &daemon_bodies()[sha];
        let dumped = if form == "ascii" {
            ferb_py::json::dumps(&json!(body))
        } else {
            ferb_py::json::dumps_utf8(&json!(body))
        };
        out = format!("{}{}{}", &out[..at], dumped, &rest[end + 3..]);
    }
    out
}

pub fn cell(v: &Value) -> Value {
    match v {
        Value::Object(m) if m.len() == 1 && m.contains_key("sha256") => {
            json!(expand(&daemon_bodies()[m["sha256"].as_str().unwrap()]))
        }
        Value::String(s) => json!(expand(s)),
        other => other.clone(),
    }
}

/// The recorder's volatile masks (make.py `scrub`).
pub fn scrub(v: &Value) -> Value {
    const KEYS: [&str; 4] = ["pid", "max_sample_ms", "executable", "code_hash"];
    match v {
        Value::Object(m) => Value::Object(
            m.iter()
                .map(|(k, x)| {
                    if KEYS.contains(&k.as_str()) {
                        (k.clone(), json!("<volatile>"))
                    } else {
                        (k.clone(), scrub(x))
                    }
                })
                .collect(),
        ),
        Value::Array(a) => Value::Array(a.iter().map(scrub).collect()),
        Value::String(s) => match serde_json::from_str::<Value>(s) {
            Ok(inner @ (Value::Object(_) | Value::Array(_))) => {
                let masked = scrub(&inner);
                if masked == inner {
                    v.clone()
                } else {
                    json!(ferb_py::json::dumps_utf8(&masked))
                }
            }
            _ => v.clone(),
        },
        other => other.clone(),
    }
}

/// The recorder's temporary root, as long as it was (the budget report counts its bytes), written back as `/FX`.
pub fn recorded_root(chat: &Value) -> String {
    let m = chat["scenarios"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|s| s["model_calls"].as_array().unwrap().iter())
        .find(|m| m["role_played"] == "worker")
        .unwrap();
    let parts: Vec<String> = strs(&resolve(&m["parts"]));
    let body = parts[..parts.len() - 1].join("\n\n");
    let report: Value =
        serde_json::from_str(parts[parts.len() - 1].trim_start_matches("Context budget: "))
            .unwrap();
    let extra = report["content_tokens_estimate"].as_u64().unwrap() as usize - body.len();
    let count = body.matches("/FX").count();
    let len = 3 + extra / count;
    format!("/{}", "r".repeat(len - 1))
}

pub fn chat() -> &'static Value {
    static C: std::sync::OnceLock<Value> = std::sync::OnceLock::new();
    C.get_or_init(|| read("rust/fixtures/daemon/chat_scenarios.json"))
}

// ---------------------------------------------------------------------------------------------------------------
// The seams
// ---------------------------------------------------------------------------------------------------------------

/// `SequentialIds(prefix)`: `id-000001`…; after a restart `r1-000001`….
pub struct Ids {
    n: AtomicUsize,
    prefix: Mutex<String>,
}

impl Ids {
    fn restart(&self, prefix: &str) {
        *self.prefix.lock().unwrap() = prefix.to_string();
        self.n.store(0, Ordering::SeqCst);
    }
}

impl ferb_core::interfaces::IdSource for Ids {
    fn new_id(&self) -> String {
        format!(
            "{}-{:06}",
            self.prefix.lock().unwrap(),
            self.n.fetch_add(1, Ordering::SeqCst) + 1
        )
    }
}

struct TestPolicy;

impl Policy for TestPolicy {
    fn site_key(&self, value: Option<&str>) -> String {
        let text = ferb_orchestrator::py::strip(value.unwrap_or("")).to_string();
        let parsed = if text.is_empty() {
            None
        } else {
            ferb_policy::urls::parse(&format!("https://{text}"))
        };
        let host = match parsed.filter(|p| !p.host.is_empty()) {
            Some(p) => p.host,
            None => ferb_orchestrator::py::lower(&text),
        };
        let host = host.trim_end_matches('.').to_string();
        host.strip_prefix("www.").map(String::from).unwrap_or(host)
    }
    fn account_key(&self, value: Option<&str>) -> String {
        ferb_policy::confirm::account_key(value)
    }
    fn url_host(&self, url: &str) -> Option<String> {
        ferb_policy::urls::parse(url).map(|u| u.host)
    }
    fn from_task(&self, text: &str) -> String {
        ferb_policy::confirm::from_task(text)
    }
    fn reads_as_ferbs_check(&self, text: &str) -> bool {
        ferb_policy::confirm::reads_as_ferbs_check(text)
    }
}

#[derive(Clone)]
pub struct Tool {
    pub name: String,
    pub row: Value,
    pub scopes: Vec<String>,
    pub identities: Vec<String>,
    pub observation: bool,
}

/// The gateway's question store (ferb-gateway `questions::Approvals`) behind the supervisor's seam. Its ids are
/// make.py's counter (`ap-a00000000001`…), kept across a restart (the module's counter) while the store itself is new
/// (a restart loses every pending question).
pub struct TestQuestions {
    pub store: Mutex<Approvals>,
}

impl TestQuestions {
    fn new(counter: Arc<AtomicUsize>) -> TestQuestions {
        TestQuestions {
            store: Mutex::new(Approvals::new(move || {
                format!("ap-a{:011}", counter.fetch_add(1, Ordering::SeqCst) + 1)
            })),
        }
    }
}

fn asked(ap: &ferb_gateway::questions::Approval) -> Asked {
    Asked {
        id: ap.id.clone(),
        capability: ap.capability.clone(),
        kind: ap.kind.clone(),
        request_id: ap.request_id.clone(),
        task_id: ap.task_id.clone(),
        state: ap.state.as_str().into(),
        options: ap.options.clone(),
        choice: ap.choice.clone(),
        answer: ap.answer.clone(),
        hidden: ap.hidden.clone(),
        options_only: ap.options_only,
        corrects: ap.corrects,
    }
}

impl Questions for TestQuestions {
    fn ask(&self, ask: &Ask) -> String {
        let mut store = self.store.lock().unwrap();
        let ap = store.question(
            &Asker {
                capability: &ask.capability,
                task_id: &ask.task_id,
                request_id: &ask.request_id,
            },
            &ask.question,
            &ask.options,
            None,
        );
        ap.part = ask.part.clone();
        ap.runtime_switch = ask
            .runtime_switch
            .as_ref()
            .and_then(|v| v.as_object().cloned());
        ap.from_task = ask.from_task;
        ap.options_only = ask.options_only;
        ap.corrects = ask.corrects;
        ap.summary = ask.summary.clone();
        ap.detail = ask.detail.clone();
        ap.exact = ask.exact.clone();
        ap.labels = ask.labels.clone();
        ap.id.clone()
    }
    fn get(&self, id: &str) -> Option<Asked> {
        self.store.lock().unwrap().get(id).map(asked)
    }
    fn supersede(&self, id: &str) -> bool {
        self.store.lock().unwrap().supersede(id).is_some()
    }
    fn pending(&self) -> Vec<Asked> {
        self.store
            .lock()
            .unwrap()
            .pending()
            .into_iter()
            .map(asked)
            .collect()
    }
    fn answer(&self, id: &str, words: &str) {
        let _ = self
            .store
            .lock()
            .unwrap()
            .resolve(id, true, None, Some(words));
    }
    fn card(&self, id: &str) -> Map<String, Value> {
        let store = self.store.lock().unwrap();
        let Some(ap) = store.get(id) else {
            return Map::new();
        };
        let mut m = ferb_gateway::questions::card_parts(ap);
        m.extend(ferb_gateway::questions::card_flags(ap));
        m
    }
}

/// What a scenario's model calls were made with, what the runtime was handed and what FERB delivered into them.
#[derive(Default)]
pub struct Calls {
    pub made: Vec<Value>,
    pub started: Vec<Value>,
    pub continued: BTreeMap<usize, Vec<String>>,
}

type Sinks = Vec<(usize, Vec<String>, BusSink)>;

pub struct TestDaemon {
    me: std::sync::Weak<TestDaemon>,
    db: Arc<Mutex<Db>>,
    ids: Arc<Ids>,
    root: String,
    tools: Vec<Tool>,
    schemas: Map<String, Value>,
    pub recorded: Vec<Value>,
    pub calls: Mutex<Calls>,
    supervise: Mutex<Vec<Value>>,
    systems: Mutex<Vec<Value>>,
    contexts: Mutex<Vec<Value>>,
    availability: String,
    requests: Mutex<HashMap<String, Request>>,
    grants: Mutex<HashMap<String, Grant>>,
    revoked: Mutex<BTreeSet<String>>,
    only: Mutex<HashMap<String, Vec<String>>>,
    tokens: AtomicUsize,
    pub unsupported: Mutex<Vec<String>>,
    spans: Vec<Vec<Value>>,
    span_columns: Vec<String>,
    /// Calls held until `release_model` (index -> opened).
    gates: Mutex<HashMap<usize, Arc<Gate>>>,
    pub questions: Mutex<Arc<TestQuestions>>,
    pub stopped: AtomicBool,
    pub unattended: Mutex<BTreeSet<String>>,
    sinks: Mutex<Sinks>,
    next_sink: AtomicUsize,
    pub published: Mutex<Vec<(String, Value)>>,
    pub cancelled: Mutex<Vec<BTreeSet<String>>>,
    /// The supervisor whose journal the daemon's own listeners write to.
    journal_to: Mutex<Option<std::sync::Weak<ferb_supervisor::flow::Inner>>>,
    asked_kinds: Mutex<HashMap<String, String>>,
    answered: Mutex<BTreeSet<String>>,
}

pub struct Gate {
    open: AtomicBool,
    notify: Notify,
}

impl Gate {
    async fn wait(&self) {
        loop {
            let n = self.notify.notified();
            tokio::pin!(n);
            n.as_mut().enable();
            if self.open.load(Ordering::SeqCst) {
                return;
            }
            n.await;
        }
    }
}

fn pop(q: &Mutex<Vec<Value>>, _what: &str) -> Value {
    let mut q = q.lock().unwrap();
    if q.is_empty() {
        // read more often than the recording did: the replay notes it where the calls are compared
        return json!({});
    }
    q.remove(0)
}

impl TestDaemon {
    fn note(&self, what: &str) {
        self.unsupported.lock().unwrap().push(what.to_string());
    }

    fn write_span(&self, kind: &str, tree_id: &str) {
        let cols = &self.span_columns;
        let at = |name: &str| cols.iter().position(|c| c == name).unwrap();
        let row = self
            .spans
            .iter()
            .find(|r| r[at("kind")] == kind && r[at("task_id")] == json!(tree_id))
            .cloned();
        let Some(mut row) = row else {
            self.note(&format!("no recorded {kind} span for {tree_id}"));
            return;
        };
        let id = ferb_core::interfaces::IdSource::new_id(self.ids.as_ref());
        row[at("id")] = json!(id);
        let to_sql = |v: &Value| match cell(v) {
            Value::Null => rusqlite::types::Value::Null,
            Value::String(s) => rusqlite::types::Value::Text(s.replace("/FX", &self.root)),
            Value::Number(n) if n.is_f64() => rusqlite::types::Value::Real(n.as_f64().unwrap()),
            Value::Number(n) => rusqlite::types::Value::Integer(n.as_i64().unwrap()),
            other => rusqlite::types::Value::Text(other.to_string()),
        };
        let params: Vec<rusqlite::types::Value> = row.iter().map(to_sql).collect();
        let db = self.db.lock().unwrap();
        db.conn()
            .execute(
                &format!(
                    "INSERT INTO spans ({}) VALUES ({})",
                    cols.join(", "),
                    vec!["?"; cols.len()].join(", ")
                ),
                rusqlite::params_from_iter(params),
            )
            .unwrap();
    }

    /// areas/compose.py:246 `_question_record`: the daemon's listener journals every question asked and answered.
    fn question_record(&self, topic: &str, data: &Value) {
        let Some(sup) = self
            .journal_to
            .lock()
            .unwrap()
            .as_ref()
            .and_then(|w| w.upgrade())
        else {
            return;
        };
        let q = self.questions.lock().unwrap().clone();
        let id = s(&data["approval_id"]);
        let Some(ap) = q.store.lock().unwrap().get(&id).cloned() else {
            return;
        };
        let given = s(&data["question_kind"]);
        let kind = if !given.is_empty() {
            given
        } else if ["owner_login_code", "owner_admin_password", "owner_secret"]
            .contains(&ap.kind.as_str())
        {
            "secret".into()
        } else {
            match ap.capability.as_str() {
                "ferb.confirm" => "confirm",
                "ferb.retry" => "retry",
                "conversation.approve" => "conversation_scope",
                "plan.ask" | "owner.ask" => "task",
                "downloads.delete" => "downloads",
                "browsing.choose" => "browsing",
                "loop.ask" => "loop",
                "runtime.switch" => "runtime_switch",
                "memory.forget" => "memory",
                other => other,
            }
            .to_string()
        };
        let first = |t: &str| -> String {
            t.lines()
                .map(|l| ferb_orchestrator::py::strip(l).to_string())
                .find(|l| !l.is_empty())
                .unwrap_or_default()
                .chars()
                .take(200)
                .collect()
        };
        if topic == "approval.requested" {
            self.asked_kinds
                .lock()
                .unwrap()
                .insert(ap.id.clone(), kind.clone());
            let asked = first(if ap.summary.is_empty() {
                &ap.reason
            } else {
                &ap.summary
            });
            let task = if data["task_id"].as_str().is_some_and(|t| !t.is_empty()) {
                data["task_id"].clone()
            } else if ap.task_id.is_empty() {
                Value::Null
            } else {
                json!(ap.task_id)
            };
            let mut row = json!({"approval_id": ap.id, "kind": kind, "capability": ap.capability,
                                 "task_id": task, "request_id": ap.request_id});
            if !asked.is_empty() {
                row["question"] = json!(asked);
            }
            let exact = first(&ap.exact);
            if !exact.is_empty() {
                row["exact"] = json!(exact);
            }
            sup.journal("question.asked", row, None);
            return;
        }
        if ap.state == ferb_gateway::questions::State::Pending
            || !self.answered.lock().unwrap().insert(ap.id.clone())
        {
            return;
        }
        let secret = ["owner_login_code", "owner_admin_password", "owner_secret"]
            .contains(&ap.kind.as_str());
        let said = if !secret && ap.hidden.is_empty() {
            first(ap.answer.as_deref().unwrap_or(""))
        } else {
            String::new()
        };
        let kind = self
            .asked_kinds
            .lock()
            .unwrap()
            .remove(&ap.id)
            .unwrap_or(kind);
        let answer = if ap.choice.is_some() {
            "option"
        } else if ap.answer.as_deref().is_some_and(|a| !a.is_empty()) {
            "words"
        } else {
            "none"
        };
        let mut row = json!({"approval_id": ap.id, "kind": kind, "state": ap.state.as_str(), "answer": answer});
        if let Some(c) = &ap.choice {
            row["choice"] = json!(c);
        } else if !said.is_empty() {
            row["said"] = json!(said);
        }
        sup.journal("question.answered", row, None);
    }

    /// The gateway's stop order (ferb-gateway `stop::StopControl`): block, revoke every run token, run the hooks.
    pub fn engage(&self, sup: &Supervisor) -> Value {
        self.stopped.store(true, Ordering::SeqCst);
        let mut control = ferb_gateway::stop::StopControl::new(
            {
                let grants: Vec<String> = self.grants.lock().unwrap().keys().cloned().collect();
                let revoked = self.revoked.lock().unwrap().clone();
                let live = grants.iter().filter(|g| !revoked.contains(*g)).count();
                move || live
            },
            |_| {},
        );
        let s = sup.clone();
        control.register(
            "runtimes",
            Box::new(move || {
                s.cancel_all();
                Ok(())
            }),
        );
        control.register("identity_browser", Box::new(|| Ok(())));
        let out = Value::Object(control.engage());
        let all: Vec<String> = self.grants.lock().unwrap().keys().cloned().collect();
        self.revoked.lock().unwrap().extend(all);
        out
    }
}

impl Daemon for TestDaemon {
    fn tool_names(&self) -> Vec<String> {
        self.tools.iter().map(|t| t.name.clone()).collect()
    }
    fn all_scopes(&self) -> BTreeSet<String> {
        self.tools
            .iter()
            .flat_map(|t| t.scopes.iter().cloned())
            .collect()
    }
    fn observes(&self, tool: &str) -> bool {
        self.tools.iter().any(|t| t.name == tool && t.observation)
    }
    fn reads_content(&self, tool: &str) -> bool {
        tool == "fs.read"
    }
    fn tools_for(&self, grant: &Grant) -> Vec<Value> {
        self.tools
            .iter()
            .filter(|t| t.scopes.iter().all(|s| grant.scopes.contains(s)))
            .filter(|t| t.identities.iter().any(|i| i == grant.identity.as_str()))
            .filter(|t| !grant.observe_only || t.observation)
            .filter(|t| {
                self.only
                    .lock()
                    .unwrap()
                    .get(&grant.worker_id)
                    .is_none_or(|only| only.contains(&t.name))
            })
            .map(|t| t.row.clone())
            .collect()
    }
    fn issue_token(
        &self,
        task_id: &str,
        worker_id: &str,
        scopes: &BTreeSet<String>,
        identity: &str,
        _ttl_s: f64,
        observe_only: bool,
        tools: Option<&[String]>,
        _max_calls: Option<usize>,
    ) -> String {
        if let Some(t) = tools {
            self.only
                .lock()
                .unwrap()
                .insert(worker_id.to_string(), t.to_vec());
        }
        let token = format!(
            "run-token-{:04}",
            self.tokens.fetch_add(1, Ordering::SeqCst) + 1
        );
        self.grants.lock().unwrap().insert(
            token.clone(),
            Grant {
                task_id: task_id.into(),
                worker_id: worker_id.into(),
                scopes: scopes.clone(),
                identity: if identity == "ferb" {
                    ferb_core::types::Identity::Ferb
                } else {
                    ferb_core::types::Identity::Owner
                },
                observe_only,
            },
        );
        token
    }
    fn validate_token(&self, token: &str) -> Option<Grant> {
        self.grants.lock().unwrap().get(token).cloned()
    }
    fn revoke_worker(&self, worker_id: &str) {
        let tokens: Vec<String> = self
            .grants
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, g)| g.worker_id == worker_id)
            .map(|(t, _)| t.clone())
            .collect();
        self.revoked.lock().unwrap().extend(tokens);
    }
    fn forget_calls(&self, _worker_id: &str) {}
    fn calls_refused(&self, _watch_id: &str) -> u64 {
        0
    }
    fn lookups_made(&self, _holder: &str) -> usize {
        0
    }
    fn initial_image_given(&self, _task_id: &str) {}
    fn unresolved(&self, _root: &str) -> Vec<Map<String, Value>> {
        Vec::new()
    }
    fn forget_tasks(&self, _ids: &BTreeSet<String>) {}
    fn task_folders(&self, _tree_id: &str, _folders: Option<&[String]>) {}
    fn record_named(&self, _words: &str, _request_id: &str) {}
    fn standing_revoke(&self, _tree_id: &str) {}
    fn standing_scopes(&self, _tree_id: &str) -> Vec<Value> {
        Vec::new()
    }
    fn record_request(&self, request: &Request) {
        self.requests
            .lock()
            .unwrap()
            .insert(request.request_id.clone(), request.clone());
    }
    fn request(&self, request_id: &str) -> Request {
        self.requests
            .lock()
            .unwrap()
            .get(request_id)
            .cloned()
            .unwrap_or(Request {
                request_id: request_id.into(),
                ..Request::default()
            })
    }
    fn forget_request(&self, request_id: &str) {
        self.requests.lock().unwrap().remove(request_id);
        self.questions
            .lock()
            .unwrap()
            .store
            .lock()
            .unwrap()
            .forget_request(request_id);
    }
    fn unattended(&self, request_id: &str) -> bool {
        self.unattended.lock().unwrap().contains(request_id)
    }
    fn pending_questions(&self) -> Vec<OpenQuestion> {
        Vec::new()
    }
    fn environment<'a>(&'a self, _words: &'a str) -> BoxFuture<'a, Environment> {
        Box::pin(async move {
            let q = self.supervise.lock().unwrap();
            let next = q
                .first()
                .cloned()
                .expect("a planner call to read the environment for");
            Environment {
                stable: s(&next["environment"]["stable"]),
                live: s(&next["environment"]["live"]),
            }
        })
    }
    fn live_facts(&self, _last_runs: bool, _owner_state: bool) -> String {
        let rec = pop(&self.supervise, "the planner's live facts");
        s(&rec["live_facts"])
    }
    fn system_inputs(&self, _live: bool) -> SystemInputs {
        let rec = pop(&self.systems, "a role's instructions");
        let i = &rec["inputs"];
        SystemInputs {
            facts: s(&i["facts"]),
            live_facts: s(&i["live_facts"]),
            capabilities: s(&i["capabilities"]),
            identity_context: s(&i["identity_context"]),
            skills_line: ferb_supervisor::prompts::catalog_line(&catalog(&i["skills_catalog"])),
            abilities_line: ferb_supervisor::prompts::catalog_line(&catalog(
                &i["abilities_catalog"],
            )),
        }
    }
    fn availability(&self) -> String {
        self.availability.clone()
    }
    fn worker_types(&self) -> Vec<String> {
        vec!["default".into(), "codex".into(), "claude".into()]
    }
    fn signals(&self) -> (Option<ferb_clock::py::Time>, Option<ferb_clock::py::Time>) {
        (None, None)
    }
    fn relevant_context(&self, _words: &str) -> Activity {
        let q = self.contexts.lock().unwrap();
        match q.first() {
            Some(rec) => Activity::from_json(&rec["activity"]),
            None => {
                drop(q);
                self.note("a context read the recording didn't make");
                Activity::from_json(&json!({}))
            }
        }
    }
    fn apps(&self) -> Vec<(String, String)> {
        Vec::new()
    }
    fn running_apps(&self) -> Vec<(String, String, i64)> {
        vec![
            ("com.apple.finder".into(), "Finder".into(), 4101),
            ("com.apple.TextEdit".into(), "TextEdit".into(), 4102),
        ]
    }
    fn shell_process(&self, _id: &str) -> Option<Map<String, Value>> {
        None
    }
    fn runtime_account(&self, _runtime: &str, _account: &str) {}
    fn on_violation(&self, runtime: &str, text: &str) {
        self.note(&format!("runtime violation {runtime}: {text}"));
    }
    fn has_route(&self) -> Option<bool> {
        None
    }
    fn identity_pending(&self) -> String {
        String::new()
    }
    fn settle(&self) -> BoxFuture<'_, ()> {
        Box::pin(async {})
    }
    fn memory_push<'a>(&'a self, _q: &'a str, _t: &'a str) -> BoxFuture<'a, Vec<ContextBlock>> {
        Box::pin(async { Vec::new() })
    }
    fn on_episode<'a>(&'a self, _tree_id: &'a str) -> BoxFuture<'a, ()> {
        Box::pin(async {})
    }
    fn last_run_origins(&self) -> BTreeSet<String> {
        BTreeSet::new()
    }
    fn on_task_start<'a>(&'a self, tree_id: &'a str) -> BoxFuture<'a, ()> {
        Box::pin(async move { self.write_span("disturbance.baseline", tree_id) })
    }
    fn on_task_finish<'a>(&'a self, tree_id: &'a str) -> BoxFuture<'a, ()> {
        Box::pin(async move { self.write_span("disturbance.summary", tree_id) })
    }
    fn disturbance_context(&self, _tree_id: &str) -> String {
        let rec = pop(&self.contexts, "a context");
        s(&rec["disturbance"])
    }
    fn on_task_end<'a>(&'a self, _t: &'a str, _s: &'a str) -> BoxFuture<'a, Vec<Value>> {
        Box::pin(async { Vec::new() })
    }
    fn changes(&self, _task_id: &str, _tree_id: &str) -> Option<Result<Vec<Change>, String>> {
        Some(Ok(Vec::new()))
    }
    fn untrusted_roots(&self) -> Vec<String> {
        Vec::new()
    }
    fn browser_context(&self, _task_id: &str) -> String {
        String::new()
    }
    fn task_tabs(&self, _task_id: &str) -> Vec<Value> {
        Vec::new()
    }
    fn keep_tabs_open(&self, _task_id: &str) {}
    fn code_checkable(&self, condition: &Condition, _task_id: &str) -> bool {
        if condition.check.kind != "none" {
            self.note("a literal check");
        }
        false
    }
    fn code_check<'a>(
        &'a self,
        _i: i64,
        _c: &'a Condition,
        _t: &'a str,
    ) -> BoxFuture<'a, ConditionResult> {
        Box::pin(async { unreachable!("no literal check in these scenarios") })
    }
    fn declined(&self, _i: i64, _c: &Condition, _t: &str) -> Option<ConditionResult> {
        None
    }
    fn sites_contacted(&self, _tree_id: &str) -> Vec<String> {
        Vec::new()
    }
    fn schedule_label(&self, schedule_id: &str) -> String {
        schedule_id.to_string()
    }
    fn left_running(&self, _resources: &[Value]) -> Vec<String> {
        Vec::new()
    }
    fn ran_cards(&self, _tree_id: &str) -> Vec<Value> {
        Vec::new()
    }
    fn write_notes(&self, _path: &str, _text: &str) -> Result<(), String> {
        Ok(())
    }
    fn front_window<'a>(&'a self, _request_id: &'a str) -> BoxFuture<'a, Option<FrontSeen>> {
        Box::pin(async { None })
    }
    fn make_runtime(
        &self,
        role: &str,
        mode: &str,
        tool_names: &[String],
        schema: &Value,
        effort: Option<&str>,
    ) -> Result<Arc<dyn ModelRuntime>, ferb_worker::Error> {
        let name = self
            .schemas
            .iter()
            .find(|(_, v)| *v == schema)
            .map(|(k, _)| k.clone())
            .unwrap_or_else(|| "(unknown schema)".into());
        let mut calls = self.calls.lock().unwrap();
        let n = calls.made.len();
        calls.made.push(
            json!({"runtime_role": role, "mode": mode, "tools": tool_names, "schema": name,
                               "effort": effort}),
        );
        let events = self
            .recorded
            .get(n)
            .map(|m| m["events"].clone())
            .unwrap_or_else(|| json!([{"kind": "(a call the recording didn't make)"}]));
        let gate = self.gates.lock().unwrap().get(&n).cloned();
        Ok(Arc::new(ScriptRuntime {
            daemon: self.me.upgrade().expect("the scenario's daemon"),
            n,
            events,
            gate,
        }))
    }
    fn publish(&self, topic: &str, data: Value) {
        self.published
            .lock()
            .unwrap()
            .push((topic.to_string(), data.clone()));
        if topic == "approval.requested" || topic == "approval.resolved" {
            self.question_record(topic, &data);
        }
        let sinks: Vec<BusSink> = self
            .sinks
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, topics, _)| {
                topics.iter().any(|t| match t.strip_suffix('*') {
                    Some(prefix) => topic.starts_with(prefix),
                    None => t == topic,
                })
            })
            .map(|(_, _, sink)| sink.clone())
            .collect();
        let py = ferb_sanitize::Py::from(&data);
        for sink in sinks {
            sink(topic, &py);
        }
    }
    fn subscribe(&self, topics: &[&str], sink: BusSink) -> Option<Unsubscribe> {
        let id = self.next_sink.fetch_add(1, Ordering::SeqCst);
        self.sinks
            .lock()
            .unwrap()
            .push((id, topics.iter().map(|t| t.to_string()).collect(), sink));
        let me = self.me.clone();
        Some(Box::new(move || {
            if let Some(me) = me.upgrade() {
                me.sinks.lock().unwrap().retain(|(i, _, _)| *i != id);
            }
        }))
    }
    fn cancel_tasks(&self, ids: &BTreeSet<String>) {
        self.cancelled.lock().unwrap().push(ids.clone());
        // the gateway's cancel of a pending question's call: the question goes (`forget_request` on the task's end)
    }
    fn hold_poll_s(&self) -> f64 {
        0.2
    }
    fn stopped(&self) -> bool {
        self.stopped.load(Ordering::SeqCst)
    }
    fn questions(&self) -> Option<Arc<dyn Questions>> {
        let q: Arc<dyn Questions> = self.questions.lock().unwrap().clone();
        Some(q)
    }
    fn set_unattended(&self, request_id: &str) {
        self.unattended
            .lock()
            .unwrap()
            .insert(request_id.to_string());
    }
}

struct ScriptRuntime {
    daemon: Arc<TestDaemon>,
    n: usize,
    events: Value,
    gate: Option<Arc<Gate>>,
}

impl ModelRuntime for ScriptRuntime {
    fn name(&self) -> String {
        "fake".into()
    }
    fn limits(&self) -> Result<ModelLimits, FerbError> {
        Ok(ModelLimits {
            model: "fake-model".into(),
            context_window: 1_000_000,
            max_output: 100_000,
            source: "test double".into(),
            window_includes_output: true,
            max_input_chars: None,
        })
    }
    fn output_schema(&self) -> Option<Value> {
        None
    }
    fn start<'a>(
        &'a self,
        task: &'a Task,
        request: &'a ModelRequest,
        _token: &'a str,
    ) -> BoxFuture<'a, Result<StartedRun, ferb_worker::Error>> {
        Box::pin(async move {
            let root = &self.daemon.root;
            let parts: Vec<String> = request
                .parts
                .iter()
                .map(|p| p.replace(root.as_str(), "/FX"))
                .collect();
            self.daemon
                .calls
                .lock()
                .unwrap()
                .started
                .push(json!({"n": self.n, "task_id": task.id,
                "system": request.system, "parts": parts, "cacheable": request.cacheable,
                "images": request.images.len()}));
            let cancelled = Arc::new((AtomicBool::new(false), Notify::new()));
            Ok(StartedRun {
                handle: RunHandle {
                    id: format!("run-{}", task.id),
                    runtime: "fake".into(),
                    task_id: task.id.clone(),
                    state: Map::new(),
                },
                events: Box::new(ScriptEvents {
                    events: self.events.as_array().cloned().unwrap_or_default(),
                    at: 0,
                    cancelled: cancelled.clone(),
                    gate: self.gate.clone(),
                }),
                control: Arc::new(ScriptControl {
                    daemon: self.daemon.clone(),
                    n: self.n,
                    cancelled,
                }),
            })
        })
    }
}

struct ScriptControl {
    daemon: Arc<TestDaemon>,
    n: usize,
    cancelled: Arc<(AtomicBool, Notify)>,
}

impl RunControl for ScriptControl {
    fn steer<'a>(&'a self, message: &'a str) -> BoxFuture<'a, Result<(), ContinueError>> {
        Box::pin(async move {
            self.daemon
                .calls
                .lock()
                .unwrap()
                .continued
                .entry(self.n)
                .or_default()
                .push(message.to_string());
            Ok(())
        })
    }
    fn cancel(&self) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            self.cancelled.0.store(true, Ordering::SeqCst);
            self.cancelled.1.notify_waiters();
        })
    }
}

struct ScriptEvents {
    events: Vec<Value>,
    at: usize,
    cancelled: Arc<(AtomicBool, Notify)>,
    gate: Option<Arc<Gate>>,
}

impl ScriptEvents {
    /// Waits until the run is cancelled (a STALL call, or a held call cancelled before its gate opened).
    async fn until_cancelled(&self) {
        loop {
            let n = self.cancelled.1.notified();
            tokio::pin!(n);
            n.as_mut().enable();
            if self.cancelled.0.load(Ordering::SeqCst) {
                return;
            }
            n.await;
        }
    }
}

impl EventSource for ScriptEvents {
    fn next(&mut self) -> BoxFuture<'_, Option<Result<RuntimeEvent, ferb_worker::Error>>> {
        Box::pin(async move {
            if self.cancelled.0.load(Ordering::SeqCst) {
                return None;
            }
            if let Some(gate) = self.gate.clone() {
                tokio::select! {
                    _ = gate.wait() => {}
                    _ = self.until_cancelled() => return None,
                }
            }
            if self.at == 0 {
                // a model's answer takes time (Python's scripted model answers from its own thread): what else is
                // ready runs first, as it did there
                for _ in 0..20 {
                    tokio::task::yield_now().await;
                }
            }
            let ev = self.events.get(self.at)?.clone();
            self.at += 1;
            let kind = s(&ev["kind"]);
            let Some(kind) = RuntimeEventKind::ALL
                .iter()
                .copied()
                .find(|k| k.as_str() == kind)
            else {
                // "(no answer: FERB cancelled or left the call)": the call answers nothing until FERB cancels it
                self.until_cancelled().await;
                return None;
            };
            Some(Ok(RuntimeEvent {
                kind,
                data: ev["data"].as_object().cloned().unwrap_or_default(),
            }))
        })
    }
}

// ---------------------------------------------------------------------------------------------------------------
// One scenario
// ---------------------------------------------------------------------------------------------------------------

/// Where a scenario stood after its replay.
#[derive(Default, Debug)]
pub struct Report {
    pub frames: Vec<String>,
    pub calls: Vec<String>,
    pub tables: Vec<String>,
    pub stopped: Option<String>,
    pub skipped: Vec<String>,
    pub checked: usize,
}

impl Report {
    pub fn full(&self) -> bool {
        self.stopped.is_none()
            && self.frames.is_empty()
            && self.calls.is_empty()
            && self.tables.is_empty()
    }

    pub fn verdict(&self) -> String {
        if self.full() {
            return "replays fully".into();
        }
        let mut why = Vec::new();
        if let Some(stop) = &self.stopped {
            why.push(format!("stops at {stop}"));
        }
        why.extend(self.frames.iter().take(2).map(|f| format!("frames: {f}")));
        why.extend(
            self.calls
                .iter()
                .take(1)
                .map(|f| format!("model calls: {f}")),
        );
        why.extend(self.tables.iter().take(3).map(|f| format!("tables: {f}")));
        format!("replays partly — {}", why.join("; "))
    }
}

fn line(v: &Value) -> String {
    ferb_py::json::dumps(v)
}

fn sql_json(v: rusqlite::types::Value) -> Value {
    match v {
        rusqlite::types::Value::Null => Value::Null,
        rusqlite::types::Value::Integer(i) => json!(i),
        rusqlite::types::Value::Real(f) => json!(f),
        rusqlite::types::Value::Text(t) => json!(t),
        rusqlite::types::Value::Blob(b) => json!({"hex": hex::encode(b)}),
    }
}

const NOT_COMPARED: [&str; 3] = ["perception_floor", "recall_state", "trail_changes"];

fn compared(table: &str) -> bool {
    !NOT_COMPARED.contains(&table) && !table.starts_with("learning_")
}

pub fn dump(db: &Db, root: &str) -> BTreeMap<String, (Vec<String>, Vec<Vec<Value>>)> {
    let names: Vec<String> = {
        let mut stmt = db
            .conn()
            .prepare("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")
            .unwrap();
        stmt.query_map([], |r| r.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    };
    let mut out = BTreeMap::new();
    for name in names {
        if name.starts_with("sqlite_") || name == "schema_version" || name == "schema_migrations" {
            continue;
        }
        let Ok(mut stmt) = db
            .conn()
            .prepare(&format!("SELECT * FROM \"{name}\" ORDER BY rowid"))
        else {
            continue;
        };
        let cols: Vec<String> = stmt.column_names().map(str::to_string).collect();
        let n = cols.len();
        let rows: Vec<Vec<Value>> = match stmt.query_map([], |r| {
            (0..n)
                .map(|i| r.get::<_, rusqlite::types::Value>(i).map(sql_json))
                .collect::<Result<Vec<_>, _>>()
        }) {
            Ok(rows) => rows.map(Result::unwrap).collect(),
            Err(_) => continue,
        };
        if rows.is_empty() {
            continue;
        }
        let rows = rows
            .into_iter()
            .map(|r| {
                r.into_iter()
                    .map(|v| match v {
                        Value::String(t) => scrub(&json!(t.replace(root, "/FX"))),
                        other => other,
                    })
                    .collect()
            })
            .collect();
        out.insert(name, (cols, rows));
    }
    out
}

fn is_shadow(name: &str) -> bool {
    ["_data", "_idx", "_content", "_docsize", "_config"]
        .iter()
        .any(|s| name.ends_with(s))
}

fn first_diff(a: &str, b: &str) -> String {
    let (ac, bc): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    let at = (0..ac.len().min(bc.len()))
        .find(|i| ac[*i] != bc[*i])
        .unwrap_or(ac.len().min(bc.len()));
    let from = at.saturating_sub(120);
    let show = |c: &[char]| {
        c[from.min(c.len())..(at + 200).min(c.len())]
            .iter()
            .collect::<String>()
    };
    format!(
        "at char {at}\n    python: …{}\n    rust:   …{}",
        show(&ac),
        show(&bc)
    )
}

fn cut(s: &str) -> String {
    if s.chars().count() > 1200 {
        s.chars().take(1200).collect::<String>() + "…"
    } else {
        s.to_string()
    }
}

/// The registered tools and the answer schemas, read once.
pub fn catalog_parts() -> &'static (Vec<Tool>, Map<String, Value>, String) {
    static P: std::sync::OnceLock<(Vec<Tool>, Map<String, Value>, String)> =
        std::sync::OnceLock::new();
    P.get_or_init(|| {
        let chat = chat();
        let root = recorded_root(chat);
        let catalog = read("rust/fixtures/catalog/tools.json");
        let registered: BTreeSet<String> = chat["scenarios"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|s| s["model_calls"].as_array().unwrap().iter())
            .filter(|m| m["role_played"] != "planner")
            .flat_map(|m| strs(&resolve(&m["tools"])))
            .chain(
                ["about.read", "chat.history", "memory.read", "task.evidence", "trail.days", "trail.query"]
                    .map(String::from),
            )
            .collect();
        let tools: Vec<Tool> = catalog["tools"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|t| registered.contains(t["name"].as_str().unwrap()))
            .map(|t| Tool {
                name: s(&t["name"]),
                row: json!({"name": t["name"], "description": t["description"], "input_schema": t["input_schema"]}),
                scopes: strs(&t["scopes"]),
                identities: strs(&t["identities"]),
                observation: t["facts"]["observation"].as_bool().unwrap_or(false),
            })
            .collect();
        let answers = read("rust/fixtures/catalog/answer_schemas.json");
        let schemas: Map<String, Value> = answers["schemas"]
            .as_object()
            .unwrap()
            .iter()
            .filter_map(|(k, v)| v.get("core").map(|c| (k.clone(), c.clone())))
            .collect();
        (tools, schemas, root)
    })
}

/// One scenario's world: its store, its daemon seam, its supervisor.
pub struct World {
    pub db: Arc<Mutex<Db>>,
    pub daemon: Arc<TestDaemon>,
    pub sup: Supervisor,
    pub ids: Arc<Ids>,
    pub root: String,
    pub dir: tempfile::TempDir,
    clock: Arc<FakeClock>,
    workspace: String,
    home: String,
    question_ids: Arc<AtomicUsize>,
    /// Deliberate differences from Python (ENG-6): (recorded text, what the port says instead), applied to every
    /// expected frame line and table cell before they are compared.
    pub fixes: Vec<(String, String)>,
}

impl World {
    fn fixed(&self, text: &str) -> String {
        self.fixes
            .iter()
            .fold(text.to_string(), |t, (a, b)| t.replace(a.as_str(), b))
    }

    fn supervisor(&self) -> Supervisor {
        new_supervisor(
            &self.db,
            &self.clock,
            &self.ids,
            &self.daemon,
            &self.workspace,
            &self.home,
        )
    }
}

fn new_supervisor(
    db: &Arc<Mutex<Db>>,
    clock: &Arc<FakeClock>,
    ids: &Arc<Ids>,
    daemon: &Arc<TestDaemon>,
    workspace: &str,
    home: &str,
) -> Supervisor {
    {
        Supervisor::new(Deps {
            db: db.clone(),
            clock: clock.clone(),
            ids: ids.clone(),
            redactor: Arc::new(ferb_sanitize::Redactor::new(&[], &[])),
            policy: Arc::new(TestPolicy),
            daemon: daemon.clone(),
            prompts: root(),
            tokenizer: Tokenizer::Utf8Bytes,
            device_id: "unknown-device".into(),
            workspace: workspace.to_string(),
            home: home.to_string(),
            wrap: Arc::new(|t: &str| ferb_sanitize::untrusted::wrap_untrusted_with_id(t, WRAP_ID)),
            new_chat: Arc::new(|| "c00000000001".into()),
            stall_s: 180.0,
            heartbeat_poll_s: 5.0,
        })
    }
}

/// The journal rows the daemon's start wrote (seq ≥ `from` up to the first row of `upto`'s id range), inserted as
/// recorded: the rows another part writes at a start.
fn insert_journal(db: &Db, rows: &[Value]) {
    for row in rows {
        let vals: Vec<rusqlite::types::Value> = row
            .as_array()
            .unwrap()
            .iter()
            .map(|v| match cell(v) {
                Value::Number(n) => rusqlite::types::Value::Integer(n.as_i64().unwrap()),
                Value::String(t) => rusqlite::types::Value::Text(t),
                _ => rusqlite::types::Value::Null,
            })
            .collect();
        db.conn()
            .execute(
                "INSERT INTO journal (seq, id, at, kind, data_json, origin) VALUES (?, ?, ?, ?, ?, ?)",
                rusqlite::params_from_iter(vals),
            )
            .unwrap();
    }
}

/// A recorded scenario (rust/fixtures/daemon/chat_scenarios.json, else control_scenarios.json here) and its index in
/// its file (its folder `/FX/<NN>`).
pub fn scenario(name: &str) -> (usize, &'static Value) {
    for file in [chat(), control()] {
        if let Some(found) = file["scenarios"]
            .as_array()
            .unwrap()
            .iter()
            .enumerate()
            .find(|(_, s)| s["name"] == name)
        {
            return found;
        }
    }
    panic!("no recorded scenario {name}")
}

/// What other parts gave the Python supervisor while the scenario ran (the composition records), bodies resolved.
fn records_of(name: &str) -> Vec<Value> {
    if let Some(recs) = control()["records"].get(name) {
        let inputs = &control()["inputs"];
        return recs
            .as_array()
            .unwrap()
            .iter()
            .map(|r| {
                let mut r = resolve(r);
                if let Some(key) = r["inputs"]["inputs"].as_str() {
                    r["inputs"] = resolve(&inputs[key]);
                }
                r
            })
            .collect();
    }
    scenarios()
        .iter()
        .find(|(n, _)| n == name)
        .map(|(_, r)| r.clone())
        .unwrap_or_default()
}

/// The scenario's world, before its first step.
pub fn world(name: &str) -> World {
    let (index, sc) = scenario(name);
    let (tools, schemas, root) = catalog_parts();
    let records: Vec<Value> = records_of(name);
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(dir.path().join("ferb.db")).unwrap();
    db.migrate_dir(&repo().join("ferb/store/migrations"))
        .unwrap();
    let first_request: usize = first_request(sc);
    let tables = &sc["tables"];
    let start_rows: Vec<Value> = tables["journal"]["rows"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|row| {
            s(&row[1]).starts_with("id-")
                && s(&row[1])[3..].parse::<usize>().unwrap_or(usize::MAX) < first_request
        })
        .cloned()
        .collect();
    insert_journal(&db, &start_rows);
    db.conn()
        .execute(
            "INSERT OR IGNORE INTO conversations(id) VALUES ('owner')",
            [],
        )
        .unwrap();
    let db = Arc::new(Mutex::new(db));
    let ids = Arc::new(Ids {
        n: AtomicUsize::new(first_request - 1),
        prefix: Mutex::new("id".into()),
    });
    let nn = format!("{:02}", index + 1);
    let of = |kind: &str| -> Vec<Value> {
        records
            .iter()
            .filter(|r| r["kind"] == kind)
            .cloned()
            .collect()
    };
    let systems = of("system");
    let availability = systems
        .first()
        .map(|r| s(&r["inputs"]["capabilities"]))
        .unwrap_or_default();
    let recorded: Vec<Value> = sc["model_calls"]
        .as_array()
        .unwrap()
        .iter()
        .map(resolve)
        .collect();
    let question_ids = Arc::new(AtomicUsize::new(0));
    // the calls held until `release_model`: each the call the wait before it named
    let mut gates: HashMap<usize, Arc<Gate>> = HashMap::new();
    let mut last_wait: Option<usize> = None;
    for st in sc["steps"].as_array().unwrap() {
        match st["do"].as_str().unwrap() {
            "wait" => {
                last_wait =
                    model_wait(&s(&st["until"])).and_then(|(k, n)| call_index(&recorded, &k, n))
            }
            "release_model" => {
                if let Some(i) = last_wait {
                    gates.insert(
                        i,
                        Arc::new(Gate {
                            open: AtomicBool::new(false),
                            notify: Notify::new(),
                        }),
                    );
                }
            }
            _ => {}
        }
    }
    let daemon = Arc::new_cyclic(|me| TestDaemon {
        me: me.clone(),
        db: db.clone(),
        ids: ids.clone(),
        root: root.clone(),
        tools: tools.to_vec(),
        schemas: schemas.clone(),
        recorded: recorded.clone(),
        calls: Mutex::default(),
        supervise: Mutex::new(of("supervise")),
        systems: Mutex::new(systems),
        contexts: Mutex::new(of("context")),
        availability,
        requests: Mutex::default(),
        grants: Mutex::default(),
        revoked: Mutex::default(),
        only: Mutex::default(),
        tokens: AtomicUsize::new(0),
        unsupported: Mutex::default(),
        spans: tables["spans"]["rows"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|r| r.as_array().unwrap().clone())
            .collect(),
        span_columns: strs(&tables["spans"]["columns"]),
        gates: Mutex::new(gates),
        questions: Mutex::new(Arc::new(TestQuestions::new(question_ids.clone()))),
        stopped: AtomicBool::new(false),
        unattended: Mutex::default(),
        sinks: Mutex::default(),
        next_sink: AtomicUsize::new(0),
        published: Mutex::default(),
        cancelled: Mutex::default(),
        journal_to: Mutex::default(),
        asked_kinds: Mutex::default(),
        answered: Mutex::default(),
    });
    let clock = Arc::new(
        FakeClock::new(ferb_clock::py::parse("2026-01-01T09:00:00+00:00")).with_zone(Zone::utc()),
    );
    let workspace = format!("{root}/{nn}/home/workspace");
    let home = format!("{root}/{nn}/user");
    let sup = new_supervisor(&db, &clock, &ids, &daemon, &workspace, &home);
    *daemon.journal_to.lock().unwrap() = Some(Arc::downgrade(sup.inner()));
    World {
        db,
        daemon,
        sup,
        ids,
        root: root.clone(),
        dir,
        clock,
        workspace,
        home,
        question_ids,
        fixes: Vec::new(),
    }
}

/// The first request id of a scenario (its first `ack`).
fn first_request(sc: &Value) -> usize {
    sc["steps"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|st| st["lines"].as_array())
        .flatten()
        .filter_map(|l| l.get("recv").and_then(Value::as_str))
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .filter(|v| v["params"]["type"] == "ack")
        .map(|v| s(&v["params"]["request_id"])[3..].parse().unwrap())
        .min()
        .unwrap_or(15)
}

/// "the model has received call 'worker' (1)" -> ("worker", 1).
fn model_wait(until: &str) -> Option<(String, usize)> {
    let rest = until.strip_prefix("the model has received call '")?;
    let (key, rest) = rest.split_once("' (")?;
    Some((key.to_string(), rest.trim_end_matches(')').parse().ok()?))
}

/// The index of the n-th recorded call with this key.
fn call_index(recorded: &[Value], key: &str, n: usize) -> Option<usize> {
    recorded
        .iter()
        .enumerate()
        .filter(|(_, m)| m["key"] == key)
        .nth(n.saturating_sub(1))
        .map(|(i, _)| i)
}

/// Lets the paused runtime run until `done` holds (virtual time moves while every task is idle).
async fn until(mut done: impl FnMut() -> bool) -> bool {
    for _ in 0..20_000 {
        if done() {
            return true;
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    done()
}

type Streams = HashMap<
    String,
    (
        u64,
        Result<tokio::sync::mpsc::UnboundedReceiver<Value>, String>,
    ),
>;

/// Walks the scenario's steps. `upto`: stop before the step at this index (None: every step).
pub fn run(w: &mut World, name: &str) -> Report {
    let (_, sc) = scenario(name);
    let mut report = Report::default();
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .start_paused(true)
        .build()
        .unwrap();
    let steps = sc["steps"].as_array().unwrap().clone();
    rt.block_on(async {
        let mut streams: Streams = HashMap::new();
        for st in &steps {
            let conn = s(&st["conn"]);
            match st["do"].as_str().unwrap() {
                "connect" | "close" | "note" => {}
                "call" => call_step(w, st, &mut report).await,
                "stream" => {
                    let sent: Value =
                        serde_json::from_str(st["lines"][0]["send"].as_str().unwrap()).unwrap();
                    if sent["method"] != "chat.send" {
                        report
                            .skipped
                            .push(format!("the stream {} (the daemon's)", sent["method"]));
                        continue;
                    }
                    let id = sent["id"].as_u64().unwrap();
                    let got = w
                        .sup
                        .chat_send_params(sent["params"].as_object().unwrap())
                        .map_err(|r| r.0);
                    streams.insert(conn, (id, got));
                    tokio::task::yield_now().await;
                }
                "stream_end" | "stream_end_at_shutdown" => {
                    let Some((id, got)) = streams.remove(&conn) else {
                        continue;
                    };
                    let lines = collect(w, id, got, &conn, &mut report).await;
                    compare_lines(w, st, &lines, &conn, &mut report);
                }
                "stream_cut" => report
                    .skipped
                    .push(format!("{conn}'s stream (the daemon's)")),
                "wait" => {
                    let until_ = s(&st["until"]);
                    let ok = wait_step(w, &until_).await;
                    if !ok {
                        report.stopped = Some(format!("a wait for {until_} (never true here)"));
                    }
                }
                "release_model" => {
                    for g in w.daemon.gates.lock().unwrap().values() {
                        g.open.store(true, Ordering::SeqCst);
                        g.notify.notify_waiters();
                    }
                }
                "restart" => {
                    restart(w, &s(&st["ids_prefix"])).await;
                    // the open streams end with the shutdown (`stream_end_at_shutdown`): what the old supervisor
                    // still sent is collected there
                }
                other => {
                    report.stopped = Some(format!("the step {other:?}"));
                }
            }
            if report.stopped.is_some() {
                break;
            }
        }
        for _ in 0..200 {
            tokio::task::yield_now().await;
        }
    });
    if let Some(n) = w.daemon.unsupported.lock().unwrap().first() {
        report.stopped.get_or_insert(n.clone());
    }
    compare_calls(w, sc, &mut report);
    compare_tables(w, sc, &mut report);
    report
}

async fn collect(
    w: &World,
    id: u64,
    got: Result<tokio::sync::mpsc::UnboundedReceiver<Value>, String>,
    conn: &str,
    report: &mut Report,
) -> Vec<String> {
    let mut lines = Vec::new();
    match got {
        Err(text) => lines.push(line(
            &json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32004,
            "message": format!("FerbError: {text}")}}),
        )),
        Ok(mut rx) => {
            loop {
                let next =
                    tokio::time::timeout(std::time::Duration::from_secs(3600), rx.recv()).await;
                let Ok(next) = next else {
                    report.stopped = Some(format!("{conn}'s stream, which never ends here"));
                    break;
                };
                let Some(ev) = next else { break };
                let ev: Value = serde_json::from_str(&line(&ev).replace(&w.root, "/FX")).unwrap();
                lines.push(line(
                    &json!({"jsonrpc": "2.0", "method": "chat.send.event", "params": ev}),
                ));
            }
            if report.stopped.is_none() {
                lines.push(line(
                    &json!({"jsonrpc": "2.0", "id": id, "result": {"ended": true}}),
                ));
            }
        }
    }
    lines
}

fn compare_lines(w: &World, st: &Value, lines: &[String], conn: &str, report: &mut Report) {
    let want: Vec<String> = st["lines"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| w.fixed(&s(&resolve(&l["recv"]))))
        .collect();
    report.checked += want.len();
    for i in 0..want.len().max(lines.len()) {
        let (w, g) = (want.get(i), lines.get(i));
        if w != g {
            report.frames.push(format!(
                "{conn} line {i}\n    python: {}\n    rust:   {}",
                w.map(String::as_str).unwrap_or("(none)"),
                g.map(String::as_str).unwrap_or("(none)")
            ));
            break;
        }
    }
}

/// The daemon's unary handlers the scenarios call (areas/chat.py, areas/rpc.py), each reply compared.
async fn call_step(w: &mut World, st: &Value, report: &mut Report) {
    let lines = st["lines"].as_array().unwrap();
    for l in lines {
        let Some(sent) = l.get("send").and_then(Value::as_str) else {
            continue;
        };
        let v: Value = serde_json::from_str(sent).unwrap();
        let method = s(&v["method"]);
        let id = v["id"].clone();
        let params = v["params"].as_object().cloned().unwrap_or_default();
        let reply = match method.as_str() {
            "auth.hello" => continue,
            "task.stop" => Some(match w.sup.task_stop(&params) {
                Ok(r) => json!({"jsonrpc": "2.0", "id": id, "result": r}),
                Err(e) => {
                    json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32004, "message": e}})
                }
            }),
            "stop.engage" => {
                let summary = w.daemon.engage(&w.sup);
                w.sup.inner().journal("stop.engaged", summary.clone(), None);
                w.daemon.publish("stop.engaged", summary.clone());
                Some(json!({"jsonrpc": "2.0", "id": id, "result": summary}))
            }
            "stop.lift" => {
                w.daemon.stopped.store(false, Ordering::SeqCst);
                w.sup.inner().journal("stop.lifted", json!({}), None);
                w.daemon.publish("stop.lifted", json!({}));
                Some(json!({"jsonrpc": "2.0", "id": id, "result": {"stopped": false}}))
            }
            "chat.approve" | "chat.answer" => {
                Some(resolve_approval(w, &method, &params, id.clone()))
            }
            other => {
                report
                    .skipped
                    .push(format!("{other} (the daemon's composition)"));
                None
            }
        };
        let Some(reply) = reply else { continue };
        let got = line(&reply);
        let want = lines
            .iter()
            .filter_map(|l| l.get("recv").and_then(Value::as_str))
            .find(|r| serde_json::from_str::<Value>(r).is_ok_and(|x| x["id"] == id))
            .unwrap_or("(none)")
            .to_string();
        report.checked += 1;
        if got != want {
            report
                .frames
                .push(format!("{method}\n    python: {want}\n    rust:   {got}"));
        }
        for _ in 0..50 {
            tokio::task::yield_now().await;
        }
    }
}

/// areas/chat.py:243 `_resolve_approval` on the gateway's question store.
fn resolve_approval(w: &World, method: &str, params: &Map<String, Value>, id: Value) -> Value {
    let q = w.daemon.questions.lock().unwrap().clone();
    let approval_id = params
        .get("approval_id")
        .map(ferb_py::value::py_str)
        .unwrap_or_default();
    let (granted, choice, text) = if method == "chat.approve" {
        (
            params.get("granted").is_some_and(ferb_py::value::truthy),
            params
                .get("choice")
                .and_then(Value::as_u64)
                .map(|c| c as usize),
            None,
        )
    } else {
        let text = params
            .get("text")
            .map(ferb_py::value::py_str)
            .unwrap_or_default();
        (!text.is_empty(), None, (!text.is_empty()).then_some(text))
    };
    let mut store = q.store.lock().unwrap();
    if let (Some(t), true) = (&text, granted) {
        let t = ferb_orchestrator::py::strip(t).to_string();
        if let Some(ap) = store.get(&approval_id) {
            let option_only = ferb_orchestrator::steering::OPTION_ONLY
                .contains(&ap.capability.as_str())
                || ap.options_only;
            if option_only && !t.is_empty() && ap.option_for(&t).is_none() {
                let choices = ferb_trace_choices(&ap.options);
                return json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32004,
                    "message": format!("Pick {choices}.")}});
            }
        }
    }
    let resolved = store.resolve(&approval_id, granted, choice, text.as_deref());
    let (state, capability) = match resolved {
        Ok(ap) => (ap.state.as_str().to_string(), ap.capability.clone()),
        Err(e) => {
            return json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32603, "message": format!("{e:?}")}});
        }
    };
    drop(store);
    w.sup.inner().journal(
        "approval.resolved",
        json!({"approval_id": approval_id, "state": state, "capability": capability}),
        None,
    );
    w.daemon.publish(
        "approval.resolved",
        json!({"approval_id": approval_id, "state": state}),
    );
    json!({"jsonrpc": "2.0", "id": id, "result": {"approval_id": approval_id, "state": state}})
}

/// ferb/trace/lines.py `choices`: "Yes or No", "A, B or C".
fn ferb_trace_choices(options: &[String]) -> String {
    match options.len() {
        0 => String::new(),
        1 => options[0].clone(),
        n => format!("{} or {}", options[..n - 1].join(", "), options[n - 1]),
    }
}

async fn wait_step(w: &World, until_: &str) -> bool {
    if let Some((key, n)) = model_wait(until_) {
        let recorded = w.daemon.recorded.clone();
        return until(|| {
            let calls = w.daemon.calls.lock().unwrap();
            calls
                .started
                .iter()
                .filter(|c| {
                    c["n"].as_u64().is_some_and(|i| {
                        recorded
                            .get(i as usize)
                            .is_some_and(|m| m["key"] == key.as_str())
                    })
                })
                .count()
                >= n
        })
        .await;
    }
    match until_ {
        "an approval is pending" | "an approval is pending again" => {
            until(|| !w.daemon.questions.lock().unwrap().pending().is_empty()).await
        }
        "the task is paused" | "the task ended" => {
            let want_paused = until_ == "the task is paused";
            until(|| {
                let db = w.db.lock().unwrap();
                let status: Option<String> = db
                    .conn()
                    .query_row(
                        "SELECT status FROM tasks WHERE parent_id IS NULL ORDER BY rowid DESC LIMIT 1",
                        [],
                        |r| r.get(0),
                    )
                    .ok();
                match status.as_deref() {
                    Some("paused") => want_paused,
                    Some("running") | Some("planned") | None => false,
                    Some(_) => !want_paused,
                }
            })
            .await
        }
        _ => false,
    }
}

/// The daemon's shutdown and a new start on the same store (main.py's `shutdown`, then a new `Daemon`).
async fn restart(w: &mut World, prefix: &str) {
    w.sup.shut_down().await;
    for _ in 0..500 {
        tokio::task::yield_now().await;
    }
    w.ids.restart(prefix);
    *w.daemon.questions.lock().unwrap() = Arc::new(TestQuestions::new(w.question_ids.clone()));
    w.sup = w.supervisor();
    *w.daemon.journal_to.lock().unwrap() = Some(Arc::downgrade(w.sup.inner()));
    w.sup.recover_after_restart();
    let sup = w.sup.clone();
    tokio::spawn(async move {
        sup.resume_saved_plans().await;
    });
    for _ in 0..50 {
        tokio::task::yield_now().await;
    }
}

fn compare_calls(w: &World, _sc: &Value, report: &mut Report) {
    let calls = w.daemon.calls.lock().unwrap();
    for (i, want) in w.daemon.recorded.iter().enumerate() {
        let made = calls.made.get(i);
        let started = calls.started.iter().find(|c| c["n"] == json!(i));
        let Some(made) = made else {
            report
                .calls
                .push(format!("call {} ({}) was not made", i + 1, s(&want["key"])));
            break;
        };
        let parts: Vec<String> = strs(&want["parts"]);
        let continued = if want["continued_with"].is_null() {
            json!([])
        } else {
            want["continued_with"].clone()
        };
        let expected = json!({"runtime_role": want["runtime_role"], "mode": want["mode"], "tools": want["tools"],
            "schema": want["schema"], "effort": want["effort"], "task_id": want["task_id"], "system": want["system"],
            "parts": parts, "cacheable": want["cacheable"], "images": want["images"], "continued_with": continued});
        let got_continued = json!(calls.continued.get(&i).cloned().unwrap_or_default());
        let actual = match started {
            None => {
                json!({"runtime_role": made["runtime_role"], "mode": made["mode"], "tools": made["tools"],
                "schema": made["schema"], "effort": made["effort"], "started": false})
            }
            Some(st) => {
                json!({"runtime_role": made["runtime_role"], "mode": made["mode"], "tools": made["tools"],
                "schema": made["schema"], "effort": made["effort"], "task_id": st["task_id"], "system": st["system"],
                "parts": st["parts"], "cacheable": st["cacheable"], "images": st["images"],
                "continued_with": got_continued})
            }
        };
        report.checked += 1;
        if expected != actual {
            let field = expected
                .as_object()
                .unwrap()
                .keys()
                .find(|k| expected[*k] != actual[*k])
                .cloned()
                .unwrap_or_default();
            let detail = if field == "parts" {
                let (e, a) = (
                    expected["parts"].as_array().unwrap(),
                    actual["parts"].as_array().cloned().unwrap_or_default(),
                );
                let at = (0..e.len().max(a.len()))
                    .find(|i| e.get(*i) != a.get(*i))
                    .unwrap_or(0);
                match (e.get(at), a.get(at)) {
                    (Some(x), Some(y)) => format!("part {at} {}", first_diff(&s(x), &s(y))),
                    (x, y) => format!("part {at}\n    python: {x:?}\n    rust:   {y:?}"),
                }
            } else {
                format!(
                    "\n    python: {}\n    rust:   {}",
                    cut(&expected[&field].to_string()),
                    cut(&actual[&field].to_string())
                )
            };
            report.calls.push(format!(
                "call {} ({}) {field}: {detail}",
                i + 1,
                s(&want["key"])
            ));
        }
    }
    if calls.made.len() > w.daemon.recorded.len() {
        report.calls.push(format!(
            "{} calls made, {} recorded",
            calls.made.len(),
            w.daemon.recorded.len()
        ));
    }
}

fn compare_tables(w: &World, sc: &Value, report: &mut Report) {
    let tables = &sc["tables"];
    let mine = dump(&w.db.lock().unwrap(), &w.root);
    if let Ok(t) = std::env::var("CONTROL_DEBUG") {
        let show = |rows: &[Vec<Value>]| {
            rows.iter()
                .map(|r| {
                    cut(&json!(r).to_string())
                        .chars()
                        .take(260)
                        .collect::<String>()
                })
                .collect::<Vec<_>>()
                .join("\n")
        };
        let want: Vec<Vec<Value>> = tables[&t]["rows"]
            .as_array()
            .map(|a| {
                a.iter()
                    .map(|r| r.as_array().unwrap().iter().map(cell).collect())
                    .collect()
            })
            .unwrap_or_default();
        eprintln!("PYTHON {t}\n{}", show(&want));
        eprintln!(
            "RUST {t}\n{}",
            show(mine.get(&t).map(|x| x.1.as_slice()).unwrap_or(&[]))
        );
    }
    let want_tables = tables.as_object().unwrap();
    for (table, want) in want_tables {
        if !compared(table) {
            continue;
        }
        let cols = strs(&want["columns"]);
        let rows: Vec<Vec<Value>> = want["rows"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| {
                r.as_array()
                    .unwrap()
                    .iter()
                    .map(|c| match scrub(&cell(c)) {
                        Value::String(t) => json!(w.fixed(&t)),
                        other => other,
                    })
                    .collect()
            })
            .collect();
        report.checked += rows.len();
        match mine.get(table) {
            None => report.tables.push(format!("{table}: no rows written")),
            Some((mcols, mrows)) => {
                if *mcols != cols {
                    report.tables.push(format!("{table}: columns {mcols:?}"));
                    continue;
                }
                for i in 0..rows.len().max(mrows.len()) {
                    if rows.get(i) != mrows.get(i) {
                        let detail = match (rows.get(i), mrows.get(i)) {
                            (Some(a), Some(b)) => {
                                let c = (0..a.len()).find(|c| a[*c] != b[*c]).unwrap_or(0);
                                format!(
                                    "{} {}",
                                    cols[c],
                                    first_diff(&a[c].to_string(), &b[c].to_string())
                                )
                            }
                            (a, b) => format!(
                                "\n    python: {}\n    rust:   {}",
                                cut(&a.map(|r| json!(r).to_string()).unwrap_or("(none)".into())),
                                cut(&b.map(|r| json!(r).to_string()).unwrap_or("(none)".into()))
                            ),
                        };
                        report.tables.push(format!("{table} row {i} {detail}"));
                        break;
                    }
                }
            }
        }
    }
    for table in mine.keys() {
        if compared(table) && !want_tables.contains_key(table) && !is_shadow(table) {
            report
                .tables
                .push(format!("{table}: written, not in the recording"));
        }
    }
}
