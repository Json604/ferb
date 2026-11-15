//! `calls.py`: one learning model call (distill, profile pass, playbook merge, Mac profile, import recall and
//! verify) through the runtime trait, with its record: one `model.learning` span holding its role, call id, runtime
//! and model; its outcome, the input's size and the runtime's documented limits; the error as the runtime reported
//! it. The context is one protected block within the runtime's limits (over its character cap it is refused, never
//! sent); the answer is checked against the schema.

use std::collections::BTreeSet;

use serde_json::{json, Map, Value};

use ferb_context::{assemble, count_tokens, AssembleError, CheckError, ContextBlock};
use ferb_core::errors::FerbError;
use ferb_core::interfaces::{AgentRuntime, BoxFuture};
use ferb_core::types::{
    Identity, ModelLimits, ModelRequest, RunHandle, RuntimeEvent, RuntimeEventKind, Task, TaskRole,
};
use ferb_orchestrator::steering::ContinueError;
use ferb_worker::ports::{EventSource, ModelRuntime, RunControl};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;

use crate::{Error, Result};

/// A runtime the factory made, with the facts Python reads off it by attribute: its documented limits
/// (`rt.limits`, None when it reports none) and model (`rt.model`). `AgentRuntime` itself carries neither; a
/// `ModelRuntime` does ([`Made::from_model`]).
pub struct Made {
    pub rt: Box<dyn AgentRuntime>,
    pub limits: Option<ModelLimits>,
    pub model: Option<String>,
}

/// The daemon's `make_runtime("worker", mode="supervisor", tool_names=[], schema=...)`.
pub type MakeRuntime<'a> = dyn Fn(&str, &str, &[String], &Value) -> Result<Made> + 'a;

/// The same factory over the runtime interface every caller shares (ferb-worker's `ModelRuntime`, which carries
/// its limits): a daemon that builds `ModelRuntime`s gives the learners one through [`Made::from_model`].
pub type MakeModelRuntime<'a> =
    dyn Fn(&str, &str, &[String], &Value) -> Result<Arc<dyn ModelRuntime>> + 'a;

impl Made {
    /// A `ModelRuntime` as a learning call's runtime: its documented limits (`rt.limits`; None when it reports none,
    /// as `ModelLimitsUnknown`) and their model (`rt.model`) are read once here.
    pub fn from_model(rt: Arc<dyn ModelRuntime>) -> Made {
        let limits = rt.limits().ok();
        let model = limits.as_ref().map(|l| l.model.clone());
        Made {
            rt: Box::new(ModelAsAgent::new(rt)),
            limits,
            model,
        }
    }
}

/// A started run's events (until `events` takes them) and its control.
type Live = (Option<Box<dyn EventSource>>, Arc<dyn RunControl>);

/// ferb-worker's `ModelRuntime` behind `ferb_core`'s older `AgentRuntime`, which [`learning_call`] drives: each run's
/// events are pumped into the channel `events` returns, and what the stream raised arrives as an `error` event with
/// `raised` set to its class (as ferb-runtimes' `CoreRuntime` does).
pub struct ModelAsAgent {
    rt: Arc<dyn ModelRuntime>,
    name: String,
    runs: Mutex<HashMap<String, Live>>,
}

impl ModelAsAgent {
    pub fn new(rt: Arc<dyn ModelRuntime>) -> ModelAsAgent {
        ModelAsAgent {
            name: rt.name(),
            rt,
            runs: Mutex::new(HashMap::new()),
        }
    }

    fn control(&self, run: &RunHandle) -> Option<Arc<dyn RunControl>> {
        self.runs
            .lock()
            .ok()
            .and_then(|r| r.get(&run.id).map(|(_, c)| c.clone()))
    }
}

impl AgentRuntime for ModelAsAgent {
    fn name(&self) -> &str {
        &self.name
    }

