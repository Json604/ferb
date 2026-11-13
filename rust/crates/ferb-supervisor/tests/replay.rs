//! The forward path replayed against the 28 chat scenarios recorded from the real Python daemon
//! (rust/fixtures/daemon/chat_scenarios.json): every frame the app received on each connection (bytes and order), every
//! model call FERB made (its runtime, tools, answer schema, instructions and context parts, byte for byte) and the store
//! tables at the end.
//!
//! Inputs are what other parts gave the Python supervisor: the scripted model answers (each call's recorded events),
//! the reads the supervisor made of other parts (its environment, live facts, the facts every role's instructions
//! carry, the relevant-activity search, the desktop observations; rust/fixtures/supervisor/scenarios.json recorded them
//! at the call), the gateway's tool listing (rust/fixtures/catalog/tools.json, the daemon's registered tools) and the
//! rows the desktop observer wrote (its `disturbance.*` spans, written through the desktop seam when it is called).
//! The store is a real FERB store in a temporary folder, migrated from ferb/store/migrations, holding the rows the
//! daemon's start wrote before the scenario's first message.
//!
//! Compared after the run: every frame line, every model call, and every table except those another part owns
//! (`learning_*`: the episode card the memory part captures after an outcome; `perception_floor`, `recall_state`,
//! `trail_changes`: the daemon's start). A scenario that needs a path the forward port doesn't have (a stop, a pause, a
//! question answered, a restart, a schedule, a tool call made by the scripted model) is reported with where it stops.

mod common;

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use common::*;
use ferb_clock::clock::FakeClock;
use ferb_clock::zone::Zone;
use ferb_context::assemble::ContextBlock;
use ferb_context::tokens::Tokenizer;
use ferb_core::errors::FerbError;
use ferb_core::types::{
    ModelLimits, ModelRequest, RunHandle, RuntimeEvent, RuntimeEventKind, Task,
};
use ferb_orchestrator::models::Condition;
use ferb_orchestrator::ports::{BoxFuture, Policy};
use ferb_orchestrator::steering::ContinueError;
use ferb_orchestrator::verify::ConditionResult;
use ferb_store::Db;
use ferb_supervisor::flow::{Change, Daemon, Deps, Environment, FrontSeen, Request, Supervisor};
use ferb_supervisor::planner::OpenQuestion;
use ferb_supervisor::prompts::SystemInputs;
use ferb_supervisor::worker_context::Activity;
use ferb_worker::ports::{EventSource, Grant, ModelRuntime, RunControl, StartedRun};
use serde_json::{json, Map, Value};

// ---------------------------------------------------------------------------------------------------------------
// Reading the fixture
// ---------------------------------------------------------------------------------------------------------------

fn bodies() -> &'static HashMap<String, String> {
    static B: std::sync::OnceLock<HashMap<String, String>> = std::sync::OnceLock::new();
    B.get_or_init(|| {
        read("rust/fixtures/daemon/bodies.json")
            .as_object()
            .unwrap()
            .iter()
            .map(|(k, v)| (k.clone(), v.as_str().unwrap().to_string()))
            .collect()
    })
}

/// A recorded table cell's text: `@@body:<sha>:utf8@@` (quotes included) stands for the body as JSON.
fn expand(text: &str) -> String {
    let mut out = text.to_string();
    while let Some(at) = out.find("\"@@body:") {
        let rest = &out[at + 8..];
        let end = rest.find("@@\"").expect("a body token's end");
        let token = &rest[..end];
        let (sha, form) = token.split_once(':').expect("a body token");
        let body = &bodies()[sha];
        let dumped = if form == "ascii" {
            ferb_py::json::dumps(&json!(body))
        } else {
            ferb_py::json::dumps_utf8(&json!(body))
        };
        out = format!("{}{}{}", &out[..at], dumped, &rest[end + 3..]);
    }
    out
}

fn cell(v: &Value) -> Value {
    match v {
        Value::Object(m) if m.len() == 1 && m.contains_key("sha256") => {
            json!(expand(&bodies()[m["sha256"].as_str().unwrap()]))
        }
        Value::String(s) => json!(expand(s)),
        other => other.clone(),
    }
}

