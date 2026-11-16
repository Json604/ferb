//! The checker's cached Codex thread: a worker-mode run with a cache role resumes FERB's fixed-id template thread (so
//! its first call carries a prompt_cache_key shared by every check on this Mac and model) with its whole context in
//! the user turn, a template holding only FERB's system prompt, and the effort the turn sends. The planner's cached
//! path is pinned by codex.rs (`codex_matches_python`, Python's recording, unchanged).

mod support;

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ferb_core::types::{Identity, RuntimeEventKind, Task, TaskRole};
use ferb_runtimes::codex::{self, CodexRuntime, Host, Mode};
use ferb_runtimes::run::{Adapter, Started};
use serde_json::{json, Value};
use support::*;

const MODEL: &str = "test-model";
const SCOPE: &str = "device-1";
const SYSTEM: &str = "FERB checker instructions";
const TRACE: &str = "TRACE {\"tools\":[{\"type\":\"function\",\"name\":\"mcp__ferb.mac_read\"}]}\n";

fn task() -> Task {
    Task {
        id: "task-1.verify".into(),
        parent_id: Some("task-1".into()),
        goal: "check the step".into(),
        device_id: SCOPE.into(),
        identity: Identity::Owner,
        scopes: BTreeSet::new(),
        role: TaskRole::Verifier,
        request_id: "req-1".into(),
        success_conditions: vec![],
        deadline: None,
    }
}

fn key() -> String {
    codex::cache_thread_id("checker", MODEL, SCOPE)
}

fn thread_result(id: &str, ephemeral: bool) -> Value {
    json!({"thread": {"id": id, "ephemeral": ephemeral}, "model": MODEL, "modelProvider": "ferb_chatgpt",
        "approvalPolicy": "never", "sandbox": {"type": "readOnly", "networkAccess": false}, "instructionSources": []})
}

/// The fake app-server: a resume answered with the checker's thread (or refused), an ephemeral start, a turn.
fn script(resume_refused: bool) -> Value {
    let resume = if resume_refused {
        json!({"id": "$ID", "error": {"code": -32603, "message": "no rollout"}})
    } else {
        json!({"id": "$ID", "result": thread_result(&key(), false)})
    };
    json!({"on": {
        "initialize": [[{"out": {"id": "$ID", "result": {}}}]],
        "thread/resume": [[{"out": resume}]],
        "thread/start": [[{"out": {"id": "$ID", "result": thread_result("th", true)}}]],
        "turn/start": [[{"err": TRACE}, {"out": {"id": "$ID", "result": {"turn": {"id": "tu", "status": "inProgress"}}}}]],
    }})
}

struct Fixture {
    _dir: tempfile::TempDir,
    tmp: PathBuf,
    spawned: Arc<Spawned>,
    notes: Arc<Mutex<Vec<Value>>>,
    rt: CodexRuntime,
}

fn fixture(script: Value, cache_role: Option<&str>, effort: Option<&str>) -> Fixture {
    let dir = tempfile::tempdir().expect("temp");
    let home = dir.path().join("home");
    let tmp = dir.path().join("tmp");
    std::fs::create_dir_all(home.join("codex-home")).unwrap();
    std::fs::create_dir_all(&tmp).unwrap();
    std::fs::write(
        home.join("codex-home/auth.json"),
        "{\"tokens\":{\"refresh_token\":\"test-only-fixture\"}}",
    )
    .unwrap();
    let spawned = Arc::new(Spawned::default());
    let host = Host {
        spawner: Arc::new(FakeSpawner {
            script,
            version: "codex-cli 0.166.0\n".into(),
            keying: Keying::Codex,
            spawned: spawned.clone(),
        }),
        temp_root: tmp.clone(),
        environ: Default::default(),
        token_hex: counter(),
        today: Arc::new(|| "2026-11-16".to_string()),
        caches: Arc::new(Default::default()),
    };
    let mut rt = CodexRuntime::new(
        PathBuf::from("/opt/ferb/codex"),
        Mode::Worker,
        home,
        MODEL,
        &["mac.read".to_string()],
        host,
    );
    rt.shim_cmd = Some(vec!["/opt/ferb/bin/ferb".into(), "mcp".into()]);
    rt.output_schema = Some(json!({"type": "object", "properties": {"ok": {"type": "boolean"}}}));
    rt.effort = effort.map(str::to_string);
    rt.cache_role = cache_role.map(str::to_string);
    rt.cache_scope = SCOPE.into();
    rt.monotonic = Arc::new(|| 0.0);
    let notes: Arc<Mutex<Vec<Value>>> = Arc::default();
    for (name, slot) in [("skipped", 0), ("fallback", 1)] {
        let n = notes.clone();
        let f: codex::TaskNote = Arc::new(move |t: &str, w: &str| {
            n.lock().unwrap().push(json!([name, t, w]));
        });
        match slot {
            0 => rt.on_cache_skipped = Some(f),
            _ => rt.on_cache_fallback = Some(f),
        }
    }
    // a checker's cache never asks the planner's developer-role question
    rt.cache_refusal = Some(Arc::new(|_t: &Task| {
        panic!("a worker-mode run asked for the refusal")
    }));
    Fixture {
        _dir: dir,
        tmp,
        spawned,
        notes,
        rt,
    }
}

