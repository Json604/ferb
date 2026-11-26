//! ORCH-719 step 1 in FERB's Claude relay, against a stand-in upstream on 127.0.0.1 (no provider is reached):
//! RUN-477, each model request's usage read from the answer as it streams past, per run, so a run Claude Code never
//! reports still has its tokens; RUN-478, Anthropic's refusal of an edited history (the history-editing check's 400,
//! in the shape the pinned Claude Code's bundled migration guide gives) answered by sending the run's own unedited
//! request and never editing that run again. T-1338: each model step's record of names, flags and times (the tools
//! its answer called, its request's tool results by name and error flag, the stop reason), timed on a stepping clock.
//! Invented bodies only.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use ferb_runtimes::claude::proxy::ToolListProxy;
use ferb_runtimes::claude::steps::{self, Steps, KEPT};
use ferb_runtimes::claude::tally::{history_refused, Tally, UsageReader};
use ferb_runtimes::claude::ToolProxy;
use ferb_runtimes::compact::{self, Policy};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;

/// The history-editing check's refusal, as the guide prints its message (the beta sentence included).
const REFUSAL: &str = "{\"type\":\"error\",\"error\":{\"type\":\"invalid_request_error\",\"message\":\"messages.3.content.0: \
Invalid `signature` in `thinking` block. The block is bound to a different conversation. Remove the block, or set \
`thinking.block_binding.prefix_mismatch_behavior` to \\\"drop_block\\\". That setting requires the \
`thinking-binding-controls-2026-08-01` value in the `anthropic-beta` header.\"},\"request_id\":\"req_invented\"}";

/// A tampered signature: the same first clause, no binding sentence. Not the history-editing check.
const TAMPERED: &str = "{\"type\":\"error\",\"error\":{\"type\":\"invalid_request_error\",\"message\":\"messages.3.content.0: \
Invalid `signature` in `thinking` block\"}}";

const EAGER: Policy = Policy {
    start_tokens: 0,
    gain_tokens: 1,
    keep_steps: 1,
    read_chars: 2_000,
    read_age: 2,
};

fn clock() -> compact::Clock {
    Arc::new(|| "10:00:00".to_string())
}

/// A streamed answer with its usage in `message_start` and the output in `message_delta`, split mid-line.
fn sse(input: i64, read: i64, w5: i64, w1: i64, output: i64) -> Vec<String> {
    let start = json!({"type": "message_start", "message": {"id": "msg", "model": "claude-opus-5-5",
        "usage": {"input_tokens": input, "cache_read_input_tokens": read,
            "cache_creation_input_tokens": w5 + w1,
            "cache_creation": {"ephemeral_5m_input_tokens": w5, "ephemeral_1h_input_tokens": w1},
            "output_tokens": 1}}});
    let delta = json!({"type": "message_delta", "delta": {"stop_reason": "end_turn"},
        "usage": {"output_tokens": output}});
    let text = format!(
        "event: message_start\ndata: {start}\n\nevent: content_block_delta\ndata: {{\"type\":\"content_block_delta\",\"index\":0,\"delta\":{{\"type\":\"text_delta\",\"text\":\"hi\"}}}}\n\nevent: message_delta\ndata: {delta}\n\nevent: message_stop\ndata: {{\"type\":\"message_stop\"}}\n\n"
    );
    let cut = text.len() / 2;
    vec![text[..cut].to_string(), text[cut..].to_string()]
}

