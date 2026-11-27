//! T-1343 (kb/plans/front-door.md, "Keeping the runtime ready"; ORCH-726 step 6): the keeper and each runtime's
//! ready program, on scripted stand-ins with an injected clock and temporary folders only.
//!
//! The keeper's rules on a stand-in runtime (a second call takes the ready program, 5 idle minutes end it, other
//! settings end it, a crash is replaced, two calls at once get two programs, the switch off is today's start, a stop
//! ends what is idle), then the real adapters: Codex's kept app-server (the per-message recipe in order, the token
//! only while its message runs, two programs for two calls at once, today's bytes with the switch off), Claude Code
//! started ahead and used once, and the API loop's kept client.

mod support;

use std::any::Any;
use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ferb_core::interfaces::BoxFuture;
use ferb_core::types::{Identity, ModelRequest, RunHandle, RuntimeEventKind as K, Task, TaskRole};
use ferb_runtimes::codex::{self, CodexRuntime, Host, Mode};
use ferb_runtimes::error::RuntimeError;
use ferb_runtimes::gen::EventStream;
use ferb_runtimes::keep::{Back, Keepable, Keeper, Kept, Ready, IDLE_S};
use ferb_runtimes::run::{Adapter, RunControl, Started};
use serde_json::{json, Map, Value};
use support::*;

fn task(id: &str) -> Task {
    Task {
        id: id.into(),
        parent_id: None,
        goal: "reply".into(),
        device_id: "device-1".into(),
        identity: Identity::Owner,
        scopes: BTreeSet::new(),
        role: TaskRole::Mixed,
        request_id: format!("req-{id}"),
        success_conditions: vec![],
        deadline: None,
    }
}

/// The keeper's clock, moved by the test.
#[derive(Clone, Default)]
struct FakeClock(Arc<Mutex<f64>>);

impl FakeClock {
    fn set(&self, s: f64) {
        *self.0.lock().unwrap() = s;
    }
    fn clock(&self) -> ferb_runtimes::keep::Clock {
        let c = self.0.clone();
        Arc::new(move || *c.lock().unwrap())
    }
}

async fn until(what: &str, f: impl Fn() -> bool) {
    for _ in 0..500 {
        if f() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("never: {what}");
}

// ---------------------------------------------------------------------------------------------------------------
// the keeper on a stand-in runtime
// ---------------------------------------------------------------------------------------------------------------

/// A stand-in program: a number, alive until killed, ended once.
struct Program {
    n: usize,
    settings: String,
    alive: AtomicBool,
    ended: AtomicBool,
    /// The token of the message it runs ("" between messages).
    token: Mutex<String>,
}

impl Ready for Program {
    fn settings(&self) -> &str {
        &self.settings
    }
    fn alive(&self) -> bool {
        self.alive.load(Ordering::SeqCst) && !self.ended.load(Ordering::SeqCst)
    }
    fn end(self: Arc<Self>) -> BoxFuture<'static, ()> {
        Box::pin(async move {
            self.ended.store(true, Ordering::SeqCst);
        })
    }
    fn as_any(self: Arc<Self>) -> Arc<dyn Any + Send + Sync> {
        self
    }
}

/// A call's hold on a stand-in program: cancel hands it back with its token blanked.
struct Hold {
    program: Arc<Program>,
    back: Back,
    done: AtomicBool,
}

impl RunControl for Hold {
    fn steer<'a>(&'a self, _m: &'a str) -> BoxFuture<'a, Result<(), RuntimeError>> {
        Box::pin(async { Err(RuntimeError::unsupported("no")) })
    }
    fn cancel(&self) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            if !self.done.swap(true, Ordering::SeqCst) {
                self.program.token.lock().unwrap().clear();
                (self.back)(self.program.clone());
            }
        })
    }
}

#[derive(Default)]
struct Made {
    programs: Mutex<Vec<Arc<Program>>>,
    fresh_starts: AtomicUsize,
}

/// A stand-in runtime: `settings` names what shapes its program; `once` as Claude Code.
struct StandIn {
    settings: String,
    once: bool,
    made: Arc<Made>,
}

impl Adapter for StandIn {
    fn name(&self) -> &str {
        "stand-in"
    }
    fn start<'a>(
        &'a self,
        task: &'a Task,
        _request: &'a ModelRequest,
        _run_token: &'a str,
    ) -> BoxFuture<'a, Result<Started, RuntimeError>> {
        // today's start: a program of its own, ended with its message
        self.made.fresh_starts.fetch_add(1, Ordering::SeqCst);
        Box::pin(async move {
            Ok(Started {
                handle: RunHandle {
                    id: "today".into(),
                    runtime: "stand-in".into(),
                    task_id: task.id.clone(),
                    state: Map::new(),
                },
                events: EventStream::spawn(|_y| async { Ok(()) }),
                control: Arc::new(Today),
            })
        })
    }
}

struct Today;

impl RunControl for Today {
    fn steer<'a>(&'a self, _m: &'a str) -> BoxFuture<'a, Result<(), RuntimeError>> {
        Box::pin(async { Err(RuntimeError::unsupported("no")) })
    }
    fn cancel(&self) -> BoxFuture<'_, ()> {
        Box::pin(async {})
    }
}

impl Keepable for StandIn {
    fn keep_settings(&self, _r: Option<&ModelRequest>, has_token: bool) -> Option<String> {
        Some(format!("{}|{has_token}", self.settings))
    }
    fn prepare<'a>(
        &'a self,
        settings: &'a str,
        _back: Back,
    ) -> BoxFuture<'a, Result<Arc<dyn Ready>, RuntimeError>> {
        Box::pin(async move {
            let mut programs = self.made.programs.lock().unwrap();
            let p = Arc::new(Program {
                n: programs.len() + 1,
                settings: settings.to_string(),
                alive: AtomicBool::new(true),
                ended: AtomicBool::new(false),
                token: Mutex::new(String::new()),
            });
            programs.push(p.clone());
            Ok(p as Arc<dyn Ready>)
        })
    }
    fn take<'a>(
        &'a self,
        ready: Arc<dyn Ready>,
        back: Back,
        task: &'a Task,
        _request: &'a ModelRequest,
        run_token: &'a str,
    ) -> BoxFuture<'a, Result<Started, RuntimeError>> {
        Box::pin(async move {
            let program = ready.as_any().downcast::<Program>().expect("a stand-in");
            {
                let mut t = program.token.lock().unwrap();
                assert!(
                    t.is_empty(),
                    "a program took a message while another ran in it"
                );
                *t = run_token.to_string();
            }
            let mut state = Map::new();
            state.insert("program".into(), json!(program.n));
            let back: Back = if self.once {
                // used once: ended after its message, never handed back
                Arc::new(|r: Arc<dyn Ready>| {
                    tokio::spawn(r.end());
                })
            } else {
                back
            };
            Ok(Started {
                handle: RunHandle {
                    id: format!("run-{}", program.n),
                    runtime: "stand-in".into(),
                    task_id: task.id.clone(),
                    state,
                },
                events: EventStream::spawn(|_y| async { Ok(()) }),
                control: Arc::new(Hold {
                    program,
                    back,
                    done: AtomicBool::new(false),
                }),
            })
        })
    }
    fn once(&self) -> bool {
        self.once
    }
}

