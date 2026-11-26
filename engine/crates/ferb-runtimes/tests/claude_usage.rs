//! ORCH-719 step 1 in the Claude adapter, on two recorded fake Claude Code runs (engine/fixtures/runtimes/claude.json)
//! with a stand-in relay that reports usage and refusals: RUN-477, a finished run's usage names how many model
//! requests made it, and a run that ends without Claude Code's result frame still hands over what the relay counted;
//! RUN-478, a refused edit is reported as a stopped compaction with its owner-facing line.

mod support;

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ferb_core::types::{Identity, RuntimeEventKind, Task, TaskRole};
use ferb_runtimes::claude::tally::{Tally, HISTORY_REFUSED_LINE};
use ferb_runtimes::claude::{ClaudeRuntime, Host, Mode, ToolProxy};
use ferb_runtimes::forms::Refusals;
use ferb_runtimes::run::{Adapter, Started};
use serde_json::{json, Value};
use support::*;

/// A relay that counted `tally` for the run (taken once) and refused `refusals` of its edits.
struct CountingRelay {
    tally: Mutex<Option<Tally>>,
    refusals: Mutex<usize>,
}

impl ToolProxy for CountingRelay {
    fn base_url(&self, run_id: &str) -> String {
        format!("http://127.0.0.1:9/run/{run_id}/fixed-secret")
    }
    fn seen(&self, _run_id: &str) -> BTreeSet<String> {
        ["mcp__ferb__mac_read".to_string()].into_iter().collect()
    }
    fn forget(&self, _run_id: &str) {}
    fn take_usage(&self, _run_id: &str) -> Option<Tally> {
        self.tally.lock().unwrap().take()
    }
    fn history_refusals(&self, _run_id: &str) -> usize {
        std::mem::take(&mut *self.refusals.lock().unwrap())
    }
}

fn counted() -> Tally {
    Tally {
        requests: 3,
        input: 30,
        cache_read: 9_000,
        cache_write: 1_200,
        cache_write_5m: 200,
        cache_write_1h: 1_000,
        output: 50,
        model: "claude-opus-5-5".into(),
    }
}

/// Starts the recorded scenario `name` with `relay`; the run's events until they end, and the started run.
async fn run(
    name: &str,
    relay: Arc<CountingRelay>,
) -> (Vec<(RuntimeEventKind, Value)>, Started, tempfile::TempDir) {
    let fx = fixture("claude.json");
    let sc = fx["cases"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| &c["scenario"])
        .find(|s| s["name"] == name)
        .unwrap_or_else(|| panic!("no recorded case {name}"))
        .clone();
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let tmp = dir.path().join("tmp");
    std::fs::create_dir_all(home.join("claude-home")).unwrap();
    std::fs::write(home.join("claude-home/.ferb-signed-in"), "x\n").unwrap();
    std::fs::create_dir_all(&tmp).unwrap();
    let spec = &sc["runtime"];
    let today = fx["today"].as_str().unwrap().to_string();
    let host = Host {
        spawner: Arc::new(FakeSpawner {
            script: sc["fake"].clone(),
            version: sc["version"].as_str().unwrap().to_string(),
            keying: Keying::Claude,
            spawned: Arc::new(Spawned::default()),
        }),
        temp_root: tmp,
        environ: env_of(&fx["base_env"]),
        token_hex: counter(),
        uuid4: Arc::new(|| uuid::Uuid::from_u128(1).to_string()),
        urandom4: Arc::new(|| "abcdef01".into()),
        today: Arc::new(move || today.clone()),
    };
    let names: Vec<String> = spec["tool_names"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s.as_str().unwrap().to_string())
        .collect();
    let mut rt = ClaudeRuntime::new(
        Mode::Worker,
        relay,
        home,
        &names,
        None,
        spec["model"].as_str().map(str::to_string),
        Arc::new(RecordedForms(fx["forms"].clone())),
        Arc::new(Refusals::new(Box::new(|| 0.0))),
        host,
    );
    rt.monotonic = Arc::new(|| 0.0);
    rt.shim_cmd = Some(vec!["/opt/ferb/bin/ferb".into(), "mcp".into()]);
    rt.cli = Some(PathBuf::from("/opt/ferb/claude"));
    let task = Task {
        id: "task-1".into(),
        parent_id: None,
        goal: "reply".into(),
        device_id: "device-1".into(),
        identity: Identity::Owner,
        scopes: BTreeSet::new(),
        role: TaskRole::Worker,
        request_id: "req-1".into(),
        success_conditions: vec![],
        deadline: None,
    };
    let mut started = rt
        .start(&task, &request_of(&sc["request"]), "tok")
        .await
        .expect("started");
    let mut events = Vec::new();
    while let Some(Ok(ev)) = tokio::time::timeout(Duration::from_secs(5), started.events.next())
        .await
        .expect("an event within 5 s")
    {
        events.push((ev.kind, Value::Object(ev.data)));
    }
    (events, started, dir)
}