/// What the stand-in answers one request with: (status, content type, body pieces).
type Answer = (u16, &'static str, Vec<String>);
/// (path, body) of each request the stand-in got.
type Got = Arc<Mutex<Vec<(String, Vec<u8>)>>>;
/// How the stand-in answers a request: from its path and body.
type Answering = Arc<dyn Fn(&str, &[u8]) -> Answer + Send + Sync>;

/// A stand-in upstream: each request is answered by `answer(path, body)`, chunked.
async fn upstream(listener: TcpListener, seen: Got, answer: Answering) {
    loop {
        let Ok((stream, _)) = listener.accept().await else {
            return;
        };
        let (seen, answer) = (seen.clone(), answer.clone());
        tokio::spawn(async move {
            let (read, mut write) = stream.into_split();
            let mut read = BufReader::new(read);
            loop {
                let mut line = String::new();
                if read.read_line(&mut line).await.unwrap_or(0) == 0 {
                    return;
                }
                let path = line
                    .split_whitespace()
                    .nth(1)
                    .unwrap_or_default()
                    .to_string();
                let mut length = 0usize;
                loop {
                    let mut h = String::new();
                    read.read_line(&mut h).await.unwrap();
                    let h = h.trim_end();
                    if h.is_empty() {
                        break;
                    }
                    let (k, v) = h.split_once(':').unwrap();
                    if k.trim().eq_ignore_ascii_case("content-length") {
                        length = v.trim().parse().unwrap();
                    }
                }
                let mut body = vec![0u8; length];
                read.read_exact(&mut body).await.unwrap();
                seen.lock().unwrap().push((path.clone(), body.clone()));
                let (status, kind, pieces) = answer(&path, &body);
                let mut out = format!(
                    "HTTP/1.1 {status} X\r\ncontent-type: {kind}\r\ntransfer-encoding: chunked\r\n\r\n"
                );
                for p in pieces {
                    out.push_str(&format!("{:x}\r\n{p}\r\n", p.len()));
                }
                out.push_str("0\r\n\r\n");
                write.write_all(out.as_bytes()).await.unwrap();
            }
        });
    }
}

async fn start(
    answer: impl Fn(&str, &[u8]) -> Answer + Send + Sync + 'static,
) -> (ToolListProxy, Got, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let up = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
    let got: Got = Arc::new(Mutex::new(Vec::new()));
    let server = tokio::spawn(upstream(listener, got.clone(), Arc::new(answer)));
    // a stopwatch that moves 0.25 s at each reading: a step reads it at arrival, first byte and end
    let ticks = Arc::new(AtomicUsize::new(0));
    let stopwatch = Arc::new(move || ticks.fetch_add(1, Ordering::SeqCst) as f64 * 0.25);
    let relay =
        ToolListProxy::start_with_clocks(&up, Arc::new(|| "s".to_string()), clock(), stopwatch)
            .await
            .unwrap();
    (relay, got, server)
}

/// Posts through the relay: (status, the answer's bytes).
async fn post(url: &str, body: &[u8]) -> (u16, Vec<u8>) {
    let resp = reqwest::Client::new()
        .post(url)
        .body(body.to_vec())
        .send()
        .await
        .unwrap();
    let status = resp.status().as_u16();
    (status, resp.bytes().await.unwrap().to_vec())
}

// decision: RUN-477
#[tokio::test]
async fn the_relay_counts_each_model_requests_usage_per_run_from_the_answers() {
    let (relay, _got, server) = start(|path, body| {
        if path.contains("count_tokens") {
            (200, "application/json", vec!["{\"input_tokens\": 999}".into()])
        } else if !String::from_utf8_lossy(body).contains("\"stream\":true") {
            (
                200,
                "application/json",
                vec![json!({"id": "m", "model": "claude-sonnet-5-5", "usage": {"input_tokens": 7,
                    "cache_read_input_tokens": 0, "cache_creation_input_tokens": 0, "output_tokens": 2}})
                .to_string()],
            )
        } else {
            (200, "text/event-stream", sse(5, 1000, 20, 300, 40))
        }
    })
    .await;
    let body = serde_json::to_vec(&json!({"model": "m", "messages": [], "stream": true})).unwrap();
    let a = relay.base_url("run-a");
    let b = relay.base_url("run-b");
    for _ in 0..3 {
        let (status, answer) = post(&format!("{a}/v1/messages?beta=true"), &body).await;
        assert_eq!(status, 200);
        assert!(
            String::from_utf8_lossy(&answer).contains("message_stop"),
            "the answer goes through whole"
        );
    }
    // a token count is no model request
    post(&format!("{a}/v1/messages/count_tokens"), &body).await;
    post(&format!("{b}/v1/messages"), &body).await;
    assert_eq!(
        relay.take_usage("run-a"),
        Some(Tally {
            requests: 3,
            input: 15,
            cache_read: 3000,
            cache_write: 960,
            cache_write_5m: 60,
            cache_write_1h: 900,
            output: 120,
            model: "claude-opus-5-5".into(),
        })
    );
    assert_eq!(relay.take_usage("run-a"), None, "taken once");
    assert_eq!(relay.take_usage("run-b").map(|t| t.requests), Some(1));
    // an answer that isn't streamed is read whole for its usage
    let c = relay.base_url("run-c");
    let plain = serde_json::to_vec(&json!({"model": "m", "messages": []})).unwrap();
    post(&format!("{c}/v1/messages"), &plain).await;
    let t = relay.take_usage("run-c").unwrap();
    assert_eq!(
        (t.requests, t.input, t.output, t.model.as_str()),
        (1, 7, 2, "claude-sonnet-5-5")
    );
    post(&format!("{c}/v1/messages"), &plain).await;
    relay.forget("run-c");
    assert_eq!(
        relay.take_usage("run-c"),
        None,
        "a forgotten run keeps nothing"
    );
    server.abort();
}

// decision: RUN-477
#[test]
fn a_json_answer_and_a_stream_cut_anywhere_give_the_same_usage() {
    let body = json!({"id": "m", "model": "claude-sonnet-5-5", "usage": {"input_tokens": 7,
        "cache_read_input_tokens": 11, "cache_creation_input_tokens": 3,
        "cache_creation": {"ephemeral_5m_input_tokens": 3, "ephemeral_1h_input_tokens": 0}, "output_tokens": 2}})
    .to_string();
    let mut r = UsageReader::new(false);
    for piece in body.as_bytes().chunks(5) {
        r.feed(piece);
    }
    let t = r.finish().unwrap();
    assert_eq!(
        (t.requests, t.input, t.cache_read, t.cache_write, t.output),
        (1, 7, 11, 3, 2)
    );
    let text: String = sse(5, 1000, 20, 300, 40).concat();
    for size in [1, 7, 64, 4096] {
        let mut r = UsageReader::new(true);
        for piece in text.as_bytes().chunks(size) {
            r.feed(piece);
        }
        let t = r.finish().unwrap();
        assert_eq!(
            (t.input, t.cache_read, t.cache_write_1h, t.output),
            (5, 1000, 300, 40),
            "{size}"
        );
    }
    // an answer without usage (an error) counts no request
    let mut r = UsageReader::new(false);
    r.feed(REFUSAL.as_bytes());
    assert_eq!(r.finish(), None);
    let anthropic = Tally {
        requests: 2,
        input: 5,
        cache_read: 10,
        cache_write: 4,
        cache_write_5m: 1,
        cache_write_1h: 3,
        output: 6,
        model: String::new(),
    }
    .as_anthropic();
    let u =
        ferb_runtimes::usage::from_anthropic("m", &anthropic, "subscription", None, "2026-11-25")
            .unwrap();
    assert_eq!(
        (
            u.input_tokens,
            u.cached_input_tokens,
            u.cache_write_tokens,
            u.cache_write_1h_tokens
        ),
        (19, 10, 4, 3)
    );
}

// decision: RUN-478
#[test]
fn the_history_editing_refusal_is_recognised_by_its_shape_only() {
    assert!(history_refused(400, REFUSAL.as_bytes()));
    // the guide's message without the beta sentence (a request that sent the header) and with the diagnostic
    // sentence naming the first changed message
    let short = REFUSAL.replace(
        " Remove the block, or set `thinking.block_binding.prefix_mismatch_behavior` to \\\"drop_block\\\". That setting requires the `thinking-binding-controls-2026-08-01` value in the `anthropic-beta` header.",
        " The first message that changed is messages.1.",
    );
    assert!(history_refused(400, short.as_bytes()));
    assert!(
        !history_refused(400, TAMPERED.as_bytes()),
        "a tampered signature is another failure"
    );
    assert!(!history_refused(429, REFUSAL.as_bytes()));
    assert!(!history_refused(400, b"not json"));
    assert!(!history_refused(
        400,
        br#"{"type":"error","error":{"type":"overloaded_error","message":"bound to a different conversation `thinking` block"}}"#
    ));
}

/// A Mac window's snapshot as the tool returns it (crate::compact finds the window in it).
fn snapshot(size: usize) -> String {
    let content = json!({"app": "X", "bundle_id": "com.x", "window": "W", "elements": ["e".repeat(size)],
        "offset": 0, "next_offset": null, "windows": [{"id": 7, "title": "", "current": true}]});
    format!("{}\n[mac]", serde_json::to_string(&content).unwrap())
}

fn tool_call(i: usize, size: usize) -> Vec<Value> {
    vec![
        json!({"role": "assistant", "content": [
            {"type": "thinking", "thinking": "", "signature": format!("sig{i}")},
            {"type": "tool_use", "id": format!("c{i}"), "name": "mcp__ferb__mac_snapshot", "input": {}}]}),
        json!({"role": "user", "content": [{"type": "tool_result", "tool_use_id": format!("c{i}"),
            "content": snapshot(size)}]}),
    ]
}

/// Claude Code's request with `n` calls on the same window, as compaction finds earlier screenshots in it.
fn history(n: usize) -> Vec<u8> {
    let mut messages =
        vec![json!({"role": "user", "content": [{"type": "text", "text": "the owner's words"}]})];
    for i in 0..n {
        messages.extend(tool_call(i, 3000));
    }
    serde_json::to_vec(&json!({"model": "claude-opus-5-5", "messages": messages, "stream": true}))
        .unwrap()
}

fn edited(body: &[u8]) -> bool {
    String::from_utf8_lossy(body).contains("FERB: an earlier")
}

// decision: RUN-478
#[tokio::test]
async fn a_refused_edit_goes_again_unedited_and_the_run_is_never_edited_again() {
    // an enforced account: any edited history is refused; the run's own requests are answered
    let (relay, got, server) = start(|_, body| {
        if edited(body) {
            (400, "application/json", vec![REFUSAL.to_string()])
        } else {
            (200, "text/event-stream", sse(5, 100, 0, 0, 3))
        }
    })
    .await;
    let url = relay.base_url("run-r");
    relay.compact("run-r", EAGER);
    for n in [1, 2, 3, 4] {
        let (status, answer) = post(&format!("{url}/v1/messages?beta=true"), &history(n)).await;
        assert_eq!(
            status,
            200,
            "request {n}: {}",
            String::from_utf8_lossy(&answer)
        );
        assert!(
            String::from_utf8_lossy(&answer).contains("message_stop"),
            "Claude Code gets the answer to its own request"
        );
    }
    let got = got.lock().unwrap().clone();
    let bodies: Vec<&Vec<u8>> = got.iter().map(|(_, b)| b).collect();
    // the first edit (request 2) was refused and went again as Claude Code sent it; nothing after was edited
    let edits: Vec<bool> = bodies.iter().map(|b| edited(b)).collect();
    assert_eq!(edits, [false, true, false, false, false], "{edits:?}");
    assert_eq!(
        *bodies[2],
        history(2),
        "the unedited request, byte for byte"
    );
    assert_eq!(*bodies[3], history(3));
    assert_eq!(relay.history_refusals("run-r"), 1);
    assert_eq!(relay.history_refusals("run-r"), 0, "reported once");
    // the refused request counted nothing; the four answered ones did
    assert_eq!(relay.take_usage("run-r").map(|t| t.requests), Some(4));
    server.abort();
}

// decision: RUN-478
#[tokio::test]
async fn any_other_refusal_of_an_edited_request_goes_back_as_it_came_and_compaction_stays() {
    let (relay, got, server) = start(|_, body| {
        if edited(body) {
            (400, "application/json", vec![TAMPERED.to_string()])
        } else {
            (200, "text/event-stream", sse(5, 100, 0, 0, 3))
        }
    })
    .await;
    let url = relay.base_url("run-t");
    relay.compact("run-t", EAGER);
    post(&format!("{url}/v1/messages"), &history(1)).await;
    let (status, answer) = post(&format!("{url}/v1/messages"), &history(2)).await;
    assert_eq!(status, 400);
    assert_eq!(String::from_utf8_lossy(&answer), TAMPERED);
    let (status, _) = post(&format!("{url}/v1/messages"), &history(3)).await;
    assert_eq!(status, 400, "still edited: compaction stays on");
    assert_eq!(got.lock().unwrap().len(), 3, "nothing sent twice");
    assert_eq!(relay.history_refusals("run-t"), 0);
    server.abort();
}

/// A streamed answer that calls `tools` (each with invented arguments) and stops for `stop`, cut into small pieces.
fn sse_calls(tools: &[&str], stop: &str) -> Vec<String> {
    let mut text = format!(
        "event: message_start\ndata: {}\n\n",
        json!({"type": "message_start", "message": {"id": "msg", "model": "claude-sonnet-5-5",
            "usage": {"input_tokens": 3, "cache_read_input_tokens": 0, "cache_creation_input_tokens": 0,
                "output_tokens": 1}}})
    );
    for (i, name) in tools.iter().enumerate() {
        let start = json!({"type": "content_block_start", "index": i,
            "content_block": {"type": "tool_use", "id": format!("toolu_{i}"), "name": name, "input": {}}});
        let delta = json!({"type": "content_block_delta", "index": i,
            "delta": {"type": "input_json_delta", "partial_json": "{\"reply\": \"private words\"}"}});
        text.push_str(&format!(
            "event: content_block_start\ndata: {start}\n\nevent: content_block_delta\ndata: {delta}\n\nevent: content_block_stop\ndata: {{\"type\":\"content_block_stop\",\"index\":{i}}}\n\n"
        ));
    }
    let end = json!({"type": "message_delta", "delta": {"stop_reason": stop}, "usage": {"output_tokens": 9}});
    text.push_str(&format!(
        "event: message_delta\ndata: {end}\n\nevent: message_stop\ndata: {{\"type\":\"message_stop\"}}\n\n"
    ));
    text.as_bytes()
        .chunks(7)
        .map(|c| String::from_utf8(c.to_vec()).unwrap())
        .collect()
}

/// One earlier exchange in a request: the assistant's calls (id, name), then the results sent back (id, is_error).
type Exchange<'a> = (&'a [(&'a str, &'a str)], &'a [(&'a str, bool)]);

