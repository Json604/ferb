//! RUN-468..RUN-470: a tool loop's earlier screens and large reads, compacted in batches. Stand-ins only: invented
//! results, the wire shapes as Claude Code and FERB's API loop write them (and the Responses shape as Codex 0.166.0 sent
//! it to a stand-in server, for when Codex joins: RUN-472), and the relay against a stand-in upstream on 127.0.0.1.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use ferb_runtimes::api::{anthropic, google, openai};
use ferb_runtimes::claude::proxy::ToolListProxy;
use ferb_runtimes::claude::ToolProxy;
use ferb_runtimes::compact::{self, Compactor, Policy, Seen};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;

/// Every result old enough is a candidate at once: the rules, not the cadence, are under test.
const EAGER: Policy = Policy {
    start_tokens: 0,
    gain_tokens: 1,
    keep_steps: 1,
    read_chars: 2_000,
    read_age: 2,
};

fn clock() -> compact::Clock {
    let n = Arc::new(Mutex::new(0));
    Arc::new(move || {
        let mut n = n.lock().unwrap();
        *n += 1;
        format!("10:00:{:02}", *n)
    })
}

fn snapshot(bundle: &str, app: &str, window: u64, title: &str, size: usize) -> String {
    let content = json!({"app": app, "bundle_id": bundle, "window": title,
        "elements": ["e".repeat(size)], "offset": 0, "next_offset": null,
        "windows": [{"id": window, "title": "", "current": true}, {"id": 7, "title": ""}, {"id": 8, "title": ""},
            {"id": 999, "title": ""}]});
    format!("{}\n[mac]", serde_json::to_string(&content).unwrap())
}

fn shot(bundle: &str, app: &str, window: u64, evidence: &str) -> String {
    let content = json!({"app": app, "bundle_id": bundle, "shot": "shot1", "width": 1568, "height": 1044,
        "window_id": window, "evidence": evidence});
    format!("{}\n[mac]", serde_json::to_string(&content).unwrap())
}

fn seen(id: &str, tool: &str, args: Value, text: String, images: usize) -> Seen {
    Seen {
        call_id: id.into(),
        tool: tool.into(),
        args,
        text,
        images,
    }
}

/// Feeds one result per model call, as a loop would, and returns the plan after the last.
fn loop_of(policy: Policy, results: &[Seen]) -> Compactor {
    let mut c = Compactor::new(policy, clock());
    for n in 1..=results.len() {
        let carried = &results[..n];
        let tokens = carried.iter().map(Seen::tokens).sum::<u64>();
        c.step(carried, tokens);
    }
    // one more call: the newest result has aged past keep_steps
    let tokens = results.iter().map(Seen::tokens).sum::<u64>();
    c.step(results, tokens);
    c
}

// decision: RUN-468
#[test]
fn a_newer_look_at_the_same_window_replaces_the_older_one_and_only_that() {
    let a1 = seen(
        "a1",
        "mac_snapshot",
        json!({"app": "pid:1"}),
        snapshot("com.x", "X", 7, "Inbox", 3000),
        0,
    );
    let b1 = seen(
        "b1",
        "mac_snapshot",
        json!({"app": "pid:1"}),
        snapshot("com.x", "X", 8, "Other", 3000),
        0,
    );
    let s1 = seen(
        "s1",
        "mac_screenshot",
        json!({"app": "pid:1"}),
        shot("com.x", "X", 7, "ev1"),
        1,
    );
    let a2 = seen(
        "a2",
        "mac_snapshot",
        json!({"app": "pid:1", "window": "7"}),
        snapshot("com.x", "X", 7, "Sent", 3000),
        0,
    );
    let s2 = seen(
        "s2",
        "mac_screenshot",
        json!({"app": "pid:1"}),
        shot("com.x", "X", 7, "ev2"),
        1,
    );
    let t1 = seen(
        "t1",
        "browser_snapshot",
        json!({"tab": "t1"}),
        format!(
            "{{\"title\": \"Home\", \"text\": \"{}\"}}\n",
            "x".repeat(400)
        ),
        0,
    );
    let t2 = seen(
        "t2",
        "browser_snapshot",
        json!({"tab": "t2"}),
        "{\"title\": \"Other\"}\n".into(),
        0,
    );
    let t1b = seen(
        "t1b",
        "browser_snapshot",
        json!({"tab": "t1"}),
        "{\"title\": \"Home 2\"}\n".into(),
        0,
    );
    let c = loop_of(EAGER, &[a1, b1, s1, a2, s2, t1, t2, t1b]);
    let plan: Vec<&str> = c.plan().keys().map(String::as_str).collect();
    // window 7's first snapshot and first screenshot, tab t1's first snapshot; never window 8, tab t2, the newest
    assert_eq!(plan, ["a1", "s1", "t1"]);
    assert_eq!(
        c.replacement("a1").unwrap(),
        "FERB: an earlier snapshot of X (window 7, 'Inbox') taken at 10:00:01; a newer one is below."
    );
    assert_eq!(
        c.replacement("s1").unwrap(),
        "FERB: an earlier screenshot of X (window 7) taken at 10:00:03; a newer one is below."
    );
    assert_eq!(
        c.replacement("t1").unwrap(),
        "FERB: an earlier snapshot of tab t1 ('Home') taken at 10:00:06; a newer one is below."
    );
}

