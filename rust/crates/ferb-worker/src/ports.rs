//! The seams: what the worker reaches the rest of FERB through. worker.py is a mixin of the supervisor
//! (`WorkerRuns`, worker.py:272: "Needs from the host: make_runtime, gateway, tokens, spans, tasks, publish, journal,
//! clock, ids, ..."); each thing it reads on `self` is a method of [`WorkerHost`], the model runtime is
//! [`ModelRuntime`], and the part of the supervisor's task tree it reads and writes is [`WorkerTree`].

use std::collections::BTreeSet;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use ferb_clock::clock::ZonedClock;
use ferb_clock::py::Time;
use ferb_context::assemble::{Assembly, ContextBlock};
use ferb_core::errors::FerbError;
use ferb_core::types::{
    Identity, ModelImage, ModelLimits, ModelRequest, RunHandle, RuntimeEvent, Task,
};
use ferb_orchestrator::ports::Policy;
use ferb_orchestrator::steering::{ContinueError, Steering};
use serde_json::{Map, Value};

use crate::errors::Error;

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// What a run token allows, as the worker reads it (auth.py `RunGrant`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Grant {
    pub task_id: String,
    pub worker_id: String,
    pub scopes: BTreeSet<String>,
    pub identity: Identity,
    pub observe_only: bool,
}

/// tree.py:33 `TaskRecord`: a task's live record.
#[derive(Clone, Debug, PartialEq)]
pub struct TaskRecord {
    pub task: Task,
    pub started_at: Time,
    /// The task's root span: tool spans hang under it.
    pub span_id: String,
    /// The request's root task.
    pub tree_id: String,
}

/// Per-worker liveness (guards.py `Activity`): the gateway reports waits on the owner, runtimes report events.
pub trait Liveness: Send + Sync {
    fn start(&self, worker: &str);
    fn beat(&self, worker: &str);
    fn waiting(&self, worker: &str, on: bool);
    fn stalled(&self, worker: &str, stall_s: f64) -> bool;
    fn end(&self, worker: &str);
}

impl Liveness for ferb_orchestrator::guards::Activity {
    fn start(&self, worker: &str) {
        ferb_orchestrator::guards::Activity::start(self, worker)
    }
    fn beat(&self, worker: &str) {
        ferb_orchestrator::guards::Activity::beat(self, worker)
    }
    fn waiting(&self, worker: &str, on: bool) {
        ferb_orchestrator::guards::Activity::waiting(self, worker, on)
    }
    fn stalled(&self, worker: &str, stall_s: f64) -> bool {
        ferb_orchestrator::guards::Activity::stalled(self, worker, stall_s)
    }
    fn end(&self, worker: &str) {
        ferb_orchestrator::guards::Activity::end(self, worker)
    }
}

/// Steering and cancel of one started run, usable while its events are being read (Python: `runtime.continue_` and
/// `runtime.cancel(run)`).
pub trait RunControl: Send + Sync {
    /// Same-run input: accepted, `Unsupported` (the update stays pending), or another failure by its class name.
    fn steer<'a>(&'a self, message: &'a str) -> BoxFuture<'a, Result<(), ContinueError>>;
    /// Ends the run; called once by the watchdog or a stop and again when the run closes, as Python does.
    fn cancel(&self) -> BoxFuture<'_, ()>;
}

/// A started run's events, pulled one at a time (Python's async generator). An `Err` is what the generator raised;
/// the stream ends after it.
pub trait EventSource: Send {
    fn next(&mut self) -> BoxFuture<'_, Option<Result<RuntimeEvent, Error>>>;
}

/// What a runtime's `start` hands back: the handle (its `state` holds facts such as `start_steps` and
/// `context_role`), the events and the control.
pub struct StartedRun {
    pub handle: RunHandle,
    pub events: Box<dyn EventSource>,
    pub control: Arc<dyn RunControl>,
}

/// One model runtime as `make_runtime` builds it for one call (types.py `AgentRuntime`, plus the two attributes
/// worker.py reads with `getattr`: `limits` and `output_schema`).
pub trait ModelRuntime: Send + Sync {
    fn name(&self) -> String;
    /// `runtime_limits(rt)` (context.py:165): the documented limits, or `ModelLimitsUnknown`.
    fn limits(&self) -> Result<ModelLimits, FerbError>;
    /// `getattr(rt, "output_schema", None)`: the schema the runtime checks its answer against.
    fn output_schema(&self) -> Option<Value>;
    fn start<'a>(
        &'a self,
        task: &'a Task,
        request: &'a ModelRequest,
        run_token: &'a str,
    ) -> BoxFuture<'a, Result<StartedRun, Error>>;
}

/// The part of the supervisor's `_Tree` the worker reads and writes. Python reads these attributes directly; the
/// supervisor's tree implements this trait (shared across tasks, so interior mutability is the tree's).
pub trait WorkerTree: Send + Sync {
    fn id(&self) -> String;
    /// `tree.worker.id`: the step's own task id, which its live line carries (ORCH-531).
    fn worker_task_id(&self) -> String;
    /// `tree.stop`: why the tree stopped; "" while it runs.
    fn stop(&self) -> String;
    /// `tree.runtime`: the owner's one-tap runtime switch (ORCH-315); "" for none.
    fn runtime(&self) -> String;
    /// `tree.spec.worker_type`.
    fn worker_type(&self) -> String;
    /// `tree.start_block`: what the owner saw at send (L12).
    fn start_block(&self) -> Option<ContextBlock>;
    /// `tree.start_image`: the start-of-task screenshot (D26).
    fn start_image(&self) -> Option<ModelImage>;
    /// `tree.result_revision = n`: how many corrections the latest worker result was made with (ORCH-509).
    fn set_result_revision(&self, revision: usize);
    /// `tree.steering`: the tree's corrections and how many each run has been given. The mutex is Python's
    /// `steering.lock`: delivering holds it, and every other read takes it briefly (the `lock` field inside
    /// `Steering` is not used).
    fn steering(&self) -> &tokio::sync::Mutex<Steering>;
}

