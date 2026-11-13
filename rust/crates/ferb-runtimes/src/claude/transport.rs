//! What the pinned claude-agent-sdk 0.2.174 does between FERB and the bundled Claude Code, without Python (ENG-2):
//! the program's argv and environment (`_internal/transport/subprocess_cli.py` `_build_command`, `connect`), its
//! `-v` probe, the stream-json frames on stdin (`client.py` `query`), the control protocol (`_internal/query.py`:
//! the `initialize` request, answers to the program's own control requests, frames meant for the SDK only), the
//! stdout reader with its 1 MB line limit, the message parser (`_internal/message_parser.py`) and the shutdown
//! (`close`: stdin closed, 5 s, SIGTERM, 5 s, SIGKILL).

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Map, Value};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;

use crate::error::{py_type, Raised, RuntimeError};
use crate::process::{ProcControl, Spawn, Spawner};

/// The pinned SDK whose behavior this reproduces, and the Claude Code it bundles (claude_sdk.py:46-48).
pub const SDK_VERSION: &str = "0.2.174";
pub const CLAUDE_CODE_VERSION: &str = "2.1.310";
/// subprocess_cli.py `_DEFAULT_MAX_BUFFER_SIZE`: one stdout message at most 1 MB.
pub const MAX_BUFFER: usize = 1024 * 1024;
const SDK_READS_SESSION_STATE: &str = "CLAUDE_CODE_SDK_READS_SESSION_STATE";

fn raised(class: &str, message: impl Into<String>) -> RuntimeError {
    RuntimeError::Raised(Raised::new(class, message))
}

/// The options claude_sdk.py `_options` sets, as subprocess_cli.py turns them into flags.
#[derive(Debug, Clone, Default)]
pub struct Options {
    pub allowed_tools: Vec<String>,
    pub model: Option<String>,
    pub mcp_servers: Option<Value>,
    pub include_partial_messages: bool,
    pub effort: Option<String>,
    /// `--json-schema` (output_format json_schema).
    pub json_schema: Option<Value>,
    /// ClaudeAgentOptions.env.
    pub env: BTreeMap<String, String>,
}

