//! rust/fixtures/daemon-a/transcripts.json (made by its make.py on the real Python daemon): group A's branches the
//! daemon recordings don't reach. Requests are replayed on the Rust handlers and compared byte for byte; schedules are
//! written as the recording wrote them, the clock moves and the timer loop ticks as it did; the seams' calls (the
//! scheduled run, the result told) and the end tables are compared.
//!
//! Named differences (ENG-6): `timeline.get {"limit": "x"}` and `schedules.change {"action": "stop"}` were internal
//! errors in Python (ValueError; KeyError after journaling a `schedule.stop` row for an action that doesn't exist);
//! here they are refusals and nothing is written. Their internal-error rows' ids (and the bogus row's) are skipped.

mod a_support;

use std::collections::BTreeMap;

use a_support::{fixture, fx, Daemon, Raw, Seq};
use ferb_clock::clock::FakeClock;
use ferb_daemon::areas_a::{memory, schedules};
use serde_json::{json, Map, Value};

/// (scenario, request id) → (the refusal, ids Python's failure took).
const FIXED: &[(&str, i64, &str, u64)] = &[
    ("memory_more", 38, "limit must be a whole number", 1),
    (
        "schedules_more",
        13,
        "action must be cancel, pause or resume",
        2,
    ),
];

fn scenario(name: &str) -> Value {
    fixture("daemon-a/transcripts.json")["scenarios"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["name"] == name)
        .cloned()
        .unwrap()
}

/// A seam call as the recording writes it (numbers compared as numbers).
fn norm(v: &Value) -> Value {
    match v {
        Value::Number(n) => json!(n.as_f64()),
        Value::Array(a) => Value::Array(a.iter().map(norm).collect()),
        Value::Object(m) => Value::Object(m.iter().map(|(k, v)| (k.clone(), norm(v))).collect()),
        other => other.clone(),
    }
}

