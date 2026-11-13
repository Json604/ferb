//! The Claude adapter (ferb/runtimes/claude_sdk.py; RUN-409, RUN-413, RUN-415, RUN-419..421, RUN-427, RUN-440,
//! RUN-445, RUN-446) driving the pinned, bundled Claude Code directly over its stream-json protocol, as the Python
//! SDK did (ENG-2). Lockdown: no built-in tools, `dontAsk`, no setting sources, strict MCP, no plugins, agents or
//! skills, telemetry off, FERB's own sign-in only (an inherited key or token is blanked; a run whose init reports an
//! API key is refused), the run token in a 0600 file, and the tool list the API receives checked through FERB's
//! tool-list proxy after every turn.

pub mod proxy;
pub mod transport;

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ferb_core::errors::FerbError;
use ferb_core::timing::{CallTimer, Steps};
use ferb_core::types::{ModelLimits, ModelRequest, RunHandle, RuntimeEventKind as K, Task};
use serde_json::{json, Map, Value};
use tokio::time::Instant;

use crate::answer_rules::FOLLOW_UP;
use crate::codex::{mcp_name, Clock};
use crate::error::{Raised, RuntimeError};
use crate::forms::{repair_note, stated, unfenced, Forms, Refusals, Shape};
use crate::gen::{Closed, EventStream, Yielder};
use crate::process::Spawner;
use crate::pyjson::repr_str;
use crate::run::{event, RunControl, Started};
use transport::{Block, Client, Message, Options, Urandom4};

/// The tool the SDK adds when output_format is set (claude_sdk.py:49).
pub const STRUCTURED_OUTPUT: &str = "StructuredOutput";
/// The result subtype when the model's answers never matched output_format.
pub const STRUCTURED_RETRIES: &str = "error_max_structured_output_retries";
/// FERB's breakpoint on its stable prefix: one hour (P20).
pub fn cache_marker() -> Value {
    json!({"type": "ephemeral", "ttl": "1h"})
}
/// L28: Claude Code's aliases and the models they resolve to on the Anthropic API.
pub const ALIASES: [(&str, &str); 4] = [
    ("opus", "claude-opus-5-5"),
    ("sonnet", "claude-sonnet-5-5"),
    ("haiku", "claude-haiku-5-5"),
    ("fable", "claude-fable-5-1"),
];
/// The marker FERB's finished sign-in leaves (claude_login.py SIGNED_IN).
pub const SIGNED_IN: &str = ".ferb-signed-in";

pub fn alias(model: &str) -> &str {
    ALIASES
        .iter()
        .find(|(a, _)| *a == model)
        .map(|(_, m)| *m)
        .unwrap_or(model)
}

/// claude_login.py `claude_home`: FERB's own Claude home once a sign-in there finished; never the owner's.
pub fn claude_home(ferb_home: &Path) -> Option<PathBuf> {
    let home = ferb_home.join("claude-home");
    home.join(SIGNED_IN).is_file().then_some(home)
}

/// claude_sdk.py `limits_for`.
pub fn limits_for(model: Option<&str>) -> Result<ModelLimits, FerbError> {
    let model = model.filter(|m| !m.is_empty()).ok_or_else(|| {
        FerbError::ModelLimitsUnknown(
            "no model is chosen for Claude, so its context window is the account's plan-dependent default"
                .into(),
        )
    })?;
    let resolved = crate::api::anthropic::limits_for(alias(model));
    Ok(ModelLimits {
        model: model.into(),
        context_window: resolved.context_window,
        max_output: resolved.max_output,
        source: format!(
            "{}; aliases: https://code.claude.com/docs/en/model-config",
            resolved.source
        ),
        window_includes_output: true,
        max_input_chars: None,
    })
}

/// The Agent SDK's AssistantMessageError kinds that are account facts (claude_sdk.py:142).
fn account_error(kind: &str) -> &'static str {
    match kind {
        "authentication_failed" => "signed_out",
        "billing_error" | "rate_limit" => "usage_limit",
        _ => "",
    }
}

/// claude_sdk.py `_content`: FERB's text as the one user message's blocks, the cache breakpoint on the last stable
/// part (or the system text), then the images.
pub fn content(request: &ModelRequest) -> Vec<Value> {
    let mut texts = vec![request.system.clone()];
    texts.extend(request.parts.iter().cloned());
    let marked = if 0 < request.cacheable && request.cacheable <= request.parts.len() {
        request.cacheable
    } else {
        0
    };
    let mut blocks: Vec<Value> = Vec::new();
    for (index, text) in texts.iter().enumerate() {
        if !text.is_empty() {
            blocks.push(json!({"type": "text", "text": text}));
        }
        if index == marked {
            if let Some(Value::Object(last)) = blocks.last_mut() {
                last.insert("cache_control".into(), cache_marker());
            }
        }
    }
    for i in &request.images {
        blocks.push(
            json!({"type": "image", "source": {"type": "base64", "media_type": i.mime,
            "data": i.base64()}}),
        );
    }
    blocks
}

