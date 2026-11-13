//! One worker attempt and the shared model call (worker.py:364-796).
//!
//! `run_worker` is one attempt of a task step (a worker, the checker, a fix turn or a re-check): a scoped run token,
//! the tools its grant offers, the instructions and context, the runtime the supervisor's factory builds, and a
//! typed result: the report the model wrote, or an outcome code wrote (a failure, a stop, a service down, an
//! overflow). No action-count, attempt-count or task-duration cutoff: the orchestrator chooses recovery.
//!
//! `run_call` is `_run`: every model call goes through it, the worker's once and any other (the planner's turn, a
//! summary, attention, the question router) with its silence guard: a call silent from its very start is tried once
//! more (owner 2026-10-27), except the planner's turn (ORCH-528).

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ferb_clock::py::fmt_g;
use ferb_context::assemble::{assemble_with, AssembleError, ContextBlock};
use ferb_context::prompts::PromptRoot;
use ferb_context::pyvalue::{self, Dumps, Py};
use ferb_context::structured::{check_final, CheckError};
use ferb_context::tokens::Tokenizer;
use ferb_core::errors::FerbError;
use ferb_core::timing::{CallTimer, Steps};
use ferb_core::types::{
    ModelImage, ModelRequest, Outcome, OutcomeStatus, RuntimeEventKind, Task, TaskRole,
};
use ferb_orchestrator::steering::{ContinueError, Steering};
use serde_json::{json, Map, Value};

use crate::errors::{
    account_problem, ended, problem_of, reason, retry_stall_problem, too_large_problem, words,
    AnswerMissing, Error, MissingKind, RuntimeViolation, ServiceError, ServiceKind,
    ACCOUNT_PROBLEMS, WRONG_CALLS_CAP,
};
use crate::ports::{ModelRuntime, RunControl, StartedRun, TaskRecord, WorkerHost, WorkerTree};
use crate::runs::{ActiveRun, ConnectionNotice, Runs, Tell};

/// worker.py:38 `NO_TOOLS_LOADED`: the fact a rerun is given when the runtime never loaded FERB's tools.
pub const NO_TOOLS_LOADED: &str = "Runtime fact: your previous attempt at this exact step never loaded FERB's tools, so it could not act or inspect anything. FERB's tools may be deferred: first call tool_search with the query \"ferb\" (and with an exact tool name if one you need is still missing), then do the step.";

const TOOLS_MEANING: &str = "tool names observed in provider request traces; not proof of invocation or complete deferred inventory";

/// How the worker runs, set once by the daemon.
#[derive(Clone, Debug)]
pub struct WorkerConfig {
    /// How often the silence watchdog looks (the supervisor's `HEARTBEAT_POLL_S`).
    pub heartbeat_poll_s: f64,
    /// Silence that stops a run (guards.py `STALL_S`, ORCH-528).
    pub stall_s: f64,
    /// How the reserve and the context are counted (o200k; the byte ceiling when the tokenizer is missing).
    pub tokenizer: Tokenizer,
    /// The package the prompt files are read from (`runtime_tools.md`).
    pub prompts: PromptRoot,
    /// The supervisor's `_approach()`: the prompts' folder ("orchestrator/prompts").
    pub approach: String,
}

/// What one attempt handed back (worker.py `run_worker`'s first value).
#[derive(Clone, Debug, PartialEq)]
pub enum Attempt {
    /// The model's report, as `json.loads` read it (an object; numbers and key order as Python holds them). The
    /// supervisor validates it (`WorkerReport`, `CheckerOutcome`, ...).
    Report(Py),
    /// An outcome code wrote.
    Outcome(AttemptOutcome),
}

#[derive(Clone, Debug, PartialEq)]
pub struct AttemptOutcome {
    pub outcome: Outcome,
    pub kind: OutcomeKind,
}

/// worker.py's `Outcome` subclasses.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OutcomeKind {
    Plain,
    /// worker.py:65 `ServiceDown`: an account fact its runtime reported (`problem`: "usage_limit" | "signed_out").
    ServiceDown {
        runtime: String,
        problem: String,
    },
    /// worker.py:85 `Overflowed`: the context overflowed (refused before sending, or the window-exceeded error).
    Overflowed,
}

/// `run_worker`'s keyword arguments.
#[derive(Clone, Debug, Default)]
pub struct RunOptions {
    /// The reasoning effort the run asks its runtime for (L39); None keeps the runtime's default.
    pub effort: Option<String>,
    /// The role blocks the run's instructions get (`_system(roles=...)`).
    pub roles: Vec<String>,
    /// The token offers and runs observation tools only (the checker always; a worker's fix turn, ORCH-545).
    pub observe_only: bool,
}

/// A callback told one record (a usage report, the call's timing).
pub type OnRecord = Arc<dyn Fn(Map<String, Value>) + Send + Sync>;
/// A callback told a streamed piece of text.
pub type OnText = Arc<dyn Fn(&str) + Send + Sync>;

/// What a non-worker call tells its caller as it goes (`_run`'s callbacks).
#[derive(Clone, Default)]
pub struct CallHooks {
    /// A streamed piece of the answer.
    pub on_delta: Option<OnText>,
    /// One usage report per model call, with its timing.
    pub on_usage: Option<OnRecord>,
    /// Where a chat turn's live line goes (its stream).
    pub notify: Option<Tell>,
    /// The model called a tool, or its answer tool's call began (ORCH-516, RUN-447).
    pub on_tool: Option<Arc<dyn Fn() + Send + Sync>>,
    /// The call's timing when it ends, however it ends (ORCH-526).
    pub on_timing: Option<OnRecord>,
}

/// One model call through `_run`.
pub struct Call<'a> {
    pub task: &'a Task,
    pub system: String,
    pub context: Vec<ContextBlock>,
    /// The run token ("" for none: no tools).
    pub token: &'a str,
    pub parent_span: &'a str,
    pub tree: Option<&'a Arc<dyn WorkerTree>>,
    /// "" for a call that is not a worker's.
    pub worker_id: &'a str,
    pub images: Vec<ModelImage>,
    pub hooks: CallHooks,
    /// False for the planner's turn (ORCH-528): a silence is not tried again.
    pub retry_silent: bool,
}

/// The worker: `WorkerRuns`, on its host.
pub struct Worker {
    host: Arc<dyn WorkerHost>,
    runs: Arc<Runs>,
    config: WorkerConfig,
}

