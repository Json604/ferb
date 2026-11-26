//! The direct-API loop against engine/fixtures/runtimes/api.json: a local scripted server on 127.0.0.1 answers as the
//! Python test's httpx transport did; every request body must be byte-identical to Python's, and the events, the
//! gateway calls, the retries and the refusal memory the same.

mod support;

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ferb_core::errors::FerbError;
use ferb_core::types::{Identity, Task, TaskRole};
use ferb_identity::Secret;
use ferb_runtimes::api::{self, anthropic, google, openai, ApiRuntime, Bridge, Provider};
use ferb_runtimes::forms::Refusals;
use ferb_runtimes::run::Adapter;
use serde_json::{json, Value};
use support::*;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;

const HEADERS: [&str; 5] = [
    "anthropic-version",
    "content-type",
    "x-api-key",
    "authorization",
    "x-goog-api-key",
];

struct FakeBridge {
    tools: Vec<Value>,
    results: Mutex<Vec<Value>>,
    log: Arc<Mutex<Vec<Value>>>,
}

impl Bridge for FakeBridge {
    fn tools(&self, _run_token: &str) -> Vec<Value> {
        self.tools.clone()
    }
    fn call<'a>(
        &'a self,
        run_token: &'a str,
        name: &'a str,
        arguments: &'a Value,
    ) -> api::BoxFuture<'a, Result<Value, FerbError>> {
        self.log
            .lock()
            .unwrap()
            .push(json!(["call", run_token, name, arguments]));
        let next = {
            let mut r = self.results.lock().unwrap();
            if r.is_empty() {
                None
            } else {
                Some(r.remove(0))
            }
        };
        let result = next.unwrap_or_else(|| {
            json!({"content": {"entries": ["a.txt"], "size": 1.5}, "footer": "[fs]",
            "is_error": false, "image": null})
        });
        Box::pin(async move {
            match result.get("refuse").and_then(Value::as_str) {
                Some(m) => Err(FerbError::Refused(m.to_string())),
                None => Ok(result),
            }
        })
    }
}

fn sse_chunks(item: &Value) -> Vec<Vec<u8>> {
    if let Some(raw) = item.get("sse_raw").and_then(Value::as_str) {
        return vec![raw.as_bytes().to_vec()];
    }
    let nl = if item.get("crlf") == Some(&json!(true)) {
        "\r\n"
    } else {
        "\n"
    };
    let text: String = item["sse"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| format!("data: {}{nl}{nl}", ferb_py::json::dumps(e)))
        .collect();
    match item.get("split").and_then(Value::as_u64) {
        Some(n) => text
            .as_bytes()
            .chunks(n as usize)
            .map(<[u8]>::to_vec)
            .collect(),
        None => vec![text.into_bytes()],
    }
}