// decision: RUN-468
#[test]
fn a_snapshot_and_a_screenshot_are_different_kinds_and_pages_are_reads() {
    let tree = seen(
        "tree",
        "mac_snapshot",
        json!({}),
        snapshot("com.x", "X", 7, "W", 100),
        0,
    );
    let image = seen(
        "image",
        "mac_screenshot",
        json!({}),
        shot("com.x", "X", 7, ""),
        1,
    );
    let page2 = {
        let content = json!({"app": "X", "bundle_id": "com.x", "window": "W", "elements": ["p".repeat(100)],
            "offset": 40, "windows": [{"id": 7, "current": true}]});
        seen(
            "page2",
            "mac_snapshot",
            json!({"offset": 40}),
            format!("{content}\n"),
            0,
        )
    };
    let c = loop_of(EAGER, &[tree, image, page2]);
    assert!(c.plan().is_empty(), "{:?}", c.plan());
}

// decision: RUN-469
#[test]
fn the_owners_words_answers_and_instructions_are_never_replaced() {
    let big = "q".repeat(5000);
    let mut results = Vec::new();
    for (i, tool) in compact::KEPT_TOOLS.iter().enumerate() {
        results.push(seen(
            &format!("k{i}"),
            tool,
            json!({}),
            format!("{{\"text\": \"{big}\"}}\n"),
            0,
        ));
    }
    // an error result names no window: nothing replaces it as a screen, and small it is no read
    results.push(seen(
        "err",
        "mac_snapshot",
        json!({}),
        "{\"error\": \"no window\"}\n".into(),
        0,
    ));
    let c = loop_of(EAGER, &results);
    assert!(c.plan().is_empty(), "{:?}", c.plan());
}

// decision: RUN-469
#[test]
fn a_large_read_shrinks_only_when_old_and_the_last_steps_stay_whole() {
    let read = |id: &str, n: usize| {
        seen(
            id,
            "mac_menu",
            json!({"app": "pid:1"}),
            format!("{{\"menus\": \"{}\"}}\n[mac]", "m".repeat(n)),
            0,
        )
    };
    let small = read("small", 100);
    let big = read("big", 5000);
    let late = read("late", 5000);
    let mut c = Compactor::new(EAGER, clock());
    // step 1: big is new; step 2: big is 1 call old (keep_steps 1 passed, read_age 2 not yet); step 3: shortened
    c.step(&[small.clone(), big.clone()], 10_000);
    assert!(c.plan().is_empty());
    c.step(&[small.clone(), big.clone()], 10_000);
    assert!(c.plan().is_empty());
    c.step(&[small.clone(), big.clone(), late.clone()], 10_000);
    assert_eq!(c.plan().keys().collect::<Vec<_>>(), ["big"]);
    let line = c.replacement("big").unwrap();
    assert!(line.starts_with("FERB: this mac_menu result from 10:00:02 (5019 characters) was shortened to save room. It began: {\"menus\": \"mmm"), "{line}");
    assert!(line.ends_with("… To see it again, call mac_menu again for a fresh read, or task_evidence with kind \"tool.call\" and query \"mac.menu\" for this one."), "{line}");
    assert!(line.matches('m').count() > 150);
    // a result naming its evidence is found again by it
    let with_evidence = compact::read_line(
        "about_diagnostics",
        "10:00:09",
        &format!(
            "{{\"evidence\": \"e9\", \"log\": \"{}\"}}\n",
            "l".repeat(300)
        ),
        "e9",
    );
    assert!(with_evidence.contains("query \"e9\""), "{with_evidence}");
}

