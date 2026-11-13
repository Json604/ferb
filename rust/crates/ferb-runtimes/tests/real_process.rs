//! The real program path (no in-process fake): `OsSpawner` starts a small shell-script Claude Code stand-in in a
//! temp folder that speaks the stream-json frames; the run starts, answers, and on cancel the program gets EOF on
//! stdin, ends and is reaped. And the tool-list proxy on 127.0.0.1 in front of a local upstream.

mod support;

use std::collections::BTreeSet;
use std::os::unix::fs::PermissionsExt;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ferb_core::types::{Identity, ModelRequest, Task, TaskRole};
use ferb_runtimes::claude::proxy::{tool_names, ToolListProxy};
use ferb_runtimes::claude::{ClaudeRuntime, Host, Mode, ToolProxy};
use ferb_runtimes::forms::Refusals;
use ferb_runtimes::run::Adapter;
use serde_json::json;
use support::RecordedForms;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

const SCRIPT: &str = r#"#!/bin/sh
if [ "$1" = "-v" ]; then echo "2.1.310 (Claude Code)"; exit 0; fi
printf '%s\n' "$@" > "$(dirname "$0")/argv.txt"
read -r init
id=$(printf '%s' "$init" | sed 's/.*"request_id": "\([^"]*\)".*/\1/')
printf '{"type": "control_response", "response": {"subtype": "success", "request_id": "%s", "response": {}}}\n' "$id"
read -r user
printf '%s\n' "$user" > "$(dirname "$0")/user.txt"
printf '%s\n' '{"type": "system", "subtype": "init", "apiKeySource": "none", "session_id": "s", "uuid": "u"}'
printf '%s\n' '{"type": "result", "subtype": "success", "duration_ms": 1, "duration_api_ms": 1, "is_error": false, "num_turns": 1, "session_id": "s", "result": "done", "usage": {"input_tokens": 3, "output_tokens": 1}}'
cat > /dev/null
echo ended > "$(dirname "$0")/ended.txt"
"#;

struct NoProxy(Mutex<Vec<String>>);

impl ToolProxy for NoProxy {
    fn base_url(&self, run_id: &str) -> String {
        format!("http://127.0.0.1:9/run/{run_id}/s")
    }
    fn seen(&self, _run_id: &str) -> BTreeSet<String> {
        BTreeSet::new()
    }
    fn forget(&self, run_id: &str) {
        self.0.lock().unwrap().push(run_id.into());
    }
}

// decision: ENG-2, RUN-409
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_real_claude_program_runs_answers_and_is_reaped() {
    let dir = tempfile::tempdir().unwrap();
    let cli = dir.path().join("claude");
    std::fs::write(&cli, SCRIPT).unwrap();
    std::fs::set_permissions(&cli, std::fs::Permissions::from_mode(0o755)).unwrap();
    let home = dir.path().join("home");
    std::fs::create_dir_all(home.join("claude-home")).unwrap();
    std::fs::write(home.join("claude-home/.ferb-signed-in"), "x").unwrap();
    let mut host = Host::system();
    host.temp_root = dir.path().to_path_buf();
    host.environ = [("PATH".to_string(), "/usr/bin:/bin".to_string())]
        .into_iter()
        .collect();
    let proxy = Arc::new(NoProxy(Mutex::new(Vec::new())));
    let mut rt = ClaudeRuntime::new(
        Mode::Supervisor,
        proxy.clone(),
        home,
        &[],
        None,
        Some("claude-opus-5-5".into()),
        Arc::new(RecordedForms(json!({}))),
        Arc::new(Refusals::new(Box::new(|| 0.0))),
        host,
    );
    rt.cli = Some(cli.clone());
    let task = Task {
        id: "t".into(),
        parent_id: None,
        goal: "g".into(),
        device_id: "d".into(),
        identity: Identity::Owner,
        scopes: BTreeSet::new(),
        role: TaskRole::Mixed,
        request_id: "r".into(),
        success_conditions: vec![],
        deadline: None,
    };
    let request = ModelRequest {
        system: "S".into(),
        parts: vec!["hello".into()],
        cacheable: 0,
        images: vec![],
    };
    let mut started = tokio::time::timeout(Duration::from_secs(10), rt.start(&task, &request, ""))
        .await
        .unwrap()
        .unwrap();
    let mut kinds = Vec::new();
    while let Some(item) = tokio::time::timeout(Duration::from_secs(10), started.events.next())
        .await
        .unwrap()
    {
        let ev = item.unwrap();
        kinds.push(ev.kind.as_str().to_string());
        if ev.kind.as_str() == "final" {
            assert_eq!(ev.data["text"], "done");
        }
    }
    assert_eq!(kinds, ["usage", "final"]);
    let user = std::fs::read_to_string(dir.path().join("user.txt")).unwrap();
    assert!(user.contains("\"session_id\": \"default\"") && user.contains("\"text\": \"S\""));
    let argv = std::fs::read_to_string(dir.path().join("argv.txt")).unwrap();
    assert!(
        argv.contains("--input-format\nstream-json") && argv.contains("--permission-mode\ndontAsk")
    );
    tokio::time::timeout(Duration::from_secs(15), started.control.cancel())
        .await
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(dir.path().join("ended.txt")).unwrap(),
        "ended\n"
    );
    assert_eq!(proxy.0.lock().unwrap().len(), 1);
}

