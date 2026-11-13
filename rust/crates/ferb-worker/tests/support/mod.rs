//! The replay tests' fakes: the same recording seams rust/fixtures/worker/make.py gives the Python worker, logging
//! each call in the same form, and the scripted runtime (tests/conftest.py's `FakeRuntime` with scripted steps).
#![allow(dead_code)]

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};

use ferb_clock::clock::{Clock, FakeClock, ZonedClock};
use ferb_clock::py::Time;
use ferb_context::assemble::{Assembly, ContextBlock};
use ferb_context::pyvalue::{self, Py};
use ferb_core::errors::FerbError;
use ferb_core::types::{
    Identity, ModelImage, ModelLimits, ModelRequest, RunHandle, RuntimeEvent, RuntimeEventKind,
    Task, TaskRole,
};
use ferb_orchestrator::guards::Activity;
use ferb_orchestrator::ports::Policy;
use ferb_orchestrator::steering::{ContinueError, Steering};
use ferb_worker::errors::{
    AnswerMissing, MissingKind, OsError, RuntimeViolation, ServiceError, ServiceKind,
};
use ferb_worker::ports::{
    BoxFuture, EventSource, Grant, Liveness, ModelRuntime, RunControl, StartedRun, TaskRecord,
    WorkerHost, WorkerTree,
};
use ferb_worker::{deliver, Error, Worker};
use serde_json::{json, Map, Value};

pub fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

fn read(path: &Path) -> Value {
    let text = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    serde_json::from_str(&text).expect("fixture JSON")
}

/// A fixture file with its `{"sha256": ...}` body references resolved.
pub fn fixture(folder: &str, name: &str) -> Value {
    let dir = repo().join("rust/fixtures").join(folder);
    let bodies = if dir.join("bodies.json").exists() {
        read(&dir.join("bodies.json"))
    } else {
        Value::Null
    };
    resolve(&read(&dir.join(name)), &bodies)
}

pub fn resolve(v: &Value, bodies: &Value) -> Value {
    match v {
        Value::Object(m) if m.len() == 1 && m.contains_key("sha256") => {
            let key = m["sha256"].as_str().unwrap_or_default();
            bodies
                .get(key)
                .cloned()
                .unwrap_or_else(|| panic!("no body {key}"))
        }
        Value::Object(m) => Value::Object(
            m.iter()
                .map(|(k, x)| (k.clone(), resolve(x, bodies)))
                .collect(),
        ),
        Value::Array(a) => Value::Array(a.iter().map(|x| resolve(x, bodies)).collect()),
        other => other.clone(),
    }
}

/// ferb/policy, through the policy part's port (as ferb-orchestrator's tests answer it).
pub struct TestPolicy;

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