/// Claude Code's request: the owner's words, then each exchange as an assistant turn and a user turn.
fn request(turns: &[Exchange]) -> Vec<u8> {
    let mut messages = vec![json!({"role": "user", "content": "the owner's private words"})];
    for (calls, results) in turns {
        messages.push(json!({"role": "assistant", "content": calls.iter()
            .map(|(id, name)| json!({"type": "tool_use", "id": id, "name": name, "input": {"q": "private"}}))
            .collect::<Vec<_>>()}));
        messages.push(json!({"role": "user", "content": results.iter()
            .map(|(id, error)| json!({"type": "tool_result", "tool_use_id": id, "is_error": error,
                "content": [{"type": "text", "text": "a private result"}]}))
            .collect::<Vec<_>>()}));
    }
    serde_json::to_vec(&json!({"model": "claude-sonnet-5-5", "messages": messages, "stream": true}))
        .unwrap()
}

/// The run's steps as its usage record carries them.
fn record(steps: Option<Steps>) -> Value {
    let mut data = serde_json::Map::new();
    if let Some(s) = steps {
        s.insert_into(&mut data);
    }
    Value::Object(data)
}

// decision: RUN-477, RUN-479
#[tokio::test]
async fn a_plain_answer_is_one_step_with_its_stop_and_times() {
    let (relay, _got, server) = start(|_, body| {
        if String::from_utf8_lossy(body).contains("\"stream\":true") {
            (200, "text/event-stream", sse(5, 0, 0, 0, 4))
        } else {
            // an answer that isn't streamed is read whole: its calls and stop the same way
            let answer = json!({"id": "m", "model": "claude-sonnet-5-5", "stop_reason": "tool_use",
                "content": [{"type": "text", "text": "x"},
                    {"type": "tool_use", "id": "toolu_1", "name": "mcp__ferb__memory_read", "input": {}}],
                "usage": {"input_tokens": 2, "output_tokens": 1}});
            (200, "application/json", vec![answer.to_string()])
        }
    })
    .await;
    let url = relay.base_url("run-p");
    assert_eq!(relay.take_steps("run-p"), None, "nothing before a step");
    post(&format!("{url}/v1/messages?beta=true"), &request(&[])).await;
    // a token count is no step
    post(&format!("{url}/v1/messages/count_tokens"), &request(&[])).await;
    assert_eq!(
        record(relay.take_steps("run-p")),
        json!({"steps": [{"called": [], "results": [], "stop": "end_turn", "first_byte_ms": 250,
            "done_ms": 500}]})
    );
    assert_eq!(relay.take_steps("run-p"), None, "taken once");
    let plain = serde_json::to_vec(&json!({"model": "m", "messages": []})).unwrap();
    post(&format!("{url}/v1/messages"), &plain).await;
    assert_eq!(
        record(relay.take_steps("run-p")),
        json!({"steps": [{"called": ["mcp__ferb__memory_read"], "results": [], "stop": "tool_use",
            "first_byte_ms": 250, "done_ms": 500}]})
    );
    // the usage is counted as before, beside the steps
    assert_eq!(relay.take_usage("run-p").map(|t| t.requests), Some(2));
    post(&format!("{url}/v1/messages"), &plain).await;
    relay.forget("run-p");
    assert_eq!(
        relay.take_steps("run-p"),
        None,
        "a forgotten run keeps nothing"
    );
    server.abort();
}