fn request() -> ferb_core::types::ModelRequest {
    // stable parts exist, yet a worker-mode run never moves them to the developer role
    request_of(
        &json!({"system": SYSTEM, "parts": ["the step and its evidence", "the step's result"],
        "cacheable": 1}),
    )
}

/// The JSON-RPC requests the run sent, by method.
fn sent(f: &Fixture, method: &str) -> Vec<Value> {
    f.spawned
        .apps
        .lock()
        .unwrap()
        .iter()
        .flat_map(|a| a.lines.lock().unwrap().clone())
        .filter_map(|l| serde_json::from_str::<Value>(&l).ok())
        .filter(|m| m["method"] == method)
        .map(|m| m["params"].clone())
        .collect()
}

fn codex_home(f: &Fixture) -> PathBuf {
    let specs = f.spawned.specs.lock().unwrap();
    let app = specs
        .iter()
        .find(|s| s.argv.get(1).is_some_and(|a| a == "app-server"));
    PathBuf::from(&app.expect("an app-server").env["CODEX_HOME"])
}

async fn start(f: &Fixture) -> Started {
    f.rt.start(&task(), &request(), "private-token")
        .await
        .unwrap_or_else(|e| panic!("start: {e}"))
}

fn template(path: &Path) -> Vec<Value> {
    std::fs::read_to_string(path)
        .expect("the template exists while the run lives")
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

// decision: RUN-416, RUN-466, RUN-467
#[tokio::test(flavor = "current_thread")]
async fn a_checker_resumes_its_fixed_thread_with_its_whole_context_in_the_user_turn() {
    let f = fixture(script(false), Some("checker"), None);
    let mut started = start(&f).await;
    assert_eq!(started.handle.state["context_role"], "user");
    assert!(sent(&f, "thread/start").is_empty(), "no ephemeral thread");
    let [resume] = sent(&f, "thread/resume").try_into().expect("one resume");
    let home = codex_home(&f);
    assert_eq!(resume["threadId"], json!(key()));
    assert!(Path::new(resume["path"].as_str().unwrap()).starts_with(&home));
    assert_eq!(resume["developerInstructions"], "");
    assert_eq!(resume["baseInstructions"], SYSTEM);
    assert_eq!(
        (
            &resume["approvalPolicy"],
            &resume["sandbox"],
            &resume["model"],
            &resume["modelProvider"]
        ),
        (
            &json!("never"),
            &json!("read-only"),
            &json!(MODEL),
            &json!("ferb_chatgpt")
        )
    );
    let [turn] = sent(&f, "turn/start").try_into().expect("one turn");
    assert_eq!(turn["threadId"], json!(key()));
    assert_eq!(
        turn["input"],
        json!([{"type": "text", "text": "the step and its evidence\n\nthe step's result"}])
    );
    assert!(
        turn.get("effort").is_none(),
        "a checker without an effort sends none"
    );
    // the template holds FERB's own instructions only: no task, page or tool content
    let lines = template(Path::new(resume["path"].as_str().unwrap()));
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0]["type"], "session_meta");
    assert_eq!(lines[0]["payload"]["base_instructions"]["text"], SYSTEM);
    assert_eq!(lines[1]["type"], "turn_context");
    let text = lines.iter().map(|l| l.to_string()).collect::<String>();
    assert!(!text.contains("the step"), "no context in the template");
    assert_eq!(
        lines[1]["payload"]["effort"],
        Value::Null,
        "the effort the turn sends: none"
    );
    // the MCP tools and the read-only, never-approve settings are the command's, the same on either path
    let specs = f.spawned.specs.lock().unwrap().clone();
    let argv = &specs.last().unwrap().argv;
    assert!(argv
        .iter()
        .any(|a| a.starts_with("mcp_servers.ferb.command=")));
    assert!(f.notes.lock().unwrap().is_empty(), "no skip, no fallback");
    // the turn ends with its answer as on the ephemeral path
    let app = f.spawned.apps.lock().unwrap().last().cloned().unwrap();
    let k = key();
    app.apply(
        &[
            json!({"out": {"method": "item/completed", "params": {"threadId": k, "turnId": "tu", "item": {
                "type": "agentMessage", "id": "a", "phase": "final_answer", "text": "{\"ok\":true}"}}}}),
            json!({"out": {"method": "turn/completed", "params": {"threadId": k, "turn": {
                "id": "tu", "status": "completed", "error": null}}}}),
        ],
        None,
    )
    .await;
    let mut kinds = Vec::new();
    while let Some(item) = tokio::time::timeout(Duration::from_secs(5), started.events.next())
        .await
        .expect("an event within 5 s")
    {
        kinds.push(item.expect("no error").kind);
    }
    assert_eq!(kinds.last(), Some(&RuntimeEventKind::Final));
    started.control.cancel().await;
    assert!(
        !home.exists(),
        "the run's folder, its template with it, is gone"
    );
}