/// The tool-list proxy every Claude run's ANTHROPIC_BASE_URL points at (proxy.py), as the runtime uses it.
pub trait ToolProxy: Send + Sync {
    /// Registers the run (until `forget`) and returns its base URL, carrying a fresh secret.
    fn base_url(&self, run_id: &str) -> String;
    /// The tool names the run's requests carried.
    fn seen(&self, run_id: &str) -> BTreeSet<String>;
    fn forget(&self, run_id: &str);
}

/// What the daemon lends every Claude run: how programs start, the daemon's environment, random ids, the date.
#[derive(Clone)]
pub struct Host {
    pub spawner: Arc<dyn Spawner>,
    /// Where the run token's owner-only folder goes (`tempfile.mkdtemp`).
    pub temp_root: PathBuf,
    pub environ: BTreeMap<String, String>,
    /// `secrets.token_hex(n)` (the run id).
    pub token_hex: Arc<dyn Fn(usize) -> String + Send + Sync>,
    /// `uuid.uuid4()` (a steering frame's id).
    pub uuid4: Arc<dyn Fn() -> String + Send + Sync>,
    /// `os.urandom(4).hex()` (a control request's id).
    pub urandom4: Urandom4,
    pub today: Arc<dyn Fn() -> String + Send + Sync>,
}

