//! FERB's own tool loop for direct provider APIs (ferb/runtimes/api.py; D32, RUN-415, RUN-419, RUN-421, RUN-423,
//! RUN-427, RUN-436..440, RUN-445..448): ask the model, run each tool call through FERB's gateway (the `Bridge`,
//! the same path as the MCP shim), send the results back, until the model answers. One `Provider` per API
//! translates requests, responses, cache markers, typed output and usage. Keys are read at start and never appear
//! in errors; a provider's error words are cut and stripped of the key before they are kept.

pub mod anthropic;
pub mod google;
pub mod openai;

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ferb_core::errors::FerbError;
use ferb_core::timing::CallTimer;
use ferb_core::types::{ModelLimits, ModelRequest, RunHandle, RuntimeEventKind as K, Task};
use ferb_identity::Secret;
use serde_json::{json, Map, Value};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio::time::Instant;

use crate::answer_rules::{
    is_empty, ANSWER_NOT_TAKEN, ANSWER_TOOL, ANSWER_TOOL_DESC, FOLLOW_UP, NOT_RUN_WITH_ANSWER,
    PLANNER_ANSWER,
};
use crate::codex::mcp_name;
use crate::error::RuntimeError;
use crate::forms::{repair_note, stated, unfenced, Forms, Refusals, SchemaTooComplex, Shape};
use crate::gen::{Closed, EventStream, Yielder};
use crate::run::{event, RunControl, Started};
use crate::usage::Usage;

/// The `limits_for` every provider module shares (L28, RUN-428): a model the owner typed gets the smallest limits
/// FERB documents for that provider, said in the source.
pub fn documented_limits(
    model: &str,
    table: &[(&str, i64, i64)],
    source: &str,
    provider: &str,
    window_includes_output: bool,
) -> ModelLimits {
    match table.iter().find(|(m, _, _)| *m == model) {
        Some((_, w, o)) => ModelLimits {
            model: model.into(),
            context_window: *w,
            max_output: *o,
            source: source.into(),
            window_includes_output,
            max_input_chars: None,
        },
        None => ModelLimits {
            model: model.into(),
            context_window: table.iter().map(|(_, w, _)| *w).min().unwrap_or(0),
            max_output: table.iter().map(|(_, _, o)| *o).min().unwrap_or(0),
            source: format!(
                "not documented to FERB for {model}: the smallest limits it knows for {provider} models ({source})"
            ),
            window_includes_output,
            max_input_chars: None,
        },
    }
}

pub const RETRY_STATUS: [u16; 8] = [408, 409, 429, 500, 502, 503, 504, 529];

/// api.py ACCOUNT_STATUS: statuses that are account facts no model can change.
pub fn account_status(status: u16) -> &'static str {
    match status {
        401 | 403 => "signed_out",
        402 | 429 => "usage_limit",
        _ => "",
    }
}

/// api.py `http_cause` (RUN-1): the plain cause from the status code alone.
pub fn http_cause(status: u16) -> &'static str {
    match status {
        400 => "it refused the request FERB sent",
        401 | 403 => "it didn't accept FERB's API key",
        402 => "the account's usage limit is reached",
        404 => "it doesn't offer the model FERB asked for to this API key: pick another model or service in Settings → AI",
        408 => "it timed out",
        413 => "the request was too large for it",
        429 => "the account's usage or rate limit is reached",
        s if s >= 500 => "it had a problem on its side",
        _ => "it refused the request",
    }
}

