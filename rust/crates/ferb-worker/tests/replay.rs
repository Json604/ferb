//! Replays rust/fixtures/worker/worker.json (made by make.py from the Python worker) against the Rust worker with
//! the same recording fakes: every seam call of every run, in order, the result, the live lines and what is left of
//! the bookkeeping; the live lines of tool calls; the owner's words for every failure class; the composed lines.
//! The stall watchdog runs on tokio's paused clock (TEST-508: no real waiting).

mod support;

use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};

use ferb_context::prompts::PromptRoot;
use ferb_context::pyvalue::Py;
use ferb_context::tokens::Tokenizer;
use ferb_core::types::Task;
use ferb_worker::errors::{self, Error};
use ferb_worker::ports::{Grant, WorkerHost, WorkerTree};
use ferb_worker::{
    Attempt, Call, CallHooks, ConnectionNotice, OutcomeKind, RunOptions, Runs, Worker, WorkerConfig,
};
use serde_json::{json, Map, Value};
use support::*;

struct Diffs {
    count: usize,
    checked: usize,
    lines: Vec<String>,
}

impl Diffs {
    fn new() -> Self {
        Diffs {
            count: 0,
            checked: 0,
            lines: vec![],
        }
    }

    fn eq(&mut self, what: &str, expected: &Value, actual: &Value) {
        self.checked += 1;
        let (e, a) = (
            serde_json::to_string(expected).unwrap(),
            serde_json::to_string(actual).unwrap(),
        );
        if e != a {
            self.count += 1;
            if self.lines.len() < 30 {
                self.lines.push(format!(
                    "{what}\n  python: {}\n  rust:   {}",
                    cut(&e),
                    cut(&a)
                ));
            }
        }
    }

    fn log(&mut self, what: &str, expected: &Value, actual: &[Value]) {
        let expected = expected.as_array().unwrap();
        for i in 0..expected.len().max(actual.len()) {
            let e = expected.get(i).cloned().unwrap_or(Value::Null);
            let a = actual.get(i).cloned().unwrap_or(Value::Null);
            self.checked += 1;
            if serde_json::to_string(&e).unwrap() != serde_json::to_string(&a).unwrap() {
                self.count += 1;
                if self.lines.len() < 30 {
                    self.lines.push(format!(
                        "{what} log[{i}]\n  python: {}\n  rust:   {}",
                        cut(&e.to_string()),
                        cut(&a.to_string())
                    ));
                }
                return; // one difference shifts the rest
            }
        }
    }

    fn finish(self, what: &str) {
        eprintln!(
            "{what}: {} comparisons, {} differ",
            self.checked, self.count
        );
        assert!(
            self.count == 0,
            "{what}: {} differences:\n{}",
            self.count,
            self.lines.join("\n")
        );
    }
}

fn cut(s: &str) -> String {
    if s.chars().count() > 1500 {
        s.chars().take(1500).collect::<String>() + "…"
    } else {
        s.to_string()
    }
}

fn config(stall_s: f64) -> WorkerConfig {
    WorkerConfig {
        heartbeat_poll_s: 0.01,
        stall_s,
        tokenizer: Tokenizer::Utf8Bytes,
        prompts: PromptRoot::new(repo().join("ferb")).unwrap(),
        approach: "orchestrator/prompts".into(),
    }
}

fn setup(fx: &Value, sc: &Value) -> (Arc<TestHost>, Arc<Worker>) {
    set_tools(
        fx["tools"].as_array().unwrap().clone(),
        fx["observe"]
            .as_array()
            .unwrap()
            .iter()
            .map(|o| o.as_str().unwrap().to_string())
            .collect(),
    );
    let host = TestHost::new(sc, &fx["schemas"]);
    let stall = sc.get("stall_s").and_then(Value::as_f64).unwrap_or(180.0);
    let worker = Arc::new(Worker::new(
        host.clone() as Arc<dyn WorkerHost>,
        Arc::new(Runs::new()),
        config(stall),
    ));
    let _ = host.worker.set(Arc::downgrade(&worker));
    (host, worker)
}

fn result_json(a: &Attempt) -> Value {
    match a {
        Attempt::Report(py) => json!({"class": "report", "report": py.to_value()}),
        Attempt::Outcome(o) => {
            let class = match &o.kind {
                OutcomeKind::Plain => "Outcome",
                OutcomeKind::ServiceDown { .. } => "ServiceDown",
                OutcomeKind::Overflowed => "Overflowed",
            };
            let mut out = json!({"class": class, "outcome": {
                "task_id": o.outcome.task_id, "status": o.outcome.status.as_str(), "summary": o.outcome.summary,
                "artifacts": o.outcome.artifacts, "evidence": o.outcome.evidence}});
            if let OutcomeKind::ServiceDown { runtime, problem } = &o.kind {
                out["runtime"] = json!(runtime);
                out["problem"] = json!(problem);
            }
            out
        }
    }
}

