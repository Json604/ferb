//! The shell tools' operations (ferb/act/shell/tools.py, guard.py): one function per tool, typed arguments matching
//! rust/fixtures/catalog/tools.json, and the result composition as pure functions (checked against
//! rust/fixtures/shell/results.json). Registration with the gateway (identities, scopes, risk, schemas) belongs to the
//! gateway port; nothing here chooses a command, a retry or a method.

use std::path::PathBuf;

use ferb_core::types::{ToolContext, ToolResult};
use serde_json::{json, Map, Value};

use crate::manager::{ProcHandle, ProcInfo, ShellError, ShellManager, SEND_WAIT_S};
use crate::protection::Protection;
use crate::proxy::DEST_REFUSED_NO;
use crate::sandbox::{
    APP_REFUSED, NESTED_SANDBOX, NETWORK_FACT, NO_APPS, SCOPE_FACT, SCOPE_REFUSED,
};

pub const TAIL: u64 = 8000;
/// Command output is unlabeled outside content (S3); hosts it reached label it per host.
pub const OUTPUT_ORIGIN: &str = "external:unlabeled";
/// What macOS prints for a write the sandbox refused (EPERM's own text).
const EPERM: &str = "Operation not permitted";

/// The tools' descriptions, as the models read them.
pub fn description(tool: &str) -> Option<String> {
    let tail = format!(
        "Commands run inside FERB's sandbox (shell.rules lists what it keeps out). {SCOPE_FACT} {NO_APPS} {NETWORK_FACT} {NESTED_SANDBOX}"
    );
    Some(match tool {
        "shell.start" => format!(
            "Start a tracked shell command immediately and return its id. Use for long commands, then wait.for (one \
             wake when it exits or prints what you name) or shell.read for actual progress and exit status. Run the \
             command directly, without nohup or trailing &: the id tracks this command, not detached children. \
             background=true keeps it alive after task completion (servers); it is not needed to work asynchronously \
             during a task. Starting a process does not prove it is ready or successful. {tail}"
        ),
        "shell.run" => format!(
            "Run a shell command (/bin/sh), waiting up to timeout_s. A live process returns its id; use wait.for to \
             await its exit or shell.read for output. For immediate asynchronous start use shell.start, without nohup \
             or trailing &. background=true survives task completion (servers). {tail}"
        ),
        "shell.wait" => "Wait up to 120 s for a tracked command to exit, returning early when it finishes. A timeout \
                         leaves it running. Returns exit_code and output from since, so failures are visible without \
                         sleeping blindly. Does not launch a retry. For a longer command, call wait.for once (it waits \
                         in code until the command exits or prints what you name) instead of calling this again and \
                         again."
            .to_string(),
        "shell.read" => "Read a command's output from a byte offset, a page at a time: start at since=0 and continue \
                         from the returned offset while more is true."
            .to_string(),
        "shell.send" => "Send text (e.g. an answer plus a newline) to a running command's input. A command that reads \
                         lines takes at most 1,023 bytes per line; a command started with an owner-approved escape \
                         takes no input from FERB."
            .to_string(),
        "shell.kill" => "Stop one of this task's commands (and its children).".to_string(),
        "shell.list" => "This task's commands: id, command, running, exit code.".to_string(),
        "shell.rules" => "What FERB's sandbox keeps FERB's shell out of: each rule's id, what it covers and whether the \
                          owner can lift it for one command; the network, apps and background jobs."
            .to_string(),
        _ => return None,
    })
}

// ---------- arguments (the catalog's input schemas; the gateway adds `why` and `effect`) ----------