/// Everything else the worker reads on `self` in Python.
pub trait WorkerHost: Send + Sync {
    // --- time, ids, this Mac ---
    fn clock(&self) -> &dyn ZonedClock;
    /// `ids.new_id()`.
    fn new_id(&self) -> String;
    fn device_id(&self) -> String;
    /// ferb/sense/network.py `has_route()`: `Some(false)` only when this Mac knows it has no route.
    fn has_route(&self) -> Option<bool>;
    /// ferb/policy's words, for the live lines (`progress.describe`).
    fn policy(&self) -> &dyn Policy;

    // --- the runtime factory ---
    /// `make_runtime(role, mode=..., tool_names=..., schema=..., effort=...)`.
    fn make_runtime(
        &self,
        role: &str,
        mode: &str,
        tool_names: &[String],
        schema: &Value,
        effort: Option<&str>,
    ) -> Result<Arc<dyn ModelRuntime>, Error>;

    // --- the gateway ---
    /// `gateway.tools_for(grant)`: the rows `{name, description, input_schema}`, registry order.
    fn tools_for(&self, grant: &Grant) -> Vec<Value>;
    /// `gateway.redact`: text or a record, redacted. Python's redactor can raise; a failure is the class it raised.
    fn redact(&self, value: &Value) -> Result<Value, Error>;
    fn initial_image_given(&self, task_id: &str);
    /// `gateway.forget_calls(worker_id)`: a run ended; its grant's counts go.
    fn forget_calls(&self, worker_id: &str);
    /// `gateway.calls_refused(watch_id)`: lookups refused after they were spent (RUN-445).
    fn calls_refused(&self, watch_id: &str) -> u64;

    // --- run tokens (auth.py `RunTokens`) ---
    fn issue_token(
        &self,
        task_id: &str,
        worker_id: &str,
        scopes: &BTreeSet<String>,
        identity: Identity,
        ttl_s: f64,
        observe_only: bool,
    ) -> String;
    /// `tokens.validate(token)`: its grant, or `AuthError`.
    fn validate_token(&self, token: &str) -> Result<Grant, Error>;
    fn revoke_worker(&self, worker_id: &str);

    // --- spans (store/spans.py `SpanStore`) ---
    /// `spans.start(kind, input=..., task_id=..., parent_id=..., device_id=...)`: the new span's id.
    fn span_start(
        &self,
        kind: &str,
        task_id: Option<&str>,
        parent_id: Option<&str>,
        device_id: Option<&str>,
        input: Map<String, Value>,
    ) -> String;
    fn span_finish(
        &self,
        span_id: &str,
        output: Option<Map<String, Value>>,
        error: Option<&str>,
        cost_usd: Option<Value>,
    );
    fn span_update_input(&self, span_id: &str, values: Map<String, Value>);

    // --- tasks (tree.py `TaskTree`) ---
    /// `tasks.get(task_id) is not None`.
    fn task_known(&self, task_id: &str) -> bool;
    fn task_add(&self, record: &TaskRecord, status: &str);
    fn task_set_span(&self, task_id: &str, span_id: &str);
    /// The supervisor's `_set_status(task_id, status, extra)`.
    fn set_status(&self, task_id: &str, status: &str, extra: Map<String, Value>);

    // --- the bus, the journal, liveness ---
    fn publish(&self, topic: &str, data: Map<String, Value>);
    /// `journal.append(kind, data)`.
    fn journal(&self, kind: &str, data: Value);
    fn activity(&self) -> &dyn Liveness;

    // --- what the supervisor knows ---
    /// `signals()`: when the owner was last active and the last trail event.
    fn signals(&self) -> (Option<Time>, Option<Time>);
    /// `browser_context(task_id)`: the checker's current browser observations.
    fn browser_context(&self, task_id: &str) -> String;
    /// `apps()`: installed (bundle id, name).
    fn apps(&self) -> Result<Vec<(String, String)>, Error>;
    /// `running_apps()`: (bundle id, name, pid).
    fn running_apps(&self) -> Vec<(String, String, i64)>;
    /// `shell_process(id)`: a task process's record (`owner_task`, `command`, `started`).
    fn shell_process(&self, id: &str) -> Result<Option<Map<String, Value>>, Error>;
    /// `on_violation(runtime, text)`.
    fn on_violation(&self, runtime: &str, text: &str);
    /// `runtime_account(runtime, account)` (RUN-424): what the runtime said about its account; "" clears it.
    fn runtime_account(&self, runtime: &str, account: &str);
    /// The supervisor's `_system(prompt, roles=roles)`: a role's instructions (ferb-supervisor `prompts::system`).
    fn system(&self, prompt: &str, roles: &[String]) -> String;
    /// The supervisor's `_with_newly_always_on(ctx)` (MEM-429).
    fn with_newly_always_on(&self, ctx: Vec<ContextBlock>) -> Vec<ContextBlock>;
    /// The supervisor's `_context_exposure(task, ctx, assembly, tree)`: what a run was shown, for its records.
    fn context_exposure(
        &self,
        task: &Task,
        ctx: &[ContextBlock],
        assembly: &Assembly,
        tree: Option<&dyn WorkerTree>,
    );
}
