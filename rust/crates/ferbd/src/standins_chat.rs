//! Test-only (feature `stand-ins`): a scripted model and the recorder's reads, so a chat scenario recorded from the
//! Python daemon (rust/fixtures/daemon/chat_scenarios.json) replays through this real program.
//!
//! `FERBD_STAND_INS`' `"script"` names a JSON file the test wrote:
//!
//! - `calls`: every recorded model call (`key`, `events`, `tool_bridge_lines`), in the recording's order; `first`:
//!   the index of the first one this process makes (a restart continues the count); `gated`: those that answer only
//!   after the control socket's `release_model`.
//! - `supervise`, `systems`, `contexts`: what the Python supervisor read at each planner call, role instruction and
//!   worker context (rust/fixtures/supervisor/scenarios.json; `skills_line` and `abilities_line` already dumped), as
//!   queues: the environment and the live facts, the facts in every role's instructions, the relevant activity and
//!   the desktop observer's context.
//! - `spans` (`columns`, `rows`, `used`): the desktop observer's rows the recorder's daemon wrote, each written here
//!   once, under the next id, when the supervisor starts or finishes a tree (and at shutdown for a tree still open).
//! - `root`, `root_resolved`: the run's folder (written `/FX` in the recording, both spellings).
//! - `note`: `[path, text]`, the file the owner writes once the first checker has answered.
//!
//! The scripted model makes its recorded tool calls as the tool bridge does: a connection to this daemon's socket as
//! `mcp_shim` with its run token, then each `tools.call` line; each reply is compared with the recorded one.
//!
//! Also here: the recorder's file facts (`fs_identity`: a file result's `modified_at`, owner, device, inode and
//! `mtime_ns` as the recorder invented them), the questions' counter ids, the byte-ceiling tokenizer, the
//! untrusted-data wrapper's fixed id and the new chat id.