/// make.py's redactor: any word starting "HIDDEN-" is masked.
pub fn redact_text(text: &str) -> String {
    text.split(' ')
        .map(|w| {
            if w.starts_with("HIDDEN-") {
                "[redacted]"
            } else {
                w
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn redact(v: &Value) -> Value {
    match v {
        Value::String(s) => Value::String(redact_text(s)),
        Value::Object(m) => Value::Object(m.iter().map(|(k, x)| (k.clone(), redact(x))).collect()),
        Value::Array(a) => Value::Array(a.iter().map(redact).collect()),
        other => other.clone(),
    }
}

/// make.py `jsonable` of a float.
pub fn float(f: f64) -> Value {
    if f.is_finite() {
        json!(f)
    } else {
        json!({"float": ferb_py::json::python_float(f)})
    }
}

pub fn time(s: &str) -> Time {
    ferb_clock::py::parse(s).unwrap_or_else(|| panic!("time {s}"))
}

pub fn task_json(t: &Task) -> Value {
    json!({"id": t.id, "parent_id": t.parent_id, "goal": t.goal, "device_id": t.device_id,
           "identity": t.identity.as_str(), "scopes": t.scopes.iter().collect::<Vec<_>>(), "role": t.role.as_str(),
           "request_id": t.request_id, "success_conditions": t.success_conditions,
           "deadline": t.deadline.as_ref().map(|d| ferb_clock::py::isoformat(d, ferb_clock::py::Spec::Auto))})
}

pub fn task_from(v: &Value) -> Task {
    let s = |k: &str| v[k].as_str().unwrap_or_default().to_string();
    Task {
        id: s("id"),
        parent_id: v["parent_id"].as_str().map(String::from),
        goal: s("goal"),
        device_id: s("device_id"),
        identity: if s("identity") == "ferb" {
            Identity::Ferb
        } else {
            Identity::Owner
        },
        scopes: v["scopes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|x| x.as_str().unwrap().to_string())
            .collect(),
        role: match s("role").as_str() {
            "worker" => TaskRole::Worker,
            "verifier" => TaskRole::Verifier,
            _ => TaskRole::Mixed,
        },
        request_id: s("request_id"),
        success_conditions: v["success_conditions"]
            .as_array()
            .unwrap()
            .iter()
            .map(|x| x.as_str().unwrap().to_string())
            .collect(),
        deadline: v["deadline"].as_str().map(time),
    }
}

pub fn block_from(v: &Value) -> ContextBlock {
    let s = |k: &str| {
        v.get(k)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    let b = |k: &str| v.get(k).and_then(Value::as_bool).unwrap_or(false);
    ContextBlock {
        text: s("text"),
        reference: s("reference"),
        protected: b("protected"),
        cache: b("cache"),
        priority: v.get("priority").and_then(Value::as_i64).unwrap_or(0),
        origin: s("origin"),
        source_refs: v
            .get("source_refs")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .map(|x| x.as_str().unwrap_or_default().to_string())
                    .collect()
            })
            .unwrap_or_default(),
        payload: v.get("payload").and_then(Value::as_str).map(String::from),
        history: b("history"),
        fold: s("fold"),
        values: v.get("values").filter(|x| !x.is_null()).cloned(),
    }
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn unhex(text: &str) -> Vec<u8> {
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).unwrap())
        .collect()
}

/// make.py `make_exc`: a scripted exception as the Rust error enum holds it.
pub fn make_exc(spec: &Value) -> Error {
    let class = spec["class"].as_str().unwrap();
    let msg = spec
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let attr = |k: &str| {
        spec.get(k)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    match class {
        "RuntimeUnavailable" => Error::Unavailable {
            message: msg,
            account: attr("account"),
            reason: attr("reason"),
        },
        "SignedOut" => Error::SignedOut {
            message: msg,
            runtime: attr("runtime"),
        },
        "PermissionError" | "OSError" | "FileNotFoundError" => {
            let errno = spec["errno"].as_i64().unwrap() as i32;
            let strerror = attr("strerror");
            let filename = spec
                .get("filename")
                .and_then(Value::as_str)
                .map(String::from);
            let message = match &filename {
                Some(f) => format!(
                    "[Errno {errno}] {strerror}: {}",
                    pyvalue::repr(&Py::Str(f.clone()))
                ),
                None => format!("[Errno {errno}] {strerror}"),
            };
            Error::Os(OsError {
                class: class.into(),
                errno: Some(errno),
                strerror: Some(strerror),
                filename,
                message,
            })
        }
        "InputTooLarge" => Error::Ferb(FerbError::input_too_large(
            spec["chars"].as_u64().unwrap(),
            spec["max_chars"].as_u64().unwrap(),
            None,
        )),
        "PlanRejected" => Error::Ferb(FerbError::PlanRejected {
            problems: spec["problems"]
                .as_array()
                .unwrap()
                .iter()
                .map(|p| p.as_str().unwrap().into())
                .collect(),
        }),
        "KeyError" => Error::other("KeyError", pyvalue::repr(&Py::Str(msg))),
        "ValueError" => Error::Value(msg),
        "InvalidAnswer" => Error::Invalid(msg),
        "Refused" => Error::Ferb(FerbError::Refused(msg)),
        "NoTarget" => Error::Ferb(FerbError::NoTarget(msg)),
        "AuthError" => Error::Ferb(FerbError::AuthError(msg)),
        "FerbError" => Error::Ferb(FerbError::Ferb(msg)),
        "Unsupported" => Error::Ferb(FerbError::Unsupported(msg)),
        "ModelLimitsUnknown" => Error::Ferb(FerbError::ModelLimitsUnknown(msg)),
        "TimeoutError" => Error::Os(OsError {
            class: class.into(),
            errno: None,
            strerror: None,
            filename: None,
            message: msg,
        }),
        other => Error::other(other, msg),
    }
}

/// An exception as make.py's `exc_json` recorded it, as the Rust enum holds it (for `reason`).
pub fn error_from_exc(exc: &Value) -> Error {
    let class = exc["class"].as_str().unwrap();
    let text = exc["str"].as_str().unwrap_or_default().to_string();
    let s = |k: &str| {
        exc.get(k)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    match class {
        "ServiceError" | "ContextOverflow" | "ImageRefused" => {
            let mut e = ServiceError::new(
                text,
                &s("runtime"),
                exc["network"].as_bool().unwrap_or(false),
                &s("account"),
            );
            e.kind = match class {
                "ContextOverflow" => ServiceKind::Overflow,
                "ImageRefused" => ServiceKind::Image,
                _ => ServiceKind::Plain,
            };
            Error::Service(e)
        }
        "InputTooLarge" => {
            let mut e = FerbError::input_too_large(
                exc["chars"].as_u64().unwrap(),
                exc["max_chars"].as_u64().unwrap(),
                None,
            );
            if let FerbError::InputTooLarge { runtime, .. } = &mut e {
                *runtime = s("runtime");
            }
            Error::Ferb(e)
        }
        "PermissionError" | "OSError" | "FileNotFoundError" | "TimeoutError" => {
            let filename = match exc.get("filename") {
                Some(Value::String(f)) => Some(f.clone()),
                Some(Value::Object(h)) => {
                    Some(String::from_utf8_lossy(&unhex(h["hex"].as_str().unwrap())).into_owned())
                }
                _ => None,
            };
            Error::Os(OsError {
                class: class.into(),
                errno: exc.get("errno").and_then(Value::as_i64).map(|e| e as i32),
                strerror: exc
                    .get("strerror")
                    .and_then(Value::as_str)
                    .map(String::from),
                filename,
                message: text,
            })
        }
        "Refused" => Error::Ferb(FerbError::Refused(text)),
        "NoTarget" => Error::Ferb(FerbError::NoTarget(text)),
        "AuthError" => Error::Ferb(FerbError::AuthError(text)),
        "FerbError" => Error::Ferb(FerbError::Ferb(text)),
        "Unsupported" => Error::Ferb(FerbError::Unsupported(text)),
        "RuntimeUnavailable" => Error::Unavailable {
            message: text,
            account: String::new(),
            reason: String::new(),
        },
        "PlanRejected" => Error::Ferb(FerbError::PlanRejected {
            problems: exc["problems"]
                .as_array()
                .unwrap()
                .iter()
                .map(|p| p.as_str().unwrap().into())
                .collect(),
        }),
        "RuntimeViolation" => Error::Violation(RuntimeViolation {
            message: text,
            runtime: s("runtime"),
            data: exc["data"].as_object().cloned().unwrap_or_default(),
        }),
        "InvalidAnswer" => Error::Invalid(text),
        "EmptyAnswer" => Error::AnswerMissing(AnswerMissing {
            kind: MissingKind::Empty,
            message: text,
            runtime: s("runtime"),
        }),
        "ToolLoop" => Error::AnswerMissing(AnswerMissing::tool_loop(
            &s("runtime"),
            exc["count"].as_u64().unwrap(),
        )),
        "ValueError" => Error::Value(text),
        "Stalled" => Error::Stalled {
            progressed: exc["progressed"].as_bool().unwrap(),
        },
        other => Error::other(other, text),
    }
}

/// make.py `exc_json` of a Rust error: class, str and the attributes Python's class has.
pub fn exc_json(e: &Error) -> Value {
    let mut out = Map::new();
    out.insert("class".into(), json!(e.class()));
    out.insert("str".into(), json!(e.to_string()));
    match e {
        Error::Service(s) => {
            out.insert("runtime".into(), json!(s.runtime));
            out.insert("network".into(), json!(s.network));
            out.insert("account".into(), json!(s.account));
        }
        Error::Stalled { progressed } => {
            out.insert("progressed".into(), json!(progressed));
        }
        Error::Ferb(FerbError::InputTooLarge {
            chars,
            max_chars,
            runtime,
            ..
        }) => {
            out.insert("runtime".into(), json!(runtime));
            out.insert("chars".into(), json!(chars));
            out.insert("max_chars".into(), json!(max_chars));
        }
        _ => {}
    }
    Value::Object(out)
}

// ---------------------------------------------------------------------------------------------------------------
// The host
// ---------------------------------------------------------------------------------------------------------------

pub struct TestTree {
    pub id: String,
    pub stop: Mutex<String>,
    pub runtime: String,
    pub worker_type: String,
    pub worker_task: String,
    pub start_block: Option<ContextBlock>,
    pub start_image: Option<ModelImage>,
    pub result_revision: AtomicUsize,
    pub steering: tokio::sync::Mutex<Steering>,
}

impl TestTree {
    pub fn from_spec(spec: &Value) -> TestTree {
        let s = |k: &str, d: &str| spec.get(k).and_then(Value::as_str).unwrap_or(d).to_string();
        let mut steering = Steering::default();
        if let Some(msgs) = spec.get("steering").and_then(Value::as_array) {
            steering.messages = msgs
                .iter()
                .map(|m| m.as_str().unwrap().to_string())
                .collect();
        }
        TestTree {
            id: s("id", "t1"),
            stop: Mutex::new(s("stop", "")),
            runtime: s("runtime", ""),
            worker_type: s("worker_type", "default"),
            worker_task: s("worker_task", "t1.worker"),
            start_block: spec.get("start_block").map(block_from),
            start_image: spec.get("start_image").map(|i| {
                ModelImage::new(
                    unhex(i["data"].as_str().unwrap()),
                    i["ref"].as_str().unwrap(),
                )
            }),
            result_revision: AtomicUsize::new(0),
            steering: tokio::sync::Mutex::new(steering),
        }
    }
}

impl WorkerTree for TestTree {
    fn id(&self) -> String {
        self.id.clone()
    }
    fn worker_task_id(&self) -> String {
        self.worker_task.clone()
    }
    fn stop(&self) -> String {
        self.stop.lock().unwrap().clone()
    }
    fn runtime(&self) -> String {
        self.runtime.clone()
    }
    fn worker_type(&self) -> String {
        self.worker_type.clone()
    }
    fn start_block(&self) -> Option<ContextBlock> {
        self.start_block.clone()
    }
    fn start_image(&self) -> Option<ModelImage> {
        self.start_image.clone()
    }
    fn set_result_revision(&self, revision: usize) {
        self.result_revision.store(revision, Ordering::SeqCst);
    }
    fn steering(&self) -> &tokio::sync::Mutex<Steering> {
        &self.steering
    }
}

pub struct LoggedActivity {
    log: Arc<Mutex<Vec<Value>>>,
    real: Activity,
}

impl Liveness for LoggedActivity {
    fn start(&self, worker: &str) {
        self.log
            .lock()
            .unwrap()
            .push(json!(["activity.start", worker]));
        self.real.start(worker);
    }
    fn beat(&self, worker: &str) {
        self.log
            .lock()
            .unwrap()
            .push(json!(["activity.beat", worker]));
        self.real.beat(worker);
    }
    fn waiting(&self, worker: &str, on: bool) {
        self.log
            .lock()
            .unwrap()
            .push(json!(["activity.waiting", {"worker": worker, "on": on}]));
        self.real.waiting(worker, on);
    }
    fn stalled(&self, worker: &str, stall_s: f64) -> bool {
        self.real.stalled(worker, stall_s)
    }
    fn end(&self, worker: &str) {
        self.log
            .lock()
            .unwrap()
            .push(json!(["activity.end", worker]));
        self.real.end(worker);
    }
}

pub struct TestHost {
    pub sc: Value,
    pub log: Arc<Mutex<Vec<Value>>>,
    pub clock: FakeClock,
    ids: AtomicU64,
    tokens: AtomicU64,
    spans: AtomicU64,
    pub grants: Mutex<HashMap<String, Grant>>,
    known: Mutex<BTreeSet<String>>,
    pub refused: AtomicU64,
    activity: LoggedActivity,
    pub route: bool,
    pub runtimes_made: AtomicUsize,
    pub system: Mutex<String>,
    pub identity_pending: String,
    pub tree: Mutex<Option<Arc<TestTree>>>,
    pub worker: OnceLock<Weak<Worker>>,
    pub schemas: Value,
    policy: TestPolicy,
    me: Weak<TestHost>,
}

impl TestHost {
    pub fn new(sc: &Value, schemas: &Value) -> Arc<TestHost> {
        let log: Arc<Mutex<Vec<Value>>> = Arc::default();
        let clock = FakeClock::new(None);
        let mono = clock.clone();
        let known = sc
            .get("known_tasks")
            .and_then(Value::as_array)
            .map(|a| a.iter().map(|x| x.as_str().unwrap().to_string()).collect())
            .unwrap_or_default();
        Arc::new_cyclic(|me| TestHost {
            sc: sc.clone(),
            log: log.clone(),
            clock,
            ids: AtomicU64::new(0),
            tokens: AtomicU64::new(0),
            spans: AtomicU64::new(0),
            grants: Mutex::default(),
            known: Mutex::new(known),
            refused: AtomicU64::new(sc.get("calls_refused").and_then(Value::as_u64).unwrap_or(0)),
            activity: LoggedActivity {
                log,
                real: Activity::new(move || mono.monotonic()),
            },
            route: sc.get("route").and_then(Value::as_bool).unwrap_or(true),
            runtimes_made: AtomicUsize::new(0),
            system: Mutex::default(),
            identity_pending: sc
                .pointer("/system_inputs/identity_pending")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            tree: Mutex::default(),
            worker: OnceLock::new(),
            schemas: schemas.clone(),
            policy: TestPolicy,
            me: me.clone(),
        })
    }

    pub fn rec(&self, what: &str, data: Value) {
        self.log.lock().unwrap().push(json!([what, data]));
    }

    pub fn take_log(&self) -> Vec<Value> {
        std::mem::take(&mut *self.log.lock().unwrap())
    }

    pub fn worker(&self) -> Arc<Worker> {
        self.worker
            .get()
            .and_then(Weak::upgrade)
            .expect("the worker")
    }

    fn tools(&self) -> Vec<Value> {
        self.sc
            .get("tools")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_else(fixture_tools)
    }

    fn schema_ref(&self, schema: &Value) -> Value {
        let text = serde_json::to_string(schema).unwrap();
        for (key, s) in self.schemas.as_object().unwrap() {
            if serde_json::to_string(s).unwrap() == text {
                return json!({"schema": key});
            }
        }
        json!({"schema": "unknown", "value": schema})
    }
}

static TOOLS: OnceLock<Vec<Value>> = OnceLock::new();
static OBSERVE: OnceLock<Vec<String>> = OnceLock::new();

pub fn set_tools(tools: Vec<Value>, observe: Vec<String>) {
    let _ = TOOLS.set(tools);
    let _ = OBSERVE.set(observe);
}

fn fixture_tools() -> Vec<Value> {
    TOOLS.get().cloned().unwrap_or_default()
}

impl WorkerHost for TestHost {
    fn clock(&self) -> &dyn ZonedClock {
        &self.clock
    }
    fn new_id(&self) -> String {
        format!("id-{:06}", self.ids.fetch_add(1, Ordering::SeqCst) + 1)
    }
    fn device_id(&self) -> String {
        "dev-fx".into()
    }
    fn has_route(&self) -> Option<bool> {
        Some(self.route)
    }
    fn policy(&self) -> &dyn Policy {
        &self.policy
    }
    fn make_runtime(
        &self,
        role: &str,
        mode: &str,
        tool_names: &[String],
        schema: &Value,
        effort: Option<&str>,
    ) -> Result<Arc<dyn ModelRuntime>, Error> {
        self.rec(
            "make_runtime",
            json!({"role": role, "mode": mode, "tool_names": tool_names, "schema": self.schema_ref(schema), "effort": effort}),
        );
        let specs = self.sc["runtimes"].as_array().unwrap();
        let n = self.runtimes_made.fetch_add(1, Ordering::SeqCst);
        let spec = &specs[n.min(specs.len() - 1)];
        if let Some(raise) = spec.get("make_raise") {
            return Err(make_exc(raise));
        }
        let me = self.me.upgrade().expect("the host");
        Ok(Arc::new(ScriptRuntime::new(me, spec.clone())))
    }
    fn tools_for(&self, grant: &Grant) -> Vec<Value> {
        self.rec(
            "gateway.tools_for",
            json!({"task_id": grant.task_id, "worker_id": grant.worker_id, "observe_only": grant.observe_only}),
        );
        let tools = self.tools();
        if grant.observe_only && self.sc.get("listed_as_is").is_none() {
            let observe = OBSERVE.get().cloned().unwrap_or_default();
            tools
                .into_iter()
                .filter(|t| observe.iter().any(|o| t["name"] == json!(o)))
                .collect()
        } else {
            tools
        }
    }
    fn redact(&self, value: &Value) -> Result<Value, Error> {
        self.rec("gateway.redact", value.clone());
        if let Some(raise) = self.sc.get("redact_raises") {
            return Err(make_exc(raise));
        }
        Ok(redact(value))
    }
    fn initial_image_given(&self, task_id: &str) {
        self.rec("gateway.initial_image_given", json!(task_id));
    }
    fn forget_calls(&self, worker_id: &str) {
        self.rec("gateway.forget_calls", json!(worker_id));
    }
    fn calls_refused(&self, watch_id: &str) -> u64 {
        self.rec("gateway.calls_refused", json!(watch_id));
        self.refused.load(Ordering::SeqCst)
    }
    fn issue_token(
        &self,
        task_id: &str,
        worker_id: &str,
        scopes: &BTreeSet<String>,
        identity: Identity,
        ttl_s: f64,
        observe_only: bool,
    ) -> String {
        let token = format!("tok-{:03}", self.tokens.fetch_add(1, Ordering::SeqCst) + 1);
        self.grants.lock().unwrap().insert(
            token.clone(),
            Grant {
                task_id: task_id.into(),
                worker_id: worker_id.into(),
                scopes: scopes.clone(),
                identity,
                observe_only,
            },
        );
        self.rec(
            "tokens.issue",
            json!({"task_id": task_id, "worker_id": worker_id, "scopes": scopes, "identity": identity.as_str(),
                   "ttl_s": float(ttl_s), "observe_only": observe_only, "token": token}),
        );
        token
    }
    fn validate_token(&self, token: &str) -> Result<Grant, Error> {
        self.rec("tokens.validate", json!(token));
        Ok(self.grants.lock().unwrap()[token].clone())
    }
    fn revoke_worker(&self, worker_id: &str) {
        self.rec("tokens.revoke_worker", json!(worker_id));
    }
    fn span_start(
        &self,
        kind: &str,
        task_id: Option<&str>,
        parent_id: Option<&str>,
        device_id: Option<&str>,
        input: Map<String, Value>,
    ) -> String {
        let id = format!("span-{:03}", self.spans.fetch_add(1, Ordering::SeqCst) + 1);
        self.rec(
            "spans.start",
            json!({"id": id, "kind": kind, "task_id": task_id, "parent_id": parent_id, "device_id": device_id, "input": input}),
        );
        id
    }
    fn span_finish(
        &self,
        span_id: &str,
        output: Option<Map<String, Value>>,
        error: Option<&str>,
        cost_usd: Option<Value>,
    ) {
        self.rec(
            "spans.finish",
            json!({"id": span_id, "output": output, "error": error, "cost_usd": cost_usd.unwrap_or(Value::Null)}),
        );
    }
    fn span_update_input(&self, span_id: &str, values: Map<String, Value>) {
        self.rec(
            "spans.update_input",
            json!({"id": span_id, "input": values}),
        );
    }
    fn task_known(&self, task_id: &str) -> bool {
        self.rec("tasks.get", json!(task_id));
        self.known.lock().unwrap().contains(task_id)
    }
    fn task_add(&self, record: &TaskRecord, status: &str) {
        self.known.lock().unwrap().insert(record.task.id.clone());
        self.rec(
            "tasks.add",
            json!({"task": task_json(&record.task),
                   "started_at": ferb_clock::py::isoformat(&record.started_at, ferb_clock::py::Spec::Auto),
                   "span_id": record.span_id, "tree_id": record.tree_id, "status": status}),
        );
    }
    fn task_set_span(&self, task_id: &str, span_id: &str) {
        self.rec(
            "tasks.set_span",
            json!({"task_id": task_id, "span_id": span_id}),
        );
    }
    fn set_status(&self, task_id: &str, status: &str, extra: Map<String, Value>) {
        self.rec(
            "set_status",
            json!({"task_id": task_id, "status": status, "extra": extra}),
        );
    }
    fn publish(&self, topic: &str, data: Map<String, Value>) {
        self.rec("publish", json!({"topic": topic, "data": data}));
    }
    fn journal(&self, kind: &str, data: Value) {
        self.rec("journal.append", json!({"kind": kind, "data": data}));
    }
    fn activity(&self) -> &dyn Liveness {
        &self.activity
    }
    fn signals(&self) -> (Option<Time>, Option<Time>) {
        self.rec("signals", Value::Null);
        let s = self
            .sc
            .get("signals")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let at = |i: usize| s.get(i).and_then(Value::as_str).map(time);
        (at(0), at(1))
    }
    fn browser_context(&self, task_id: &str) -> String {
        self.rec("browser_context", json!(task_id));
        if let Some(text) = self.sc.get("browser_context").and_then(Value::as_str) {
            return text.to_string();
        }
        format!("Current browser observations for {task_id} (data): one tab, example.com")
    }
    fn apps(&self) -> Result<Vec<(String, String)>, Error> {
        self.rec("apps", Value::Null);
        if let Some(raise) = self.sc.get("apps_raise") {
            return Err(make_exc(raise));
        }
        Ok(vec![
            ("com.apple.TextEdit".into(), "TextEdit".into()),
            ("com.example.Notes".into(), "Notes \u{e9}".into()),
        ])
    }
    fn running_apps(&self) -> Vec<(String, String, i64)> {
        self.rec("running_apps", Value::Null);
        vec![("com.apple.finder".into(), "Finder".into(), 4101)]
    }
    fn shell_process(&self, id: &str) -> Result<Option<Map<String, Value>>, Error> {
        self.rec("shell_process", json!(id));
        Ok(self
            .sc
            .get("shells")
            .and_then(|s| s.get(id))
            .and_then(Value::as_object)
            .cloned())
    }
    fn on_violation(&self, runtime: &str, text: &str) {
        self.rec("on_violation", json!({"runtime": runtime, "text": text}));
    }
    fn runtime_account(&self, runtime: &str, account: &str) {
        self.rec(
            "runtime_account",
            json!({"runtime": runtime, "account": account}),
        );
    }
    fn system(&self, _prompt: &str, _roles: &[String]) -> String {
        self.system.lock().unwrap().clone()
    }
    fn with_newly_always_on(&self, ctx: Vec<ContextBlock>) -> Vec<ContextBlock> {
        // supervisor.py:1260 (the supervisor's own; here as the fixture's host runs it)
        if self.identity_pending.is_empty() {
            return ctx;
        }
        let at = ctx
            .iter()
            .position(|b| !b.text.is_empty() && !b.cache)
            .unwrap_or(ctx.len());
        let mut block = ContextBlock::new(self.identity_pending.clone(), "memory.read");
        block.protected = true;
        let mut out = ctx;
        out.insert(at, block);
        out
    }
    fn context_exposure(
        &self,
        task: &Task,
        ctx: &[ContextBlock],
        assembly: &Assembly,
        tree: Option<&dyn WorkerTree>,
    ) {
        self.rec(
            "context_exposure",
            json!({"task_id": task.id, "references": ctx.iter().map(|b| b.reference.clone()).collect::<Vec<_>>(),
                   "parts": assembly.parts.len(), "shown": assembly.shown, "tree": tree.map(|t| t.id())}),
        );
    }
}

// ---------------------------------------------------------------------------------------------------------------
// The scripted runtime
// ---------------------------------------------------------------------------------------------------------------

pub struct ScriptRuntime {
    host: Arc<TestHost>,
    spec: Value,
    name: String,
    runs: AtomicUsize,
}

impl ScriptRuntime {
    pub fn new(host: Arc<TestHost>, spec: Value) -> ScriptRuntime {
        let name = spec
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("fake")
            .to_string();
        ScriptRuntime {
            host,
            spec,
            name,
            runs: AtomicUsize::new(0),
        }
    }

    fn plan(&self, n: usize) -> Value {
        let calls = self
            .spec
            .get("calls")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_else(|| vec![json!({})]);
        calls[(n - 1).min(calls.len() - 1)].clone()
    }
}

/// What the supervisor's stop does to a running worker: the tree's stop words, then each active run cancelled.
async fn stop_tree(host: &TestHost, reason: &str) {
    let tree = host.tree.lock().unwrap().clone();
    if let Some(tree) = tree {
        *tree.stop.lock().unwrap() = reason.to_string();
    }
    let worker = host.worker();
    for run in worker.runs().active() {
        run.control.cancel().await;
    }
}

impl ModelRuntime for ScriptRuntime {
    fn name(&self) -> String {
        self.name.clone()
    }
    fn limits(&self) -> Result<ModelLimits, FerbError> {
        match self.spec.get("limits") {
            None => Ok(ModelLimits {
                model: "fake-model".into(),
                context_window: 1_000_000,
                max_output: 100_000,
                source: "test double".into(),
                window_includes_output: true,
                max_input_chars: None,
            }),
            Some(Value::Null) => Err(FerbError::ModelLimitsUnknown(format!(
                "the {} runtime doesn't report its model's documented context window",
                self.name
            ))),
            Some(l) => Ok(ModelLimits {
                model: l["model"].as_str().unwrap().into(),
                context_window: l["context_window"].as_i64().unwrap(),
                max_output: l["max_output"].as_i64().unwrap(),
                source: l["source"].as_str().unwrap().into(),
                window_includes_output: l
                    .get("window_includes_output")
                    .and_then(Value::as_bool)
                    .unwrap_or(true),
                max_input_chars: l.get("max_input_chars").and_then(Value::as_i64),
            }),
        }
    }
    fn output_schema(&self) -> Option<Value> {
        self.spec.get("output_schema").cloned()
    }
    fn start<'a>(
        &'a self,
        task: &'a Task,
        request: &'a ModelRequest,
        run_token: &'a str,
    ) -> BoxFuture<'a, Result<StartedRun, Error>> {
        Box::pin(async move {
            let n = self.runs.fetch_add(1, Ordering::SeqCst) + 1;
            self.host.rec(
                "rt.start",
                json!({"runtime": self.name, "task": task_json(task), "token": run_token, "request": {
                    "system": request.system, "parts": request.parts, "cacheable": request.cacheable,
                    "images": request.images.iter().map(|i| json!({"ref": i.reference, "mime": i.mime, "data": hex(&i.data)})).collect::<Vec<_>>()}}),
            );
            let plan = self.plan(n);
            for step in plan
                .get("start")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default()
            {
                if let Some(s) = step.get("advance").and_then(Value::as_f64) {
                    self.host.clock.advance(s);
                }
                if let Some(reason) = step.get("stop").and_then(Value::as_str) {
                    stop_tree(&self.host, reason).await;
                }
                if let Some(raise) = step.get("raise") {
                    return Err(make_exc(raise));
                }
                if step.get("hang").and_then(Value::as_bool) == Some(true) {
                    std::future::pending::<()>().await;
                }
            }
            let mut state = Map::new();
            state.insert("token".into(), json!(run_token));
            state.insert("cancel".into(), json!(false));
            state.insert("n".into(), json!(n));
            if let Some(Value::Object(more)) = plan.get("state") {
                state.extend(more.clone());
            }
            let handle = RunHandle {
                id: format!("run-{n}"),
                runtime: self.name.clone(),
                task_id: task.id.clone(),
                state,
            };
            let cancelled = Arc::new(AtomicBool::new(false));
            let control = Arc::new(ScriptControl {
                host: self.host.clone(),
                run_id: handle.id.clone(),
                how: self
                    .spec
                    .get("continue")
                    .and_then(Value::as_str)
                    .unwrap_or("accept")
                    .to_string(),
                cancelled: cancelled.clone(),
            });
            let steps = plan
                .get("steps")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            Ok(StartedRun {
                handle,
                events: Box::new(ScriptEvents {
                    host: self.host.clone(),
                    steps,
                    idx: 0,
                    token: run_token.to_string(),
                    cancelled,
                }),
                control,
            })
        })
    }
}

struct ScriptControl {
    host: Arc<TestHost>,
    run_id: String,
    how: String,
    cancelled: Arc<AtomicBool>,
}

impl RunControl for ScriptControl {
    fn steer<'a>(&'a self, message: &'a str) -> BoxFuture<'a, Result<(), ContinueError>> {
        Box::pin(async move {
            self.host.rec(
                "rt.continue",
                json!({"run": self.run_id, "message": message}),
            );
            match self.how.as_str() {
                "accept" => Ok(()),
                "unsupported" | "not_implemented" => Err(ContinueError::Unsupported),
                other => Err(ContinueError::Failed(other.to_string())),
            }
        })
    }
    fn cancel(&self) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            self.host.rec("rt.cancel", json!(self.run_id));
            self.cancelled.store(true, Ordering::SeqCst);
        })
    }
}

