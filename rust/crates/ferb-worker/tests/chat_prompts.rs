//! The worker and checker prompts inside the recorded daemon runs (rust/fixtures/daemon/chat_scenarios.json, 28 real
//! runs of the Python daemon): from the same inputs the Rust worker must send the same request, byte for byte.
//!
//! What the worker composes is checked; what it is handed is taken from the recording: the supervisor's context
//! blocks (every part before the runtime-tools block), the instructions (`_system`, ferb-supervisor's), the tool
//! rows the gateway listed (rust/fixtures/catalog/tools.json by name), the corrections the tree held (the part
//! after the runtime-tools block), and the time the run began (the clock is set to it). The worker then adds the
//! runtime-tools block and the time facts, the corrections, assembles, counts the reserve and writes the budget
//! report; its request (system text, every part, the cacheable count), the runtime role it asks the factory for,
//! the tool names and the answer schema must equal the recorded call's.
//!
//! The daemon fixture writes its temporary root as `/FX`, but the budget report counted the real path's bytes
//! (`$TMPDIR/ferbdx`, 55 characters on the machine that recorded it). The root's length is read once from the
//! first call (its report's count minus the bytes of its parts, over the `/FX` it holds); every call is then run
//! with `/FX` standing for a root of that length and written back as `/FX`, and must match whole.

mod support;

use std::sync::Arc;

use ferb_clock::clock::ZonedClock;
use ferb_context::prompts::PromptRoot;
use ferb_context::tokens::Tokenizer;
use ferb_core::types::{Identity, Task, TaskRole};
use ferb_worker::ports::{WorkerHost, WorkerTree};
use ferb_worker::{RunOptions, Runs, Worker, WorkerConfig};
use serde_json::{json, Map, Value};
use support::*;

