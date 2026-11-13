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
//! Compared after the run: every frame line, every model call, every tool call's reply through the bridge, the chat
//! RPCs' replies (`chat.status`, `chat.approve`, `chat.answer`, `task.stop`) and every table except those another part
//! owns (`learning_*`: the episode card the memory part captures after an outcome; `perception_floor`,
//! `recall_state`, `trail_changes`: the daemon's start).
//!
//! The daemon's side is real where it is the supervisor's partner: ferb-gateway's `Gateway` (its pipeline, spans,
//! ledger rows, questions and events) with the change record (ferb-fs) behind its hooks, scripted tools answering with
//! the recorded tool results, the bus a chat turn subscribes to, and compose.py's event consumers a chat scenario
//! reaches (`_publish`'s request id on a task's question, `_question_record`). The daemon's other RPC areas, its
//! notifications and the timer loop's delivery are passed over and named in the report; a restart isn't replayed.
//! FERB hashes real paths into a few records and this run's folder can't be the recorder's, so 64-hex hashes are
//! compared as `<sha256>`.

mod common;
#[path = "replay/daemon_side.rs"]
mod daemon_side;
#[path = "replay/gateway.rs"]
mod gw;

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
use ferb_supervisor::flow::{
    BusSink, Change, Daemon, Deps, Environment, FrontSeen, LiveTask, Request, Supervisor,
    Unsubscribe,
};
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

/// How long the recorder's temporary root was (see ferb-worker's chat_prompts test: the budget report counts its
/// bytes), so the same parts come out.
fn recorded_root_len(chat: &Value) -> usize {
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
    3 + extra / count
}

/// This run's folder: a real folder under the system's temp folder (the tools write files in it, the change record
/// reads them), as long as the recorder's (`$TMPDIR/ferbdx`) so the budget lines count the same bytes, and spelled
/// as it was (macOS's temp folder resolves through `/private`, as the recorder's did). Written back as `/FX` in both
/// spellings, as make.py masks both. Never the recorder's own folder: a recorder running meanwhile empties it.
struct RunRoot {
    root: String,
    resolved: String,
}

impl RunRoot {
    /// The folder (removed when the returned guard drops: the test holds it, since the scenarios' supervisor and
    /// daemon hold each other and are never dropped) and its spellings.
    fn new(len: usize) -> (tempfile::TempDir, RunRoot) {
        let base = std::env::temp_dir();
        let base = base.to_str().unwrap().trim_end_matches('/').to_string();
        let name = len
            .checked_sub(base.len() + 1)
            .filter(|n| *n >= 3)
            .unwrap_or(6);
        let dir = tempfile::Builder::new()
            .prefix("fx")
            .rand_bytes(name - 2)
            .tempdir_in(&base)
            .unwrap();
        let root = dir.path().to_str().unwrap().to_string();
        let resolved = std::fs::canonicalize(&root)
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();
        (dir, RunRoot { root, resolved })
    }

    /// The run's folder written `/FX`, its resolved spelling first (it holds the other).
    fn mask(&self, text: &str) -> String {
        text.replace(&self.resolved, "/FX")
            .replace(&self.root, "/FX")
    }
}

// ---------------------------------------------------------------------------------------------------------------
// The seams
// ---------------------------------------------------------------------------------------------------------------

/// `SequentialIds("id")`; after a restart `SequentialIds("r1")` (as the recorder's `own_daemon`).
struct Ids {
    prefix: Mutex<String>,
    n: AtomicUsize,
}

impl Ids {
    fn new(prefix: &str, from: usize) -> Ids {
        Ids {
            prefix: Mutex::new(prefix.into()),
            n: AtomicUsize::new(from),
        }
    }

    fn restart(&self, prefix: &str) {
        *self.prefix.lock().unwrap() = prefix.into();
        self.n.store(0, Ordering::SeqCst);
    }
}