/// subprocess_cli.py `_build_command` for FERB's options (tools=[], dontAsk, no setting sources, strict MCP, no
/// plugins, agents or skills, `no-session-persistence` and `replay-user-messages`).
pub fn command(cli: &str, o: &Options) -> Vec<String> {
    let mut cmd: Vec<String> = [cli, "--output-format", "stream-json", "--verbose"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    cmd.extend(["--system-prompt".into(), String::new()]);
    cmd.extend(["--tools".into(), String::new()]);
    if !o.allowed_tools.is_empty() {
        cmd.extend(["--allowedTools".into(), o.allowed_tools.join(",")]);
    }
    if let Some(m) = o.model.as_ref().filter(|m| !m.is_empty()) {
        cmd.extend(["--model".into(), m.clone()]);
    }
    cmd.extend(["--permission-mode".into(), "dontAsk".into()]);
    if let Some(servers) = o.mcp_servers.as_ref().filter(|s| ferb_py::value::truthy(s)) {
        cmd.extend([
            "--mcp-config".into(),
            ferb_py::json::dumps(&json!({"mcpServers": servers})),
        ]);
    }
    if o.include_partial_messages {
        cmd.push("--include-partial-messages".into());
    }
    cmd.push("--strict-mcp-config".into());
    cmd.push("--setting-sources=".into());
    cmd.push("--no-session-persistence".into());
    cmd.push("--replay-user-messages".into());
    if let Some(e) = &o.effort {
        cmd.extend(["--effort".into(), e.clone()]);
    }
    if let Some(schema) = &o.json_schema {
        cmd.extend(["--json-schema".into(), ferb_py::json::dumps(schema)]);
    }
    cmd.extend(["--input-format".into(), "stream-json".into()]);
    cmd
}

/// subprocess_cli.py `connect`'s environment: the daemon's own without CLAUDECODE, the entry point, the options'
/// env, the SDK version, and the session-state frames the SDK asks for.
pub fn environment(environ: &BTreeMap<String, String>, o: &Options) -> BTreeMap<String, String> {
    let mut env: BTreeMap<String, String> = environ
        .iter()
        .filter(|(k, _)| k.as_str() != "CLAUDECODE")
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    env.insert("CLAUDE_CODE_ENTRYPOINT".into(), "sdk-py".into());
    for (k, v) in &o.env {
        env.insert(k.clone(), v.clone());
    }
    env.insert("CLAUDE_AGENT_SDK_VERSION".into(), SDK_VERSION.into());
    if !env
        .keys()
        .any(|k| k.to_uppercase() == SDK_READS_SESSION_STATE)
    {
        env.insert(SDK_READS_SESSION_STATE.into(), "1".into());
    }
    env
}

/// Ids the SDK draws at random: a control request's suffix (`os.urandom(4).hex()`).
pub type Urandom4 = Arc<dyn Fn() -> String + Send + Sync>;

struct Shared {
    pending: Mutex<HashMap<String, oneshot::Sender<Result<Value, RuntimeError>>>>,
    closed: Mutex<bool>,
    last_error_result: Mutex<Option<Value>>,
}

/// One connected Claude Code: its stdin, its messages (control traffic already taken out), its process.
pub struct Client {
    control: Arc<dyn ProcControl>,
    stdin: Arc<tokio::sync::Mutex<Option<Box<dyn AsyncWrite + Send + Unpin>>>>,
    messages: tokio::sync::Mutex<mpsc::Receiver<Result<Value, RuntimeError>>>,
    reader: Mutex<Option<JoinHandle<()>>>,
    handlers: Arc<Mutex<Vec<JoinHandle<()>>>>,
    shared: Arc<Shared>,
    counter: Mutex<u64>,
    urandom4: Urandom4,
    disconnected: tokio::sync::Mutex<bool>,
    receive_closed: std::sync::atomic::AtomicBool,
}

impl Client {
    /// ClaudeSDKClient.connect: the `-v` probe, the program, the reader, the `initialize` request. A failure after
    /// the program started disconnects it first.
    pub async fn connect(
        spawner: &dyn Spawner,
        cli: &str,
        environ: &BTreeMap<String, String>,
        options: &Options,
        urandom4: Urandom4,
    ) -> Result<Client, RuntimeError> {
        if !environ.contains_key("CLAUDE_AGENT_SDK_SKIP_VERSION_CHECK") {
            version_probe(spawner, cli, environ).await;
        }
        let mut spec = Spawn::new(command(cli, options), environment(environ, options));
        // stderr is the daemon's own: the SDK pipes it only for a stderr callback, and FERB sets none
        spec.stderr = false;
        let child = spawner.spawn(spec).map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                raised(
                    "CLINotFoundError",
                    format!("Claude Code not found at: {cli}"),
                )
            } else {
                raised(
                    "CLIConnectionError",
                    format!("Failed to start Claude Code: {e}"),
                )
            }
        })?;
        let (tx, rx) = mpsc::channel(100);
        let shared = Arc::new(Shared {
            pending: Mutex::new(HashMap::new()),
            closed: Mutex::new(false),
            last_error_result: Mutex::new(None),
        });
        let stdin = Arc::new(tokio::sync::Mutex::new(child.stdin));
        let handlers = Arc::new(Mutex::new(Vec::new()));
        let client = Client {
            control: child.control.clone(),
            stdin: stdin.clone(),
            messages: tokio::sync::Mutex::new(rx),
            reader: Mutex::new(None),
            handlers: handlers.clone(),
            shared: shared.clone(),
            counter: Mutex::new(0),
            urandom4,
            disconnected: tokio::sync::Mutex::new(false),
            receive_closed: Default::default(),
        };
        if let Some(stdout) = child.stdout {
            let task = tokio::spawn(read_messages(
                stdout,
                tx,
                shared,
                stdin,
                handlers,
                child.control.clone(),
            ));
            *client.reader.lock().unwrap_or_else(|e| e.into_inner()) = Some(task);
        }
        let init = client
            .control_request(json!({"subtype": "initialize", "hooks": null}), 60.0)
            .await;
        if let Err(e) = init {
            client.disconnect().await;
            return Err(e);
        }
        Ok(client)
    }

    /// `Transport.write`: one frame, as the SDK's write lock serializes them.
    pub async fn write(&self, line: &str) -> Result<(), RuntimeError> {
        let mut stdin = self.stdin.lock().await;
        let Some(w) = stdin.as_mut() else {
            return Err(raised(
                "CLIConnectionError",
                "ProcessTransport is not ready for writing",
            ));
        };
        if let Some(code) = self.control.returncode() {
            return Err(raised(
                "CLIConnectionError",
                format!("Cannot write to terminated process (exit code: {code})"),
            ));
        }
        let out = async {
            w.write_all(line.as_bytes()).await?;
            w.flush().await
        }
        .await;
        out.map_err(|e| {
            raised(
                "CLIConnectionError",
                format!("Failed to write to process stdin: {e}"),
            )
        })
    }

    /// query.py `_send_control_request`.
    async fn control_request(&self, request: Value, timeout_s: f64) -> Result<Value, RuntimeError> {
        let n = {
            let mut c = self.counter.lock().unwrap_or_else(|e| e.into_inner());
            *c += 1;
            *c
        };
        let request_id = format!("req_{n}_{}", (self.urandom4)());
        let (tx, rx) = oneshot::channel();
        self.shared
            .pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(request_id.clone(), tx);
        let subtype = ferb_py::value::py_str(request.get("subtype").unwrap_or(&Value::Null));
        let frame =
            json!({"type": "control_request", "request_id": request_id, "request": request});
        self.write(&format!("{}\n", ferb_py::json::dumps(&frame)))
            .await?;
        match tokio::time::timeout(Duration::from_secs_f64(timeout_s), rx).await {
            Ok(Ok(Ok(response))) => Ok(match response.get("response") {
                Some(v @ Value::Object(_)) => v.clone(),
                _ => json!({}),
            }),
            Ok(Ok(Err(e))) => Err(e),
            Ok(Err(_)) => Err(raised("Exception", "Unknown error")),
            Err(_) => {
                self.shared
                    .pending
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .remove(&request_id);
                Err(raised(
                    "Exception",
                    format!("Control request timeout: {subtype}"),
                ))
            }
        }
    }

    /// ClaudeSDKClient.query with message frames: each gets `session_id` "default" when it has none.
    pub async fn query(&self, frames: Vec<Value>) -> Result<(), RuntimeError> {
        for mut frame in frames {
            if let Value::Object(m) = &mut frame {
                if !m.contains_key("session_id") {
                    m.insert("session_id".into(), json!("default"));
                }
            }
            self.write(&format!("{}\n", ferb_py::json::dumps(&frame)))
                .await?;
        }
        Ok(())
    }

    /// The next SDK message as the CLI wrote it (query.py `receive_messages`); `None` at the end of the stream.
    /// After `disconnect` the SDK's receive stream is closed: reading it raises anyio's ClosedResourceError.
    pub async fn next_raw(&self) -> Option<Result<Value, RuntimeError>> {
        use std::sync::atomic::Ordering;
        if self.receive_closed.load(Ordering::SeqCst) {
            return Some(Err(raised("ClosedResourceError", "")));
        }
        let mut rx = self.messages.lock().await;
        let item = rx.recv().await;
        if self.receive_closed.load(Ordering::SeqCst) {
            return Some(Err(raised("ClosedResourceError", "")));
        }
        item
    }

    /// ClaudeSDKClient.disconnect: the reader and request handlers stopped, stdin closed, then up to 5 s for the
    /// program to end, SIGTERM, 5 s, SIGKILL.
    pub async fn disconnect(&self) {
        let mut done = self.disconnected.lock().await;
        if *done {
            return;
        }
        *done = true;
        *self.shared.closed.lock().unwrap_or_else(|e| e.into_inner()) = true;
        self.receive_closed
            .store(true, std::sync::atomic::Ordering::SeqCst);
        for h in self
            .handlers
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .drain(..)
        {
            h.abort();
        }
        let reader = self.reader.lock().unwrap_or_else(|e| e.into_inner()).take();
        if let Some(r) = reader {
            r.abort();
            let _ = r.await;
        }
        if let Ok(Some(mut w)) = tokio::time::timeout(Duration::from_secs(5), self.stdin.lock())
            .await
            .map(|mut g| g.take())
        {
            let _ = w.shutdown().await;
        }
        if self.control.returncode().is_none() {
            let wait = tokio::time::timeout(Duration::from_secs(5), self.control.wait()).await;
            if wait.is_err() {
                self.control.terminate();
                if tokio::time::timeout(Duration::from_secs(5), self.control.wait())
                    .await
                    .is_err()
                {
                    self.control.kill();
                    let _ = tokio::time::timeout(Duration::from_secs(5), self.control.wait()).await;
                }
            }
        }
    }
}