use std::collections::{BTreeSet, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use ferb_core::interfaces::IdSource;
use ferb_core::types::{
    ModelLimits, ModelRequest, RunHandle, RuntimeEvent, RuntimeEventKind, Task, ToolContext,
};
use ferb_gateway::tool::{Args, Tool, ToolError, ToolOutput};
use ferb_orchestrator::ports::BoxFuture;
use ferb_orchestrator::steering::ContinueError;
use ferb_supervisor::flow::Environment;
use ferb_supervisor::prompts::SystemInputs;
use ferb_supervisor::worker_context::Activity;
use ferb_worker::ports::{EventSource, ModelRuntime, RunControl, StartedRun};
use serde_json::{json, Map, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use crate::basics::{lock, SharedDb};
use crate::compose::supervisor::Reads;

fn s(v: &Value) -> String {
    match v {
        Value::String(t) => t.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

/// What the scripted model and the recorded reads did, read back through the control socket.
#[derive(Default)]
pub struct ScriptState {
    /// `make_runtime`'s arguments, one per call made.
    pub made: Vec<Value>,
    /// Each started call: its index, task, instructions and parts (`/FX` written back).
    pub started: Vec<Value>,
    /// The calls whose final answer was given.
    pub answered: Vec<usize>,
    /// A tool call through the bridge whose reply differed from the recording.
    pub tool_diffs: Vec<String>,
    pub tool_calls: usize,
    pub supervise_used: usize,
    pub systems_used: usize,
    pub contexts_used: usize,
    pub spans_used: BTreeSet<usize>,
    /// The trees the desktop observer watches (a baseline written, no summary yet).
    pub watching: BTreeSet<String>,
    /// A read the recording didn't make.
    pub problems: Vec<String>,
}

/// The scripted model, the recorded reads and their state.
pub struct Script {
    pub file: Value,
    pub state: Mutex<ScriptState>,
    supervise: Mutex<VecDeque<Value>>,
    systems: Mutex<VecDeque<Value>>,
    contexts: Mutex<VecDeque<Value>>,
    pub released: AtomicBool,
    /// The questions asked so far in the scenario (the counter's state; `questions_from` at start).
    pub asked: Arc<AtomicU64>,
    pub released_now: tokio::sync::Notify,
    note: Mutex<Option<(String, String)>>,
    /// Filled once the daemon is built.
    pub db: OnceLock<SharedDb>,
    pub ids: OnceLock<Arc<dyn IdSource>>,
    pub socket: OnceLock<PathBuf>,
    root: String,
    resolved: String,
}

impl Script {
    pub fn load(path: &str) -> Option<Arc<Script>> {
        let file: Value = serde_json::from_slice(&std::fs::read(path).ok()?).ok()?;
        let queue = |k: &str| {
            Mutex::new(
                file[k]
                    .as_array()
                    .cloned()
                    .unwrap_or_default()
                    .into_iter()
                    .collect::<VecDeque<_>>(),
            )
        };
        let note = file["note"].as_array().and_then(|n| {
            Some((
                n.first()?.as_str()?.to_string(),
                n.get(1)?.as_str()?.to_string(),
            ))
        });
        let used: BTreeSet<usize> = file["spans"]["used"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_u64())
                    .map(|v| v as usize)
                    .collect()
            })
            .unwrap_or_default();
        Some(Arc::new(Script {
            supervise: queue("supervise"),
            systems: queue("systems"),
            contexts: queue("contexts"),
            state: Mutex::new(ScriptState {
                spans_used: used,
                ..ScriptState::default()
            }),
            released: AtomicBool::new(false),
            asked: Arc::new(AtomicU64::new(file["questions_from"].as_u64().unwrap_or(0))),
            released_now: tokio::sync::Notify::new(),
            note: Mutex::new(note),
            db: OnceLock::new(),
            ids: OnceLock::new(),
            socket: OnceLock::new(),
            root: s(&file["root"]),
            resolved: s(&file["root_resolved"]),
            file,
        }))
    }

    /// The run's folder written `/FX`, its resolved spelling first.
    pub fn mask(&self, text: &str) -> String {
        if self.root.is_empty() {
            return text.to_string();
        }
        text.replace(&self.resolved, "/FX")
            .replace(&self.root, "/FX")
    }

    fn problem(&self, what: String) {
        lock(&self.state).problems.push(what);
    }

    fn calls(&self) -> &[Value] {
        self.file["calls"]
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    fn first(&self) -> usize {
        self.file["first"].as_u64().unwrap_or(0) as usize
    }

    fn gated(&self, n: usize) -> bool {
        self.file["gated"]
            .as_array()
            .is_some_and(|g| g.iter().any(|v| v.as_u64() == Some(n as u64)))
    }

    /// The owner's file, written when its moment comes (the first checker's answer, or a `note` step).
    pub fn apply_note(&self) {
        if let Some((path, text)) = lock(&self.note).take() {
            let _ = std::fs::write(path, text);
        }
    }

    /// The recorded desktop-observer row of this kind for this tree, written now under the next id.
    fn write_span(&self, kind: &str, tree_id: &str) {
        let cols: Vec<String> = self.file["spans"]["columns"]
            .as_array()
            .map(|a| a.iter().map(s).collect())
            .unwrap_or_default();
        let at = |name: &str| cols.iter().position(|c| c == name);
        let (Some(kc), Some(tc), Some(ic)) = (at("kind"), at("task_id"), at("id")) else {
            return;
        };
        let rows = self.file["spans"]["rows"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let found = {
            let mut st = lock(&self.state);
            let found = rows.iter().enumerate().find(|(i, r)| {
                !st.spans_used.contains(i) && r[kc] == json!(kind) && r[tc] == json!(tree_id)
            });
            if let Some((i, _)) = found {
                st.spans_used.insert(i);
            }
            found.map(|(_, r)| r.clone())
        };
        let Some(mut row) = found else {
            self.problem(format!("no recorded {kind} span for {tree_id}"));
            return;
        };
        let (Some(ids), Some(db)) = (self.ids.get(), self.db.get()) else {
            return;
        };
        row[ic] = json!(ids.new_id());
        let to_sql = |v: &Value| match v {
            Value::Null => rusqlite::types::Value::Null,
            Value::String(t) => rusqlite::types::Value::Text(t.replace("/FX", &self.root)),
            Value::Number(n) if n.is_f64() => {
                rusqlite::types::Value::Real(n.as_f64().unwrap_or(0.0))
            }
            Value::Number(n) => rusqlite::types::Value::Integer(n.as_i64().unwrap_or(0)),
            other => rusqlite::types::Value::Text(other.to_string()),
        };
        let params: Vec<rusqlite::types::Value> = row
            .as_array()
            .map(|r| r.iter().map(to_sql).collect())
            .unwrap_or_default();
        let db = lock(db);
        if let Err(e) = db.conn().execute(
            &format!(
                "INSERT INTO spans ({}) VALUES ({})",
                cols.join(", "),
                vec!["?"; cols.len()].join(", ")
            ),
            rusqlite::params_from_iter(params),
        ) {
            drop(db);
            self.problem(format!("the recorded {kind} span: {e}"));
        }
    }

    /// The state as the control socket returns it.
    pub fn report(&self) -> Value {
        let st = lock(&self.state);
        json!({"made": st.made, "started": st.started, "answered": st.answered, "tool_diffs": st.tool_diffs,
               "tool_calls": st.tool_calls, "supervise_used": st.supervise_used,
               "systems_used": st.systems_used, "contexts_used": st.contexts_used,
               "spans_used": st.spans_used, "problems": st.problems,
               "questions": self.asked.load(Ordering::SeqCst)})
    }
}

/// The recorder's reads and the scripted model, as the supervisor's [`Reads`].
pub struct ScriptReads(pub Arc<Script>);

impl ScriptReads {
    fn pop(&self, q: &Mutex<VecDeque<Value>>, what: &str) -> Value {
        match lock(q).pop_front() {
            Some(v) => v,
            None => {
                self.0.problem(format!(
                    "the supervisor read {what} more often than the recording did"
                ));
                Value::Null
            }
        }
    }
}

impl Reads for ScriptReads {
    fn captures_at_send(&self) -> Option<bool> {
        Some(
            self.0.file["captures"]
                .as_array()
                .is_some_and(|a| !a.is_empty()),
        )
    }
    /// The front window the recorder's daemon captured as this message arrived (its picture: invented bytes).
    fn front_capture<'a>(
        &'a self,
        request_id: &'a str,
    ) -> Option<BoxFuture<'a, Result<Option<ferb_supervisor::flow::FrontCapture>, String>>> {
        let got = self.0.file["captures"]
            .as_array()
            .and_then(|a| a.iter().find(|c| c["request_id"] == request_id))
            .map(|c| {
                let image_ref = s(&c["image_ref"]);
                ferb_supervisor::flow::FrontCapture {
                    window: ferb_supervisor::planner::FrontWindow {
                        facts: c["facts"].as_object().cloned().unwrap_or_default(),
                        image_ref: image_ref.clone(),
                        withheld: String::new(),
                        masking: String::new(),
                        origin: s(&c["origin"]),
                    },
                    image: Some(ferb_core::types::ModelImage::new(
                        s(&c["image"]).into_bytes(),
                        &image_ref,
                    )),
                }
            });
        Some(Box::pin(async move { Ok(got) }))
    }
    fn environment<'a>(&'a self, _words: &'a str) -> BoxFuture<'a, Environment> {
        let next = lock(&self.0.supervise).front().cloned();
        Box::pin(async move {
            let next = next.unwrap_or(Value::Null);
            Environment {
                stable: s(&next["environment"]["stable"]),
                live: s(&next["environment"]["live"]),
            }
        })
    }
    fn live_facts(&self, _last_runs: bool, _owner_state: bool) -> String {
        let rec = self.pop(&self.0.supervise, "the planner's live facts");
        lock(&self.0.state).supervise_used += 1;
        s(&rec["live_facts"])
    }
    fn system_inputs(&self, _live: bool) -> SystemInputs {
        let rec = self.pop(&self.0.systems, "a role's instructions");
        lock(&self.0.state).systems_used += 1;
        let i = &rec["inputs"];
        SystemInputs {
            facts: s(&i["facts"]),
            live_facts: s(&i["live_facts"]),
            capabilities: s(&i["capabilities"]),
            identity_context: s(&i["identity_context"]),
            skills_line: s(&i["skills_line"]),
            abilities_line: s(&i["abilities_line"]),
        }
    }
    fn availability(&self) -> String {
        s(&self.0.file["availability"])
    }
    fn relevant_context(&self, _words: &str) -> Activity {
        let next = lock(&self.0.contexts).front().cloned();
        let Some(next) = next else {
            self.0
                .problem("a context read the recording didn't make".into());
            return Activity::Text(None);
        };
        // the activity search's rows held this run's paths (written `/FX`, as the owner's activity spells them)
        let text = serde_json::to_string(&next["activity"])
            .unwrap_or_default()
            .replace("/FX", &self.0.root);
        Activity::from_json(&serde_json::from_str(&text).unwrap_or(Value::Null))
    }
    fn disturbance_context(&self, _tree_id: &str) -> String {
        let rec = self.pop(&self.0.contexts, "a context");
        lock(&self.0.state).contexts_used += 1;
        s(&rec["disturbance"])
    }
    fn on_task_start<'a>(&'a self, tree_id: &'a str) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            lock(&self.0.state).watching.insert(tree_id.to_string());
            self.0.write_span("disturbance.baseline", tree_id);
        })
    }
    fn on_task_finish<'a>(&'a self, tree_id: &'a str) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            lock(&self.0.state).watching.remove(tree_id);
            self.0.write_span("disturbance.summary", tree_id);
        })
    }
    fn running_apps(&self) -> Vec<(String, String, i64)> {
        vec![
            ("com.apple.finder".into(), "Finder".into(), 4101),
            ("com.apple.TextEdit".into(), "TextEdit".into(), 4102),
        ]
    }
    fn memory_push<'a>(
        &'a self,
        _query: &'a str,
        _targets: &'a str,
    ) -> BoxFuture<'a, Vec<ferb_context::assemble::ContextBlock>> {
        // what memory gave this context's worker, as recorded (make_more.py keeps it)
        let blocks: Vec<ferb_context::assemble::ContextBlock> = lock(&self.0.contexts)
            .front()
            .and_then(|r| r["memory"].as_array().cloned())
            .unwrap_or_default()
            .iter()
            .map(|v| ferb_context::assemble::ContextBlock {
                text: s(&v["text"]),
                reference: s(&v["reference"]),
                protected: v["protected"].as_bool().unwrap_or(false),
                cache: v["cache"].as_bool().unwrap_or(false),
                priority: v["priority"].as_i64().unwrap_or(0),
                origin: s(&v["origin"]),
                source_refs: v["source_refs"]
                    .as_array()
                    .map(|a| a.iter().map(s).collect())
                    .unwrap_or_default(),
                payload: v["payload"].as_str().map(str::to_string),
                history: v["history"].as_bool().unwrap_or(false),
                fold: s(&v["fold"]),
                values: (!v["values"].is_null()).then(|| v["values"].clone()),
            })
            .collect();
        Box::pin(async move { blocks })
    }
    fn worker_limits(&self, _worker_type: &str) -> Option<ModelLimits> {
        // the scripted runtime's (a runtime made only to read its limits is never started, never recorded)
        ScriptRuntime::fake_limits().ok()
    }
    fn worker_types(&self) -> Vec<String> {
        // the recorder's daemon had Codex and Claude available (`_runtime_available`)
        vec!["default".into(), "codex".into(), "claude".into()]
    }
    fn shutdown(&self) {
        let open: Vec<String> = std::mem::take(&mut lock(&self.0.state).watching)
            .into_iter()
            .collect();
        for tree in open {
            self.0.write_span("disturbance.summary", &tree);
        }
        // what this process used, for the process a restart starts next (the control socket is gone by then)
        if let Some(out) = self.0.file["state_out"].as_str() {
            let _ = std::fs::write(out, self.0.report().to_string());
        }
    }
    fn make_runtime(
        &self,
        role: &str,
        mode: &str,
        tool_names: &[String],
        schema: &Value,
        effort: Option<&str>,
    ) -> Result<Arc<dyn ModelRuntime>, ferb_worker::Error> {
        let n = {
            let mut st = lock(&self.0.state);
            st.made.push(
                json!({"runtime_role": role, "mode": mode, "tools": tool_names,
                                "schema": schema, "effort": effort}),
            );
            self.0.first() + st.made.len() - 1
        };
        let call = self.0.calls().get(n).cloned().unwrap_or(Value::Null);
        Ok(Arc::new(ScriptRuntime {
            script: self.0.clone(),
            n,
            events: call["events"]
                .as_array()
                .cloned()
                .unwrap_or_else(|| vec![json!({"kind": "(a call the recording didn't make)"})]),
            bridge: call["tool_bridge_lines"]
                .as_array()
                .cloned()
                .unwrap_or_default(),
        }))
    }
}