fn plain_request() -> ModelRequest {
    ModelRequest {
        system: "FERB".into(),
        parts: vec!["ok no worries".into()],
        cacheable: 0,
        images: vec![],
    }
}

fn stand_in(keeper: &Keeper, settings: &str, made: &Arc<Made>) -> Kept {
    Kept::new(
        keeper.clone(),
        "front",
        Arc::new(StandIn {
            settings: settings.into(),
            once: false,
            made: made.clone(),
        }),
    )
}

fn program_of(s: &Started) -> Value {
    s.handle
        .state
        .get("program")
        .cloned()
        .unwrap_or(Value::Null)
}

// decision: ORCH-726, RUN-481, RUN-482
#[tokio::test(flavor = "current_thread")]
async fn a_second_call_takes_the_ready_program_and_its_token_lives_only_while_its_message_runs() {
    let clock = FakeClock::default();
    let keeper = Keeper::new(clock.clock());
    let made = Arc::new(Made::default());
    let kept = stand_in(&keeper, "model-a", &made);
    let first = kept
        .start(&task("t1"), &plain_request(), "tok-1")
        .await
        .unwrap();
    let p1 = made.programs.lock().unwrap()[0].clone();
    assert_eq!(*p1.token.lock().unwrap(), "tok-1");
    first.control.cancel().await;
    assert_eq!(
        *p1.token.lock().unwrap(),
        "",
        "the token outlived its message"
    );
    assert!(keeper.has_idle("front"));
    clock.set(IDLE_S - 1.0);
    let second = kept
        .start(&task("t2"), &plain_request(), "tok-2")
        .await
        .unwrap();
    assert_eq!(
        program_of(&second),
        json!(1),
        "the ready program was not taken"
    );
    assert_eq!(
        made.programs.lock().unwrap().len(),
        1,
        "a new program was started"
    );
    assert_eq!(*p1.token.lock().unwrap(), "tok-2");
    // a late cancel of the first message does nothing to the program now running the second
    first.control.cancel().await;
    assert_eq!(*p1.token.lock().unwrap(), "tok-2");
    second.control.cancel().await;
    assert_eq!(made.fresh_starts.load(Ordering::SeqCst), 0);
}

// decision: ORCH-726, RUN-481
#[tokio::test(flavor = "current_thread")]
async fn five_idle_minutes_end_the_ready_program() {
    let clock = FakeClock::default();
    let keeper = Keeper::new(clock.clock());
    let made = Arc::new(Made::default());
    let kept = stand_in(&keeper, "model-a", &made);
    let s = kept
        .start(&task("t1"), &plain_request(), "tok")
        .await
        .unwrap();
    clock.set(10.0);
    s.control.cancel().await;
    let p1 = made.programs.lock().unwrap()[0].clone();
    clock.set(10.0 + IDLE_S - 0.5);
    keeper.sweep();
    assert!(keeper.has_idle("front") && !p1.ended.load(Ordering::SeqCst));
    clock.set(10.0 + IDLE_S);
    keeper.sweep();
    assert!(!keeper.has_idle("front"));
    until("the idle program ended", || p1.ended.load(Ordering::SeqCst)).await;
    // the next call starts a new one
    let s = kept
        .start(&task("t2"), &plain_request(), "tok")
        .await
        .unwrap();
    assert_eq!(program_of(&s), json!(2));
}

// decision: ORCH-726, RUN-481
#[tokio::test(flavor = "current_thread")]
async fn a_change_of_model_or_settings_ends_the_ready_program() {
    let clock = FakeClock::default();
    let keeper = Keeper::new(clock.clock());
    let made = Arc::new(Made::default());
    let s = stand_in(&keeper, "model-a", &made)
        .start(&task("t1"), &plain_request(), "tok")
        .await
        .unwrap();
    s.control.cancel().await;
    let p1 = made.programs.lock().unwrap()[0].clone();
    let s = stand_in(&keeper, "model-b", &made)
        .start(&task("t2"), &plain_request(), "tok")
        .await
        .unwrap();
    assert_eq!(
        program_of(&s),
        json!(2),
        "the other model's program was taken"
    );
    until("the old model's program ended", || {
        p1.ended.load(Ordering::SeqCst)
    })
    .await;
    s.control.cancel().await;
    // a call without tools is another program too
    let s = stand_in(&keeper, "model-b", &made)
        .start(&task("t3"), &plain_request(), "")
        .await
        .unwrap();
    assert_eq!(program_of(&s), json!(3));
}

// decision: ORCH-726, RUN-481
#[tokio::test(flavor = "current_thread")]
async fn a_crashed_ready_program_is_replaced() {
    let clock = FakeClock::default();
    let keeper = Keeper::new(clock.clock());
    let made = Arc::new(Made::default());
    let kept = stand_in(&keeper, "model-a", &made);
    let s = kept
        .start(&task("t1"), &plain_request(), "tok")
        .await
        .unwrap();
    s.control.cancel().await;
    let p1 = made.programs.lock().unwrap()[0].clone();
    p1.alive.store(false, Ordering::SeqCst);
    keeper.sweep();
    until("a new program is ready", || {
        made.programs.lock().unwrap().len() == 2 && keeper.has_idle("front")
    })
    .await;
    assert!(p1.ended.load(Ordering::SeqCst));
    let s = kept
        .start(&task("t2"), &plain_request(), "tok")
        .await
        .unwrap();
    assert_eq!(program_of(&s), json!(2));
    // found dead by a call instead: that call gets a new one
    s.control.cancel().await;
    made.programs.lock().unwrap()[1]
        .alive
        .store(false, Ordering::SeqCst);
    let s = kept
        .start(&task("t3"), &plain_request(), "tok")
        .await
        .unwrap();
    assert_eq!(program_of(&s), json!(3));
}

