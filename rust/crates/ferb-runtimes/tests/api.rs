//! The direct-API loop against rust/fixtures/runtimes/api.json: a local scripted server on 127.0.0.1 answers as the
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