/// The volatile keys the recorder masked (make.py `scrub`), masked here the same way at any depth, in JSON text too.
fn scrub(v: &Value) -> Value {
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

/// The recorder's temporary root, as long as it was (see ferb-worker's chat_prompts test: the budget report counts
/// its bytes), so the same parts come out; written back as `/FX`.
fn recorded_root(chat: &Value) -> String {
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

// ---------------------------------------------------------------------------------------------------------------
// The seams
// ---------------------------------------------------------------------------------------------------------------

struct Ids(AtomicUsize);

impl ferb_core::interfaces::IdSource for Ids {
    fn new_id(&self) -> String {
        format!("id-{:06}", self.0.fetch_add(1, Ordering::SeqCst) + 1)
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

/// One registered tool as the gateway lists it.
#[derive(Clone)]
struct Tool {
    name: String,
    row: Value,
    scopes: Vec<String>,
    identities: Vec<String>,
    observation: bool,
}

/// What a scenario's model calls were made with and what the runtime was handed.
#[derive(Default)]
struct Calls {
    made: Vec<Value>,
    started: Vec<Value>,
}

struct TestDaemon {
    me: std::sync::Weak<TestDaemon>,
    db: Arc<Mutex<Db>>,
    ids: Arc<Ids>,
    root: String,
    tools: Vec<Tool>,
    schemas: Map<String, Value>,
    recorded: Vec<Value>,
    calls: Mutex<Calls>,
    supervise: Mutex<Vec<Value>>,
    systems: Mutex<Vec<Value>>,
    contexts: Mutex<Vec<Value>>,
    availability: String,
    requests: Mutex<HashMap<String, Request>>,
    grants: Mutex<HashMap<String, Grant>>,
    /// A planner call's lookups (its token names its tools), by its holder.
    only: Mutex<HashMap<String, Vec<String>>>,
    tokens: AtomicUsize,
    unsupported: Mutex<Vec<String>>,
    spans: Vec<Vec<Value>>,
    span_columns: Vec<String>,
}

fn pop(q: &Mutex<Vec<Value>>, what: &str) -> Value {
    let mut q = q.lock().unwrap();
    assert!(
        !q.is_empty(),
        "the supervisor read {what} more often than the recording did"
    );
    q.remove(0)
}

impl TestDaemon {
    fn note(&self, what: &str) {
        self.unsupported.lock().unwrap().push(what.to_string());
    }

    /// The recorded span the desktop observer wrote for this tree, written now with the next id.
    fn write_span(&self, kind: &str, tree_id: &str) {
        let cols = &self.span_columns;
        let at = |name: &str| cols.iter().position(|c| c == name).unwrap();
        let row = self
            .spans
            .iter()
            .find(|r| r[at("kind")] == kind && r[at("task_id")] == json!(tree_id))
            .cloned();
        let Some(row) = row else {
            self.note(&format!("no recorded {kind} span for {tree_id}"));
            return;
        };
        let id = ferb_core::interfaces::IdSource::new_id(self.ids.as_ref());
        if json!(id) != row[at("id")] {
            // the ids already went apart (a tool call the gateway would have recorded): written under the new id
            self.note(&format!(
                "the desktop observer's span {kind} would be {}, here {id}",
                row[at("id")]
            ));
        }
        let mut row = row;
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
    fn revoke_worker(&self, _worker_id: &str) {}
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
    }
    fn unattended(&self, _request_id: &str) -> bool {
        false
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
    fn live_facts(&self, last_runs: bool, _owner_state: bool) -> String {
        assert!(
            last_runs,
            "the planner reads its live facts with their last runs"
        );
        let rec = pop(&self.supervise, "the planner's live facts");
        assert!(rec["owner_active"].is_null() && rec["last_event"].is_null());
        s(&rec["live_facts"])
    }
    fn system_inputs(&self, live: bool) -> SystemInputs {
        let rec = pop(&self.systems, "a role's instructions");
        assert_eq!(
            rec["live"].as_bool(),
            Some(live),
            "a role's instructions read live facts or not"
        );
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
        assert!(!q.is_empty(), "a context read the recording didn't make");
        Activity::from_json(&q[0]["activity"])
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
    fn memory_push<'a>(
        &'a self,
        _query: &'a str,
        _targets: &'a str,
    ) -> BoxFuture<'a, Vec<ContextBlock>> {
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
    fn on_task_end<'a>(
        &'a self,
        _task_id: &'a str,
        _span_id: &'a str,
    ) -> BoxFuture<'a, Vec<Value>> {
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
    fn declined(
        &self,
        _index: i64,
        _condition: &Condition,
        _task_id: &str,
    ) -> Option<ConditionResult> {
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
        Ok(Arc::new(ScriptRuntime {
            daemon: self.me.upgrade().expect("the scenario's daemon"),
            n,
            events,
        }))
    }
    fn publish(&self, _topic: &str, _data: Value) {}
}

struct ScriptRuntime {
    daemon: Arc<TestDaemon>,
    n: usize,
    events: Value,
}

impl ScriptRuntime {
    fn daemon(&self) -> &TestDaemon {
        &self.daemon
    }
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
            let root = &self.daemon().root;
            let parts: Vec<String> = request
                .parts
                .iter()
                .map(|p| p.replace(root.as_str(), "/FX"))
                .collect();
            self.daemon().calls.lock().unwrap().started.push(
                json!({"n": self.n, "task_id": task.id,
                "system": request.system, "parts": parts, "cacheable": request.cacheable,
                "images": request.images.len()}),
            );
            let cancelled = Arc::new(AtomicBool::new(false));
            Ok(StartedRun {
                handle: RunHandle {
                    id: format!("run-{}", self.n + 1),
                    runtime: "fake".into(),
                    task_id: task.id.clone(),
                    state: Map::new(),
                },
                events: Box::new(ScriptEvents {
                    events: self.events.as_array().cloned().unwrap_or_default(),
                    at: 0,
                    cancelled: cancelled.clone(),
                }),
                control: Arc::new(ScriptControl { cancelled }),
            })
        })
    }
}

struct ScriptControl {
    cancelled: Arc<AtomicBool>,
}

impl RunControl for ScriptControl {
    fn steer<'a>(&'a self, _message: &'a str) -> BoxFuture<'a, Result<(), ContinueError>> {
        Box::pin(async { Ok(()) })
    }
    fn cancel(&self) -> BoxFuture<'_, ()> {
        Box::pin(async move { self.cancelled.store(true, Ordering::SeqCst) })
    }
}