#[tokio::test(start_paused = true)]
async fn worker_and_checker_prompts_match_the_recorded_daemon() {
    let chat = fixture("daemon", "chat_scenarios.json");
    let tools = fixture("catalog", "tools.json");
    let answers = fixture("catalog", "answer_schemas.json");
    let rows: Map<String, Value> = tools["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| {
            (
                t["name"].as_str().unwrap().to_string(),
                json!({"name": t["name"], "description": t["description"], "input_schema": t["input_schema"]}),
            )
        })
        .collect();
    let schemas: Map<String, Value> = answers["schemas"]
        .as_object()
        .unwrap()
        .iter()
        .filter_map(|(k, v)| v.get("core").map(|c| (k.clone(), c.clone())))
        .collect();
    let schemas = Value::Object(schemas);
    let (mut checked, mut differ, mut notes) = (0, 0, vec![]);
    let root = recorded_root(&chat);
    let mut masked = 0;
    for sc in chat["scenarios"].as_array().unwrap() {
        for m in sc["model_calls"].as_array().unwrap() {
            if m["role_played"] == "planner" {
                continue;
            }
            let call = format!(
                "{} call {} ({})",
                sc["name"].as_str().unwrap(),
                m["n"],
                m["key"].as_str().unwrap()
            );
            let parts: Vec<String> = m["parts"]
                .as_array()
                .unwrap()
                .iter()
                .map(|p| p.as_str().unwrap().to_string())
                .collect();
            let at = parts
                .iter()
                .position(|p| p.starts_with("Your FERB tools (MCP server `ferb`)"))
                .expect("the tools block");
            let cacheable = m["cacheable"].as_u64().unwrap() as usize;
            let ctx = parts[..at]
                .iter()
                .enumerate()
                .map(|(i, text)| {
                    let mut b = ferb_context::assemble::ContextBlock::new(
                        text.replace("/FX", &root),
                        format!("part {i}"),
                    );
                    b.cache = i < cacheable;
                    b
                })
                .collect();
            let corrections: Vec<Value> = parts[at + 1..parts.len() - 1]
                .iter()
                .map(|p| json!(p.replace("/FX", &root)))
                .collect();
            let header = &parts[at][parts[at].find("[now ").unwrap()..];
            let now = header[5..].split(" | ").next().unwrap(); // "Thursday 2026-01-01 09:00:00 UTC"
            let words: Vec<&str> = now.split(' ').collect();
            let start = time(&format!("{}T{}+00:00", words[1], words[2]));
            let tool_rows: Vec<Value> = m["tools"]
                .as_array()
                .unwrap()
                .iter()
                .map(|n| rows[n.as_str().unwrap()].clone())
                .collect();
            let sc_value = json!({
                "tools": tool_rows,
                "listed_as_is": true,
                "browser_context": "",
                "runtimes": [{"calls": [{"steps": [{"event": "final", "data": {"text": "{}"}}]}]}],
            });
            set_tools(vec![], vec![]);
            let host = TestHost::new(&sc_value, &schemas);
            host.clock.set_wall(start);
            *host.system.lock().unwrap() = m["system"].as_str().unwrap().to_string();
            let worker = Arc::new(Worker::new(
                host.clone() as Arc<dyn WorkerHost>,
                Arc::new(Runs::new()),
                WorkerConfig {
                    heartbeat_poll_s: 0.01,
                    stall_s: 180.0,
                    tokenizer: Tokenizer::Utf8Bytes,
                    prompts: PromptRoot::new(repo().join("ferb")).unwrap(),
                    approach: "orchestrator/prompts".into(),
                },
            ));
            let _ = host.worker.set(Arc::downgrade(&worker));
            let runtime_role = m["runtime_role"].as_str().unwrap();
            let verifier = runtime_role.starts_with("checker");
            let tree = Arc::new(TestTree::from_spec(&json!({
                "worker_type": runtime_role.strip_prefix("worker:").unwrap_or("default"),
                "steering": corrections,
            })));
            *host.tree.lock().unwrap() = Some(tree.clone());
            let task = Task {
                id: m["task_id"].as_str().unwrap().to_string(),
                parent_id: None,
                goal: "recorded".into(),
                device_id: "dev-fx".into(),
                identity: Identity::Owner,
                scopes: Default::default(),
                role: if verifier {
                    TaskRole::Verifier
                } else {
                    TaskRole::Worker
                },
                request_id: "req".into(),
                success_conditions: vec![],
                deadline: None,
            };
            let schema = &schemas[m["schema"].as_str().unwrap()];
            let wtree: Arc<dyn WorkerTree> = tree.clone();
            let options = RunOptions {
                effort: m["effort"].as_str().map(String::from),
                ..RunOptions::default()
            };
            let _ = worker
                .run_worker(&wtree, &task, "recorded.md", ctx, schema, "span", &options)
                .await;
            let log = host.take_log();
            let made = log
                .iter()
                .find(|e| e[0] == "make_runtime")
                .map(|e| e[1].clone())
                .unwrap_or_default();
            let mut started = log
                .iter()
                .find(|e| e[0] == "rt.start")
                .map(|e| e[1]["request"].clone())
                .unwrap_or_default();
            if let Some(parts) = started.get_mut("parts").and_then(Value::as_array_mut) {
                for p in parts.iter_mut() {
                    *p = json!(p.as_str().unwrap_or_default().replace(&root, "/FX"));
                }
            }
            let expected = json!({
                "role": runtime_role, "tool_names": m["tools"], "schema": {"schema": m["schema"]}, "effort": m["effort"],
                "system": m["system"], "parts": m["parts"], "cacheable": m["cacheable"], "images": m["images"],
            });
            let actual = json!({
                "role": made["role"], "tool_names": made["tool_names"], "schema": made["schema"], "effort": made["effort"],
                "system": started["system"], "parts": started["parts"], "cacheable": started["cacheable"],
                "images": started["images"].as_array().map(Vec::len).unwrap_or(99),
            });
            checked += 1;
            let mut actual = actual;
            if expected != actual && masked_root_only(&expected["parts"], &actual["parts"]) {
                // equal but for the root's form the recorder wrote as /FX (see `masked_root_only`)
                masked += 1;
                actual["parts"] = expected["parts"].clone();
            }
            if expected != actual {
                differ += 1;
                if expected["parts"] != actual["parts"] && notes.len() < 20 {
                    let (e, a) = (
                        expected["parts"].as_array().unwrap(),
                        actual["parts"].as_array().unwrap(),
                    );
                    for i in 0..e.len().max(a.len()) {
                        if e.get(i) != a.get(i) {
                            notes.push(format!(
                                "{call}: part {i}\n  python: {:?}\n  rust:   {:?}",
                                e.get(i),
                                a.get(i)
                            ));
                            break;
                        }
                    }
                }
                for k in expected.as_object().unwrap().keys() {
                    if k != "parts" && expected[k] != actual[k] && notes.len() < 20 {
                        notes.push(format!(
                            "{call}: {k}\n  python: {}\n  rust:   {}",
                            expected[k], actual[k]
                        ));
                    }
                }
            }
            let _ = host.clock.tzname();
        }
    }
    eprintln!(
        "chat prompts: {checked} worker and checker calls, {differ} differ; {masked} of them equal but for the \
         budget report's count of the masked root"
    );
    assert!(
        checked == 80,
        "the fixture has 80 worker and checker calls, saw {checked}"
    );
    assert!(differ == 0, "{differ} differ:\n{}", notes.join("\n"));
}