// decision: ORCH-726, RUN-481, RUN-482
#[tokio::test(flavor = "current_thread")]
async fn two_calls_at_once_never_share_a_program_or_a_token_and_only_one_stays_idle() {
    let clock = FakeClock::default();
    let keeper = Keeper::new(clock.clock());
    let made = Arc::new(Made::default());
    let kept = stand_in(&keeper, "model-a", &made);
    let a = kept
        .start(&task("ta"), &plain_request(), "tok-a")
        .await
        .unwrap();
    let b = kept
        .start(&task("tb"), &plain_request(), "tok-b")
        .await
        .unwrap();
    assert_ne!(program_of(&a), program_of(&b));
    let programs = made.programs.lock().unwrap().clone();
    assert_eq!(
        (
            programs[0].token.lock().unwrap().clone(),
            programs[1].token.lock().unwrap().clone()
        ),
        ("tok-a".to_string(), "tok-b".to_string())
    );
    a.control.cancel().await;
    b.control.cancel().await;
    // at most one ready program per role beside those in use
    until("the second one ended", || {
        programs[1].ended.load(Ordering::SeqCst)
    })
    .await;
    assert!(keeper.has_idle("front") && !programs[0].ended.load(Ordering::SeqCst));
}

// decision: ORCH-726, RUN-481
#[tokio::test(flavor = "current_thread")]
async fn switched_off_every_call_is_todays_start_and_the_idle_program_ends() {
    let clock = FakeClock::default();
    let keeper = Keeper::new(clock.clock());
    let made = Arc::new(Made::default());
    let kept = stand_in(&keeper, "model-a", &made);
    let s = kept
        .start(&task("t1"), &plain_request(), "tok")
        .await
        .unwrap();
    s.control.cancel().await;
    keeper.set_on(false);
    let p1 = made.programs.lock().unwrap()[0].clone();
    until("the idle program ended", || p1.ended.load(Ordering::SeqCst)).await;
    for n in 0..2 {
        let s = kept
            .start(&task("t2"), &plain_request(), "tok")
            .await
            .unwrap();
        assert_eq!(s.handle.id, "today");
        s.control.cancel().await;
        assert_eq!(made.fresh_starts.load(Ordering::SeqCst), n + 1);
    }
    assert_eq!(made.programs.lock().unwrap().len(), 1);
    assert!(!keeper.has_idle("front"));
}

// decision: ORCH-726, RUN-481
#[tokio::test(flavor = "current_thread")]
async fn a_run_once_program_is_replaced_ahead_and_a_stop_ends_what_is_idle() {
    let clock = FakeClock::default();
    let keeper = Keeper::new(clock.clock());
    let made = Arc::new(Made::default());
    let kept = Kept::new(
        keeper.clone(),
        "front",
        Arc::new(StandIn {
            settings: "claude".into(),
            once: true,
            made: made.clone(),
        }),
    );
    let s = kept
        .start(&task("t1"), &plain_request(), "tok")
        .await
        .unwrap();
    until("the next program is ready ahead", || {
        keeper.has_idle("front")
    })
    .await;
    s.control.cancel().await;
    let programs = made.programs.lock().unwrap().clone();
    assert_eq!(programs.len(), 2);
    until("the used one ended", || {
        programs[0].ended.load(Ordering::SeqCst)
    })
    .await;
    let s = kept
        .start(&task("t2"), &plain_request(), "tok")
        .await
        .unwrap();
    assert_eq!(
        program_of(&s),
        json!(2),
        "the program made ahead was not taken"
    );
    s.control.cancel().await;
    until("the third is ready", || keeper.has_idle("front")).await;
    keeper.shut_down().await;
    let programs = made.programs.lock().unwrap().clone();
    assert!(programs.iter().all(|p| p.ended.load(Ordering::SeqCst)));
    // after the stop nothing is kept
    let s = kept
        .start(&task("t3"), &plain_request(), "tok")
        .await
        .unwrap();
    assert_eq!(s.handle.id, "today");
}

// ---------------------------------------------------------------------------------------------------------------
// Codex's kept app-server
// ---------------------------------------------------------------------------------------------------------------

const MODEL: &str = "test-model";
const SCOPE: &str = "device-1";
const TRACE: &str =
    "TRACE {\"tools\":[{\"type\":\"function\",\"name\":\"mcp__ferb.memory_read\"}]}\n";

fn key() -> String {
    codex::cache_thread_id("planner", MODEL, SCOPE)
}

fn thread_result(id: &str, ephemeral: bool) -> Value {
    json!({"thread": {"id": id, "ephemeral": ephemeral}, "model": MODEL, "modelProvider": "ferb_chatgpt",
        "approvalPolicy": "never", "sandbox": {"type": "readOnly", "networkAccess": false}, "instructionSources": []})
}

fn note(method: &str, turn: &str, params: Value) -> Value {
    let mut p = params;
    p["threadId"] = json!(key());
    if method != "turn/completed" {
        p["turnId"] = json!(turn);
    }
    json!({"out": {"method": method, "params": p}})
}

/// One message's turn `n`: its trace, the answer "answer n", its usage, its end.
fn turn(n: usize) -> Vec<Value> {
    let tu = format!("tu{n}");
    let text = format!("answer {n}");
    vec![
        json!({"err": TRACE}),
        json!({"out": {"id": "$ID", "result": {"turn": {"id": tu, "status": "inProgress"}}}}),
        note(
            "item/started",
            &tu,
            json!({"item": {"type": "agentMessage", "id": format!("a{n}"), "phase": "final_answer", "text": ""}}),
        ),
        note(
            "item/agentMessage/delta",
            &tu,
            json!({"itemId": format!("a{n}"), "delta": text}),
        ),
        note(
            "item/completed",
            &tu,
            json!({"item": {"type": "agentMessage", "id": format!("a{n}"), "phase": "final_answer", "text": text}}),
        ),
        note(
            "thread/tokenUsage/updated",
            &tu,
            json!({"tokenUsage": {"last": {"inputTokens": 100 + n, "cachedInputTokens": 90, "outputTokens": 4,
            "reasoningOutputTokens": 0}}}),
        ),
        note(
            "turn/completed",
            &tu,
            json!({"turn": {"id": tu, "status": "completed", "error": null}}),
        ),
    ]
}

/// A Codex app-server answering `turns` messages on the planner's cached thread.
fn codex_script(turns: usize) -> Value {
    let resumes: Vec<Value> = (0..turns)
        .map(|_| json!([{"out": {"id": "$ID", "result": thread_result(&key(), false)}}]))
        .collect();
    let unsubscribes: Vec<Value> = (0..turns)
        .map(|_| json!([{"out": {"id": "$ID", "result": {"status": "unsubscribed"}}}]))
        .collect();
    let starts: Vec<Value> = (1..=turns).map(|n| Value::Array(turn(n))).collect();
    json!({"on": {
        "initialize": [[{"out": {"id": "$ID", "result": {}}}]],
        "thread/resume": resumes,
        "thread/unsubscribe": unsubscribes,
        "turn/start": starts,
    }})
}