/// A report as make.py wrote it (`jsonable(dict)`): Python's own JSON reading, then serde's.
fn report_value(py: &Py) -> Value {
    let text = ferb_context::pyvalue::dumps(py, ferb_context::pyvalue::Dumps::DEFAULT);
    serde_json::from_str(&text).unwrap_or(Value::Null)
}

fn live_json(
    rx: &mut Option<(
        String,
        tokio::sync::mpsc::UnboundedReceiver<Map<String, Value>>,
    )>,
) -> Value {
    let mut out = Map::new();
    if let Some((id, rx)) = rx {
        let mut items = vec![];
        while let Ok(e) = rx.try_recv() {
            items.push(Value::Object(e));
        }
        out.insert(id.clone(), json!(items));
    }
    Value::Object(out)
}

#[tokio::test(start_paused = true)]
async fn worker_runs_match_python() {
    let fx = fixture("worker", "worker.json");
    let mut d = Diffs::new();
    for sc_rec in fx["worker"].as_array().unwrap() {
        let name = sc_rec["name"].as_str().unwrap();
        let sc = &sc_rec["scenario"];
        let (host, worker) = setup(&fx, sc);
        let tree_spec = sc.get("tree").cloned().unwrap_or(json!({}));
        let tree = Arc::new(TestTree::from_spec(&tree_spec));
        *host.tree.lock().unwrap() = Some(tree.clone());
        let mut live = tree_spec
            .get("live")
            .and_then(Value::as_bool)
            .filter(|l| *l)
            .map(|_| (tree.id.clone(), worker.runs().open_live(&tree.id)));
        let wtree: Arc<dyn WorkerTree> = tree.clone();
        for (i, rec) in sc_rec["attempts"].as_array().unwrap().iter().enumerate() {
            let inputs = &rec["inputs"];
            *host.system.lock().unwrap() = inputs["system"].as_str().unwrap().to_string();
            let task = task_from(&inputs["task"]);
            let ctx = inputs["ctx"]
                .as_array()
                .unwrap()
                .iter()
                .map(block_from)
                .collect();
            let schema = &fx["schemas"][inputs["schema"]["schema"].as_str().unwrap()];
            let options = RunOptions {
                effort: inputs
                    .get("effort")
                    .and_then(Value::as_str)
                    .map(String::from),
                roles: inputs
                    .get("roles")
                    .and_then(Value::as_array)
                    .map(|r| r.iter().map(|x| x.as_str().unwrap().to_string()).collect())
                    .unwrap_or_default(),
                observe_only: inputs
                    .get("observe_only")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
            };
            let (result, worker_id) = worker
                .run_worker(
                    &wtree,
                    &task,
                    inputs["prompt"].as_str().unwrap(),
                    ctx,
                    schema,
                    inputs["span"].as_str().unwrap(),
                    &options,
                )
                .await
                .unwrap_or_else(|e| panic!("{name}: escaped: {e:?}"));
            let what = format!("{name}#{i}");
            d.log(&what, &rec["log"], &host.take_log());
            let mut got = result_json(&result);
            if let Attempt::Report(py) = &result {
                got["report"] = report_value(py);
            }
            d.eq(&format!("{what} result"), &rec["result"], &got);
            d.eq(
                &format!("{what} worker id"),
                &rec["worker_id"],
                &json!(worker_id),
            );
            d.eq(
                &format!("{what} result_revision"),
                &rec["result_revision"],
                &json!(tree.result_revision.load(Ordering::SeqCst)),
            );
            let delivered: Map<String, Value> = tree
                .steering
                .lock()
                .await
                .delivered
                .iter()
                .map(|(k, v)| (k.clone(), json!(v)))
                .collect();
            d.eq(
                &format!("{what} delivered"),
                &rec["delivered"],
                &Value::Object(delivered),
            );
        }
        d.eq(
            &format!("{name} leftovers"),
            &sc_rec["leftovers"],
            &worker.runs().left(),
        );
        d.eq(
            &format!("{name} live"),
            &sc_rec["live"],
            &live_json(&mut live),
        );
    }
    d.finish("worker runs");
}

