//! The shell's egress decisions as the daemon relays them, local connection reports, and the `ferb ps` RPCs. Port of
//! `ferb/daemon/areas/egress.py`. Must not allow a destination itself: the gateway decides.
//!
//! RPCs: `ps.list`, `ps.kill`, `ps.attach` (a stream), `ps.send`, over ferb-shell's real `ShellManager`. Python
//! relays journal rows from proxy threads onto its loop (`_journal_from_any_thread`); the Rust journal takes rows from
//! any thread, so the helpers here write directly.

use std::sync::Arc;
use std::time::Duration;

use ferb_rpc::{BoxFuture, EventStream, HandlerError, HandlerResult, Server};
use ferb_shell::manager::{ProcInfo, ShellError, ShellManager};
use serde_json::{json, Map, Value};
use tokio::sync::mpsc;

use super::core::{object, ok, refusal, Host, P};

pub struct Deps {
    /// `self.shells`; None before the shell is built (Python's `assert self.shells`).
    pub shells: Option<Arc<ShellManager>>,
    /// How long `ps.attach` waits between reads of a running process (Python: 0.1 s).
    pub poll: Duration,
}

/// A process as `ferb ps` shows it: ProcInfo's fields in their dataclass order, the command redacted (rule 14).
pub fn proc_json(h: &Host, p: &ProcInfo) -> Value {
    let direct: Vec<Value> = p
        .direct
        .iter()
        .map(|d| json!({"port": d.port, "executable": d.executable.to_string_lossy()}))
        .collect();
    json!({"id": p.id, "owner_task": p.owner_task, "command": h.redact_text(&p.command), "cwd": p.cwd,
           "started": p.started, "running": p.running, "exit_code": p.exit_code, "long_lived": p.long_lived,
           "network": p.network, "capture_error": p.capture_error, "sandboxed": p.sandboxed, "lifted": p.lifted,
           "read_path": p.read_path, "sandbox_ms": p.sandbox_ms, "direct": direct, "stopped": p.stopped})
}

fn not_built() -> HandlerError {
    HandlerError::fault("AssertionError", "")
}

fn shell_error(e: ShellError) -> HandlerError {
    match e {
        ShellError::Refused(text) => HandlerError::refusal("Refused", text),
        other => HandlerError::fault("OSError", other.to_string()),
    }
}

pub fn register(server: &Server, host: &Host, deps: &Deps) {
    let (h, shells) = (host.clone(), deps.shells.clone());
    server.register_unary("ps.list", move |_s, _p: Map<String, Value>| {
        let (h, shells) = (h.clone(), shells.clone());
        async move {
            let shells = shells.ok_or_else(not_built)?;
            let processes: Vec<Value> = shells.list().iter().map(|p| proc_json(&h, p)).collect();
            ok(json!({"processes": processes}))
        }
    });
    let shells = deps.shells.clone();
    server.register_unary("ps.kill", move |_s, p: Map<String, Value>| {
        let shells = shells.clone();
        async move {
            let shells = shells.ok_or_else(not_built)?;
            let id = P(&p).text("id", "");
            ok(json!({"killed": shells.kill_id(&id).await}))
        }
    });
    let shells = deps.shells.clone();
    server.register_unary("ps.send", move |_s, p: Map<String, Value>| {
        let shells = shells.clone();
        async move {
            let shells = shells.ok_or_else(not_built)?;
            let p = P(&p);
            let text = p.text("text", "");
            let took = shells
                .owner_send(&p.text("id", ""), &p.text("lease", ""), &text)
                .map_err(shell_error)?;
            ok(json!({"sent": took == text.len(), "bytes": took}))
        }
    });
    let (shells, poll) = (deps.shells.clone(), deps.poll);
    server.register_stream(
        "ps.attach",
        move |_s, p: Map<String, Value>| -> Box<dyn EventStream> {
            let (tx, rx) = mpsc::channel(16);
            let shells = shells.clone();
            tokio::spawn(attach(shells, p, poll, tx));
            Box::new(rx)
        },
    );
}

/// `_ps_attach`: owner-only stream. The connection's lifetime owns input; leaving releases it even after a CLI crash.
async fn attach(
    shells: Option<Arc<ShellManager>>,
    params: Map<String, Value>,
    poll: Duration,
    tx: mpsc::Sender<HandlerResult>,
) {
    let Some(shells) = shells else {
        let _ = tx.send(Err(not_built())).await;
        return;
    };
    let p = P(&params);
    let proc_id = p.text("id", "");
    let Some(task) = shells.owner_of(&proc_id) else {
        let _ = tx
            .send(Err(refusal("That process is not available.")))
            .await;
        return;
    };
    let handle = shells.handle(&proc_id, &task);
    let lease = if p.truthy("take") {
        match shells.take_input(&proc_id) {
            Ok(l) => l,
            Err(e) => {
                let _ = tx.send(Err(shell_error(e))).await;
                return;
            }
        }
    } else {
        String::new()
    };
    // `int(params.get('since', 0))`: ENG-6, a value that isn't a whole number is refused (Python's ValueError or
    // TypeError ended the stream with that exception's type)
    let since = match params.get("since") {
        None => 0,
        Some(Value::Number(n)) if n.as_i64().is_some() => n.as_i64().unwrap_or(0),
        Some(Value::Number(n)) if n.as_f64().is_some() => n.as_f64().unwrap_or(0.0).trunc() as i64,
        Some(Value::Bool(b)) => i64::from(*b),
        Some(Value::String(s)) if ferb_py::value::strip(s).parse::<i64>().is_ok() => {
            ferb_py::value::strip(s).parse::<i64>().unwrap_or(0)
        }
        Some(_) => {
            shells.release_input(&proc_id, &lease);
            let _ = tx.send(Err(refusal("since must be a byte offset"))).await;
            return;
        }
    };
    let mut offset = since.max(0);
    let release = Release {
        shells: shells.clone(),
        proc_id: proc_id.clone(),
        lease: lease.clone(),
    };
    if tx
        .send(Ok(object(json!({"lease": lease, "id": proc_id}))))
        .await
        .is_err()
    {
        return;
    }
    loop {
        let read = shells.read(&handle, offset, None).await;
        let info = read.and_then(|r| shells.info(&handle).map(|i| (r, i)));
        let ((text, next, _), info) = match info {
            Ok(x) => x,
            Err(ShellError::Refused(_)) => {
                // it ended after its task and FERB forgot it: nothing more to show
                let _ = tx
                    .send(Ok(object(
                        json!({"text": "", "offset": offset, "running": false,
                                           "exit_code": null, "forgotten": true}),
                    )))
                    .await;
                break;
            }
            Err(e) => {
                let _ = tx.send(Err(shell_error(e))).await;
                break;
            }
        };
        offset = next as i64;
        let event = json!({"text": text, "offset": offset, "running": info.running, "exit_code": info.exit_code});
        if tx.send(Ok(object(event))).await.is_err() || !info.running {
            break;
        }
        tokio::select! {
            _ = tokio::time::sleep(poll) => {}
            _ = tx.closed() => break,
        }
    }
    drop(release);
}