struct Cx {
    _dir: tempfile::TempDir,
    tmp: PathBuf,
    spawned: Arc<Spawned>,
    rt: CodexRuntime,
}

fn cx(script: Value) -> Cx {
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
        environ: [("PATH".to_string(), "/usr/bin:/bin".to_string())]
            .into_iter()
            .collect(),
        token_hex: counter(),
        today: Arc::new(|| "2026-11-27".to_string()),
        caches: Arc::new(Default::default()),
    };
    let mut rt = CodexRuntime::new(
        PathBuf::from("/opt/ferb/codex"),
        Mode::Supervisor,
        home,
        MODEL,
        &["memory.read".to_string()],
        host,
    );
    rt.shim_cmd = Some(vec!["/opt/ferb/bin/ferb".into(), "mcp".into()]);
    rt.cache_role = Some("planner".into());
    rt.cache_scope = SCOPE.into();
    rt.monotonic = Arc::new(|| 0.0);
    Cx {
        _dir: dir,
        tmp,
        spawned,
        rt,
    }
}

fn cached_request(message: &str) -> ModelRequest {
    ModelRequest {
        system: "FERB front door".into(),
        parts: vec!["what FERB knows (stable)".into(), message.into()],
        cacheable: 1,
        images: vec![],
    }
}

/// The run's answer, usage and failure events until the stream ends.
async fn drain(started: &mut Started) -> Vec<Value> {
    let mut out = Vec::new();
    while let Some(item) = tokio::time::timeout(Duration::from_secs(5), started.events.next())
        .await
        .expect("an event within 5 s")
    {
        let ev = item.expect("no error raised");
        match ev.kind {
            K::Final => out.push(json!({"final": ev.data["text"]})),
            K::Usage => out.push(json!({"usage": ev.data["input_tokens"]})),
            K::Error | K::Violation => out.push(json!({"failed": Value::Object(ev.data)})),
            _ => {}
        }
    }
    out
}

fn methods(app: &FakeApp) -> Vec<String> {
    app.lines
        .lock()
        .unwrap()
        .iter()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .map(|m| m["method"].as_str().unwrap_or("").to_string())
        .collect()
}

fn token_at(s: &Started) -> String {
    let path = s.handle.state["token_file"].as_str().expect("a token file");
    std::fs::read_to_string(path).unwrap()
}

fn apps(spawned: &Spawned) -> Vec<Arc<FakeApp>> {
    spawned.apps.lock().unwrap().clone()
}

// decision: ORCH-726, RUN-481
#[tokio::test(flavor = "current_thread")]
async fn a_kept_codex_takes_each_message_by_the_recipe_and_leaves_nothing_of_the_last_one() {
    let c = cx(codex_script(2));
    let keeper = Keeper::new(FakeClock::default().clock());
    let kept = Kept::new(keeper.clone(), "planner", Arc::new(c.rt.clone()));
    let mut first = kept
        .start(&task("t1"), &cached_request("ok no worries"), "tok-1")
        .await
        .expect("started");
    assert_eq!(token_at(&first), "tok-1");
    assert_eq!(first.handle.state["kept"], json!("new"));
    assert_eq!(
        drain(&mut first).await,
        vec![json!({"usage": 101}), json!({"final": "answer 1"})]
    );
    first.control.cancel().await;
    assert_eq!(token_at(&first), "", "the token outlived its message");
    assert!(keeper.has_idle("planner"));
    let app = apps(&c.spawned)[0].clone();
    assert!(app.exited().is_none(), "the kept app-server was ended");
    // what Codex would have appended to the template thread: the next message must not see it
    let template = {
        let lines = app.lines.lock().unwrap().clone();
        let resume: Value = lines
            .iter()
            .filter_map(|l| serde_json::from_str::<Value>(l).ok())
            .find(|m| m["method"] == "thread/resume")
            .unwrap();
        PathBuf::from(resume["params"]["path"].as_str().unwrap())
    };
    let mut appended = std::fs::read_to_string(&template).unwrap();
    appended.push_str("{\"type\":\"response_item\",\"payload\":\"the first message's turn\"}\n");
    std::fs::write(&template, appended).unwrap();

    let mut second = kept
        .start(&task("t2"), &cached_request("and the code word?"), "tok-2")
        .await
        .expect("started");
    assert_eq!(second.handle.state["kept"], json!("ready"));
    assert_eq!(token_at(&second), "tok-2");
    assert_eq!(
        drain(&mut second).await,
        vec![json!({"usage": 102}), json!({"final": "answer 2"})],
        "the second message carried something of the first"
    );
    second.control.cancel().await;
    assert_eq!(apps(&c.spawned).len(), 1, "a second app-server was started");
    assert_eq!(
        methods(&app),
        [
            "initialize",
            "initialized",
            "thread/resume",
            "turn/start",
            "thread/unsubscribe",
            "thread/resume",
            "turn/start",
            "thread/unsubscribe"
        ],
        "not the measured recipe, in order"
    );
    // both resumes named the same cached thread, and the second read a template written again: two lines
    let lines = app.lines.lock().unwrap().clone();
    let resumes: Vec<Value> = lines
        .iter()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .filter(|m| m["method"] == "thread/resume")
        .collect();
    assert!(resumes
        .iter()
        .all(|r| r["params"]["threadId"] == json!(key())));
    let read = app.read.lock().unwrap().clone();
    assert_eq!(read.len(), 2);
    assert_eq!(
        read[1].lines().count(),
        2,
        "the template was not written again"
    );
    assert_eq!(read[0], read[1]);
    let unsubscribed: Vec<Value> = lines
        .iter()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .filter(|m| m["method"] == "thread/unsubscribe")
        .map(|m| m["params"].clone())
        .collect();
    assert_eq!(unsubscribed[0], json!({"threadId": key()}));
    // the second turn's input is its own message only
    let turn_inputs: Vec<String> = lines
        .iter()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .filter(|m| m["method"] == "turn/start")
        .map(|m| {
            m["params"]["input"][0]["text"]
                .as_str()
                .unwrap()
                .to_string()
        })
        .collect();
    assert_eq!(turn_inputs, ["ok no worries", "and the code word?"]);
    keeper.shut_down().await;
    assert!(
        app.exited().is_some(),
        "the stop left the app-server running"
    );
    let left: Vec<_> = std::fs::read_dir(&c.tmp).unwrap().collect();
    assert!(left.is_empty(), "the program's folder is left");
}