/// worker.py:249 `reserve_tokens`: tokens kept beside the context: the instructions, the tool and answer schemas
/// (each as `json.dumps` writes it), and room for the runtime's own framing.
pub fn reserve_tokens(
    tokenizer: Tokenizer,
    system: &str,
    tool_schemas: &[Value],
    output_schema: Option<&Value>,
) -> i64 {
    let tools = pyvalue::dumps_value(&Value::Array(tool_schemas.to_vec()), Dumps::DEFAULT);
    let schema = match output_schema {
        Some(s) => pyvalue::dumps_value(s, Dumps::DEFAULT),
        None => "null".to_string(),
    };
    (tokenizer.count(system) + tokenizer.count(&tools) + tokenizer.count(&schema) + 1024) as i64
}

/// `task.id if task.id != task.request_id else None`: a chat turn's call has no task of its own.
fn span_task(task: &Task) -> Option<&str> {
    (task.id != task.request_id).then_some(task.id.as_str())
}

fn obj(v: Value) -> Map<String, Value> {
    match v {
        Value::Object(m) => m,
        _ => Map::new(),
    }
}

/// `str(value)` of a decoded JSON value, as Python writes it.
fn py_str(v: &Value) -> String {
    pyvalue::to_str(&Py::from_value(v))
}

/// `str(data.get(key, default))`.
fn get_str(data: &Map<String, Value>, key: &str, default: Option<&str>) -> String {
    match data.get(key) {
        Some(v) => py_str(v),
        None => default.unwrap_or("None").to_string(),
    }
}

fn truthy(data: &Map<String, Value>, key: &str) -> bool {
    data.get(key).is_some_and(ferb_py::value::truthy)
}

fn redact_text(host: &dyn WorkerHost, text: &str) -> String {
    match host.redact(&Value::String(text.to_string())) {
        Ok(v) => py_str(&v),
        // the owner's line can't be redacted: nothing of the words is shown
        Err(_) => String::new(),
    }
}

/// steering.py:66 `deliver`: the corrections a run hasn't been given, sent into it as one message. The caller holds
/// the tree's steering lock. A runtime that can't take same-run input leaves the update pending; any other failure
/// is journaled (`task.steering_delivery_failed`) and left pending too. The same as ferb-orchestrator's
/// `steering::deliver`, whose `&dyn Appender` journal keeps a future holding it from being `Send`; here the journal
/// is the host's.
pub async fn deliver(
    host: &dyn WorkerHost,
    tree_id: &str,
    state: &mut Steering,
    run_id: &str,
    run_task_id: &str,
    control: &dyn RunControl,
) -> bool {
    let after = state.delivered.get(run_id).copied().unwrap_or(0);
    let revision = state.messages.len();
    if after == revision {
        return true;
    }
    let text = state.messages[after..revision].join("\n\n");
    match control.steer(&text).await {
        Ok(()) => {}
        Err(ContinueError::Unsupported) => return false,
        Err(ContinueError::Failed(kind)) => {
            host.journal(
                "task.steering_delivery_failed",
                json!({"task_id": tree_id, "run_id": run_id, "error": kind, "revision": revision}),
            );
            return false;
        }
    }
    state.delivered.insert(run_id.to_string(), revision);
    host.journal(
        "task.steering_delivered",
        json!({"task_id": tree_id, "worker_task_id": run_task_id, "run_id": run_id, "revision": revision}),
    );
    true
}

/// worker.py:789 `_watch`: stops a silent run (starting or running); the owner's waits suspend it (the host's
/// liveness). Fires once.
struct Watchdog<'a> {
    host: &'a dyn WorkerHost,
    watch_id: &'a str,
    stall_s: f64,
    poll: Duration,
    sleep: Pin<Box<tokio::time::Sleep>>,
    fired: bool,
}

impl<'a> Watchdog<'a> {
    fn new(host: &'a dyn WorkerHost, watch_id: &'a str, poll_s: f64, stall_s: f64) -> Self {
        let poll = Duration::from_secs_f64(poll_s.max(0.0));
        Watchdog {
            host,
            watch_id,
            stall_s,
            poll,
            sleep: Box::pin(tokio::time::sleep(poll)),
            fired: false,
        }
    }

    /// Completes when the run has shown no sign of life for stall_s; never again after that.
    async fn fire(&mut self) {
        if self.fired {
            return std::future::pending().await;
        }
        loop {
            self.sleep.as_mut().await;
            let next = tokio::time::Instant::now() + self.poll;
            self.sleep.as_mut().reset(next);
            if self.host.activity().stalled(self.watch_id, self.stall_s) {
                self.fired = true;
                return;
            }
        }
    }
}

/// Awaits `fut` while the watchdog watches; when it fires, the run is marked stalled and cancelled, and `fut` is
/// still awaited (Python's watchdog is its own task: `stop()` cancels the run, the caller's await goes on).
async fn watched<T>(
    fut: impl Future<Output = T>,
    wd: &mut Watchdog<'_>,
    runs: &Runs,
    control: &Arc<dyn RunControl>,
) -> T {
    tokio::pin!(fut);
    loop {
        tokio::select! {
            biased;
            out = &mut fut => return out,
            _ = wd.fire() => {
                runs.mark_stalled(wd.watch_id);
                control.cancel().await;
            }
        }
    }
}

/// What Python's `finally` blocks still do when the task running an attempt or a call is cancelled (in Rust: its
/// future dropped, by a shutdown or a caller that gave up): the run is forgotten and cancelled, its liveness ends,
/// and an attempt's token and gateway counts go. Async steps can't run in `drop`: the run's cancel is spawned on the
/// current tokio runtime, and the steering bookkeeping, the result revision and the timing are not written (the
/// tree is being torn down). Disarmed on every path that finishes normally.
struct Cleanup<'a> {
    worker: &'a Worker,
    armed: bool,
    /// The started run: its id and control.
    run: Option<(String, Arc<dyn RunControl>)>,
    /// The worker id whose run, notice, token and counts go ("" for a call that is not a worker's).
    worker_id: String,
    /// A liveness id to end.
    watch: Option<String>,
    /// An attempt (`_run_worker_once`): its token is revoked and its gateway counts forgotten.
    attempt: bool,
}

impl<'a> Cleanup<'a> {
    fn new(worker: &'a Worker, worker_id: &str) -> Self {
        Cleanup {
            worker,
            armed: true,
            run: None,
            worker_id: worker_id.to_string(),
            watch: None,
            attempt: false,
        }
    }
}

