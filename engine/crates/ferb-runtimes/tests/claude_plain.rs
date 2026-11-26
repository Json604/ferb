//! RUN-480 (kb/plans/front-door.md, step 4) in the Claude adapter: a plain-mode run (a supervisor run with no
//! answer form) streams its answer text as Claude Code writes it (`--include-partial-messages`), reports each tool
//! call when its `tool_use` block starts and again when its result comes back in the next user message, and a cancel
//! right after a completed call interrupts the turn, ends the program, hands over what the relay counted and raises
//! nothing. Scripted fake Claude Code, no model. Today's answer-form runs are the recorded fixtures
//! (`claude_matches_python`, "a typed answer streams only the StructuredOutput payload"), unchanged.

mod support;

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ferb_core::types::{Identity, ModelRequest, RuntimeEventKind as K, Task, TaskRole};
use ferb_runtimes::claude::tally::Tally;
use ferb_runtimes::claude::{ClaudeRuntime, Host, Mode, ToolProxy};
use ferb_runtimes::forms::Refusals;
use ferb_runtimes::run::{Adapter, Started};
use serde_json::{json, Value};
use support::*;

const TOOL: &str = "mcp__ferb__memory_read";

struct Relay {
    tally: Mutex<Option<Tally>>,
}

impl ToolProxy for Relay {
    fn base_url(&self, run_id: &str) -> String {
        format!("http://127.0.0.1:9/run/{run_id}/fixed-secret")
    }
    fn seen(&self, _run_id: &str) -> BTreeSet<String> {
        [TOOL.to_string()].into_iter().collect()
    }
    fn forget(&self, _run_id: &str) {}
    fn take_usage(&self, _run_id: &str) -> Option<Tally> {
        self.tally.lock().unwrap().take()
    }
}

fn task() -> Task {
    Task {
        id: "task-1".into(),
        parent_id: None,
        goal: "reply".into(),
        device_id: "device-1".into(),
        identity: Identity::Owner,
        scopes: BTreeSet::new(),
        role: TaskRole::Mixed,
        request_id: "req-1".into(),
        success_conditions: vec![],
        deadline: None,
    }
}

fn stream(event: Value) -> Value {
    json!({"out": {"type": "stream_event", "uuid": "se", "session_id": "s", "parent_tool_use_id": null,
        "event": event}})
}

fn text_block(index: u64, pieces: &[&str]) -> Vec<Value> {
    let mut out = vec![stream(
        json!({"type": "content_block_start", "index": index, "content_block": {"type": "text", "text": ""}}),
    )];
    for p in pieces {
        out.push(stream(
            json!({"type": "content_block_delta", "index": index,
            "delta": {"type": "text_delta", "text": p}}),
        ));
    }
    out.push(stream(
        json!({"type": "content_block_stop", "index": index}),
    ));
    out
}

fn tool_block(index: u64, id: &str, partial: &str) -> Vec<Value> {
    vec![
        stream(json!({"type": "content_block_start", "index": index,
            "content_block": {"type": "tool_use", "id": id, "name": TOOL, "input": {}}})),
        stream(json!({"type": "content_block_delta", "index": index,
            "delta": {"type": "input_json_delta", "partial_json": partial}})),
        stream(json!({"type": "content_block_stop", "index": index})),
    ]
}

fn assistant(content: Value) -> Value {
    json!({"out": {"type": "assistant", "message": {"model": "claude-sonnet-5-5", "id": "msg",
        "content": content, "stop_reason": null}, "session_id": "s", "uuid": "ua", "parent_tool_use_id": null}})
}

fn tool_result(id: &str, is_error: bool) -> Value {
    json!({"out": {"type": "user", "message": {"role": "user", "content": [
        {"type": "tool_result", "tool_use_id": id, "content": "found", "is_error": is_error}]},
        "session_id": "s", "uuid": "ur", "parent_tool_use_id": null}})
}

fn result(text: &str) -> Value {
    json!({"out": {"type": "result", "subtype": "success", "duration_ms": 1, "duration_api_ms": 1,
        "is_error": false, "num_turns": 1, "session_id": "s", "result": text,
        "usage": {"input_tokens": 10, "output_tokens": 5}, "stop_reason": "end_turn"}})
}

fn init() -> Vec<Value> {
    vec![
        json!({"out": {"type": "system", "subtype": "init", "apiKeySource": "none", "session_id": "s",
        "uuid": "u0", "tools": [TOOL]}}),
        stream(json!({"type": "message_start", "message": {}})),
    ]
}