struct ScriptRuntime {
    script: Arc<Script>,
    n: usize,
    events: Vec<Value>,
    bridge: Vec<Value>,
}

impl ModelRuntime for ScriptRuntime {
    fn name(&self) -> String {
        "fake".into()
    }
    fn limits(&self) -> Result<ModelLimits, ferb_core::errors::FerbError> {
        ScriptRuntime::fake_limits()
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
        self.started(task, request, token)
    }
}

impl ScriptRuntime {
    fn fake_limits() -> Result<ModelLimits, ferb_core::errors::FerbError> {
        Ok(ModelLimits {
            model: "fake-model".into(),
            context_window: 1_000_000,
            max_output: 100_000,
            source: "test double".into(),
            window_includes_output: true,
            max_input_chars: None,
        })
    }
    fn started<'a>(
        &'a self,
        task: &'a Task,
        request: &'a ModelRequest,
        token: &'a str,
    ) -> BoxFuture<'a, Result<StartedRun, ferb_worker::Error>> {
        Box::pin(async move {
            let parts: Vec<String> = request.parts.iter().map(|p| self.script.mask(p)).collect();
            lock(&self.script.state)
                .started
                .push(json!({"n": self.n, "task_id": task.id,
                "system": request.system, "parts": parts, "cacheable": request.cacheable,
                "images": request.images.len()}));
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
                    script: self.script.clone(),
                    events: self.events.clone(),
                    at: 0,
                    cancelled: cancelled.clone(),
                    woken: woken.clone(),
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
    script: Arc<Script>,
    events: Vec<Value>,
    at: usize,
    cancelled: Arc<AtomicBool>,
    woken: Arc<tokio::sync::Notify>,
    n: usize,
    token: String,
    /// The tool calls still to make (before the first event: the scripted model calls its tools first).
    bridge: Option<Vec<Value>>,
}

impl ScriptEvents {
    /// The recorded tool calls, as the tool bridge makes them: `auth.hello` as `mcp_shim` with this run's token, then
    /// each `tools.call`; each reply compared with the recorded line.
    async fn call_tools(&self, lines: &[Value]) {
        let sent: Vec<Value> = lines
            .iter()
            .filter_map(|l| l.get("send").and_then(Value::as_str))
            .filter_map(|l| serde_json::from_str(l).ok())
            .collect();
        if sent.is_empty() {
            return;
        }
        let recv: Vec<Value> = lines
            .iter()
            .filter_map(|l| l.get("recv").and_then(Value::as_str))
            .filter_map(|l| serde_json::from_str(l).ok())
            .collect();
        let closed = lines.iter().any(|l| l.get("closed").is_some());
        let Some(socket) = self.script.socket.get() else {
            return;
        };
        let Ok(conn) = tokio::net::UnixStream::connect(socket).await else {
            self.script
                .problem(format!("call {}: no socket", self.n + 1));
            return;
        };
        let (r, mut w) = conn.into_split();
        let mut r = BufReader::new(r).lines();
        for msg in sent {
            let mut msg = msg;
            if msg["method"] == "auth.hello" {
                msg["params"]["token"] = json!(self.token);
            }
            let line = format!("{}\n", ferb_py::json::dumps(&msg));
            if w.write_all(line.as_bytes()).await.is_err() {
                break;
            }
            // the reply with this id (a stream line in between is skipped)
            let got = loop {
                match r.next_line().await {
                    Ok(Some(l)) => {
                        let v: Value = serde_json::from_str(&l).unwrap_or(Value::Null);
                        if v["id"] == msg["id"] {
                            break Some(v);
                        }
                    }
                    _ => break None,
                }
            };
            if msg["method"] == "auth.hello" {
                continue;
            }
            lock(&self.script.state).tool_calls += 1;
            let want = recv
                .iter()
                .find(|v| v["id"] == msg["id"])
                .map(|v| v["result"].clone());
            let want = match want {
                Some(w) => w,
                None if closed => json!({"error": "Cancelled"}),
                None => Value::Null,
            };
            let got = match got {
                Some(v) if v.get("result").is_some() => v["result"].clone(),
                Some(v) => json!({"error": v["error"]["message"]}),
                None => json!({"error": "Cancelled"}),
            };
            let got: Value = serde_json::from_str(&self.script.mask(&ferb_py::json::dumps(&got)))
                .unwrap_or(Value::Null);
            if got != want {
                lock(&self.script.state).tool_diffs.push(format!(
                    "call {} {}\n    python: {}\n    rust:   {}",
                    self.n + 1,
                    msg["params"]["name"],
                    ferb_py::json::dumps(&want),
                    ferb_py::json::dumps(&got)
                ));
            }
        }
    }
}

impl EventSource for ScriptEvents {
    fn next(&mut self) -> BoxFuture<'_, Option<Result<RuntimeEvent, ferb_worker::Error>>> {
        Box::pin(async move {
            // as tests/conftest.py's FakeRuntime: a cancel ends only a call that never answers (STALL); a scripted
            // answer already under way still comes
            if self.at == 0 && self.script.gated(self.n) {
                loop {
                    let opened = self.script.released_now.notified();
                    let woken = self.woken.notified();
                    tokio::pin!(opened, woken);
                    opened.as_mut().enable();
                    woken.as_mut().enable();
                    if self.script.released.load(Ordering::SeqCst) {
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
                lock(&self.script.state).answered.push(self.n);
                let first_check = self
                    .script
                    .calls()
                    .iter()
                    .position(|m| m["key"] == "worker.verify");
                if first_check == Some(self.n) {
                    self.script.apply_note();
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
                // "(no answer: FERB cancelled or left the call)": nothing until FERB cancels it
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
// the recorder's file facts
// ---------------------------------------------------------------------------------------------------------------

/// A file tool whose results carry the recorder's invented identity facts (make.py `files.identity`):
/// `modified_at` 2026-01-01T09:00:00+00:00, `owner_uid` 501, `owner_gid` 20, `device` 1, `inode` the first 6 hex of
/// the SHA-256 of the `/FX` path, `mtime_ns` 1767258000000000000. Everything else is the real tool.
pub struct RecordedFacts {
    pub script: Arc<Script>,
}

impl RecordedFacts {
    fn rewrite(&self, v: &mut Value) {
        match v {
            Value::Object(m) => {
                if m.contains_key("inode") && m.contains_key("mtime_ns") {
                    let path = m.get("path").map(s).unwrap_or_default();
                    let fx = self.script.mask(&path);
                    use sha2::Digest;
                    let digest = hex::encode(sha2::Sha256::digest(fx.as_bytes()));
                    let inode = i64::from_str_radix(&digest[..6], 16).unwrap_or(0);
                    for (k, val) in [
                        ("modified_at", json!("2026-01-01T09:00:00+00:00")),
                        ("owner_uid", json!(501)),
                        ("owner_gid", json!(20)),
                        ("device", json!(1)),
                        ("inode", json!(inode)),
                        ("mtime_ns", json!(1_767_258_000_000_000_000i64)),
                    ] {
                        if m.contains_key(k) {
                            m.insert(k.into(), val);
                        }
                    }
                }
                for x in m.values_mut() {
                    self.rewrite(x);
                }
            }
            Value::Array(a) => a.iter_mut().for_each(|x| self.rewrite(x)),
            _ => {}
        }
    }
}

impl crate::compose::tools::wrap::Around for RecordedFacts {
    fn run<'a>(
        &'a self,
        inner: &'a Arc<dyn Tool>,
        args: &'a Args,
        ctx: &'a ToolContext,
    ) -> BoxFuture<'a, Result<ToolOutput, ToolError>> {
        Box::pin(async move {
            // `inner` is the registry's tool: a file tool already does its work in a thread (compose/tools/wrap.rs)
            let mut out = inner.run(args, ctx).await?;
            let mut content = Value::Object(std::mem::take(&mut out.result.content));
            self.rewrite(&mut content);
            if let Value::Object(m) = content {
                out.result.content = m;
            }
            Ok(out)
        })
    }
}

/// The questions' ids as the recorder's counter gives them (`ap-a00000000001`…): one counter per scenario, so a
/// restarted process continues from `asked` (the control socket's `script` report gives it).
pub fn question_counter(asked: Arc<AtomicU64>) -> crate::compose::gateway::QuestionIds {
    Arc::new(move || format!("ap-a{:011}", asked.fetch_add(1, Ordering::SeqCst) + 1))
}

/// The control socket's commands for a chat scenario.
pub fn control(
    d: &Arc<crate::compose::Daemon>,
    script: Option<&Arc<Script>>,
    req: &Value,
    what: &str,
) -> Option<Result<Value, String>> {
    Some(match what {
        "script" => match script {
            Some(s) => Ok(s.report()),
            None => Err("no script".into()),
        },
        "release_model" => match script {
            Some(s) => {
                s.released.store(true, Ordering::SeqCst);
                s.released_now.notify_waiters();
                Ok(Value::Null)
            }
            None => Err("no script".into()),
        },
        "note" => {
            if let Some(s) = script {
                s.apply_note();
            }
            Ok(Value::Null)
        }
        "pending" => {
            let n = ferb_supervisor::flow::Daemon::approvals(d.supervisor.daemon.as_ref()).len();
            Ok(json!(n))
        }
        "sleeping" => {
            let n = d.supervisor.supervisor.inner().live().sleeping.len();
            Ok(json!(n))
        }
        "root_status" => {
            let db = lock(&d.db);
            Ok(db
                .conn()
                .query_row(
                    "SELECT status FROM tasks WHERE parent_id IS NULL ORDER BY rowid DESC LIMIT 1",
                    [],
                    |r| r.get::<_, String>(0),
                )
                .map(Value::String)
                .unwrap_or(Value::Null))
        }
        // make_more.py `loop_status`: a loop's latest status says it watches
        "loop_watching" => Ok(json!(d
            .supervisor
            .supervisor
            .loop_statuses()
            .iter()
            .any(|v| v["state"] == "watching"))),
        // a plan step waits for an event (`_plan_runs`)
        "plan_waits" => Ok(json!(!d
            .supervisor
            .supervisor
            .inner()
            .live()
            .plan_runs
            .is_empty())),
        // make_more.py: the owner is back (`mention_paused`, a task of its own on the daemon's loop)
        "mention_paused" => {
            let inner = d.supervisor.supervisor.inner().clone();
            tokio::spawn(async move {
                let _ = inner.mention_paused().await;
            });
            Ok(Value::Null)
        }
        // make_more.py: an event reaches the supervisor (`notify_plan_event`)
        "plan_event" => {
            let topic = req["topic"].as_str().unwrap_or_default().to_string();
            d.supervisor
                .supervisor
                .inner()
                .notify_plan_event(&topic, &req["data"]);
            Ok(Value::Null)
        }
        // make_more.py: the app's one tap on a runtime offer (`runtime_offer`, `answer_runtime_switch`)
        "runtime_switch" => {
            let inner = d.supervisor.supervisor.inner();
            let task = req["task_id"].as_str().unwrap_or_default();
            let runtime = req["runtime"].as_str().unwrap_or_default();
            let offer = inner.runtime_offer(task).unwrap_or(Value::Null);
            let answered = inner
                .answer_runtime_switch(task, runtime)
                .unwrap_or_else(|e| json!({"error": e}));
            Ok(json!({"offer": offer, "answered": answered}))
        }
        "quiet_mark" => {
            let db = lock(&d.db);
            let q = |sql: &str| {
                db.conn()
                    .query_row(sql, [], |r| r.get::<_, i64>(0))
                    .unwrap_or(0)
            };
            Ok(json!([
                q("SELECT COALESCE(MAX(seq), 0) FROM journal"),
                q("SELECT COUNT(*) FROM spans")
            ]))
        }
        _ => {
            let _ = req;
            return None;
        }
    })
}