// decision: ORCH-726, RUN-481
#[tokio::test(flavor = "current_thread")]
async fn two_codex_calls_at_once_get_two_app_servers_and_two_token_files() {
    let one = codex_script(1);
    let c = cx(json!({"apps": [one.clone(), one]}));
    let keeper = Keeper::new(FakeClock::default().clock());
    let kept = Kept::new(keeper.clone(), "planner", Arc::new(c.rt.clone()));
    let mut a = kept
        .start(&task("ta"), &cached_request("first"), "tok-a")
        .await
        .unwrap();
    let mut b = kept
        .start(&task("tb"), &cached_request("second"), "tok-b")
        .await
        .unwrap();
    assert_eq!(apps(&c.spawned).len(), 2);
    assert_ne!(a.handle.state["token_file"], b.handle.state["token_file"]);
    assert_eq!(
        (token_at(&a), token_at(&b)),
        ("tok-a".into(), "tok-b".into())
    );
    drain(&mut a).await;
    drain(&mut b).await;
    a.control.cancel().await;
    b.control.cancel().await;
    // one stays ready, the other ended
    until("one app-server ended", || {
        apps(&c.spawned)
            .iter()
            .filter(|a| a.exited().is_some())
            .count()
            == 1
    })
    .await;
    assert!(keeper.has_idle("planner"));
    keeper.shut_down().await;
    assert!(apps(&c.spawned).iter().all(|a| a.exited().is_some()));
}

// decision: ORCH-726, RUN-481
#[tokio::test(flavor = "current_thread")]
async fn a_kept_codex_ends_on_another_effort_and_after_five_idle_minutes() {
    let one = codex_script(1);
    let c = cx(json!({"apps": [one.clone(), one]}));
    let clock = FakeClock::default();
    let keeper = Keeper::new(clock.clock());
    let mut s = Kept::new(keeper.clone(), "planner", Arc::new(c.rt.clone()))
        .start(&task("t1"), &cached_request("hi"), "tok")
        .await
        .unwrap();
    drain(&mut s).await;
    s.control.cancel().await;
    let mut high = c.rt.clone();
    high.effort = Some("high".into());
    let mut s = Kept::new(keeper.clone(), "planner", Arc::new(high))
        .start(&task("t2"), &cached_request("hi"), "tok")
        .await
        .unwrap();
    assert_eq!(
        apps(&c.spawned).len(),
        2,
        "the other effort took the kept program"
    );
    until("the first app-server ended", || {
        apps(&c.spawned)[0].exited().is_some()
    })
    .await;
    drain(&mut s).await;
    s.control.cancel().await;
    clock.set(IDLE_S);
    keeper.sweep();
    until("the idle app-server ended", || {
        apps(&c.spawned)[1].exited().is_some()
    })
    .await;
    until("its folder is gone", || {
        std::fs::read_dir(&c.tmp).unwrap().next().is_none()
    })
    .await;
}

// decision: ORCH-726, RUN-481
#[tokio::test(flavor = "current_thread")]
async fn a_kept_codex_that_crashed_while_idle_is_replaced() {
    let one = codex_script(1);
    let c = cx(json!({"apps": [one.clone(), one]}));
    let keeper = Keeper::new(FakeClock::default().clock());
    let kept = Kept::new(keeper.clone(), "planner", Arc::new(c.rt.clone()));
    let mut s = kept
        .start(&task("t1"), &cached_request("hi"), "tok")
        .await
        .unwrap();
    drain(&mut s).await;
    s.control.cancel().await;
    apps(&c.spawned)[0].kill();
    until("the crash is seen", || {
        keeper.sweep();
        apps(&c.spawned).len() == 2
    })
    .await;
    until("its replacement is ready", || keeper.has_idle("planner")).await;
    let mut s = kept
        .start(&task("t2"), &cached_request("hi"), "tok")
        .await
        .unwrap();
    assert_eq!(
        drain(&mut s).await.last().unwrap(),
        &json!({"final": "answer 1"})
    );
    s.control.cancel().await;
    assert_eq!(apps(&c.spawned).len(), 2);
    keeper.shut_down().await;
}

// decision: ORCH-726, RUN-481
#[tokio::test(flavor = "current_thread")]
async fn a_kept_codex_whose_sign_in_changed_is_not_taken() {
    let one = codex_script(1);
    let c = cx(json!({"apps": [one.clone(), one]}));
    let keeper = Keeper::new(FakeClock::default().clock());
    let kept = Kept::new(keeper.clone(), "planner", Arc::new(c.rt.clone()));
    let mut s = kept
        .start(&task("t1"), &cached_request("hi"), "tok")
        .await
        .unwrap();
    drain(&mut s).await;
    s.control.cancel().await;
    // another run (or a new sign-in) replaced FERB's sign-in: this program's copy is stale
    std::fs::write(
        c.rt.ferb_home.join("codex-home/auth.json"),
        "{\"tokens\":{\"refresh_token\":\"test-only-rotated\"}}",
    )
    .unwrap();
    let mut s = kept
        .start(&task("t2"), &cached_request("hi"), "tok")
        .await
        .unwrap();
    assert_eq!(apps(&c.spawned).len(), 2);
    drain(&mut s).await;
    s.control.cancel().await;
    until("the stale one ended", || {
        apps(&c.spawned)[0].exited().is_some()
    })
    .await;
    keeper.shut_down().await;
}

// decision: ORCH-726, RUN-481
#[tokio::test(flavor = "current_thread")]
async fn switched_off_a_codex_call_is_todays_bytes_and_its_program_ends_with_it() {
    let today = cx(codex_script(1));
    let mut s = today
        .rt
        .start(&task("t1"), &cached_request("hi"), "tok")
        .await
        .unwrap();
    let told_today = drain(&mut s).await;
    s.control.cancel().await;

    let off = cx(codex_script(1));
    let keeper = Keeper::new(FakeClock::default().clock());
    keeper.set_on(false);
    let kept = Kept::new(keeper.clone(), "planner", Arc::new(off.rt.clone()));
    let mut k = kept
        .start(&task("t1"), &cached_request("hi"), "tok")
        .await
        .unwrap();
    assert_eq!(drain(&mut k).await, told_today);
    k.control.cancel().await;
    let (a, b) = (
        apps(&today.spawned)[0].clone(),
        apps(&off.spawned)[0].clone(),
    );
    assert!(b.exited().is_some(), "the program outlived its message");
    // the run folder's name is random (`tempfile.TemporaryDirectory`): the rest is byte for byte
    let folder = regex::Regex::new("ferb-codex-run-[0-9a-f]{8}").unwrap();
    let norm = |app: &FakeApp, tmp: &std::path::Path| -> Vec<String> {
        let t = tmp.to_string_lossy().to_string();
        app.lines
            .lock()
            .unwrap()
            .iter()
            .map(|l| {
                folder
                    .replace_all(&l.replace(&t, "TMP"), "RUN")
                    .into_owned()
            })
            .collect()
    };
    assert_eq!(norm(&a, &today.tmp), norm(&b, &off.tmp));
    let argv = |sp: &Spawned, tmp: &std::path::Path, home: &std::path::Path| -> Vec<String> {
        let (t, h) = (
            tmp.to_string_lossy().to_string(),
            home.to_string_lossy().to_string(),
        );
        sp.specs.lock().unwrap()[1]
            .argv
            .iter()
            .map(|x| {
                folder
                    .replace_all(&x.replace(&t, "TMP").replace(&h, "HOME"), "RUN")
                    .into_owned()
            })
            .collect()
    };
    assert_eq!(
        argv(&today.spawned, &today.tmp, &today.rt.ferb_home),
        argv(&off.spawned, &off.tmp, &off.rt.ferb_home)
    );
    assert!(!k.handle.state.contains_key("kept"));
    assert!(!argv(&off.spawned, &off.tmp, &off.rt.ferb_home)
        .iter()
        .any(|a| a.contains("FERB_RUN_TOKEN_WAIT")));
}