/// Python's `format(x, 'g')`.
pub fn py_g(x: f64) -> String {
    if x.is_nan() {
        return "nan".into();
    }
    if x.is_infinite() {
        return if x > 0.0 { "inf" } else { "-inf" }.into();
    }
    if x == 0.0 {
        return if x.is_sign_negative() { "-0" } else { "0" }.into();
    }
    let exp = x.abs().log10().floor() as i32;
    let sci = format!("{:.5e}", x);
    let (mantissa, e) = sci.split_once('e').unwrap_or((&sci, "0"));
    let e: i32 = e.parse().unwrap_or(exp);
    if !(-4..6).contains(&e) {
        let m = mantissa.trim_end_matches('0').trim_end_matches('.');
        return format!("{m}e{}{:02}", if e < 0 { '-' } else { '+' }, e.abs());
    }
    let decimals = (5 - e).max(0) as usize;
    let s = format!("{:.*}", decimals, x);
    if s.contains('.') {
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    } else {
        s
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ToolSpec {
    /// As the model sees it (dots become underscores).
    pub name: String,
    pub description: String,
    pub schema: Value,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: Value,
    /// Set when the model's arguments weren't a JSON object.
    pub arguments_error: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ToolOutcome {
    pub call: ToolCall,
    pub text: String,
    pub is_error: bool,
    /// {"data": base64, "mime": "image/png"}.
    pub image: Option<Value>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Turn {
    pub calls: Vec<ToolCall>,
    pub final_text: Option<String>,
    pub usage: Usage,
    pub stop: String,
    /// The kinds of the call's output pieces as the provider names them (RUN-446).
    pub blocks: Vec<String>,
}

/// A request to send: URL, headers (in order) and the JSON body.
pub type HttpRequest = (String, Vec<(String, String)>, Value);

/// One streamed event's fate.
pub enum FeedError {
    /// Python's KeyError/TypeError/ValueError in the parser: "malformed response stream event".
    Malformed,
    /// The parser's own ProviderError.
    Provider(String),
}

/// One streamed reply being assembled (each provider's `*Stream`).
pub trait StreamParser: Send {
    /// The answer text this event adds ("" for none).
    fn feed(&mut self, event: &Value) -> Result<String, FeedError>;
    /// Whether the answer tool's call started with the last event (read and cleared).
    fn take_answer_began(&mut self) -> bool;
    /// The whole reply, as the unstreamed API would have answered.
    fn finish(&mut self) -> Result<Value, RuntimeError>;
}

/// api.py `Provider`: one API's translation.
pub trait Provider: Send + Sync {
    fn name(&self) -> &str;
    fn model(&self) -> &str;
    /// FERB's output schema in the form this provider accepts (None: as it is).
    fn shape(
        &self,
        _forms: &dyn Forms,
        _schema: &Value,
    ) -> Option<Result<Shape, SchemaTooComplex>> {
        None
    }
    fn limits(&self) -> Option<ModelLimits>;
    fn begin(
        &self,
        request: &ModelRequest,
        tools: &[ToolSpec],
        schema: Option<&Value>,
        effort: Option<&str>,
        cache_key: &str,
    ) -> Value;
    fn http(&self, state: &Value) -> HttpRequest;
    /// The streamed request (None: this provider isn't streamed).
    fn stream_http(&self, _state: &Value) -> Option<HttpRequest> {
        None
    }
    fn stream_parser(&self, _answer: &str) -> Option<Box<dyn StreamParser>> {
        None
    }
    fn auth(&self, key: &str) -> Vec<(String, String)>;
    fn parse(&self, state: &mut Value, data: &Value, today: &str) -> Result<Turn, RuntimeError>;
    fn add_results(&self, state: &mut Value, turn: &Turn, results: &[ToolOutcome]);
    fn window_exceeded(&self, status: u16, error: &Map<String, Value>) -> bool;
    fn image_refused(&self, status: u16, error: &Map<String, Value>) -> bool;
    fn add_message(&self, state: &mut Value, message: &str);
    /// RUN-427 / RUN-447: the request without its typed format, the answer form stated and the answer tool offered.
    /// False when the provider has no such form (then a refused format is an error as any other).
    fn without_format(
        &self,
        _forms: &dyn Forms,
        _state: &mut Value,
        _statement: &str,
        _tool: Option<&ToolSpec>,
    ) -> bool {
        false
    }
    fn has_without_format(&self) -> bool {
        false
    }
}

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// api.py `Bridge`: FERB's gateway for the tools a run token covers.
pub trait Bridge: Send + Sync {
    fn tools(&self, run_token: &str) -> Vec<Value>;
    fn call<'a>(
        &'a self,
        run_token: &'a str,
        name: &'a str,
        arguments: &'a Value,
    ) -> BoxFuture<'a, Result<Value, FerbError>>;
}

/// api.py `inline_refs`: local `#/$defs` references written out; a reference already being expanded becomes a plain
/// object, so recursive models can't loop.
pub fn inline_refs(schema: &Value) -> Value {
    let defs = schema
        .get("$defs")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    fn walk(node: &Value, stack: &[String], defs: &Map<String, Value>) -> Value {
        match node {
            Value::Object(m) => {
                if let Some(Value::String(r)) = m.get("$ref") {
                    if let Some(name) = r.strip_prefix("#/$defs/") {
                        if stack.iter().any(|s| s == name) || !defs.contains_key(name) {
                            return json!({"type": "object"});
                        }
                        let mut deeper = stack.to_vec();
                        deeper.push(name.to_string());
                        let expanded = walk(&defs[name], &deeper, defs);
                        let Value::Object(mut out) = expanded else {
                            return expanded;
                        };
                        for (k, v) in m {
                            if k != "$ref" && k != "$defs" {
                                out.insert(k.clone(), walk(v, stack, defs));
                            }
                        }
                        return Value::Object(out);
                    }
                }
                Value::Object(
                    m.iter()
                        .filter(|(k, _)| k.as_str() != "$defs")
                        .map(|(k, v)| (k.clone(), walk(v, stack, defs)))
                        .collect(),
                )
            }
            Value::Array(items) => {
                Value::Array(items.iter().map(|v| walk(v, stack, defs)).collect())
            }
            other => other.clone(),
        }
    }
    walk(schema, &[], &defs)
}

pub type Sleep = Arc<dyn Fn(f64) -> BoxFuture<'static, ()> + Send + Sync>;
pub type KeyFn = Arc<dyn Fn(String) -> BoxFuture<'static, Option<Secret>> + Send + Sync>;

/// api.py `ApiRuntime`.
#[derive(Clone)]
pub struct ApiRuntime {
    pub provider: Arc<dyn Provider>,
    pub supervisor: bool,
    pub role: String,
    pub tool_names: Vec<String>,
    pub output_schema: Option<Value>,
    pub bridge: Option<Arc<dyn Bridge>>,
    pub key: KeyFn,
    pub sleep: Sleep,
    pub retries: u32,
    pub backoff_s: f64,
    pub timeout_s: f64,
    pub effort: Option<String>,
    pub limit_s: Option<f64>,
    pub monotonic: Arc<dyn Fn() -> f64 + Send + Sync>,
    pub refusals: Arc<Refusals>,
    pub forms: Arc<dyn Forms>,
    pub planner: bool,
    pub shape: Option<Shape>,
    pub stated_only: bool,
    pub format_key: String,
    /// RUN-439: each answer this runtime retried after (status, the provider's retry-after).
    pub retried: Arc<Mutex<Vec<Value>>>,
    /// Tests: every request goes to this origin (http://127.0.0.1:port) instead of the provider's.
    pub origin: Option<String>,
    pub token_hex: Arc<dyn Fn(usize) -> String + Send + Sync>,
    pub today: Arc<dyn Fn() -> String + Send + Sync>,
    /// FERB's redactor for a provider's error words, after the key is taken out. Python's loop passes none (only
    /// the key is removed); `None` keeps that. The daemon should give its redactor (see the journal entry).
    pub redact: Option<crate::codex::Redact>,
}

impl ApiRuntime {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        provider: Arc<dyn Provider>,
        supervisor: bool,
        role: &str,
        tool_names: &[String],
        output_schema: Option<Value>,
        bridge: Option<Arc<dyn Bridge>>,
        key: KeyFn,
        forms: Arc<dyn Forms>,
        refusals: Arc<Refusals>,
    ) -> Result<ApiRuntime, SchemaTooComplex> {
        let mut shape = None;
        let mut stated_only = false;
        if let Some(schema) = output_schema.as_ref().filter(|s| ferb_py::value::truthy(s)) {
            match provider.shape(forms.as_ref(), schema) {
                None => shape = Some(Shape::same(schema)),
                Some(Ok(s)) => shape = Some(s),
                Some(Err(e)) => {
                    if !provider.has_without_format() {
                        return Err(e);
                    }
                    shape = Some(Shape::same(schema));
                    stated_only = true;
                }
            }
        }
        let format_key = shape
            .as_ref()
            .map(|s| Refusals::key(provider.name(), provider.model(), &s.wire))
            .unwrap_or_default();
        Ok(ApiRuntime {
            provider,
            supervisor,
            role: role.into(),
            tool_names: tool_names.to_vec(),
            output_schema,
            bridge,
            key,
            sleep: Arc::new(|s| Box::pin(tokio::time::sleep(Duration::from_secs_f64(s.max(0.0))))),
            retries: 4,
            backoff_s: 1.0,
            timeout_s: 600.0,
            effort: None,
            limit_s: None,
            monotonic: Arc::new(|| 0.0),
            refusals,
            forms,
            planner: false,
            shape,
            stated_only,
            format_key,
            retried: Arc::new(Mutex::new(Vec::new())),
            origin: None,
            token_hex: Arc::new(crate::codex::token_hex),
            today: Arc::new(crate::usage::today),
            redact: None,
        })
    }

    pub fn name(&self) -> &str {
        self.provider.name()
    }

    /// The provider's documented limits for its model (L28).
    pub fn limits(&self) -> Result<ModelLimits, FerbError> {
        self.provider.limits().ok_or_else(|| {
            FerbError::ModelLimitsUnknown(format!(
                "the {} adapter reports no context window for {}",
                self.name(),
                crate::pyjson::repr_str(self.provider.model())
            ))
        })
    }

    /// api.py `_state_format`: True when the answer tool is offered.
    fn state_format(&self, state: &mut Value) -> bool {
        let schema = self.output_schema.clone().unwrap_or(Value::Null);
        if self.planner {
            let tool = ToolSpec {
                name: ANSWER_TOOL.into(),
                description: ANSWER_TOOL_DESC.into(),
                schema: inline_refs(&schema),
            };
            self.provider
                .without_format(self.forms.as_ref(), state, PLANNER_ANSWER, Some(&tool));
            return true;
        }
        self.provider
            .without_format(self.forms.as_ref(), state, &stated(&schema), None);
        false
    }
}

struct Run {
    state: Value,
    key: Secret,
    token: String,
    names: HashMap<String, String>,
    cancelled: bool,
    closed: bool,
    updates: Vec<String>,
    inflight: Option<JoinHandle<()>>,
    timer: CallTimer,
    typed: String,
    repaired: bool,
    follow_up: String,
    answer_tool: bool,
}

/// One live direct-API run.
pub struct ApiRun {
    rt: ApiRuntime,
    run: Mutex<Run>,
    deadline: Option<Instant>,
    client: reqwest::Client,
}

impl ApiRun {
    fn lock(&self) -> std::sync::MutexGuard<'_, Run> {
        self.run.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// api.py `continue_`: consumed in order before the next model decision.
    pub fn steer(&self, message: &str) -> Result<(), RuntimeError> {
        let mut r = self.lock();
        if r.closed || r.cancelled {
            return Err(RuntimeError::unsupported(
                "the direct-API run has already ended",
            ));
        }
        r.updates.push(message.to_string());
        Ok(())
    }

    /// api.py `cancel`.
    pub fn cancel(&self) {
        let mut r = self.lock();
        r.cancelled = true;
        if let Some(t) = r.inflight.take() {
            t.abort();
        }
    }
}

/// api.py `start`.
pub async fn start(
    rt: &ApiRuntime,
    task: &Task,
    request: &ModelRequest,
    run_token: &str,
) -> Result<Started, RuntimeError> {
    let key = (rt.key)(rt.name().to_string()).await;
    let Some(key) = key.filter(|k| !k.expose().is_empty()) else {
        return Err(RuntimeError::unsupported(format!(
            "FERB has no {} API key: add one in Settings → AI",
            rt.name()
        )));
    };
    let mut tools = Vec::new();
    let mut names = HashMap::new();
    if (!rt.supervisor || !run_token.is_empty()) && !rt.tool_names.is_empty() {
        if let Some(bridge) = &rt.bridge {
            let offered: HashMap<String, Value> = bridge
                .tools(run_token)
                .into_iter()
                .map(|t| (ferb_py::value::py_str(&t["name"]), t))
                .collect();
            for capability in &rt.tool_names {
                if let Some(t) = offered.get(capability) {
                    tools.push(ToolSpec {
                        name: mcp_name(capability),
                        description: ferb_py::value::py_str(&t["description"]),
                        schema: inline_refs(&t["input_schema"]),
                    });
                    names.insert(mcp_name(capability), capability.clone());
                }
            }
        }
    }
    let effort = rt
        .effort
        .clone()
        .or_else(|| rt.supervisor.then(|| "low".to_string()));
    let mode = if rt.supervisor {
        "supervisor"
    } else {
        "worker"
    };
    let mut state = rt.provider.begin(
        request,
        &tools,
        rt.shape.as_ref().map(|s| &s.wire),
        effort.as_deref(),
        &format!(
            "ferb:{}:{mode}",
            rt.role.split(':').next().unwrap_or_default()
        ),
    );
    let (mut typed, mut answer_tool) = (String::new(), false);
    if rt.shape.is_some() {
        typed = "strict".into();
        if rt.stated_only || rt.refusals.refused(&rt.format_key) {
            answer_tool = rt.state_format(&mut state);
            typed = "stated".into();
        }
    }
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs_f64(rt.timeout_s))
        .build()
        .map_err(|e| RuntimeError::Raised(crate::error::Raised::new("OSError", e.to_string())))?;
    let deadline = rt
        .limit_s
        .map(|s| Instant::now() + Duration::from_secs_f64(s.max(0.0)));
    let m = rt.monotonic.clone();
    let run = Arc::new(ApiRun {
        rt: rt.clone(),
        run: Mutex::new(Run {
            state,
            key,
            token: run_token.to_string(),
            names,
            cancelled: false,
            closed: false,
            updates: Vec::new(),
            inflight: None,
            timer: CallTimer::new(Box::new(move || m()), "runtime"),
            typed,
            repaired: false,
            follow_up: String::new(),
            answer_tool,
        }),
        deadline,
        client,
    });
    let handle = RunHandle {
        id: format!("run-{}", (rt.token_hex)(6)),
        runtime: rt.name().into(),
        task_id: task.id.clone(),
        state: Map::new(),
    };
    let control: Arc<ApiRun> = run.clone();
    let events = EventStream::spawn(move |y| events_body(run, y));
    Ok(Started {
        handle,
        events,
        control,
    })
}

/// A streamed fragment: answer text ("" for life only), or the answer tool's call starting (RUN-447).
enum Fragment {
    Text(String),
    AnswerBegins,
}

enum Exit {
    Closed,
    Fail(RuntimeError),
}

impl From<Closed> for Exit {
    fn from(_: Closed) -> Self {
        Exit::Closed
    }
}

/// api.py `events`: the run's events within the call's one overall limit.
async fn events_body(run: Arc<ApiRun>, mut y: Yielder) -> Result<(), RuntimeError> {
    let main = event_loop(&run, &mut y);
    let out = match run.deadline {
        Some(d) => match tokio::time::timeout_at(d, main).await {
            Ok(r) => r,
            Err(_) => {
                if !run.lock().cancelled {
                    let msg = format!(
                        "{}: the call took longer than its {} s limit",
                        run.rt.name(),
                        py_g(run.rt.limit_s.unwrap_or(0.0))
                    );
                    let _ = y.emit(event(K::Error, json!({"message": msg}))).await;
                }
                Ok(())
            }
        },
        None => main.await,
    };
    {
        let mut r = run.lock();
        r.closed = true;
        if let Some(t) = r.inflight.take() {
            t.abort();
        }
    }
    match out {
        Ok(()) | Err(Exit::Closed) => Ok(()),
        Err(Exit::Fail(e)) => Err(e),
    }
}

fn provider_error_event(e: &RuntimeError, key: &str) -> Value {
    let RuntimeError::Provider {
        message,
        overflow,
        account,
        image,
        ..
    } = e
    else {
        return json!({"message": e.to_string()});
    };
    let mut data = json!({"message": if key.is_empty() { message.clone() } else { message.replace(key, "[key]") }});
    if *overflow {
        data["overflow"] = json!(true);
    }
    if !account.is_empty() {
        data["account"] = json!(account);
    }
    if *image {
        data["image"] = json!(true);
    }
    data
}

/// api.py `_events`.
async fn event_loop(run: &Arc<ApiRun>, y: &mut Yielder) -> Result<(), Exit> {
    let rt = &run.rt;
    loop {
        if run.lock().cancelled {
            return Ok(());
        }
        apply_updates(run);
        let (tx, mut rx) = mpsc::unbounded_channel::<Fragment>();
        let (turn_tx, turn_rx) = tokio::sync::oneshot::channel();
        let call_run = run.clone();
        let task = tokio::spawn(async move {
            let out = call(&call_run, &tx).await;
            let _ = turn_tx.send(out);
        });
        run.lock().inflight = Some(task);
        while let Some(fragment) = rx.recv().await {
            if run.lock().cancelled {
                return Ok(());
            }
            let ev = match fragment {
                Fragment::AnswerBegins => event(K::TextDelta, json!({"text": "", "restart": true})),
                Fragment::Text(t) if !t.is_empty() => event(K::TextDelta, json!({"text": t})),
                Fragment::Text(_) => event(K::Progress, json!({})),
            };
            y.emit(ev).await?;
        }
        let turn = match turn_rx.await {
            Ok(Ok(t)) => t,
            Ok(Err(e @ RuntimeError::Provider { .. })) => {
                run.lock().closed = true;
                let key = run.lock().key.expose().to_string();
                let mut data = provider_error_event(&e, &key);
                if let (Some(redact), Some(Value::String(m))) = (&rt.redact, data.get("message")) {
                    data["message"] = json!(redact(m));
                }
                y.emit(event(K::Error, data)).await?;
                return Ok(());
            }
            Ok(Err(e)) => return Err(Exit::Fail(e)),
            Err(_) => return Ok(()), // cancelled
        };
        let (answer_tool, updates_pending, typed, repaired) = {
            let r = run.lock();
            (
                r.answer_tool,
                !r.updates.is_empty(),
                r.typed.clone(),
                r.repaired,
            )
        };
        let mut final_text = String::new();
        let mut misfit: Option<String> = None;
        let mut via = "";
        let answer = if answer_tool {
            turn.calls.iter().find(|c| c.name == ANSWER_TOOL).cloned()
        } else {
            None
        };
        if let (Some(a), false) = (&answer, updates_pending) {
            final_text = ferb_py::json::dumps_utf8(&a.arguments);
            via = "tool";
            if !a.arguments_error.is_empty() {
                misfit = Some(format!(
                    "the answer is not a JSON object ({})",
                    a.arguments_error
                ));
            } else if let Err(e) = check(rt, &final_text) {
                misfit = Some(e);
            }
        } else if turn.calls.is_empty()
            && !ferb_py::value::strip(turn.final_text.as_deref().unwrap_or_default()).is_empty()
            && !updates_pending
        {
            via = "text";
            let text = turn.final_text.clone().unwrap_or_default();
            let text = if typed.starts_with("stated") {
                unfenced(&text)
            } else {
                text
            };
            final_text = match &rt.shape {
                Some(s) => s.answer_text(&text),
                None => text,
            };
            if typed.starts_with("stated") {
                if let Err(e) = check(rt, &final_text) {
                    misfit = Some(e);
                }
            }
        }
        let repair = if repaired && !final_text.is_empty() {
            if misfit.is_some() {
                "failed"
            } else {
                "fixed"
            }
        } else if misfit.is_some() {
            "asked"
        } else {
            ""
        };
        let empty = is_empty(turn.calls.len(), turn.final_text.as_deref());
        let mut follow = "";
        {
            let mut r = run.lock();
            if r.follow_up == "asked" {
                follow = if empty { "failed" } else { "fixed" };
                r.follow_up = "done".into();
            } else if empty && r.updates.is_empty() && r.follow_up.is_empty() {
                follow = "asked";
            }
        }
        let mut data = turn.usage.as_dict();
        data.insert("timing".into(), Value::Object(run.lock().timer.done()));
        if !typed.is_empty() {
            data.insert("typed".into(), json!(typed));
        }
        if !repair.is_empty() {
            data.insert("repair".into(), json!(repair));
        }
        data.insert("stop".into(), json!(turn.stop));
        data.insert("blocks".into(), json!(turn.blocks));
        if !follow.is_empty() {
            data.insert("follow_up".into(), json!(follow));
        }
        if !via.is_empty() && answer_tool {
            data.insert("answer_via".into(), json!(via));
        }
        y.emit(event(K::Usage, Value::Object(data))).await?;
        if let (Some(a), false) = (&answer, updates_pending) {
            if misfit.is_some() && !repaired {
                // RUN-427's one repair, told back as the answer call's own result
                let outcomes: Vec<ToolOutcome> = turn
                    .calls
                    .iter()
                    .map(|c| ToolOutcome {
                        call: c.clone(),
                        text: if c == a {
                            repair_note(misfit.as_deref().unwrap_or_default())
                        } else {
                            NOT_RUN_WITH_ANSWER.into()
                        },
                        is_error: true,
                        image: None,
                    })
                    .collect();
                let mut r = run.lock();
                r.repaired = true;
                rt.provider.add_results(&mut r.state, &turn, &outcomes);
                continue;
            }
            run.lock().closed = true;
            y.emit(event(K::Final, json!({"text": final_text}))).await?;
            return Ok(());
        }
        if turn.calls.is_empty() {
            if !run.lock().updates.is_empty() {
                continue; // the answer predates an accepted owner correction
            }
            if empty {
                if follow == "asked" {
                    let mut r = run.lock();
                    r.follow_up = "asked".into();
                    rt.provider.add_message(&mut r.state, FOLLOW_UP);
                    continue;
                }
                run.lock().closed = true;
                y.emit(event(
                    K::Error,
                    json!({"message": format!("{}: the model ended without an answer twice", rt.name()),
                        "empty": true}),
                ))
                .await?;
                return Ok(());
            }
            if misfit.is_some() && !repaired {
                let mut r = run.lock();
                r.repaired = true;
                rt.provider.add_message(
                    &mut r.state,
                    &repair_note(misfit.as_deref().unwrap_or_default()),
                );
                continue;
            }
            run.lock().closed = true;
            y.emit(event(K::Final, json!({"text": final_text}))).await?;
            return Ok(());
        }
        let mut results = Vec::new();
        for c in &turn.calls {
            let (answer_tool, names, has_updates) = {
                let r = run.lock();
                (r.answer_tool, r.names.clone(), !r.updates.is_empty())
            };
            if answer_tool && c.name == ANSWER_TOOL {
                results.push(ToolOutcome {
                    call: c.clone(),
                    text: ANSWER_NOT_TAKEN.into(),
                    is_error: true,
                    image: None,
                });
                continue;
            }
            let Some(capability) = names.get(&c.name).cloned() else {
                // a name this run was never offered: nothing runs, the shared run counts it (RUN-445)
                y.emit(event(
                    K::ToolCall,
                    json!({"tool": c.name, "arguments": c.arguments, "offered": false}),
                ))
                .await?;
                if run.lock().cancelled {
                    return Ok(());
                }
                results.push(ToolOutcome {
                    call: c.clone(),
                    text: format!(
                        "FERB refused: no tool named {} was offered to this run; use one of the tools listed for it.",
                        crate::pyjson::repr_str(&c.name)
                    ),
                    is_error: true,
                    image: None,
                });
                continue;
            };
            if !has_updates {
                y.emit(event(
                    K::ToolCall,
                    json!({"tool": c.name, "arguments": c.arguments}),
                ))
                .await?;
            }
            if run.lock().cancelled {
                return Ok(());
            }
            if !run.lock().updates.is_empty() {
                results.push(ToolOutcome {
                    call: c.clone(),
                    text: "Tool not executed: the owner updated the task; reconsider this action using the new instructions.".into(),
                    is_error: true,
                    image: None,
                });
                continue;
            }
            match execute(run, &capability, c).await {
                None => return Ok(()),
                Some(outcome) => {
                    if run.lock().cancelled {
                        return Ok(());
                    }
                    results.push(outcome);
                    y.emit(event(K::Progress, json!({}))).await?;
                }
            }
        }
        let mut r = run.lock();
        rt.provider.add_results(&mut r.state, &turn, &results);
    }
}

fn check(rt: &ApiRuntime, text: &str) -> Result<(), String> {
    match &rt.output_schema {
        Some(schema) if ferb_py::value::truthy(schema) => rt.forms.check_final(text, schema),
        _ => Ok(()),
    }
}

fn apply_updates(run: &ApiRun) {
    let mut r = run.lock();
    let updates = std::mem::take(&mut r.updates);
    for m in updates {
        run.rt.provider.add_message(&mut r.state, &m);
    }
}

/// api.py `_execute`: None when the run was cancelled. The gateway call runs as its own task.
async fn execute(run: &Arc<ApiRun>, capability: &str, call: &ToolCall) -> Option<ToolOutcome> {
    if !call.arguments_error.is_empty() {
        return Some(ToolOutcome {
            call: call.clone(),
            text: format!(
                "FERB refused: the tool arguments were not a JSON object ({})",
                call.arguments_error
            ),
            is_error: true,
            image: None,
        });
    }
    let bridge = run.rt.bridge.clone()?;
    let token = run.lock().token.clone();
    let (cap, args) = (capability.to_string(), call.arguments.clone());
    let (tx, rx) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(async move {
        let out = bridge.call(&token, &cap, &args).await;
        let _ = tx.send(out);
    });
    run.lock().inflight = Some(task);
    let out = match rx.await {
        Ok(o) => o,
        Err(_) => {
            if run.lock().cancelled {
                return None;
            }
            return Some(ToolOutcome {
                call: call.clone(),
                text: "FERB stopped this tool call".into(),
                is_error: true,
                image: None,
            });
        }
    };
    Some(match out {
        Err(e) => ToolOutcome {
            call: call.clone(),
            text: format!("FERB refused: {e}"),
            is_error: true,
            image: None,
        },
        Ok(out) => ToolOutcome {
            call: call.clone(),
            text: format!(
                "{}\n{}",
                ferb_py::json::dumps_utf8(out.get("content").unwrap_or(&Value::Null)),
                ferb_py::value::py_str(out.get("footer").unwrap_or(&json!("")))
            ),
            is_error: out.get("is_error").is_some_and(ferb_py::value::truthy),
            image: out.get("image").filter(|v| !v.is_null()).cloned(),
        },
    })
}

/// api.py `_call`: one model call; a typed format the provider refused goes once more stated (RUN-427).
async fn call(
    run: &Arc<ApiRun>,
    tx: &mpsc::UnboundedSender<Fragment>,
) -> Result<Turn, RuntimeError> {
    run.lock().timer.begin();
    match send(run, tx).await {
        Ok(t) => return Ok(t),
        Err(e) => {
            let refusable = matches!(
                &e,
                RuntimeError::Provider {
                    status: 400,
                    overflow: false,
                    image: false,
                    ..
                }
            ) && run.lock().typed == "strict"
                && run.rt.provider.has_without_format();
            if !refusable {
                return Err(e);
            }
            let mut r = run.lock();
            let mut state = std::mem::take(&mut r.state);
            r.answer_tool = run.rt.state_format(&mut state);
            r.state = state;
            r.typed = "stated_after_refusal".into();
        }
    }
    let turn = send(run, tx).await?;
    run.rt.refusals.remember(&run.rt.format_key);
    Ok(turn)
}

fn retry_after(headers: &reqwest::header::HeaderMap) -> Option<f64> {
    let v = headers.get("retry-after")?.to_str().ok()?;
    let f: f64 = ferb_py::value::strip(v).parse().ok()?;
    Some(f.min(60.0))
}

fn error_of(body: &[u8]) -> Value {
    let parsed = std::str::from_utf8(body)
        .ok()
        .and_then(|t| ferb_py::json::loads(t).ok());
    match parsed {
        Some(Value::Object(m)) => match m.get("error") {
            Some(e) if ferb_py::value::truthy(e) => e.clone(),
            _ => json!({}),
        },
        _ => json!({}),
    }
}

/// api.py `_error_text`.
fn error_text(
    name: &str,
    status: u16,
    body: &[u8],
    headers: &reqwest::header::HeaderMap,
    key: &str,
) -> String {
    let err = error_of(body);
    let detail = match &err {
        Value::Object(m) => {
            let pick = ["message", "type", "status"]
                .iter()
                .filter_map(|k| m.get(*k))
                .find(|v| ferb_py::value::truthy(v))
                .cloned()
                .unwrap_or(json!(""));
            ferb_py::value::py_str(&pick)
        }
        other => ferb_py::value::py_str(other),
    };
    let detail = if key.is_empty() {
        detail
    } else {
        detail.replace(key, "[key]")
    };
    let technical = if detail.is_empty() {
        format!("HTTP {status}")
    } else {
        format!("HTTP {status}: {}", ferb_py::value::prefix(&detail, 300))
    };
    let wait = if status == 429 {
        retry_after(headers).filter(|w| *w != 0.0 && !w.is_nan())
    } else {
        None
    };
    let _ = name;
    match wait {
        Some(w) => format!(
            "{} ({technical}); it says to retry after {} s",
            http_cause(status),
            py_g(w)
        ),
        None => format!("{} ({technical})", http_cause(status)),
    }
}

/// httpx's class for a transport failure, as Python's message names it.
fn transport_class(e: &reqwest::Error) -> &'static str {
    if e.is_connect() {
        "ConnectError"
    } else if e.is_timeout() {
        "ReadTimeout"
    } else {
        "RemoteProtocolError"
    }
}

