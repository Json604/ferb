//! `ferb/daemon/task_preview.py` against `engine/fixtures/daemon-b/task_preview.json` (made by `task_preview.py` beside
//! it): every scenario's steps replayed on the Rust previews with the real resource locks (ferb-mac-hands) and the
//! same scripted captures, comparing each answer, snapshot and journal note byte for byte (`json.dumps`). Time is a
//! fake clock: readings move only by a step or a capture's cost, and a request's limit runs out exactly when a `slow`
//! capture starts (no real waiting).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use ferb_daemon::areas_b::task_preview::{BoxFuture, PreviewAnswer, PreviewClock, TaskPreviews};
use ferb_mac_hands::error::HandError;
use ferb_mac_hands::locks::{Held, ResourceLocks};
use ferb_mac_hands::point::{OwnerOnlyFrame, PreviewFrameFn, PreviewTarget, ResolveFn};
use serde_json::{json, Map, Value};
use tokio::sync::watch;

fn fixture() -> Value {
    let path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/daemon-b/task_preview.json");
    let path = if path.exists() {
        path
    } else {
        PathBuf::from("/Users/kartikey/Desktop/FERB/rust/fixtures/daemon-b/task_preview.json")
    };
    serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap()
}

fn dumps(v: &Value) -> String {
    ferb_py::json::dumps(v)
}

struct Clock {
    t: Mutex<f64>,
    expire: watch::Sender<u64>,
}

impl PreviewClock for Clock {
    fn monotonic(&self) -> f64 {
        *self.t.lock().unwrap()
    }
    fn sleep(&self, _seconds: f64) -> BoxFuture<()> {
        let mut rx = self.expire.subscribe();
        rx.mark_unchanged();
        Box::pin(async move {
            let _ = rx.changed().await;
        })
    }
}

struct World {
    clock: Arc<Clock>,
    surfaces: Map<String, Value>,
    used: Mutex<HashMap<String, usize>>,
    gates: Mutex<HashMap<String, watch::Sender<bool>>>,
    entered: Mutex<HashMap<String, watch::Sender<bool>>>,
}

impl World {
    fn event(
        table: &Mutex<HashMap<String, watch::Sender<bool>>>,
        name: &str,
    ) -> watch::Sender<bool> {
        table
            .lock()
            .unwrap()
            .entry(name.to_owned())
            .or_insert_with(|| watch::channel(false).0)
            .clone()
    }

    fn behavior(&self, name: &str) -> Value {
        let queue = self.surfaces[name].as_array().unwrap();
        let mut used = self.used.lock().unwrap();
        let n = used.entry(name.to_owned()).or_default();
        let b = queue[(*n).min(queue.len() - 1)].clone();
        *n += 1;
        b
    }

    fn advance(&self, s: f64) {
        *self.clock.t.lock().unwrap() += s;
    }
}

fn error(b: &Value) -> HandError {
    let text = b["text"].as_str().unwrap_or("").to_owned();
    match b["raise"].as_str().unwrap() {
        "Withheld" => HandError::Withheld(text),
        "Refused" => HandError::Refused(text),
        "StaleRef" => HandError::StaleRef(text),
        "NoTarget" => HandError::NoTarget(text),
        "ScreenOff" => HandError::ScreenOff(text),
        "CaptureBusy" => HandError::CaptureBusy(text),
        "RuntimeError" => HandError::Other(text),
        "CaptureError" => HandError::Capture {
            message: text,
            domain: b["domain"].as_str().unwrap().to_owned(),
            code: b["code"].as_i64(),
        },
        other => panic!("error {other}"),
    }
}