impl Host {
    pub fn system() -> Host {
        Host {
            spawner: Arc::new(crate::process::OsSpawner),
            temp_root: std::env::temp_dir(),
            environ: std::env::vars().collect(),
            token_hex: Arc::new(crate::codex::token_hex),
            uuid4: Arc::new(|| uuid::Uuid::new_v4().to_string()),
            urandom4: Arc::new(|| crate::codex::token_hex(4)),
            today: Arc::new(crate::usage::today),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Supervisor,
    Worker,
}

/// claude_sdk.ClaudeRuntime.
#[derive(Clone)]
pub struct ClaudeRuntime {
    pub mode: Mode,
    pub proxy: Arc<dyn ToolProxy>,
    pub ferb_home: PathBuf,
    pub shim_cmd: Option<Vec<String>>,
    pub output_schema: Option<Value>,
    pub model: Option<String>,
    pub effort: Option<String>,
    /// One overall limit on this call (catalog.call_limit); None for a worker.
    pub limit_s: Option<f64>,
    pub monotonic: Clock,
    pub refusals: Arc<Refusals>,
    pub forms: Arc<dyn Forms>,
    /// The pinned Claude Code FERB bundles; `None`: missing (every run is refused).
    pub cli: Option<PathBuf>,
    pub shape: Option<Shape>,
    pub stated_only: bool,
    pub format_key: String,
    pub allowed: BTreeSet<String>,
    pub host: Host,
}

impl ClaudeRuntime {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        mode: Mode,
        proxy: Arc<dyn ToolProxy>,
        ferb_home: PathBuf,
        tool_names: &[String],
        output_schema: Option<Value>,
        model: Option<String>,
        forms: Arc<dyn Forms>,
        refusals: Arc<Refusals>,
        host: Host,
    ) -> ClaudeRuntime {
        let mut shape = None;
        let mut stated_only = false;
        if let Some(schema) = output_schema.as_ref().filter(|s| ferb_py::value::truthy(s)) {
            match forms.anthropic(schema) {
                Ok(s) => shape = Some(s),
                Err(_) => {
                    shape = Some(Shape::same(schema));
                    stated_only = true;
                }
            }
        }
        let format_key = shape
            .as_ref()
            .map(|s| {
                Refusals::key(
                    "anthropic",
                    alias(model.as_deref().unwrap_or_default()),
                    &s.wire,
                )
            })
            .unwrap_or_default();
        let mut allowed: BTreeSet<String> = tool_names
            .iter()
            .map(|n| format!("mcp__ferb__{}", mcp_name(n)))
            .collect();
        if output_schema.as_ref().is_some_and(ferb_py::value::truthy) {
            allowed.insert(STRUCTURED_OUTPUT.into());
        }
        ClaudeRuntime {
            mode,
            proxy,
            ferb_home,
            shim_cmd: None,
            output_schema,
            model,
            effort: None,
            limit_s: None,
            monotonic: Arc::new(|| 0.0),
            refusals,
            forms,
            cli: None,
            shape,
            stated_only,
            format_key,
            allowed,
            host,
        }
    }

    pub fn limits(&self) -> Result<ModelLimits, FerbError> {
        limits_for(self.model.as_deref())
    }

    fn now(&self) -> ferb_core::timing::Now {
        let m = self.monotonic.clone();
        Box::new(move || m())
    }

    /// claude_sdk.py `_options`: FERB's own sign-in and bundled Claude Code, or the run is refused.
    fn options(
        &self,
        run_id: &str,
        token_file: Option<&Path>,
        stated_form: bool,
    ) -> Result<(String, Options), RuntimeError> {
        let Some(own) = claude_home(&self.ferb_home) else {
            return Err(RuntimeError::unsupported(
                "Claude isn't signed in for FERB: sign in to Claude in Settings → AI",
            ));
        };
        let Some(cli) = self.cli.as_ref() else {
            return Err(RuntimeError::unsupported(
                "the Claude Agent SDK's bundled Claude Code is missing",
            ));
        };
        let mut env = BTreeMap::new();
        for (k, v) in [
            ("ANTHROPIC_BASE_URL", self.proxy.base_url(run_id)),
            ("DISABLE_TELEMETRY", "1".into()),
            ("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC", "1".into()),
            ("DISABLE_ERROR_REPORTING", "1".into()),
            ("ANTHROPIC_API_KEY", String::new()),
            ("ANTHROPIC_AUTH_TOKEN", String::new()),
            ("CLAUDE_CODE_OAUTH_TOKEN", String::new()),
            ("CLAUDE_CODE_MCP_TOOL_IDLE_TIMEOUT", "0".into()),
            ("CLAUDE_CONFIG_DIR", own.to_string_lossy().into_owned()),
            ("CLAUDE_CODE_PROMPT_CACHE_TTL", "1h".into()),
            ("ENABLE_TOOL_SEARCH", "false".into()),
        ] {
            env.insert(k.to_string(), v);
        }
        let mut mcp = None;
        if self.mode == Mode::Worker || token_file.is_some() {
            let shim = match &self.shim_cmd {
                Some(s) if !s.is_empty() => s,
                _ => {
                    return Err(crate::codex::value_error(
                        "worker mode needs the ferb mcp shim",
                    ))
                }
            };
            let Some(token_file) = token_file else {
                return Err(crate::codex::value_error(
                    "worker mode needs a run token file",
                ));
            };
            mcp = Some(
                json!({"ferb": {"type": "stdio", "command": shim[0], "args": &shim[1..],
                "env": {"FERB_RUN_TOKEN_FILE": token_file.to_string_lossy(),
                    "FERB_HOME": self.ferb_home.to_string_lossy()}}}),
            );
        }
        let allowed: Vec<String> = self
            .allowed
            .iter()
            .filter(|n| n.as_str() != STRUCTURED_OUTPUT)
            .cloned()
            .collect();
        let options = Options {
            allowed_tools: allowed,
            model: self.model.clone(),
            mcp_servers: mcp,
            include_partial_messages: self.mode == Mode::Supervisor && self.output_schema.is_some(),
            effort: self.effort.clone(),
            json_schema: match (&self.shape, stated_form) {
                (Some(s), false) => Some(s.wire.clone()),
                _ => None,
            },
            env,
        };
        Ok((cli.to_string_lossy().into_owned(), options))
    }

    /// claude_sdk.py `_request`: the answer form stated after FERB's instructions when the typed format isn't used.
    fn request_as_sent(&self, request: &ModelRequest, typed: &str) -> ModelRequest {
        let Some(schema) = self.output_schema.as_ref() else {
            return request.clone();
        };
        if !typed.starts_with("stated") {
            return request.clone();
        }
        let mut out = request.clone();
        out.system = [request.system.clone(), stated(schema)]
            .into_iter()
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join("\n\n");
        out
    }

    async fn connect(
        &self,
        run_id: &str,
        token_file: Option<&Path>,
        stated_form: bool,
    ) -> Result<Client, RuntimeError> {
        let (cli, options) = self.options(run_id, token_file, stated_form)?;
        Client::connect(
            self.host.spawner.as_ref(),
            &cli,
            &self.host.environ,
            &options,
            self.host.urandom4.clone(),
        )
        .await
    }
}

/// claude_sdk.py `write_token_file`: the run token in a fresh owner-only folder, as a 0600 file.
pub fn write_token_file(temp_root: &Path, run_token: &str) -> std::io::Result<PathBuf> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let folder = tempfile::Builder::new()
        .prefix("ferb-claude-run-")
        .tempdir_in(temp_root)?
        .keep();
    let path = folder.join("run-token");
    let mut f = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&path)?;
    f.write_all(run_token.as_bytes())?;
    Ok(path)
}