fn with_origin(url: &str, origin: &Option<String>) -> String {
    let Some(origin) = origin else {
        return url.to_string();
    };
    match url
        .find("://")
        .and_then(|i| url[i + 3..].find('/').map(|j| i + 3 + j))
    {
        Some(path) => format!("{origin}{}", &url[path..]),
        None => origin.clone(),
    }
}

/// api.py `_send`: retries on the provider's busy statuses and on transport failures before any answer text.
async fn send(
    run: &Arc<ApiRun>,
    tx: &mpsc::UnboundedSender<Fragment>,
) -> Result<Turn, RuntimeError> {
    let rt = &run.rt;
    let streaming = rt.supervisor && rt.output_schema.is_some();
    let (state, key, answer_tool) = {
        let r = run.lock();
        (r.state.clone(), r.key.expose().to_string(), r.answer_tool)
    };
    let request = if streaming {
        rt.provider.stream_http(&state)
    } else {
        None
    };
    let streaming = request.is_some();
    let (url, headers, body) = request.unwrap_or_else(|| rt.provider.http(&state));
    let url = with_origin(&url, &rt.origin);
    let mut all_headers = headers.clone();
    all_headers.extend(rt.provider.auth(&key));
    let payload = crate::pyjson::compact(&body);
    let mut delay = rt.backoff_s;
    for attempt in 0..=rt.retries {
        let mut emitted = false;
        run.lock().timer.sent();
        let mut req = run.client.post(&url);
        for (k, v) in &all_headers {
            req = req.header(k, v);
        }
        let attempt_out: Result<Turn, Result<RuntimeError, reqwest::Error>> = async {
            let mut resp = req.body(payload.clone()).send().await.map_err(Err)?;
            let status = resp.status().as_u16();
            if RETRY_STATUS.contains(&status) && attempt < rt.retries {
                let wait = retry_after(resp.headers());
                let _ = resp.bytes().await;
                rt.retried
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .push(json!({"status": status, "retry_after": wait}));
                let pause = match wait {
                    Some(w) if w != 0.0 && !w.is_nan() => w,
                    _ => delay,
                };
                (rt.sleep)(pause).await;
                delay *= 2.0;
                return Err(Ok(RuntimeError::protocol("retry")));
            }
            if status != 200 {
                let headers = resp.headers().clone();
                let body = resp.bytes().await.map_err(Err)?;
                let err = error_of(&body);
                let shape = err.as_object().cloned().unwrap_or_default();
                return Err(Ok(RuntimeError::Provider {
                    message: error_text(rt.name(), status, &body, &headers, &key),
                    overflow: rt.provider.window_exceeded(status, &shape),
                    account: account_status(status).into(),
                    image: rt.provider.image_refused(status, &shape),
                    status,
                }));
            }
            let sse = resp
                .headers()
                .get("content-type")
                .and_then(|v| v.to_str().ok())
                .map(|v| {
                    v.split(';')
                        .next()
                        .unwrap_or_default()
                        .trim()
                        .to_lowercase()
                })
                == Some("text/event-stream".into());
            let data = if streaming && sse {
                let mut parser = rt
                    .provider
                    .stream_parser(if answer_tool { ANSWER_TOOL } else { "" })
                    .expect("a streamed provider has a parser");
                let mut lines = SseLines::default();
                loop {
                    let chunk = resp.chunk().await.map_err(|e| {
                        if emitted {
                            Ok(RuntimeError::provider(format!(
                                "{}: network error ({})",
                                rt.name(),
                                transport_class(&e)
                            )))
                        } else {
                            Err(e)
                        }
                    })?;
                    let done = chunk.is_none();
                    let events = match chunk {
                        Some(c) => lines.push(&c),
                        None => lines.end(),
                    }
                    .map_err(Ok)?;
                    for ev in events {
                        run.lock().timer.output();
                        let text = match parser.feed(&ev) {
                            Ok(t) => t,
                            Err(FeedError::Malformed) => {
                                return Err(Ok(RuntimeError::provider(format!(
                                    "{}: malformed response stream event",
                                    rt.name()
                                ))))
                            }
                            Err(FeedError::Provider(m)) => {
                                return Err(Ok(RuntimeError::provider(m)))
                            }
                        };
                        if parser.take_answer_began() {
                            let _ = tx.send(Fragment::AnswerBegins);
                        }
                        if !text.is_empty() {
                            emitted = true;
                        }
                        let _ = tx.send(Fragment::Text(text));
                    }
                    if done {
                        break;
                    }
                }
                parser.finish().map_err(Ok)?
            } else {
                let body = resp.bytes().await.map_err(Err)?;
                run.lock().timer.output();
                std::str::from_utf8(&body)
                    .ok()
                    .and_then(|t| ferb_py::json::loads(t).ok())
                    .ok_or_else(|| {
                        Ok(RuntimeError::provider(format!(
                            "{}: the reply was not JSON",
                            rt.name()
                        )))
                    })?
            };
            let mut r = run.lock();
            rt.provider
                .parse(&mut r.state, &data, &(rt.today)())
                .map_err(Ok)
        }
        .await;
        match attempt_out {
            Ok(turn) => return Ok(turn),
            Err(Ok(RuntimeError::Protocol { message, .. })) if message == "retry" => continue,
            Err(Ok(e)) => return Err(e),
            Err(Err(e)) => {
                // once a draft has been exposed, silently restarting would concatenate different answers
                if !emitted && attempt < rt.retries {
                    (rt.sleep)(delay).await;
                    delay *= 2.0;
                    continue;
                }
                return Err(RuntimeError::provider(format!(
                    "{}: network error ({})",
                    rt.name(),
                    transport_class(&e)
                )));
            }
        }
    }
    Err(RuntimeError::provider(format!(
        "{}: still failing after {} retries",
        rt.name(),
        rt.retries
    )))
}