fn capture_of(world: Arc<World>, name: String) -> PreviewFrameFn {
    Arc::new(move || {
        let world = world.clone();
        let name = name.clone();
        Box::pin(async move {
            let b = world.behavior(&name);
            if let Some(g) = b["gate"].as_str() {
                let _ = World::event(&world.entered, g).send_replace(true);
                let mut rx = World::event(&world.gates, g).subscribe();
                let _ = rx.wait_for(|v| *v).await;
            }
            if b["slow"].as_bool() == Some(true) {
                world.clock.expire.send_modify(|n| *n += 1); // the request's limit runs out meanwhile
                std::future::pending::<()>().await;
            }
            world.advance(b["cost_s"].as_f64().unwrap_or(0.0));
            if b.get("raise").is_some() {
                return Err(error(&b));
            }
            Ok((
                OwnerOnlyFrame::captured(b["frame"].as_str().unwrap().as_bytes().to_vec()),
                b["note"].as_str().unwrap_or("").to_owned(),
            ))
        })
    })
}

fn target(world: &Arc<World>, spec: &Value) -> Option<PreviewTarget> {
    if spec.is_null() {
        return None;
    }
    let scope = spec["scope"].as_str().unwrap().to_owned();
    let label = spec["label"].as_str().unwrap().to_owned();
    let resolve: Option<ResolveFn> = spec["surface"].as_str().map(|surface| {
        let world = world.clone();
        let surface = surface.to_owned();
        let rs = spec.get("resolve").cloned().unwrap_or(json!({}));
        let scope = scope.clone();
        let f: ResolveFn = Arc::new(move || {
            let world = world.clone();
            let surface = surface.clone();
            let rs = rs.clone();
            let scope = scope.clone();
            Box::pin(async move {
                world.advance(rs["cost_s"].as_f64().unwrap_or(0.0));
                if rs.get("raise").is_some() {
                    return Err(error(&rs));
                }
                let mut facts = Map::new();
                facts.insert("scope".into(), json!(scope));
                Ok((facts, capture_of(world, surface)))
            })
        });
        f
    });
    Some(PreviewTarget {
        scope,
        label,
        resolve,
    })
}

fn shown(a: &PreviewAnswer) -> Value {
    a.to_value(|b| Value::String(String::from_utf8_lossy(b).into_owned()))
}

async fn yields(n: usize) {
    for _ in 0..n {
        tokio::task::yield_now().await;
    }
}

/// ENG-6: the deliberate differences, each named.
/// 1. Python's frozen copy of the last frame (a new surface declared before it has a frame of its own) kept the image
///    but not its `seq` and capture time, so the answer said `seq: 0` and an age counted from the monotonic clock's
///    zero; Rust keeps both.
/// 2. APP-922 (owner): the preview shows frames as captured. There is no text-reading pass, so no snapshot has an
///    `<kind>_ocr_ms` phase; and the one frame Python's sanitizer refused (`failures_journaled`'s `bad`, journaled as
///    preview.failed ValueError) is shown, so that scenario's later answers, snapshot and notes are as written here.
fn fixed(scenario: &str, step: usize, do_: &str, expected: &Value) -> Option<Value> {
    if scenario == "no_picture_first" && step == 9 {
        let mut v = expected.clone();
        assert_eq!(v["seq"], json!(0));
        v["seq"] = json!(1);
        v["age_ms"] = json!(0);
        return Some(v);
    }
    let line = json!({"state": "working", "line": "Working in Notes", "worker": "t.worker", "step": "worker"});
    let with = |extra: Value| {
        let mut v = line.clone();
        for (k, x) in extra.as_object().unwrap() {
            v[k] = x.clone();
        }
        v
    };
    if scenario == "failures_journaled" {
        let gone = |text: &str| {
            with(
                json!({"image": "bad", "fresh": false, "seq": 1, "age_ms": 0,
                        "problem": {"kind": "gone", "text": text}}),
            )
        };
        let rust = match step {
            4 => Some(with(
                json!({"image": "bad", "fresh": true, "seq": 1, "age_ms": 0}),
            )),
            5 => Some(gone("window closed")),
            6 => Some(gone("no window")),
            7 => Some(with(
                json!({"image": "ok", "fresh": true, "seq": 2, "age_ms": 0}),
            )),
            8 => Some(
                json!({"requests": 6, "frames": 2, "capture_ms": 0.0, "cached": 4, "busy": 0,
                "window_capture_ms": 0.0, "window_failed": 2, "window_frames": 2, "window_gone": 2,
                "window_requests": 6, "window_resolve_ms": 0.0, "max_window_age_ms": 0.0,
                "max_window_gap_ms": 601000.0}),
            ),
            _ => None,
        };
        if rust.is_some() {
            return rust;
        }
    }
    if do_ == "snapshot" {
        let mut v = expected.clone();
        let o = v.as_object_mut()?;
        let before = o.len();
        o.retain(|k, _| !k.ends_with("_ocr_ms"));
        if o.len() != before {
            return Some(v);
        }
    }
    None
}