#[tokio::test(start_paused = true)]
async fn calls_match_python() {
    let fx = fixture("worker", "worker.json");
    let mut d = Diffs::new();
    for rec in fx["calls"].as_array().unwrap() {
        let name = rec["name"].as_str().unwrap();
        let sc = &rec["scenario"];
        let inputs = &rec["inputs"];
        let (host, worker) = setup(&fx, sc);
        let tree = sc.get("tree").map(|t| Arc::new(TestTree::from_spec(t)));
        *host.tree.lock().unwrap() = tree.clone();
        let mut live = tree
            .as_ref()
            .filter(|_| sc["tree"].get("live").and_then(Value::as_bool) == Some(true))
            .map(|t| (t.id.clone(), worker.runs().open_live(&t.id)));
        let wtree: Option<Arc<dyn WorkerTree>> = tree.clone().map(|t| t as Arc<dyn WorkerTree>);
        let task: Task = task_from(&inputs["task"]);
        let rt: Arc<dyn ferb_worker::ports::ModelRuntime> =
            Arc::new(ScriptRuntime::new(host.clone(), sc["runtimes"][0].clone()));
        let token = if sc.get("token").and_then(Value::as_bool) == Some(true) {
            host.issue_token(&task.id, "", &task.scopes, task.identity, 60.0, false)
        } else {
            String::new()
        };
        let seen: Arc<Mutex<Map<String, Value>>> = Arc::new(Mutex::new(Map::new()));
        for k in ["delta", "usage", "tool", "timing", "notify"] {
            seen.lock().unwrap().insert(k.into(), json!([]));
        }
        let push = |k: &'static str| {
            let seen = seen.clone();
            move |v: Value| seen.lock().unwrap()[k].as_array_mut().unwrap().push(v)
        };
        let cb: Vec<&str> = inputs["callbacks"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c.as_str().unwrap())
            .collect();
        let mut hooks = CallHooks::default();
        if cb.contains(&"delta") {
            let p = push("delta");
            hooks.on_delta = Some(Arc::new(move |t: &str| p(json!(t))));
        }
        if cb.contains(&"usage") {
            let p = push("usage");
            hooks.on_usage = Some(Arc::new(move |u| p(Value::Object(u))));
        }
        if cb.contains(&"tool") {
            let p = push("tool");
            hooks.on_tool = Some(Arc::new(move || p(json!(true))));
        }
        if cb.contains(&"timing") {
            let p = push("timing");
            hooks.on_timing = Some(Arc::new(move |t| p(Value::Object(t))));
        }
        if cb.contains(&"notify") {
            let p = push("notify");
            hooks.notify = Some(Arc::new(move |e| p(Value::Object(e))));
        }
        let images = inputs["images"]
            .as_array()
            .unwrap()
            .iter()
            .map(|i| {
                ferb_core::types::ModelImage::new(
                    unhex(i["data"].as_str().unwrap()),
                    i["ref"].as_str().unwrap(),
                )
            })
            .collect();
        let call = Call {
            task: &task,
            system: inputs["system"].as_str().unwrap().to_string(),
            context: inputs["ctx"]
                .as_array()
                .unwrap()
                .iter()
                .map(block_from)
                .collect(),
            token: &token,
            parent_span: "span-parent",
            tree: wtree.as_ref(),
            worker_id: "",
            images,
            hooks,
            retry_silent: inputs["retry_silent"].as_bool().unwrap(),
        };
        let out = worker.run_call(&rt, call).await;
        let got = match &out {
            Ok(text) => json!({"returned": text}),
            Err(e) => json!({"raised": exc_json(e)}),
        };
        d.log(name, &rec["log"], &host.take_log());
        d.eq(&format!("{name} outcome"), &rec["outcome"], &got);
        d.eq(
            &format!("{name} callbacks"),
            &rec["callbacks_seen"],
            &Value::Object(seen.lock().unwrap().clone()),
        );
        d.eq(
            &format!("{name} leftovers"),
            &rec["leftovers"],
            &worker.runs().left(),
        );
        d.eq(&format!("{name} live"), &rec["live"], &live_json(&mut live));
    }
    d.finish("calls");
}