/// api.py `_sse_events`: SSE framing across transport splits, multiline data, `[DONE]`.
#[derive(Default)]
struct SseLines {
    buf: Vec<u8>,
    data: Vec<String>,
}

impl SseLines {
    fn decode(&mut self) -> Result<Option<Value>, RuntimeError> {
        let text = self.data.join("\n");
        self.data.clear();
        if text.is_empty() || text == "[DONE]" {
            return Ok(None);
        }
        match ferb_py::json::loads(&text) {
            Ok(v @ Value::Object(_)) => Ok(Some(v)),
            Ok(_) => Err(RuntimeError::provider(
                "The response stream contained a non-object event",
            )),
            Err(_) => Err(RuntimeError::provider(
                "The response stream contained invalid JSON",
            )),
        }
    }

    fn line(&mut self, line: &str, out: &mut Vec<Value>) -> Result<(), RuntimeError> {
        if line.is_empty() {
            if let Some(ev) = self.decode()? {
                out.push(ev);
            }
        } else if let Some(rest) = line.strip_prefix("data:") {
            self.data
                .push(rest.strip_prefix(' ').unwrap_or(rest).to_string());
        }
        Ok(())
    }

    /// httpx's `aiter_lines`: a line ends at \n, \r\n or \r.
    fn push(&mut self, chunk: &[u8]) -> Result<Vec<Value>, RuntimeError> {
        self.buf.extend_from_slice(chunk);
        let mut out = Vec::new();
        while let Some(i) = self.buf.iter().position(|b| *b == b'\n' || *b == b'\r') {
            if self.buf[i] == b'\r' && i + 1 == self.buf.len() {
                break; // a \r at the end may be the start of \r\n
            }
            let line: Vec<u8> = self.buf.drain(..i).collect();
            let sep = if self.buf.first() == Some(&b'\r') && self.buf.get(1) == Some(&b'\n') {
                2
            } else {
                1
            };
            self.buf.drain(..sep);
            let text = String::from_utf8_lossy(&line).into_owned();
            self.line(&text, &mut out)?;
        }
        Ok(out)
    }