impl Drop for Cleanup<'_> {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        let runs = &self.worker.runs;
        let host = self.worker.host();
        if let Some((run_id, control)) = self.run.take() {
            runs.remove_active(&run_id);
            runs.remove_worker_run(&self.worker_id);
            runs.remove_connection(&self.worker_id);
            if let Ok(handle) = tokio::runtime::Handle::try_current() {
                handle.spawn(async move { control.cancel().await });
            }
        }
        if let Some(watch) = self.watch.take() {
            host.activity().end(&watch);
        }
        if self.attempt {
            runs.unmark_stalled(&self.worker_id);
            runs.take_retrying(&self.worker_id);
            host.activity().end(&self.worker_id);
            host.revoke_worker(&self.worker_id);
            host.forget_calls(&self.worker_id);
        }
    }
}

impl Worker {
    pub fn new(host: Arc<dyn WorkerHost>, runs: Arc<Runs>, config: WorkerConfig) -> Self {
        Worker { host, runs, config }
    }

    pub fn host(&self) -> &dyn WorkerHost {
        self.host.as_ref()
    }

    pub fn runs(&self) -> &Arc<Runs> {
        &self.runs
    }

    pub fn config(&self) -> &WorkerConfig {
        &self.config
    }

    /// worker.py:199 `reason` with this host's redactor and network fact.
    pub fn reason(&self, e: &Error) -> String {
        let host = self.host();
        reason(e, Some(&|t| redact_text(host, t)), &|| host.has_route())
    }