// decision: RUN-477
#[tokio::test(flavor = "current_thread")]
async fn a_finished_runs_usage_names_its_model_requests_and_nothing_is_left_over() {
    let relay = Arc::new(CountingRelay {
        tally: Mutex::new(Some(counted())),
        refusals: Mutex::new(0),
    });
    let (events, started, _dir) =
        run("a worker calls FERB's tool, reports, and answers", relay).await;
    let usage = events
        .iter()
        .find(|(k, _)| *k == RuntimeEventKind::Usage)
        .map(|(_, d)| d.clone())
        .expect("a usage event");
    // Claude Code's own total is the record; the relay adds how many requests it took (RUN-421's normalizing)
    assert_eq!(usage["input_tokens"], json!(15));
    assert_eq!(usage["requests"], json!(3));
    started.control.cancel().await;
    assert!(started.control.unreported_usage().is_empty());
}

// decision: RUN-477
#[tokio::test(flavor = "current_thread")]
async fn a_run_that_ends_without_its_result_still_hands_over_what_the_relay_counted() {
    let relay = Arc::new(CountingRelay {
        tally: Mutex::new(Some(counted())),
        refusals: Mutex::new(0),
    });
    let (events, started, _dir) =
        run("the program ending with an error status mid-run", relay).await;
    assert!(
        !events.iter().any(|(k, _)| *k == RuntimeEventKind::Usage),
        "Claude Code never reported: {events:?}"
    );
    // the worker cancels the run as it closes it, then asks
    started.control.cancel().await;
    let left = started.control.unreported_usage();
    assert_eq!(left.len(), 1, "{left:?}");
    let u = &left[0];
    assert_eq!(u["model"], json!("claude-opus-5-5"));
    assert_eq!(u["input_tokens"], json!(30 + 9_000 + 1_200));
    assert_eq!(u["cached_input_tokens"], json!(9_000));
    assert_eq!(
        (&u["cache_write_5m_tokens"], &u["cache_write_1h_tokens"]),
        (&json!(200), &json!(1_000))
    );
    assert_eq!(u["output_tokens"], json!(50));
    assert_eq!(u["requests"], json!(3));
    assert_eq!(u["recorded_at_end"], json!(true));
    assert!(started.control.unreported_usage().is_empty(), "taken once");
}

// decision: RUN-478
#[tokio::test(flavor = "current_thread")]
async fn a_refused_edit_is_reported_with_its_owner_facing_line_before_the_runs_usage() {
    let relay = Arc::new(CountingRelay {
        tally: Mutex::new(None),
        refusals: Mutex::new(1),
    });
    let (events, started, _dir) =
        run("a worker calls FERB's tool, reports, and answers", relay).await;
    let kinds: Vec<RuntimeEventKind> = events.iter().map(|(k, _)| *k).collect();
    let at = kinds
        .iter()
        .position(|k| *k == RuntimeEventKind::Compacted)
        .expect("reported");
    assert_eq!(kinds[at + 1], RuntimeEventKind::Usage);
    assert_eq!(
        events[at].1,
        json!({"by": "ferb", "stopped": "history_refused", "said": HISTORY_REFUSED_LINE})
    );
    assert!(
        events[at + 1].1.get("requests").is_none(),
        "no count, no field"
    );
    started.control.cancel().await;
}
