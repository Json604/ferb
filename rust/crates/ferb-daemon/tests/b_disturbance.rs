//! `ferb/daemon/disturbance.py` against `rust/fixtures/daemon-b/disturbance.json` (made by `disturbance.py` beside
//! it): every scenario's steps replayed on the Rust sentinel with the same fakes, comparing every record, every step's
//! result and the desktop probe's calls byte for byte (`json.dumps`), and the pure helpers.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use ferb_core::when::DateTime;
use ferb_daemon::areas_b::disturbance::{
    elapsed, screen_changes, told, DesktopRead, DisturbanceSentinel, Seams,
};
use serde_json::{Map, Value};

fn fixture() -> Value {
    let path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/daemon-b/disturbance.json");
    let path = if path.exists() {
        path
    } else {
        PathBuf::from("/Users/kartikey/Desktop/FERB/rust/fixtures/daemon-b/disturbance.json")
    };
    serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap()
}

fn dumps(v: &Value) -> String {
    ferb_py::json::dumps(v)
}

/// A Python exception as `f"{type(e).__name__}: {e}"` writes it (a KeyError's text is its key's repr).
fn py_err(spec: &str) -> String {
    let (kind, text) = spec.split_once(": ").unwrap_or((spec, ""));
    if kind == "KeyError" {
        format!("{kind}: '{text}'")
    } else {
        format!("{kind}: {text}")
    }
}

#[derive(Default)]
struct World {
    state: Map<String, Value>,
    fail: VecDeque<String>,
    owner: Value,
    procs: Value,
    now: Option<DateTime>,
    mono: f64,
    paused: bool,
    arm_pause: bool,
    probe: Vec<Value>,
    records: Vec<Value>,
}

type W = Arc<Mutex<World>>;

struct Probe {
    w: W,
    hooks: bool,
}

impl DesktopRead for Probe {
    fn start(&self) {
        if self.hooks {
            self.w.lock().unwrap().probe.push("start".into());
        }
    }
    fn pump(&self) {
        if self.hooks {
            self.w.lock().unwrap().probe.push("pump".into());
        }
    }
    fn close(&self) {
        if self.hooks {
            self.w.lock().unwrap().probe.push("close".into());
        }
    }
    fn read(&self) -> Result<Map<String, Value>, String> {
        let mut w = self.w.lock().unwrap();
        w.probe.push("read".into());
        if w.arm_pause {
            w.paused = true;
        }
        if let Some(e) = w.fail.pop_front() {
            return Err(py_err(&e));
        }
        Ok(w.state.clone())
    }
}

fn obj(v: &Value) -> Map<String, Value> {
    v.as_object().cloned().unwrap_or_default()
}