fn script(user: Vec<Value>) -> Value {
    json!({"on": {
        "control:initialize": [[{"out": {"type": "control_response", "response": {"subtype": "success",
            "request_id": "$ID", "response": {"commands": []}}}}]],
        "control:interrupt": [[{"out": {"type": "control_response", "response": {"subtype": "success",
            "request_id": "$ID", "response": {}}}}]],
        "user": [user]}})
}

struct Played {
    started: Started,
    spawned: Arc<Spawned>,
    tmp: std::path::PathBuf,
    _dir: tempfile::TempDir,
}

async fn start(fake: Value, tally: Option<Tally>) -> Played {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let tmp = dir.path().join("tmp");
    std::fs::create_dir_all(home.join("claude-home")).unwrap();
    std::fs::write(home.join("claude-home/.ferb-signed-in"), "x\n").unwrap();
    std::fs::create_dir_all(&tmp).unwrap();
    let spawned = Arc::new(Spawned::default());
    let host = Host {
        spawner: Arc::new(FakeSpawner {
            script: fake,
            version: "2.1.310 (Claude Code)\n".into(),
            keying: Keying::Claude,
            spawned: spawned.clone(),
        }),
        temp_root: tmp.clone(),
        environ: [("PATH".to_string(), "/usr/bin:/bin".to_string())]
            .into_iter()
            .collect(),
        token_hex: counter(),
        uuid4: Arc::new(|| uuid::Uuid::from_u128(1).to_string()),
        urandom4: Arc::new(|| "abcdef01".into()),
        today: Arc::new(|| "2026-11-26".into()),
    };
    let mut rt = ClaudeRuntime::new(
        Mode::Supervisor,
        Arc::new(Relay {
            tally: Mutex::new(tally),
        }),
        home,
        &["memory.read".to_string()],
        None,
        Some("claude-sonnet-5-5".into()),
        Arc::new(RecordedForms(json!({}))),
        Arc::new(Refusals::new(Box::new(|| 0.0))),
        host,
    );
    rt.monotonic = Arc::new(|| 0.0);
    rt.shim_cmd = Some(vec!["/opt/ferb/bin/ferb".into(), "mcp".into()]);
    rt.cli = Some("/opt/ferb/claude".into());
    let request = ModelRequest {
        system: "FERB".into(),
        parts: vec!["ok no worries".into()],
        cacheable: 0,
        images: vec![],
    };
    let started = rt.start(&task(), &request, "tok").await.expect("started");
    Played {
        started,
        spawned,
        tmp,
        _dir: dir,
    }
}

/// The run's text, tool, final and failure events until it ends (or `until` matched one).
async fn told(started: &mut Started, until: Option<&dyn Fn(&Value) -> bool>) -> Vec<Value> {
    let mut out = Vec::new();
    while let Some(item) = tokio::time::timeout(Duration::from_secs(5), started.events.next())
        .await
        .expect("an event within 5 s")
    {
        let ev = item.expect("no error raised");
        if matches!(
            ev.kind,
            K::TextDelta | K::ToolCall | K::Final | K::Error | K::Violation | K::InvalidAnswer
        ) {
            let v = json!({"kind": ev.kind.as_str(), "data": Value::Object(ev.data)});
            let stop = until.is_some_and(|f| f(&v));
            out.push(v);
            if stop {
                break;
            }
        }
    }
    out
}

fn delta(t: &str) -> Value {
    json!({"kind": "text_delta", "data": {"text": t}})
}

// decision: RUN-480
#[tokio::test(flavor = "current_thread")]
async fn a_plain_reply_streams_its_pieces_and_ends_with_them_as_its_answer() {
    let mut user = init();
    user.extend(text_block(0, &["Sure, ", "no ", "worries."]));
    user.push(assistant(
        json!([{"type": "text", "text": "Sure, no worries."}]),
    ));
    user.push(result("Sure, no worries."));
    let mut p = start(script(user), None).await;
    let argv = p.spawned.specs.lock().unwrap()[1].argv.clone();
    assert!(
        argv.iter().any(|a| a == "--include-partial-messages"),
        "{argv:?}"
    );
    let got = told(&mut p.started, None).await;
    assert_eq!(
        got,
        vec![
            delta("Sure, "),
            delta("no "),
            delta("worries."),
            json!({"kind": "final", "data": {"text": "Sure, no worries.", "tools_seen": [TOOL]}}),
        ]
    );
    p.started.control.cancel().await;
}