// decision: ORCH-726, RUN-481
#[tokio::test(flavor = "current_thread")]
async fn a_kept_codex_tool_bridge_reads_its_token_file_at_each_request() {
    let c = cx(codex_script(1));
    let keeper = Keeper::new(FakeClock::default().clock());
    let kept = Kept::new(keeper.clone(), "planner", Arc::new(c.rt.clone()));
    let mut s = kept
        .start(&task("t1"), &cached_request("hi"), "tok")
        .await
        .unwrap();
    drain(&mut s).await;
    s.control.cancel().await;
    let argv = c.spawned.specs.lock().unwrap()[1].argv.join(" ");
    assert!(
        argv.contains(&format!(
            "FERB_RUN_TOKEN_WAIT={}",
            ferb_py::json::dumps(&format!("{}", ferb_runtimes::codex::kept::BRIDGE_WAIT_S))
        )),
        "{argv}"
    );
    keeper.shut_down().await;
}

// ---------------------------------------------------------------------------------------------------------------
// Claude Code started ahead, used once
// ---------------------------------------------------------------------------------------------------------------

struct Relay;

impl ferb_runtimes::claude::ToolProxy for Relay {
    fn base_url(&self, run_id: &str) -> String {
        format!("http://127.0.0.1:9/run/{run_id}/fixed-secret")
    }
    fn seen(&self, _run_id: &str) -> BTreeSet<String> {
        ["mcp__ferb__memory_read".to_string()].into_iter().collect()
    }
    fn forget(&self, _run_id: &str) {}
}

fn claude_script() -> Value {
    let stream = |event: Value| {
        json!({"out": {"type": "stream_event", "uuid": "se", "session_id": "s", "parent_tool_use_id": null,
            "event": event}})
    };
    json!({"on": {
    "control:initialize": [[{"out": {"type": "control_response", "response": {"subtype": "success",
        "request_id": "$ID", "response": {"commands": []}}}}]],
    "control:interrupt": [[{"out": {"type": "control_response", "response": {"subtype": "success",
        "request_id": "$ID", "response": {}}}}]],
    "user": [[
        {"out": {"type": "system", "subtype": "init", "apiKeySource": "none", "session_id": "s",
            "uuid": "u0", "tools": ["mcp__ferb__memory_read"]}},
        stream(json!({"type": "message_start", "message": {}})),
        stream(json!({"type": "content_block_start", "index": 0, "content_block": {"type": "text", "text": ""}})),
        stream(json!({"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": "Sure."}})),
        stream(json!({"type": "content_block_stop", "index": 0})),
        {"out": {"type": "assistant", "message": {"model": "claude-sonnet-5-5", "id": "msg",
            "content": [{"type": "text", "text": "Sure."}], "stop_reason": null}, "session_id": "s",
            "uuid": "ua", "parent_tool_use_id": null}},
        {"out": {"type": "result", "subtype": "success", "duration_ms": 1, "duration_api_ms": 1,
            "is_error": false, "num_turns": 1, "session_id": "s", "result": "Sure.",
            "usage": {"input_tokens": 10, "output_tokens": 5}, "stop_reason": "end_turn"}},
    ]]}})
}

/// The token file and the bridge's wait named in a spawned Claude Code's MCP configuration.
fn claude_mcp(spec: &ferb_runtimes::process::Spawn) -> (PathBuf, Value) {
    let at = spec.argv.iter().position(|a| a == "--mcp-config").unwrap();
    let config: Value = serde_json::from_str(&spec.argv[at + 1]).unwrap();
    let env = config["mcpServers"]["ferb"]["env"].clone();
    (
        PathBuf::from(env["FERB_RUN_TOKEN_FILE"].as_str().unwrap()),
        env,
    )
}