    fn start<'a>(
        &'a self,
        task: &'a Task,
        request: &'a ModelRequest,
        run_token: &'a str,
    ) -> BoxFuture<'a, std::result::Result<RunHandle, FerbError>> {
        Box::pin(async move {
            let started = self
                .rt
                .start(task, request, run_token)
                .await
                .map_err(|e| match e {
                    ferb_worker::Error::Ferb(f) => f,
                    ferb_worker::Error::Unavailable { message, .. } => {
                        FerbError::RuntimeUnavailable(message)
                    }
                    other => FerbError::Ferb(other.to_string()),
                })?;
            if let Ok(mut runs) = self.runs.lock() {
                runs.insert(
                    started.handle.id.clone(),
                    (Some(started.events), started.control),
                );
            }
            Ok(started.handle)
        })
    }

    fn events(&self, run: &RunHandle) -> mpsc::Receiver<RuntimeEvent> {
        let (tx, rx) = mpsc::channel(1);
        let stream = self
            .runs
            .lock()
            .ok()
            .and_then(|mut r| r.get_mut(&run.id).and_then(|(s, _)| s.take()));
        if let Some(mut stream) = stream {
            tokio::spawn(async move {
                while let Some(item) = stream.next().await {
                    let ev = match item {
                        Ok(ev) => ev,
                        Err(e) => {
                            let mut data = Map::new();
                            data.insert("message".into(), Value::String(e.to_string()));
                            data.insert("raised".into(), Value::String(e.class()));
                            RuntimeEvent {
                                kind: RuntimeEventKind::Error,
                                data,
                            }
                        }
                    };
                    if tx.send(ev).await.is_err() {
                        return;
                    }
                }
            });
        }
        rx
    }

    fn continue_run<'a>(
        &'a self,
        run: &'a RunHandle,
        message: &'a str,
    ) -> BoxFuture<'a, std::result::Result<(), FerbError>> {
        Box::pin(async move {
            match self.control(run) {
                Some(c) => c.steer(message).await.map_err(|e| match e {
                    ContinueError::Unsupported => {
                        FerbError::Unsupported("the run can't take more input".into())
                    }
                    ContinueError::Failed(class) => FerbError::Ferb(class),
                }),
                None => Err(FerbError::Unsupported("the run has already ended".into())),
            }
        })
    }

    fn cancel<'a>(&'a self, run: &'a RunHandle) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            let control = self
                .runs
                .lock()
                .ok()
                .and_then(|mut r| r.remove(&run.id).map(|(_, c)| c));
            if let Some(c) = control {
                c.cancel().await;
            }
        })
    }
}

/// Where each call is recorded (the daemon's span store).
pub trait Spans {
    fn start(
        &self,
        kind: &str,
        task_id: Option<&str>,
        input: Value,
        parent_id: Option<&str>,
    ) -> String;
    fn finish(&self, id: &str, output: Option<Value>, error: Option<String>);
}

/// `Task(ident, None, goal, device_id, Identity.OWNER, set(), "verifier", request_id, [], None)`.
pub fn learning_task(id: &str, goal: &str, device_id: &str, request_id: &str) -> Task {
    Task {
        id: id.into(),
        parent_id: None,
        goal: goal.into(),
        device_id: device_id.into(),
        identity: Identity::Owner,
        scopes: BTreeSet::new(),
        role: TaskRole::Verifier,
        request_id: request_id.into(),
        success_conditions: Vec::new(),
        deadline: None,
    }
}

/// calls.py:18 `learning_call`: the answer text, checked against `schema`.
#[allow(clippy::too_many_arguments)]
pub async fn learning_call(
    made: &Made,
    task: &Task,
    system: &str,
    ctx: &str,
    reference: &str,
    schema: &Value,
    role: &str,
    spans: Option<&dyn Spans>,
    usage: &dyn Fn(&str, &Value),
) -> Result<String> {
    let rt = &made.rt;
    let span = spans.map(|s| {
        s.start(
            "model.learning",
            None,
            json!({"role": role, "call": task.id, "goal": task.goal, "runtime": rt.name(), "model": made.model}),
            None,
        )
    });
    let mut facts = Map::new();
    let out = run(
        made,
        task,
        system,
        ctx,
        reference,
        schema,
        spans,
        span.as_deref(),
        usage,
        &mut facts,
    )
    .await;
    if let (Some(s), Some(id)) = (spans, span.as_deref()) {
        let outcome = if out.is_ok() { "answered" } else { "failed" };
        let mut output = Map::new();
        output.insert("outcome".into(), json!(outcome));
        output.extend(facts);
        s.finish(
            id,
            Some(Value::Object(output)),
            out.as_ref().err().map(Error::described),
        );
    }
    out
}

