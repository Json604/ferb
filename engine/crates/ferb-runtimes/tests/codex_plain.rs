//! RUN-480 (kb/plans/front-door.md, step 4) in the Codex adapter: a plain-mode run (a supervisor run with no
//! answer form) streams every `item/agentMessage/delta` whatever its phase (text before a call is `commentary`),
//! reports a FERB tool call on `item/started` and again on `item/completed`, answers with its last message, and a
//! cancel right after a completed call sends `turn/interrupt` before the app-server is killed, so the usage Codex
//! reports for the turn is handed over and nothing is raised. Scripted fake app-server, no model. Today's answer-form
//! runs are the recorded fixtures (`codex_matches_python`, "a typed answer streams its deltas, then usage and the
//! final answer" and "commentary is never a delta"), unchanged.

mod support;

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use ferb_core::types::{Identity, ModelRequest, RuntimeEventKind as K, Task, TaskRole};
use ferb_runtimes::codex::{CodexRuntime, Host, Mode};
use ferb_runtimes::run::{Adapter, Started};
use serde_json::{json, Value};
use support::*;

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

fn note(method: &str, params: Value) -> Value {
    let mut p = params;
    p["threadId"] = json!("th");
    if method != "turn/completed" {
        p["turnId"] = json!("tu");
    }
    json!({"out": {"method": method, "params": p}})
}

fn message(id: &str, phase: Value, pieces: &[&str]) -> Vec<Value> {
    let mut out = vec![note(
        "item/started",
        json!({"item": {"type": "agentMessage", "id": id, "phase": phase, "text": ""}}),
    )];
    for p in pieces {
        out.push(note(
            "item/agentMessage/delta",
            json!({"itemId": id, "delta": p}),
        ));
    }
    out.push(note(
        "item/completed",
        json!({"item": {"type": "agentMessage", "id": id, "phase": phase, "text": pieces.concat()}}),
    ));
    out
}

fn call(id: &str, status: &str, result: Value) -> Vec<Value> {
    vec![
        note(
            "item/started",
            json!({"item": {"id": id, "type": "mcpToolCall", "server": "ferb", "tool": "memory_read",
            "arguments": {"query": "x"}, "status": "inProgress"}}),
        ),
        note(
            "item/completed",
            json!({"item": {"id": id, "type": "mcpToolCall", "server": "ferb", "tool": "memory_read",
            "arguments": {"query": "x"}, "status": status, "result": result}}),
        ),
    ]
}

fn usage(input: u64) -> Value {
    note(
        "thread/tokenUsage/updated",
        json!({"tokenUsage": {"last": {"inputTokens": input, "cachedInputTokens": 2, "outputTokens": 4,
        "reasoningOutputTokens": 0}}}),
    )
}

fn done(status: &str) -> Value {
    note(
        "turn/completed",
        json!({"turn": {"id": "tu", "status": status, "error": null}}),
    )
}

fn script(turn: Vec<Value>) -> Value {
    let mut first = vec![
        json!({"err": "TRACE {\"tools\":[{\"type\":\"function\",\"name\":\"mcp__ferb.memory_read\"}]}\n"}),
        json!({"out": {"id": "$ID", "result": {"turn": {"id": "tu", "status": "inProgress"}}}}),
    ];
    first.extend(turn);
    json!({"on": {
        "initialize": [[{"out": {"id": "$ID", "result": {}}}]],
        "thread/start": [[{"out": {"id": "$ID", "result": {"thread": {"id": "th", "ephemeral": true},
            "model": "test-model", "modelProvider": "ferb_chatgpt", "approvalPolicy": "never",
            "sandbox": {"type": "readOnly", "networkAccess": false}, "instructionSources": []}}}]],
        "turn/start": [first],
        // measured on 0.166.0: the interrupt is acknowledged, the turn's usage reported, the turn ended
        "turn/interrupt": [[{"out": {"id": "$ID", "result": {}}}, usage(30), done("interrupted")]]}})
}

struct Played {
    started: Started,
    spawned: Arc<Spawned>,
    tmp: PathBuf,
    _dir: tempfile::TempDir,
}

async fn start(fake: Value) -> Played {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let tmp = dir.path().join("tmp");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&tmp).unwrap();
    write_files(
        &home,
        &json!({"codex-home/auth.json": "{\"tokens\":{\"refresh_token\":\"test-only-fixture\"}}"}),
    );
    let spawned = Arc::new(Spawned::default());
    let host = Host {
        spawner: Arc::new(FakeSpawner {
            script: fake,
            version: "codex-cli 0.166.0\n".into(),
            keying: Keying::Codex,
            spawned: spawned.clone(),
        }),
        temp_root: tmp.clone(),
        environ: [("PATH".to_string(), "/usr/bin:/bin".to_string())]
            .into_iter()
            .collect(),
        token_hex: counter(),
        today: Arc::new(|| "2026-11-26".into()),
        caches: Arc::new(Default::default()),
    };
    let mut rt = CodexRuntime::new(
        PathBuf::from("/opt/ferb/codex"),
        Mode::Supervisor,
        home,
        "test-model",
        &["memory.read".to_string()],
        host,
    );
    rt.shim_cmd = Some(vec!["/opt/ferb/bin/ferb".into(), "mcp".into()]);
    rt.monotonic = Arc::new(|| 0.0);
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