/// The journal notes, with APP-922's named difference: no preview.failed for the frame Python's sanitizer refused.
fn fixed_notes(scenario: &str, expected: &Value) -> Value {
    if scenario == "failures_journaled" {
        let first = expected[0].clone();
        assert_eq!(expected[1][1]["error"], json!("ValueError"));
        return json!([first]);
    }
    expected.clone()
}

async fn run(sc: &Value) -> (usize, usize, Vec<String>) {
    let clock = Arc::new(Clock {
        t: Mutex::new(500.0),
        expire: watch::channel(0).0,
    });
    let world = Arc::new(World {
        clock: clock.clone(),
        surfaces: sc["surfaces"].as_object().unwrap().clone(),
        used: Mutex::new(HashMap::new()),
        gates: Mutex::new(HashMap::new()),
        entered: Mutex::new(HashMap::new()),
    });
    let locks = ResourceLocks::new();
    let during = sc["during_reads"].as_bool().unwrap();
    let notes: Arc<Mutex<Vec<Value>>> = Arc::new(Mutex::new(vec![]));
    let l = locks.clone();
    let n = notes.clone();
    let allowed = sc["allowed"].as_bool().unwrap();
    let previews = TaskPreviews::new(
        Arc::new(
            move |keys: &[String], holder: &str| match l.try_hold(keys, holder, during) {
                Ok(Some(held)) => Ok(Some(Box::new(held) as Box<dyn Send>)),
                Ok(None) => Ok(None),
                Err(_) => Err("ValueError".to_owned()),
            },
        ),
        Arc::new(move || allowed),
        sc["limit_s"].as_f64().unwrap(),
        Arc::new(move |kind: &str, data: Value| n.lock().unwrap().push(json!([kind, data]))),
        clock.clone(),
    );
    let mut tasks: HashMap<String, tokio::task::JoinHandle<PreviewAnswer>> = HashMap::new();
    let mut holds: HashMap<String, Held> = HashMap::new();
    let results = sc["results"].as_array().unwrap();
    let name = sc["name"].as_str().unwrap();
    let (mut same, mut named, mut diffs) = (0, 0, vec![]);
    for (i, st) in sc["steps"].as_array().unwrap().iter().enumerate() {
        let s = |k: &str| st[k].as_str().unwrap_or("").to_owned();
        let worker = st["worker"].as_str().map(str::to_owned);
        let have = st["have"].as_i64();
        let out: Value = match st["do"].as_str().unwrap() {
            "declare" => {
                let locks: Vec<String> = st["locks"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|v| v.as_str().unwrap().to_owned())
                    .collect();
                previews.declare(
                    &s("root"),
                    &s("worker"),
                    target(&world, &st["target"]),
                    &locks,
                );
                Value::Null
            }
            "frame" => shown(&previews.frame(&s("root"), worker.as_deref(), have).await),
            "frame_bg" => {
                let p = previews.clone();
                let root = s("root");
                tasks.insert(
                    s("id"),
                    tokio::spawn(async move { p.frame(&root, worker.as_deref(), have).await }),
                );
                yields(1).await;
                Value::Null
            }
            "wait_entered" => {
                let mut rx = World::event(&world.entered, &s("gate")).subscribe();
                let _ = rx.wait_for(|v| *v).await;
                Value::Null
            }
            "await" => shown(&tasks.remove(&s("id")).unwrap().await.unwrap()),
            "cancel" => {
                let t = tasks.remove(&s("id")).unwrap();
                t.abort();
                let out = match t.await {
                    Ok(_) => "finished",
                    Err(_) => "cancelled",
                };
                yields(3).await;
                json!(out)
            }
            "release" => {
                let _ = World::event(&world.gates, &s("gate")).send_replace(true);
                yields(3).await;
                Value::Null
            }
            "remove" => {
                let roots: Vec<String> = st["roots"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|v| v.as_str().unwrap().to_owned())
                    .collect();
                previews.remove(&roots);
                yields(3).await;
                Value::Null
            }
            "status" => {
                let table = st["table"].clone();
                previews.set_status(Arc::new(move |_root: &str, worker: &str| {
                    let row = table.get(worker).or_else(|| table.get("*"))?;
                    if row.is_null() {
                        return None;
                    }
                    Some((
                        row[0].as_str().unwrap().to_owned(),
                        row[1].as_str().unwrap().to_owned(),
                    ))
                }));
                Value::Null
            }
            "hold" => {
                let keys: Vec<String> = st["keys"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|v| v.as_str().unwrap().to_owned())
                    .collect();
                let reading = st["reading"].as_bool().unwrap_or(false);
                holds.insert(s("id"), locks.hold(&keys, "worker", reading).await.unwrap());
                Value::Null
            }
            "unhold" => {
                holds.remove(&s("id"));
                Value::Null
            }
            "free" => {
                let keys: Vec<String> = st["keys"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|v| v.as_str().unwrap().to_owned())
                    .collect();
                json!(locks.try_hold(&keys, "probe", false).unwrap().is_some())
            }
            "clock" => {
                world.advance(st["advance_s"].as_f64().unwrap());
                Value::Null
            }
            "snapshot" => Value::Object(previews.snapshot()),
            other => panic!("step {other}"),
        };
        let (expected, is_named) = match fixed(name, i, st["do"].as_str().unwrap(), &results[i]) {
            Some(v) => (v, true),
            None => (results[i].clone(), false),
        };
        if dumps(&out) == dumps(&expected) {
            if is_named {
                named += 1;
            } else {
                same += 1;
            }
        } else {
            diffs.push(format!(
                "{name} step {i} {}:\n  python: {}\n  rust:   {}",
                st["do"],
                dumps(&expected),
                dumps(&out)
            ));
        }
    }
    let got = Value::Array(notes.lock().unwrap().clone());
    let want = fixed_notes(name, &sc["notes"]);
    if dumps(&got) == dumps(&want) {
        if want == sc["notes"] {
            same += 1;
        } else {
            named += 1;
        }
    } else {
        diffs.push(format!(
            "{name} notes:\n  python: {}\n  rust:   {}",
            dumps(&want),
            dumps(&got)
        ));
    }
    (same, named, diffs)
}

// decision: APP-403, APP-404, APP-405, APP-922, DMN-403, DMN-404, DMN-406, DMN-407, DMN-408, DMN-584
#[tokio::test]
async fn every_scenario_matches_python() {
    let f = fixture();
    let scenarios = f["scenarios"].as_array().unwrap();
    let (mut same, mut named, mut diffs) = (0, 0, vec![]);
    for sc in scenarios {
        let (s, n, d) = run(sc).await;
        same += s;
        named += n;
        diffs.extend(d);
    }
    assert!(
        diffs.is_empty(),
        "{} difference(s):\n{}",
        diffs.len(),
        diffs.join("\n")
    );
    assert_eq!(named, 20, "the named differences were compared");
    println!(
        "{} scenarios: {same} results equal, {named} differ by the named differences (frozen frame keeps its seq; \
         APP-922: no OCR phase, the refused frame shown), 0 other differences",
        scenarios.len()
    );
}