// decision: RUN-469
#[test]
fn a_window_the_app_no_longer_lists_becomes_an_old_read() {
    let gone = seen(
        "gone",
        "mac_snapshot",
        json!({}),
        snapshot("com.x", "X", 5, "Old", 3000),
        0,
    );
    let mut c = Compactor::new(EAGER, clock());
    c.step(std::slice::from_ref(&gone), 10_000);
    // the next snapshot is of window 7 and lists 7, 8 and 999, not 5
    let now = seen(
        "now",
        "mac_snapshot",
        json!({}),
        snapshot("com.x", "X", 7, "New", 3000),
        0,
    );
    c.step(&[gone.clone(), now.clone()], 10_000);
    c.step(&[gone.clone(), now.clone()], 10_000);
    assert_eq!(c.plan().keys().collect::<Vec<_>>(), ["gone"]);
    assert!(c
        .replacement("gone")
        .unwrap()
        .starts_with("FERB: this mac_snapshot result from 10:00:01 ("));
    // while the app still lists it, the last look at a window stays
    let listed = seen(
        "listed",
        "mac_snapshot",
        json!({}),
        snapshot("com.x", "X", 999, "Kept", 3000),
        0,
    );
    let mut c = Compactor::new(EAGER, clock());
    for _ in 0..4 {
        c.step(&[listed.clone(), now.clone()], 10_000);
    }
    assert!(c.plan().is_empty());
}

// decision: RUN-469
#[test]
fn a_closed_tabs_last_look_becomes_an_old_read() {
    let look = |id: &str, tab: &str| {
        seen(
            id,
            "browser_snapshot",
            json!({"tab": tab}),
            format!("{{\"title\": \"T\", \"text\": \"{}\"}}\n", "x".repeat(3000)),
            0,
        )
    };
    let (t1, t2) = (look("t1", "t1"), look("t2", "t2"));
    let close = seen(
        "close",
        "browser_close",
        json!({"tab": "t1"}),
        "{\"tab\": \"t1\", \"closed\": true}\n".into(),
        0,
    );
    let failed = seen(
        "failed",
        "browser_close",
        json!({"tab": "t2"}),
        "{\"error\": \"no tab t2\"}\n".into(),
        0,
    );
    let c = loop_of(EAGER, &[t1, t2, close, failed]);
    assert_eq!(
        c.plan().keys().collect::<Vec<_>>(),
        ["t1"],
        "t2's close failed: its last look stays"
    );
    assert!(c
        .replacement("t1")
        .unwrap()
        .starts_with("FERB: this browser_snapshot result from 10:00:01 ("));
}

// decision: RUN-470
#[test]
fn compaction_waits_for_its_size_and_gain_and_the_plan_stays_the_same_between() {
    let policy = Policy {
        start_tokens: 20_000,
        gain_tokens: 6_000,
        keep_steps: 2,
        read_chars: 2_000,
        read_age: 4,
    };
    // the same window looked at on every call: 4,000 characters (about 1,334 tokens) each
    let looks: Vec<Seen> = (0..40)
        .map(|i| {
            seen(
                &format!("c{i}"),
                "mac_snapshot",
                json!({}),
                snapshot("com.x", "X", 7, "W", 3900),
                0,
            )
        })
        .collect();
    let mut c = Compactor::new(policy, clock());
    let mut made = Vec::new();
    let mut plans = Vec::new();
    for n in 1..=looks.len() {
        let carried = &looks[..n];
        let tokens = 10_000
            + carried
                .iter()
                .map(|s| match c.replacement(&s.call_id) {
                    Some(l) => compact::text_tokens(l),
                    None => s.tokens(),
                })
                .sum::<u64>();
        if let Some(m) = c.step(carried, tokens) {
            assert!(m.tokens_before >= policy.start_tokens);
            assert!(m.tokens_before - m.tokens_after >= policy.gain_tokens);
            made.push(n);
        }
        plans.push(c.plan().clone());
    }
    // no compaction under 20,000 tokens, then one each time the replaceable looks reach 6,000 tokens: every fifth
    // or sixth call, never on every call
    assert_eq!(made, [8, 14, 20, 25, 30, 35, 40]);
    for w in plans.windows(2) {
        // a replacement once made never changes or goes away
        for (k, v) in &w[0] {
            assert_eq!(w[1].get(k), Some(v));
        }
    }
    // the last two calls' looks and the newest stay whole
    let last = plans.last().unwrap();
    assert!(!last.contains_key("c39") && !last.contains_key("c38"));
}

