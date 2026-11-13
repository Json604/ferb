//! The tool bridge against rust/fixtures/runtimes/mcp.json: the same MCP lines in, the same bytes out, and the same
//! requests reaching the daemon's socket, as the Python shim on the `mcp` library.

mod support;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Value};
use support::*;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixListener;

async fn fake_daemon(listener: UnixListener, sc: Value, log: Arc<Mutex<Vec<Vec<String>>>>) {
    let calls = Arc::new(Mutex::new(
        sc["calls"].as_array().cloned().unwrap_or_default(),
    ));
    loop {
        let Ok((stream, _)) = listener.accept().await else {
            return;
        };
        let index = {
            let mut l = log.lock().unwrap();
            l.push(Vec::new());
            l.len() - 1
        };
        let (log, calls, sc) = (log.clone(), calls.clone(), sc.clone());
        tokio::spawn(async move {
            let (read, mut write) = stream.into_split();
            let mut read = BufReader::new(read);
            loop {
                let mut line = String::new();
                if read.read_line(&mut line).await.unwrap_or(0) == 0 {
                    return;
                }
                log.lock().unwrap()[index].push(line.clone());
                let msg: Value = serde_json::from_str(&line).unwrap();
                let id = msg["id"].clone();
                let reply = match msg["method"].as_str().unwrap() {
                    "auth.hello" => {
                        json!({"jsonrpc": "2.0", "id": id, "result": {"ok": true, "role": "mcp_shim"}})
                    }
                    "tools.list" if sc["tools"].is_null() => json!({"jsonrpc": "2.0", "id": id,
                        "error": {"code": -32002, "message": "run token revoked"}}),
                    "tools.list" => {
                        json!({"jsonrpc": "2.0", "id": id, "result": {"tools": sc["tools"]}})
                    }
                    "tools.call" => {
                        let step = calls.lock().unwrap().remove(0);
                        if step.get("hang").is_some() {
                            let mut sink = String::new();
                            while read.read_line(&mut sink).await.unwrap_or(0) > 0 {}
                            log.lock().unwrap()[index].push("<closed>".into());
                            return;
                        }
                        if step.get("break").is_some() {
                            return;
                        }
                        match step.get("error") {
                            Some(e) => json!({"jsonrpc": "2.0", "id": id, "error": e}),
                            None => json!({"jsonrpc": "2.0", "id": id, "result": step["result"]}),
                        }
                    }
                    _ => {
                        json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32601, "message": "method not found"}})
                    }
                };
                let line = format!("{}\n", ferb_py::json::dumps(&reply));
                if write.write_all(line.as_bytes()).await.is_err() {
                    return;
                }
            }
        });
    }
}

async fn play(case: &Value) -> Value {
    let sc = &case["scenario"];
    let want = &case["python"];
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("run")).unwrap();
    let sock = dir.path().join("run").join("ferb.sock");
    let log = Arc::new(Mutex::new(Vec::new()));
    let listener = UnixListener::bind(&sock).unwrap();
    let daemon = tokio::spawn(fake_daemon(listener, sc.clone(), log.clone()));
    let mut env = HashMap::new();
    if let Some(f) = sc["token"].get("file").and_then(Value::as_str) {
        std::fs::write(dir.path().join("token"), f).unwrap();
        env.insert(
            "FERB_RUN_TOKEN_FILE".to_string(),
            dir.path().join("token").to_string_lossy().to_string(),
        );
    }
    if let Some(t) = sc["token"].get("env").and_then(Value::as_str) {
        env.insert("FERB_RUN_TOKEN".to_string(), t.to_string());
    }
    let Some(token) = ferb_runtimes::mcp_bridge::token_from(&env) else {
        daemon.abort();
        return json!({"name": sc["name"], "stdout": [], "daemon": [], "exit": 1,
            "stderr_last": ferb_runtimes::mcp_bridge::NO_TOKEN});
    };
    let (mut to_bridge, bridge_in) = tokio::io::duplex(1 << 20);
    let (bridge_out, from_bridge) = tokio::io::duplex(1 << 20);
    let server = tokio::spawn(ferb_runtimes::mcp_bridge::serve(
        bridge_in,
        bridge_out,
        sock.clone(),
        token,
    ));
    let mut reader = BufReader::new(from_bridge);
    let expected_ids: Vec<Value> = want["stdout"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| serde_json::from_str::<Value>(l.as_str().unwrap()).unwrap()["id"].clone())
        .collect();
    let mut out: Vec<String> = Vec::new();
    for line in sc["stdin"].as_array().unwrap() {
        let line = line.as_str().unwrap();
        to_bridge.write_all(line.as_bytes()).await.unwrap();
        let id = serde_json::from_str::<Value>(line)
            .ok()
            .and_then(|m| m.get("id").cloned());
        let answered = id.as_ref().is_some_and(|i| expected_ids.contains(i));
        if answered {
            loop {
                let mut got = String::new();
                tokio::time::timeout(Duration::from_secs(5), reader.read_line(&mut got))
                    .await
                    .expect("an answer within 5 s")
                    .unwrap();
                out.push(got.clone());
                let reply: Value = serde_json::from_str(&got).unwrap();
                if Some(&reply["id"]) == id.as_ref() {
                    break;
                }
            }
        } else {
            // Python got nothing back for this line: neither may Rust
            let mut got = String::new();
            if let Ok(Ok(n)) =
                tokio::time::timeout(Duration::from_millis(150), reader.read_line(&mut got)).await
            {
                if n > 0 {
                    out.push(got);
                }
            }
        }
    }
    drop(to_bridge);
    tokio::time::timeout(Duration::from_secs(5), server)
        .await
        .expect("the bridge ends")
        .unwrap();
    let mut rest = String::new();
    while reader.read_line(&mut rest).await.unwrap_or(0) > 0 {
        out.push(std::mem::take(&mut rest));
    }
    tokio::task::yield_now().await;
    daemon.abort();
    let daemon_log = log.lock().unwrap().clone();
    json!({"name": sc["name"], "stdout": out, "daemon": daemon_log, "exit": 0,
        "stderr_last": want["stderr_last"]})
}

// decision: RUN-415, RUN-407, RUN-426
#[tokio::test(flavor = "current_thread")]
async fn bridge_matches_python() {
    let fx = fixture("mcp.json");
    let cases = fx["cases"].as_array().unwrap();
    let mut failures = Vec::new();
    for case in cases {
        let got = play(case).await;
        if let Some(d) = first_diff(&got, &case["python"], "") {
            failures.push(format!("{}: {d}", case["scenario"]["name"]));
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {} differ:\n{}",
        failures.len(),
        cases.len(),
        failures.join("\n")
    );
}