// decision: ORCH-726, RUN-481
#[tokio::test(flavor = "current_thread")]
async fn claude_code_is_started_ahead_takes_one_message_and_the_next_is_made_ready() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let tmp = dir.path().join("tmp");
    std::fs::create_dir_all(home.join("claude-home")).unwrap();
    std::fs::write(home.join("claude-home/.ferb-signed-in"), "x\n").unwrap();
    std::fs::create_dir_all(&tmp).unwrap();
    let spawned = Arc::new(Spawned::default());
    let host = ferb_runtimes::claude::Host {
        spawner: Arc::new(FakeSpawner {
            script: claude_script(),
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
        today: Arc::new(|| "2026-11-27".into()),
    };
    let mut rt = ferb_runtimes::claude::ClaudeRuntime::new(
        ferb_runtimes::claude::Mode::Supervisor,
        Arc::new(Relay),
        home.clone(),
        &["memory.read".to_string()],
        None,
        Some("claude-sonnet-5-5".into()),
        Arc::new(RecordedForms(json!({}))),
        Arc::new(ferb_runtimes::forms::Refusals::new(Box::new(|| 0.0))),
        host,
    );
    rt.monotonic = Arc::new(|| 0.0);
    rt.shim_cmd = Some(vec!["/opt/ferb/bin/ferb".into(), "mcp".into()]);
    rt.cli = Some("/opt/ferb/claude".into());
    let keeper = Keeper::new(FakeClock::default().clock());
    let rt = Arc::new(rt);
    // the app said the owner started typing: Claude Code is started before the message
    keeper.prepare_for("front", rt.clone(), true);
    until("ready ahead", || keeper.has_idle("front")).await;
    let programs = |s: &Spawned| -> Vec<ferb_runtimes::process::Spawn> {
        s.specs
            .lock()
            .unwrap()
            .iter()
            .filter(|s| s.argv.get(1).is_none_or(|a| a != "-v"))
            .cloned()
            .collect()
    };
    let first = programs(&spawned)[0].clone();
    let (token_file, env) = claude_mcp(&first);
    assert_eq!(
        std::fs::read_to_string(&token_file).unwrap(),
        "",
        "a token before its message"
    );
    assert_eq!(
        env["FERB_RUN_TOKEN_WAIT"],
        json!(format!("{}", ferb_runtimes::claude::kept::BRIDGE_WAIT_S))
    );
    assert_eq!(
        first.env.get("MCP_TIMEOUT").map(String::as_str),
        Some(
            format!(
                "{}",
                (ferb_runtimes::claude::kept::BRIDGE_WAIT_S * 1000.0) as u64
            )
            .as_str()
        )
    );
    let app = apps(&spawned)[0].clone();
    assert!(app
        .lines
        .lock()
        .unwrap()
        .iter()
        .all(|l| !l.contains("\"type\": \"user\"")));

    let kept = Kept::new(keeper.clone(), "front", rt.clone());
    let mut s = kept
        .start(&task("t1"), &plain_request(), "tok-1")
        .await
        .unwrap();
    assert_eq!(s.handle.state["kept"], json!("ready"));
    assert_eq!(std::fs::read_to_string(&token_file).unwrap(), "tok-1");
    until("the message reached the program made ahead", || {
        app.lines
            .lock()
            .unwrap()
            .iter()
            .any(|l| l.contains("\"type\": \"user\""))
    })
    .await;
    let mut finals = Vec::new();
    while let Some(item) = tokio::time::timeout(Duration::from_secs(5), s.events.next())
        .await
        .unwrap()
    {
        let ev = item.unwrap();
        if ev.kind == K::Final {
            finals.push(ev.data["text"].clone());
        }
    }
    assert_eq!(finals, [json!("Sure.")]);
    s.control.cancel().await;
    assert!(app.exited().is_some(), "a used Claude Code was kept");
    assert!(!token_file.exists(), "its token file is left");
    // the next one is ready for the next message, with a token file of its own
    until("the next one is ready", || {
        keeper.has_idle("front") && programs(&spawned).len() == 2
    })
    .await;
    let (next_file, _) = claude_mcp(&programs(&spawned)[1]);
    assert_ne!(next_file, token_file);
    assert_eq!(std::fs::read_to_string(&next_file).unwrap(), "");
    keeper.shut_down().await;
    assert!(apps(&spawned).iter().all(|a| a.exited().is_some()));
    assert!(!next_file.exists());
}

// ---------------------------------------------------------------------------------------------------------------
// the API loop's kept connection
// ---------------------------------------------------------------------------------------------------------------

/// A provider stand-in on 127.0.0.1 answering every request on a kept-alive connection with a short streamed
/// Anthropic reply; counts the connections it accepted.
async fn api_server(listener: tokio::net::TcpListener, accepted: Arc<AtomicUsize>) {
    use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
    let events = [
        json!({"type": "message_start", "message": {"role": "assistant", "content": [], "usage": {"input_tokens": 7}}}),
        json!({"type": "content_block_start", "index": 0, "content_block": {"type": "text", "text": ""}}),
        json!({"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": "Sure."}}),
        json!({"type": "content_block_stop", "index": 0}),
        json!({"type": "message_delta", "delta": {"stop_reason": "end_turn"}, "usage": {"output_tokens": 3}}),
        json!({"type": "message_stop"}),
    ];
    let body: String = events
        .iter()
        .map(|e| format!("data: {}\n\n", ferb_py::json::dumps(e)))
        .collect();
    loop {
        let Ok((stream, _)) = listener.accept().await else {
            return;
        };
        accepted.fetch_add(1, Ordering::SeqCst);
        let body = body.clone();
        tokio::spawn(async move {
            let (read, mut write) = stream.into_split();
            let mut read = BufReader::new(read);
            loop {
                let mut line = String::new();
                if read.read_line(&mut line).await.unwrap_or(0) == 0 {
                    return;
                }
                let mut length = 0usize;
                loop {
                    let mut h = String::new();
                    read.read_line(&mut h).await.unwrap();
                    let h = h.trim_end().to_lowercase();
                    if h.is_empty() {
                        break;
                    }
                    if let Some(v) = h.strip_prefix("content-length:") {
                        length = v.trim().parse().unwrap();
                    }
                }
                let mut b = vec![0u8; length];
                read.read_exact(&mut b).await.unwrap();
                let head = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\n\r\n",
                    body.len()
                );
                write.write_all(head.as_bytes()).await.unwrap();
                write.write_all(body.as_bytes()).await.unwrap();
                write.flush().await.unwrap();
            }
        });
    }
}

async fn api_calls(on: bool) -> usize {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
    let accepted = Arc::new(AtomicUsize::new(0));
    let srv = tokio::spawn(api_server(listener, accepted.clone()));
    let mut rt = ferb_runtimes::api::ApiRuntime::new(
        Arc::new(ferb_runtimes::api::anthropic::AnthropicProvider::new(Some(
            "claude-sonnet-5-5",
        ))),
        true,
        "front",
        &[],
        None,
        None,
        Arc::new(|_name: String| Box::pin(async { Some(ferb_identity::Secret::from("k")) })),
        Arc::new(ferb_runtimes::forms::ContextForms),
        Arc::new(ferb_runtimes::forms::Refusals::new(Box::new(|| 0.0))),
    )
    .expect("a runtime");
    rt.monotonic = Arc::new(|| 0.0);
    rt.origin = Some(origin);
    rt.token_hex = counter();
    rt.today = Arc::new(|| "2026-11-27".into());
    let keeper = Keeper::new(FakeClock::default().clock());
    keeper.set_on(on);
    let kept = Kept::new(keeper.clone(), "front", Arc::new(rt));
    for n in 0..2 {
        let mut s = kept
            .start(&task(&format!("t{n}")), &plain_request(), "")
            .await
            .unwrap();
        let mut finals = Vec::new();
        while let Some(item) = tokio::time::timeout(Duration::from_secs(5), s.events.next())
            .await
            .unwrap()
        {
            let ev = item.unwrap();
            if ev.kind == K::Final {
                finals.push(ev.data["text"].clone());
            }
        }
        assert_eq!(finals, [json!("Sure.")]);
        s.control.cancel().await;
    }
    keeper.shut_down().await;
    srv.abort();
    accepted.load(Ordering::SeqCst)
}

// decision: ORCH-726, RUN-481
#[tokio::test(flavor = "current_thread")]
async fn the_api_loop_keeps_its_connection_between_messages() {
    assert_eq!(
        api_calls(true).await,
        1,
        "the second message opened a new connection"
    );
    assert_eq!(
        api_calls(false).await,
        2,
        "switched off, each message has its own client"
    );
}