async fn run(sc: &Value) -> Vec<String> {
    let w: W = Arc::new(Mutex::new(World {
        state: obj(&sc["state"]),
        owner: sc["owner_input"].clone(),
        procs: sc["processes"].clone(),
        now: ferb_core::when::parse("2026-10-09T16:35:55+00:00"),
        mono: 1000.0,
        paused: sc["paused"].as_bool().unwrap_or(false),
        ..World::default()
    }));
    let read = Arc::new(Probe {
        w: w.clone(),
        hooks: sc["probe_hooks"].as_bool().unwrap_or(true),
    });
    let wr = w.clone();
    let record = Arc::new(move |task: &str, kind: &str, data: Value| {
        wr.lock()
            .unwrap()
            .records
            .push(serde_json::json!([task, kind, data]));
    });
    let mut seams = Seams::new(read, record);
    let wo = w.clone();
    seams.owner_input = Arc::new(move || match wo.lock().unwrap().owner.clone() {
        Value::String(s) => Err(py_err(&s)),
        other => Ok(obj(&other)),
    });
    let wp = w.clone();
    seams.processes = Arc::new(move || match wp.lock().unwrap().procs.clone() {
        Value::String(s) => Err(py_err(&s)),
        Value::Array(a) => Ok(a),
        _ => Ok(vec![]),
    });
    let wn = w.clone();
    seams.now = Arc::new(move || wn.lock().unwrap().now.unwrap());
    seams.local = Arc::new(|d: &DateTime| *d);
    let wpa = w.clone();
    seams.paused = Arc::new(move || wpa.lock().unwrap().paused);
    let wm = w.clone();
    seams.monotonic = Arc::new(move || {
        let mut w = wm.lock().unwrap();
        w.mono += 0.25;
        w.mono
    });
    let s = DisturbanceSentinel::new(seams);
    let mut diffs = vec![];
    let results = sc["results"].as_array().unwrap();
    for (i, st) in sc["steps"].as_array().unwrap().iter().enumerate() {
        let txt = |k: &str| st[k].as_str().unwrap_or("").to_owned();
        let out: Value = match st["do"].as_str().unwrap() {
            "set" => {
                w.lock().unwrap().state = obj(&st["state"]);
                Value::Null
            }
            "patch" => {
                let mut g = w.lock().unwrap();
                for (k, v) in obj(&st["state"]) {
                    g.state.insert(k, v);
                }
                for k in st["drop"].as_array().into_iter().flatten() {
                    g.state.shift_remove(k.as_str().unwrap());
                }
                Value::Null
            }
            "read_fails" => {
                w.lock().unwrap().fail.push_back(txt("error"));
                Value::Null
            }
            "owner_input" => {
                w.lock().unwrap().owner = st["value"].clone();
                Value::Null
            }
            "processes" => {
                w.lock().unwrap().procs = st["value"].clone();
                Value::Null
            }
            "now" => {
                let mut g = w.lock().unwrap();
                let us = (st["advance_s"].as_f64().unwrap() * 1e6).round() as i64;
                g.now = Some(g.now.unwrap() + chrono::Duration::microseconds(us));
                Value::Null
            }
            "mono" => {
                w.lock().unwrap().mono += st["advance_s"].as_f64().unwrap();
                Value::Null
            }
            "paused" => {
                w.lock().unwrap().paused = st["on"].as_bool().unwrap();
                Value::Null
            }
            "arm_pause" => {
                w.lock().unwrap().arm_pause = st["on"].as_bool().unwrap();
                Value::Null
            }
            "start" => {
                s.start(&txt("task")).await;
                Value::Null
            }
            "finish" => {
                s.finish(&txt("task")).await;
                Value::Null
            }
            "shutdown" => {
                s.shutdown().await;
                Value::Null
            }
            "finish_and_start" => {
                let (a, b) = (txt("finish"), txt("start"));
                tokio::join!(s.finish(&a), s.start(&b));
                Value::Null
            }
            "finish_and_shutdown" => {
                let t = txt("task");
                tokio::join!(s.finish(&t), s.shutdown());
                Value::Null
            }
            "before" => {
                s.before_action(
                    &txt("task"),
                    &txt("span"),
                    &txt("tool"),
                    &obj(&st["args"]),
                    st["on_screen"].as_bool().unwrap_or(false),
                )
                .await;
                Value::Null
            }
            "after" => Value::Object(s.after_action(&txt("span")).await),
            "call_started" => {
                s.call_started(&txt("task"), &txt("span"), &txt("tool"));
                Value::Null
            }
            "call_ended" => {
                s.call_ended(&txt("span"));
                Value::Null
            }
            "sound" => {
                s.sound_changed(&st["tab"], &obj(&st["change"]));
                Value::Null
            }
            "sample" => s.sample().await.map_or(Value::Null, Value::Object),
            "context" => Value::String(s.context(&txt("task"))),
            "facts" => Value::Array(s.facts(&txt("task"))),
            "active" => Value::Array(s.active().into_iter().map(Value::String).collect()),
            other => panic!("step {other}"),
        };
        if dumps(&out) != dumps(&results[i]) {
            diffs.push(format!(
                "{} step {i} {}:\n  python: {}\n  rust:   {}",
                sc["name"],
                st["do"],
                dumps(&results[i]),
                dumps(&out)
            ));
        }
    }
    let g = w.lock().unwrap();
    for (what, got, want) in [
        ("records", Value::Array(g.records.clone()), &sc["records"]),
        ("probe", Value::Array(g.probe.clone()), &sc["probe"]),
    ] {
        if dumps(&got) != dumps(want) {
            diffs.push(format!(
                "{} {what}:\n  python: {}\n  rust:   {}",
                sc["name"],
                dumps(want),
                dumps(&got)
            ));
        }
    }
    diffs
}

// decision: DMN-5, DMN-501, HAND-207, HAND-213, HAND-230, SENSE-406
#[tokio::test]
async fn every_scenario_matches_python() {
    let f = fixture();
    let scenarios = f["scenarios"].as_array().unwrap();
    let mut diffs = vec![];
    for sc in scenarios {
        diffs.extend(run(sc).await);
    }
    assert!(
        diffs.is_empty(),
        "{} difference(s):\n{}",
        diffs.len(),
        diffs.join("\n")
    );
    println!("{} scenarios, 0 differ", scenarios.len());
}

// decision: DMN-5, HAND-212
#[test]
fn the_pure_helpers_match_python() {
    let f = fixture();
    let p = &f["pure"];
    let mut n = 0;
    for case in p["screen_changes"].as_array().unwrap() {
        let got = screen_changes(
            Some(&case[0]).filter(|v| !v.is_null()),
            Some(&case[1]).filter(|v| !v.is_null()),
        );
        assert_eq!(dumps(&Value::Object(got)), dumps(&case[2]), "{case}");
        n += 1;
    }
    for case in p["told"].as_array().unwrap() {
        assert_eq!(
            dumps(&Value::Object(told(&obj(&case[0])))),
            dumps(&case[1]),
            "{case}"
        );
        n += 1;
    }
    for case in p["elapsed"].as_array().unwrap() {
        let got = elapsed(case[0].as_f64().unwrap());
        assert_eq!(dumps(&serde_json::json!(got)), dumps(&case[1]), "{case}");
        n += 1;
    }
    println!("{n} pure cases, 0 differ");
}