fn anthropic_body(results: &[(&str, &str, String, bool)]) -> Value {
    let mut messages =
        vec![json!({"role": "user", "content": [{"type": "text", "text": "the owner's words"}]})];
    for (id, tool, text, image) in results {
        messages.push(
            json!({"role": "assistant", "content": [{"type": "tool_use", "id": id,
            "name": format!("mcp__ferb__{tool}"), "input": {"app": "pid:1"}}]}),
        );
        let mut content = vec![json!({"type": "text", "text": text})];
        if *image {
            content.push(json!({"type": "image", "source": {"type": "base64", "media_type": "image/jpeg", "data": "AAAA"}}));
        }
        messages.push(
            json!({"role": "user", "content": [{"type": "tool_result", "tool_use_id": id,
            "content": content}]}),
        );
    }
    json!({"model": "m", "system": [{"type": "text", "text": "FERB"}], "messages": messages})
}

// decision: RUN-470
#[test]
fn the_anthropic_shape_reads_and_rewrites_tool_results_only() {
    let body = anthropic_body(&[
        ("u1", "mac_screenshot", shot("com.x", "X", 7, "e1"), true),
        ("u2", "mac_screenshot", shot("com.x", "X", 7, "e2"), true),
    ]);
    let seen = anthropic::results(&body);
    assert_eq!(seen.len(), 2);
    assert_eq!(
        (seen[0].tool.as_str(), seen[0].images, &seen[0].args),
        ("mac_screenshot", 1, &json!({"app": "pid:1"}))
    );
    let mut plan = BTreeMap::new();
    plan.insert("u1".to_string(), "LINE".to_string());
    let mut after = body.clone();
    assert_eq!(anthropic::rewrite(&mut after, &plan), 1);
    assert_eq!(
        after["messages"][2]["content"][0]["content"],
        json!([{"type": "text", "text": "LINE"}])
    );
    // everything else is as it was, and applying the plan again changes nothing
    let mut expected = body.clone();
    expected["messages"][2]["content"][0]["content"] = json!([{"type": "text", "text": "LINE"}]);
    assert_eq!(after, expected);
    assert_eq!(anthropic::rewrite(&mut after, &plan), 0);
}

// decision: RUN-468
#[test]
fn the_responses_shape_reads_codexs_namespaced_calls_and_its_wall_time_lines() {
    // as Codex 0.166.0 sent them to a stand-in server
    let out = |id: &str, text: String| {
        json!({"type": "function_call_output", "id": format!("fco_{id}"), "call_id": id,
        "output": [{"type": "input_text", "text": "Wall time: 0.0032 seconds\nOutput:"},
            {"type": "input_text", "text": text}, {"type": "input_image", "image_url": "data:image/jpeg;base64,AAAA"}]})
    };
    let body = json!({"model": "gpt-5.6-sol", "input": [
        {"type": "message", "role": "user", "content": [{"type": "input_text", "text": "the owner's words"}]},
        {"type": "function_call", "id": "fc_1", "name": "mac_screenshot", "namespace": "mcp__ferb", "arguments": "{\"app\": \"pid:1\"}", "call_id": "call_1"},
        out("call_1", shot("com.x", "X", 7, "e1")),
        {"type": "function_call", "id": "fc_2", "name": "mac_screenshot", "namespace": "mcp__ferb", "arguments": "{\"app\": \"pid:1\"}", "call_id": "call_2"},
        out("call_2", shot("com.x", "X", 7, "e2")),
    ]});
    let seen = openai::results(&body);
    assert_eq!(seen.len(), 2);
    assert_eq!(
        (seen[0].tool.as_str(), seen[0].images),
        ("mac_screenshot", 1)
    );
    let mut c = Compactor::new(EAGER, clock());
    c.step(&seen, 50_000);
    c.step(&seen, 50_000);
    assert_eq!(c.plan().keys().collect::<Vec<_>>(), ["call_1"]);
    let mut after = body.clone();
    assert_eq!(openai::rewrite(&mut after, c.plan()), 1);
    assert_eq!(
        after["input"][2]["output"],
        json!(
            "FERB: an earlier screenshot of X (window 7) taken at 10:00:01; a newer one is below."
        )
    );
    assert_eq!(after["input"][4], body["input"][4]);
}