    fn route(&self) -> impl Fn() -> Option<bool> + '_ {
        move || self.host.has_route()
    }

    /// worker.py:364 `run_worker`: one attempt. A run whose runtime shows FERB's tools were never loaded is a
    /// runtime fault, not a judgment: it is re-run once with that fact. Returns the result and the attempt's worker
    /// id ("" when the tree had stopped before it began). An `Err` is what Python lets escape before the attempt's
    /// own handling (the run token or the prompt file failing).
    #[allow(clippy::too_many_arguments)] // worker.py:364's own arguments, as the supervisor passes them
    pub async fn run_worker(
        &self,
        tree: &Arc<dyn WorkerTree>,
        task: &Task,
        prompt: &str,
        ctx: Vec<ContextBlock>,
        schema: &Value,
        span: &str,
        options: &RunOptions,
    ) -> Result<(Attempt, String), Error> {
        let (result, worker_id) = self
            .run_worker_once(tree, task, prompt, ctx.clone(), schema, span, options)
            .await?;
        let seen = self.runs.take_tools_seen(&worker_id);
        let offered = self.runs.take_tools_offered(&worker_id);
        if let Some(seen) = seen {
            if offered
                && !seen.iter().any(|name| name.contains("ferb"))
                && matches!(result, Attempt::Report(_))
                && tree.stop().is_empty()
            {
                let mut input = Map::new();
                input.insert("tools_seen".into(), json!(seen));
                let s = self.host.span_start(
                    "runtime.tools_not_loaded",
                    Some(&task.id),
                    Some(span),
                    None,
                    input,
                );
                self.host
                    .span_finish(&s, Some(obj(json!({"rerun": true}))), None, None);
                let mut fact =
                    ContextBlock::new(NO_TOOLS_LOADED, "runtime fact: FERB tools were not loaded");
                fact.protected = true;
                let mut ctx = ctx;
                ctx.push(fact);
                let (result, worker_id) = self
                    .run_worker_once(tree, task, prompt, ctx, schema, span, options)
                    .await?;
                self.runs.take_tools_seen(&worker_id);
                self.runs.take_tools_offered(&worker_id);
                return Ok((result, worker_id));
            }
        }
        Ok((result, worker_id))
    }

    #[allow(clippy::too_many_arguments)]
    async fn run_worker_once(
        &self,
        tree: &Arc<dyn WorkerTree>,
        task: &Task,
        prompt: &str,
        mut ctx: Vec<ContextBlock>,
        schema: &Value,
        span: &str,
        options: &RunOptions,
    ) -> Result<(Attempt, String), Error> {
        let host = self.host();
        let stop = tree.stop();
        if !stop.is_empty() {
            return Ok((
                plain(task, OutcomeStatus::Cancelled, stop, vec![]),
                String::new(),
            ));
        }
        let worker_id = format!("w-{}", host.new_id());
        self.runs.set_tree(&task.id, tree.clone());
        let started = host.clock().now();
        let mut input = Map::new();
        input.insert("goal".into(), json!(task.goal));
        input.insert("role".into(), json!(task.role.as_str()));
        input.insert(
            "scopes".into(),
            json!(task.scopes.iter().collect::<Vec<_>>()),
        );
        if let Some(effort) = options.effort.as_deref().filter(|e| !e.is_empty()) {
            input.insert("effort".into(), json!(effort));
        }
        let device_id = host.device_id();
        let wspan = host.span_start(
            "worker",
            Some(&task.id),
            Some(span),
            Some(&device_id),
            input,
        );
        if !host.task_known(&task.id) {
            let record = TaskRecord {
                task: task.clone(),
                started_at: started,
                span_id: wspan.clone(),
                tree_id: tree.id(),
            };
            host.task_add(&record, "running");
        } else {
            host.set_status(&task.id, "running", Map::new());
        }
        host.task_set_span(&task.id, &wspan);
        let ttl = match &task.deadline {
            Some(deadline) => seconds_between(&started, deadline),
            None => f64::INFINITY,
        };
        // The final checker's token is observe-only: the gateway offers and runs observation tools only (V3, W4).
        let token = host.issue_token(
            &task.id,
            &worker_id,
            &task.scopes,
            task.identity,
            ttl.max(60.0),
            options.observe_only || task.role == TaskRole::Verifier,
        );
        let tools: Vec<String> = host
            .tools_for(&host.validate_token(&token)?)
            .iter()
            .map(|t| t.get("name").map(py_str).unwrap_or_default())
            .collect();
        self.runs.set_tools_offered(&worker_id, !tools.is_empty());
        let names = if tools.is_empty() {
            "none".to_string()
        } else {
            tools
                .iter()
                .map(|n| n.replace('.', "_"))
                .collect::<Vec<_>>()
                .join(", ")
        };
        let (owner_active, last_event) = host.signals();
        if task.role == TaskRole::Verifier {
            let mut block = ContextBlock::new(
                host.browser_context(&task.id),
                "current browser observations",
            );
            block.protected = true;
            ctx.push(block);
        }
        let mut images: Vec<ModelImage> = Vec::new();
        if task.role == TaskRole::Worker {
            // L12: the request's worker starts from what the owner saw at send
            if let Some(block) = tree.start_block() {
                ctx.push(block);
            }
            if let Some(image) = tree.start_image() {
                images.push(image);
                host.initial_image_given(&task.id);
            }
        }
        let now = host.clock().now();
        let head = ferb_clock::context::time_block(
            &now,
            &host.clock().tzname(),
            &ferb_clock::context::Facts {
                task_started: Some(&started),
                deadline: task.deadline.as_ref(),
                owner_last_active: owner_active.as_ref(),
                last_trail_event: last_event.as_ref(),
                ages: &[],
                since_last_talk: None,
            },
        );
        let tools_text = self
            .config
            .prompts
            .load(
                &format!("{}/runtime_tools.md", self.config.approach),
                &[("tool_names", &names)],
            )
            .map_err(|e| Error::other("OSError", e.to_string()))?;
        let mut block = ContextBlock::new(
            format!("{tools_text}\n\n{head}"),
            "runtime tools and time facts",
        );
        block.protected = true;
        ctx.push(block);
        let mut data = Map::new();
        data.insert("task_id".into(), json!(task.id));
        data.insert("worker_id".into(), json!(worker_id));
        host.publish("task.started", data);
        host.activity().start(&worker_id);
        let mut cleanup = Cleanup::new(self, &worker_id);
        cleanup.attempt = true;

        let timing: Arc<Mutex<Map<String, Value>>> = Arc::default();
        let outcome = async {
            if task.role == TaskRole::Worker {
                // a startup failure is a current-attempt failure, not a stale result to retry forever
                let revision = tree.steering().lock().await.messages.len();
                tree.set_result_revision(revision);
            }
            // the owner's one-tap switch for this task (ORCH-315) runs its worker and its checker there
            let runtime = tree.runtime();
            let role = if task.role == TaskRole::Verifier {
                if runtime.is_empty() {
                    "checker".to_string()
                } else {
                    format!("checker:{runtime}")
                }
            } else if runtime.is_empty() {
                format!("worker:{}", tree.worker_type())
            } else {
                format!("worker:{runtime}")
            };
            let effort = options.effort.as_deref().filter(|e| !e.is_empty());
            let rt = host.make_runtime(&role, "worker", &tools, schema, effort)?;
            let sink = timing.clone();
            let call = Call {
                task,
                system: host.system(prompt, &options.roles),
                context: host.with_newly_always_on(ctx),
                token: &token,
                parent_span: &wspan,
                tree: Some(tree),
                worker_id: &worker_id,
                images,
                hooks: CallHooks {
                    on_timing: Some(Arc::new(move |facts| {
                        sink.lock().unwrap_or_else(|e| e.into_inner()).extend(facts)
                    })),
                    ..CallHooks::default()
                },
                retry_silent: true,
            };
            let text = self.run_call(&rt, call).await?;
            match pyvalue::loads(&text) {
                Ok(value) if value.as_dict().is_some() => Ok(value),
                Ok(_) => Err(Error::Invalid("the answer is not a JSON object".into())),
                Err(pyvalue::LoadError::Value(msg)) => {
                    Err(Error::Invalid(format!("the answer is not JSON ({msg})")))
                }
                Err(pyvalue::LoadError::TooDeep) => Err(Error::other(
                    "RecursionError",
                    "maximum recursion depth exceeded while decoding a JSON document",
                )),
            }
        }
        .await;
        cleanup.armed = false;
        let (result, failure) = match outcome {
            Ok(report) => (Attempt::Report(report), None),
            Err(e) => self.classify(tree, task, &worker_id, e),
        };
        self.runs.unmark_stalled(&worker_id);
        self.runs.take_retrying(&worker_id);
        host.activity().end(&worker_id);
        host.revoke_worker(&worker_id);
        host.forget_calls(&worker_id);
        let mut output = Map::new();
        match &result {
            Attempt::Outcome(o) => {
                output.insert("result".into(), json!(o.outcome.summary));
                output.insert("status".into(), json!(o.outcome.status.as_str()));
            }
            Attempt::Report(report) => {
                output.insert(
                    "result".into(),
                    json!(pyvalue::dumps(report, Dumps::DEFAULT)),
                );
                output.insert(
                    "status".into(),
                    report
                        .get("status")
                        .map(Py::to_value)
                        .unwrap_or(Value::Null),
                );
            }
        }
        let timing = timing.lock().unwrap_or_else(|e| e.into_inner()).clone();
        if !timing.is_empty() {
            output.insert("timing".into(), Value::Object(timing));
        }
        host.span_finish(&wspan, Some(output), failure.as_deref(), None);
        Ok((result, worker_id)) // `task.finished` is the tree's, published once when it ends
    }

    /// worker.py:446-488: an attempt's failure as the outcome code writes, and the failure the records keep.
    fn classify(
        &self,
        tree: &Arc<dyn WorkerTree>,
        task: &Task,
        worker_id: &str,
        e: Error,
    ) -> (Attempt, Option<String>) {
        let host = self.host();
        let redact = |t: &str| redact_text(host, t);
        let route = self.route();
        match &e {
            Error::Violation(_) | Error::AnswerMissing(_) => {
                // plain words with the reason, which the owner may read (L11); the exact failure is evidence
                let failure = e.failure();
                let summary = format!(
                    "This attempt was stopped: {}",
                    reason(&e, Some(&redact), &route)
                );
                (
                    plain(task, OutcomeStatus::Failed, summary, vec![failure.clone()]),
                    Some(failure),
                )
            }
            Error::Service(s) => {
                let failure = format!("{}: {}", s.runtime, e.failure());
                if ACCOUNT_PROBLEMS.contains(&s.account.as_str()) {
                    // step 8b: code writes it and offers another runtime
                    let outcome = outcome(
                        task,
                        OutcomeStatus::Blocked,
                        account_problem(&s.runtime, &s.account),
                        vec![failure.clone()],
                        OutcomeKind::ServiceDown {
                            runtime: s.runtime.clone(),
                            problem: s.account.clone(),
                        },
                    );
                    (outcome, Some(failure))
                } else {
                    let kind = if s.kind == ServiceKind::Overflow {
                        OutcomeKind::Overflowed
                    } else {
                        OutcomeKind::Plain
                    };
                    let outcome = outcome(
                        task,
                        OutcomeStatus::Blocked,
                        problem_of(s, &route),
                        vec![],
                        kind,
                    );
                    (outcome, Some(failure))
                }
            }
            Error::SignedOut { runtime, .. } => {
                // the runtime can't start: FERB's sign-in for it is missing
                let failure = format!("{runtime}: {}", e.failure());
                let outcome = outcome(
                    task,
                    OutcomeStatus::Blocked,
                    account_problem(runtime, "signed_out"),
                    vec![failure.clone()],
                    OutcomeKind::ServiceDown {
                        runtime: runtime.clone(),
                        problem: "signed_out".into(),
                    },
                );
                (outcome, Some(failure))
            }
            Error::Ferb(FerbError::InputTooLarge {
                chars,
                max_chars,
                runtime,
                ..
            }) => {
                // refused before sending (L28): not the service's error
                let failure = e.failure();
                let outcome = outcome(
                    task,
                    OutcomeStatus::Blocked,
                    too_large_problem(*chars, *max_chars, runtime),
                    vec![],
                    OutcomeKind::Overflowed,
                );
                (outcome, Some(failure))
            }
            _ if e.is_value_error() || matches!(e, Error::Stalled { .. }) => {
                let failure = e.failure();
                if self.runs.is_stalled(worker_id) {
                    let retried = self.runs.retrying_of(worker_id);
                    let stall = fmt_g(self.config.stall_s);
                    let summary = match &retried {
                        Some((runtime, message)) => {
                            retry_stall_problem(runtime, message, self.config.stall_s, &route)
                        }
                        None => format!(
                            "This attempt stopped responding: the AI runtime showed no sign of life for {stall} s, so FERB stopped it."
                        ),
                    };
                    let evidence = format!(
                        "no sign of life for {stall} s, not counting waits on the owner{}",
                        if retried.is_some() {
                            " (only retries)"
                        } else {
                            ""
                        }
                    );
                    (
                        plain(task, OutcomeStatus::Failed, summary, vec![evidence]),
                        Some(failure),
                    )
                } else {
                    // an answer outside FERB's form says what was wrong; another ValueError here is the run's own
                    // words for why it gave no usable report ("runtime finished without an answer")
                    let why = if matches!(e, Error::Validation(_) | Error::Invalid(_)) {
                        reason(&e, Some(&redact), &route)
                    } else {
                        let mut text = e.to_string();
                        if text.is_empty() {
                            text = e.class();
                        }
                        ended(&words(&text, Some(&redact)))
                    };
                    let stop = tree.stop();
                    let attempt = if stop.is_empty() {
                        plain(
                            task,
                            OutcomeStatus::Failed,
                            format!("The report from this attempt wasn't usable: {why}"),
                            vec![failure.clone()],
                        )
                    } else {
                        plain(task, OutcomeStatus::Cancelled, stop, vec![])
                    };
                    (attempt, Some(failure))
                }
            }
            _ => {
                let failure = e.failure();
                let summary = format!(
                    "This attempt couldn't run: {}",
                    reason(&e, Some(&redact), &route)
                );
                (
                    plain(task, OutcomeStatus::Blocked, summary, vec![failure.clone()]),
                    Some(failure),
                )
            }
        }
    }

    /// worker.py:502 `_run`: a call, once for a worker (its tree's orchestrator decides any retry); any other call
    /// is watched for silence too, and one silent from its very start is tried once more; a second silence, or a
    /// call that went quiet after answering in part, fails with that reason. `retry_silent` false (the planner's
    /// turn, ORCH-528): `Stalled` goes to the caller, which tells the owner plainly.
    pub async fn run_call(
        &self,
        rt: &Arc<dyn ModelRuntime>,
        call: Call<'_>,
    ) -> Result<String, Error> {
        if !call.worker_id.is_empty() {
            return self.run_once(rt, &call).await;
        }
        let stall = fmt_g(self.config.stall_s);
        let note = |attempt: u32| {
            let mut input = Map::new();
            input.insert("runtime".into(), json!(rt.name()));
            input.insert("attempt".into(), json!(attempt));
            let s = self.host.span_start(
                "model.stall",
                span_task(call.task),
                Some(call.parent_span),
                None,
                input,
            );
            self.host.span_finish(
                &s,
                None,
                Some(&format!("no sign of life for {stall} s")),
                None,
            );
        };
        if !call.retry_silent {
            let out = self.run_once(rt, &call).await;
            if matches!(out, Err(Error::Stalled { .. })) {
                note(1);
            }
            return out;
        }
        for attempt in 1..=2u32 {
            match self.run_once(rt, &call).await {
                Err(Error::Stalled { progressed }) => {
                    note(attempt);
                    if progressed || attempt == 2 {
                        let twice = if attempt == 2 { " twice" } else { "" };
                        return Err(Error::Service(ServiceError::new(
                            format!(
                                "the AI service ({}) showed no sign of life for {stall} s{twice}",
                                rt.name()
                            ),
                            &rt.name(),
                            true,
                            "",
                        )));
                    }
                }
                other => return other,
            }
        }
        unreachable!("the second silence returned above")
    }

    /// worker.py:541 `_run_once`. Timing (ORCH-526), on the injected clock, never any content: prepare, start,
    /// stream, close, with the number of model calls; each usage report carries its call's timing (the runtime's
    /// own, else one from the events' arrival here).
    async fn run_once(&self, rt: &Arc<dyn ModelRuntime>, call: &Call<'_>) -> Result<String, Error> {
        let host = self.host();
        let task = call.task;
        let tree = call.tree;
        let worker_id = call.worker_id;
        let hooks = &call.hooks;
        let name = rt.name();
        let mono = {
            let h = self.host.clone();
            move || h.clock().monotonic()
        };
        let mut steps = Steps::new(Box::new(mono.clone()));
        let mut arrival = CallTimer::new(Box::new(mono), "arrival");
        let mut calls: u64 = 0;
        let mut start_state: Option<Map<String, Value>> = None;
        let tell_timing =
            |steps: &mut Steps, last: &str, calls: u64, state: &Option<Map<String, Value>>| {
                steps.mark(last);
                let Some(on_timing) = &hooks.on_timing else {
                    return;
                };
                let mut facts = steps.facts();
                facts.insert("model_calls".into(), json!(calls));
                if let Some(Value::Object(held)) = state.as_ref().and_then(|s| s.get("start_steps"))
                {
                    facts.insert("start_steps_ms".into(), Value::Object(held.clone()));
                }
                on_timing(facts);
            };

        let mut ctx = call.context.clone();
        let revision = match tree {
            Some(t) => {
                let steering = t.steering().lock().await;
                let revision = steering.messages.len();
                if task.role == TaskRole::Worker {
                    // a startup failure is a current-attempt failure, not a stale result to retry forever
                    t.set_result_revision(revision);
                }
                if revision > 0 {
                    // internal adapter retries may reuse a context captured before a correction arrived
                    let mut block = ContextBlock::new(
                        steering.messages.join("\n\n"),
                        "latest owner corrections",
                    );
                    block.protected = true;
                    ctx.push(block);
                }
                revision
            }
            None => 0,
        };
        let tool_schemas = if call.token.is_empty() {
            Vec::new()
        } else {
            host.tools_for(&host.validate_token(call.token)?)
        };
        let output_schema = rt.output_schema();
        let limits = rt.limits().map_err(|e| match e {
            FerbError::ModelLimitsUnknown(m) => Error::Service(ServiceError::new(
                format!("no documented context window: {m}"),
                &name,
                false,
                "",
            )),
            other => Error::from(other),
        })?;
        let budget = limits.context_budget();
        let reserved = reserve_tokens(
            self.config.tokenizer,
            &call.system,
            &tool_schemas,
            output_schema.as_ref(),
        );
        let assembly = match assemble_with(
            self.config.tokenizer,
            &ctx,
            budget,
            reserved,
            limits.max_input_chars,
        ) {
            Ok(a) => a,
            Err(AssembleError::TooLarge(FerbError::InputTooLarge {
                chars,
                max_chars,
                what,
                ..
            })) => {
                // refused by name, never sent to be rejected; which block was too big: its reference and size
                let blocks: Vec<Value> = ctx
                    .iter()
                    .filter(|b| !b.text.is_empty())
                    .map(|b| json!({"reference": b.reference, "chars": b.text.chars().count()}))
                    .collect();
                let e = FerbError::InputTooLarge {
                    chars,
                    max_chars,
                    what,
                    runtime: name.clone(),
                };
                let s = host.span_start(
                    "context.too_large",
                    span_task(task),
                    Some(call.parent_span),
                    None,
                    obj(json!({"runtime": name, "blocks": blocks})),
                );
                host.span_finish(
                    &s,
                    Some(obj(json!({"chars": chars, "max_input_chars": max_chars}))),
                    Some(&e.to_string()),
                    None,
                );
                tell_timing(&mut steps, "prepare", calls, &start_state);
                return Err(Error::Ferb(e));
            }
            Err(AssembleError::TooLarge(other)) => return Err(Error::from(other)),
            Err(AssembleError::Arguments(m)) => return Err(Error::Value(m)),
        };
        host.context_exposure(task, &ctx, &assembly, tree.map(|t| t.as_ref()));
        let request = ModelRequest {
            system: call.system.clone(),
            parts: assembly.parts.clone(),
            cacheable: assembly.cacheable,
            images: call.images.clone(),
        };
        let mut input = Map::new();
        input.insert("context".into(), json!(request.context()));
        input.insert("system_prompt".into(), json!(call.system));
        input.insert("context_budget_tokens".into(), json!(budget));
        input.insert(
            "model_limits".into(),
            json!({"model": limits.model, "context_window": limits.context_window,
                   "max_output": limits.max_output, "max_input_chars": limits.max_input_chars,
                   "source": limits.source}),
        );
        input.insert("cacheable_parts".into(), json!(assembly.cacheable));
        if !call.images.is_empty() {
            // the images' evidence rows, never their bytes (L12)
            let refs: Vec<&str> = call.images.iter().map(|i| i.reference.as_str()).collect();
            input.insert("image_evidence".into(), json!(refs));
        }
        host.span_update_input(call.parent_span, input);
        steps.mark("prepare");

        // The stall watchdog covers startup too, and every other call under its own id.
        let watch_id = if worker_id.is_empty() {
            format!("call:{}", call.parent_span)
        } else {
            worker_id.to_string()
        };
        let mut cleanup = Cleanup::new(self, worker_id);
        if worker_id.is_empty() {
            host.activity().start(&watch_id);
            cleanup.watch = Some(watch_id.clone());
        }
        let mut wd = Watchdog::new(
            host,
            &watch_id,
            self.config.heartbeat_poll_s,
            self.config.stall_s,
        );
        let started = {
            let start = rt.start(task, &request, call.token);
            tokio::pin!(start);
            tokio::select! {
                biased;
                out = &mut start => Some(out),
                _ = wd.fire() => {
                    self.runs.mark_stalled(&watch_id);
                    None // the start is dropped: Python's `start.cancel()`
                }
            }
        };
        let started = match started {
            Some(Ok(started)) => started,
            failed => {
                cleanup.armed = false;
                tell_timing(&mut steps, "start", calls, &start_state);
                if worker_id.is_empty() {
                    host.activity().end(&watch_id);
                }
                let stalled = self.runs.is_stalled(&watch_id);
                if worker_id.is_empty() {
                    self.runs.unmark_stalled(&watch_id);
                }
                return Err(match failed {
                    Some(Err(Error::Unavailable {
                        message,
                        account,
                        reason,
                    })) => {
                        // a login problem is never the network's
                        if ACCOUNT_PROBLEMS.contains(&account.as_str()) {
                            host.runtime_account(&name, &account); // RUN-424; Settings → AI
                        }
                        Error::Service(ServiceError::new(
                            message,
                            &name,
                            reason != "auth_required",
                            &account,
                        ))
                    }
                    // an AccountError is a RuntimeUnavailable: the same, with the adapter's own facts
                    Some(Err(Error::Adapter(a)))
                        if a.class == crate::errors::AdapterClass::Account =>
                    {
                        if ACCOUNT_PROBLEMS.contains(&a.account.as_str()) {
                            host.runtime_account(&name, &a.account); // RUN-424; Settings → AI
                        }
                        Error::Service(ServiceError::new(
                            a.message,
                            &name,
                            a.reason != "auth_required",
                            &a.account,
                        ))
                    }
                    Some(Err(e)) => e,
                    // the watchdog's cancellation, not Stop or shutdown
                    _ => {
                        debug_assert!(stalled);
                        Error::Stalled { progressed: false }
                    }
                });
            }
        };
        let StartedRun {
            handle,
            mut events,
            control,
        } = started;
        start_state = Some(handle.state.clone());
        steps.mark("start");
        arrival.begin();
        // which role held the context's parts (P20): "developer" for a cached Codex planner turn
        let role = handle
            .state
            .get("context_role")
            .map(py_str)
            .unwrap_or_else(|| "user".into());
        host.span_update_input(call.parent_span, obj(json!({"context_role": role})));
        let active = ActiveRun {
            runtime: name.clone(),
            handle: handle.clone(),
            control: control.clone(),
        };
        // registered and given its revision under the steering lock, as Python does it in one step
        let mut steering = match tree {
            Some(t) => Some(t.steering().lock().await),
            None => None,
        };
        self.runs.add_active(active.clone());
        cleanup.run = Some((handle.id.clone(), control.clone()));
        let tell: Option<Tell> = match (&hooks.notify, tree) {
            (Some(n), _) => Some(n.clone()),
            (None, Some(t)) => {
                let runs = self.runs.clone();
                let tree_id = t.id();
                Some(Arc::new(move |event: Map<String, Value>| {
                    if let Some(live) = runs.live_sender(&tree_id) {
                        let mut event = event;
                        event.insert("task_id".into(), Value::String(tree_id.clone()));
                        let _ = live.send(event);
                    }
                }))
            }
            (None, None) => None,
        };
        let notice = tell.map(|tell| Arc::new(ConnectionNotice::new(&name, tell, tree.is_some())));
        if !worker_id.is_empty() {
            self.runs.add_worker_run(worker_id, active);
            if let Some(n) = &notice {
                self.runs.set_connection(worker_id, n.clone());
            }
        }
        if let Some(s) = steering.as_mut() {
            s.delivered.insert(handle.id.clone(), revision);
        }

        let mut final_text = String::new();
        let mut made_up: u64 = 0; // RUN-445: calls to tools this run wasn't offered
        let mut progressed = false;
        let body = async {
            if let Some(t) = tree {
                if !t.stop().is_empty() {
                    drop(steering.take());
                    return Err(Error::value("task stopped while the runtime was starting"));
                }
                if let Some(mut s) = steering.take() {
                    let tree_id = t.id();
                    let delivery = deliver(host, &tree_id, &mut s, &handle.id, &handle.task_id, control.as_ref());
                    watched(delivery, &mut wd, &self.runs, &control).await;
                }
            }
            loop {
                let next = watched(events.next(), &mut wd, &self.runs, &control).await;
                let Some(item) = next else { break };
                let ev = item?;
                let kind = ev.kind;
                let data = &ev.data;
                // a retry is no sign of life (Codex retries a lost connection without end)
                if kind != RuntimeEventKind::Retrying {
                    host.activity().beat(&watch_id);
                    if !worker_id.is_empty() {
                        self.runs.take_retrying(worker_id);
                    }
                    progressed = progressed
                        || matches!(
                            kind,
                            RuntimeEventKind::TextDelta
                                | RuntimeEventKind::Final
                                | RuntimeEventKind::ToolCall
                                | RuntimeEventKind::Message
                        );
                }
                if let Some(n) = &notice {
                    if !matches!(
                        kind,
                        RuntimeEventKind::Retrying | RuntimeEventKind::Error | RuntimeEventKind::Violation
                    ) {
                        n.back(host)?; // a real event: the connection is back
                    }
                }
                if matches!(
                    kind,
                    RuntimeEventKind::TextDelta | RuntimeEventKind::Message | RuntimeEventKind::ToolCall
                ) {
                    arrival.output(); // model output, as it reached FERB (ORCH-526)
                }
                match kind {
                    RuntimeEventKind::TextDelta => {
                        if truthy(data, "restart") {
                            if let Some(on_tool) = &hooks.on_tool {
                                on_tool(); // the answer tool's call begins (RUN-447)
                            }
                        }
                        if let Some(on_delta) = &hooks.on_delta {
                            on_delta(&get_str(data, "text", Some("")));
                        }
                    }
                    RuntimeEventKind::ToolCall => {
                        if data.get("offered") == Some(&Value::Bool(false)) {
                            made_up += 1;
                        }
                        if let Some(on_tool) = &hooks.on_tool {
                            on_tool();
                        }
                    }
                    RuntimeEventKind::Usage => {
                        // one usage report per model call: its timing is the runtime's own, else its arrival here
                        calls += 1;
                        let seen = arrival.done();
                        let mut usage = data.clone();
                        if !data.contains_key("timing") {
                            usage.insert("timing".into(), Value::Object(seen));
                        }
                        if let Some(on_usage) = &hooks.on_usage {
                            on_usage(usage.clone());
                        }
                        let s = host.span_start(
                            "model.usage",
                            span_task(task),
                            Some(call.parent_span),
                            None,
                            obj(json!({"runtime": name})),
                        );
                        // traced, never limited
                        host.span_finish(&s, Some(usage), None, data.get("cost_usd").cloned());
                    }
                    RuntimeEventKind::Retrying => {
                        // an error the runtime retries itself: recorded, the run goes on
                        let message = get_str(data, "message", Some(""));
                        let s = host.span_start(
                            "model.retry",
                            span_task(task),
                            Some(call.parent_span),
                            None,
                            obj(json!({"runtime": name})),
                        );
                        host.span_finish(&s, None, Some(&message), None);
                        if !worker_id.is_empty() {
                            self.runs.set_retrying(worker_id, &name, &message);
                        }
                        if let Some(n) = &notice {
                            n.retrying(host, &message)?; // the owner hears at once, once per outage
                        }
                    }
                    RuntimeEventKind::Compacted => {
                        // the runtime compressed the run's memory (RUN-425)
                        let s = host.span_start(
                            "model.compacted",
                            span_task(task),
                            Some(call.parent_span),
                            None,
                            obj(json!({"runtime": name})),
                        );
                        let kept: Map<String, Value> = data
                            .iter()
                            .filter(|(_, v)| !v.is_null())
                            .map(|(k, v)| (k.clone(), v.clone()))
                            .collect();
                        host.span_finish(&s, Some(kept), None, None);
                    }
                    RuntimeEventKind::Violation => {
                        let said = pyvalue::repr(&Py::from_value(&Value::Object(data.clone())));
                        host.on_violation(&name, &said);
                        return Err(Error::Violation(RuntimeViolation {
                            message: format!("{name}: {said}"),
                            runtime: name.clone(),
                            data: data.clone(),
                        }));
                    }
                    RuntimeEventKind::Error if truthy(data, "overflow") => {
                        // the window-exceeded error, as marked
                        let mut e = ServiceError::new(get_str(data, "message", None), &name, false, "");
                        e.kind = ServiceKind::Overflow;
                        return Err(Error::Service(e));
                    }
                    RuntimeEventKind::Error if truthy(data, "image") => {
                        // the provider refused the image, as marked
                        let mut e = ServiceError::new(get_str(data, "message", None), &name, false, "");
                        e.kind = ServiceKind::Image;
                        return Err(Error::Service(e));
                    }
                    RuntimeEventKind::Error if truthy(data, "empty") => {
                        // no answer twice, as marked (RUN-446)
                        return Err(Error::AnswerMissing(AnswerMissing {
                            kind: MissingKind::Empty,
                            message: get_str(data, "message", None),
                            runtime: name.clone(),
                        }));
                    }
                    RuntimeEventKind::Error => {
                        let account = data
                            .get("account")
                            .filter(|v| ferb_py::value::truthy(v))
                            .map(py_str)
                            .unwrap_or_default();
                        let known = ACCOUNT_PROBLEMS.contains(&account.as_str());
                        if known {
                            host.runtime_account(&name, &account); // what the runtime said (RUN-424)
                        }
                        return Err(Error::Service(ServiceError::new(
                            get_str(data, "message", None),
                            &name,
                            account.is_empty(),
                            if known { &account } else { "" },
                        )));
                    }
                    RuntimeEventKind::InvalidAnswer => {
                        // the adapter's own schema check found the answer outside it
                        return Err(Error::Invalid(get_str(data, "message", Some(""))));
                    }
                    RuntimeEventKind::Final => {
                        if let Some(seen) = data.get("tools_seen") {
                            if !worker_id.is_empty() {
                                let names: Vec<String> = match seen {
                                    Value::Array(items) => items.iter().map(py_str).collect(),
                                    Value::String(s) => s.chars().map(String::from).collect(),
                                    _ => Vec::new(),
                                };
                                self.runs.set_tools_seen(worker_id, names);
                            }
                            let s = host.span_start(
                                "runtime.tools",
                                Some(&task.id),
                                Some(call.parent_span),
                                None,
                                obj(json!({"runtime": name})),
                            );
                            host.span_finish(
                                &s,
                                Some(obj(json!({"tools_seen": seen,
                                                "returncode": data.get("returncode").cloned().unwrap_or(Value::Null),
                                                "meaning": TOOLS_MEANING}))),
                                None,
                                None,
                            );
                        }
                        final_text = get_str(data, "text", Some(""));
                        host.runtime_account(&name, ""); // it answered: an earlier refusal no longer stands
                    }
                    _ => {}
                }
                // RUN-445: wrong calls (made-up tools, and lookups the gateway refused once they were spent) stop
                // the run at the cap; the close below cancels it
                let refused = host.calls_refused(&watch_id);
                if made_up + refused >= WRONG_CALLS_CAP && kind != RuntimeEventKind::Final {
                    let s = host.span_start(
                        "model.tool_loop",
                        span_task(task),
                        Some(call.parent_span),
                        None,
                        obj(json!({"runtime": name})),
                    );
                    host.span_finish(
                        &s,
                        Some(obj(json!({"wrong_calls": made_up + refused, "made_up": made_up,
                                        "refused_lookups": refused, "cap": WRONG_CALLS_CAP}))),
                        Some("stopped at the wrong-call cap"),
                        None,
                    );
                    return Err(Error::AnswerMissing(AnswerMissing::tool_loop(
                        &name,
                        made_up + refused,
                    )));
                }
            }
            Ok(())
        }
        .await;
        drop(steering);
        steps.mark("stream");
        drop(wd);
        if worker_id.is_empty() {
            host.activity().end(&watch_id);
        }
        // close: under the steering lock when the call belongs to a tree
        {
            let guard = match tree {
                Some(t) => Some((t, t.steering().lock().await)),
                None => None,
            };
            if let Some((t, mut s)) = guard {
                let applied = s.delivered.shift_remove(&handle.id).unwrap_or(revision);
                if task.role == TaskRole::Worker {
                    t.set_result_revision(applied);
                }
                self.close_run(&handle.id, worker_id, &control).await;
                drop(s);
            } else {
                self.close_run(&handle.id, worker_id, &control).await;
            }
        }
        cleanup.armed = false;
        tell_timing(&mut steps, "close", calls, &start_state);
        body?;
        if self.runs.is_stalled(&watch_id) {
            if worker_id.is_empty() {
                self.runs.unmark_stalled(&watch_id);
            }
            return Err(Error::Stalled { progressed });
        }
        if final_text.is_empty() {
            return Err(Error::value("runtime finished without an answer"));
        }
        match check_final(&final_text, output_schema.as_ref()) {
            Ok(text) => Ok(text),
            Err(CheckError::Invalid(e)) => Err(Error::Invalid(e.0)),
            Err(CheckError::Schema(e)) => Err(Error::Ferb(FerbError::Unsupported(e.to_string()))),
            Err(CheckError::TooDeep) => Err(Error::other(
                "RecursionError",
                "maximum recursion depth exceeded while decoding a JSON document",
            )),
        }
    }

    async fn close_run(&self, run_id: &str, worker_id: &str, control: &Arc<dyn RunControl>) {
        self.runs.remove_active(run_id);
        self.runs.remove_worker_run(worker_id);
        self.runs.remove_connection(worker_id);
        control.cancel().await;
    }
}

/// `(b - a).total_seconds()`: whole microseconds over 10**6.
fn seconds_between(a: &ferb_clock::py::Time, b: &ferb_clock::py::Time) -> f64 {
    let d = *b - *a;
    let us = d.num_microseconds().unwrap_or(i64::MAX);
    us as f64 / 1e6
}

fn plain(task: &Task, status: OutcomeStatus, summary: String, evidence: Vec<String>) -> Attempt {
    outcome(task, status, summary, evidence, OutcomeKind::Plain)
}

fn outcome(
    task: &Task,
    status: OutcomeStatus,
    summary: String,
    evidence: Vec<String>,
    kind: OutcomeKind,
) -> Attempt {
    Attempt::Outcome(AttemptOutcome {
        outcome: Outcome {
            task_id: task.id.clone(),
            status,
            summary,
            artifacts: vec![],
            evidence,
        },
        kind,
    })
}
