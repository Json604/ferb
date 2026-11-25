//! RUN-477 for Codex (ORCH-719 step 1): Codex reports each model call's usage as it happens; a call it reported that
//! the run's events never carried (the run stopped before its consumer read that far) is handed over at the end, one
//! record per call, never twice. The recorded fake app-server run "a typed answer streams its deltas, then usage and
//! the final answer" (engine/fixtures/runtimes/codex.json).

mod support;

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use ferb_core::types::{Identity, RuntimeEventKind, Task, TaskRole};
use ferb_runtimes::codex::{CodexRuntime, Host, Mode};
use ferb_runtimes::run::Adapter;
use serde_json::{json, Value};
use support::*;

const CASE: &str = "a typed answer streams its deltas, then usage and the final answer";

/// Starts the case, reads its first three events, then has the app report the call's usage and finish; `read_on`:
/// the consumer reads on to the end. The run is cancelled at the end; what it handed over is returned.
async fn played(read_on: bool) -> (Vec<RuntimeEventKind>, Vec<serde_json::Map<String, Value>>) {
    let fx = fixture("codex.json");
    let case = fx["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["scenario"]["name"] == CASE)
        .unwrap()
        .clone();
    let sc = &case["scenario"];
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let tmp = dir.path().join("tmp");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&tmp).unwrap();
    write_files(&home, &sc["files"]);
    let spawned = Arc::new(Spawned::default());
    let today = fx["today"].as_str().unwrap().to_string();
    let host = Host {
        spawner: Arc::new(FakeSpawner {
            script: sc["fake"].clone(),
            version: sc["version"].as_str().unwrap().to_string(),
            keying: Keying::Codex,
            spawned: spawned.clone(),
        }),
        temp_root: tmp,
        environ: env_of(&fx["base_env"]),
        token_hex: counter(),
        today: Arc::new(move || today.clone()),
        caches: Arc::new(Default::default()),
    };
    let mut rt = CodexRuntime::new(
        PathBuf::from("/opt/ferb/codex"),
        Mode::Supervisor,
        home,
        "test-model",
        &[],
        host,
    );
    rt.output_schema = Some(sc["runtime"]["output_schema"].clone());
    rt.monotonic = Arc::new(|| 0.0);
    let task = Task {
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
    };
    let mut started = rt
        .start(&task, &request_of(&sc["request"]), "")
        .await
        .expect("started");
    let mut kinds = Vec::new();
    for _ in 0..3 {
        let ev = tokio::time::timeout(Duration::from_secs(5), started.events.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        kinds.push(ev.kind);
    }
    let push = sc["ops"]
        .as_array()
        .unwrap()
        .iter()
        .find(|o| o["op"] == "push")
        .unwrap();
    let app = spawned.apps.lock().unwrap().last().cloned().unwrap();
    app.apply(push["actions"].as_array().unwrap(), None).await;
    if read_on {
        while let Some(Ok(ev)) = tokio::time::timeout(Duration::from_secs(5), started.events.next())
            .await
            .unwrap()
        {
            kinds.push(ev.kind);
        }
    } else {
        // the app's reader takes the lines in as they come, whether or not anyone reads the events
        for _ in 0..50 {
            tokio::task::yield_now().await;
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    }
    started.control.cancel().await;
    let left = started.control.unreported_usage();
    assert!(started.control.unreported_usage().is_empty(), "taken once");
    (kinds, left)
}

// decision: RUN-477
#[tokio::test(flavor = "current_thread")]
async fn a_call_codex_reported_that_no_event_carried_is_handed_over_at_the_end() {
    let (kinds, left) = played(false).await;
    assert!(!kinds.contains(&RuntimeEventKind::Usage));
    assert_eq!(left.len(), 1, "{left:?}");
    let u = &left[0];
    assert_eq!(
        (
            &u["model"],
            &u["input_tokens"],
            &u["cached_input_tokens"],
            &u["output_tokens"]
        ),
        (&json!("test-model"), &json!(8), &json!(2), &json!(4))
    );
    assert_eq!(u["recorded_at_end"], json!(true));
}

// decision: RUN-477
#[tokio::test(flavor = "current_thread")]
async fn a_call_the_events_carried_is_never_handed_over_again() {
    let (kinds, left) = played(true).await;
    assert!(kinds.contains(&RuntimeEventKind::Usage), "{kinds:?}");
    assert!(left.is_empty(), "{left:?}");
}