// decision: RUN-468
#[test]
fn the_gemini_shape_pairs_responses_without_ids_by_place() {
    let body = json!({"contents": [
        {"role": "model", "parts": [{"functionCall": {"name": "mac_screenshot", "args": {"app": "pid:1"}}}]},
        {"role": "user", "parts": [{"functionResponse": {"name": "mac_screenshot", "response": {"result": shot("com.x", "X", 7, "")},
            "parts": [{"inlineData": {"mimeType": "image/jpeg", "data": "AAAA"}}]}}]},
        {"role": "model", "parts": [{"functionCall": {"name": "mac_screenshot", "args": {"app": "pid:1"}, "id": "g2"}}]},
        {"role": "user", "parts": [{"functionResponse": {"name": "mac_screenshot", "id": "g2", "response": {"result": shot("com.x", "X", 7, "")},
            "parts": [{"inlineData": {"mimeType": "image/jpeg", "data": "AAAA"}}]}}]},
    ]});
    let seen = google::results(&body);
    assert_eq!(
        seen.iter().map(|s| s.call_id.as_str()).collect::<Vec<_>>(),
        ["ferb-local:1:0", "g2"]
    );
    assert_eq!(seen[0].images, 1);
    let mut plan = BTreeMap::new();
    plan.insert("ferb-local:1:0".to_string(), "LINE".to_string());
    let mut after = body.clone();
    assert_eq!(google::rewrite(&mut after, &plan), 1);
    assert_eq!(
        after["contents"][1]["parts"][0]["functionResponse"],
        json!({"name": "mac_screenshot", "response": {"result": "LINE"}})
    );
    assert_eq!(after["contents"][3], body["contents"][3]);
}

/// A stand-in upstream: records each request's path, encoding and body, answers 200 with a small JSON.
/// (path, content-encoding, body) of each request.
type Got = Arc<Mutex<Vec<(String, Option<String>, Vec<u8>)>>>;

async fn upstream(listener: TcpListener, seen: Got) {
    loop {
        let Ok((stream, _)) = listener.accept().await else {
            return;
        };
        let seen = seen.clone();
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
                let (mut length, mut encoding) = (0usize, None);
                loop {
                    let mut h = String::new();
                    read.read_line(&mut h).await.unwrap();
                    let h = h.trim_end();
                    if h.is_empty() {
                        break;
                    }
                    let (k, v) = h.split_once(':').unwrap();
                    match k.trim().to_lowercase().as_str() {
                        "content-length" => length = v.trim().parse().unwrap(),
                        "content-encoding" => encoding = Some(v.trim().to_string()),
                        _ => {}
                    }
                }
                let mut body = vec![0u8; length];
                read.read_exact(&mut body).await.unwrap();
                seen.lock().unwrap().push((path, encoding, body));
                write
                    .write_all(b"HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: 2\r\n\r\n{}")
                    .await
                    .unwrap();
            }
        });
    }
}

async fn post(url: &str, body: Vec<u8>, encoding: Option<&str>) {
    let mut req = reqwest::Client::new().post(url).body(body);
    if let Some(e) = encoding {
        req = req.header("content-encoding", e);
    }
    let resp = req.send().await.unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    let _ = resp.bytes().await;
}

