//! The resource locks (`ferb/act/locks.py`) against `rust/fixtures/gateway/locks.json` (made by `make_locks.py`
//! beside it): every scenario's steps replayed on `ResourceLocks` on a single-threaded runtime, comparing what each
//! `try_hold` answered and who holds each key, byte for byte (`json.dumps`). The cases are the Python tests' lock
//! cases (tests/test_hangs_and_previews.py, tests/test_task_preview.py) and edges read from the code: a lock just
//! handed to a waking worker counts as busy, several keys are all taken or none, a read is looked at alongside.

use std::collections::HashMap;
use std::path::PathBuf;

use ferb_gateway::locks::{Held, ResourceLocks};
use serde_json::{json, Map, Value};

fn fixture() -> Value {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/gateway/locks.json");
    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
}

fn keys(st: &Value) -> Vec<String> {
    st["keys"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_owned())
        .collect()
}

async fn settle() {
    for _ in 0..10 {
        tokio::task::yield_now().await;
    }
}

enum Holding {
    Now(Held),
    Queued(tokio::task::JoinHandle<Held>),
    Bystander(Option<Held>),
}

async fn run(sc: &Value) -> Vec<Value> {
    let locks = ResourceLocks::new();
    let mut held: HashMap<String, Holding> = HashMap::new();
    let mut out = vec![];
    for st in sc["steps"].as_array().unwrap() {
        let id = st["id"].as_str().unwrap_or("").to_owned();
        let holder = st["holder"].as_str().unwrap_or("").to_owned();
        let reading = st["reading"].as_bool().unwrap_or(false);
        let during = st["during_reads"].as_bool().unwrap_or(false);
        let mut result = Value::Null;
        match st["do"].as_str().unwrap() {
            "hold" => {
                let h = locks.hold(&keys(st), &holder, reading).await.unwrap();
                held.insert(id, Holding::Now(h));
            }
            "hold_bg" => {
                let (l, k) = (locks.clone(), keys(st));
                let task = tokio::spawn(async move { l.hold(&k, &holder, reading).await.unwrap() });
                held.insert(id, Holding::Queued(task));
                settle().await;
            }
            "release" => match held.remove(&id).unwrap() {
                Holding::Now(h) => drop(h),
                Holding::Queued(t) => drop(t.await.unwrap()),
                Holding::Bystander(h) => drop(h),
            },
            "run" => settle().await,
            "cancel" => {
                if let Some(Holding::Queued(t)) = held.remove(&id) {
                    t.abort();
                }
                settle().await;
            }
            "try" => {
                result = match locks.try_hold(&keys(st), &holder, during) {
                    Ok(h) => json!(h.is_some()),
                    Err(e) => json!({"error": e}),
                };
            }
            "try_keep" => {
                result = match locks.try_hold(&keys(st), &holder, during) {
                    Ok(h) => {
                        let got = h.is_some();
                        held.insert(id, Holding::Bystander(h));
                        json!(got)
                    }
                    Err(e) => json!({"error": e}),
                };
            }
            "holders" => {
                let mut m = Map::new();
                for k in keys(st) {
                    m.insert(k.clone(), json!(locks.holder(&k)));
                }
                result = Value::Object(m);
            }
            other => panic!("step {other}"),
        }
        out.push(result);
    }
    out
}

// decision: DMN-403, DMN-406
#[tokio::test(flavor = "current_thread")]
async fn try_hold_matches_python() {
    let f = fixture();
    let scenarios = f["scenarios"].as_array().unwrap();
    let mut diffs = vec![];
    let mut compared = 0;
    for sc in scenarios {
        let got = run(sc).await;
        let want = sc["results"].as_array().unwrap();
        for (i, (g, w)) in got.iter().zip(want).enumerate() {
            compared += 1;
            if ferb_py::json::dumps(g) != ferb_py::json::dumps(w) {
                diffs.push(format!(
                    "{} step {i} ({}): python {w}, rust {g}",
                    sc["name"], sc["steps"][i]["do"]
                ));
            }
        }
        assert_eq!(got.len(), want.len());
    }
    assert!(diffs.is_empty(), "{}", diffs.join("\n"));
    println!(
        "{} scenarios, {compared} results equal, 0 differ",
        scenarios.len()
    );
}