#[allow(clippy::too_many_arguments)]
async fn run(
    made: &Made,
    task: &Task,
    system: &str,
    ctx: &str,
    reference: &str,
    schema: &Value,
    spans: Option<&dyn Spans>,
    span: Option<&str>,
    usage: &dyn Fn(&str, &Value),
    facts: &mut Map<String, Value>,
) -> Result<String> {
    let rt = &made.rt;
    let limits = ferb_context::assemble::runtime_limits(Some(rt.name()), made.limits.as_ref())
        .map_err(Error::Ferb)?;
    facts.insert(
        "context_budget_tokens".into(),
        json!(limits.context_budget()),
    );
    facts.insert("max_input_chars".into(), json!(limits.max_input_chars));
    let block = ContextBlock {
        protected: true,
        ..ContextBlock::new(ctx, reference)
    };
    let out = match assemble(
        &[block],
        limits.context_budget(),
        count_tokens(system) as i64,
        limits.max_input_chars,
    ) {
        Ok(out) => out,
        Err(AssembleError::TooLarge(e)) => {
            if let FerbError::InputTooLarge { chars, .. } = &e {
                facts.insert("input_chars".into(), json!(chars));
            }
            return Err(Error::Ferb(e));
        }
        Err(AssembleError::Arguments(m)) => return Err(Error::Value(m)),
    };
    facts.insert("input_chars".into(), json!(out.chars));
    facts.insert("input_tokens_estimate".into(), json!(out.tokens));
    let request = ModelRequest {
        system: system.into(),
        parts: out.parts.clone(),
        cacheable: out.cacheable,
        images: Vec::new(),
    };
    let handle = rt.start(task, &request, "").await.map_err(Error::Ferb)?;
    let mut final_text = String::new();
    let mut events = rt.events(&handle);
    let mut failed: Option<Error> = None;
    while let Some(event) = events.recv().await {
        let message = || {
            event
                .data
                .get("message")
                .filter(|v| ferb_py::value::truthy(v))
                .map_or_else(
                    || crate::py::py_str(&Value::Object(event.data.clone())),
                    crate::py::py_str,
                )
        };
        match event.kind {
            RuntimeEventKind::Final => {
                final_text = event
                    .data
                    .get("text")
                    .map_or_else(String::new, crate::py::py_str)
            }
            RuntimeEventKind::Usage => usage(rt.name(), &Value::Object(event.data.clone())),
            RuntimeEventKind::Retrying => {
                if let (Some(s), Some(parent)) = (spans, span) {
                    let retry = s.start(
                        "model.retry",
                        None,
                        json!({"runtime": rt.name()}),
                        Some(parent),
                    );
                    let text = event
                        .data
                        .get("message")
                        .map_or_else(String::new, crate::py::py_str);
                    s.finish(&retry, None, Some(text));
                }
            }
            RuntimeEventKind::Error | RuntimeEventKind::Violation => {
                failed = Some(Error::Runtime(message()));
                break;
            }
            RuntimeEventKind::InvalidAnswer => {
                let text = event
                    .data
                    .get("message")
                    .map_or_else(String::new, crate::py::py_str);
                failed = Some(Error::InvalidAnswer(text));
                break;
            }
            _ => {}
        }
    }
    rt.cancel(&handle).await;
    if let Some(e) = failed {
        return Err(e);
    }
    match ferb_context::check_final(&final_text, Some(schema)) {
        Ok(answer) => Ok(answer),
        Err(CheckError::Invalid(e)) => Err(Error::InvalidAnswer(e.0)),
        Err(CheckError::Schema(e)) => Err(Error::Crash("Unsupported", e.to_string())),
        Err(CheckError::TooDeep) => Err(Error::Crash(
            "RecursionError",
            "maximum recursion depth exceeded".into(),
        )),
    }
}