#[derive(Debug, Clone, PartialEq)]
pub struct StartArgs {
    pub command: String,
    /// Working folder (default: FERB's workspace).
    pub cwd: Option<String>,
    pub background: bool,
    /// Declared source of outside code; recorded by the gateway, never detected here.
    pub external_source: Option<String>,
    /// One rule id from shell.rules to lift for this exact command once (the owner was asked).
    pub escape: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RunArgs {
    pub start: StartArgs,
    /// 1..=120, default 30.
    pub timeout_s: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct WaitArgs {
    pub id: String,
    /// 0..=120, default 30.
    pub timeout_s: f64,
    pub since: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ReadArgs {
    pub id: String,
    pub since: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SendArgs {
    pub id: String,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct KillArgs {
    pub id: String,
}

// ---------- the working folder (guard.py) ----------

/// The fs tools' path resolution (ferb/act/fs/files.py `resolve`, ported in the files part): the path as checked
/// for this call, or why it is refused (`PathRefused`'s words).
pub trait ResolvePath: Send + Sync {
    fn resolve(&self, path: &str, ctx: &ToolContext, must_exist: bool) -> Result<PathBuf, String>;
}

/// guard.py: the command's working folder: the given one or the workspace, resolved by the fs tools' rules.
pub fn working_directory(
    cwd: Option<&str>,
    ctx: &ToolContext,
    resolver: &dyn ResolvePath,
) -> Result<PathBuf, ShellError> {
    let wanted = cwd.filter(|c| !c.is_empty()).unwrap_or(&ctx.workspace);
    match resolver.resolve(wanted, ctx, true) {
        Ok(folder) if folder.is_dir() => Ok(folder),
        Ok(folder) => Err(ShellError::Refused(format!(
            "working folder is not a directory: {}",
            folder.to_string_lossy()
        ))),
        Err(e) => Err(ShellError::Refused(format!("working folder refused: {e}"))),
    }
}

// ---------- result composition ----------

fn err(msg: &str) -> ToolResult {
    let mut content = Map::new();
    content.insert("error".into(), json!(msg));
    ToolResult {
        content,
        is_error: true,
        ..Default::default()
    }
}

/// What to do after a refusal the owner decided (a No to a host in this task).
fn refused_next(network: &Map<String, Value>) -> Option<String> {
    network.get("refused").and_then(Value::as_array)?.iter().find_map(|r| {
        (r.get("why").and_then(Value::as_str) == Some(DEST_REFUSED_NO)).then(|| {
            format!(
                "The owner said no to reaching {} in this task; don't try another way. Report that it wasn't done.",
                r.get("host").and_then(Value::as_str).unwrap_or("")
            )
        })
    })
}

/// One web source per host the command reached (an IPv6 literal bracketed so the URL parses).
fn sources(network: &Map<String, Value>) -> Vec<(String, String)> {
    let hosts = network
        .get("destinations")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    crate::pyos::dedupe(hosts.iter().map(|d| {
        let h = d.get("host").and_then(Value::as_str).unwrap_or("");
        let shown = if h.contains(':') && !h.starts_with('[') {
            format!("[{h}]")
        } else {
            h.to_string()
        };
        ("web".to_string(), format!("https://{shown}"))
    }))
}

fn read_sources(network: &Map<String, Value>, info: &ProcInfo) -> Vec<(String, String)> {
    let web = sources(network);
    match &info.read_path {
        Some(path) => {
            let mut out = if web.is_empty() {
                vec![("origin".to_string(), OUTPUT_ORIGIN.to_string())]
            } else {
                web
            };
            out.push(("file".to_string(), path.clone()));
            out
        }
        None => web,
    }
}

/// HAND-203: a failed command whose output shows macOS refusing an operation gets the folder scope's reason.
fn scope_note(out: &mut Map<String, Value>, failed: bool) {
    let text = out.get("output").and_then(Value::as_str).unwrap_or("");
    if failed && text.contains(EPERM) {
        out.insert(
            "sandbox_note".into(),
            json!(format!("{SCOPE_REFUSED} Also, {APP_REFUSED}")),
        );
    }
}

fn stopped_note(out: &mut Map<String, Value>, stopped: Option<&str>) {
    if let Some(s) = stopped.filter(|s| !s.is_empty()) {
        out.insert("stopped_by_ferb".into(), json!(s));
    }
}

fn failed(code: Option<i32>) -> bool {
    !matches!(code, None | Some(0))
}

/// shell.start's result: (result, whether its end was shown).
pub fn start_result(id: &str, running: bool) -> (ToolResult, bool) {
    let mut content = Map::new();
    content.insert("id".into(), json!(id));
    content.insert("running".into(), json!(running));
    content.insert("exit_code".into(), Value::Null);
    (
        ToolResult {
            content,
            ..Default::default()
        },
        !running,
    )
}

/// shell.wait and shell.read (tools.py `_output`): (result, whether its end was shown).
pub fn output_result(
    id: &str,
    text: &str,
    offset: u64,
    total: u64,
    info: &ProcInfo,
    network: Map<String, Value>,
) -> (ToolResult, bool) {
    let mut out = Map::new();
    out.insert("id".into(), json!(id));
    out.insert("output".into(), json!(text));
    out.insert("offset".into(), json!(offset));
    out.insert("total".into(), json!(total));
    out.insert("more".into(), json!(offset < total));
    out.insert("running".into(), json!(info.running));
    out.insert("exit_code".into(), json!(info.exit_code));
    out.insert("capture_error".into(), json!(info.capture_error));
    out.insert("network".into(), Value::Object(network.clone()));
    out.insert("sandbox_ms".into(), json!(info.sandbox_ms));
    out.insert(
        "retrieval".into(),
        if offset < total {
            json!("shell.read with since=offset until more=false")
        } else {
            Value::Null
        },
    );
    if let Some(next) = refused_next(&network) {
        out.insert("next".into(), json!(next));
    }
    scope_note(&mut out, failed(info.exit_code));
    stopped_note(&mut out, info.stopped.as_deref());
    let is_error = failed(info.exit_code)
        || info.capture_error.as_deref().is_some_and(|c| !c.is_empty())
        || info.stopped.as_deref().is_some_and(|s| !s.is_empty());
    (
        ToolResult {
            content: out,
            is_error,
            read_sources: read_sources(&network, info),
            ..Default::default()
        },
        !info.running,
    )
}

/// shell.run (tools.py ShellRun._run after the wait): `code` is what the wait returned.
pub fn run_result(
    id: &str,
    code: Option<i32>,
    text: &str,
    offset: u64,
    total: u64,
    info: &ProcInfo,
    network: Map<String, Value>,
) -> (ToolResult, bool) {
    let mut out = Map::new();
    out.insert("id".into(), json!(id));
    out.insert("running".into(), json!(code.is_none()));
    out.insert("exit_code".into(), json!(code));
    out.insert("output".into(), json!(text));
    out.insert("offset".into(), json!(offset));
    out.insert("total".into(), json!(total));
    if total > TAIL {
        out.insert("output_truncated".into(), json!(true));
        out.insert(
            "next".into(),
            json!("shell.read with since=0 reads from the beginning, a page at a time"),
        );
    }
    out.insert("capture_error".into(), json!(info.capture_error));
    out.insert("sandbox_ms".into(), json!(info.sandbox_ms));
    out.insert("network".into(), Value::Object(network.clone()));
    if let Some(next) = refused_next(&network) {
        out.insert("next".into(), json!(next));
    }
    scope_note(&mut out, failed(code));
    stopped_note(&mut out, info.stopped.as_deref());
    let is_error = failed(code)
        || info.capture_error.as_deref().is_some_and(|c| !c.is_empty())
        || info.stopped.as_deref().is_some_and(|s| !s.is_empty());
    (
        ToolResult {
            content: out,
            is_error,
            read_sources: read_sources(&network, info),
            ..Default::default()
        },
        code.is_some(),
    )
}

pub fn send_result(sent: usize, total: usize) -> ToolResult {
    let mut out = Map::new();
    if sent < total {
        out.insert("sent".into(), json!(false));
        out.insert("bytes_sent".into(), json!(sent));
        out.insert("bytes".into(), json!(total));
        out.insert(
            "error".into(),
            json!(format!(
                "only {sent} of {total} bytes went in: the command ended or isn't reading its input; check its output \
                 (shell.read) before sending the rest"
            )),
        );
        return ToolResult {
            content: out,
            is_error: true,
            ..Default::default()
        };
    }
    out.insert("sent".into(), json!(true));
    out.insert("bytes".into(), json!(total));
    ToolResult {
        content: out,
        ..Default::default()
    }
}

/// shell.list: this task's commands (the rest are never shown).
pub fn list_result(all: &[ProcInfo], task: &str) -> (ToolResult, Vec<String>) {
    let mine: Vec<&ProcInfo> = all.iter().filter(|p| p.owner_task == task).collect();
    let told = mine
        .iter()
        .filter(|p| !p.running)
        .map(|p| p.id.clone())
        .collect();
    let processes: Vec<Value> = mine
        .iter()
        .map(|p| json!({"id": p.id, "command": p.command, "running": p.running, "exit_code": p.exit_code, "background": p.long_lived}))
        .collect();
    let mut out = Map::new();
    out.insert("processes".into(), Value::Array(processes));
    (
        ToolResult {
            content: out,
            ..Default::default()
        },
        told,
    )
}

/// shell.rules: the lists' rules and what the network, apps and nested sandboxes mean (tools.py:318).
pub fn rules_result(protection: &dyn Protection, proxy_port: Option<u16>) -> ToolResult {
    let local = "this Mac's own servers only if started inside FERB's shell: directly when one was already listening \
                 as the command started, else through the proxy, which checks each connection's listener (NO_PROXY is \
                 empty, so HTTP clients take the proxy route); other programs' servers are out of reach; \
                 protection.local_ports allows approved TCP services: directly when the port's listener runs its \
                 approved executable as the command starts (listed under the command's network local_direct, with the \
                 connections the kernel reported: a lower bound, at least one per program that connected), and through \
                 the proxy, which checks the executable for each connection and records its bytes.";
    let network = match proxy_port {
        None => format!("no network beyond this Mac; FERB's proxy is not running; {local}"),
        Some(port) => format!(
            "any remote host, without asking, only through FERB's proxy on 127.0.0.1:{port}, which records each host, \
             port and byte count (clients that ignore HTTPS_PROXY/HTTP_PROXY get no network; no ssh); {local}"
        ),
    };
    let mut out = Map::new();
    out.insert("rules".into(), Value::Array(protection.rules()));
    out.insert("network".into(), json!(network));
    out.insert("apps".into(), json!(NO_APPS));
    out.insert("own_sandbox".into(), json!(NESTED_SANDBOX));
    out.insert(
        "local_ports".into(),
        Value::Array(
            protection
                .local_ports()
                .iter()
                .map(|p| json!({"port": p.port, "executable": p.executable.to_string_lossy()}))
                .collect(),
        ),
    );
    ToolResult {
        content: out,
        ..Default::default()
    }
}

// ---------- the tools ----------

/// A Refused becomes the tool's error result; any other failure is the caller's (Python raised it).
fn refusal(e: ShellError) -> Result<ToolResult, ShellError> {
    match e {
        ShellError::Refused(msg) => Ok(err(&msg)),
        other => Err(other),
    }
}

async fn start(
    shells: &ShellManager,
    args: &StartArgs,
    ctx: &ToolContext,
    resolver: &dyn ResolvePath,
) -> Result<ProcHandle, ShellError> {
    let cwd = working_directory(args.cwd.as_deref(), ctx, resolver)?;
    let lift = args.escape.as_deref().filter(|e| !e.is_empty());
    if let Some(l) = lift {
        if ctx.escape.as_ref().is_none_or(|e| e.rule != l) {
            return Err(ShellError::Refused(
                "this command has no owner-approved escape grant".into(),
            ));
        }
    }
    shells
        .start(
            &args.command,
            &cwd,
            &ctx.task_id,
            args.background,
            lift,
            ctx.escape.as_ref(),
            &ctx.folders,
        )
        .await
}

pub async fn shell_start(
    shells: &ShellManager,
    args: &StartArgs,
    ctx: &ToolContext,
    resolver: &dyn ResolvePath,
) -> Result<ToolResult, ShellError> {
    let h = match start(shells, args, ctx, resolver).await {
        Ok(h) => h,
        Err(e) => return refusal(e),
    };
    let running = match shells.info(&h) {
        Ok(i) => i.running,
        Err(e) => return refusal(e),
    };
    let (result, ended) = start_result(&h.id, running);
    if ended {
        shells.exit_told(&h);
    }
    Ok(result)
}

pub async fn shell_run(
    shells: &ShellManager,
    args: &RunArgs,
    ctx: &ToolContext,
    resolver: &dyn ResolvePath,
) -> Result<ToolResult, ShellError> {
    let go = async {
        let h = start(shells, &args.start, ctx, resolver).await?;
        let code = shells.wait(&h, args.timeout_s).await?;
        if code.is_some() {
            shells.exit_told(&h);
        }
        let (text, offset, total) = shells.tail(&h, TAIL).await?;
        let info = shells.info(&h)?;
        let network = shells.network(&h)?;
        Ok::<_, ShellError>(run_result(&h.id, code, &text, offset, total, &info, network).0)
    };
    match go.await {
        Ok(r) => Ok(r),
        Err(e) => refusal(e),
    }
}

async fn output(
    shells: &ShellManager,
    h: &ProcHandle,
    since: i64,
) -> Result<ToolResult, ShellError> {
    let (text, offset, total) = shells.read(h, since, Some(TAIL)).await?;
    let info = shells.info(h)?;
    if !info.running {
        shells.exit_told(h); // its end is in this result (ORCH-639)
    }
    let network = shells.network(h)?;
    Ok(output_result(&h.id, &text, offset, total, &info, network).0)
}

pub async fn shell_wait(
    shells: &ShellManager,
    args: &WaitArgs,
    ctx: &ToolContext,
) -> Result<ToolResult, ShellError> {
    let h = shells.handle(&args.id, &ctx.task_id);
    let go = async {
        shells.wait(&h, args.timeout_s).await?;
        output(shells, &h, args.since).await
    };
    match go.await {
        Ok(r) => Ok(r),
        Err(e) => refusal(e),
    }
}

pub async fn shell_read(
    shells: &ShellManager,
    args: &ReadArgs,
    ctx: &ToolContext,
) -> Result<ToolResult, ShellError> {
    let h = shells.handle(&args.id, &ctx.task_id);
    match output(shells, &h, args.since).await {
        Ok(r) => Ok(r),
        Err(e) => refusal(e),
    }
}

pub async fn shell_send(
    shells: &ShellManager,
    args: &SendArgs,
    ctx: &ToolContext,
) -> Result<ToolResult, ShellError> {
    let h = shells.handle(&args.id, &ctx.task_id);
    match shells.send(&h, &args.text, SEND_WAIT_S).await {
        Ok((sent, total)) => Ok(send_result(sent, total)),
        Err(e) => refusal(e),
    }
}

pub async fn shell_kill(
    shells: &ShellManager,
    args: &KillArgs,
    ctx: &ToolContext,
) -> Result<ToolResult, ShellError> {
    let h = shells.handle(&args.id, &ctx.task_id);
    if let Err(e) = shells.kill(&h).await {
        return refusal(e);
    }
    shells.exit_told(&h); // the worker ended it: its end wakes no wait (ORCH-639)
    let mut out = Map::new();
    out.insert("killed".into(), json!(true));
    Ok(ToolResult {
        content: out,
        ..Default::default()
    })
}

pub fn shell_list(shells: &ShellManager, ctx: &ToolContext) -> ToolResult {
    let (result, told) = list_result(&shells.list(), &ctx.task_id);
    for id in told {
        shells.exit_told(&shells.handle(&id, &ctx.task_id));
    }
    result
}

pub fn shell_rules(protection: &dyn Protection, proxy_port: Option<u16>) -> ToolResult {
    rules_result(protection, proxy_port)
}