#[test]
fn tool_names_are_the_only_thing_read() {
    assert_eq!(
        tool_names(br#"{"tools":[{"name":"a"},{"type":"web_search_2"},{},5],"messages":[]}"#),
        ["?", "a", "web_search_2"]
            .into_iter()
            .map(String::from)
            .collect::<BTreeSet<_>>()
    );
    assert!(tool_names(b"not json").is_empty());
    assert!(tool_names(br#"{"tools": "x"}"#).is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_proxy_forwards_only_for_a_live_run_and_keeps_only_tool_names() {
    // a local upstream that answers every request with two chunks
    let upstream = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let up = format!("http://127.0.0.1:{}", upstream.local_addr().unwrap().port());
    let seen_paths = Arc::new(Mutex::new(Vec::new()));
    let paths = seen_paths.clone();
    tokio::spawn(async move {
        loop {
            let Ok((s, _)) = upstream.accept().await else {
                return;
            };
            let paths = paths.clone();
            tokio::spawn(async move {
                let (r, mut w) = s.into_split();
                let mut r = BufReader::new(r);
                let mut line = String::new();
                r.read_line(&mut line).await.unwrap();
                paths.lock().unwrap().push(line.trim().to_string());
                let mut len = 0;
                loop {
                    let mut h = String::new();
                    r.read_line(&mut h).await.unwrap();
                    if h.trim().is_empty() {
                        break;
                    }
                    if let Some(v) = h.to_lowercase().strip_prefix("content-length:") {
                        len = v.trim().parse().unwrap();
                    }
                }
                let mut body = vec![0; len];
                r.read_exact(&mut body).await.unwrap();
                w.write_all(b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: 11\r\n\r\nhello world").await.unwrap();
            });
        }
    });
    let proxy = ToolListProxy::start(&up, Arc::new(|| "secret-1".to_string()))
        .await
        .unwrap();
    let base = proxy.base_url("run-1");
    assert!(base.ends_with("/run/run-1/secret-1"));
    let client = reqwest::Client::new();
    let body = r#"{"tools":[{"name":"mcp__ferb__x"},{"name":"Bash"}],"messages":[{"role":"user","content":"private"}]}"#;
    let resp = client
        .post(format!("{base}/v1/messages?beta=true"))
        .body(body)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.text().await.unwrap(), "hello world");
    assert_eq!(
        proxy.seen("run-1"),
        ["Bash", "mcp__ferb__x"]
            .into_iter()
            .map(String::from)
            .collect()
    );
    assert_eq!(
        seen_paths.lock().unwrap()[0],
        "POST /v1/messages?beta=true HTTP/1.1"
    );
    let port = base
        .split(':')
        .nth(2)
        .unwrap()
        .split('/')
        .next()
        .unwrap()
        .to_string();
    for wrong in [
        format!("http://127.0.0.1:{port}/run/run-1/wrong/v1/messages"),
        format!("http://127.0.0.1:{port}/run/run-2/secret-1/v1/messages"),
        format!("http://127.0.0.1:{port}/v1/messages"),
    ] {
        let r = reqwest::Client::new()
            .post(wrong)
            .body("{}")
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 403);
    }
    proxy.forget("run-1");
    assert!(proxy.seen("run-1").is_empty());
    let r = reqwest::Client::new()
        .post(format!("{base}/v1/messages"))
        .body("{}")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 403);
    assert_eq!(seen_paths.lock().unwrap().len(), 1);
}
