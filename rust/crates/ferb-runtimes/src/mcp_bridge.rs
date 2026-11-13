//! The models' tool bridge (ferb/cli/mcp.py, `ferb mcp`): the stdio MCP server Codex and Claude Code start. No
//! logic of its own: it lists exactly the tools the gateway gives the run's token and forwards each call to FERB's
//! daemon over the RPC socket with the `mcp_shim` role and that token (AGENTS.md rule 12). Every decision is the
//! daemon's.
//!
//! The Python shim ran on the `mcp` library 2.2.0 (handshake era); this writes what that library wrote for the
//! requests the model programs send (rust/fixtures/runtimes/mcp.json): the `initialize` answer and its version
//! choice, `tools/list` and `tools/call` results in its key order, `ping`, "Method not found" for any other
//! method, -32602 for a request before `initialize` or with parameters it can't read, nothing for a line that
//! isn't a request, and no answer to a call the client cancelled (whose daemon connection is closed at once).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use ferb_identity::Secret;
use serde_json::{json, Map, Value};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::task::JoinHandle;

use crate::codex::mcp_name;

/// The versions the `initialize` handshake can agree on (mcp_types/version.py HANDSHAKE_PROTOCOL_VERSIONS).
pub const HANDSHAKE_VERSIONS: [&str; 4] = ["2024-11-05", "2025-03-26", "2025-06-18", "2025-11-25"];
pub const LATEST_HANDSHAKE: &str = "2025-11-25";

/// cli/mcp.py `Shim.__init__`: FERB_RUN_TOKEN, else the first line of the 0600 file FERB_RUN_TOKEN_FILE names.
pub fn token_from(env: &HashMap<String, String>) -> Option<Secret> {
    if let Some(t) = env.get("FERB_RUN_TOKEN").filter(|t| !t.is_empty()) {
        return Some(Secret::from(t.as_str()));
    }
    let file = env.get("FERB_RUN_TOKEN_FILE").filter(|f| !f.is_empty())?;
    let text = std::fs::read_to_string(file).ok()?;
    let token = ferb_py::value::strip(&text);
    (!token.is_empty()).then(|| Secret::from(token))
}

/// What the shim prints and exits 1 with when no runtime FERB started gave it a token.
pub const NO_TOKEN: &str =
    "ferb mcp: FERB_RUN_TOKEN is not set (only runtimes started by FERB may use this)";

/// One daemon call's failure, as the shim tells it.
#[derive(Debug)]
enum CallError {
    /// The daemon's own refusal (cli/client.py RpcError): its message.
    Rpc(String),
    /// The connection broke (NotRunning, OSError, ValueError): the class name and words.
    Broken(String, String),
}