// decision: RUN-480
#[tokio::test(flavor = "current_thread")]
async fn a_line_then_a_call_then_the_answer_tell_the_call_started_and_completed_in_order() {
    let mut user = init();
    user.extend(text_block(0, &["Let me ", "look."]));
    user.extend(tool_block(1, "tu1", "{\"query\": \"x\"}"));
    user.push(assistant(json!([{"type": "text", "text": "Let me look."},
        {"type": "tool_use", "id": "tu1", "name": TOOL, "input": {"query": "x"}}])));
    user.push(tool_result("tu1", false));
    user.push(stream(json!({"type": "message_start", "message": {}})));
    user.extend(text_block(0, &["It is ", "x."]));
    user.push(assistant(json!([{"type": "text", "text": "It is x."}])));
    user.push(result("It is x."));
    let mut p = start(script(user), None).await;
    let got = told(&mut p.started, None).await;
    assert_eq!(
        got,
        vec![
            delta("Let me "),
            delta("look."),
            json!({"kind": "tool_call", "data": {"tool": TOOL, "name": "memory.read", "phase": "started",
                "arguments": null}}),
            json!({"kind": "tool_call", "data": {"tool": TOOL, "name": "memory.read", "phase": "completed",
                "arguments": {"query": "x"}, "error": false}}),
            delta("It is "),
            delta("x."),
            json!({"kind": "final", "data": {"text": "It is x.", "tools_seen": [TOOL]}}),
        ]
    );
    p.started.control.cancel().await;
}

// decision: RUN-480
#[tokio::test(flavor = "current_thread")]
async fn a_call_whose_result_is_an_error_completes_with_its_error() {
    let mut user = init();
    user.extend(tool_block(0, "tu1", "{}"));
    user.push(assistant(
        json!([{"type": "tool_use", "id": "tu1", "name": TOOL, "input": {}}]),
    ));
    user.push(tool_result("tu1", true));
    user.extend(text_block(0, &["That failed."]));
    user.push(assistant(json!([{"type": "text", "text": "That failed."}])));
    user.push(result("That failed."));
    let mut p = start(script(user), None).await;
    let got = told(&mut p.started, None).await;
    assert_eq!(
        got[1],
        json!({"kind": "tool_call", "data": {"tool": TOOL, "name": "memory.read", "phase": "completed",
            "arguments": {}, "error": true}})
    );
    assert_eq!(got.len(), 4, "{got:?}");
    p.started.control.cancel().await;
}

// decision: RUN-480, RUN-477
#[tokio::test(flavor = "current_thread")]
async fn a_cancel_right_after_a_completed_call_ends_the_program_reports_usage_and_raises_nothing() {
    let mut user = init();
    user.extend(text_block(0, &["On it."]));
    user.extend(tool_block(1, "tu1", "{\"query\": \"x\"}"));
    user.push(assistant(json!([{"type": "text", "text": "On it."},
        {"type": "tool_use", "id": "tu1", "name": TOOL, "input": {"query": "x"}}])));
    user.push(tool_result("tu1", false));
    // Claude Code goes on to its next request; no result frame comes before FERB ends the run
    user.push(stream(json!({"type": "message_start", "message": {}})));
    let tally = Tally {
        requests: 1,
        input: 40,
        output: 12,
        model: "claude-sonnet-5-5".into(),
        ..Tally::default()
    };
    let mut p = start(script(user), Some(tally)).await;
    let completed = |v: &Value| v["data"]["phase"] == "completed";
    let got = told(&mut p.started, Some(&completed)).await;
    assert_eq!(got.last().unwrap()["data"]["phase"], "completed", "{got:?}");
    tokio::time::timeout(Duration::from_secs(10), p.started.control.cancel())
        .await
        .expect("the cancel ends");
    let app = p.spawned.apps.lock().unwrap().last().cloned().unwrap();
    assert!(app.exited().is_some(), "the program is left running");
    let lines = app.lines.lock().unwrap().clone();
    assert!(
        lines
            .iter()
            .any(|l| l.contains("\"subtype\": \"interrupt\"")),
        "the turn was not interrupted: {lines:?}"
    );
    let left = p.started.control.unreported_usage();
    assert_eq!(left.len(), 1, "{left:?}");
    assert_eq!(
        (
            &left[0]["input_tokens"],
            &left[0]["output_tokens"],
            &left[0]["recorded_at_end"]
        ),
        (&json!(40), &json!(12), &json!(true))
    );
    // nothing after the cancel: the stream ends without an error or a violation
    let after = tokio::time::timeout(Duration::from_secs(5), p.started.events.next())
        .await
        .expect("the stream ends");
    assert!(after.is_none(), "{after:?}");
    let folders: Vec<_> = std::fs::read_dir(&p.tmp).unwrap().collect();
    assert!(folders.is_empty(), "the run token's folder is left");
}