async fn server(
    listener: TcpListener,
    responses: Vec<Value>,
    requests: Arc<Mutex<Vec<Value>>>,
    key: String,
) {
    let queue = Arc::new(Mutex::new(responses));
    loop {
        let Ok((stream, _)) = listener.accept().await else {
            return;
        };
        let (queue, requests, key) = (queue.clone(), requests.clone(), key.clone());
        tokio::spawn(async move {
            let (read, mut write) = stream.into_split();
            let mut read = BufReader::new(read);
            loop {
                let mut line = String::new();
                if read.read_line(&mut line).await.unwrap_or(0) == 0 {
                    return;
                }
                let mut parts = line.split_whitespace();
                let method = parts.next().unwrap_or_default().to_string();
                let path = parts.next().unwrap_or_default().to_string();
                let mut headers = serde_json::Map::new();
                let mut length = 0usize;
                loop {
                    let mut h = String::new();
                    read.read_line(&mut h).await.unwrap();
                    let h = h.trim_end();
                    if h.is_empty() {
                        break;
                    }
                    let (k, v) = h.split_once(':').unwrap();
                    let (k, v) = (k.trim().to_lowercase(), v.trim().to_string());
                    if k == "content-length" {
                        length = v.parse().unwrap();
                    }
                    if HEADERS.contains(&k.as_str()) {
                        headers.insert(k, json!(v.replace(&key, "[key]")));
                    }
                }
                let mut body = vec![0u8; length];
                read.read_exact(&mut body).await.unwrap();
                requests
                    .lock()
                    .unwrap()
                    .push(json!({"method": method, "path": path, "headers": headers,
                    "body": String::from_utf8(body).unwrap()}));
                let item = queue.lock().unwrap().remove(0);
                if item.get("transport_error").is_some() {
                    return;
                }
                if item.get("hang_request").is_some() {
                    std::future::pending::<()>().await;
                }
                if let Some(raw) = item.get("raw").and_then(Value::as_str) {
                    let head = format!(
                        "HTTP/1.1 {} X\r\ncontent-length: {}\r\n\r\n",
                        item["status"],
                        raw.len()
                    );
                    write.write_all(head.as_bytes()).await.unwrap();
                    write.write_all(raw.as_bytes()).await.unwrap();
                    continue;
                }
                if item.get("sse").is_some() || item.get("sse_raw").is_some() {
                    write.write_all(b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ntransfer-encoding: chunked\r\n\r\n").await.unwrap();
                    for c in sse_chunks(&item) {
                        write
                            .write_all(format!("{:x}\r\n", c.len()).as_bytes())
                            .await
                            .unwrap();
                        write.write_all(&c).await.unwrap();
                        write.write_all(b"\r\n").await.unwrap();
                        write.flush().await.unwrap();
                    }
                    if item.get("broken").is_some() {
                        return;
                    }
                    if item.get("hang").is_some() {
                        std::future::pending::<()>().await;
                    }
                    write.write_all(b"0\r\n\r\n").await.unwrap();
                    continue;
                }
                let status = item.get("status").and_then(Value::as_u64).unwrap_or(200);
                let text = ferb_py::json::dumps(&item["json"]);
                let mut head = format!("HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\n", text.len());
                if let Some(Value::Object(h)) = item.get("headers") {
                    for (k, v) in h {
                        head.push_str(&format!("{k}: {}\r\n", v.as_str().unwrap()));
                    }
                }
                head.push_str("\r\n");
                write.write_all(head.as_bytes()).await.unwrap();
                write.write_all(text.as_bytes()).await.unwrap();
            }
        });
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

fn strings(v: &Value) -> Vec<String> {
    v.as_array()
        .map(|a| a.iter().map(|s| s.as_str().unwrap().to_string()).collect())
        .unwrap_or_default()
}

async fn play(case: &Value, fx: &Value) -> Value {
    let sc = &case["scenario"];
    let key = fx["key"].as_str().unwrap().to_string();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
    let requests = Arc::new(Mutex::new(Vec::new()));
    let srv = tokio::spawn(server(
        listener,
        sc["responses"].as_array().unwrap().clone(),
        requests.clone(),
        key.clone(),
    ));
    let model = sc["model"].as_str();
    let provider: Arc<dyn Provider> = match sc["provider"].as_str().unwrap() {
        "anthropic" => Arc::new(anthropic::AnthropicProvider::new(model)),
        "openai" => Arc::new(openai::OpenAIProvider::new(model)),
        _ => Arc::new(google::GoogleProvider::new(model)),
    };
    let log = Arc::new(Mutex::new(Vec::new()));
    let bridge = Arc::new(FakeBridge {
        tools: sc["bridge"]["tools"].as_array().unwrap().clone(),
        results: Mutex::new(sc["bridge"]["results"].as_array().unwrap().clone()),
        log: log.clone(),
    });
    let the_key = match sc.get("key") {
        Some(Value::Null) => None,
        _ => Some(key.clone()),
    };
    let refusals = Arc::new(Refusals::new(Box::new(|| 0.0)));
    let schema = (!sc["schema"].is_null()).then(|| sc["schema"].clone());
    let mut rt = ApiRuntime::new(
        provider,
        sc["mode"] == "supervisor",
        sc["role"].as_str().unwrap(),
        &strings(&sc["tool_names"]),
        schema,
        Some(bridge),
        Arc::new(move |_name: String| {
            let k = the_key.clone();
            Box::pin(async move { k.map(|k| Secret::from(k.as_str())) })
        }),
        Arc::new(RecordedForms(fx["forms"].clone())),
        refusals.clone(),
    )
    .expect("a runtime");
    rt.monotonic = Arc::new(|| 0.0);
    let sleeps = log.clone();
    rt.sleep = Arc::new(move |s: f64| {
        sleeps.lock().unwrap().push(json!(["sleep", s]));
        Box::pin(async {})
    });
    rt.effort = sc["effort"].as_str().map(str::to_string);
    rt.limit_s = sc["limit_s"].as_f64();
    rt.planner = sc["planner"] == true;
    rt.origin = Some(origin);
    rt.token_hex = counter();
    let today = fx["today"].as_str().unwrap().to_string();
    rt.today = Arc::new(move || today.clone());
    if sc["refused"] == true {
        refusals.remember(&rt.format_key);
    }
    let request = request_of(&sc["request"]);
    let token = sc["run_token"].as_str().unwrap().to_string();
    let task = task();
    let mut events = Vec::new();
    let mut started = None;
    for op in sc["ops"].as_array().unwrap() {
        match op["op"].as_str().unwrap() {
            "start" => match rt.start(&task, &request, &token).await {
                Ok(s) => {
                    events.push(json!({"op": "start"}));
                    started = Some(s);
                }
                Err(e) => events.push(json!({"op": "start", "raised": raised(&e)})),
            },
            "events" => {
                let s = started.as_mut().unwrap();
                let mut count = 0;
                while op
                    .get("n")
                    .and_then(Value::as_u64)
                    .is_none_or(|n| count < n)
                {
                    match tokio::time::timeout(Duration::from_secs(5), s.events.next())
                        .await
                        .expect("an event in 5 s")
                    {
                        None => {
                            events.push(json!({"op": "end"}));
                            break;
                        }
                        Some(Err(e)) => {
                            events.push(json!({"op": "raised", "raised": raised(&e)}));
                            break;
                        }
                        Some(Ok(ev)) => {
                            events.push(json!({"op": "event", "event": event_text(&ev)}));
                            count += 1;
                        }
                    }
                }
            }
            "steer" => {
                let out = started
                    .as_ref()
                    .unwrap()
                    .control
                    .steer(op["text"].as_str().unwrap())
                    .await;
                events.push(match out {
                    Ok(()) => json!({"op": "steer", "ok": true}),
                    Err(e) => json!({"op": "steer", "raised": raised(&e)}),
                });
            }
            "cancel" => {
                started.as_ref().unwrap().control.cancel().await;
                events.push(json!({"op": "cancel"}));
            }
            other => panic!("unknown op {other}"),
        }
    }
    if let Some(s) = started {
        s.events.close().await;
    }
    srv.abort();
    let retried = rt.retried.lock().unwrap().clone();
    let remembered = if rt.format_key.is_empty() {
        Value::Null
    } else {
        json!(refusals.refused(&rt.format_key))
    };
    let reqs = requests.lock().unwrap().clone();
    let calls = log.lock().unwrap().clone();
    json!({"name": sc["name"], "requests": reqs, "log": events, "calls": calls, "retried": retried,
        "remembered": remembered})
}

// decision: RUN-1, RUN-415, RUN-418, RUN-419, RUN-423, RUN-447, RUN-430, RUN-436, RUN-437, RUN-438, RUN-445, RUN-446, RUN-447, RUN-448
#[tokio::test(flavor = "current_thread")]
async fn api_matches_python() {
    let fx = fixture("api.json");
    let cases = fx["cases"].as_array().unwrap();
    let mut failures = Vec::new();
    for case in cases {
        let got = play(case, &fx).await;
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

// decision: RUN-468
#[tokio::test]
async fn a_worker_loops_earlier_looks_at_a_window_are_compacted_in_its_own_requests() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
    let requests = Arc::new(Mutex::new(Vec::new()));
    let call = |i: usize| {
        json!({"json": {"status": "completed", "output": [{"type": "function_call", "id": format!("fc_{i}"),
            "call_id": format!("call_{i}"), "name": "mac_snapshot", "arguments": "{\"app\": \"pid:1\"}"}],
            "usage": {"input_tokens": 10, "output_tokens": 2}}})
    };
    let mut responses: Vec<Value> = (1..=4).map(call).collect();
    responses.push(json!({"json": {"status": "completed", "output": [{"type": "message", "content": [
        {"type": "output_text", "text": "done"}]}], "usage": {"input_tokens": 10, "output_tokens": 2}}}));
    let srv = tokio::spawn(server(listener, responses, requests.clone(), "k".into()));
    let look = |i: usize| {
        json!({"content": {"app": "X", "bundle_id": "com.x", "window": "Inbox", "elements": ["e".repeat(3000)],
            "n": i, "windows": [{"id": 7, "current": true}]}, "footer": "[mac]", "is_error": false, "image": null})
    };
    let bridge = Arc::new(FakeBridge {
        tools: vec![
            json!({"name": "mac.snapshot", "description": "Look.", "input_schema": {"type": "object"}}),
        ],
        results: Mutex::new((1..=4).map(look).collect()),
        log: Arc::new(Mutex::new(Vec::new())),
    });
    let mut rt = ApiRuntime::new(
        Arc::new(openai::OpenAIProvider::new(None)),
        false,
        "worker",
        &["mac.snapshot".to_string()],
        None,
        Some(bridge),
        Arc::new(|_name: String| Box::pin(async { Some(Secret::from("k")) })),
        Arc::new(ferb_runtimes::forms::ContextForms),
        Arc::new(Refusals::new(Box::new(|| 0.0))),
    )
    .expect("a runtime");
    rt.monotonic = Arc::new(|| 0.0);
    rt.origin = Some(origin);
    rt.token_hex = counter();
    rt.compaction = Some(ferb_runtimes::compact::Policy {
        start_tokens: 0,
        gain_tokens: 1,
        keep_steps: 1,
        read_chars: 100_000,
        read_age: 100,
    });
    rt.clock = Arc::new(|| "10:00:00".to_string());
    let request =
        request_of(&json!({"system": "FERB", "parts": ["the owner's words"], "cacheable": 1}));
    let mut started = rt.start(&task(), &request, "tok").await.expect("started");
    let mut kinds = Vec::new();
    while let Some(ev) = tokio::time::timeout(Duration::from_secs(5), started.events.next())
        .await
        .expect("no stall")
    {
        let ev = ev.expect("an event");
        if ev.kind == ferb_core::types::RuntimeEventKind::Compacted {
            assert_eq!(ev.data["by"], json!("ferb"));
            kinds.push(format!("compacted {}", ev.data["screens_replaced"]));
        }
    }
    let bodies: Vec<Value> = requests
        .lock()
        .unwrap()
        .iter()
        .map(|r| serde_json::from_str(r["body"].as_str().unwrap()).unwrap())
        .collect();
    assert_eq!(bodies.len(), 5);
    let outputs = |b: &Value| -> Vec<String> {
        b["input"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|i| i["type"] == "function_call_output")
            .map(|i| i["output"].as_str().unwrap().chars().take(40).collect())
            .collect()
    };
    let line = "FERB: an earlier snapshot of X (window 7";
    // request 3 is the first with a superseded look a call old; each later one adds the look before the newest
    assert_eq!(kinds, ["compacted 1", "compacted 1", "compacted 1"]);
    let last = outputs(&bodies[4]);
    assert_eq!(last.len(), 4);
    assert!(last[..3].iter().all(|o| o == line), "{last:?}");
    assert!(
        last[3].starts_with("{\"app\": \"X\""),
        "the newest look stays whole: {last:?}"
    );
    assert_eq!(
        bodies[4]["input"][0], bodies[0]["input"][0],
        "the owner's words are as sent"
    );
    srv.abort();
}

// ---- RUN-480 (kb/plans/front-door.md, step 4): a plain-mode run (a supervisor run with no answer form) streams its
// text and reports each tool call just before it runs and once its result is known. Today's answer-form runs are the
// recorded fixtures (`api_matches_python`, "anthropic: a supervisor's typed answer streams"), unchanged.

fn sse(events: Vec<Value>) -> Value {
    json!({ "sse": events })
}

fn a_text(index: u64, pieces: &[&str]) -> Vec<Value> {
    let mut out = vec![
        json!({"type": "content_block_start", "index": index, "content_block": {"type": "text", "text": ""}}),
    ];
    for p in pieces {
        out.push(json!({"type": "content_block_delta", "index": index,
            "delta": {"type": "text_delta", "text": p}}));
    }
    out.push(json!({"type": "content_block_stop", "index": index}));
    out
}

fn a_reply(blocks: Vec<Value>, stop: &str) -> Value {
    let mut events = vec![json!({"type": "message_start",
        "message": {"role": "assistant", "content": [], "usage": {"input_tokens": 7}}})];
    events.extend(blocks);
    events.push(
        json!({"type": "message_delta", "delta": {"stop_reason": stop}, "usage": {"output_tokens": 3}}),
    );
    events.push(json!({"type": "message_stop"}));
    sse(events)
}

fn a_call(index: u64, id: &str, name: &str, args: &str) -> Vec<Value> {
    vec![
        json!({"type": "content_block_start", "index": index,
            "content_block": {"type": "tool_use", "id": id, "name": name, "input": {}}}),
        json!({"type": "content_block_delta", "index": index,
            "delta": {"type": "input_json_delta", "partial_json": args}}),
        json!({"type": "content_block_stop", "index": index}),
    ]
}

struct PlainRun {
    started: ferb_runtimes::run::Started,
    requests: Arc<Mutex<Vec<Value>>>,
    calls: Arc<Mutex<Vec<Value>>>,
    srv: tokio::task::JoinHandle<()>,
}

async fn plain_start(
    provider: Arc<dyn Provider>,
    responses: Vec<Value>,
    results: Vec<Value>,
) -> PlainRun {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
    let requests = Arc::new(Mutex::new(Vec::new()));
    let srv = tokio::spawn(server(listener, responses, requests.clone(), "k".into()));
    let calls = Arc::new(Mutex::new(Vec::new()));
    let bridge = Arc::new(FakeBridge {
        tools: vec![json!({"name": "memory.read", "description": "Read memory.",
            "input_schema": {"type": "object"}})],
        results: Mutex::new(results),
        log: calls.clone(),
    });
    let mut rt = ApiRuntime::new(
        provider,
        true,
        "orchestrator",
        &["memory.read".to_string()],
        None,
        Some(bridge),
        Arc::new(|_name: String| Box::pin(async { Some(Secret::from("k")) })),
        Arc::new(ferb_runtimes::forms::ContextForms),
        Arc::new(Refusals::new(Box::new(|| 0.0))),
    )
    .expect("a runtime");
    rt.monotonic = Arc::new(|| 0.0);
    rt.origin = Some(origin);
    rt.token_hex = counter();
    rt.today = Arc::new(|| "2026-11-26".into());
    let request =
        request_of(&json!({"system": "FERB", "parts": ["ok no worries"], "cacheable": 0}));
    let started = rt.start(&task(), &request, "tok").await.expect("started");
    PlainRun {
        started,
        requests,
        calls,
        srv,
    }
}

/// The run's text, tool, usage, final and failure events until it ends (or `until` matched one).
async fn plain_told(
    started: &mut ferb_runtimes::run::Started,
    until: Option<&dyn Fn(&Value) -> bool>,
) -> Vec<Value> {
    use ferb_core::types::RuntimeEventKind as K;
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

fn plain_delta(t: &str) -> Value {
    json!({"kind": "text_delta", "data": {"text": t}})
}

// decision: RUN-480
#[tokio::test]
async fn a_plain_reply_streams_its_pieces_and_ends_with_them_as_its_answer_on_every_provider() {
    let anthropic = a_reply(a_text(0, &["Sure, ", "no worries."]), "end_turn");
    let openai = sse(vec![
        json!({"type": "response.output_text.delta", "delta": "Sure, "}),
        json!({"type": "response.output_text.delta", "delta": "no worries."}),
        json!({"type": "response.completed", "response": {"status": "completed", "usage": {"input_tokens": 7},
            "output": [{"type": "message", "content": [{"type": "output_text", "text": "Sure, no worries."}]}]}}),
    ]);
    let google = sse(vec![
        json!({"candidates": [{"index": 0, "content": {"role": "model", "parts": [{"text": "Sure, "}]}}]}),
        json!({"candidates": [{"index": 0, "content": {"role": "model", "parts": [{"text": "no worries."}]},
            "finishReason": "STOP"}], "usageMetadata": {"promptTokenCount": 7}}),
    ]);
    let providers: Vec<(Arc<dyn Provider>, Value)> = vec![
        (Arc::new(anthropic::AnthropicProvider::new(None)), anthropic),
        (Arc::new(openai::OpenAIProvider::new(None)), openai),
        (Arc::new(google::GoogleProvider::new(None)), google),
    ];
    for (provider, reply) in providers {
        let name = provider.name().to_string();
        let mut run = plain_start(provider, vec![reply], vec![]).await;
        let got = plain_told(&mut run.started, None).await;
        assert_eq!(
            got,
            vec![
                plain_delta("Sure, "),
                plain_delta("no worries."),
                json!({"kind": "usage", "data": {"input_tokens": 7}}),
                json!({"kind": "final", "data": {"text": "Sure, no worries."}}),
            ],
            "{name}"
        );
        run.srv.abort();
    }
}

// decision: RUN-480
#[tokio::test]
async fn a_line_then_a_call_then_the_answer_tell_the_call_started_and_completed_in_order() {
    let mut first = a_text(0, &["Let me ", "look."]);
    first.extend(a_call(1, "tu1", "memory_read", "{\"query\": \"x\"}"));
    let responses = vec![
        a_reply(first, "tool_use"),
        a_reply(a_text(0, &["It is ", "x."]), "end_turn"),
    ];
    let mut run = plain_start(
        Arc::new(anthropic::AnthropicProvider::new(None)),
        responses,
        vec![],
    )
    .await;
    let got = plain_told(&mut run.started, None).await;
    assert_eq!(
        got,
        vec![
            plain_delta("Let me "),
            plain_delta("look."),
            json!({"kind": "usage", "data": {"input_tokens": 7}}),
            json!({"kind": "tool_call", "data": {"tool": "memory_read", "name": "memory.read",
                "phase": "started", "arguments": {"query": "x"}}}),
            json!({"kind": "tool_call", "data": {"tool": "memory_read", "name": "memory.read",
                "phase": "completed", "arguments": {"query": "x"}, "error": false}}),
            plain_delta("It is "),
            plain_delta("x."),
            json!({"kind": "usage", "data": {"input_tokens": 7}}),
            json!({"kind": "final", "data": {"text": "It is x."}}),
        ]
    );
    assert_eq!(run.calls.lock().unwrap().len(), 1);
    run.srv.abort();
}

// decision: RUN-480, RUN-445
#[tokio::test]
async fn a_call_whose_result_is_an_error_or_that_was_never_offered_completes_with_its_error() {
    let mut first = a_call(0, "tu1", "memory_read", "{}");
    first.extend(a_call(1, "tu2", "made_up", "{}"));
    let responses = vec![
        a_reply(first, "tool_use"),
        a_reply(a_text(0, &["Nothing."]), "end_turn"),
    ];
    let refused = vec![
        json!({"content": {"error": "no such entry"}, "footer": "", "is_error": true, "image": null}),
    ];
    let mut run = plain_start(
        Arc::new(anthropic::AnthropicProvider::new(None)),
        responses,
        refused,
    )
    .await;
    let got = plain_told(&mut run.started, None).await;
    let tools: Vec<Value> = got
        .iter()
        .filter(|v| v["kind"] == "tool_call")
        .cloned()
        .collect();
    assert_eq!(
        tools,
        vec![
            json!({"kind": "tool_call", "data": {"tool": "memory_read", "name": "memory.read",
                "phase": "started", "arguments": {}}}),
            json!({"kind": "tool_call", "data": {"tool": "memory_read", "name": "memory.read",
                "phase": "completed", "arguments": {}, "error": true}}),
            // a name the run was never offered: counted once as a wrong call, nothing runs
            json!({"kind": "tool_call", "data": {"tool": "made_up", "phase": "started", "arguments": {},
                "offered": false}}),
            json!({"kind": "tool_call", "data": {"tool": "made_up", "phase": "completed", "arguments": {},
                "error": true}}),
        ]
    );
    assert_eq!(got.last().unwrap()["data"]["text"], "Nothing.");
    run.srv.abort();
}

// decision: RUN-480
#[tokio::test]
async fn a_cancel_right_after_a_completed_call_asks_the_model_nothing_more_and_raises_nothing() {
    let responses = vec![
        a_reply(a_call(0, "tu1", "memory_read", "{}"), "tool_use"),
        a_reply(a_text(0, &["never asked"]), "end_turn"),
    ];
    let mut run = plain_start(
        Arc::new(anthropic::AnthropicProvider::new(None)),
        responses,
        vec![],
    )
    .await;
    let completed = |v: &Value| v["data"]["phase"] == "completed";
    let got = plain_told(&mut run.started, Some(&completed)).await;
    // the call's usage was told before the call ran: nothing is left unreported
    assert_eq!(
        got[0],
        json!({"kind": "usage", "data": {"input_tokens": 7}}),
        "{got:?}"
    );
    assert_eq!(got.last().unwrap()["data"]["phase"], "completed");
    run.started.control.cancel().await;
    let after = tokio::time::timeout(Duration::from_secs(5), run.started.events.next())
        .await
        .expect("the stream ends");
    assert!(after.is_none(), "{after:?}");
    assert!(run.started.control.unreported_usage().is_empty());
    assert_eq!(
        run.requests.lock().unwrap().len(),
        1,
        "a second model request went out"
    );
    run.srv.abort();
}