#[tokio::test(start_paused = true)]
async fn tool_lines_match_python() {
    let fx = fixture("worker", "worker.json");
    let mut d = Diffs::new();
    for rec in fx["tool_progress"].as_array().unwrap() {
        let case = &rec["case"];
        let name = case["name"].as_str().unwrap();
        let (host, worker) = setup(&fx, case);
        let tree = Arc::new(TestTree::from_spec(&json!({})));
        let mut live = Some((tree.id.clone(), worker.runs().open_live(&tree.id)));
        worker
            .runs()
            .set_tree("t1.worker", tree.clone() as Arc<dyn WorkerTree>);
        let g = case.get("grant").cloned().unwrap_or(json!({}));
        let grant = Grant {
            task_id: g
                .get("task_id")
                .and_then(Value::as_str)
                .unwrap_or("t1.worker")
                .into(),
            worker_id: g
                .get("worker_id")
                .and_then(Value::as_str)
                .unwrap_or("w-1")
                .into(),
            scopes: Default::default(),
            identity: ferb_core::types::Identity::Owner,
            observe_only: false,
        };
        if case.get("lost").and_then(Value::as_bool) == Some(true) {
            let log = host.clone();
            let notice = Arc::new(ConnectionNotice::new(
                "codex",
                Arc::new(move |e: Map<String, Value>| log.rec("notice", Value::Object(e))),
                true,
            ));
            notice.set_lost(true);
            worker.runs().set_connection("w-1", notice);
        }
        host.clock.advance(125.5);
        let args = case["args"].as_object().cloned().unwrap_or_default();
        let _ = worker.tool_progress(
            &grant,
            case["tool"].as_str().unwrap(),
            &args,
            case["kind"].as_str().unwrap(),
            case.get("site").and_then(Value::as_str),
        );
        d.log(name, &rec["log"], &host.take_log());
        d.eq(&format!("{name} live"), &rec["live"], &live_json(&mut live));
        let mut last = Map::new();
        if let Some(step) = worker.runs().last_step("t1") {
            last.insert("t1".into(), step);
        }
        d.eq(
            &format!("{name} last step"),
            &rec["last_steps"],
            &Value::Object(last),
        );
    }
    d.finish("tool lines");
}

#[test]
fn owner_words_match_python() {
    let fx = fixture("worker", "worker.json");
    let mut d = Diffs::new();
    let online = || Some(true);
    for rec in fx["reasons"].as_array().unwrap() {
        let name = rec["name"].as_str().unwrap();
        let e = match rec.get("validation") {
            Some(v) => {
                let input = Py::from_value(&v["input"]);
                let err = ferb_orchestrator::validate::validate_by_name(
                    v["model"].as_str().unwrap(),
                    &input,
                    None,
                )
                .expect_err("the fixture's input is invalid");
                Error::validation(err)
            }
            None => error_from_exc(&rec["exc"]),
        };
        d.eq(
            &format!("{name} class"),
            &rec["exc"]["class"],
            &json!(e.class()),
        );
        d.eq(
            &format!("{name} str"),
            &rec["exc"]["str"],
            &json!(e.to_string()),
        );
        d.eq(
            &format!("{name} reason"),
            &rec["reason"],
            &json!(errors::reason(&e, None, &online)),
        );
        d.eq(
            &format!("{name} reason redacted"),
            &rec["reason_redacted"],
            &json!(errors::reason(&e, Some(&redact_text), &online)),
        );
        let what = if rec.get("validation").is_some() {
            "read the report"
        } else {
            "finish the step"
        };
        d.eq(
            &format!("{name} couldnt"),
            &rec["couldnt"],
            &json!(errors::couldnt(what, &e, Some(&redact_text), &online)),
        );
    }
    let lines = &fx["lines"];
    for c in lines["service_problem"].as_array().unwrap() {
        let a = &c["args"];
        let route = c["route"].as_bool().unwrap();
        let text = errors::service_problem(
            a[0].as_str().unwrap(),
            a[1].as_str().unwrap(),
            a[2].as_bool().unwrap(),
            &|| Some(route),
        );
        d.eq("service_problem", &c["text"], &json!(text));
    }
    for c in lines["retry_stall_problem"].as_array().unwrap() {
        let a = &c["args"];
        let route = c["route"].as_bool().unwrap();
        let text = errors::retry_stall_problem(
            a[0].as_str().unwrap(),
            a[1].as_str().unwrap(),
            a[2].as_f64().unwrap(),
            &|| Some(route),
        );
        d.eq("retry_stall_problem", &c["text"], &json!(text));
    }
    for c in lines["connection_lost"].as_array().unwrap() {
        let a = &c["args"];
        let route = c["route"].as_bool().unwrap();
        let text = errors::connection_lost(
            a[0].as_str().unwrap(),
            a[1].as_str().unwrap(),
            a[2].as_bool().unwrap(),
            &|| Some(route),
        );
        d.eq("connection_lost", &c["text"], &json!(text));
    }
    for c in lines["account_problem"].as_array().unwrap() {
        let a = &c["args"];
        d.eq(
            "account_problem",
            &c["text"],
            &json!(errors::account_problem(
                a[0].as_str().unwrap(),
                a[1].as_str().unwrap()
            )),
        );
    }
    for c in lines["too_large_problem"].as_array().unwrap() {
        let a = &c["args"];
        let text = errors::too_large_problem(
            a[0].as_u64().unwrap(),
            a[1].as_u64().unwrap(),
            a[2].as_str().unwrap(),
        );
        d.eq("too_large_problem", &c["text"], &json!(text));
    }
    for c in lines["reserve_tokens"].as_array().unwrap() {
        let tools = c["tools"].as_array().unwrap().clone();
        let schema = c["schema"]
            .get("schema")
            .map(|k| fx["schemas"][k.as_str().unwrap()].clone());
        let n = ferb_worker::reserve_tokens(
            Tokenizer::Utf8Bytes,
            c["system"].as_str().unwrap(),
            &tools,
            schema.as_ref(),
        );
        d.eq("reserve_tokens", &c["tokens"], &json!(n));
    }
    let k = &lines["constants"];
    d.eq(
        "NO_TOOLS_LOADED",
        &k["NO_TOOLS_LOADED"],
        &json!(ferb_worker::NO_TOOLS_LOADED),
    );
    d.eq("BUG", &k["BUG"], &json!(errors::BUG));
    d.eq(
        "REASON_CHARS",
        &k["REASON_CHARS"],
        &json!(errors::REASON_CHARS),
    );
    d.eq(
        "ACCOUNT_PROBLEMS",
        &k["ACCOUNT_PROBLEMS"],
        &json!(errors::ACCOUNT_PROBLEMS),
    );
    d.finish("owner words");
}