struct ScriptEvents {
    host: Arc<TestHost>,
    steps: Vec<Value>,
    idx: usize,
    token: String,
    cancelled: Arc<AtomicBool>,
}

impl EventSource for ScriptEvents {
    fn next(&mut self) -> BoxFuture<'_, Option<Result<RuntimeEvent, Error>>> {
        Box::pin(async move {
            loop {
                if self.idx > 0 && self.cancelled.load(Ordering::SeqCst) {
                    return None;
                }
                let step = self.steps.get(self.idx)?.clone();
                self.idx += 1;
                if let Some(kind) = step.get("event").and_then(Value::as_str) {
                    let kind = RuntimeEventKind::ALL
                        .iter()
                        .copied()
                        .find(|k| k.as_str() == kind)
                        .unwrap();
                    let data = step
                        .get("data")
                        .and_then(Value::as_object)
                        .cloned()
                        .unwrap_or_default();
                    return Some(Ok(RuntimeEvent { kind, data }));
                } else if let Some(s) = step.get("advance").and_then(Value::as_f64) {
                    self.host.clock.advance(s);
                } else if let Some(p) = step.get("tool_progress") {
                    let grant = self.host.grants.lock().unwrap()[&self.token].clone();
                    let args = p
                        .get("args")
                        .and_then(Value::as_object)
                        .cloned()
                        .unwrap_or_default();
                    let _ = self.host.worker().tool_progress(
                        &grant,
                        p["name"].as_str().unwrap(),
                        &args,
                        p.get("kind").and_then(Value::as_str).unwrap_or(""),
                        p.get("site").and_then(Value::as_str),
                    );
                } else if let Some(on) = step.get("waiting").and_then(Value::as_bool) {
                    let grant = self.host.grants.lock().unwrap()[&self.token].clone();
                    self.host.activity().waiting(&grant.worker_id, on);
                } else if let Some(text) = step.get("steer").and_then(Value::as_str) {
                    // the supervisor's steering.apply: under the lock, the words added, then each live run told
                    let tree = self.host.tree.lock().unwrap().clone().unwrap();
                    let mut guard = tree.steering.lock().await;
                    guard.messages.push(text.to_string());
                    for (_, run) in self.host.worker().runs().worker_runs() {
                        let host: &dyn WorkerHost = self.host.as_ref();
                        deliver(
                            host,
                            &tree.id,
                            &mut guard,
                            &run.handle.id,
                            &run.handle.task_id,
                            run.control.as_ref(),
                        )
                        .await;
                    }
                } else if let Some(reason) = step.get("stop").and_then(Value::as_str) {
                    stop_tree(&self.host, reason).await;
                } else if step.get("stall").is_some() {
                    while !self.cancelled.load(Ordering::SeqCst) {
                        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
                    }
                    return None;
                } else if let Some(raise) = step.get("raise") {
                    return Some(Err(make_exc(raise)));
                } else if let Some(n) = step.get("refused").and_then(Value::as_u64) {
                    self.host.refused.store(n, Ordering::SeqCst);
                }
            }
        })
    }
}