/// One request on its own connection (cli/client.py `Client`): hello with the role and token, then the call.
async fn daemon_call(
    socket: &Path,
    token: &Secret,
    method: &str,
    params: Value,
) -> Result<Value, CallError> {
    let stream = UnixStream::connect(socket)
        .await
        .map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused => {
                CallError::Broken(
                    "NotRunning".into(),
                    "FERB daemon is not running (start it with `ferb daemon run`)".into(),
                )
            }
            _ => {
                let r = crate::error::os_error(&e);
                CallError::Broken(r.class, r.message)
            }
        })?;
    let (read, mut write) = stream.into_split();
    let mut read = BufReader::new(read);
    let mut next_id = 0i64;
    let mut call = |method: &str, params: Value| {
        next_id += 1;
        (
            next_id,
            format!(
                "{}\n",
                ferb_py::json::dumps(
                    &json!({"jsonrpc": "2.0", "id": next_id, "method": method, "params": params})
                )
            ),
        )
    };
    let mut exchange = async |line: (i64, String)| -> Result<Value, CallError> {
        write
            .write_all(line.1.as_bytes())
            .await
            .map_err(|e| match e.kind() {
                std::io::ErrorKind::BrokenPipe | std::io::ErrorKind::ConnectionReset => {
                    CallError::Broken("NotRunning".into(), "FERB daemon went away".into())
                }
                _ => {
                    let r = crate::error::os_error(&e);
                    CallError::Broken(r.class, r.message)
                }
            })?;
        loop {
            let mut buf = String::new();
            let n = read.read_line(&mut buf).await.map_err(|e| match e.kind() {
                std::io::ErrorKind::ConnectionReset => {
                    CallError::Broken("NotRunning".into(), "FERB daemon went away".into())
                }
                _ => {
                    let r = crate::error::os_error(&e);
                    CallError::Broken(r.class, r.message)
                }
            })?;
            if n == 0 {
                return Err(CallError::Broken(
                    "NotRunning".into(),
                    "FERB daemon closed the connection".into(),
                ));
            }
            let msg = ferb_py::json::loads(&buf)
                .map_err(|e| CallError::Broken("JSONDecodeError".into(), e))?;
            if msg.get("id") != Some(&json!(line.0)) {
                continue;
            }
            if let Some(err) = msg.get("error") {
                return Err(CallError::Rpc(ferb_py::value::py_str(
                    err.get("message").unwrap_or(&Value::Null),
                )));
            }
            return Ok(msg.get("result").cloned().unwrap_or(Value::Null));
        }
    };
    let mut hello = Map::new();
    hello.insert("role".into(), json!("mcp_shim"));
    hello.insert("token".into(), json!(token.expose()));
    exchange(call("auth.hello", Value::Object(hello))).await?;
    exchange(call(method, params)).await
}

/// `inputSchema` as the `mcp` library writes it: `properties`, `required`, `type`, then every other key in order.
fn input_schema(schema: &Value) -> Value {
    let Value::Object(m) = schema else {
        return schema.clone();
    };
    let mut out = Map::new();
    for k in ["properties", "required", "type"] {
        if let Some(v) = m.get(k) {
            out.insert(k.into(), v.clone());
        }
    }
    for (k, v) in m {
        if !out.contains_key(k) {
            out.insert(k.clone(), v.clone());
        }
    }
    Value::Object(out)
}

/// The library's JSON: compact, text as written.
fn wire(value: &Value) -> String {
    serde_json::to_string(value).unwrap_or_default()
}

fn result_frame(id: &Value, result: Value) -> String {
    let mut m = Map::new();
    m.insert("jsonrpc".into(), json!("2.0"));
    m.insert("id".into(), id.clone());
    m.insert("result".into(), result);
    wire(&Value::Object(m))
}

fn error_frame(id: &Value, code: i64, message: &str, data: Option<&str>) -> String {
    let mut e = Map::new();
    e.insert("code".into(), json!(code));
    e.insert("message".into(), json!(message));
    if let Some(d) = data {
        e.insert("data".into(), json!(d));
    }
    let mut m = Map::new();
    m.insert("jsonrpc".into(), json!("2.0"));
    m.insert("id".into(), id.clone());
    m.insert("error".into(), Value::Object(e));
    wire(&Value::Object(m))
}

fn text_result(text: &str, is_error: bool) -> Value {
    json!({"content": [{"text": text, "type": "text"}], "isError": is_error})
}

struct Bridge {
    socket: PathBuf,
    token: Secret,
    /// mcp name -> capability name, from the last list.
    names: Mutex<HashMap<String, String>>,
    out: tokio::sync::Mutex<Box<dyn AsyncWrite + Send + Unpin>>,
    calls: Mutex<HashMap<String, JoinHandle<()>>>,
}

impl Bridge {
    async fn send(&self, line: String) {
        let mut out = self.out.lock().await;
        let _ = out.write_all(format!("{line}\n").as_bytes()).await;
        let _ = out.flush().await;
    }