fn remove_token(path: Option<&PathBuf>) {
    if let Some(p) = path.and_then(|p| p.parent()) {
        let _ = fs::remove_dir_all(p);
    }
}

struct State {
    client: Option<Arc<Client>>,
    checked_auth: bool,
    token_file: Option<PathBuf>,
    timer: CallTimer,
    typed: String,
    closed: bool,
    pending_updates: BTreeSet<String>,
    emitted: bool,
    structured: Option<Value>,
    account: String,
    blocks: Vec<String>,
    cost_so_far: f64,
    follow_up: String,
    restart: bool,
    remembered: bool,
    repaired: bool,
}

/// One live Claude run.
pub struct ClaudeRun {
    rt: ClaudeRuntime,
    id: String,
    request: ModelRequest,
    deadline: Option<Instant>,
    st: Mutex<State>,
    input_lock: tokio::sync::Mutex<()>,
}

impl ClaudeRun {
    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.st.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn client(&self) -> Option<Arc<Client>> {
        self.lock().client.clone()
    }

    /// claude_sdk.py `continue_`: a second stream-json user frame on the same live Claude Code, its uuid the
    /// acknowledgement Claude Code replays (`--replay-user-messages`).
    pub async fn steer(&self, message: &str) -> Result<(), RuntimeError> {
        let _guard = self.input_lock.lock().await;
        let client = {
            let st = self.lock();
            match (&st.client, st.closed) {
                (Some(c), false) => c.clone(),
                _ => {
                    return Err(RuntimeError::unsupported(
                        "the Claude run has already ended",
                    ))
                }
            }
        };
        let ident = (self.rt.host.uuid4)();
        self.lock().pending_updates.insert(ident.clone());
        let frame = json!({"type": "user", "message": {"role": "user", "content": message},
            "parent_tool_use_id": null, "uuid": ident});
        if let Err(e) = client.query(vec![frame]).await {
            self.lock().pending_updates.remove(&ident);
            self.cancel().await;
            return Err(e);
        }
        Ok(())
    }

    /// claude_sdk.py `cancel`.
    pub async fn cancel(&self) {
        let client = {
            let mut st = self.lock();
            st.closed = true;
            st.client.take()
        };
        if let Some(c) = client {
            c.disconnect().await;
            self.rt.proxy.forget(&self.id);
        }
        let token = self.lock().token_file.take();
        remove_token(token.as_ref());
    }

    /// claude_sdk.py `_restart` (RUN-427): the typed format refused before any output; the same run starts again
    /// in a fresh Claude Code with the answer form stated.
    async fn restart(&self) -> Result<(), RuntimeError> {
        let old = {
            let mut st = self.lock();
            st.typed = "stated_after_refusal".into();
            st.checked_auth = false;
            st.client.take()
        };
        if let Some(old) = old {
            old.disconnect().await;
        }
        let token = self.lock().token_file.clone();
        let client = Arc::new(self.rt.connect(&self.id, token.as_deref(), true).await?);
        self.lock().client = Some(client.clone());
        self.lock().timer.begin();
        let request = self
            .rt
            .request_as_sent(&self.request, "stated_after_refusal");
        client.query(vec![first_frame(&request)]).await
    }
}

fn first_frame(request: &ModelRequest) -> Value {
    json!({"type": "user", "parent_tool_use_id": null,
        "message": {"role": "user", "content": content(request)}})
}

