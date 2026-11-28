//! ORCH-731 (front door step 2a): a call with no task tree hands back its answer as soon as the stream ended and the
//! answer passed its check; its program closes after. A stand-in runtime whose `cancel` waits on a gate (released by
//! the test, the way a Claude Code program takes about 2 s to exit) shows what returns when. Not from the fixture.
//! Tokio's paused clock: no real waiting (TEST-508).
//!
//! ORCH-733 (front door step 4), on the same stand-in: each tool event reaches `on_tool_event`, and a plain-mode run
//! (no answer form) ends on an accepted call of `Call::ends_on`, closing after as above.

mod support;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ferb_context::assemble::ContextBlock;
use ferb_context::prompts::PromptRoot;
use ferb_context::tokens::Tokenizer;
use ferb_core::errors::FerbError;
use ferb_core::types::{
    Identity, ModelLimits, ModelRequest, RunHandle, RuntimeEvent, RuntimeEventKind, Task, TaskRole,
};
use ferb_orchestrator::steering::ContinueError;
use ferb_worker::ports::{
    BoxFuture, EventSource, ModelRuntime, RunControl, StartedRun, WorkerHost, WorkerTree,
};
use ferb_worker::{Call, CallHooks, Error, Runs, Worker, WorkerConfig};
use serde_json::{json, Map, Value};
use support::{repo, TestHost, TestTree};

/// How long the stand-in program takes to close once its gate opens (the injected clock).
const CLOSE_S: f64 = 2.1;

fn config(answer_before_close: bool) -> WorkerConfig {
    WorkerConfig {
        heartbeat_poll_s: 0.01,
        stall_s: 180.0,
        tokenizer: Tokenizer::Utf8Bytes,
        prompts: PromptRoot::new(repo().join("engine/runtime")).unwrap(),
        approach: "orchestrator/prompts".into(),
        answer_before_close,
        fresh_run: false,
    }
}

fn obj(v: Value) -> Map<String, Value> {
    v.as_object().cloned().unwrap()
}

/// A runtime whose one run plays `events` and whose `cancel` waits for the gate, then takes CLOSE_S.
struct Gated {
    host: Arc<TestHost>,
    events: Vec<RuntimeEvent>,
    /// The answer form (None: plain mode).
    schema: Option<Value>,
    /// How long reading one event takes on the injected clock.
    read_s: f64,
    gate: tokio::sync::watch::Receiver<bool>,
    unreported: Vec<Map<String, Value>>,
    reads: Arc<AtomicUsize>,
}

struct GatedControl {
    host: Arc<TestHost>,
    gate: tokio::sync::watch::Receiver<bool>,
    unreported: Mutex<Vec<Map<String, Value>>>,
    reads: Arc<AtomicUsize>,
}

impl RunControl for GatedControl {
    fn steer<'a>(&'a self, _message: &'a str) -> BoxFuture<'a, Result<(), ContinueError>> {
        Box::pin(async { Err(ContinueError::Unsupported) })
    }
    fn cancel(&self) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            self.host.rec("rt.cancel.begin", Value::Null);
            let mut gate = self.gate.clone();
            let _ = gate.wait_for(|open| *open).await;
            self.host.clock.advance(CLOSE_S);
            self.host.rec("rt.cancel.end", Value::Null);
        })
    }
    fn unreported_usage(&self) -> Vec<Map<String, Value>> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        std::mem::take(&mut *self.unreported.lock().unwrap())
    }
}

/// The run's events; each one read is logged (`rt.read`) and takes `read_s`.
struct Events {
    host: Arc<TestHost>,
    events: std::vec::IntoIter<RuntimeEvent>,
    read_s: f64,
}

impl EventSource for Events {
    fn next(&mut self) -> BoxFuture<'_, Option<Result<RuntimeEvent, Error>>> {
        let next = self.events.next();
        if next.is_some() {
            self.host.clock.advance(self.read_s);
            self.host.rec("rt.read", Value::Null);
        }
        Box::pin(async move { next.map(Ok) })
    }
}