    async fn list(&self, id: &Value) -> String {
        match daemon_call(&self.socket, &self.token, "tools.list", json!({})).await {
            Ok(r) => {
                let tools = r
                    .get("tools")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                let mut names = HashMap::new();
                let mut listed = Vec::new();
                for t in &tools {
                    let name = ferb_py::value::py_str(t.get("name").unwrap_or(&Value::Null));
                    names.insert(mcp_name(&name), name.clone());
                    listed.push(
                        json!({"description": t.get("description").cloned().unwrap_or(Value::Null),
                        "inputSchema": input_schema(t.get("input_schema").unwrap_or(&Value::Null)),
                        "name": mcp_name(&name)}),
                    );
                }
                *self.names.lock().unwrap_or_else(|e| e.into_inner()) = names;
                result_frame(id, json!({"tools": listed}))
            }
            Err(CallError::Rpc(m)) | Err(CallError::Broken(_, m)) => error_frame(id, 0, &m, None),
        }
    }

    /// cli/mcp.py `call_tool`.
    async fn call(&self, id: &Value, params: &Map<String, Value>) -> String {
        let asked = params
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let name = self
            .names
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(asked)
            .cloned()
            .unwrap_or_else(|| asked.replacen('_', ".", 1));
        let mut request = Map::new();
        request.insert("name".into(), json!(name));
        request.insert(
            "arguments".into(),
            match params.get("arguments") {
                Some(v @ Value::Object(m)) if !m.is_empty() => v.clone(),
                _ => json!({}),
            },
        );
        if let Some(Value::Object(meta)) = params.get("_meta") {
            if let (Some(Value::String(thread)), Some(Value::String(call))) =
                (meta.get("threadId"), meta.get("callId"))
            {
                if !thread.is_empty() && !call.is_empty() {
                    // Codex names its call: its adapter can tell the daemon when Codex drops it silently
                    request.insert("call_id".into(), json!(format!("{thread}/{call}")));
                }
            }
        }
        let out = daemon_call(
            &self.socket,
            &self.token,
            "tools.call",
            Value::Object(request),
        )
        .await;
        let result = match out {
            Err(CallError::Rpc(m)) => text_result(&format!("FERB refused: {m}"), true),
            Err(CallError::Broken(class, words)) => text_result(
                &format!(
                    "FERB's connection broke during this call ({class}: {words}); the call's outcome is unknown. Check what it would have changed before trying it again."
                ),
                true,
            ),
            Ok(r) => {
                let text = format!(
                    "{}\n{}",
                    ferb_py::json::dumps_utf8(r.get("content").unwrap_or(&Value::Null)),
                    ferb_py::value::py_str(r.get("footer").unwrap_or(&Value::Null))
                );
                let mut content = vec![json!({"text": text, "type": "text"})];
                if let Some(image) = r.get("image").filter(|i| ferb_py::value::truthy(i)) {
                    content.push(json!({"data": image.get("data"), "mimeType": image.get("mime"),
                        "type": "image"}));
                }
                let is_error = r.get("is_error").is_some_and(ferb_py::value::truthy);
                json!({"content": content, "isError": is_error})
            }
        };
        result_frame(id, result)
    }
}

fn valid_initialize(params: Option<&Value>) -> Option<String> {
    let p = params?.as_object()?;
    let version = p.get("protocolVersion")?.as_str()?;
    p.get("capabilities")?.as_object()?;
    let info = p.get("clientInfo")?.as_object()?;
    info.get("name")?.as_str()?;
    info.get("version")?.as_str()?;
    Some(if HANDSHAKE_VERSIONS.contains(&version) {
        version.to_string()
    } else {
        LATEST_HANDSHAKE.to_string()
    })
}

fn id_key(id: &Value) -> String {
    ferb_py::json::dumps(id)
}