/// claude_sdk.py `start`.
pub async fn start(
    rt: &ClaudeRuntime,
    task: &Task,
    request: &ModelRequest,
    run_token: &str,
) -> Result<Started, RuntimeError> {
    let run_id = format!("run-{}", (rt.host.token_hex)(6));
    let deadline = rt
        .limit_s
        .map(|s| Instant::now() + Duration::from_secs_f64(s.max(0.0)));
    let mut steps = Steps::new(rt.now());
    let mut timer = CallTimer::new(rt.now(), "runtime");
    let token_file = if rt.mode == Mode::Worker || !run_token.is_empty() {
        Some(write_token_file(&rt.host.temp_root, run_token)?)
    } else {
        None
    };
    let typed = match &rt.shape {
        Some(_) if rt.stated_only || rt.refusals.refused(&rt.format_key) => "stated",
        Some(_) => "strict",
        None => "",
    }
    .to_string();
    let mut client: Option<Arc<Client>> = None;
    let body = async {
        let (cli, options) = rt.options(&run_id, token_file.as_deref(), typed == "stated")?;
        steps.mark("options");
        let inner = async {
            let c = Arc::new(
                Client::connect(
                    rt.host.spawner.as_ref(),
                    &cli,
                    &rt.host.environ,
                    &options,
                    rt.host.urandom4.clone(),
                )
                .await?,
            );
            client = Some(c.clone());
            steps.mark("connect");
            timer.begin();
            c.query(vec![first_frame(&rt.request_as_sent(request, &typed))])
                .await?;
            steps.mark("query");
            Ok::<(), RuntimeError>(())
        };
        match deadline {
            Some(d) => match tokio::time::timeout_at(d, inner).await {
                Ok(r) => r,
                Err(_) => Err(RuntimeError::Raised(Raised::timeout())),
            },
            None => inner.await,
        }
    };
    if let Err(e) = body.await {
        if let Some(c) = &client {
            c.disconnect().await;
        }
        rt.proxy.forget(&run_id);
        remove_token(token_file.as_ref());
        return Err(e);
    }
    let mut state = Map::new();
    state.insert("start_steps".into(), Value::Object(steps.steps.clone()));
    state.insert("typed".into(), json!(typed));
    if let Some(t) = &token_file {
        state.insert("token_file".into(), json!(t.to_string_lossy()));
    }
    let run = Arc::new(ClaudeRun {
        rt: rt.clone(),
        id: run_id.clone(),
        request: request.clone(),
        deadline,
        st: Mutex::new(State {
            client,
            checked_auth: false,
            token_file,
            timer,
            typed,
            closed: false,
            pending_updates: BTreeSet::new(),
            emitted: false,
            structured: None,
            account: String::new(),
            blocks: Vec::new(),
            cost_so_far: 0.0,
            follow_up: String::new(),
            restart: false,
            remembered: false,
            repaired: false,
        }),
        input_lock: tokio::sync::Mutex::new(()),
    });
    let handle = RunHandle {
        id: run_id,
        runtime: "claude".into(),
        task_id: task.id.clone(),
        state,
    };
    let control: Arc<ClaudeRun> = run.clone();
    let events = EventStream::spawn(move |y| events_body(run, y));
    Ok(Started {
        handle,
        events,
        control,
    })
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

impl From<RuntimeError> for Exit {
    fn from(e: RuntimeError) -> Self {
        Exit::Fail(e)
    }
}

impl From<Raised> for Exit {
    fn from(e: Raised) -> Self {
        Exit::Fail(RuntimeError::Raised(e))
    }
}

/// claude_sdk.py `events`: the turn's events, once more in a fresh Claude Code after a refused typed format.
async fn events_body(run: Arc<ClaudeRun>, mut y: Yielder) -> Result<(), RuntimeError> {
    {
        let st = run.lock();
        if st.client.is_none() || st.closed {
            return Ok(());
        }
    }
    let main = async {
        loop {
            let client = match run.client() {
                Some(c) => c,
                None => return Ok(()),
            };
            inner_events(&run, &client, &mut y).await?;
            let again = std::mem::take(&mut run.lock().restart);
            if !again {
                return Ok(());
            }
            run.restart().await?;
        }
    };
    let out: Result<(), Exit> = match run.deadline {
        Some(d) => match tokio::time::timeout_at(d, main).await {
            Ok(r) => r,
            Err(_) => Err(Exit::Fail(RuntimeError::Raised(Raised::timeout()))),
        },
        None => main.await,
    };
    let result = match out {
        Ok(()) | Err(Exit::Closed) => Ok(()),
        Err(Exit::Fail(RuntimeError::Raised(r))) if r.class == "TimeoutError" => {
            let _ = y
                .emit(event(
                    K::Error,
                    json!({"message": "Claude runtime timed out"}),
                ))
                .await;
            Ok(())
        }
        Err(Exit::Fail(e)) => Err(e),
    };
    run.lock().closed = true;
    result
}

/// claude_sdk.py `_events`.
async fn inner_events(run: &Arc<ClaudeRun>, client: &Client, y: &mut Yielder) -> Result<(), Exit> {
    let rt = &run.rt;
    let mut structured_blocks: BTreeSet<String> = BTreeSet::new();
    let typed = run.lock().typed.clone();
    loop {
        let raw = match client.next_raw().await {
            None => return Ok(()),
            Some(Err(e)) => return Err(Exit::Fail(e)),
            Some(Ok(v)) => v,
        };
        let m = transport::parse_message(&raw)?;
        if matches!(m, Message::Stream { .. } | Message::Assistant { .. }) {
            run.lock().timer.output();
        }
        match m {
            Message::User { blocks, uuid } => {
                if blocks
                    .as_ref()
                    .is_some_and(|b| b.iter().any(|b| matches!(b, Block::ToolResult)))
                {
                    run.lock().timer.handed_back();
                    y.emit(event(K::Progress, json!({}))).await?;
                }
                if let Value::String(id) = &uuid {
                    run.lock().pending_updates.remove(id);
                }
                continue;
            }
            Message::System { subtype, data } => {
                if subtype.as_str() == Some("init") {
                    let source = data.get("apiKeySource").cloned().unwrap_or(json!("none"));
                    if source.as_str() != Some("none") {
                        run.cancel().await;
                        let shown = match &source {
                            Value::String(s) => repr_str(s),
                            other => crate::pyjson::py_repr(other),
                        };
                        y.emit(event(
                            K::Violation,
                            json!({"auth": format!(
                            "API key source {shown}; only the subscription login is allowed")}),
                        ))
                        .await?;
                        return Ok(());
                    }
                    run.lock().checked_auth = true;
                }
            }
            Message::Stream {
                event: ev,
                parent_tool_use_id,
            } => {
                if !run.lock().checked_auth || !parent_tool_use_id.is_null() {
                    continue;
                }
                let get = |k: &str| -> Result<Option<Value>, Raised> {
                    Ok(crate::pyjson::get(&ev, k)?.cloned())
                };
                let kind = get("type")?;
                let index_key = |v: Option<Value>| -> Result<String, Raised> {
                    match v {
                        Some(Value::Array(_)) | Some(Value::Object(_)) => {
                            Err(Raised::new("TypeError", "unhashable type"))
                        }
                        Some(v) => Ok(ferb_py::json::dumps(&normalize_number(&v))),
                        None => Ok("null".into()),
                    }
                };
                match kind.as_ref().and_then(Value::as_str) {
                    Some("message_start") => structured_blocks.clear(),
                    Some("content_block_start") => {
                        let block = match get("content_block")? {
                            Some(b) if ferb_py::value::truthy(&b) => b,
                            _ => json!({}),
                        };
                        let btype = crate::pyjson::get(&block, "type")?.cloned();
                        let name = crate::pyjson::get(&block, "name")?.cloned();
                        let is_answer = (btype.as_ref().and_then(Value::as_str)
                            == Some("tool_use")
                            && name.as_ref().and_then(Value::as_str) == Some(STRUCTURED_OUTPUT))
                            || (btype.as_ref().and_then(Value::as_str) == Some("text")
                                && typed.starts_with("stated"));
                        if is_answer {
                            let index = ev
                                .get("index")
                                .cloned()
                                .ok_or_else(|| Raised::key_error("index"))?;
                            structured_blocks.insert(index_key(Some(index))?);
                        }
                    }
                    Some("content_block_delta")
                        if structured_blocks.contains(&index_key(get("index")?)?) =>
                    {
                        let delta = match get("delta")? {
                            Some(d) if ferb_py::value::truthy(&d) => d,
                            _ => json!({}),
                        };
                        let dtype = crate::pyjson::get(&delta, "type")?.cloned();
                        let piece = match dtype.as_ref().and_then(Value::as_str) {
                            Some("input_json_delta") => {
                                crate::pyjson::get(&delta, "partial_json")?.cloned()
                            }
                            Some("text_delta") => crate::pyjson::get(&delta, "text")?.cloned(),
                            _ => None,
                        };
                        if let Some(p) = piece.filter(ferb_py::value::truthy) {
                            run.lock().emitted = true;
                            y.emit(event(K::TextDelta, json!({"text": p}))).await?;
                            continue;
                        }
                    }
                    Some("content_block_stop") => {
                        structured_blocks.remove(&index_key(get("index")?)?);
                    }
                    _ => {}
                }
                y.emit(event(K::Progress, json!({}))).await?;
            }
            Message::Assistant { blocks, error } => {
                let account = account_error(&match &error {
                    Value::Null => String::new(),
                    v if !ferb_py::value::truthy(v) => String::new(),
                    v => ferb_py::value::py_str(v),
                });
                if !account.is_empty() {
                    run.lock().account = account.into();
                }
                for b in blocks {
                    run.lock().blocks.push(b.kind().into());
                    match b {
                        Block::ToolUse { name, input, .. } => {
                            run.lock().emitted = true;
                            if name.as_str() == Some(STRUCTURED_OUTPUT) {
                                run.lock().structured = Some(input);
                            } else {
                                let offered = name.as_str().is_some_and(|n| rt.allowed.contains(n));
                                let mut data = json!({"tool": name, "arguments": input});
                                if !offered {
                                    data["offered"] = json!(false);
                                }
                                y.emit(event(K::ToolCall, data)).await?;
                            }
                        }
                        Block::Text(text) if !text.is_empty() => {
                            run.lock().emitted = true;
                            y.emit(event(K::Message, json!({"text": text}))).await?;
                        }
                        _ => {}
                    }
                }
            }
            Message::Result(m) => {
                if result_message(run, client, &m, y).await? {
                    return Ok(());
                }
            }
            Message::Other => {}
        }
    }
}

fn normalize_number(v: &Value) -> Value {
    match v {
        Value::Number(n) if n.is_f64() => match n.as_f64() {
            Some(f) if f.fract() == 0.0 => json!(f as i64),
            _ => v.clone(),
        },
        Value::Bool(b) => json!(i64::from(*b)),
        other => other.clone(),
    }
}

fn opt(m: &Value, key: &str) -> Value {
    m.get(key).cloned().unwrap_or(Value::Null)
}

/// One ResultMessage; true when the run's events end here.
async fn result_message(
    run: &Arc<ClaudeRun>,
    client: &Client,
    m: &Value,
    y: &mut Yielder,
) -> Result<bool, Exit> {
    let rt = &run.rt;
    let typed = run.lock().typed.clone();
    let is_error = opt(m, "is_error");
    let is_error = ferb_py::value::truthy(&is_error);
    let result_text = match opt(m, "result") {
        Value::String(s) => s,
        Value::Null => String::new(),
        other => ferb_py::value::py_str(&other),
    };
    let subtype = ferb_py::value::py_str(&opt(m, "subtype"));
    // total_cost_usd is cumulative across one session's turns; usage is per turn
    let mut cost = match opt(m, "total_cost_usd") {
        Value::Null => None,
        v => v.as_f64(),
    };
    if let Some(c) = cost {
        let mut st = run.lock();
        let so_far = st.cost_so_far;
        st.cost_so_far = c;
        cost = Some((c - so_far).max(0.0));
    }
    let structured_output = opt(m, "structured_output");
    let empty = {
        let st = run.lock();
        !is_error
            && st.pending_updates.is_empty()
            && structured_output.is_null()
            && st.structured.is_none()
            && ferb_py::value::strip(&result_text).is_empty()
    };
    let mut follow = String::new();
    {
        let mut st = run.lock();
        if st.follow_up == "asked" && st.pending_updates.is_empty() {
            follow = if empty || is_error { "failed" } else { "fixed" }.into();
            st.follow_up = "done".into();
        } else if empty && st.follow_up.is_empty() {
            follow = "asked".into();
        }
    }
    let usage_raw = match opt(m, "usage") {
        Value::Object(u) => u,
        Value::Null => Map::new(),
        v if !ferb_py::value::truthy(&v) => Map::new(),
        other => {
            return Err(Exit::Fail(RuntimeError::Raised(Raised::new(
                "TypeError",
                format!("'{}' object is not iterable", crate::error::py_type(&other)),
            ))))
        }
    };
    let usage = crate::usage::from_anthropic(
        rt.model.as_deref().unwrap_or("claude"),
        &usage_raw,
        "subscription",
        cost,
        &(rt.host.today)(),
    )?;
    let mut data = usage.as_dict();
    data.insert("timing".into(), Value::Object(run.lock().timer.done()));
    if !typed.is_empty() {
        data.insert("typed".into(), json!(typed));
    }
    if !empty {
        if let Some(mark) = repair_mark(run, m, &typed, is_error) {
            data.insert("repair".into(), json!(mark));
        }
    }
    let stop = match opt(m, "stop_reason") {
        v if ferb_py::value::truthy(&v) => ferb_py::value::py_str(&v),
        _ => {
            let s = opt(m, "subtype");
            if ferb_py::value::truthy(&s) {
                ferb_py::value::py_str(&s)
            } else {
                String::new()
            }
        }
    };
    data.insert("stop".into(), json!(stop));
    data.insert(
        "blocks".into(),
        json!(std::mem::take(&mut run.lock().blocks)),
    );
    if !follow.is_empty() {
        data.insert("follow_up".into(), json!(follow));
    }
    y.emit(event(K::Usage, Value::Object(data))).await?;
    if is_error {
        let emitted = run.lock().emitted;
        let status_400 = opt(m, "api_error_status").as_i64() == Some(400);
        if typed == "strict" && status_400 && !emitted && subtype != STRUCTURED_RETRIES {
            run.lock().restart = true; // RUN-427: once more without the typed format
            return Ok(true);
        }
        run.lock().closed = true;
        let detail = if !result_text.is_empty() {
            result_text.clone()
        } else {
            // str(errors): "None" when absent, which is never empty
            let shown = match opt(m, "errors") {
                Value::Null => "None".to_string(),
                Value::String(s) => s,
                v @ (Value::Array(_) | Value::Object(_)) => crate::pyjson::py_repr(&v),
                v => ferb_py::value::py_str(&v),
            };
            if !shown.is_empty() {
                shown
            } else {
                subtype.clone()
            }
        };
        let kind = if subtype == STRUCTURED_RETRIES {
            K::InvalidAnswer
        } else {
            K::Error
        };
        let mut data = json!({"message": detail});
        let account = run.lock().account.clone();
        if !account.is_empty() {
            data["account"] = json!(account);
        }
        y.emit(event(kind, data)).await?;
        return Ok(true);
    }
    let extra: Vec<String> = rt
        .proxy
        .seen(&run.id)
        .difference(&rt.allowed)
        .cloned()
        .collect();
    if !extra.is_empty() {
        run.cancel().await;
        y.emit(event(K::Violation, json!({"unexpected_tools": extra})))
            .await?;
        return Ok(true);
    }
    if !run.lock().pending_updates.is_empty() {
        // a late result from the previous input can't finish accepted corrections
        run.lock().structured = None;
        return Ok(false);
    }
    if empty {
        if follow == "asked" {
            run.lock().follow_up = "asked".into();
            steer_inner(run, client, FOLLOW_UP).await?;
            return Ok(false);
        }
        run.lock().closed = true;
        y.emit(event(
            K::Error,
            json!({"message": "claude: the model ended without an answer twice", "empty": true}),
        ))
        .await?;
        return Ok(true);
    }
    let structured = if !structured_output.is_null() {
        Some(structured_output)
    } else {
        run.lock().structured.clone()
    };
    let mut text = result_text.clone();
    if let Some(s) = &structured {
        let value = rt.shape.as_ref().map_or(s.clone(), |sh| sh.value(s));
        text = ferb_py::json::dumps(&value);
    } else if let Some(sh) = &rt.shape {
        text = sh.answer_text(&if typed.starts_with("stated") {
            unfenced(&text)
        } else {
            text.clone()
        });
    }
    if typed == "stated_after_refusal" && !run.lock().remembered {
        run.lock().remembered = true;
        rt.refusals.remember(&rt.format_key);
    }
    if typed.starts_with("stated") && !run.lock().repaired {
        if let Some(schema) = &rt.output_schema {
            if let Err(e) = rt.forms.check_final(&text, schema) {
                run.lock().repaired = true;
                steer_inner(run, client, &repair_note(&e)).await?;
                return Ok(false);
            }
        }
    }
    run.lock().closed = true;
    let seen: Vec<String> = rt.proxy.seen(&run.id).into_iter().collect();
    y.emit(event(K::Final, json!({"text": text, "tools_seen": seen})))
        .await?;
    Ok(true)
}

/// `continue_` from inside the run's own events (FOLLOW_UP, a repair note).
async fn steer_inner(run: &Arc<ClaudeRun>, _client: &Client, message: &str) -> Result<(), Exit> {
    run.steer(message).await.map_err(Exit::Fail)
}

/// claude_sdk.py `_repair_mark` (RUN-440).
fn repair_mark(run: &ClaudeRun, m: &Value, typed: &str, is_error: bool) -> Option<&'static str> {
    if !typed.starts_with("stated") || is_error || !run.lock().pending_updates.is_empty() {
        return None;
    }
    let rt = &run.rt;
    let structured = match opt(m, "structured_output") {
        Value::Null => run.lock().structured.clone(),
        v => Some(v),
    };
    let text = match structured {
        Some(s) => ferb_py::json::dumps(&rt.shape.as_ref().map_or(s.clone(), |sh| sh.value(&s))),
        None => {
            let t = match opt(m, "result") {
                Value::String(s) => s,
                _ => String::new(),
            };
            match &rt.shape {
                Some(sh) => sh.answer_text(&unfenced(&t)),
                None => t,
            }
        }
    };
    let fits = rt
        .output_schema
        .as_ref()
        .is_none_or(|schema| rt.forms.check_final(&text, schema).is_ok());
    if run.lock().repaired {
        return Some(if fits { "fixed" } else { "failed" });
    }
    if fits {
        None
    } else {
        Some("asked")
    }
}

impl RunControl for ClaudeRun {
    fn steer<'a>(
        &'a self,
        message: &'a str,
    ) -> ferb_core::interfaces::BoxFuture<'a, Result<(), RuntimeError>> {
        Box::pin(ClaudeRun::steer(self, message))
    }

    fn cancel(&self) -> ferb_core::interfaces::BoxFuture<'_, ()> {
        Box::pin(ClaudeRun::cancel(self))
    }
}

impl crate::run::Adapter for ClaudeRuntime {
    fn name(&self) -> &str {
        "claude"
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