impl ModelRuntime for Gated {
    fn name(&self) -> String {
        "gated".into()
    }
    fn limits(&self) -> Result<ModelLimits, FerbError> {
        Ok(ModelLimits {
            model: "fake-model".into(),
            context_window: 1_000_000,
            max_output: 100_000,
            source: "test double".into(),
            window_includes_output: true,
            max_input_chars: None,
        })
    }
    fn output_schema(&self) -> Option<Value> {
        self.schema.clone()
    }
    fn start<'a>(
        &'a self,
        task: &'a Task,
        _request: &'a ModelRequest,
        _run_token: &'a str,
    ) -> BoxFuture<'a, Result<StartedRun, Error>> {
        Box::pin(async move {
            Ok(StartedRun {
                handle: RunHandle {
                    id: "run-1".into(),
                    runtime: "gated".into(),
                    task_id: task.id.clone(),
                    state: Map::new(),
                },
                events: Box::new(Events {
                    host: self.host.clone(),
                    events: self.events.clone().into_iter(),
                    read_s: self.read_s,
                }),
                control: Arc::new(GatedControl {
                    host: self.host.clone(),
                    gate: self.gate.clone(),
                    unreported: Mutex::new(self.unreported.clone()),
                    reads: self.reads.clone(),
                }),
            })
        })
    }
}

fn event(kind: RuntimeEventKind, data: Value) -> RuntimeEvent {
    RuntimeEvent {
        kind,
        data: obj(data),
    }
}

fn answered() -> Vec<RuntimeEvent> {
    vec![
        event(
            RuntimeEventKind::Usage,
            json!({"model": "m", "input_tokens": 10, "output_tokens": 2}),
        ),
        event(RuntimeEventKind::Final, json!({"text": "the answer"})),
    ]
}

/// The run stopped before it answered (its last request's usage still with the provider).
fn unanswered() -> Vec<RuntimeEvent> {
    vec![event(
        RuntimeEventKind::Usage,
        json!({"model": "m", "input_tokens": 10, "output_tokens": 2}),
    )]
}

fn at_end() -> Map<String, Value> {
    obj(
        json!({"model": "m", "input_tokens": 1234, "output_tokens": 5, "requests": 2,
               "recorded_at_end": true, "cost_usd": null}),
    )
}

fn task() -> Task {
    Task {
        id: "req-1".into(),
        parent_id: None,
        goal: "chat turn".into(),
        device_id: "dev-1".into(),
        identity: Identity::Owner,
        scopes: Default::default(),
        role: TaskRole::Mixed,
        request_id: "req-1".into(),
        success_conditions: Vec::new(),
        deadline: None,
    }
}

struct Run {
    log: Vec<Value>,
    out: Result<String, Error>,
    timing: Vec<Map<String, Value>>,
    reads: usize,
    /// What `on_tool_event` was told, in order.
    tool_events: Vec<Value>,
    /// ORCH-753: the runs whose looks the gateway was told to reset.
    looks_reset: Vec<String>,
}

impl Run {
    fn at(&self, what: &str) -> usize {
        self.log
            .iter()
            .position(|e| e[0] == what)
            .unwrap_or_else(|| panic!("no {what} in {:#?}", self.log))
    }

    /// The `spans.finish` entries whose output holds `key`.
    fn finished_with(&self, key: &str) -> Vec<(usize, Value)> {
        self.log
            .iter()
            .enumerate()
            .filter(|(_, e)| e[0] == "spans.finish" && e[1]["output"].get(key).is_some())
            .map(|(i, e)| (i, e[1].clone()))
            .collect()
    }

    fn count(&self, what: &str) -> usize {
        self.log.iter().filter(|e| e[0] == what).count()
    }

    fn started(&self, kind: &str) -> Vec<usize> {
        self.log
            .iter()
            .enumerate()
            .filter(|(_, e)| e[0] == "spans.start" && e[1]["kind"] == kind)
            .map(|(i, _)| i)
            .collect()
    }
}

/// How one call is set up.
struct Setup {
    answer_before_close: bool,
    events: Vec<RuntimeEvent>,
    with_tree: bool,
    schema: Option<Value>,
    ends_on: Vec<String>,
    read_s: f64,
    worker_id: &'static str,
}

impl Setup {
    fn new(answer_before_close: bool, events: Vec<RuntimeEvent>) -> Self {
        Setup {
            answer_before_close,
            events,
            with_tree: false,
            schema: None,
            ends_on: Vec::new(),
            read_s: 0.0,
            worker_id: "",
        }
    }
}