    fn end(&mut self) -> Result<Vec<Value>, RuntimeError> {
        let mut out = Vec::new();
        if !self.buf.is_empty() {
            let rest = std::mem::take(&mut self.buf);
            let text = String::from_utf8_lossy(&rest)
                .trim_end_matches('\r')
                .to_string();
            self.line(&text, &mut out)?;
        }
        if let Some(ev) = self.decode()? {
            out.push(ev);
        }
        Ok(out)
    }
}

impl RunControl for ApiRun {
    fn steer<'a>(
        &'a self,
        message: &'a str,
    ) -> ferb_core::interfaces::BoxFuture<'a, Result<(), RuntimeError>> {
        let out = ApiRun::steer(self, message);
        Box::pin(async move { out })
    }

    fn cancel(&self) -> ferb_core::interfaces::BoxFuture<'_, ()> {
        ApiRun::cancel(self);
        Box::pin(async {})
    }
}

impl crate::run::Adapter for ApiRuntime {
    fn name(&self) -> &str {
        self.provider.name()
    }

    fn start<'a>(
        &'a self,
        task: &'a Task,
        request: &'a ModelRequest,
        run_token: &'a str,
    ) -> ferb_core::interfaces::BoxFuture<'a, Result<Started, RuntimeError>> {
        Box::pin(start(self, task, request, run_token))
    }
}
