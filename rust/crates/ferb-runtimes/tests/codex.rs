//! The Codex adapter against rust/fixtures/runtimes/codex.json: each scenario's script played by an in-process
//! fake app-server; argv, environment, working folder, stdin frames, events, errors, callbacks and the files left
//! behind must be what the Python adapter produced.

mod support;

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ferb_core::types::{Identity, Task, TaskRole};
use ferb_runtimes::codex::{self, CodexRuntime, Host, Mode};
use ferb_runtimes::run::Adapter;
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

async fn play(case: &Value, index: usize, base_env: &Value, today: &str) -> Value {
    let sc = &case["scenario"];
    let dir = tempfile::tempdir().expect("temp");
    let root = dir.path().to_path_buf();
    let home = root.join(format!("codex{index}")).join("home");
    let tmp = root.join("tmp");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&tmp).unwrap();
    write_files(&home, &sc["files"]);
    let paths = Paths {
        root: root.clone(),
        home: home.clone(),
        tmp: tmp.clone(),
    };
    let spawned = Arc::new(Spawned::default());
    let today = today.to_string();
    let host = Host {
        spawner: Arc::new(FakeSpawner {
            script: sc["fake"].clone(),
            version: sc["version"].as_str().unwrap().to_string(),
            keying: Keying::Codex,
            spawned: spawned.clone(),
        }),
        temp_root: tmp.clone(),
        environ: env_of(base_env),
        token_hex: counter(),
        today: Arc::new(move || today.clone()),
        caches: Arc::new(Default::default()),
    };
    let spec = &sc["runtime"];
    let mode = if spec["mode"] == "worker" {
        Mode::Worker
    } else {
        Mode::Supervisor
    };
    let tools: Vec<String> = spec["tool_names"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect();
    let mut rt = CodexRuntime::new(
        PathBuf::from("/opt/ferb/codex"),
        mode,
        home.clone(),
        spec["model"].as_str().unwrap(),
        &tools,
        host,
    );
    let notes: Arc<Mutex<Vec<Value>>> = Arc::new(Mutex::new(Vec::new()));
    rt.shim_cmd = spec["shim_cmd"]
        .as_array()
        .map(|a| a.iter().map(|v| v.as_str().unwrap().to_string()).collect());
    rt.output_schema = (!spec["output_schema"].is_null()).then(|| spec["output_schema"].clone());
    rt.timeout_s = spec["timeout_s"].as_f64();
    rt.effort = spec["effort"].as_str().map(str::to_string);
    rt.cache_role = spec["cache_role"].as_str().map(str::to_string);
    rt.cache_scope = spec["cache_scope"].as_str().unwrap_or_default().to_string();
    if let Some(r) = spec["cache_refusal"].as_str() {
        let r = r.to_string();
        rt.cache_refusal = Some(Arc::new(move |_t: &Task| r.clone()));
    }
    let pairs: Vec<(String, String)> = spec["redact"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| (p[0].as_str().unwrap().into(), p[1].as_str().unwrap().into()))
        .collect();
    rt.redact = Arc::new(move |s: &str| {
        let mut s = s.to_string();
        for (a, b) in &pairs {
            s = s.replace(a, b);
        }
        s
    });
    for (name, slot) in [("abandoned", 0), ("skipped", 1), ("fallback", 2)] {
        let n = notes.clone();
        let f: codex::TaskNote = Arc::new(move |t: &str, w: &str| {
            n.lock().unwrap().push(json!([name, t, w]));
        });
        match slot {
            0 => rt.on_abandoned = Some(f),
            1 => rt.on_cache_skipped = Some(f),
            _ => rt.on_cache_fallback = Some(f),
        }
    }
    rt.monotonic = Arc::new(|| 0.0);
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
                        "context_role": s.handle.state["context_role"]}),
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
            "push" => {
                let app = spawned
                    .apps
                    .lock()
                    .unwrap()
                    .last()
                    .cloned()
                    .expect("an app");
                app.apply(op["actions"].as_array().unwrap(), None).await;
                tokio::task::yield_now().await;
            }
            "cancel" => {
                started.as_ref().expect("started").control.cancel().await;
                log.push(json!({"op": "cancel"}));
            }
            "close" => {
                if let Some(s) = started.take() {
                    let control = s.control.clone();
                    s.events.close().await;
                    started = Some(ferb_runtimes::run::Started {
                        handle: s.handle,
                        events: ferb_runtimes::gen::EventStream::empty(),
                        control,
                    });
                }
                log.push(json!({"op": "close"}));
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
    let read = |p: PathBuf| std::fs::read_to_string(p).ok();
    let mut folders: Vec<String> = std::fs::read_dir(&tmp)
        .unwrap()
        .filter_map(Result::ok)
        .map(|e| e.file_name().to_string_lossy().to_string())
        .filter(|n| n.starts_with("ferb-codex-run-"))
        .collect();
    folders.sort();
    let after = json!({
        "auth": read(home.join("codex-home/auth.json")),
        "run_folders": folders,
        "locked_catalog": read(home.join("run/codex-models.json")),
        "user_home": home.join("codex-user").is_dir(),
    });
    let notes = notes.lock().unwrap().clone();
    paths.normalize(
        &json!({"name": sc["name"], "spawns": specs, "stdin": stdin, "log": log,
        "notes": notes, "after": after}),
    )
}

// decision: RUN-405, RUN-410, RUN-415, RUN-416, RUN-420, RUN-421, RUN-422, RUN-425, RUN-426, RUN-449, RUN-451
#[tokio::test(flavor = "current_thread")]
async fn codex_matches_python() {
    let fx = fixture("codex.json");
    let cases = fx["cases"].as_array().unwrap();
    let mut failures = Vec::new();
    for (i, case) in cases.iter().enumerate() {
        let got = play(case, i, &fx["base_env"], fx["today"].as_str().unwrap()).await;
        let want = &case["python"];
        if let Some(d) = first_diff(&got, want, "") {
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
