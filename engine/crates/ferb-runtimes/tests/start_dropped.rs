//! T-1349: a start that is dropped part way (the worker's silence watchdog gave up before the start returned) still
//! ends the model program it started, and leaves no run folder, run token or relay registration behind. The scripted
//! program never answers its first request, so the start is still waiting when it is dropped. No model.

mod support;

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ferb_core::types::{Identity, ModelRequest, Task, TaskRole};
use ferb_runtimes::run::Adapter;
use serde_json::json;
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

fn request() -> ModelRequest {
    ModelRequest {
        system: "FERB".into(),
        parts: vec!["ok no worries".into()],
        cacheable: 0,
        images: vec![],
    }
}

/// The program the start spawned (the version probe aside) ends within a second.
async fn ended(spawned: &Spawned) -> Option<i32> {
    for _ in 0..100 {
        let app = spawned.apps.lock().unwrap().last().cloned();
        if let Some(code) = app.as_ref().and_then(|a| a.exited()) {
            return Some(code);
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    None
}

fn left_in(tmp: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(tmp)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect()
}

// decision: RUN-420
#[tokio::test]
async fn a_codex_start_dropped_part_way_ends_its_app_server_and_folder() {
    use ferb_runtimes::codex::{CodexRuntime, Host, Mode};
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
            // `initialize` is never answered
            script: json!({"on": {}}),
            version: "codex-cli 0.166.0\n".into(),
            keying: Keying::Codex,
            spawned: spawned.clone(),
        }),
        temp_root: tmp.clone(),
        environ: [("PATH".to_string(), "/usr/bin:/bin".to_string())]
            .into_iter()
            .collect(),
        token_hex: counter(),
        today: Arc::new(|| "2026-11-28".into()),
        caches: Arc::new(Default::default()),
    };
    let mut rt = CodexRuntime::new(
        PathBuf::from("/opt/ferb/codex"),
        Mode::Worker,
        home,
        "test-model",
        &["memory.read".to_string()],
        host,
    );
    rt.shim_cmd = Some(vec!["/opt/ferb/bin/ferb".into(), "mcp".into()]);
    rt.monotonic = Arc::new(|| 0.0);
    let (t, r) = (task(), request());
    let start = rt.start(&t, &r, "tok");
    assert!(
        tokio::time::timeout(Duration::from_millis(300), start)
            .await
            .is_err(),
        "the start waits for initialize"
    );
    assert!(
        !spawned.apps.lock().unwrap().is_empty(),
        "the app-server was started"
    );
    assert_eq!(ended(&spawned).await, Some(-9), "the app-server is killed");
    for _ in 0..100 {
        if left_in(&tmp).is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(
        left_in(&tmp),
        Vec::<PathBuf>::new(),
        "the run's folder goes"
    );
}

struct Relay {
    forgot: Mutex<Vec<String>>,
}

impl ferb_runtimes::claude::ToolProxy for Relay {
    fn base_url(&self, run_id: &str) -> String {
        format!("http://127.0.0.1:9/run/{run_id}/fixed-secret")
    }
    fn seen(&self, _run_id: &str) -> BTreeSet<String> {
        BTreeSet::new()
    }
    fn forget(&self, run_id: &str) {
        self.forgot.lock().unwrap().push(run_id.to_string());
    }
}

// decision: RUN-420
#[tokio::test]
async fn a_claude_start_dropped_part_way_ends_its_program_token_and_relay_entry() {
    use ferb_runtimes::claude::{ClaudeRuntime, Host, Mode};
    use ferb_runtimes::forms::Refusals;
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let tmp = dir.path().join("tmp");
    std::fs::create_dir_all(home.join("claude-home")).unwrap();
    std::fs::write(home.join("claude-home/.ferb-signed-in"), "x\n").unwrap();
    std::fs::create_dir_all(&tmp).unwrap();
    let spawned = Arc::new(Spawned::default());
    let relay = Arc::new(Relay {
        forgot: Mutex::new(Vec::new()),
    });
    let host = Host {
        spawner: Arc::new(FakeSpawner {
            // the SDK's `initialize` control request is never answered
            script: json!({"on": {}}),
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
        today: Arc::new(|| "2026-11-28".into()),
    };
    let mut rt = ClaudeRuntime::new(
        Mode::Worker,
        relay.clone(),
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
    let (t, r) = (task(), request());
    let start = rt.start(&t, &r, "tok");
    assert!(
        tokio::time::timeout(Duration::from_millis(300), start)
            .await
            .is_err(),
        "the start waits for initialize"
    );
    assert!(
        !spawned.apps.lock().unwrap().is_empty(),
        "the program was started"
    );
    assert_eq!(ended(&spawned).await, Some(-9), "the program is killed");
    assert_eq!(
        left_in(&tmp),
        Vec::<PathBuf>::new(),
        "the run token's folder goes"
    );
    assert_eq!(
        relay.forgot.lock().unwrap().len(),
        1,
        "the relay forgets the run"
    );
}