async fn run(answer_before_close: bool, events: Vec<RuntimeEvent>, with_tree: bool) -> Run {
    run_with(Setup {
        with_tree,
        ..Setup::new(answer_before_close, events)
    })
    .await
}

/// One call through `run_call`, its gate released 5 s (paused clock) after the test begins; then the worker's
/// background closes joined, as the daemon's shutdown does.
async fn run_with(setup: Setup) -> Run {
    let Setup {
        answer_before_close,
        events,
        with_tree,
        schema,
        ends_on,
        read_s,
        worker_id,
    } = setup;
    let host = TestHost::new(&json!({}), &json!({}));
    let worker = Arc::new(Worker::new(
        host.clone() as Arc<dyn WorkerHost>,
        Arc::new(Runs::new()),
        config(answer_before_close),
    ));
    let _ = host.worker.set(Arc::downgrade(&worker));
    let (open, gate) = tokio::sync::watch::channel(false);
    let reads = Arc::new(AtomicUsize::new(0));
    let rt: Arc<dyn ModelRuntime> = Arc::new(Gated {
        host: host.clone(),
        events,
        schema,
        read_s,
        gate,
        unreported: vec![at_end()],
        reads: reads.clone(),
    });
    let tree: Option<Arc<dyn WorkerTree>> =
        with_tree.then(|| Arc::new(TestTree::from_spec(&json!({}))) as Arc<dyn WorkerTree>);
    let task = task();
    let timing: Arc<Mutex<Vec<Map<String, Value>>>> = Arc::default();
    let sink = timing.clone();
    let tool_events: Arc<Mutex<Vec<Value>>> = Arc::default();
    let told = tool_events.clone();
    let call = Call {
        task: &task,
        system: "instructions".into(),
        context: vec![ContextBlock::new("the message", "message")],
        token: "",
        parent_span: "span-turn",
        tree: tree.as_ref(),
        worker_id,
        images: Vec::new(),
        hooks: CallHooks {
            on_timing: Some(Arc::new(move |t| sink.lock().unwrap().push(t))),
            on_tool_event: Some(Arc::new(move |e: &Value| {
                told.lock().unwrap().push(e.clone())
            })),
            ..CallHooks::default()
        },
        retry_silent: false,
        ends_on,
    };
    let called = async {
        let out = worker.run_call(&rt, call).await;
        let active = worker.runs().active().len();
        host.rec("returned", json!({"ok": out.is_ok(), "active": active}));
        worker.closes_done().await;
        host.rec("closes_done", Value::Null);
        out
    };
    let release = async {
        tokio::time::sleep(Duration::from_secs(5)).await;
        host.rec("released", Value::Null);
        open.send_replace(true);
    };
    let (out, ()) = tokio::join!(called, release);
    let timing = timing.lock().unwrap().clone();
    let tool_events = tool_events.lock().unwrap().clone();
    let looks_reset = host.looks_reset.lock().unwrap().clone();
    Run {
        log: host.take_log(),
        out,
        timing,
        reads: reads.load(Ordering::SeqCst),
        tool_events,
        looks_reset,
    }
}

/// The answer is back while the program is still closing; the run already left the running calls; the close's
/// own time is recorded after, and the shutdown's join waits for it.
// decision: ORCH-731
#[tokio::test(start_paused = true)]
async fn a_tree_less_answer_returns_before_its_slow_close() {
    let r = run(true, answered(), false).await;
    assert_eq!(r.out.as_deref().unwrap(), "the answer");
    let returned = r.at("returned");
    assert!(
        returned < r.at("released"),
        "the answer waited for the close: {:#?}",
        r.log
    );
    assert!(r.at("rt.cancel.begin") < r.at("released"));
    assert_eq!(
        r.log[returned][1]["active"], 0,
        "a returned call's run is no longer running"
    );
    assert!(r.at("released") < r.at("rt.cancel.end"));
    assert!(r.at("rt.cancel.end") < r.at("closes_done"));
    // the owner's call ended at the stream: its close step is only the tables' update
    assert_eq!(r.timing.len(), 1, "{:?}", r.timing);
    let steps = &r.timing[0]["steps_ms"];
    let keys: Vec<&String> = steps.as_object().unwrap().keys().collect();
    assert_eq!(keys, ["prepare", "start", "stream", "close"]);
    assert_eq!(steps["close"], 0);
    assert_eq!(r.timing[0]["total_ms"], 0);
    // the close's own time, under the call's parent, started as the close began
    let close = r.started("model.close");
    assert_eq!(close.len(), 1, "{:#?}", r.log);
    assert_eq!(r.log[close[0]][1]["parent_id"], "span-turn");
    assert!(close[0] < r.at("rt.cancel.begin"));
    let closed = r.finished_with("close_ms");
    assert_eq!(closed.len(), 1);
    assert_eq!(closed[0].1["output"]["close_ms"], 2100);
    assert!(closed[0].0 < r.at("closes_done"));
}