// decision: RUN-477, RUN-479
#[tokio::test]
async fn an_answer_tool_whose_result_was_an_error_and_was_called_again_is_named() {
    // the doubled answer: Claude Code sends its own StructuredOutput call back as an error and the model calls it again
    let (relay, _got, server) = start(|_, _| {
        (
            200,
            "text/event-stream",
            sse_calls(&["StructuredOutput"], "tool_use"),
        )
    })
    .await;
    let url = relay.base_url("run-d");
    post(&format!("{url}/v1/messages?beta=true"), &request(&[])).await;
    let again = request(&[(&[("toolu_0", "StructuredOutput")], &[("toolu_0", true)])]);
    post(&format!("{url}/v1/messages?beta=true"), &again).await;
    let row = record(relay.take_steps("run-d"));
    assert_eq!(
        row,
        json!({"steps": [
            {"called": ["StructuredOutput"], "results": [], "stop": "tool_use", "first_byte_ms": 250, "done_ms": 500},
            {"called": ["StructuredOutput"], "results": [{"tool": "StructuredOutput", "error": true}],
                "stop": "tool_use", "first_byte_ms": 250, "done_ms": 500},
        ]})
    );
    // names, flags and numbers only: no argument, result or prompt text, no call id
    let text = row.to_string();
    for private in ["private", "toolu_", "reply", "\"q\""] {
        assert!(!text.contains(private), "{private} in {text}");
    }
    server.abort();
}