/// `finally: self.shells.release_input(proc_id, lease)`.
struct Release {
    shells: Arc<ShellManager>,
    proc_id: String,
    lease: String,
}

impl Drop for Release {
    fn drop(&mut self) {
        self.shells.release_input(&self.proc_id, &self.lease);
    }
}

/// `_local_refused`: whether this task's tree has the owner's No to one of this Mac's addresses (the sink rule's
/// key `[SINK_DESTINATION, host]`, SQ6): its commands then reach pinned local ports only through the proxy (L24). A
/// task FERB can't place counts as refused. `refused_keys` are the confirmations' refused keys of the tree.
pub fn local_refused(root: Option<&str>, refused_keys: &[String]) -> bool {
    if root.is_none() {
        return true;
    }
    for key in refused_keys {
        let Ok(Value::Array(pair)) = serde_json::from_str::<Value>(key) else {
            continue;
        };
        let [rule, Value::String(host)] = pair.as_slice() else {
            continue;
        };
        if rule == ferb_policy::confirm::SINK_DESTINATION
            && (ferb_shell::proxy::local_host(host)
                || host.trim_end_matches('.').ends_with(".localhost"))
        {
            return true;
        }
    }
    false
}

/// `_local_reports_stopped`: the kernel's reports of direct local connections stopped (told once).
pub fn local_reports_stopped(h: &Host, why: &str) -> Result<(), HandlerError> {
    h.journal("shell.local_reports_stopped", json!({"why": why}), None)
}

/// `_shell_network`: a finished shell process's final network record.
pub fn shell_network(
    h: &Host,
    info: &ProcInfo,
    facts: &Map<String, Value>,
) -> Result<(), HandlerError> {
    let mut row = Map::new();
    row.insert("task_id".into(), json!(info.owner_task));
    row.insert("process".into(), json!(info.id));
    for (k, v) in facts {
        row.insert(k.clone(), v.clone());
    }
    h.journal("shell.network", Value::Object(row), None)
}

/// `_shell_destination`: a shell process's first connection to a host and port, journaled as it happens.
pub fn shell_destination(
    h: &Host,
    shells: Option<&ShellManager>,
    proc_id: &str,
    host: &str,
    port: u16,
) -> Result<(), HandlerError> {
    let task = shells.and_then(|s| s.owner_of(proc_id));
    h.journal(
        "shell.destination",
        json!({"task_id": task, "process": proc_id, "host": host, "port": port}),
        None,
    )
}

/// What `_shell_received` asks of the browser's downloads (HAND-210): the bytes count against the task tree's
/// download budget; the answer is why the command must stop, or None.
pub trait DownloadBudget: Send + Sync {
    fn shell_received(&self, task: &str, n: i64);
    fn shell_stop(&self, task: &str) -> Option<String>;
}

/// `_shell_received`.
pub fn shell_received(
    h: &Host,
    downloads: Option<&dyn DownloadBudget>,
    task: &str,
    n: i64,
) -> Result<Option<String>, HandlerError> {
    let Some(d) = downloads else {
        return Ok(None);
    };
    d.shell_received(task, n);
    let why = d.shell_stop(task);
    if let Some(w) = &why {
        h.journal(
            "shell.download_stopped",
            json!({"task_id": task, "why": w}),
            None,
        )?;
    }
    Ok(why)
}

/// `_allow_destination` / `_allow_local_destination`: on a proxy thread, the gateway's answer from the daemon's
/// runtime within 10 s; anything that stops it answering refuses.
pub fn allow_destination(
    rt: &tokio::runtime::Handle,
    ask: BoxFuture<'static, (bool, String)>,
    local: bool,
) -> (bool, String) {
    let refused = if local {
        "FERB could not check this local destination"
    } else {
        "FERB could not check this destination"
    };
    let (tx, rx) = std::sync::mpsc::channel();
    rt.spawn(async move {
        let _ = tx.send(ask.await);
    });
    rx.recv_timeout(Duration::from_secs(10))
        .unwrap_or((false, refused.to_owned()))
}