impl ferb_core::interfaces::IdSource for Ids {
    fn new_id(&self) -> String {
        let prefix = self.prefix.lock().unwrap().clone();
        format!("{prefix}-{:06}", self.n.fetch_add(1, Ordering::SeqCst) + 1)
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
    run: Arc<RunRoot>,
    /// The daemon's gateway (ferb-gateway), built on the scenario's store (a new one after a restart).
    gateway: Mutex<Option<Arc<ferb_gateway::gateway::Gateway>>>,
    /// The change record (L39) the gateway's hooks write and the checker reads.
    changes: Arc<ferb_fs::changes::Changes>,
    /// The daemon's bus: (subscriber, its topics, where its events go).
    bus: Mutex<Vec<(usize, Vec<String>, BusSink)>>,
    subscribers: AtomicUsize,
    /// The run tokens as the server checks them (`RunTokens.validate`): their grants.
    run_grants: Mutex<HashMap<String, ferb_rpc::auth::RunGrant>>,
    /// The live task records the gateway reads.
    live_tasks: Mutex<HashMap<String, LiveTask>>,
    /// The model calls whose final answer was delivered.
    answered: Mutex<Vec<usize>>,
    /// A file the owner writes when the first checker has answered (a `note` step): (path, text).
    note: Mutex<Option<(String, String)>>,
    /// Where a tool call through the bridge differed from the recording.
    tool_diffs: Mutex<Vec<String>>,
    tool_calls: AtomicUsize,
    redactor: Arc<ferb_sanitize::Redactor>,
    clock: Arc<FakeClock>,
    /// The supervisor, for the journal rows the daemon's event consumers write (a new one after a restart).
    sup: Mutex<Option<Supervisor>>,
    /// compose.py `_asked_kinds`, `_answered_ids`.
    asked_kinds: Mutex<HashMap<String, String>>,
    answered_ids: Mutex<BTreeSet<String>>,
    /// The supervisor's own questions (see [`OwnQuestions`]); in memory, so a restart starts a new store.
    own: Mutex<Arc<OwnQuestions>>,
    /// The recorded desktop-observer rows already written (each is written once).
    spans_used: Mutex<BTreeSet<usize>>,
    /// The trees the desktop observer watches now (a baseline taken, no summary yet).
    watching: Mutex<BTreeSet<String>>,
    /// The daemon's group A (schedules, reminders, timers) with the supervisor in its seams, when the scenario
    /// reaches it.
    side: Mutex<Option<Arc<daemon_side::DaemonSide>>>,
    /// `state.approvals.unattended`.
    unattended: Mutex<BTreeSet<String>>,
    /// `state.stopped`: the global stop is engaged (`stop.engage`).
    stopped: Arc<AtomicBool>,
    /// FERB's workspace and the owner's home (the schedules port normalizes paths as a task's).
    workspace: String,
    user_home: String,
    /// Calls the recorder held until `release_model` (their index), and whether they are released.
    gated: BTreeSet<usize>,
    released: Arc<AtomicBool>,
    released_now: Arc<tokio::sync::Notify>,
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
    fn gw(&self) -> Arc<ferb_gateway::gateway::Gateway> {
        self.gateway
            .lock()
            .unwrap()
            .clone()
            .expect("the scenario's gateway")
    }

    fn own(&self) -> Arc<OwnQuestions> {
        self.own.lock().unwrap().clone()
    }

    fn sup(&self) -> Option<Supervisor> {
        self.sup.lock().unwrap().clone()
    }

    /// A question of the gateway's or the supervisor's own, by id.
    fn approval(&self, id: &str) -> Option<ferb_gateway::questions::Approval> {
        self.gw()
            .approval(id)
            .or_else(|| self.own().store.lock().unwrap().get(id).cloned())
    }

    /// Every pending question, in asking order.
    fn pending(&self) -> Vec<ferb_gateway::questions::Approval> {
        ChatGw(self.gw(), self.own(), self.stopped.clone()).all_pending()
    }

    /// compose.py `_publish` and the consumers `_wire_events` installs, in their order, as far as a chat scenario
    /// reaches them: a task's question names its request (the event contract); the event is redacted once; the bus
    /// hands it to its subscribers; `_question_record` journals every question asked and how it was answered.
    fn events(&self, topic: &str, data: ferb_sanitize::Py) {
        let mut data = ferb_gateway::gateway::py_value(&data)
            .as_object()
            .cloned()
            .unwrap_or_default();
        let marked = self
            .redactor
            .py(&ferb_sanitize::Py::from(&Value::Object(data.clone())));
        if topic == "approval.requested"
            && data.get("task_id").is_some_and(ferb_py::value::truthy)
            && !data.contains_key("request_id")
        {
            if let Some(ap) = self.approval(&s(&data["approval_id"])) {
                if !ap.request_id.is_empty() {
                    data.insert("request_id".into(), json!(ap.request_id));
                }
            }
        }
        let py = if data.contains_key("request_id") && topic == "approval.requested" {
            self.redactor
                .py(&ferb_sanitize::Py::from(&Value::Object(data.clone())))
        } else {
            marked
        };
        self.bus_publish(topic, &py);
        if topic == "approval.requested" || topic == "approval.resolved" {
            self.question_record(topic, &data);
        }
    }

    /// compose.py `_question_record` (DMN-583).
    fn question_record(&self, topic: &str, data: &Map<String, Value>) {
        let Some(ap) = self.approval(&s(&data["approval_id"])) else {
            return;
        };
        let Some(sup) = self.sup() else { return };
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
        let kind = if let Some(given) = given {
            given
        } else if ["owner_login_code", "owner_admin_password", "owner_secret"]
            .contains(&ap.kind.as_str())
        {
            "secret".to_string()
        } else {
            match ap.capability.as_str() {
                "ferb.confirm" => "confirm",
                "ferb.retry" => "retry",
                "plan.ask" | "owner.ask" => "task",
                "loop.ask" => "loop",
                "runtime.switch" => "runtime_switch",
                other => other,
            }
            .to_string()
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
            sup.inner().journal("question.asked", row, Some("ferb"));
            return;
        }
        if ap.state == ferb_gateway::questions::State::Pending
            || !self.answered_ids.lock().unwrap().insert(ap.id.clone())
        {
            return;
        }
        let kind = self
            .asked_kinds
            .lock()
            .unwrap()
            .remove(&ap.id)
            .unwrap_or(kind);
        let answer = ap.answer.clone().unwrap_or_default();
        let choice = ap.choice.clone().unwrap_or_default();
        let said = if ap.hidden.is_empty() {
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
        sup.inner().journal("question.answered", row, Some("ferb"));
    }

    /// The daemon's bus: an event to every subscriber whose topics take it.
    fn bus_publish(&self, topic: &str, data: &ferb_sanitize::Py) {
        let sinks: Vec<BusSink> = self
            .bus
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
        for sink in sinks {
            sink(topic, data);
        }
    }

    /// The gateway's live record of a task (tree.py `TaskRecord`), as the supervisor last gave it.
    fn add_task(&self, rec: &LiveTask) {
        self.gw().add_task(ferb_gateway::gateway::TaskRecord {
            task_id: rec.task_id.clone(),
            request_id: rec.request_id.clone(),
            started_at: ferb_clock::py::parse(&rec.started_at).unwrap(),
            deadline: rec.deadline.as_deref().and_then(ferb_clock::py::parse),
            span_id: rec.span_id.clone(),
            tree_id: rec.tree_id.clone(),
        });
    }

    /// What a daemon keeps only in memory goes with a restart: its requests, run tokens, bus subscriptions, live task
    /// records, questions and compose.py's question records.
    fn restart(&self) {
        self.requests.lock().unwrap().clear();
        self.grants.lock().unwrap().clear();
        self.run_grants.lock().unwrap().clear();
        self.only.lock().unwrap().clear();
        self.bus.lock().unwrap().clear();
        self.live_tasks.lock().unwrap().clear();
        self.asked_kinds.lock().unwrap().clear();
        self.answered_ids.lock().unwrap().clear();
    }

    /// The owner's file a `note` step names, written when its moment comes.
    fn apply_note(&self) {
        if let Some((path, text)) = self.note.lock().unwrap().take() {
            std::fs::write(path, text).unwrap();
        }
    }

    fn note(&self, what: &str) {
        self.unsupported.lock().unwrap().push(what.to_string());
    }

    /// The recorded span the desktop observer wrote for this tree, written now with the next id.
    fn write_span(&self, kind: &str, tree_id: &str) {
        let cols = &self.span_columns;
        let at = |name: &str| cols.iter().position(|c| c == name).unwrap();
        let found = {
            let mut used = self.spans_used.lock().unwrap();
            let found = self.spans.iter().enumerate().find(|(i, r)| {
                !used.contains(i) && r[at("kind")] == kind && r[at("task_id")] == json!(tree_id)
            });
            if let Some((i, _)) = found {
                used.insert(i);
            }
            found.map(|(_, r)| r.clone())
        };
        let Some(row) = found else {
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
        max_calls: Option<usize>,
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
        self.run_grants.lock().unwrap().insert(
            token.clone(),
            ferb_rpc::auth::RunGrant {
                task_id: task_id.into(),
                worker_id: worker_id.into(),
                scopes: scopes.clone(),
                identity: if identity == "ferb" {
                    ferb_rpc::auth::Identity::Ferb
                } else {
                    ferb_rpc::auth::Identity::Owner
                },
                expires_at: f64::INFINITY,
                observe_only,
                tools: tools
                    .map(|t| t.iter().cloned().collect())
                    .unwrap_or_default(),
                max_calls: max_calls.unwrap_or(0) as u64,
            },
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
        self.run_grants
            .lock()
            .unwrap()
            .retain(|_, g| g.worker_id != worker_id);
    }
    fn forget_calls(&self, worker_id: &str) {
        self.gw().forget_calls(worker_id)
    }
    fn calls_refused(&self, watch_id: &str) -> u64 {
        self.gw().calls_refused(watch_id)
    }
    fn lookups_made(&self, holder: &str) -> usize {
        self.gw().lookups_made(holder) as usize
    }
    fn initial_image_given(&self, task_id: &str) {
        self.gw().initial_image_given(task_id)
    }
    fn unresolved(&self, root: &str) -> Vec<Map<String, Value>> {
        let ledger = gw::RealLedger {
            db: self.db.clone(),
            clock: self.clock.clone(),
            ids: self.ids.clone(),
        };
        ledger
            .with(|l| l.unresolved(root))
            .unwrap_or_default()
            .into_iter()
            .filter_map(|v| v.as_object().cloned())
            .collect()
    }
    fn forget_tasks(&self, ids: &BTreeSet<String>) {
        self.gw().forget_tasks(ids)
    }
    fn task_folders(&self, tree_id: &str, folders: Option<&[String]>) {
        self.gw()
            .set_task_folders(tree_id, folders.map(<[String]>::to_vec).unwrap_or_default())
    }
    fn record_named(&self, _words: &str, _request_id: &str) {}
    fn standing_revoke(&self, _tree_id: &str) {}
    fn standing_scopes(&self, _tree_id: &str) -> Vec<Value> {
        Vec::new()
    }
    fn record_request(&self, request: &Request) {
        let set = |v: &Vec<String>| v.iter().cloned().collect::<BTreeSet<String>>();
        self.gw().record_request(ferb_policy::small::Request {
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
        self.gw().forget_request(request_id);
        self.requests.lock().unwrap().remove(request_id);
    }
    fn unattended(&self, request_id: &str) -> bool {
        self.unattended.lock().unwrap().contains(request_id)
    }
    fn stopped(&self) -> bool {
        self.stopped.load(Ordering::SeqCst)
    }
    fn set_unattended(&self, request_id: &str) {
        self.unattended
            .lock()
            .unwrap()
            .insert(request_id.to_string());
    }
    fn schedules_chat(
        &self,
    ) -> Option<Arc<dyn ferb_supervisor::flow::schedules_chat::SchedulesChat>> {
        let side = self.side.lock().unwrap().clone()?;
        Some(Arc::new(
            ferb_supervisor::flow::daemon_seams::DaemonSchedules {
                local: side.local.clone(),
                home: self.user_home.clone(),
                workspace: self.workspace.clone(),
                blocking: true,
            },
        ))
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
        // what the activity search read held this run's paths (the recorder wrote them `/FX`, as the owner's
        // activity journal spells them: as written)
        let activity: Value =
            serde_json::from_str(&line(&q[0]["activity"]).replace("/FX", &self.run.root)).unwrap();
        Activity::from_json(&activity)
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
        Box::pin(async move {
            self.watching.lock().unwrap().insert(tree_id.to_string());
            self.write_span("disturbance.baseline", tree_id)
        })
    }
    fn on_task_finish<'a>(&'a self, tree_id: &'a str) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            self.watching.lock().unwrap().remove(tree_id);
            self.write_span("disturbance.summary", tree_id)
        })
    }
    fn mark_unknown_after_restart(&self) -> Vec<String> {
        let ledger = gw::RealLedger {
            db: self.db.clone(),
            clock: self.clock.clone(),
            ids: self.ids.clone(),
        };
        ledger
            .with(|l| l.mark_unknown_after_restart())
            .unwrap_or_default()
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
    fn changes(&self, task_id: &str, tree_id: &str) -> Option<Result<Vec<Change>, String>> {
        let found = self.changes.changes(task_id, Some(tree_id));
        Some(Ok(found
            .iter()
            .map(|c| Change {
                path: c.path.clone(),
                what: c.what.clone(),
                source: c.source.clone(),
                content: c.content,
                named: c.named,
                text: self.changes.describe(c),
            })
            .collect()))
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
        let bridge = self
            .recorded
            .get(n)
            .and_then(|m| m["tool_bridge_lines"].as_array().cloned())
            .unwrap_or_default();
        Ok(Arc::new(ScriptRuntime {
            daemon: self.me.upgrade().expect("the scenario's daemon"),
            n,
            events,
            bridge,
        }))
    }
    fn publish(&self, topic: &str, data: Value) {
        self.events(topic, ferb_sanitize::Py::from(&data));
    }
    fn cancel_tasks(&self, ids: &BTreeSet<String>) {
        self.gw().cancel_tasks(ids)
    }
    fn interrupt_tasks(&self, ids: &BTreeSet<String>) {
        self.gw().interrupt_tasks(ids)
    }
    fn in_flight(&self, ids: &BTreeSet<String>) -> bool {
        self.gw().in_flight(ids)
    }
    fn task_record(&self, record: &LiveTask) {
        self.live_tasks
            .lock()
            .unwrap()
            .insert(record.task_id.clone(), record.clone());
        self.add_task(record);
    }
    fn task_record_span(&self, task_id: &str, span_id: &str) {
        let rec = {
            let mut live = self.live_tasks.lock().unwrap();
            let Some(r) = live.get_mut(task_id) else {
                return;
            };
            r.span_id = span_id.to_string();
            r.clone()
        };
        self.add_task(&rec);
    }
    fn task_record_removed(&self, task_id: &str) {
        self.live_tasks.lock().unwrap().remove(task_id);
        self.gw().remove_task(task_id);
    }
    fn approvals(&self) -> Vec<ferb_gateway::questions::Approval> {
        self.pending()
    }
    fn questions(&self) -> Option<Arc<dyn ferb_supervisor::flow::owner_wait::Questions>> {
        let q: Arc<dyn ferb_supervisor::flow::owner_wait::Questions> = self.own();
        Some(q)
    }
    fn subscribe(&self, topics: &[&str], sink: BusSink) -> Option<Unsubscribe> {
        let id = self.subscribers.fetch_add(1, Ordering::SeqCst);
        self.bus
            .lock()
            .unwrap()
            .push((id, topics.iter().map(|t| t.to_string()).collect(), sink));
        let me = self.me.clone();
        Some(Box::new(move || {
            if let Some(d) = me.upgrade() {
                d.bus.lock().unwrap().retain(|(i, _, _)| *i != id);
            }
        }))
    }
}

struct ScriptRuntime {
    daemon: Arc<TestDaemon>,
    n: usize,
    events: Value,
    /// The lines its tool calls wrote and read on the tool bridge (`mcp_shim`).
    bridge: Vec<Value>,
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
        token: &'a str,
    ) -> BoxFuture<'a, Result<StartedRun, ferb_worker::Error>> {
        Box::pin(async move {
            let parts: Vec<String> = request
                .parts
                .iter()
                .map(|p| self.daemon().run.mask(p))
                .collect();
            if std::env::var("REPLAY_PATHS").is_ok() {
                let run = &self.daemon().run;
                for (i, p) in request.parts.iter().enumerate() {
                    let r = p.matches(run.resolved.as_str()).count();
                    let u = p.matches(run.root.as_str()).count() - r;
                    if r + u > 0 {
                        eprintln!(
                            "call {} part {i}: {r} resolved, {u} as written: {}",
                            self.n + 1,
                            cut(&run.mask(p)).chars().take(300).collect::<String>()
                        );
                    }
                }
            }
            self.daemon().calls.lock().unwrap().started.push(
                json!({"n": self.n, "task_id": task.id,
                "system": request.system, "parts": parts, "cacheable": request.cacheable,
                "images": request.images.len()}),
            );
            let cancelled = Arc::new(AtomicBool::new(false));
            let woken = Arc::new(tokio::sync::Notify::new());
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
                    woken: woken.clone(),
                    daemon: self.daemon.clone(),
                    n: self.n,
                    token: token.to_string(),
                    bridge: Some(self.bridge.clone()),
                }),
                control: Arc::new(ScriptControl { cancelled, woken }),
            })
        })
    }
}