/// subprocess_cli.py `_check_claude_version`: `claude -v` with a 2 s limit; its answer only ever logged a warning.
async fn version_probe(spawner: &dyn Spawner, cli: &str, environ: &BTreeMap<String, String>) {
    let mut spec = Spawn::new(vec![cli.to_string(), "-v".into()], environ.clone());
    spec.stdin = false;
    let Ok(mut child) = spawner.spawn(spec) else {
        return;
    };
    if let Some(mut out) = child.stdout.take() {
        let mut buf = [0u8; 65536];
        let _ = tokio::time::timeout(Duration::from_secs(2), out.read(&mut buf)).await;
    }
    child.control.terminate();
    let _ = child.control.wait().await;
}

/// subprocess_cli.py `_LineFramer` + `_parse_stdout_line` + query.py `_read_messages`.
async fn read_messages(
    mut stdout: Box<dyn AsyncRead + Send + Unpin>,
    tx: mpsc::Sender<Result<Value, RuntimeError>>,
    shared: Arc<Shared>,
    stdin: Arc<tokio::sync::Mutex<Option<Box<dyn AsyncWrite + Send + Unpin>>>>,
    handlers: Arc<Mutex<Vec<JoinHandle<()>>>>,
    control: Arc<dyn ProcControl>,
) {
    let mut pending: Vec<u8> = Vec::new();
    let mut chunk = vec![0u8; 65536];
    let failure: Option<RuntimeError> = 'read: loop {
        let n = match stdout.read(&mut chunk).await {
            Ok(0) | Err(_) => break 'read None,
            Ok(n) => n,
        };
        pending.extend_from_slice(&chunk[..n]);
        while let Some(i) = pending.iter().position(|b| *b == b'\n') {
            let line: Vec<u8> = pending.drain(..=i).collect();
            let text = String::from_utf8_lossy(&line[..line.len() - 1]).into_owned();
            if text.chars().count() > MAX_BUFFER {
                break 'read Some(raised(
                    "CLIJSONDecodeError",
                    format!(
                        "Failed to decode JSON: JSON message exceeded maximum buffer size of {MAX_BUFFER} bytes..."
                    ),
                ));
            }
            match parse_line(&text) {
                Err(e) => break 'read Some(e),
                Ok(None) => {}
                Ok(Some(message)) => {
                    if *shared.closed.lock().unwrap_or_else(|e| e.into_inner()) {
                        break 'read None;
                    }
                    if !route(message, &tx, &shared, &stdin, &handlers).await {
                        break 'read None;
                    }
                }
            }
        }
        if String::from_utf8_lossy(&pending).chars().count() > MAX_BUFFER {
            break 'read Some(raised(
                "CLIJSONDecodeError",
                format!(
                    "Failed to decode JSON: JSON message exceeded maximum buffer size of {MAX_BUFFER} bytes..."
                ),
            ));
        }
    };
    let failure = match failure {
        Some(f) => Some(f),
        None => {
            // the tail without a newline: yielded when whole, dropped when cut off
            let tail = String::from_utf8_lossy(&pending).into_owned();
            if let Ok(Some(message)) = parse_line(&tail) {
                route(message, &tx, &shared, &stdin, &handlers).await;
            }
            let code = control.wait().await;
            if code != 0 {
                let last = shared
                    .last_error_result
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .clone();
                Some(match last {
                    Some(result) => raised(
                        "ResultError",
                        format!(
                            "Claude Code returned an error result: {} (exit code: {code})",
                            error_result_text(&result)
                        ),
                    ),
                    None => raised(
                        "ProcessError",
                        format!(
                            "Command failed with exit code {code} (exit code: {code})\nError output: Check stderr output for details"
                        ),
                    ),
                })
            } else {
                None
            }
        }
    };
    if let Some(f) = failure {
        let waiting: Vec<_> = shared
            .pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .drain()
            .map(|(_, w)| w)
            .collect();
        for w in waiting {
            let _ = w.send(Err(f.clone()));
        }
        let _ = tx.send(Err(f)).await;
    }
}