struct ScriptEvents {
    events: Vec<Value>,
    at: usize,
    cancelled: Arc<AtomicBool>,
}

impl EventSource for ScriptEvents {
    fn next(&mut self) -> BoxFuture<'_, Option<Result<RuntimeEvent, ferb_worker::Error>>> {
        Box::pin(async move {
            if self.cancelled.load(Ordering::SeqCst) {
                return None;
            }
            let ev = self.events.get(self.at)?.clone();
            self.at += 1;
            let kind = s(&ev["kind"]);
            let Some(kind) = RuntimeEventKind::ALL
                .iter()
                .copied()
                .find(|k| k.as_str() == kind)
            else {
                // "(no answer: FERB cancelled or left the call)": the call never answers
                std::future::pending::<()>().await;
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

#[derive(Default)]
struct Report {
    frames: Vec<String>,
    calls: Vec<String>,
    tables: Vec<String>,
    stopped: Option<String>,
    checked: usize,
}

impl Report {
    fn full(&self) -> bool {
        self.stopped.is_none()
            && self.frames.is_empty()
            && self.calls.is_empty()
            && self.tables.is_empty()
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

fn dump(db: &Db, root: &str) -> BTreeMap<String, (Vec<String>, Vec<Vec<Value>>)> {
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

fn run_scenario(
    sc: &Value,
    records: &[Value],
    index: usize,
    root_len_root: &str,
    tools: &[Tool],
    schemas: &Map<String, Value>,
) -> Report {
    let mut report = Report::default();
    let name = s(&sc["name"]);
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(dir.path().join("ferb.db")).unwrap();
    db.migrate_dir(&repo().join("ferb/store/migrations"))
        .unwrap();
    // the rows the daemon's start wrote before the first message
    let first_request: usize = sc["steps"]
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
        .unwrap_or(15);
    let tables = &sc["tables"];
    for row in tables["journal"]["rows"].as_array().unwrap() {
        let n: usize = s(&row[1])[3..].parse().unwrap_or(usize::MAX);
        if n < first_request {
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
    db.conn()
        .execute(
            "INSERT OR IGNORE INTO conversations(id) VALUES ('owner')",
            [],
        )
        .unwrap();
    let db = Arc::new(Mutex::new(db));
    let ids = Arc::new(Ids(AtomicUsize::new(first_request - 1)));
    let nn = format!("{:02}", index + 1);
    let root = root_len_root.to_string();
    let workspace = format!("{root}/{nn}/home/workspace");
    let home = format!("{root}/{nn}/user");
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
    let span_columns: Vec<String> = strs(&tables["spans"]["columns"]);
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
        span_columns,
    });
    let clock = Arc::new(
        FakeClock::new(ferb_clock::py::parse("2026-01-01T09:00:00+00:00")).with_zone(Zone::utc()),
    );
    let sup = Supervisor::new(Deps {
        db: db.clone(),
        clock,
        ids: ids.clone(),
        redactor: Arc::new(ferb_sanitize::Redactor::new(&[], &[])),
        policy: Arc::new(TestPolicy),
        daemon: daemon.clone(),
        prompts: root_prompts(),
        tokenizer: Tokenizer::Utf8Bytes,
        device_id: "unknown-device".into(),
        workspace,
        home,
        wrap: Arc::new(|t: &str| ferb_sanitize::untrusted::wrap_untrusted_with_id(t, WRAP_ID)),
        new_chat: Arc::new(|| "c00000000001".into()),
        stall_s: 180.0,
        heartbeat_poll_s: 5.0,
    });
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .start_paused(true)
        .build()
        .unwrap();
    rt.block_on(async {
        let mut streams: HashMap<String, (u64, Result<tokio::sync::mpsc::UnboundedReceiver<Value>, String>)> =
            HashMap::new();
        for st in sc["steps"].as_array().unwrap() {
            let conn = s(&st["conn"]);
            match st["do"].as_str().unwrap() {
                "connect" | "close" | "note" => {}
                "call" => {
                    for l in st["lines"].as_array().unwrap() {
                        if let Some(sent) = l.get("send") {
                            let v: Value = serde_json::from_str(sent.as_str().unwrap()).unwrap();
                            if v["method"] != "auth.hello" {
                                report.stopped = Some(format!("the unary call {} (the daemon's)", v["method"]));
                            }
                        }
                    }
                }
                "stream" => {
                    let sent: Value =
                        serde_json::from_str(st["lines"][0]["send"].as_str().unwrap()).unwrap();
                    if sent["method"] != "chat.send" {
                        report.stopped = Some(format!("the stream {} (the daemon's)", sent["method"]));
                        break;
                    }
                    let id = sent["id"].as_u64().unwrap();
                    let got = sup
                        .chat_send_params(sent["params"].as_object().unwrap())
                        .map_err(|r| r.0);
                    streams.insert(conn, (id, got));
                    tokio::task::yield_now().await;
                }
                "stream_end" => {
                    let Some((id, got)) = streams.remove(&conn) else { continue };
                    let mut lines = Vec::new();
                    match got {
                        Err(text) => lines.push(line(&json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32004,
                            "message": format!("FerbError: {text}")}}))),
                        Ok(mut rx) => {
                            // a stream that waits for a step this port doesn't take (a stop, an answer) never ends:
                            // the paused clock moves an hour while every task is idle, and the scenario stops there
                            loop {
                                let next = tokio::time::timeout(std::time::Duration::from_secs(3600), rx.recv()).await;
                                let Ok(next) = next else {
                                    report.stopped = Some(format!("{conn}'s stream, which never ends here"));
                                    break;
                                };
                                let Some(ev) = next else { break };
                                let ev: Value = serde_json::from_str(&line(&ev).replace(&root, "/FX")).unwrap();
                                lines.push(line(&json!({"jsonrpc": "2.0", "method": "chat.send.event", "params": ev})));
                            }
                            if report.stopped.is_some() {
                                break;
                            }
                            lines.push(line(&json!({"jsonrpc": "2.0", "id": id, "result": {"ended": true}})));
                        }
                    }
                    let want: Vec<String> = st["lines"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|l| s(&resolve(&l["recv"])))
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
                "wait" => {
                    let until = s(&st["until"]);
                    let Some(n) = until
                        .strip_prefix("the model has received call ")
                        .and_then(|r| r.rsplit_once('('))
                        .and_then(|(_, n)| n.trim_end_matches(')').parse::<usize>().ok())
                    else {
                        report.stopped = Some(format!("a wait for {until}"));
                        break;
                    };
                    for _ in 0..10_000 {
                        if daemon.calls.lock().unwrap().started.len() >= n {
                            break;
                        }
                        tokio::task::yield_now().await;
                    }
                }
                other => {
                    report.stopped = Some(format!("the step {other:?}"));
                    break;
                }
            }
            if report.stopped.is_some() {
                break;
            }
        }
        for _ in 0..100 {
            tokio::task::yield_now().await;
        }
    });
    if let Some(n) = daemon.unsupported.lock().unwrap().first() {
        report.stopped.get_or_insert(n.clone());
    }
    if std::env::var("REPLAY_DEBUG").is_ok() {
        let db = db.lock().unwrap();
        let mut st = db
            .conn()
            .prepare("SELECT seq, id, kind, data_json FROM journal WHERE seq >= 15 ORDER BY seq")
            .unwrap();
        let rows: Vec<String> = st
            .query_map([], |r| {
                Ok(format!(
                    "{} {} {} {}",
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?
                ))
            })
            .unwrap()
            .map(Result::unwrap)
            .collect();
        eprintln!("JOURNAL\n{}", rows.join("\n"));
        let mut st = db
            .conn()
            .prepare("SELECT id, kind, task_id, error FROM spans ORDER BY rowid")
            .unwrap();
        let rows: Vec<String> = st
            .query_map([], |r| {
                Ok(format!(
                    "{} {} {:?} {:?}",
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, Option<String>>(2)?,
                    r.get::<_, Option<String>>(3)?
                ))
            })
            .unwrap()
            .map(Result::unwrap)
            .collect();
        eprintln!("SPANS\n{}", rows.join("\n"));
    }
    // the model calls
    let calls = daemon.calls.lock().unwrap();
    for (i, want) in recorded.iter().enumerate() {
        let made = calls.made.get(i);
        let started = calls.started.iter().find(|c| c["n"] == json!(i));
        let Some(made) = made else {
            report
                .calls
                .push(format!("call {} ({}) was not made", i + 1, s(&want["key"])));
            break;
        };
        let parts: Vec<String> = strs(&want["parts"]);
        let expected = json!({"runtime_role": want["runtime_role"], "mode": want["mode"], "tools": want["tools"],
            "schema": want["schema"], "effort": want["effort"], "task_id": want["task_id"], "system": want["system"],
            "parts": parts, "cacheable": want["cacheable"], "images": want["images"]});
        let actual = match started {
            None => {
                json!({"runtime_role": made["runtime_role"], "mode": made["mode"], "tools": made["tools"],
                "schema": made["schema"], "effort": made["effort"], "started": false})
            }
            Some(st) => {
                json!({"runtime_role": made["runtime_role"], "mode": made["mode"], "tools": made["tools"],
                "schema": made["schema"], "effort": made["effort"], "task_id": st["task_id"], "system": st["system"],
                "parts": st["parts"], "cacheable": st["cacheable"], "images": st["images"]})
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
                format!(
                    "part {at}\n    python: {:?}\n    rust:   {:?}",
                    e.get(at),
                    a.get(at)
                )
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
    if calls.made.len() > recorded.len() {
        report.calls.push(format!(
            "{} calls made, {} recorded",
            calls.made.len(),
            recorded.len()
        ));
    }
    drop(calls);
    // the tables
    let mine = dump(&db.lock().unwrap(), &root);
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
                    .map(|c| scrub(&cell(c)))
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
                        let show = |r: Option<&Vec<Value>>| {
                            cut(&r.map(|r| json!(r).to_string()).unwrap_or("(none)".into()))
                        };
                        let col = match (rows.get(i), mrows.get(i)) {
                            (Some(a), Some(b)) => (0..a.len())
                                .find(|c| a[*c] != b[*c])
                                .map(|c| cols[c].clone())
                                .unwrap_or_default(),
                            _ => String::new(),
                        };
                        let detail = match (rows.get(i), mrows.get(i)) {
                            (Some(a), Some(b)) => {
                                let c = cols.iter().position(|x| *x == col).unwrap_or(0);
                                first_diff(&a[c].to_string(), &b[c].to_string())
                            }
                            _ => format!(
                                "\n    python: {}\n    rust:   {}",
                                show(rows.get(i)),
                                show(mrows.get(i))
                            ),
                        };
                        report
                            .tables
                            .push(format!("{table} row {i} {col} {detail}"));
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
    let _ = name;
    report
}

/// Where two texts first differ, with some context on each side.
fn first_diff(a: &str, b: &str) -> String {
    let (ac, bc): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    let at = (0..ac.len().min(bc.len()))
        .find(|i| ac[*i] != bc[*i])
        .unwrap_or(ac.len().min(bc.len()));
    let from = at.saturating_sub(150);
    let show = |c: &[char]| {
        c[from.min(c.len())..(at + 250).min(c.len())]
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

fn root_prompts() -> ferb_context::prompts::PromptRoot {
    root()
}

/// The scenarios the forward path replays fully: the plain reply, the clarifying question and its answer, the
/// one-step task, the two-step plan, the plan sent back for repair, the checker's rejection, the checker's findings
/// with the fix turn, the merged message and the attachment with chat.send's refusals.
const FULL: [&str; 9] = [
    "plain_reply",
    "clarify_then_answer",
    "one_step_task",
    "multi_step_plan",
    "plan_repair",
    "checker_rejects",
    "checker_findings_fix",
    "second_message_merged",
    "attachment_and_bad_params",
];

#[test]
fn the_recorded_chat_scenarios() {
    let chat = read("rust/fixtures/daemon/chat_scenarios.json");
    let root = recorded_root(&chat);
    let catalog = read("rust/fixtures/catalog/tools.json");
    // the daemon's registry: the tools its owner workers were offered (every one but those a missing grant hides)
    let registered: BTreeSet<String> = chat["scenarios"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|s| s["model_calls"].as_array().unwrap().iter())
        .filter(|m| m["role_played"] != "planner")
        .flat_map(|m| strs(&resolve(&m["tools"])))
        .chain(
            [
                "about.read",
                "chat.history",
                "memory.read",
                "task.evidence",
                "trail.days",
                "trail.query",
            ]
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
    let supervisor = scenarios();
    let mut lines = Vec::new();
    let mut failures = Vec::new();
    let mut checked = 0;
    let only = std::env::var("REPLAY").ok();
    for (i, sc) in chat["scenarios"].as_array().unwrap().iter().enumerate() {
        let name = s(&sc["name"]);
        if only.as_ref().is_some_and(|o| *o != name) {
            continue;
        }
        let records = &supervisor.iter().find(|(n, _)| *n == name).unwrap().1;
        let r = run_scenario(sc, records, i, &root, &tools, &schemas);
        checked += r.checked;
        let verdict = if r.full() {
            "replays fully".to_string()
        } else {
            let mut why = Vec::new();
            if let Some(stop) = &r.stopped {
                why.push(format!("stops at {stop}"));
            }
            why.extend(r.frames.iter().take(1).map(|f| format!("frames: {f}")));
            why.extend(r.calls.iter().take(1).map(|f| format!("model calls: {f}")));
            why.extend(r.tables.iter().take(2).map(|f| format!("tables: {f}")));
            format!("replays partly — {}", why.join("; "))
        };
        lines.push(format!("{name}: {verdict}"));
        if FULL.contains(&name.as_str()) && !r.full() {
            failures.push(format!("{name}: {verdict}"));
        }
    }
    eprintln!("{}", lines.join("\n"));
    eprintln!("{checked} comparisons");
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}

/// The one deliberate difference from Python on the planner's answer (ENG-6): a malformed legacy answer whose validator
/// raised TypeError went past Python's `except ValidationError` and failed the turn; here it is repaired like any
/// invalid answer, under the exception's class.
#[test]
fn a_legacy_answer_python_raised_on_is_repaired() {
    for legacy in [
        r#"{"kind": "task", "text": "x", "task": {"goal": "g", "name": "n", "identity": "owner", "worker_type": "default",
            "targets": {"apps": [], "sites": [], "paths": [], "recipients": []}, "success_conditions": [],
            "plan": {"steps": [{"id": "a", "kind": "map", "goal": "g", "inputs": 5}]}}}"#,
        r#"{"kind": "reply", "text": "x", "schedule": {"request": "r"}, "schedules": 7}"#,
    ] {
        let e = ferb_orchestrator::validate::supervisor_outcome_validate_json(legacy).unwrap_err();
        assert_eq!(
            e.raised().map(|r| r.exception),
            Some("TypeError"),
            "Python raises TypeError on {legacy}"
        );
        assert_eq!(ferb_supervisor::flow::turn::repair_class(&e), "TypeError");
    }
}