/// The supervisor runs attempts and calls on spawned tasks: both futures must be `Send` (checked when this compiles).
#[allow(dead_code)]
fn attempts_and_calls_can_be_spawned(
    worker: Arc<Worker>,
    tree: Arc<dyn WorkerTree>,
    task: Task,
    rt: Arc<dyn ferb_worker::ports::ModelRuntime>,
) {
    let w = worker.clone();
    let (t, k) = (tree.clone(), task.clone());
    tokio::spawn(async move {
        let _ = w
            .run_worker(
                &t,
                &k,
                "worker.md",
                vec![],
                &Value::Null,
                "span",
                &RunOptions::default(),
            )
            .await;
    });
    tokio::spawn(async move {
        let call = Call {
            task: &task,
            system: String::new(),
            context: vec![],
            token: "",
            parent_span: "span",
            tree: Some(&tree),
            worker_id: "",
            images: vec![],
            hooks: CallHooks::default(),
            retry_silent: true,
        };
        let _ = worker.run_call(&rt, call).await;
    });
}

/// Not from the fixture: a Python attempt cancelled mid-run still runs its `finally` blocks; a dropped Rust future
/// does the same bookkeeping in `Cleanup` (the run forgotten and cancelled, liveness ended, the token revoked).
#[tokio::test(start_paused = true)]
async fn a_dropped_attempt_cleans_up() {
    let fx = fixture("worker", "worker.json");
    let sc = json!({"runtimes": [{"calls": [{"steps": [{"event": "message", "data": {"text": "x"}}, {"stall": true}]}]}]});
    let (host, worker) = setup(&fx, &sc);
    let tree = Arc::new(TestTree::from_spec(&json!({})));
    *host.tree.lock().unwrap() = Some(tree.clone());
    let wtree: Arc<dyn WorkerTree> = tree;
    let task = task_from(&fx["worker"][0]["attempts"][0]["inputs"]["task"]);
    let schema = json!({});
    let options = RunOptions::default();
    let attempt = worker.run_worker(
        &wtree,
        &task,
        "worker.md",
        vec![],
        &schema,
        "span",
        &options,
    );
    assert!(
        tokio::time::timeout(std::time::Duration::from_secs(1), attempt)
            .await
            .is_err()
    );
    tokio::task::yield_now().await;
    let log = host.take_log();
    let tail: Vec<&str> = log
        .iter()
        .rev()
        .take(5)
        .map(|e| e[0].as_str().unwrap())
        .collect();
    assert_eq!(
        tail,
        [
            "rt.cancel",
            "gateway.forget_calls",
            "tokens.revoke_worker",
            "activity.end",
            "gateway.calls_refused"
        ]
    );
    let left = worker.runs().left();
    assert_eq!(left["active"], json!([]));
    assert_eq!(left["worker_runs"], json!([]));
}