/// A stand-in for the recording machine's temporary root, as long as it was: from the first worker call, the
/// budget report's count of the body's bytes minus the bytes of its recorded parts, spread over the `/FX` in them.
fn recorded_root(chat: &Value) -> String {
    let m = chat["scenarios"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|s| s["model_calls"].as_array().unwrap().iter())
        .find(|m| m["role_played"] == "worker")
        .unwrap();
    let parts: Vec<&str> = m["parts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p.as_str().unwrap())
        .collect();
    let body = parts[..parts.len() - 1].join("\n\n");
    let report: Value =
        serde_json::from_str(parts[parts.len() - 1].trim_start_matches("Context budget: "))
            .unwrap();
    let extra = report["content_tokens_estimate"].as_u64().unwrap() as usize - body.len();
    let count = body.matches("/FX").count();
    assert!(
        count > 0 && extra.is_multiple_of(count),
        "the first worker call names the temporary root"
    );
    let len = 3 + extra / count;
    format!("/{}", "r".repeat(len - 1))
}

/// The recorder wrote two forms of its temporary root as `/FX`: `$TMPDIR/ferbdx` and its resolved form, 8 characters
/// longer (`/private` first), which a file tool's resolved paths carry. Which `/FX` stood for which isn't recorded,
/// so where they mix, the budget report's byte count can only be checked up to those 8 characters per `/FX`: every
/// part but the report equal, and the report equal but for a count larger by a multiple of 8, at most 8 per `/FX`.
fn masked_root_only(expected: &Value, actual: &Value) -> bool {
    let (e, a) = (expected.as_array().unwrap(), actual.as_array().unwrap());
    if e.len() != a.len() || e[..e.len() - 1] != a[..a.len() - 1] {
        return false;
    }
    let read = |v: &Value| -> Value {
        serde_json::from_str(v.as_str().unwrap().trim_start_matches("Context budget: ")).unwrap()
    };
    let (mut re, mut ra) = (read(&e[e.len() - 1]), read(&a[a.len() - 1]));
    let (ce, ca) = (
        re["content_tokens_estimate"].as_u64().unwrap(),
        ra["content_tokens_estimate"].as_u64().unwrap(),
    );
    let roots: usize = e[..e.len() - 1]
        .iter()
        .map(|p| p.as_str().unwrap().matches("/FX").count())
        .sum();
    re["content_tokens_estimate"] = json!(0);
    ra["content_tokens_estimate"] = json!(0);
    re == ra && ce > ca && (ce - ca).is_multiple_of(8) && (ce - ca) / 8 <= roots as u64
}
