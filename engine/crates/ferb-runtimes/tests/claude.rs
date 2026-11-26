//! The Claude adapter against engine/fixtures/runtimes/claude.json: what the Python adapter and the pinned SDK wrote
//! together (argv, environment, every stdin frame including the SDK's `initialize` and its answers to the
//! program's control requests) and every event, must be what this crate writes and yields with no SDK.

mod support;

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ferb_core::types::{Identity, ModelRequest, Task, TaskRole};
use ferb_runtimes::claude::{
    tail_after_marker, ClaudeRuntime, Host, Mode, ToolProxy, MESSAGES_TTL,
};
use ferb_runtimes::forms::Refusals;
use ferb_runtimes::run::Adapter;
use serde_json::{json, Value};
use support::*;

struct FakeProxy {
    seen: BTreeSet<String>,
    notes: Arc<Mutex<Vec<Value>>>,
}

impl ToolProxy for FakeProxy {
    fn base_url(&self, run_id: &str) -> String {
        format!("http://127.0.0.1:9/run/{run_id}/fixed-secret")
    }
    fn seen(&self, _run_id: &str) -> BTreeSet<String> {
        self.seen.clone()
    }
    fn forget(&self, run_id: &str) {
        self.notes.lock().unwrap().push(json!(["forget", run_id]));
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

async fn play(case: &Value, index: usize, fx: &Value) -> Value {
    let sc = &case["scenario"];
    let dir = tempfile::tempdir().expect("temp");
    let root = dir.path().to_path_buf();
    let home = root.join(format!("claude{index}")).join("home");
    let tmp = root.join("tmp");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&tmp).unwrap();
    let spec = &sc["runtime"];
    if spec["signed_in"] == true {
        std::fs::create_dir_all(home.join("claude-home")).unwrap();
        std::fs::write(home.join("claude-home/.ferb-signed-in"), "x\n").unwrap();
    }
    let paths = Paths {
        root: root.clone(),
        home: home.clone(),
        tmp: tmp.clone(),
    };
    let spawned = Arc::new(Spawned::default());
    let today = fx["today"].as_str().unwrap().to_string();
    let uuids = Arc::new(Mutex::new(0u128));
    let host = Host {
        spawner: Arc::new(FakeSpawner {
            script: sc["fake"].clone(),
            version: sc["version"].as_str().unwrap().to_string(),
            keying: Keying::Claude,
            spawned: spawned.clone(),
        }),
        temp_root: tmp.clone(),
        environ: env_of(&fx["base_env"]),
        token_hex: counter(),
        uuid4: Arc::new(move || {
            let mut n = uuids.lock().unwrap();
            *n += 1;
            uuid::Uuid::from_u128(*n).to_string()
        }),
        urandom4: Arc::new(|| "abcdef01".into()),
        today: Arc::new(move || today.clone()),
    };
    let notes = Arc::new(Mutex::new(Vec::new()));
    let proxy = Arc::new(FakeProxy {
        seen: strings(&spec["seen"]).into_iter().collect(),
        notes: notes.clone(),
    });
    let mode = if spec["mode"] == "worker" {
        Mode::Worker
    } else {
        Mode::Supervisor
    };
    let schema = (!spec["output_schema"].is_null()).then(|| spec["output_schema"].clone());
    let mut rt = ClaudeRuntime::new(
        mode,
        proxy,
        home.clone(),
        &strings(&spec["tool_names"]),
        schema,
        spec["model"].as_str().map(str::to_string),
        Arc::new(RecordedForms(fx["forms"].clone())),
        Arc::new(Refusals::new(Box::new(|| 0.0))),
        host,
    );
    rt.monotonic = Arc::new(|| 0.0);
    rt.shim_cmd = spec["shim_cmd"]
        .as_array()
        .map(|_| strings(&spec["shim_cmd"]));
    rt.effort = spec["effort"].as_str().map(str::to_string);
    rt.limit_s = spec["limit_s"].as_f64();
    rt.cli = (spec["cli"] == true).then(|| PathBuf::from("/opt/ferb/claude"));
    let request = request_of(&sc["request"]);
    let run_token = sc["run_token"].as_str().unwrap_or_default().to_string();
    let task = task();
    let mut log: Vec<Value> = Vec::new();
    let mut started = None;
    for op in sc["ops"].as_array().unwrap() {
        match op["op"].as_str().unwrap() {
            "start" => match rt.start(&task, &request, &run_token).await {
                Ok(s) => {
                    log.push(
                        json!({"op": "start", "start_steps": s.handle.state["start_steps"],
                        "typed": s.handle.state["typed"]}),
                    );
                    started = Some(s);
                }
                Err(e) => log.push(json!({"op": "start", "raised": raised(&e)})),
            },
            "events" => {
                let s = started.as_mut().expect("started");
                let mut count = 0;
                while op
                    .get("n")
                    .and_then(Value::as_u64)
                    .is_none_or(|n| count < n)
                {
                    let item = tokio::time::timeout(Duration::from_secs(5), s.events.next())
                        .await
                        .expect("an event within 5 s");
                    match item {
                        None => {
                            log.push(json!({"op": "end"}));
                            break;
                        }
                        Some(Err(e)) => {
                            log.push(json!({"op": "raised", "raised": raised(&e)}));
                            break;
                        }
                        Some(Ok(ev)) => {
                            log.push(json!({"op": "event", "event": event_text(&ev)}));
                            count += 1;
                        }
                    }
                }
            }
            "steer" => {
                let s = started.as_ref().expect("started");
                let out = tokio::time::timeout(
                    Duration::from_secs(5),
                    s.control.steer(op["text"].as_str().unwrap()),
                )
                .await
                .expect("steer answered");
                log.push(match out {
                    Ok(()) => json!({"op": "steer", "ok": true}),
                    Err(e) => json!({"op": "steer", "raised": raised(&e)}),
                });
            }
            "cancel" => {
                started.as_ref().expect("started").control.cancel().await;
                log.push(json!({"op": "cancel"}));
            }
            other => panic!("unknown op {other}"),
        }
    }
    if let Some(s) = &started {
        s.control.cancel().await;
    }
    let specs: Vec<Value> = spawned
        .specs
        .lock()
        .unwrap()
        .iter()
        .map(|s| {
            json!({"argv": s.argv, "env": s.env,
                "cwd": s.cwd.as_ref().map(|c| c.to_string_lossy().to_string())})
        })
        .collect();
    let stdin: Vec<String> = spawned
        .apps
        .lock()
        .unwrap()
        .iter()
        .flat_map(|a| a.lines.lock().unwrap().clone())
        .collect();
    let mut folders: Vec<String> = std::fs::read_dir(&tmp)
        .unwrap()
        .filter_map(Result::ok)
        .map(|e| e.file_name().to_string_lossy().to_string())
        .filter(|n| n.starts_with("ferb-claude-run-"))
        .collect();
    folders.sort();
    let notes = notes.lock().unwrap().clone();
    paths.normalize(
        &json!({"name": sc["name"], "spawns": specs, "stdin": stdin, "log": log,
        "notes": notes, "after": {"token_folders": folders}}),
    )
}

/// Whether a spawn is Claude Code itself (not its `-v` probe).
fn is_program(spawn: &Value) -> bool {
    spawn["argv"]
        .as_array()
        .is_some_and(|a| a.iter().any(|x| x == "--input-format"))
}

// decision: RUN-476
/// RUN-476's named difference (T-1324): a run whose request has text after FERB's marker starts Claude Code with its
/// message breakpoints at five minutes; Python's environment didn't name the setting. Nothing else in a recording
/// changes.
fn messages_ttl(mut python: Value, request: &ModelRequest) -> Value {
    if !tail_after_marker(request) {
        return python;
    }
    for spawn in python["spawns"].as_array_mut().into_iter().flatten() {
        if is_program(spawn) {
            spawn["env"][MESSAGES_TTL.0] = json!(MESSAGES_TTL.1);
        }
    }
    python
}

// decision: RUN-409, RUN-413, RUN-415, RUN-419, RUN-420, RUN-421, RUN-447, RUN-440, RUN-445, RUN-446, ENG-2
#[tokio::test(flavor = "current_thread")]
async fn claude_matches_python() {
    let fx = fixture("claude.json");
    let cases = fx["cases"].as_array().unwrap();
    let mut failures = Vec::new();
    for (i, case) in cases.iter().enumerate() {
        let got = play(case, i, &fx).await;
        let want = messages_ttl(
            case["python"].clone(),
            &request_of(&case["scenario"]["request"]),
        );
        if let Some(d) = first_diff(&got, &want, "") {
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

fn text_request(system: &str, parts: &[&str], cacheable: usize) -> ModelRequest {
    request_of(&json!({"system": system, "parts": parts, "cacheable": cacheable}))
}

// decision: RUN-476
#[test]
fn claude_codes_end_breakpoint_is_five_minutes_only_where_a_tail_follows_ferbs_marker() {
    // a planner turn: the stable prefix, then the live facts and the new message
    assert!(tail_after_marker(&text_request(
        "instructions",
        &["facts", "conversation", "now", "message"],
        2
    )));
    // no stable part: the marker is on the system text, the parts follow
    assert!(tail_after_marker(&text_request(
        "instructions",
        &["message"],
        0
    )));
    // no marker at all: everything is the tail
    assert!(tail_after_marker(&text_request("", &["message"], 0)));
    // pictures after a fully stable text are a tail of their own
    let mut pictured = request_of(&json!({"system": "s", "parts": ["a"], "cacheable": 1,
        "images": [{"data_b64": "aW1n", "ref": "r", "mime": "image/png"}]}));
    assert!(tail_after_marker(&pictured));
    // FERB's marker on the last block: Claude Code's would replace it, so the hour stays
    pictured.images.clear();
    assert!(!tail_after_marker(&pictured));
    assert!(!tail_after_marker(&text_request("instructions", &[], 0)));
    // Claude Code drops blank text blocks, so only blank text after the marker is no tail
    assert!(!tail_after_marker(&text_request("s", &["a", "  \n"], 1)));
}

// decision: RUN-476
#[tokio::test(flavor = "current_thread")]
async fn claude_code_starts_with_its_message_breakpoints_at_five_minutes_unless_ferbs_marker_ends_the_request(
) {
    let fx = fixture("claude.json");
    let case = fx["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| {
            c["scenario"]["name"] == "a planner turn with lookups, images, an alias and an effort"
        })
        .expect("the planner case")
        .clone();
    let program_env = |got: &Value| -> Vec<Value> {
        got["spawns"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|s| is_program(s))
            .map(|s| s["env"].clone())
            .collect()
    };
    let got = play(&case, 0, &fx).await;
    let envs = program_env(&got);
    assert_eq!(envs.len(), 1);
    assert_eq!(envs[0][MESSAGES_TTL.0], MESSAGES_TTL.1);
    // FERB's own marker keeps the hour
    assert_eq!(envs[0]["CLAUDE_CODE_PROMPT_CACHE_TTL"], "1h");
    let probe = &got["spawns"][0];
    assert!(!is_program(probe) && probe["env"].get(MESSAGES_TTL.0).is_none());

    let mut whole = case.clone();
    whole["scenario"]["request"] = json!({"system": "", "parts": ["a", "b"], "cacheable": 2});
    let got = play(&whole, 1, &fx).await;
    let envs = program_env(&got);
    assert_eq!(envs.len(), 1);
    assert!(envs[0].get(MESSAGES_TTL.0).is_none(), "{}", envs[0]);
    assert_eq!(envs[0]["CLAUDE_CODE_PROMPT_CACHE_TTL"], "1h");
}
