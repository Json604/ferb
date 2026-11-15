//! A run token's connection to the real daemon (the MCP shim's role) and a daemon with one live task.
#![allow(dead_code)]

use serde_json::{json, Value};

use crate::support::*;

/// A run token's connection (the MCP shim's role), its calls through the gateway.
pub struct Shim(Raw, i64);

impl Shim {
    /// As `open`, its calls waiting up to `seconds` for an answer (a real-machine check's slow first text recognition).
    pub fn open_waiting(run: &Run, token: &str, seconds: u64) -> Shim {
        let s = std::os::unix::net::UnixStream::connect(run.socket()).expect("the socket");
        s.set_read_timeout(Some(std::time::Duration::from_secs(seconds)))
            .unwrap();
        let mut raw = Raw::from_stream(s).expect("the socket");
        raw.send(
            &json!({"jsonrpc": "2.0", "id": 0, "method": "auth.hello",
                    "params": {"role": "mcp_shim", "token": token}})
            .to_string(),
        );
        let hello: Value = serde_json::from_str(&raw.recv().expect("hello")).unwrap();
        assert!(hello.get("error").is_none(), "{hello}");
        Shim(raw, 1)
    }

    pub fn open(run: &Run, token: &str) -> Shim {
        let mut raw = Raw::connect(&run.socket()).expect("the socket");
        raw.send(
            &json!({"jsonrpc": "2.0", "id": 0, "method": "auth.hello",
                    "params": {"role": "mcp_shim", "token": token}})
            .to_string(),
        );
        let hello: Value = serde_json::from_str(&raw.recv().expect("hello")).unwrap();
        assert!(hello.get("error").is_none(), "{hello}");
        Shim(raw, 1)
    }

    /// `tools.call`: the gateway's reply (`content`, `is_error`, `footer`).
    pub fn call(&mut self, name: &str, args: Value) -> Value {
        self.1 += 1;
        self.0.send(
            &json!({"jsonrpc": "2.0", "id": self.1, "method": "tools.call",
                    "params": {"name": name, "arguments": args}})
            .to_string(),
        );
        let line = self
            .0
            .recv()
            .unwrap_or_else(|| panic!("{name}: no answer within the read timeout"));
        let v: Value = serde_json::from_str(&line).unwrap();
        v.get("result")
            .cloned()
            .unwrap_or_else(|| panic!("{name}: {line}"))
    }
}

pub fn all_scopes() -> Value {
    fixture("catalog/tools.json")["listings"]["worker_owner_all_scopes"]["grant"]["scopes"].clone()
}

/// A daemon with one live task (`task-1`) and an owner token holding every scope.
pub fn started(tmp: &std::path::Path, nn: &str) -> (Run, Shim) {
    let run = Run::start(tmp, nn, json!({"ids": "id"}));
    let token = run.control(
        json!({"do": "issue", "task_id": "task-1", "worker_id": "w-1",
                                   "identity": "owner", "scopes": all_scopes()}),
    );
    run.control(json!({"do": "hold_task", "task_id": "task-1"}));
    std::fs::create_dir_all(run.home.join("workspace")).unwrap();
    let shim = Shim::open(&run, token.as_str().unwrap());
    (run, shim)
}