async fn run(name: &str, nn: &str) -> (usize, usize) {
    let tmp = tempfile::tempdir().unwrap();
    let ids = Seq::new("id");
    let mut d = Daemon::start(tmp.path(), nn, "id", FakeClock::new(None), ids.clone()).await;
    d.on(|st| Box::pin(async move { memory::start(&st).unwrap() }))
        .await;
    ids.set_next(15);
    let sc = scenario(name);
    let fixed: BTreeMap<i64, (&str, u64)> = FIXED
        .iter()
        .filter(|f| f.0 == name)
        .map(|f| (f.1, (f.2, f.3)))
        .collect();
    let mut conns: BTreeMap<String, Raw> = BTreeMap::new();
    let (mut same, mut named) = (0, 0);
    for step in sc["steps"].as_array().unwrap() {
        let conn = step["conn"].as_str().unwrap_or("").to_owned();
        match step["do"].as_str().unwrap() {
            "connect" => {
                conns.insert(conn, Raw::connect(d.server.path()).await);
            }
            "close" => {
                conns.remove(&conn);
            }
            "clock" => d.clock.advance(step["advance_s"].as_f64().unwrap()),
            "timers_tick" => {
                let fired = d
                    .on(|st| Box::pin(async move { schedules::tick(&st).await.unwrap() }))
                    .await;
                d.on(|st| Box::pin(async move { schedules::settle_runs(&st).await }))
                    .await;
                assert_eq!(json!(fired), step["fired"], "{name}: fired");
            }
            "add_schedule" => {
                let spec = step["spec"].clone();
                let id = d
                    .on(move |st| {
                        Box::pin(async move {
                            let rule = ferb_clock::schedules::Rule::from_json(
                                spec["rule"].as_object().unwrap(),
                            );
                            let first =
                                ferb_clock::py::parse(spec["first"].as_str().unwrap()).unwrap();
                            let s = st
                                .schedules()
                                .add(
                                    spec["request"].as_str().unwrap(),
                                    spec["own_words"].as_str().unwrap(),
                                    spec["targets"].as_object().unwrap(),
                                    &rule,
                                    &first,
                                    spec["watch"].as_str().unwrap(),
                                    None,
                                    spec["name"].as_str().unwrap(),
                                )
                                .unwrap();
                            let due = s.next_run.unwrap();
                            let t = schedules::book(&st, &s, &due, "", None).unwrap();
                            st.schedules()
                                .set_timer(&s.id, Some(&t.id), Some(&due))
                                .unwrap();
                            s.id
                        })
                    })
                    .await;
                assert_eq!(json!(id), step["id"]);
            }
            "seams" => {
                let mut got: Vec<Value> = d
                    .stand
                    .calls
                    .take()
                    .into_iter()
                    .filter(|c| !c.as_object().unwrap().contains_key("create_scheduled"))
                    .collect();
                for c in &mut got {
                    if let Some(m) = c.as_object_mut() {
                        m.remove("created");
                    }
                }
                assert_eq!(norm(&json!(got)), norm(&step["calls"]), "{name}: seams");
            }
            "note" => {
                // the daily schedule's booked timer was set cancelled (a crash between a run and the next booking)
                d.sql(vec![(
                    "UPDATE timers SET state = 'cancelled' WHERE id = (SELECT timer_id FROM schedules WHERE id = \
                     'sc-id-000017')"
                        .into(),
                    vec![],
                )])
                .await;
            }
            "restart" => {
                conns.clear();
                let root = d.root.clone();
                // the recording's restart builds a new FakeClock (2026-01-01T09:00Z), as make.py's new_host does
                let clock = FakeClock::new(None);
                d.close().await;
                let ids = Seq::new(step["ids_prefix"].as_str().unwrap());
                d = Daemon::start(&root, nn, "r1", clock, ids).await;
                d.on(|st| Box::pin(async move { schedules::reconcile_schedules(&st).unwrap() }))
                    .await;
            }
            "call" => {
                let lines = step["lines"].as_array().unwrap();
                let sent = lines[0]["send"].as_str().unwrap();
                let v: Value = serde_json::from_str(sent).unwrap();
                let id = v["id"].as_i64();
                let c = conns.get_mut(&conn).unwrap();
                c.send(sent).await;
                for want in lines[1..].iter().filter_map(|l| l["recv"].as_str()) {
                    let got = fx(&c.recv().await, &d.root);
                    match id.and_then(|i| fixed.get(&i)) {
                        Some((message, took)) => {
                            let rust = ferb_rpc::wire::error_line(
                                &ferb_rpc::Id::Int(serde_json::Number::from(id.unwrap())),
                                -32004,
                                message,
                            );
                            assert!(want.contains("-32603"), "{name} {id:?}: Python's answer");
                            assert_eq!(got.as_bytes(), &rust[..rust.len() - 1]);
                            for _ in 0..*took {
                                d.ids.bump();
                            }
                            named += 1;
                        }
                        None => {
                            assert_eq!(got, want, "{name}: {sent}");
                            same += 1;
                        }
                    }
                }
            }
            _ => {}
        }
        if step["do"] == "add_schedule" && name == "schedules_more" {
            let mut runs = d.stand.runs.lock().unwrap();
            if runs.is_empty() {
                for (task, status, key, text) in [
                    ("invented-run-1", "done", "answer", "No open slots yet."),
                    (
                        "invented-run-2",
                        "failed",
                        "answer",
                        "The invented garden was locked.",
                    ),
                    (
                        "invented-run-3",
                        "done_unverified",
                        "summary",
                        "Watered, unconfirmed.",
                    ),
                ] {
                    let mut m = Map::new();
                    m.insert("task_id".into(), json!(task));
                    m.insert("status".into(), json!(status));
                    m.insert(key.into(), json!(text));
                    if task == "invented-run-2" {
                        m.insert(
                            "cards".into(),
                            json!([{"type": "note", "text": "kept"}, "not a card"]),
                        );
                    }
                    runs.push(m);
                }
            }
        }
    }
    // the end tables group A writes
    let tables = &sc["tables"];
    let kinds = |k: &str| {
        ["memory.", "schedule.", "timer.", "downloads."]
            .iter()
            .any(|p| k.starts_with(p))
    };
    let pick = |rows: Vec<Vec<Value>>| -> Vec<Value> {
        rows.into_iter()
            .filter(|r| {
                let id = r[1].as_str().unwrap_or("");
                // the start's own rows (Python's start took ids 1-14 for other parts' rows too)
                let started = id.starts_with("id-") && id < "id-000015";
                kinds(r[3].as_str().unwrap_or(""))
                    && !started
                    && !(name == "schedules_more" && r[3] == "schedule.stop")
            })
            .map(|r| json!(r[1..]))
            .collect()
    };
    let want_journal = pick(serde_json::from_value(tables["journal"]["rows"].clone()).unwrap());
    assert_eq!(
        json!(pick(d.table("journal").await)),
        json!(want_journal),
        "{name}: journal"
    );
    let names: &[&str] = if name == "schedules_more" {
        &["schedules", "schedule_names", "timer_deliveries", "timers"]
    } else {
        &[
            "memory_items",
            "memory_sources",
            "memory_forgotten",
            "memory_profile",
            "chat_history",
        ]
    };
    for t in names {
        let mut got = json!(d.table(t).await);
        let mut want = tables[*t]["rows"].clone();
        if *t == "timers" {
            // claim tokens: the Python daemon's own timer loop also ticked in real time
            for rows in [&mut got, &mut want] {
                for r in rows.as_array_mut().unwrap() {
                    r[8] = Value::Null;
                }
            }
        }
        assert_eq!(got, want, "{name}: {t}");
    }
    d.close().await;
    (same, named)
}

#[tokio::test(flavor = "multi_thread")]
async fn memory_more() {
    assert_eq!(run("memory_more", "01").await, (46, 1));
}

#[tokio::test(flavor = "multi_thread")]
async fn schedules_more() {
    assert_eq!(run("schedules_more", "02").await, (14, 1));
}