/// Serves MCP on `input`/`output` until the input ends, forwarding to the daemon at `socket` with `token`.
pub async fn serve<R, W>(input: R, output: W, socket: PathBuf, token: Secret)
where
    R: AsyncRead + Unpin + Send + 'static,
    W: AsyncWrite + Unpin + Send + 'static,
{
    let bridge = Arc::new(Bridge {
        socket,
        token,
        names: Mutex::new(HashMap::new()),
        out: tokio::sync::Mutex::new(Box::new(output)),
        calls: Mutex::new(HashMap::new()),
    });
    let mut lines = BufReader::new(input);
    let mut initialized = false;
    loop {
        let mut raw = String::new();
        match lines.read_line(&mut raw).await {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        let Ok(Value::Object(msg)) = serde_json::from_str::<Value>(&raw) else {
            continue; // not a message: nothing comes back
        };
        let Some(Value::String(method)) = msg.get("method").cloned() else {
            continue; // a response from the client
        };
        let params = msg.get("params");
        if params.is_some_and(|p| !p.is_object() && !p.is_null()) {
            continue;
        }
        let Some(id) = msg.get("id").cloned() else {
            if method == "notifications/cancelled" {
                let request = params
                    .and_then(|p| p.get("requestId"))
                    .map(id_key)
                    .unwrap_or_default();
                if let Some(task) = bridge
                    .calls
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .remove(&request)
                {
                    // the daemon sees its caller leave (an owner question stays open for a retry)
                    task.abort();
                }
            }
            continue;
        };
        let line = match method.as_str() {
            "initialize" => match valid_initialize(params) {
                Some(version) => {
                    initialized = true;
                    result_frame(
                        &id,
                        json!({"capabilities": {"experimental": {}, "tools": {"listChanged": false}},
                            "protocolVersion": version, "serverInfo": {"name": "ferb", "version": "1"}}),
                    )
                }
                None => error_frame(&id, -32602, "Invalid request parameters", Some("")),
            },
            "ping" => result_frame(&id, json!({})),
            _ if !initialized => error_frame(&id, -32602, "Invalid request parameters", Some("")),
            "tools/list" => {
                let b = bridge.clone();
                tokio::spawn(async move {
                    let line = b.list(&id).await;
                    b.send(line).await;
                });
                continue;
            }
            "tools/call" => {
                let p = params
                    .and_then(Value::as_object)
                    .cloned()
                    .unwrap_or_default();
                let fits = p.get("name").is_some_and(Value::is_string)
                    && p.get("arguments")
                        .is_none_or(|a| a.is_object() || a.is_null());
                if !fits {
                    error_frame(&id, -32602, "Invalid request parameters", Some(""))
                } else {
                    let b = bridge.clone();
                    let key = id_key(&id);
                    let k2 = key.clone();
                    let task = tokio::spawn(async move {
                        let line = b.call(&id, &p).await;
                        b.calls
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .remove(&k2);
                        b.send(line).await;
                    });
                    bridge
                        .calls
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .insert(key, task);
                    continue;
                }
            }
            other => error_frame(&id, -32601, "Method not found", Some(other)),
        };
        bridge.send(line).await;
    }
    // what is still running finishes before the bridge ends, as the library's task group waits
    let pending: Vec<JoinHandle<()>> = bridge
        .calls
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .drain()
        .map(|(_, t)| t)
        .collect();
    for t in pending {
        let _ = t.await;
    }
}

/// `ferb mcp` on the process's own stdio: the token from the environment, the socket in FERB's home. Returns the
/// exit status (1 without a token, after printing NO_TOKEN).
pub async fn run_stdio(ferb_home: &Path) -> i32 {
    let env: HashMap<String, String> = std::env::vars().collect();
    let Some(token) = token_from(&env) else {
        eprintln!("{NO_TOKEN}");
        return 1;
    };
    serve(
        tokio::io::stdin(),
        tokio::io::stdout(),
        ferb_home.join("run").join("ferb.sock"),
        token,
    )
    .await;
    0
}