/// `json.loads(answer)` of a checked answer (it is JSON by then).
pub fn loads(text: &str) -> Result<Value> {
    ferb_py::json::loads(text).map_err(Error::Json)
}

/// `f"{text}@{sha256(system)[:8]}"`: the prompt part of a `made_by`.
pub fn prompt_tag(path: &str, system: &str) -> String {
    format!("prompt:{path}@{}", &crate::py::sha256_hex(system)[..8])
}

#[cfg(test)]
mod model_runtime {
    use super::*;
    use ferb_worker::ports::StartedRun;

    struct Events(Vec<std::result::Result<RuntimeEvent, ferb_worker::Error>>);
    impl EventSource for Events {
        fn next(
            &mut self,
        ) -> ferb_worker::ports::BoxFuture<
            '_,
            Option<std::result::Result<RuntimeEvent, ferb_worker::Error>>,
        > {
            let next = (!self.0.is_empty()).then(|| self.0.remove(0));
            Box::pin(async move { next })
        }
    }

    struct Control;
    impl RunControl for Control {
        fn steer<'a>(
            &'a self,
            _m: &'a str,
        ) -> ferb_worker::ports::BoxFuture<'a, std::result::Result<(), ContinueError>> {
            Box::pin(async { Err(ContinueError::Unsupported) })
        }
        fn cancel(&self) -> ferb_worker::ports::BoxFuture<'_, ()> {
            Box::pin(async {})
        }
    }

    struct Rt;
    impl ModelRuntime for Rt {
        fn name(&self) -> String {
            "claude".into()
        }
        fn limits(&self) -> std::result::Result<ModelLimits, FerbError> {
            Ok(ModelLimits {
                model: "invented-model".into(),
                context_window: 1000,
                max_output: 100,
                source: "test".into(),
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
            _r: &'a ModelRequest,
            _t: &'a str,
        ) -> ferb_worker::ports::BoxFuture<'a, std::result::Result<StartedRun, ferb_worker::Error>>
        {
            Box::pin(async move {
                let mut data = Map::new();
                data.insert("text".into(), json!("hello"));
                Ok(StartedRun {
                    handle: RunHandle {
                        id: "run-1".into(),
                        runtime: "claude".into(),
                        task_id: task.id.clone(),
                        state: Default::default(),
                    },
                    events: Box::new(Events(vec![
                        Ok(RuntimeEvent {
                            kind: RuntimeEventKind::Final,
                            data,
                        }),
                        Err(ferb_worker::Error::other("KeyError", "'x'")),
                    ])),
                    control: Arc::new(Control),
                })
            })
        }
    }

    #[tokio::test]
    async fn a_model_runtime_drives_a_learning_call_through_the_old_trait() {
        let made = Made::from_model(Arc::new(Rt));
        assert_eq!(made.model.as_deref(), Some("invented-model"));
        assert_eq!(made.limits.as_ref().map(|l| l.context_window), Some(1000));
        assert_eq!(made.rt.name(), "claude");
        let task = learning_task("call-1", "goal", "dev", "req");
        let request = ModelRequest {
            system: "system".into(),
            parts: vec!["part".into()],
            cacheable: 0,
            images: vec![],
        };
        let run = made.rt.start(&task, &request, "").await.unwrap();
        let mut events = made.rt.events(&run);
        let first = events.recv().await.unwrap();
        assert_eq!(first.kind, RuntimeEventKind::Final);
        let second = events.recv().await.unwrap();
        assert_eq!(second.kind, RuntimeEventKind::Error);
        assert_eq!(second.data["raised"], "KeyError");
        assert!(events.recv().await.is_none());
        assert!(matches!(
            made.rt.continue_run(&run, "more").await,
            Err(FerbError::Unsupported(_))
        ));
        made.rt.cancel(&run).await;
    }
}
