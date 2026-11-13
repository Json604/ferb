//! The unchanged Python client (`ferb/cli/client.py`, what the CLI and the MCP shim use) against the real `ferbd
//! daemon` program (ENG-1). It needs the checkout's Python environment (`.venv`); without it the test says so and
//! passes nothing off as checked. The Python side gets FERB_HOME and HOME in the test's temporary root.

mod support;

use std::process::Command;

use serde_json::{json, Value};
use support::*;

const SCRIPT: &str = r#"
import json, sys
from pathlib import Path
from ferb.cli.client import Client, RpcError

sock, token = Path(sys.argv[1]), sys.argv[2]
out = {}
c = Client(sock)
out["ping"] = c.call("ping")["role"]
out["memory"] = c.call("memory.list")
out["reminders"] = c.call("reminders.list")
for name, method in (("supervisor", "chat.status"), ("not_ported", "doctor.check"), ("unknown", "nope")):
    try:
        c.call(method)
    except RpcError as e:
        out[name] = [e.code, str(e)]
c.close()
shim = Client(sock, role="mcp_shim", token=token, timeout=None)
out["tools"] = [t["name"] for t in shim.call("tools.list")["tools"]]
out["call"] = shim.call("tools.call", {"name": "fs.list", "arguments": {"path": "."}})
try:
    shim.call("memory.list")
except RpcError as e:
    out["shim_other"] = [e.code, str(e)]
shim.close()
try:
    Client(sock, role="mcp_shim", token="made-up")
except RpcError as e:
    out["bad_token"] = [e.code, str(e)]
print(json.dumps(out))
"#;

#[test]
fn the_python_client_talks_to_the_rust_daemon() {
    let python = repo().join(".venv/bin/python");
    if !python.exists() {
        eprintln!(
            "SKIPPED: no {} (run `uv sync --group dev`)",
            python.display()
        );
        return;
    }
    let tmp = root();
    let run = Run::start(tmp.path(), "01", json!({}));
    let token = run.control(
        json!({"do": "issue", "task_id": "task-1", "worker_id": "w-1", "identity": "owner",
                                   "scopes": ["fs:read"]}),
    );
    run.control(json!({"do": "hold_task", "task_id": "task-1"}));
    std::fs::create_dir_all(run.home.join("workspace")).unwrap();
    let output = Command::new(python)
        .args(["-P", "-c", SCRIPT])
        .arg(run.socket())
        .arg(token.as_str().unwrap())
        .current_dir(repo())
        .env_clear()
        .env("FERB_HOME", &run.home)
        .env("HOME", &run.user)
        .env("PYTHONPATH", repo())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let out: Value = serde_json::from_slice(&output.stdout).unwrap();
    eprintln!("{out:#}");
    assert_eq!(out["ping"], "cli");
    assert_eq!(out["memory"], json!({"items": []}));
    assert_eq!(out["reminders"], json!({"pending": []}));
    assert_eq!(out["supervisor"][0], -32004);
    assert!(out["supervisor"][1]
        .as_str()
        .unwrap()
        .contains("supervisor"));
    assert_eq!(out["not_ported"][0], -32004);
    assert_eq!(out["unknown"], json!([-32001, "nope not allowed for cli"]));
    // the shim's grant (owner, fs:read): its listing, and a call through the gateway
    assert_eq!(
        out["tools"],
        json!([
            "actions.search",
            "abilities.list",
            "abilities.read",
            "fs.list",
            "fs.read",
            "fs.look",
            "fs.disk_usage",
            "owner.ask",
            "recording.inspect",
            "recovery.snapshot",
            "about.read"
        ])
    );
    // fs.list of the workspace through the gateway's whole path: the empty folder listed
    assert_eq!(out["call"]["is_error"], false, "{}", out["call"]);
    assert!(
        out["call"]["content"].get("entries").is_some(),
        "{}",
        out["call"]
    );
    assert!(out["shim_other"][0].as_i64().is_some());
    assert!(out["bad_token"][0].as_i64().is_some());
}