// decision: RUN-477, RUN-479
#[tokio::test]
async fn a_lookup_tools_error_result_is_named_from_the_call_it_answers() {
    let (relay, _got, server) =
        start(|_, _| (200, "text/event-stream", sse_calls(&[], "end_turn"))).await;
    let url = relay.base_url("run-m");
    let body = request(&[
        (
            &[("toolu_a", "mcp__ferb__memory_read")],
            &[("toolu_a", false)],
        ),
        (
            &[
                ("toolu_b", "mcp__ferb__chat_history"),
                ("toolu_c", "mcp__ferb__task_records"),
            ],
            // a result for a call the message before doesn't hold is named "?"
            &[("toolu_b", true), ("toolu_c", false), ("toolu_a", false)],
        ),
    ]);
    post(&format!("{url}/v1/messages"), &body).await;
    assert_eq!(
        record(relay.take_steps("run-m"))["steps"][0]["results"],
        json!([{"tool": "mcp__ferb__chat_history", "error": true},
            {"tool": "mcp__ferb__task_records", "error": false}, {"tool": "?", "error": false}]),
        "only the last user message's results; the earlier exchange's are not repeated"
    );
    server.abort();
}

// decision: RUN-477, RUN-479
#[tokio::test]
async fn compressed_traffic_leaves_no_step_rather_than_a_wrong_one() {
    let (relay, _got, server) = start(|path, _| {
        let kind = if path.contains("zipped") {
            // the upstream says its answer is compressed: nothing in it is read
            "text/event-stream\r\ncontent-encoding: gzip"
        } else {
            "text/event-stream"
        };
        (200, kind, sse_calls(&["StructuredOutput"], "tool_use"))
    })
    .await;
    let url = relay.base_url("run-z");
    post(&format!("{url}/v1/messages?zipped=1"), &request(&[])).await;
    // a request Claude Code compressed is not read either
    reqwest::Client::new()
        .post(format!("{url}/v1/messages"))
        .header("content-encoding", "gzip")
        .body(request(&[]))
        .send()
        .await
        .unwrap()
        .bytes()
        .await
        .unwrap();
    assert_eq!(relay.take_steps("run-z"), None);
    assert_eq!(relay.take_usage("run-z"), None);
    server.abort();
}