// decision: RUN-467
#[tokio::test(flavor = "current_thread")]
async fn the_template_effort_is_the_effort_the_turn_sends() {
    let f = fixture(script(false), Some("checker"), Some("high"));
    let started = start(&f).await;
    let [resume] = sent(&f, "thread/resume").try_into().expect("one resume");
    let [turn] = sent(&f, "turn/start").try_into().expect("one turn");
    assert_eq!(turn["effort"], "high");
    let lines = template(Path::new(resume["path"].as_str().unwrap()));
    assert_eq!(lines[1]["payload"]["effort"], "high");
    started.control.cancel().await;
}

// decision: RUN-416, RUN-467
#[tokio::test(flavor = "current_thread")]
async fn a_refused_resume_falls_back_to_the_ephemeral_thread_and_says_why() {
    let cached = fixture(script(true), Some("checker"), None);
    let started = start(&cached).await;
    assert_eq!(started.handle.state["context_role"], "user");
    assert_eq!(
        *cached.notes.lock().unwrap(),
        vec![json!([
            "fallback",
            "task-1.verify",
            "Codex could not complete thread/resume: no rollout (code -32603)"
        ])]
    );
    started.control.cancel().await;
    // the ephemeral path is today's, byte for byte: the same thread/start and turn/start as a run without a cache
    let plain = fixture(script(false), None, None);
    let p = start(&plain).await;
    assert!(sent(&plain, "thread/resume").is_empty());
    p.control.cancel().await;
    let strip = |v: Vec<Value>, root: &Path| -> Value {
        let text = serde_json::to_string(&v).unwrap();
        let paths = Paths {
            root: root.to_path_buf(),
            home: root.join("unused"),
            tmp: root.to_path_buf(),
        };
        serde_json::from_str(&paths.text(&text)).unwrap()
    };
    for method in ["thread/start", "turn/start"] {
        assert_eq!(
            strip(sent(&cached, method), &cached.tmp),
            strip(sent(&plain, method), &plain.tmp),
            "{method}"
        );
    }
    assert_eq!(sent(&cached, "turn/start")[0]["threadId"], "th");
}

// decision: RUN-467
#[tokio::test(flavor = "current_thread")]
async fn checks_at_once_each_write_their_own_template() {
    let a = fixture(script(false), Some("checker"), None);
    let mut b = fixture(script(false), Some("checker"), None);
    // both runs under one temporary folder, as the daemon's are
    b.rt.host.temp_root = a.tmp.clone();
    let (ra, rb) = tokio::join!(start(&a), start(&b));
    let pa = sent(&a, "thread/resume")[0]["path"]
        .as_str()
        .unwrap()
        .to_string();
    let pb = sent(&b, "thread/resume")[0]["path"]
        .as_str()
        .unwrap()
        .to_string();
    assert_ne!(pa, pb, "one template per run, in the run's own CODEX_HOME");
    assert_ne!(codex_home(&a), codex_home(&b));
    assert!(
        pa.contains(&key()) && pb.contains(&key()),
        "one thread id, one cache key"
    );
    assert!(Path::new(&pa).exists() && Path::new(&pb).exists());
    ra.control.cancel().await;
    assert!(
        Path::new(&pb).exists(),
        "one run's end leaves the other's template"
    );
    rb.control.cancel().await;
}