/// The run's text, tool, usage, final and failure events until it ends (or `until` matched one).
async fn told(started: &mut Started, until: Option<&dyn Fn(&Value) -> bool>) -> Vec<Value> {
    let mut out = Vec::new();
    while let Some(item) = tokio::time::timeout(Duration::from_secs(5), started.events.next())
        .await
        .expect("an event within 5 s")
    {
        let ev = item.expect("no error raised");
        if matches!(
            ev.kind,
            K::TextDelta | K::ToolCall | K::Final | K::Error | K::Violation | K::Usage
        ) {
            let mut data = Value::Object(ev.data);
            if ev.kind == K::Usage {
                data = json!({"input_tokens": data["input_tokens"]});
            }
            let v = json!({"kind": ev.kind.as_str(), "data": data});
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

fn final_(t: &str) -> Value {
    json!({"kind": "final", "data": {"text": t, "returncode": 0, "tools_seen": ["mcp__ferb.memory_read"]}})
}

// decision: RUN-480
#[tokio::test(flavor = "current_thread")]
async fn a_plain_reply_streams_its_pieces_and_ends_with_them_as_its_answer() {
    let mut turn = message("a", json!("final_answer"), &["Sure, ", "no ", "worries."]);
    turn.push(usage(8));
    turn.push(done("completed"));
    let mut p = start(script(turn)).await;
    let got = told(&mut p.started, None).await;
    assert_eq!(
        got,
        vec![
            delta("Sure, "),
            delta("no "),
            delta("worries."),
            json!({"kind": "usage", "data": {"input_tokens": 8}}),
            final_("Sure, no worries."),
        ]
    );
}

// decision: RUN-480
#[tokio::test(flavor = "current_thread")]
async fn a_line_then_a_call_then_the_answer_tell_the_call_started_and_completed_in_order() {
    // the line before the call is commentary (what Codex marks text that isn't its answer); the answer has no
    // phase at all here, which a plain-mode run streams just the same
    let mut turn = message("a1", json!("commentary"), &["Let me ", "look."]);
    turn.extend(call(
        "m1",
        "completed",
        json!({"content": [{"type": "text", "text": "x"}]}),
    ));
    turn.push(usage(8));
    turn.extend(message("a2", Value::Null, &["It is ", "x."]));
    turn.push(usage(9));
    turn.push(done("completed"));
    let mut p = start(script(turn)).await;
    let got = told(&mut p.started, None).await;
    assert_eq!(
        got,
        vec![
            delta("Let me "),
            delta("look."),
            json!({"kind": "tool_call", "data": {"tool": "memory_read", "name": "memory.read",
                "phase": "started", "arguments": {"query": "x"}}}),
            json!({"kind": "tool_call", "data": {"tool": "memory_read", "name": "memory.read",
                "phase": "completed", "arguments": {"query": "x"}, "error": false}}),
            json!({"kind": "usage", "data": {"input_tokens": 8}}),
            delta("It is "),
            delta("x."),
            json!({"kind": "usage", "data": {"input_tokens": 9}}),
            final_("It is x."),
        ]
    );
}

// decision: RUN-480
#[tokio::test(flavor = "current_thread")]
async fn a_call_that_failed_or_whose_result_is_an_error_completes_with_its_error() {
    let mut turn = call("m1", "failed", Value::Null);
    turn.extend(call(
        "m2",
        "completed",
        json!({"content": [{"type": "text", "text": "no such entry"}], "isError": true}),
    ));
    turn.extend(call("m3", "completed", json!({"content": []})));
    turn.extend(message("a", json!("final_answer"), &["Nothing found."]));
    turn.push(usage(8));
    turn.push(done("completed"));
    let mut p = start(script(turn)).await;
    let got = told(&mut p.started, None).await;
    let errors: Vec<Value> = got
        .iter()
        .filter(|v| v["data"]["phase"] == "completed")
        .map(|v| v["data"]["error"].clone())
        .collect();
    assert_eq!(errors, [json!(true), json!(true), json!(false)], "{got:?}");
    assert_eq!(got.last().unwrap(), &final_("Nothing found."));
}

// decision: RUN-480, RUN-477
#[tokio::test(flavor = "current_thread")]
async fn a_cancel_right_after_a_completed_call_interrupts_the_turn_reports_usage_and_raises_nothing(
) {
    let mut turn = message("a1", json!("commentary"), &["On it."]);
    turn.extend(call("m1", "completed", json!({"content": []})));
    let mut p = start(script(turn)).await;
    let completed = |v: &Value| v["data"]["phase"] == "completed";
    let got = told(&mut p.started, Some(&completed)).await;
    assert_eq!(got.len(), 3, "{got:?}");
    tokio::time::timeout(Duration::from_secs(10), p.started.control.cancel())
        .await
        .expect("the cancel ends");
    let app = p.spawned.apps.lock().unwrap().last().cloned().unwrap();
    let lines = app.lines.lock().unwrap().clone();
    assert!(
        lines
            .iter()
            .any(|l| l.contains("\"method\": \"turn/interrupt\"")
                && l.contains("\"turnId\": \"tu\"")),
        "the turn was not interrupted: {lines:?}"
    );
    assert!(app.exited().is_some(), "the app-server is left running");
    let left = p.started.control.unreported_usage();
    assert_eq!(left.len(), 1, "{left:?}");
    assert_eq!(
        (&left[0]["input_tokens"], &left[0]["recorded_at_end"]),
        (&json!(30), &json!(true))
    );
    let after = tokio::time::timeout(Duration::from_secs(5), p.started.events.next())
        .await
        .expect("the stream ends");
    assert!(after.is_none(), "{after:?}");
    let folders: Vec<_> = std::fs::read_dir(&p.tmp).unwrap().collect();
    assert!(folders.is_empty(), "the run's folder is left");
}