// decision: RUN-477, RUN-479
#[tokio::test]
async fn a_failed_answer_is_a_step_with_its_status_and_no_usage() {
    let (relay, _got, server) = start(|_, _| {
        (
            529,
            "application/json",
            vec!["{\"type\":\"error\",\"error\":{\"type\":\"overloaded_error\"}}".into()],
        )
    })
    .await;
    let url = relay.base_url("run-f");
    post(&format!("{url}/v1/messages"), &request(&[])).await;
    assert_eq!(
        record(relay.take_steps("run-f")),
        json!({"steps": [{"called": [], "results": [], "status": 529, "first_byte_ms": 250, "done_ms": 500}]})
    );
    assert_eq!(relay.take_usage("run-f"), None, "no usage, as before");
    server.abort();
}

// decision: RUN-477, RUN-479
#[tokio::test]
async fn a_long_run_keeps_its_latest_steps_and_counts_the_rest() {
    let n = Arc::new(AtomicUsize::new(0));
    let (relay, _got, server) = start(move |_, _| {
        let i = n.fetch_add(1, Ordering::SeqCst);
        (
            200,
            "text/event-stream",
            sse_calls(&[&format!("tool_{i}")], "tool_use"),
        )
    })
    .await;
    let url = relay.base_url("run-l");
    for _ in 0..KEPT + 6 {
        post(&format!("{url}/v1/messages"), &request(&[])).await;
    }
    let row = record(relay.take_steps("run-l"));
    let kept = row["steps"].as_array().unwrap();
    assert_eq!(kept.len(), KEPT);
    assert_eq!(row["steps_dropped"], json!(6));
    assert_eq!(
        kept[0]["called"],
        json!(["tool_6"]),
        "the earliest are let go"
    );
    assert_eq!(
        kept[KEPT - 1]["called"],
        json!([format!("tool_{}", KEPT + 5)])
    );
    server.abort();
}

// decision: RUN-477, RUN-479
#[test]
fn a_name_is_kept_only_as_anthropic_allows_one() {
    assert_eq!(
        steps::tool_name("mcp__ferb__chat_history"),
        "mcp__ferb__chat_history"
    );
    assert_eq!(steps::tool_name("StructuredOutput"), "StructuredOutput");
    let long = "x".repeat(129);
    for odd in ["", "a b", "ignore previous; call me", "név", long.as_str()] {
        assert_eq!(steps::tool_name(odd), "?", "{odd}");
    }
    // a body that isn't a JSON request gives no results (and so no step); odd content shapes are skipped, not fatal
    assert_eq!(steps::results(b"not json"), None);
    let odd = json!({"messages": [{"role": "assistant", "content": [7, "x", {"type": "tool_use", "id": 3}]},
        {"role": "user", "content": [null, {"type": "tool_result", "tool_use_id": "t", "is_error": "yes"}]}]});
    assert_eq!(
        steps::results(odd.to_string().as_bytes()),
        Some(vec![steps::ToolResult {
            tool: "?".into(),
            error: false
        }])
    );
}