/// RUN-477 after an early answer: the usage the provider counted that no event carried is read once, after the
/// program closed, and recorded once under the call's parent, before the close's own record.
// decision: RUN-477
// decision: ORCH-731
#[tokio::test(start_paused = true)]
async fn unreported_usage_is_recorded_once_after_the_late_close() {
    let r = run(true, answered(), false).await;
    assert_eq!(r.reads, 1);
    let usage = r.finished_with("recorded_at_end");
    assert_eq!(usage.len(), 1, "{:#?}", r.log);
    assert_eq!(usage[0].1["output"]["input_tokens"], 1234);
    assert!(r.at("returned") < usage[0].0);
    assert!(r.at("rt.cancel.end") < usage[0].0);
    let started = r.started("model.usage");
    assert_eq!(
        started.len(),
        2,
        "the stream's report and the one at the end"
    );
    assert_eq!(r.log[started[1]][1]["parent_id"], "span-turn");
    assert!(usage[0].0 < r.finished_with("close_ms")[0].0);
}

/// A call that failed closes first, as before: its error comes back only once the program closed and its usage
/// was recorded; no close of its own is recorded.
// decision: ORCH-731
#[tokio::test(start_paused = true)]
async fn a_failed_call_still_closes_before_it_returns() {
    let r = run(true, unanswered(), false).await;
    let e = r.out.as_ref().unwrap_err();
    assert_eq!(e.to_string(), "runtime finished without an answer");
    assert!(r.at("released") < r.at("rt.cancel.end"));
    assert!(r.at("rt.cancel.end") < r.at("returned"), "{:#?}", r.log);
    let usage = r.finished_with("recorded_at_end");
    assert_eq!(usage.len(), 1);
    assert!(usage[0].0 < r.at("returned"));
    assert!(r.started("model.close").is_empty());
    assert_eq!(r.timing[0]["steps_ms"]["close"], 2100);
}

/// A call of a task tree is unchanged: it closes under the tree's steering lock before it returns, and its close
/// step holds the close.
// decision: ORCH-731
#[tokio::test(start_paused = true)]
async fn a_call_with_a_tree_closes_before_it_returns() {
    let r = run(true, answered(), true).await;
    assert_eq!(r.out.as_deref().unwrap(), "the answer");
    assert!(r.at("released") < r.at("rt.cancel.end"));
    assert!(r.at("rt.cancel.end") < r.at("returned"), "{:#?}", r.log);
    assert!(r.finished_with("recorded_at_end")[0].0 < r.at("returned"));
    assert!(r.started("model.close").is_empty());
    assert_eq!(r.timing[0]["steps_ms"]["close"], 2100);
    assert_eq!(r.timing[0]["total_ms"], 2100);
}

/// The switch off (the recorded replays): the answer waits for the close, as Python's did.
#[tokio::test(start_paused = true)]
async fn switched_off_the_answer_waits_for_the_close() {
    let r = run(false, answered(), false).await;
    assert_eq!(r.out.as_deref().unwrap(), "the answer");
    assert!(r.at("rt.cancel.end") < r.at("returned"), "{:#?}", r.log);
    assert!(r.started("model.close").is_empty());
    assert_eq!(r.timing[0]["steps_ms"]["close"], 2100);
}

fn text(t: &str) -> RuntimeEvent {
    event(RuntimeEventKind::TextDelta, json!({"text": t}))
}

/// A plain-mode tool event (the runtimes' contract, front door step 4).
fn tool(name: &str, phase: &str, arguments: Value, error: Option<bool>) -> RuntimeEvent {
    let mut data = json!({"tool": format!("mcp__ferb__{name}"), "name": name, "phase": phase,
                          "arguments": arguments});
    if let Some(e) = error {
        data["error"] = json!(e);
    }
    event(RuntimeEventKind::ToolCall, data)
}