struct ScriptControl {
    cancelled: Arc<AtomicBool>,
    woken: Arc<tokio::sync::Notify>,
}

impl RunControl for ScriptControl {
    fn steer<'a>(&'a self, _message: &'a str) -> BoxFuture<'a, Result<(), ContinueError>> {
        Box::pin(async { Ok(()) })
    }
    fn cancel(&self) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            self.cancelled.store(true, Ordering::SeqCst);
            self.woken.notify_waiters();
        })
    }
}

struct ScriptEvents {
    events: Vec<Value>,
    at: usize,
    cancelled: Arc<AtomicBool>,
    woken: Arc<tokio::sync::Notify>,
    daemon: Arc<TestDaemon>,
    n: usize,
    token: String,
    /// The tool calls still to make, before the first event (the scripted model calls its tools first).
    bridge: Option<Vec<Value>>,
}

impl ScriptEvents {
    /// The scripted model's tool calls, through the tool bridge's path: the server checks the run token
    /// (`auth.hello` as `mcp_shim`, then before each call) and `tools.call` hands the call to the gateway with the
    /// token's grant (areas/chat.py:186). Each reply is compared with the line the Python daemon wrote.
    async fn call_tools(&self, lines: &[Value]) {
        let d = &self.daemon;
        let sent: Vec<Value> = lines
            .iter()
            .filter_map(|l| l.get("send").and_then(Value::as_str))
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        let recv: Vec<Value> = lines
            .iter()
            .filter_map(|l| l.get("recv").and_then(Value::as_str))
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        for msg in sent {
            // the scripted model's step runs in a thread (tests/conftest.py `anyio.to_thread.run_sync`) and each of
            // its calls is a socket round trip: the daemon's loop runs whatever is ready before and after each one
            tokio::task::yield_now().await;
            let params = &msg["params"];
            if msg["method"] == "auth.hello" {
                if params["token"] != json!(self.token) {
                    d.tool_diffs.lock().unwrap().push(format!(
                        "call {}: the bridge's token {} (here {})",
                        self.n + 1,
                        params["token"],
                        self.token
                    ));
                }
                continue;
            }
            let want = recv.iter().find(|r| r["id"] == msg["id"]).cloned();
            let grant = d.run_grants.lock().unwrap().get(&self.token).cloned();
            let got = match grant {
                None => json!({"error": "no run token"}),
                Some(grant) => {
                    let args = params["arguments"].as_object().cloned().unwrap_or_default();
                    let name = s(&params["name"]);
                    // `Gateway::call`'s future isn't Send (ferb-policy's `confirm::Facts` holds `&dyn Fn` across an
                    // await); the replay's runtime has one thread, where it is made, polled and dropped
                    let gateway = d.gw();
                    match OneThread(gateway.call(&grant, &name, &args)).await {
                        Ok(reply) => reply.to_json(),
                        Err(e) => json!({"error": format!("{e:?}")}),
                    }
                }
            };
            d.tool_calls.fetch_add(1, Ordering::SeqCst);
            tokio::task::yield_now().await;
            let got: Value = serde_json::from_str(&d.run.mask(&line(&got))).unwrap();
            // a call the daemon ended without a reply (a stop while it waited): the bridge saw its connection close
            let closed = lines.iter().any(|l| l.get("closed").is_some());
            let want = match want {
                Some(w) => w["result"].clone(),
                None if closed => json!({"error": "Cancelled"}),
                None => Value::Null,
            };
            if got != want {
                d.tool_diffs.lock().unwrap().push(format!(
                    "call {} {}\n    python: {}\n    rust:   {}",
                    self.n + 1,
                    params["name"],
                    cut(&line(&want)),
                    cut(&line(&got))
                ));
            }
        }
    }
}