// ---------------------------------------------------------------------------------------------------------------
// the tool bridge of a kept program
// ---------------------------------------------------------------------------------------------------------------

/// A daemon stand-in that records the token each connection's hello carried and lists one tool.
async fn token_daemon(listener: tokio::net::UnixListener, tokens: Arc<Mutex<Vec<String>>>) {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    loop {
        let Ok((stream, _)) = listener.accept().await else {
            return;
        };
        let tokens = tokens.clone();
        tokio::spawn(async move {
            let (read, mut write) = stream.into_split();
            let mut read = BufReader::new(read);
            loop {
                let mut line = String::new();
                if read.read_line(&mut line).await.unwrap_or(0) == 0 {
                    return;
                }
                let msg: Value = serde_json::from_str(&line).unwrap();
                let result = match msg["method"].as_str() {
                    Some("auth.hello") => {
                        tokens
                            .lock()
                            .unwrap()
                            .push(msg["params"]["token"].as_str().unwrap().to_string());
                        json!({})
                    }
                    Some("tools.list") => json!({"tools": [{"name": "memory.read",
                        "description": "Read memory.", "input_schema": {"type": "object"}}]}),
                    _ => json!({"content": "ok", "footer": "", "is_error": false}),
                };
                let reply = json!({"jsonrpc": "2.0", "id": msg["id"], "result": result});
                write
                    .write_all(format!("{}\n", ferb_py::json::dumps(&reply)).as_bytes())
                    .await
                    .unwrap();
            }
        });
    }
}

// decision: ORCH-726, RUN-481
#[tokio::test(flavor = "current_thread")]
async fn a_kept_programs_bridge_waits_for_its_token_and_reads_it_at_each_request() {
    use ferb_runtimes::mcp_bridge::{serve_from, source_from, TokenSource, NO_GRANT};
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    let dir = tempfile::Builder::new()
        .prefix("k")
        .tempdir_in("/tmp")
        .unwrap();
    let socket = dir.path().join("d.sock");
    let token_file = dir.path().join("run-token");
    std::fs::write(&token_file, "").unwrap();
    let tokens = Arc::new(Mutex::new(Vec::new()));
    let daemon = tokio::spawn(token_daemon(
        tokio::net::UnixListener::bind(&socket).unwrap(),
        tokens.clone(),
    ));
    let env: std::collections::HashMap<String, String> = [
        (
            "FERB_RUN_TOKEN_FILE".to_string(),
            token_file.to_string_lossy().to_string(),
        ),
        ("FERB_RUN_TOKEN_WAIT".to_string(), "0.5".to_string()),
    ]
    .into_iter()
    .collect();
    let source = source_from(&env).expect("a source");
    assert!(matches!(source, TokenSource::File { .. }));
    // today's bridge (no wait) with a blank file has no token at all and never starts
    let mut today = env.clone();
    today.remove("FERB_RUN_TOKEN_WAIT");
    assert!(source_from(&today).is_none());

    let (client, server) = tokio::io::duplex(1 << 16);
    let (s_read, s_write) = tokio::io::split(server);
    let bridge = tokio::spawn(serve_from(s_read, s_write, socket.clone(), source));
    let (c_read, mut c_write) = tokio::io::split(client);
    let mut lines = BufReader::new(c_read);
    let mut send = async |m: Value| {
        c_write
            .write_all(format!("{}\n", serde_json::to_string(&m).unwrap()).as_bytes())
            .await
            .unwrap();
    };
    let init = json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2025-06-18",
        "capabilities": {}, "clientInfo": {"name": "codex", "version": "1"}}});
    // the initialize answer waits for the message's token: blank for the whole wait, it is refused
    send(init.clone()).await;
    let mut reply = String::new();
    lines.read_line(&mut reply).await.unwrap();
    assert!(reply.contains(NO_GRANT), "{reply}");
    // with the token there it answers, lists and calls with it; a new message's token is used at the next request
    std::fs::write(&token_file, "tok-1").unwrap();
    send(json!({"jsonrpc": "2.0", "id": 2, "method": "initialize", "params": init["params"]}))
        .await;
    reply.clear();
    lines.read_line(&mut reply).await.unwrap();
    assert!(reply.contains("protocolVersion"), "{reply}");
    send(json!({"jsonrpc": "2.0", "id": 3, "method": "tools/list"})).await;
    reply.clear();
    lines.read_line(&mut reply).await.unwrap();
    assert!(reply.contains("memory_read"), "{reply}");
    std::fs::write(&token_file, "tok-2").unwrap();
    send(json!({"jsonrpc": "2.0", "id": 4, "method": "tools/call",
        "params": {"name": "memory_read", "arguments": {}}}))
    .await;
    reply.clear();
    lines.read_line(&mut reply).await.unwrap();
    assert!(reply.contains("\"isError\":false"), "{reply}");
    // between messages the file is blank: a request is refused without reaching the daemon
    std::fs::write(&token_file, "").unwrap();
    send(json!({"jsonrpc": "2.0", "id": 5, "method": "tools/call",
        "params": {"name": "memory_read", "arguments": {}}}))
    .await;
    reply.clear();
    lines.read_line(&mut reply).await.unwrap();
    assert!(
        reply.contains(NO_GRANT) && reply.contains("\"isError\":true"),
        "{reply}"
    );
    assert_eq!(*tokens.lock().unwrap(), ["tok-1", "tok-2"]);
    drop(send);
    drop(c_write);
    drop(lines);
    let _ = tokio::time::timeout(Duration::from_secs(5), bridge).await;
    daemon.abort();
}

// decision: ORCH-726, RUN-481
#[tokio::test(flavor = "current_thread")]
async fn the_workers_runtime_takes_the_ready_program_through_the_kept_adapter() {
    use ferb_worker::ports::ModelRuntime;
    let keeper = Keeper::new(FakeClock::default().clock());
    let made = Arc::new(Made::default());
    let rt = ferb_runtimes::worker::WorkerRuntime {
        adapter: Arc::new(stand_in(&keeper, "model-a", &made)),
        limits: Err(ferb_core::errors::FerbError::ModelLimitsUnknown(
            "stand-in".into(),
        )),
        output_schema: None,
    };
    for n in 1..=2 {
        let started = rt
            .start(&task(&format!("t{n}")), &plain_request(), "tok")
            .await
            .unwrap_or_else(|_| panic!("started"));
        assert_eq!(started.handle.state["program"], json!(1));
        started.control.cancel().await;
    }
    assert_eq!(made.programs.lock().unwrap().len(), 1);
}
