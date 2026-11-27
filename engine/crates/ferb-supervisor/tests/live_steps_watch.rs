//! A watch that starts mid-task gets each running step's state at once (owner, 2026-11-28: a phone paired again
//! during a task showed its card with no step states until each step next changed). Live state is never stored
//! (ENG-8), so the catch-up alone had none: right after `caught_up` the watch now sends the last live frame of each
//! step of a running task, and an older state coming live after it is left out. An ended task's steps are not sent.
// decision: ENG-25

mod common;
mod support_control;

use std::sync::Arc;

use ferb_rpc::server::EventStream;
use serde_json::{json, Value};
use support_control::{world_with, World};

const REQUEST: &str = "rq-invented-live";

fn conversation(w: &World) -> String {
    let db = w.db.lock().unwrap();
    ferb_supervisor::flow::chat_log::current(db.conn()).unwrap()
}

fn step(task: &str, step: &str, status: &str, seq: &str) -> Value {
    json!({"type": "step", "task_id": task, "request_id": REQUEST, "step": step,
           "status": status, "waits_on": [], "attempt": 1, "seq": seq})
}

fn tell(w: &World, c: &str, frame: Value) {
    let inner = w.sup.inner();
    inner.chat_log.frame(inner, c, REQUEST, &frame);
    w.sup.chat_flush();
}

fn watch(w: &World) -> Box<dyn EventStream> {
    let p = json!({}).as_object().unwrap().clone();
    ferb_supervisor::flow::entries_rpc::watch(w.sup.inner(), &p)
        .unwrap_or_else(|_| panic!("the watch starts"))
}

/// The next `n` frames (each is there already: none waits on time).
fn next(rt: &tokio::runtime::Runtime, s: &mut Box<dyn EventStream>, n: usize) -> Vec<Value> {
    rt.block_on(async {
        let mut out = Vec::new();
        for _ in 0..n {
            out.push(Value::Object(s.next().await.unwrap().unwrap()));
        }
        out
    })
}

#[test]
fn a_watch_that_starts_mid_task_gets_each_running_step_s_state_after_its_catch_up() {
    let w = world_with("one_step_task", |_| {});
    let rt = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    let c = conversation(&w);
    tell(
        &w,
        &c,
        step("tk-a", "s1", "running", "2026-01-01T00:00:01.000000+00:00"),
    );
    tell(
        &w,
        &c,
        step("tk-a", "s2", "waiting", "2026-01-01T00:00:02.000000+00:00"),
    );
    tell(
        &w,
        &c,
        step("tk-a", "s1", "done", "2026-01-01T00:00:03.000000+00:00"),
    );
    tell(
        &w,
        &c,
        step("tk-b", "s1", "running", "2026-01-01T00:00:04.000000+00:00"),
    );
    // tk-b ends: its steps are no longer live
    let inner = Arc::clone(w.sup.inner());
    inner.chat_log.bus(
        &inner,
        "task.result",
        json!({"task_id": "tk-b", "request_id": REQUEST, "status": "done"}),
    );
    w.sup.chat_flush();

    let mut s = watch(&w);
    let got = next(&rt, &mut s, 3);
    assert_eq!(got[0]["type"], "caught_up", "{got:?}");
    let shown: Vec<(String, String, String)> = got[1..]
        .iter()
        .map(|f| {
            assert_eq!(f["type"], "live");
            assert_eq!(f["request_id"], REQUEST);
            let fr = &f["frame"];
            (
                fr["task_id"].as_str().unwrap().into(),
                fr["step"].as_str().unwrap().into(),
                fr["status"].as_str().unwrap().into(),
            )
        })
        .collect();
    assert_eq!(
        shown,
        [
            ("tk-a".to_string(), "s2".to_string(), "waiting".to_string()),
            ("tk-a".to_string(), "s1".to_string(), "done".to_string()),
        ]
    );

    // live from here: an older state of a step already sent is left out, a newer one comes
    tell(
        &w,
        &c,
        step("tk-a", "s2", "running", "2026-01-01T00:00:01.500000+00:00"),
    );
    tell(
        &w,
        &c,
        step("tk-a", "s2", "running", "2026-01-01T00:00:05.000000+00:00"),
    );
    let got = next(&rt, &mut s, 1);
    assert_eq!(
        got[0]["frame"]["seq"], "2026-01-01T00:00:05.000000+00:00",
        "{got:?}"
    );
}
