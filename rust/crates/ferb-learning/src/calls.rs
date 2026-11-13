//! `calls.py`: one learning model call (distill, profile pass, playbook merge, Mac profile, import recall and
//! verify) through the runtime trait, with its record: one `model.learning` span holding its role, call id, runtime
//! and model; its outcome, the input's size and the runtime's documented limits; the error as the runtime reported
//! it. The context is one protected block within the runtime's limits (over its character cap it is refused, never
//! sent); the answer is checked against the schema.

use std::collections::BTreeSet;

use serde_json::{json, Map, Value};

use ferb_context::{assemble, count_tokens, AssembleError, CheckError, ContextBlock};
use ferb_core::errors::FerbError;
use ferb_core::interfaces::AgentRuntime;
use ferb_core::types::{Identity, ModelLimits, ModelRequest, RuntimeEventKind, Task, TaskRole};

use crate::{Error, Result};

/// A runtime the factory made, with the facts Python reads off it by attribute: its documented limits
/// (`rt.limits`, None when it reports none) and model (`rt.model`). `AgentRuntime` itself carries neither yet.
pub struct Made {
    pub rt: Box<dyn AgentRuntime>,
    pub limits: Option<ModelLimits>,
    pub model: Option<String>,
}

/// The daemon's `make_runtime("worker", mode="supervisor", tool_names=[], schema=...)`.
pub type MakeRuntime<'a> = dyn Fn(&str, &str, &[String], &Value) -> Result<Made> + 'a;

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