/// The supervisor's own questions (a plan's ask step, a checker's ask, the check-in, a runtime switch): Python keeps
/// them in the gateway's one `state.approvals`; ferb-gateway's `Gateway` offers no way to add one, so they are a
/// second `Approvals` sharing the gateway's id counter, and the chat RPCs read both.
struct OwnQuestions {
    store: Mutex<ferb_gateway::questions::Approvals>,
}

fn asked_of(ap: &ferb_gateway::questions::Approval) -> ferb_supervisor::flow::owner_wait::Asked {
    ferb_supervisor::flow::owner_wait::Asked {
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

impl ferb_supervisor::flow::owner_wait::Questions for OwnQuestions {
    fn ask(&self, ask: &ferb_supervisor::flow::owner_wait::Ask) -> String {
        let mut store = self.store.lock().unwrap();
        let ap = store.question(
            &ferb_gateway::questions::Asker {
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
    fn get(&self, id: &str) -> Option<ferb_supervisor::flow::owner_wait::Asked> {
        self.store.lock().unwrap().get(id).map(asked_of)
    }
    fn supersede(&self, id: &str) -> bool {
        self.store.lock().unwrap().supersede(id).is_some()
    }
    fn pending(&self) -> Vec<ferb_supervisor::flow::owner_wait::Asked> {
        self.store
            .lock()
            .unwrap()
            .pending()
            .into_iter()
            .map(asked_of)
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

/// The chat RPCs' view of the scenario's gateway (and the supervisor's own questions, in asking order).
struct ChatGw(
    Arc<ferb_gateway::gateway::Gateway>,
    Arc<OwnQuestions>,
    Arc<AtomicBool>,
);

impl ChatGw {
    fn all_pending(&self) -> Vec<ferb_gateway::questions::Approval> {
        let mut all = self.0.pending();
        all.extend(self.1.store.lock().unwrap().pending().into_iter().cloned());
        all.sort_by(|a, b| a.id.cmp(&b.id));
        all
    }
}

impl ferb_supervisor::flow::chat_rpc::ChatGateway for ChatGw {
    fn pending(&self) -> Vec<ferb_gateway::questions::Approval> {
        self.all_pending()
    }
    fn approval(&self, id: &str) -> Option<ferb_gateway::questions::Approval> {
        self.0
            .approval(id)
            .or_else(|| self.1.store.lock().unwrap().get(id).cloned())
    }
    fn resolve(
        &self,
        id: &str,
        granted: bool,
        choice: Option<usize>,
        text: Option<&str>,
        hidden: &[String],
    ) -> Result<ferb_gateway::questions::Approval, ferb_gateway::questions::ResolveError> {
        if self.0.approval(id).is_some() {
            return self.0.resolve(id, granted, choice, text);
        }
        let mut store = self.1.store.lock().unwrap();
        let ap = store.resolve(id, granted, choice, text)?.clone();
        if let Some(held) = store.get_mut(id) {
            held.hidden = hidden.to_vec();
        }
        Ok(ap)
    }
    fn tools_for(&self, _grant: &ferb_rpc::auth::RunGrant) -> Vec<Value> {
        Vec::new()
    }
    fn call<'a>(
        &'a self,
        grant: &'a ferb_rpc::auth::RunGrant,
        name: &'a str,
        arguments: &'a Map<String, Value>,
    ) -> BoxFuture<'a, Result<Value, String>> {
        Box::pin(OneThread(async move {
            self.0
                .call(grant, name, arguments)
                .await
                .map(|r| r.to_json())
                .map_err(|e| format!("{e:?}"))
        }))
    }
    fn stopped(&self) -> bool {
        self.2.load(Ordering::SeqCst)
    }
}

/// A future polled only on the thread that made it: the replay's runtime is current-thread, so every task (and
/// everything it awaits) is created, polled and dropped on that one thread.
struct OneThread<F>(F);

// SAFETY: the replay builds a current-thread runtime (`new_current_thread`) and never hands a task to another
// thread, so a `OneThread` never leaves the thread it was made on; `Send` is only what the runtime's task type asks.
unsafe impl<F> Send for OneThread<F> {}

impl<F: std::future::Future> std::future::Future for OneThread<F> {
    type Output = F::Output;
    fn poll(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<F::Output> {
        // SAFETY: the inner future is never moved out of the pinned wrapper (structural pinning of field 0).
        unsafe { self.map_unchecked_mut(|s| &mut s.0) }.poll(cx)
    }
}

impl EventSource for ScriptEvents {
    fn next(&mut self) -> BoxFuture<'_, Option<Result<RuntimeEvent, ferb_worker::Error>>> {
        Box::pin(async move {
            // as tests/conftest.py's FakeRuntime: a cancel ends only a call that never answers (STALL); a scripted
            // answer already under way still comes
            if self.at == 0 && self.daemon.gated.contains(&self.n) {
                // a call the recorder held: it answers at `release_model`, or never when FERB cancels it first
                loop {
                    let opened = self.daemon.released_now.notified();
                    let woken = self.woken.notified();
                    tokio::pin!(opened, woken);
                    opened.as_mut().enable();
                    woken.as_mut().enable();
                    if self.daemon.released.load(Ordering::SeqCst) {
                        break;
                    }
                    if self.cancelled.load(Ordering::SeqCst) {
                        return None;
                    }
                    tokio::select! {
                        _ = opened => {}
                        _ = woken => {}
                    }
                }
            }
            if let Some(lines) = self.bridge.take() {
                self.call_tools(&lines).await;
            }
            if self.at + 1 == self.events.len() {
                // the final answer: a step the recorder took at this moment (a file the owner wrote) comes with it
                self.daemon.answered.lock().unwrap().push(self.n);
                let first_check = self
                    .daemon
                    .recorded
                    .iter()
                    .position(|m| m["key"] == "worker.verify");
                if first_check == Some(self.n) {
                    self.daemon.apply_note();
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
                // "(no answer: FERB cancelled or left the call)": it answers nothing until FERB cancels it
                loop {
                    let woken = self.woken.notified();
                    tokio::pin!(woken);
                    woken.as_mut().enable();
                    if self.cancelled.load(Ordering::SeqCst) {
                        return None;
                    }
                    woken.await;
                }
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
    /// The daemon's own lines (its other RPC areas, its notifications, the timer loop): passed over, not compared.
    skipped: Vec<String>,
}

impl Report {
    fn full(&self) -> bool {
        self.stopped.is_none()
            && self.skipped.is_empty()
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

fn dump(db: &Db, run: &RunRoot) -> BTreeMap<String, (Vec<String>, Vec<Vec<Value>>)> {
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
                        Value::String(t) => scrub(&json!(unhash(&run.mask(&t)))),
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
    run: &Arc<RunRoot>,
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
        let id = s(&row[1]);
        let n: usize = id
            .strip_prefix("id-")
            .and_then(|n| n.parse().ok())
            .unwrap_or(usize::MAX);
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
    let ids = Arc::new(Ids::new("id", first_request - 1));
    let nn = format!("{:02}", index + 1);
    let root = run.root.clone();
    let ferb_home = format!("{root}/{nn}/home");
    let workspace = format!("{ferb_home}/workspace");
    let home = format!("{root}/{nn}/user");
    std::fs::create_dir_all(&workspace).unwrap();
    std::fs::create_dir_all(&home).unwrap();
    let clock = Arc::new(
        FakeClock::new(ferb_clock::py::parse("2026-01-01T09:00:00+00:00")).with_zone(Zone::utc()),
    );
    let redactor = Arc::new(ferb_sanitize::Redactor::new(&[], &[]));
    let changes = Arc::new(ferb_fs::changes::Changes::new(
        &format!("{ferb_home}/changes"),
        None,
        &home,
    ));
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
    // the calls held until `release_model`: each the call the model wait before it named
    let mut gated: BTreeSet<usize> = BTreeSet::new();
    let mut last_wait: Option<usize> = None;
    for st in sc["steps"].as_array().unwrap() {
        match st["do"].as_str().unwrap() {
            "wait" => {
                last_wait = s(&st["until"])
                    .strip_prefix("the model has received call ")
                    .and_then(|r| r.rsplit_once(" ("))
                    .and_then(|(key, n)| {
                        let key = key.trim_matches('\'');
                        let n: usize = n.trim_end_matches(')').parse().ok()?;
                        recorded
                            .iter()
                            .enumerate()
                            .filter(|(_, m)| m["key"] == key)
                            .nth(n - 1)
                            .map(|(i, _)| i)
                    })
            }
            "release_model" => {
                if let Some(i) = last_wait {
                    gated.insert(i);
                }
            }
            _ => {}
        }
    }
    let question_ids = Arc::new(AtomicUsize::new(0));
    let new_own = {
        let question_ids = question_ids.clone();
        move || {
            Arc::new(OwnQuestions {
                store: Mutex::new(ferb_gateway::questions::Approvals::new({
                    let ids = question_ids.clone();
                    move || format!("ap-a{:011}", ids.fetch_add(1, Ordering::SeqCst) + 1)
                })),
            })
        }
    };
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
        run: run.clone(),
        gateway: Mutex::new(None),
        changes: changes.clone(),
        bus: Mutex::default(),
        subscribers: AtomicUsize::new(0),
        run_grants: Mutex::default(),
        live_tasks: Mutex::default(),
        answered: Mutex::default(),
        note: Mutex::new(note_of(sc, &workspace)),
        tool_diffs: Mutex::default(),
        tool_calls: AtomicUsize::new(0),
        redactor: redactor.clone(),
        clock: clock.clone(),
        sup: Mutex::new(None),
        asked_kinds: Mutex::default(),
        answered_ids: Mutex::default(),
        own: Mutex::new(new_own()),
        spans_used: Mutex::default(),
        watching: Mutex::default(),
        side: Mutex::new(None),
        unattended: Mutex::default(),
        stopped: Arc::new(AtomicBool::new(false)),
        workspace: workspace.clone(),
        user_home: home.clone(),
        gated: gated.clone(),
        released: Arc::new(AtomicBool::new(false)),
        released_now: Arc::new(tokio::sync::Notify::new()),
    });
    let waiting = Arc::new(AtomicBool::new(false));
    // the daemon's composition (main.py `start`): its gateway on the store, the supervisor, the chat RPCs; a restart
    // makes them again on the same store
    let start = || {
        let bus = Arc::downgrade(&daemon);
        let gateway = Arc::new(gw::build(gw::Parts {
            db: db.clone(),
            clock: clock.clone(),
            ids: ids.clone(),
            redactor: redactor.clone(),
            publish: Arc::new(move |topic: &str, data: ferb_sanitize::Py| {
                // the daemon's events (ferb/daemon/events.py) redact each event once, FERB's own marked words kept
                if let Some(d) = bus.upgrade() {
                    d.events(topic, data);
                }
            }),
            home: ferb_home.clone(),
            user: home.clone(),
            changes: changes.clone(),
            calls: sc["gateway_calls"].as_array().cloned().unwrap_or_default(),
            real_root: run.resolved.clone(),
            device_id: "unknown-device".into(),
            wait: gw::Wait {
                clock: clock.clone(),
                status: {
                    let d = Arc::downgrade(&daemon);
                    Arc::new(
                        move |worker: &str,
                              task: &str,
                              until: Option<&ferb_clock::py::Time>,
                              line: Option<&str>| {
                            if let Some(sup) = d.upgrade().and_then(|d| d.sup()) {
                                sup.wait_status(worker, task, until, line);
                            }
                        },
                    )
                },
                running: waiting.clone(),
            },
            question_ids: question_ids.clone(),
        }));
        *daemon.gateway.lock().unwrap() = Some(gateway.clone());
        let sup = Supervisor::new(Deps {
            db: db.clone(),
            clock: clock.clone(),
            ids: ids.clone(),
            redactor: redactor.clone(),
            policy: Arc::new(TestPolicy),
            daemon: daemon.clone(),
            prompts: root_prompts(),
            tokenizer: Tokenizer::Utf8Bytes,
            device_id: "unknown-device".into(),
            workspace: workspace.clone(),
            home: home.clone(),
            wrap: Arc::new(|t: &str| ferb_sanitize::untrusted::wrap_untrusted_with_id(t, WRAP_ID)),
            new_chat: Arc::new(|| "c00000000001".into()),
            stall_s: 180.0,
            heartbeat_poll_s: 5.0,
        });
        // supervisor.py:1223 `gateway.attach(_Watch(self), ...)`
        gateway.attach(sup.watch());
        *daemon.sup.lock().unwrap() = Some(sup.clone());
        let rpc = ferb_supervisor::flow::chat_rpc::ChatRpc::new(
            ferb_supervisor::flow::chat_rpc::ChatDeps {
                supervisor: sup.clone(),
                gateway: Arc::new(ChatGw(
                    gateway.clone(),
                    daemon.own(),
                    daemon.stopped.clone(),
                )),
                persona: None,
                prepare_persona: Arc::new(|| {}),
                notices: Some(Arc::new(Vec::new)),
                conversations: Arc::new(Vec::new),
                sensing_paused: Arc::new(|| false),
                shutting_down: Arc::new(|| false),
                question_answered: Arc::new(|_ap| String::new()),
                ask_input_monitoring: Arc::new(|| {}),
                click_through: None,
            },
        );
        (sup, rpc)
    };
    let (mut sup, mut rpc) = start();
    let session = ferb_rpc::server::Session {
        role: ferb_rpc::auth::Role::Cli,
        grant: None,
        pid: None,
        token: None,
        app: None,
    };
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .start_paused(true)
        .build()
        .unwrap();
    rt.block_on(async {
        let mut streams: HashMap<String, (u64, Result<tokio::sync::mpsc::UnboundedReceiver<Value>, String>)> =
            HashMap::new();
        // the daemon's group A when the scenario reaches it (its RPCs, its timer loop)
        let reaches_side = sc["steps"].as_array().unwrap().iter().any(|st| {
            (st["do"] == "timers_tick" && st["fired"] != 0)
                || st["lines"].as_array().is_some_and(|ls| {
                    ls.iter().filter_map(|l| l.get("send").and_then(Value::as_str)).any(|l| {
                        serde_json::from_str::<Value>(l).is_ok_and(|v| {
                            ferb_daemon::areas_a::METHODS.contains(&v["method"].as_str().unwrap_or(""))
                        })
                    })
                })
        });
        if reaches_side {
            let first_token = tables["timers"]["rows"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|r| r[8].as_str())
                .filter_map(|t| t.trim_start_matches('t').parse::<u64>().ok())
                .min()
                .unwrap_or(1);
            let bus = Arc::downgrade(&daemon);
            let side = daemon_side::DaemonSide::start(daemon_side::Parts {
                home: ferb_home.clone().into(),
                user: home.clone().into(),
                db_path: dir.path().join("ferb.db"),
                repo: repo(),
                clock: clock.clone(),
                ids: ids.clone(),
                redactor: redactor.clone(),
                publish: Arc::new(move |topic: &str, data: Map<String, Value>| {
                    if let Some(d) = bus.upgrade() {
                        d.events(topic, ferb_sanitize::Py::from(&Value::Object(data)));
                    }
                }),
                first_token,
                supervisor: sup.clone(),
            });
            *daemon.side.lock().unwrap() = Some(Arc::new(side));
        }
        let side = || daemon.side.lock().unwrap().clone();
        let mut notif_conns: BTreeSet<String> = BTreeSet::new();
        let mut daemon_conns: BTreeSet<String> = BTreeSet::new();
        for st in sc["steps"].as_array().unwrap() {
            let conn = s(&st["conn"]);
            match st["do"].as_str().unwrap() {
                "connect" | "close" => {}
                // the owner's file was written when its moment came (the first checker's answer); else now
                "note" => daemon.apply_note(),
                "call" => {
                    let lines = st["lines"].as_array().unwrap();
                    for l in lines {
                        let Some(sent) = l.get("send") else { continue };
                        let v: Value = serde_json::from_str(sent.as_str().unwrap()).unwrap();
                        let method = s(&v["method"]);
                        if method == "auth.hello" {
                            continue;
                        }
                        if let (true, Some(side)) =
                            (ferb_daemon::areas_a::METHODS.contains(&method.as_str()), side())
                        {
                            let id = v["id"].clone();
                            let got = run.mask(&side.call(&conn, sent.as_str().unwrap(), &id));
                            let want = lines
                                .iter()
                                .filter_map(|l| l.get("recv"))
                                .map(|r| fixed(&name, &s(&resolve(r))))
                                .find(|r| serde_json::from_str::<Value>(r).is_ok_and(|x| x["id"] == id))
                                .unwrap_or_default();
                            report.checked += 1;
                            if got != want {
                                report.frames.push(format!(
                                    "{conn} {method}\n    python: {}\n    rust:   {}",
                                    cut(&want),
                                    cut(&got)
                                ));
                            }
                            settle(&db).await;
                            continue;
                        }
                        if method == "stop.engage" || method == "stop.lift" {
                            // areas/rpc.py `_stop_engage` / `_stop_lift`: the gateway's stop order (ferb-gateway
                            // `stop`: block, revoke every run token, the hooks "runtimes" = the supervisor's
                            // `cancel_all` and "identity_browser"), journaled and published
                            let result = if method == "stop.engage" {
                                let gateway = daemon.gw();
                                let grants = daemon.clone();
                                let mut control = ferb_gateway::stop::StopControl::new(
                                    move || {
                                        let mut held = grants.run_grants.lock().unwrap();
                                        let n = held.len();
                                        held.clear();
                                        grants.grants.lock().unwrap().clear();
                                        n
                                    },
                                    move |on| gateway.set_stopped(on),
                                );
                                daemon.stopped.store(true, Ordering::SeqCst);
                                let hook = sup.clone();
                                control.register(
                                    "runtimes",
                                    Box::new(move || {
                                        hook.cancel_all();
                                        Ok(())
                                    }),
                                );
                                control.register("identity_browser", Box::new(|| Ok(())));
                                let summary = Value::Object(control.engage());
                                sup.inner().journal("stop.engaged", summary.clone(), None);
                                daemon.publish("stop.engaged", summary.clone());
                                summary
                            } else {
                                daemon.stopped.store(false, Ordering::SeqCst);
                                daemon.gw().set_stopped(false);
                                sup.inner().journal("stop.lifted", json!({}), None);
                                daemon.publish("stop.lifted", json!({}));
                                json!({"stopped": false})
                            };
                            let id = v["id"].clone();
                            let got = line(&json!({"jsonrpc": "2.0", "id": id, "result": result}));
                            let want = lines
                                .iter()
                                .filter_map(|l| l.get("recv"))
                                .map(|r| fixed(&name, &s(&resolve(r))))
                                .find(|r| serde_json::from_str::<Value>(r).is_ok_and(|x| x["id"] == id))
                                .unwrap_or_default();
                            report.checked += 1;
                            if got != want {
                                report.frames.push(format!(
                                    "{conn} {method}\n    python: {}\n    rust:   {}",
                                    cut(&want),
                                    cut(&got)
                                ));
                            }
                            for _ in 0..100 {
                                tokio::task::yield_now().await;
                            }
                            continue;
                        }
                        if !CHAT_RPCS.contains(&method.as_str()) {
                            report.skipped.push(format!("the call {method}"));
                            continue;
                        }
                        let params = v["params"].as_object().cloned().unwrap_or_default();
                        let got = rpc.unary(&method, &session, params).await;
                        let id = v["id"].clone();
                        let got = match got {
                            Ok(result) => json!({"jsonrpc": "2.0", "id": id, "result": result}),
                            Err(e) if e.failure == ferb_rpc::server::Failure::Refusal => json!({"jsonrpc": "2.0",
                                "id": id, "error": {"code": -32004, "message": e.message}}),
                            Err(e) => json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32603,
                                "message": e.type_name}}),
                        };
                        let got = run.mask(&line(&got));
                        let want = lines
                            .iter()
                            .filter_map(|l| l.get("recv"))
                            .map(|r| fixed(&name, &s(&resolve(r))))
                            .find(|r| serde_json::from_str::<Value>(r).is_ok_and(|x| x["id"] == id))
                            .unwrap_or_default();
                        report.checked += 1;
                        if got != want {
                            report.frames.push(format!(
                                "{conn} {method}\n    python: {}\n    rust:   {}",
                                cut(&want),
                                cut(&got)
                            ));
                        }
                        for _ in 0..100 {
                            tokio::task::yield_now().await;
                        }
                    }
                    if report.stopped.is_some() {
                        break;
                    }
                }
                "stream" => {
                    let sent: Value =
                        serde_json::from_str(st["lines"][0]["send"].as_str().unwrap()).unwrap();
                    if sent["method"] == "notifications.subscribe" && side().is_some() {
                        notif_conns.insert(conn.clone());
                        continue;
                    }
                    if sent["method"] != "chat.send" {
                        report.skipped.push(format!("the stream {}", sent["method"]));
                        daemon_conns.insert(conn.clone());
                        continue;
                    }
                    let id = sent["id"].as_u64().unwrap();
                    let got = rpc
                        .chat_send(sent["params"].as_object().unwrap())
                        .map_err(|e| e.message);
                    streams.insert(conn, (id, got));
                    tokio::task::yield_now().await;
                }
                "restart" => {
                    // main.py `shutdown`: the supervisor suspends its trees and every turn and run is drained
                    sup.shut_down().await;
                    // the desktop observer's shutdown: the summary of each task it still watched
                    let open: Vec<String> = std::mem::take(&mut *daemon.watching.lock().unwrap())
                        .into_iter()
                        .collect();
                    for tree in open {
                        daemon.write_span("disturbance.summary", &tree);
                    }
                    sup.inner().journal("daemon.stopped", json!({"pid": "<volatile>"}), None);
                    // a new daemon on the same home: its in-memory state starts empty, ids under the new prefix
                    let prefix = s(&st["ids_prefix"]);
                    daemon.restart();
                    *daemon.own.lock().unwrap() = new_own();
                    ids.restart(&prefix);
                    (sup, rpc) = start();
                    // main.py's start: what was running is interrupted, then the daemon's own rows (as recorded),
                    // the ledger's unknown outcomes, then the saved plans resume
                    sup.recover_after_restart();
                    for row in tables["journal"]["rows"].as_array().unwrap() {
                        let kind = s(&row[3]);
                        if s(&row[1]).starts_with(&format!("{prefix}-"))
                            && ["daemon.started", "runtime.limits_unknown"].contains(&kind.as_str())
                        {
                            let data: Value = serde_json::from_str(&s(&cell(&row[4]))).unwrap();
                            sup.inner().journal(&kind, data, None);
                        }
                    }
                    sup.mark_unknown_after_restart();
                    sup.start_resume();
                    for _ in 0..100 {
                        tokio::task::yield_now().await;
                    }
                }
                "stream_end" | "stream_end_at_shutdown" => {
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
                                let ev: Value = serde_json::from_str(&run.mask(&line(&ev))).unwrap();
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
                        .map(|l| fixed(&name, &s(&resolve(&l["recv"]))))
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
                "wait" if s(&st["until"]).starts_with("an approval is pending") => {
                    for _ in 0..10_000 {
                        if !daemon.pending().is_empty() {
                            break;
                        }
                        tokio::task::yield_now().await;
                    }
                    if daemon.pending().is_empty() {
                        report.stopped = Some(format!("a wait for {} (none was asked)", s(&st["until"])));
                        break;
                    }
                }
                "release_model" => {
                    daemon.released.store(true, Ordering::SeqCst);
                    daemon.released_now.notify_waiters();
                    for _ in 0..100 {
                        tokio::task::yield_now().await;
                    }
                }
                "wait" if st["until"] == "the task is paused" || st["until"] == "the task ended" => {
                    let want_paused = st["until"] == "the task is paused";
                    let status = || -> Option<String> {
                        db.lock()
                            .unwrap()
                            .conn()
                            .query_row(
                                "SELECT status FROM tasks WHERE parent_id IS NULL ORDER BY rowid DESC LIMIT 1",
                                [],
                                |r| r.get(0),
                            )
                            .ok()
                    };
                    let reached = |st: Option<String>| match st.as_deref() {
                        Some("paused") => want_paused,
                        Some("running") | Some("planned") | None => false,
                        Some(_) => !want_paused,
                    };
                    for i in 0..10_000 {
                        if reached(status()) {
                            break;
                        }
                        // a wait for the owner looks at its question every `hold_poll_s` on tokio's clock, which
                        // moves only while every task is idle
                        if i % 100 == 99 {
                            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                        } else {
                            tokio::task::yield_now().await;
                        }
                    }
                    if !reached(status()) {
                        report.stopped = Some(format!("a wait for {} (never true here)", s(&st["until"])));
                        break;
                    }
                }
                "wait" if st["until"] == "the worker's wait.for is running" => {
                    for _ in 0..10_000 {
                        if waiting.load(Ordering::SeqCst) {
                            break;
                        }
                        tokio::task::yield_now().await;
                    }
                }
                "clock" => {
                    clock.advance(st["advance_s"].as_f64().unwrap());
                    for _ in 0..100 {
                        tokio::task::yield_now().await;
                    }
                }
                // one pass of the timer loop: no timer is due in a scenario this port replays (the step records none)
                "timers_tick" if st["fired"] == 0 => {}
                // one pass of the daemon's timer loop (group A's), its timers handed over
                "timers_tick" if side().is_some() => {
                    let fired = side().unwrap().tick();
                    report.checked += 1;
                    if json!(fired) != st["fired"] {
                        report.frames.push(format!("timers_tick fired {fired}, recorded {}", st["fired"]));
                    }
                    settle(&db).await;
                }
                "timers_tick" => report.skipped.push("a timer firing".into()),
                "stream_cut" if notif_conns.contains(&conn) => {
                    let want: Vec<String> = st["lines"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .filter_map(|l| l.get("recv"))
                        .map(|r| fixed(&name, &s(&resolve(r))))
                        .collect();
                    let got: Vec<String> = side()
                        .unwrap()
                        .notes
                        .lock()
                        .unwrap()
                        .iter()
                        .map(|l| run.mask(l))
                        .collect();
                    report.checked += want.len();
                    if got != want {
                        report.frames.push(format!(
                            "{conn}\n    python: {want:?}\n    rust:   {got:?}"
                        ));
                    }
                }
                "stream_cut" if daemon_conns.contains(&conn) => {}
                "wait" if st["until"] == "the first checker has answered" => {
                    let first = recorded.iter().position(|m| m["key"] == "worker.verify");
                    for _ in 0..10_000 {
                        if first.is_some_and(|f| daemon.answered.lock().unwrap().contains(&f)) {
                            break;
                        }
                        tokio::task::yield_now().await;
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
                    // the nth call of that key, by its place in the recording
                    let key = until
                        .strip_prefix("the model has received call ")
                        .and_then(|r| r.rsplit_once(" ("))
                        .map(|(k, _)| k.trim_matches('\'').to_string())
                        .unwrap_or_default();
                    let at = recorded
                        .iter()
                        .enumerate()
                        .filter(|(_, m)| m["key"] == json!(key))
                        .nth(n.saturating_sub(1))
                        .map(|(i, _)| i);
                    for _ in 0..10_000 {
                        let started = daemon.calls.lock().unwrap().started.iter().any(|c| {
                            at.is_some_and(|i| c["n"] == json!(i))
                        });
                        if started || (at.is_none() && daemon.calls.lock().unwrap().started.len() >= n) {
                            break;
                        }
                        tokio::task::yield_now().await;
                        if side().is_some() {
                            // a scheduled run starts on the daemon's loop thread
                            std::thread::sleep(std::time::Duration::from_micros(200));
                        }
                    }
                    if side().is_some() {
                        settle(&db).await;
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
        if side().is_some() {
            settle(&db).await;
        }
        // the daemon's side stops with the scenario (its loop thread and server)
        daemon.side.lock().unwrap().take();
    });
    if let Some(n) = daemon.unsupported.lock().unwrap().first() {
        report.stopped.get_or_insert(n.clone());
    }
    // the tool calls through the bridge: every recorded one made, each reply as the Python gateway gave it
    let bridged: usize = recorded
        .iter()
        .flat_map(|m| {
            m["tool_bridge_lines"]
                .as_array()
                .cloned()
                .unwrap_or_default()
        })
        .filter(|l| {
            l.get("send")
                .and_then(Value::as_str)
                .is_some_and(|t| t.contains("\"tools.call\""))
        })
        .count();
    report.checked += bridged;
    report
        .calls
        .extend(daemon.tool_diffs.lock().unwrap().iter().cloned());
    let made = daemon.tool_calls.load(Ordering::SeqCst);
    if made != bridged {
        report
            .calls
            .push(format!("{made} tool calls made, {bridged} recorded"));
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
        let mut st = db
            .conn()
            .prepare("SELECT id, target_json, payload_hash FROM action_ledger ORDER BY rowid")
            .unwrap();
        let rows: Vec<String> = st
            .query_map([], |r| {
                Ok(format!(
                    "{} {} {}",
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?
                ))
            })
            .unwrap()
            .map(Result::unwrap)
            .collect();
        eprintln!("LEDGER\n{}", rows.join("\n"));
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
        let parts: Vec<String> = strs(&want["parts"]).iter().map(|p| unhash(p)).collect();
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
                "parts": strs(&st["parts"]).iter().map(|p| unhash(p)).collect::<Vec<_>>(),
                "cacheable": st["cacheable"], "images": st["images"]})
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
    let mine = dump(&db.lock().unwrap(), run);
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
                        Value::String(t) => json!(fixed(&name, &unhash(&t))),
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

/// The deliberate differences from Python (ENG-6), applied to what the recording expects before it is compared:
/// (scenario, recorded text, what the port says instead).
const FIXES: [(&str, &str, &str); 1] = [
    // GATE-581: Python ends a task the global stop cut with "(from the task) FERB was stopped", marking FERB's own words
    // as the task's; the port says "FERB was stopped" (the outcome frame, the task row, the span, the journal)
    (
        "global_stop",
        "(from the task) FERB was stopped",
        "FERB was stopped",
    ),
];

fn fixed(scenario: &str, text: &str) -> String {
    FIXES
        .iter()
        .filter(|(n, _, _)| *n == scenario)
        .fold(text.to_string(), |t, (_, a, b)| t.replace(a, b))
}

/// Until FERB is quiet (the recorder's `settle`): no new journal row or span, here or on the daemon's loop thread,
/// for a while; the replay's runtime runs meanwhile.
async fn settle(db: &Arc<Mutex<Db>>) {
    let mark = || -> (i64, i64) {
        let db = db.lock().unwrap();
        let q = |sql: &str| {
            db.conn()
                .query_row(sql, [], |r| r.get::<_, i64>(0))
                .unwrap_or(0)
        };
        (
            q("SELECT COALESCE(MAX(seq), 0) FROM journal"),
            q("SELECT COUNT(*) FROM spans"),
        )
    };
    let mut last = mark();
    let mut quiet = 0;
    for _ in 0..2000 {
        for _ in 0..50 {
            tokio::task::yield_now().await;
        }
        std::thread::sleep(std::time::Duration::from_millis(1));
        let now = mark();
        if now == last {
            quiet += 1;
            if quiet >= 25 {
                return;
            }
        } else {
            quiet = 0;
            last = now;
        }
    }
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

/// The scenarios that replay fully: the plain reply, the clarifying question and its answer, the one-step task, a
/// task's tool calls through the real gateway (a file written and read back, the change record its checker reads or
/// skips, a failed call and the retry, a wait in code while the clock moves), the two-step plan, the plan sent back for
/// repair, a protected file's confirmation answered Yes, No or left pending under a stop, the checker's rejection, its
/// findings with the fix turn, a plan step's question answered in chat, the check-in after five attempts, pause and
/// steer and resume, a correction delivered into a running call, the stops (by task.stop, by chat, with a new task), the
/// merged message and the attachment with chat.send's refusals. Control and recovery's paths are the other port's
/// (control.rs, owner_wait.rs), run here through the real gateway.
const FULL: [&str; 28] = [
    "plain_reply",
    "clarify_then_answer",
    "one_step_task",
    "one_step_with_tools",
    "checker_skips_change_records",
    "multi_step_plan",
    "confirm_yes",
    "confirm_no",
    "confirm_stop_pending",
    "pause_steer_resume",
    "steer_running_task",
    "stop_by_task_stop",
    "stop_by_chat",
    "stop_plus_new_task",
    "second_message_merged",
    "wait_for_time",
    "tool_failure_then_retry",
    "plan_repair",
    "checker_rejects",
    "checker_findings_fix",
    "worker_question_answered",
    "check_in_after_five",
    "attachment_and_bad_params",
    "restart_mid_task",
    "restart_with_open_question",
    "timer_set_and_fired",
    "schedule_created_and_fired",
    "global_stop",
];

#[test]
fn the_recorded_chat_scenarios() {
    let chat = read("rust/fixtures/daemon/chat_scenarios.json");
    let (_folder, run) = RunRoot::new(recorded_root_len(&chat));
    let run = Arc::new(run);
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
        let r = run_scenario(sc, records, i, &run, &tools, &schemas);
        checked += r.checked;
        let verdict = if r.full() {
            "replays fully".to_string()
        } else {
            let mut why = Vec::new();
            if let Some(stop) = &r.stopped {
                why.push(format!("stops at {stop}"));
            }
            if !r.skipped.is_empty() {
                why.push(format!(
                    "the daemon's own lines not compared ({})",
                    r.skipped.join(", ")
                ));
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

/// A `note` step's file: "the owner writes workspace/<path> (<its text, JSON-quoted>) …" → (its path here, text).
fn note_of(sc: &Value, workspace: &str) -> Option<(String, String)> {
    let text = sc["steps"]
        .as_array()?
        .iter()
        .find(|st| st["do"] == "note")
        .map(|st| s(&st["text"]))?;
    let rest = text.split_once("writes workspace/")?.1;
    let (path, rest) = rest.split_once(" (")?;
    let end = rest.rfind(')')?;
    let body: String = serde_json::from_str(&rest[..end]).ok()?;
    Some((format!("{workspace}/{path}"), body))
}

/// The unary methods areas/chat.py answers (ported in ferb-supervisor's `chat_rpc`); every other is the daemon's.
const CHAT_RPCS: [&str; 13] = [
    "chat.status",
    "tasks.list",
    "results.ack",
    "chat.approve",
    "chat.answer",
    "chat.correct",
    "chat.attach",
    "task.stop",
    "tools.list",
    "tools.call",
    "conversations.list",
    "conversations.change",
    "ui.click_through",
];

/// A SHA-256 in hex, written `<sha256>`. FERB hashes real paths into a few records (P19's payload hash, a protected
/// path's confirmation key) and this run's folder can't be the recorder's (a recorder may be running in it), so those
/// hashes are compared as hashes only; every other byte around them is compared.
fn unhash(text: &str) -> String {
    static HEX: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    HEX.get_or_init(|| regex::Regex::new("[0-9a-f]{64}").unwrap())
        .replace_all(text, "<sha256>")
        .into_owned()
}