fn hand_off() -> Value {
    json!({"ack": "On it: the flights for Friday.", "goal": "find flights for Friday"})
}

/// `ends_on` the hand-off, the early-close switch off, each event read taking 1 s.
fn ends_on_dispatch(events: Vec<RuntimeEvent>) -> Setup {
    Setup {
        ends_on: vec!["dispatch_task".into()],
        read_s: 1.0,
        ..Setup::new(false, events)
    }
}

/// Each tool event reaches the caller as the runtime reported it, in order: the start (the live status line's
/// trigger) and then the completion; the run goes on to its own answer.
// decision: ORCH-733
#[tokio::test(start_paused = true)]
async fn tool_events_reach_the_caller_in_order_with_both_phases() {
    let r = run_with(Setup::new(
        true,
        vec![
            text("Let me look. "),
            tool("memory_read", "started", json!({"query": "flights"}), None),
            tool(
                "memory_read",
                "completed",
                json!({"query": "flights"}),
                Some(false),
            ),
            text("Nothing saved."),
            event(RuntimeEventKind::Final, json!({"text": "Nothing saved."})),
        ],
    ))
    .await;
    assert_eq!(r.out.as_deref().unwrap(), "Nothing saved.");
    let seen: Vec<(&str, &str)> = r
        .tool_events
        .iter()
        .map(|e| (e["name"].as_str().unwrap(), e["phase"].as_str().unwrap()))
        .collect();
    assert_eq!(
        seen,
        [("memory_read", "started"), ("memory_read", "completed")]
    );
    assert_eq!(r.tool_events[0]["arguments"], json!({"query": "flights"}));
    assert_eq!(r.tool_events[1]["error"], false);
}

/// The front door's hand-off: a completed, accepted call of `ends_on` is the answer. The stream is read no further,
/// the call returns with which call ended it, its arguments and the text before it, while the program is still
/// closing (the switch off: the call itself is the answer); the close, RUN-477's usage and `model.close` follow
/// once, and `stream` is marked at the accepted call.
// decision: ORCH-733
// decision: ORCH-731
#[tokio::test(start_paused = true)]
async fn an_accepted_final_call_ends_the_run_and_closes_after() {
    let r = run_with(ends_on_dispatch(vec![
        text("On it."),
        tool("dispatch_task", "started", hand_off(), None),
        tool("dispatch_task", "completed", hand_off(), Some(false)),
        text(" More words the model never gets to."),
        event(RuntimeEventKind::Final, json!({"text": "unread"})),
    ]))
    .await;
    let out: Value = serde_json::from_str(r.out.as_deref().unwrap()).unwrap();
    assert_eq!(
        out,
        json!({"ended_on": "dispatch_task", "arguments": hand_off(), "text": "On it."})
    );
    assert_eq!(
        r.count("rt.read"),
        3,
        "read past the accepted call: {:#?}",
        r.log
    );
    let returned = r.at("returned");
    assert!(
        returned < r.at("released"),
        "the answer waited: {:#?}",
        r.log
    );
    assert_eq!(r.log[returned][1]["active"], 0);
    assert!(r.at("released") < r.at("rt.cancel.end"));
    assert!(r.at("rt.cancel.end") < r.at("closes_done"));
    // RUN-477: read once, after the close, recorded once
    assert_eq!(r.reads, 1);
    let usage = r.finished_with("recorded_at_end");
    assert_eq!(usage.len(), 1, "{:#?}", r.log);
    assert!(r.at("rt.cancel.end") < usage[0].0);
    let closed = r.finished_with("close_ms");
    assert_eq!(closed.len(), 1, "{:#?}", r.log);
    assert_eq!(closed[0].1["output"]["close_ms"], 2100);
    assert_eq!(
        r.log[r.started("model.close")[0]][1]["parent_id"],
        "span-turn"
    );
    // timing: the stream ends at the third event (1 s each); the close is only the tables' update
    let steps = &r.timing[0]["steps_ms"];
    let keys: Vec<&String> = steps.as_object().unwrap().keys().collect();
    assert_eq!(keys, ["prepare", "start", "stream", "close"]);
    assert_eq!(steps["stream"], 3000);
    assert_eq!(steps["close"], 0);
}