/// query.py `_error_result_text`.
fn error_result_text(m: &Value) -> String {
    let errors = normalize_errors(m.get("errors"));
    if !errors.is_empty() {
        return errors.join("; ");
    }
    if let Some(Value::String(r)) = m.get("result") {
        let r = ferb_py::value::strip(r);
        if !r.is_empty() {
            return r.to_string();
        }
    }
    if let Some(Value::String(s)) = m.get("subtype") {
        if !s.is_empty() && s != "success" {
            return s.clone();
        }
    }
    match m.get("api_error_status") {
        Some(v) if !v.is_null() => format!("API error (HTTP {})", ferb_py::value::py_str(v)),
        _ => "unknown error".into(),
    }
}

fn normalize_errors(raw: Option<&Value>) -> Vec<String> {
    let items = match raw {
        Some(Value::String(s)) => vec![Value::String(s.clone())],
        Some(Value::Array(a)) => a.clone(),
        _ => return Vec::new(),
    };
    items
        .iter()
        .filter_map(Value::as_str)
        .map(ferb_py::value::strip)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

fn parse_line(line: &str) -> Result<Option<Value>, RuntimeError> {
    let line = ferb_py::value::strip(line);
    if line.is_empty() || !line.starts_with('{') {
        return Ok(None);
    }
    match ferb_py::json::loads(line) {
        Ok(v) => Ok(Some(v)),
        Err(_) => Err(raised(
            "CLIJSONDecodeError",
            format!(
                "Failed to decode JSON: {}...",
                ferb_py::value::prefix(line, 100)
            ),
        )),
    }
}

/// query.py `_read_messages`' routing; false when the consumer is gone.
async fn route(
    message: Value,
    tx: &mpsc::Sender<Result<Value, RuntimeError>>,
    shared: &Arc<Shared>,
    stdin: &Arc<tokio::sync::Mutex<Option<Box<dyn AsyncWrite + Send + Unpin>>>>,
    handlers: &Arc<Mutex<Vec<JoinHandle<()>>>>,
) -> bool {
    let kind = message.get("type").and_then(Value::as_str).unwrap_or("");
    match kind {
        "control_response" => {
            let response = message
                .get("response")
                .cloned()
                .unwrap_or_else(|| json!({}));
            let id = response
                .get("request_id")
                .map(ferb_py::value::py_str)
                .unwrap_or_default();
            let waiter = shared
                .pending
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(&id);
            if let Some(w) = waiter {
                let out = if response.get("subtype").and_then(Value::as_str) == Some("error") {
                    let words = match response.get("error") {
                        Some(v) => ferb_py::value::py_str(v),
                        None => "Unknown error".into(),
                    };
                    Err(raised("Exception", words))
                } else {
                    Ok(response)
                };
                let _ = w.send(out);
            }
            return true;
        }
        "control_request" => {
            // FERB's options give the SDK no hooks, permission callback or SDK servers: every request the
            // program makes is answered with the error the SDK writes for it
            let stdin = stdin.clone();
            let h = tokio::spawn(async move {
                let request_id = message.get("request_id").cloned().unwrap_or(Value::Null);
                let data = message.get("request").cloned().unwrap_or(Value::Null);
                let subtype = data.get("subtype").map(ferb_py::value::py_str);
                let reply = match subtype.as_deref() {
                    Some("can_use_tool") => {
                        error_reply(&request_id, "canUseTool callback is not provided")
                    }
                    Some("hook_callback") => error_reply(
                        &request_id,
                        &format!(
                            "No hook callback found for ID: {}",
                            ferb_py::value::py_str(data.get("callback_id").unwrap_or(&Value::Null))
                        ),
                    ),
                    Some("mcp_message") => {
                        let server = data
                            .get("server_name")
                            .filter(|v| ferb_py::value::truthy(v));
                        let msg = data.get("message").filter(|v| ferb_py::value::truthy(v));
                        match (server, msg) {
                            (Some(server), Some(msg)) => json!({"type": "control_response",
                                "response": {"subtype": "success", "request_id": request_id,
                                    "response": {"mcp_response": {"jsonrpc": "2.0", "id": msg.get("id"),
                                        "error": {"code": -32601, "message": format!("Server '{}' not found",
                                            ferb_py::value::py_str(server))}}}}}),
                            _ => error_reply(
                                &request_id,
                                "Missing server_name or message for MCP request",
                            ),
                        }
                    }
                    Some(other) => error_reply(
                        &request_id,
                        &format!("Unsupported control request subtype: {other}"),
                    ),
                    None => return,
                };
                let mut w = stdin.lock().await;
                if let Some(w) = w.as_mut() {
                    let line = format!("{}\n", ferb_py::json::dumps(&reply));
                    let _ = w.write_all(line.as_bytes()).await;
                    let _ = w.flush().await;
                }
            });
            handlers.lock().unwrap_or_else(|e| e.into_inner()).push(h);
            return true;
        }
        "control_cancel_request" | "transcript_mirror" => return true,
        _ => {}
    }
    if kind == "system"
        && message.get("subtype").and_then(Value::as_str) == Some("session_state_changed")
        && message.get("sdk_host_only") == Some(&Value::Bool(true))
    {
        return true;
    }
    if kind == "result" {
        let last = if message.get("is_error").is_some_and(ferb_py::value::truthy) {
            Some(message.clone())
        } else {
            None
        };
        *shared
            .last_error_result
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = last;
    } else if !(kind == "system"
        && message.get("subtype").and_then(Value::as_str) == Some("session_state_changed"))
    {
        *shared
            .last_error_result
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = None;
    }
    tx.send(Ok(message)).await.is_ok()
}

fn error_reply(request_id: &Value, words: &str) -> Value {
    json!({"type": "control_response",
        "response": {"subtype": "error", "request_id": request_id, "error": words}})
}

// ---------- message_parser.py ----------

/// A content block, by the SDK's classes.
#[derive(Debug, Clone, PartialEq)]
pub enum Block {
    Text(String),
    Thinking,
    ToolUse {
        id: Value,
        name: Value,
        input: Value,
    },
    ToolResult,
    ServerToolUse,
    ServerToolResult,
}

impl Block {
    /// claude_sdk.py `_kind`: the block's class name as the Messages API names it.
    pub fn kind(&self) -> &'static str {
        match self {
            Block::Text(_) => "text",
            Block::Thinking => "thinking",
            Block::ToolUse { .. } => "tool_use",
            Block::ToolResult => "tool_result",
            Block::ServerToolUse => "server_tool_use",
            Block::ServerToolResult => "server_tool_result",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Message {
    /// `content`: blocks when the message's content was a list, else None (text).
    User {
        blocks: Option<Vec<Block>>,
        uuid: Value,
    },
    Assistant {
        blocks: Vec<Block>,
        error: Value,
    },
    System {
        subtype: Value,
        data: Value,
    },
    Result(Value),
    Stream {
        event: Value,
        parent_tool_use_id: Value,
    },
    /// A message FERB's code ignores (hook events, rate limits, task progress, resets).
    Other,
}

fn parse_error(message: impl Into<String>) -> RuntimeError {
    raised("MessageParseError", message)
}

fn req<'a>(v: &'a Value, key: &str) -> Result<&'a Value, String> {
    match v {
        Value::Object(m) => m.get(key).ok_or_else(|| format!("'{key}'")),
        other => Err(format!("{} is not subscriptable", py_type(other))),
    }
}

fn blocks_of(raw: &[Value], assistant: bool) -> Result<Vec<Block>, RuntimeError> {
    let what = if assistant { "assistant" } else { "user" };
    let mut out = Vec::new();
    for block in raw {
        if !block.is_object() {
            return Err(parse_error(format!(
                "Invalid content block (expected dict, got {})",
                py_type(block)
            )));
        }
        let missing =
            |k: String| parse_error(format!("Missing required field in {what} message: {k}"));
        let kind = req(block, "type").map_err(missing)?;
        let b = match kind.as_str() {
            Some("text") => {
                Block::Text(ferb_py::value::py_str(req(block, "text").map_err(missing)?))
            }
            Some("thinking") if assistant => {
                req(block, "thinking").map_err(missing)?;
                req(block, "signature").map_err(missing)?;
                Block::Thinking
            }
            Some("tool_use") => Block::ToolUse {
                id: req(block, "id").map_err(missing)?.clone(),
                name: req(block, "name").map_err(missing)?.clone(),
                input: req(block, "input").map_err(missing)?.clone(),
            },
            Some("tool_result") => {
                req(block, "tool_use_id").map_err(missing)?;
                Block::ToolResult
            }
            Some("server_tool_use") if assistant => {
                req(block, "id").map_err(missing)?;
                req(block, "name").map_err(missing)?;
                req(block, "input").map_err(missing)?;
                Block::ServerToolUse
            }
            Some("advisor_tool_result") if assistant => {
                req(block, "tool_use_id").map_err(missing)?;
                req(block, "content").map_err(missing)?;
                Block::ServerToolResult
            }
            _ => continue,
        };
        out.push(b);
    }
    Ok(out)
}

/// message_parser.py `parse_message`.
pub fn parse_message(data: &Value) -> Result<Message, RuntimeError> {
    let Value::Object(m) = data else {
        return Err(parse_error(format!(
            "Invalid message data type (expected dict, got {})",
            py_type(data)
        )));
    };
    let kind = m.get("type");
    if kind.and_then(Value::as_str) == Some("system")
        && matches!(
            m.get("subtype").and_then(Value::as_str),
            Some("hook_started" | "hook_response")
        )
    {
        return Ok(Message::Other);
    }
    if !kind.is_some_and(ferb_py::value::truthy) {
        return Err(parse_error("Message missing 'type' field"));
    }
    let missing = |what: &str| {
        let what = what.to_string();
        move |k: String| parse_error(format!("Missing required field in {what} message: {k}"))
    };
    match kind.and_then(Value::as_str).unwrap_or("") {
        "user" => {
            let message = req(data, "message").map_err(missing("user"))?;
            let content = req(message, "content").map_err(missing("user"))?;
            let blocks = match content {
                Value::Array(items) => Some(blocks_of(items, false)?),
                _ => None,
            };
            Ok(Message::User {
                blocks,
                uuid: m.get("uuid").cloned().unwrap_or(Value::Null),
            })
        }
        "assistant" => {
            let message = req(data, "message").map_err(missing("assistant"))?;
            let content = req(message, "content").map_err(missing("assistant"))?;
            let Value::Array(items) = content else {
                return Err(parse_error(format!(
                    "Invalid assistant content (expected list, got {})",
                    py_type(content)
                )));
            };
            let blocks = blocks_of(items, true)?;
            req(message, "model").map_err(missing("assistant"))?;
            Ok(Message::Assistant {
                blocks,
                error: m.get("error").cloned().unwrap_or(Value::Null),
            })
        }
        "system" => {
            let subtype = req(data, "subtype").map_err(missing("system"))?;
            let needed: &[&str] = match subtype.as_str() {
                Some("task_started") => &["task_id", "description", "uuid", "session_id"],
                Some("task_progress") => &["task_id", "description", "usage", "uuid", "session_id"],
                Some("task_notification") => &[
                    "task_id",
                    "status",
                    "output_file",
                    "summary",
                    "uuid",
                    "session_id",
                ],
                _ => &[],
            };
            for k in needed {
                req(data, k).map_err(missing("system"))?;
            }
            Ok(Message::System {
                subtype: subtype.clone(),
                data: data.clone(),
            })
        }
        "result" => {
            for k in [
                "subtype",
                "duration_ms",
                "duration_api_ms",
                "is_error",
                "num_turns",
                "session_id",
            ] {
                req(data, k).map_err(missing("result"))?;
            }
            if let Some(d) = m
                .get("deferred_tool_use")
                .filter(|d| ferb_py::value::truthy(d))
            {
                for k in ["id", "name", "input"] {
                    req(d, k).map_err(missing("result"))?;
                }
            }
            Ok(Message::Result(data.clone()))
        }
        "stream_event" => {
            for k in ["uuid", "session_id", "event"] {
                req(data, k).map_err(missing("stream_event"))?;
            }
            Ok(Message::Stream {
                event: m["event"].clone(),
                parent_tool_use_id: m.get("parent_tool_use_id").cloned().unwrap_or(Value::Null),
            })
        }
        "rate_limit_event" => {
            let info = req(data, "rate_limit_info").map_err(missing("rate_limit_event"))?;
            req(info, "status").map_err(missing("rate_limit_event"))?;
            req(data, "uuid").map_err(missing("rate_limit_event"))?;
            req(data, "session_id").map_err(missing("rate_limit_event"))?;
            Ok(Message::Other)
        }
        "conversation_reset" => {
            for k in ["new_conversation_id", "uuid", "session_id"] {
                req(data, k).map_err(missing("conversation_reset"))?;
            }
            Ok(Message::Other)
        }
        _ => Ok(Message::Other),
    }
}

#[allow(dead_code)]
fn _unused(_: Map<String, Value>) {}