// decision: RUN-470
#[tokio::test]
async fn the_relay_compacts_only_a_registered_runs_history_and_keeps_its_bytes_between_compactions()
{
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let up = format!(
        "http://127.0.0.1:{}/backend",
        listener.local_addr().unwrap().port()
    );
    let got = Arc::new(Mutex::new(Vec::new()));
    let server = tokio::spawn(upstream(listener, got.clone()));
    let relay = ToolListProxy::start_with_clock(&up, Arc::new(|| "s".to_string()), clock())
        .await
        .unwrap();
    let call = |i: usize| {
        vec![
            json!({"role": "assistant", "content": [{"type": "tool_use", "id": format!("c{i}"),
                "name": "mcp__ferb__mac_snapshot", "input": {}}]}),
            json!({"role": "user", "content": [{"type": "tool_result", "tool_use_id": format!("c{i}"),
                "content": snapshot("com.x", "X", 7, "W", 3000)}]}),
        ]
    };
    let body = |n: usize| {
        let mut messages = vec![
            json!({"role": "user", "content": [{"type": "text", "text": "the owner's words"}]}),
        ];
        for i in 0..n {
            messages.extend(call(i));
        }
        serde_json::to_vec(&json!({"model": "m", "messages": messages, "stream": true})).unwrap()
    };
    // a run that never asked for compaction goes through byte for byte
    let plain = relay.base_url("plain");
    post(&format!("{plain}/v1/messages"), body(5), None).await;
    let policy = Policy {
        start_tokens: 0,
        gain_tokens: 2_500,
        keep_steps: 1,
        read_chars: 2_000,
        read_age: 2,
    };
    let url = relay.base_url("run-1");
    relay.compact("run-1", policy);
    for n in 1..=7 {
        post(&format!("{url}/v1/messages?beta=true"), body(n), None).await;
    }
    // a body the client compressed is not read and goes on with its encoding
    post(
        &format!("{url}/v1/messages"),
        b"\x28\xb5\x2f\xfd".to_vec(),
        Some("zstd"),
    )
    .await;
    // a request that isn't a model step is forwarded as it came
    post(&format!("{url}/v1/messages/count_tokens"), body(6), None).await;
    let got = got.lock().unwrap().clone();
    assert_eq!(got.len(), 10);
    assert_eq!(got[0], ("/backend/v1/messages".to_string(), None, body(5)));
    let outputs = |b: &[u8]| -> Vec<String> {
        let v: Value = serde_json::from_slice(b).unwrap();
        v["messages"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|m| m["content"].as_array().unwrap().clone())
            .filter(|c| c["type"] == "tool_result")
            .map(|c| c["content"].as_str().unwrap().chars().take(30).collect())
            .collect()
    };
    let line = "FERB: an earlier snapshot of X";
    // each look is about 1,030 tokens: request 2 and 3's replaceable looks (one, two) aren't worth a break, request
    // 4's three are; 5 and 6 keep that plan; 7 replaces the next three
    for (k, expect_lines) in [(1, 0), (2, 0), (3, 0), (4, 3), (5, 3), (6, 3), (7, 6)] {
        let outs = outputs(&got[k].2);
        assert_eq!(
            outs.iter().filter(|o| o.starts_with(line)).count(),
            expect_lines,
            "request {k}: {outs:?}"
        );
    }
    // between compactions the forwarded prefix is the same bytes: each request starts with the one before (less its end)
    for k in [4, 5] {
        let (a, b) = (&got[k].2, &got[k + 1].2);
        let cut = a.len() - "]".len() - ",\"stream\":true}".len();
        assert_eq!(&a[..cut], &b[..cut], "requests {k} and {}", k + 1);
    }
    assert_eq!(
        got[8],
        (
            "/backend/v1/messages".to_string(),
            Some("zstd".to_string()),
            b"\x28\xb5\x2f\xfd".to_vec()
        )
    );
    assert_eq!(got[9].2, body(6));
    let made = relay.compactions("run-1");
    assert_eq!(
        made.iter()
            .map(|m| (m.step, m.screens, m.reads))
            .collect::<Vec<_>>(),
        [(4, 3, 0), (7, 3, 0)]
    );
    assert!(relay.compactions("run-1").is_empty(), "reported once");
    assert!(relay.compactions("plain").is_empty());
    relay.forget("run-1");
    server.abort();
}

// decision: RUN-470
#[tokio::test]
async fn claude_codes_relay_still_records_tool_names_and_compacts_messages() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let up = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
    let got = Arc::new(Mutex::new(Vec::new()));
    let server = tokio::spawn(upstream(listener, got.clone()));
    let relay = ToolListProxy::start_with_clock(&up, Arc::new(|| "s".to_string()), clock())
        .await
        .unwrap();
    let url = relay.base_url("run-c");
    relay.compact("run-c", EAGER);
    let mut body = anthropic_body(&[
        ("u1", "mac_screenshot", shot("com.x", "X", 7, "e1"), true),
        ("u2", "mac_screenshot", shot("com.x", "X", 7, "e2"), true),
    ]);
    body["tools"] = json!([{"name": "mcp__ferb__mac_screenshot"}]);
    for _ in 0..2 {
        post(
            &format!("{url}/v1/messages?beta=true"),
            serde_json::to_vec(&body).unwrap(),
            None,
        )
        .await;
    }
    assert_eq!(
        relay.seen("run-c").into_iter().collect::<Vec<_>>(),
        ["mcp__ferb__mac_screenshot"]
    );
    let got = got.lock().unwrap().clone();
    let last: Value = serde_json::from_slice(&got[1].2).unwrap();
    assert_eq!(
        last["messages"][2]["content"][0]["content"][0]["text"],
        json!(
            "FERB: an earlier screenshot of X (window 7) taken at 10:00:01; a newer one is below."
        )
    );
    assert_eq!(
        last["messages"][4], body["messages"][4],
        "the newest look stays whole"
    );
    server.abort();
}