/// A completed call the gateway refused does not end the run: the model sees the tool's error and goes on.
// decision: ORCH-733
#[tokio::test(start_paused = true)]
async fn a_refused_final_call_does_not_end_the_run() {
    let r = run_with(ends_on_dispatch(vec![
        tool("dispatch_task", "started", hand_off(), None),
        tool("dispatch_task", "completed", hand_off(), Some(true)),
        text("Which Friday do you mean?"),
        event(
            RuntimeEventKind::Final,
            json!({"text": "Which Friday do you mean?"}),
        ),
    ]))
    .await;
    assert_eq!(r.out.as_deref().unwrap(), "Which Friday do you mean?");
    assert_eq!(r.count("rt.read"), 4);
    // the switch is off: an ordinary answer closes first, as before
    assert!(r.at("rt.cancel.end") < r.at("returned"), "{:#?}", r.log);
    assert!(r.started("model.close").is_empty());
}

/// A call not named in `ends_on`, accepted, is a lookup: the run goes on to its answer.
// decision: ORCH-733
#[tokio::test(start_paused = true)]
async fn a_call_not_in_ends_on_does_not_end_the_run() {
    let r = run_with(ends_on_dispatch(vec![
        tool("memory_read", "started", json!({}), None),
        tool("memory_read", "completed", json!({}), Some(false)),
        event(RuntimeEventKind::Final, json!({"text": "You have none."})),
    ]))
    .await;
    assert_eq!(r.out.as_deref().unwrap(), "You have none.");
    assert_eq!(r.count("rt.read"), 3);
    assert_eq!(r.tool_events.len(), 2);
}

/// With an answer form nothing changes: today's tool events (no phase) reach the caller, and even an event shaped
/// like an accepted call of `ends_on` does not end the run; its answer is the form's.
// decision: ORCH-733
#[tokio::test(start_paused = true)]
async fn with_an_answer_form_ends_on_changes_nothing() {
    let r = run_with(Setup {
        schema: Some(json!({"type": "object"})),
        ..ends_on_dispatch(vec![
            event(
                RuntimeEventKind::ToolCall,
                json!({"tool": "mcp__ferb__dispatch_task", "name": "dispatch_task"}),
            ),
            tool("dispatch_task", "completed", hand_off(), Some(false)),
            event(RuntimeEventKind::Final, json!({"text": "{\"ok\": true}"})),
        ])
    })
    .await;
    assert_eq!(r.out.as_deref().unwrap(), "{\"ok\": true}");
    assert_eq!(r.count("rt.read"), 3);
    assert_eq!(r.tool_events.len(), 2);
    assert!(r.tool_events[0].get("phase").is_none());
}

/// RUN-445 counts a made-up call once though plain mode reports it twice (started, completed): 6 made-up calls
/// stay under the cap of 10.
// decision: ORCH-733
#[tokio::test(start_paused = true)]
async fn a_made_up_call_reported_in_two_phases_counts_once() {
    let mut events = Vec::new();
    for _ in 0..6 {
        for (phase, error) in [("started", None), ("completed", Some(true))] {
            let mut e = tool("invented_tool", phase, json!({}), error);
            e.data.insert("offered".into(), json!(false));
            events.push(e);
        }
    }
    events.push(event(RuntimeEventKind::Final, json!({"text": "done"})));
    let r = run_with(Setup::new(true, events)).await;
    assert_eq!(r.out.as_deref().unwrap(), "done", "{:?}", r.out);
    assert_eq!(r.tool_events.len(), 12);
}

/// ORCH-753: a run whose memory the runtime compacted has the gateway reset its looks (its next look at each place is
/// sent whole), once per compaction; a run without one resets nothing.
// decision: ORCH-753
#[tokio::test(start_paused = true)]
async fn a_compaction_resets_the_runs_looks_in_the_gateway() {
    let mut events = answered();
    events.insert(
        0,
        event(
            RuntimeEventKind::Compacted,
            json!({"by": "ferb", "removed_tokens": 900}),
        ),
    );
    let r = run_with(Setup {
        worker_id: "w-1",
        ..Setup::new(true, events)
    })
    .await;
    assert_eq!(r.out.as_deref().unwrap(), "the answer");
    assert_eq!(r.looks_reset, ["w-1"]);
    let r = run_with(Setup {
        worker_id: "w-2",
        ..Setup::new(true, answered())
    })
    .await;
    assert!(r.looks_reset.is_empty());
}
