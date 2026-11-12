//! The unchanged Python client (`ferb/cli/client.py`, what the CLI and the MCP shim use) against the Rust
//! server (ENG-1). It needs the checkout's Python environment, so it runs only when asked:
//!
//!     cd rust && cargo test -p ferb-rpc --test python_client -- --ignored
//!
//! The Python side gets FERB_HOME in a temporary folder and is given the test's socket; it never looks for
//! ~/.ferb.

mod common;

use std::path::PathBuf;
use std::process::Command;

use common::new_server;
use ferb_rpc::auth::{GrantSpec, Identity};

const SCRIPT: &str = r#"
import json, sys
from pathlib import Path
from ferb.cli.client import Client, RpcError

sock, token = Path(sys.argv[1]), sys.argv[2]
out = {}
c = Client(sock)
out["ping"] = c.call("ping")
out["echo"] = c.call("trace.get", {"task_id": "x", "text": "caf\xe9 \U0001f600", "n": [1, 2.5, None]})
for name, method in (("refused", "doctor.check"), ("fault", "memory.list"), ("app_only", "tasks.preview"),
                     ("unknown", "nope"), ("unregistered", "ps.list")):
    try:
        c.call(method)
    except RpcError as e:
        out[name] = [e.code, str(e)]
out["stream"] = list(c.stream("logs.subscribe", {"n": 3}))
try:
    list(c.stream("notifications.subscribe"))
except RpcError as e:
    out["stream_error"] = [e.code, str(e)]
c.set_timeout(10)
out["after"] = c.call("ping")["role"]
c.close()
shim = Client(sock, role="mcp_shim", token=token, timeout=None)
out["shim"] = shim.call("tools.call", {"call_id": "c1"})
try:
    shim.call("trace.get")
except RpcError as e:
    out["shim_other"] = [e.code, str(e)]
shim.close()
for role, tok in (("mcp_shim", "made-up"), ("native_host", None)):
    try:
        Client(sock, role=role, token=tok)
    except RpcError as e:
        out["hello_" + role] = [e.code, str(e)]
print(json.dumps(out))
"#;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "needs the checkout's Python environment (.venv); see the file's first lines"]
async fn the_python_client_talks_to_the_rust_server() {
    let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../..")
        .canonicalize()
        .unwrap();
    let python = repo.join(".venv/bin/python");
    assert!(
        python.exists(),
        "no {} (run `uv sync --group dev`)",
        python.display()
    );
    let dir = tempfile::tempdir().unwrap();
    let (server, _) = new_server(dir.path());
    let token = server
        .tokens()
        .issue("task-1", "w-1", GrantSpec::new(Identity::Owner, 60.0));
    server.start().await.unwrap();

    let (socket, home) = (server.path().to_owned(), dir.path().join("home"));
    let output = tokio::task::spawn_blocking(move || {
        Command::new(python)
            .args(["-P", "-c", SCRIPT])
            .arg(socket)
            .arg(token)
            .current_dir(&repo)
            .env("PYTHONPATH", &repo)
            .env("FERB_HOME", home)
            .output()
            .unwrap()
    })
    .await
    .unwrap();
    server.close().await;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let out: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let expected = serde_json::json!({
        "ping": {"pid": 4242, "role": "cli", "now": "2026-11-13T10:00:00+00:00", "stopped": false},
        "echo": {"echo": {"task_id": "x", "text": "caf\u{e9} \u{1f600}", "n": [1, 2.5, null]}},
        "refused": [-32004, "the check said no"],
        "fault": [-32603, "RuntimeError"],
        "app_only": [-32005, "tasks.preview is for FERB.app only"],
        "unknown": [-32001, "nope not allowed for cli"],
        "unregistered": [-32601, "no method ps.list"],
        "stream": [{"i": 0}, {"i": 1}, {"i": 2}],
        "stream_error": [-32004, "Refused: the stream said no"],
        "after": "cli",
        "shim": {"task": "task-1", "worker": "w-1", "left": false},
        "shim_other": [-32001, "trace.get not allowed for mcp_shim"],
        "hello_mcp_shim": [-32001, "unknown or revoked run token"],
        "hello_native_host": [-32003, "FERB's browser bridge isn't running"],
    });
    assert_eq!(out, expected);
}
