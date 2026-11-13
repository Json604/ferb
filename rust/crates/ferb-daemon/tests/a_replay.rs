//! The judge: rust/fixtures/daemon/rpc_transcripts.json, recorded from the real Python daemon. Every recorded request
//! of a group A method is sent to the Rust handlers on a real `ferb_rpc::Server` over temp stores set up as the
//! scenario's earlier steps left them, and the response line is compared byte for byte. The end tables of group A's
//! areas are compared too.
//!
//! Named differences (ENG-6), each the bug rule for these areas: a missing or malformed parameter that Python answered
//! with an internal error (-32603 and the exception's class) is a refusal (-32004) naming the parameter. The internal
//! error's `daemon.internal_error` journal row (the server's `on_internal`, which took an id) is not written; the test
//! skips that id so the ids after it line up with the recording.

mod a_support;

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use a_support::{fixture, fx, Daemon, Raw, Seq};
use ferb_clock::clock::FakeClock;
use ferb_daemon::areas_a::{self, memory, schedules};
use serde_json::{json, Value};

/// (scenario, request id) → the refusal Rust answers where Python failed inside.
const FIXED: &[(&str, i64, &str)] = &[
    (
        "memory",
        4,
        "memory.add needs content: the words to remember",
    ),
    ("memory", 8, "memory.show needs a subject"),
    (
        "memory",
        11,
        "memory.edit needs the memory's id and its new content",
    ),
    ("memory", 13, "memory.retract needs the memory's id"),
    ("memory", 18, "memory.forget needs the memory's id"),
    (
        "schedules_reminders_timers",
        8,
        "schedules.change needs a schedule's id and an action: cancel, pause or resume",
    ),
    (
        "schedules_reminders_timers",
        15,
        "reminders.cancel needs a reminder's id",
    ),
    (
        "settings_setup_about",
        76,
        "since must be a date and time, like 2026-01-01T09:00:00+00:00",
    ),
];

fn method_of(line: &str) -> (String, Option<i64>) {
    let v: Value = serde_json::from_str(line).unwrap_or(Value::Null);
    (
        v["method"].as_str().unwrap_or("").to_owned(),
        v["id"].as_i64(),
    )
}

fn ours(method: &str) -> bool {
    method == "auth.hello"
        || method == "settings.get"
        || method == "settings.set"
        || areas_a::METHODS.contains(&method)
}

struct Settings;

impl areas_a::settings::SettingsHost for Settings {
    fn apply_privacy(&self) {}
    fn journal(&self, _kind: &str, _data: Value, _origin: &str) {}
}

/// Walk a scenario's steps for the connections `only` names (all when empty), sending group A's requests.
async fn replay(name: &str, d: &Daemon, only: &[&str]) -> (usize, usize) {
    let scenario = fixture("daemon/rpc_transcripts.json")["scenarios"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["name"] == name)
        .cloned()
        .unwrap();
    let fixed: BTreeMap<i64, &str> = FIXED
        .iter()
        .filter(|(s, _, _)| *s == name)
        .map(|(_, id, m)| (*id, *m))
        .collect();
    let mut conns: BTreeMap<String, Raw> = BTreeMap::new();
    let (mut same, mut named) = (0, 0);
    for step in scenario["steps"].as_array().unwrap() {
        let conn = step["conn"].as_str().unwrap_or("").to_owned();
        if !only.is_empty() && !conn.is_empty() && !only.contains(&conn.as_str()) {
            continue;
        }
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
                assert_eq!(json!(fired), step["fired"], "{name}: timers fired");
            }
            "call" => {
                let lines = step["lines"].as_array().unwrap();
                let sent = lines[0]["send"].as_str().unwrap();
                let (method, id) = method_of(sent);
                if !ours(&method) {
                    continue;
                }
                let c = conns.get_mut(&conn).unwrap();
                c.send(sent).await;
                for want in lines[1..].iter().filter_map(|l| l["recv"].as_str()) {
                    let got = fx(&c.recv().await, &d.root);
                    match id.and_then(|i| fixed.get(&i)) {
                        Some(message) => {
                            let python: Value = serde_json::from_str(want).unwrap();
                            assert_eq!(
                                python["error"]["code"], -32603,
                                "{name} {id:?}: Python's answer"
                            );
                            let rust = ferb_rpc::wire::error_line(
                                &ferb_rpc::Id::Int(serde_json::Number::from(id.unwrap())),
                                -32004,
                                message,
                            );
                            assert_eq!(got.as_bytes(), &rust[..rust.len() - 1], "{name} {id:?}");
                            d.ids.bump(); // the id Python's internal-error row took
                            named += 1;
                        }
                        None => {
                            assert_eq!(got, want, "{name}: {method} (id {id:?})");
                            same += 1;
                        }
                    }
                }
            }
            _ => {}
        }
    }
    (same, named)
}

/// Compare the end tables group A writes with the recording's (`tables`), skipping the journal rows of other parts.
async fn tables(name: &str, d: &Daemon, names: &[&str], journal_from: &str) {
    let scenario = fixture("daemon/rpc_transcripts.json")["scenarios"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["name"] == name)
        .cloned()
        .unwrap();
    for t in names {
        let want = scenario["tables"][*t]["rows"].clone();
        let got = d.table(t).await;
        let got = if *t == "journal" {
            // (id, at, kind, data_json, origin) of rows from `journal_from` on, not the server's internal errors
            let pick = |rows: Vec<Vec<Value>>| -> Vec<Value> {
                rows.into_iter()
                    .filter(|r| {
                        r[1].as_str().unwrap_or("") >= journal_from
                            && r[3] != "daemon.internal_error"
                    })
                    .map(|r| json!(r[1..]))
                    .collect()
            };
            let w = pick(serde_json::from_value(want).unwrap());
            assert_eq!(json!(pick(got)), json!(w), "{name}: journal");
            continue;
        } else {
            json!(got)
        };
        assert_eq!(
            got,
            if want.is_null() { json!([]) } else { want },
            "{name}: table {t}"
        );
    }
}

fn fake_clock() -> FakeClock {
    FakeClock::new(None)
}

#[tokio::test(flavor = "multi_thread")]
async fn memory_scenario() {
    let tmp = tempfile::tempdir().unwrap();
    let ids = Seq::new("id");
    let d = Daemon::start(tmp.path(), "05", "id", fake_clock(), ids.clone()).await;
    d.on(|st| Box::pin(async move { memory::start(&st).unwrap() }))
        .await;
    ids.set_next(15);
    let (same, named) = replay("memory", &d, &[]).await;
    assert_eq!((same, named), (20, 5));
    tables(
        "memory",
        &d,
        &[
            "memory_items",
            "memory_sources",
            "memory_forgotten",
            "journal",
        ],
        "id-000015",
    )
    .await;
    d.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn chat_history_scenario() {
    let tmp = tempfile::tempdir().unwrap();
    let d = Daemon::start(tmp.path(), "06", "id", fake_clock(), Seq::new("id")).await;
    let (same, named) = replay("chat_history", &d, &[]).await;
    assert_eq!((same, named), (12, 0));
    tables("chat_history", &d, &["chat_history"], "").await;
    d.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn protocol_app_only_refusal() {
    let tmp = tempfile::tempdir().unwrap();
    let d = Daemon::start(tmp.path(), "01", "id", fake_clock(), Seq::new("id")).await;
    d.server.set_is_app(|_| false);
    let (same, named) = replay("protocol", &d, &["not-the-app"]).await;
    assert_eq!((same, named), (2, 0));
    d.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn schedules_scenario() {
    let tmp = tempfile::tempdir().unwrap();
    let ids = Seq::new("id");
    let d = Daemon::start(tmp.path(), "09", "id", fake_clock(), ids.clone()).await;
    // what the scenario's chat turn left: a reminder, a timer and a daily schedule with its first run booked
    let at = "2026-01-01T09:00:00+00:00";
    let timer = |id: &str, kind: &str, fire: &str, msg: &str, req: Value| {
        (
            "INSERT INTO timers (id, kind, fire_at, message, task_id, request_id, state, created_at) VALUES \
             (?, ?, ?, ?, NULL, ?, 'pending', ?)"
                .to_owned(),
            vec![json!(id), json!(kind), json!(fire), json!(msg), req, json!(at)],
        )
    };
    d.sql(vec![
        timer("tm-id-000022", "reminder", "2026-01-01T09:30:00+00:00", "Call the invented plumber", json!("id-000015")),
        ("INSERT INTO timer_names VALUES ('tm-id-000022', 'Plumber')".into(), vec![]),
        timer("tm-id-000024", "timer", "2026-01-01T09:10:00+00:00", "Tea is ready", json!("id-000015")),
        ("INSERT INTO timer_names VALUES ('tm-id-000024', 'Tea')".into(), vec![]),
        timer("tm-id-000028", "schedule", "2026-01-02T08:00:00+00:00", "sc-id-000027", Value::Null),
        (
            "INSERT INTO schedules (id, request, own_words, intent_json, rule_json, next_run, timer_id, state, \
             created_at, watch) VALUES (?, ?, ?, ?, ?, ?, ?, 'active', ?, 'none')"
                .into(),
            vec![
                json!("sc-id-000027"),
                json!("Water the invented plants"),
                json!("water the plants every morning at 8, remind me in 30 minutes to call the plumber, and set a 10 minute timer for tea"),
                json!("{\"apps\": [], \"sites\": [], \"paths\": [], \"recipients\": [], \"run_scope\": {\"folders\": [], \"identity\": \"owner\", \"worker_type\": \"default\", \"profile\": \"\", \"account\": \"\"}}"),
                json!("{\"repeat\": \"daily\", \"at\": \"08:00\", \"weekday\": null, \"every_minutes\": null, \"tz\": \"UTC\", \"when_active\": false, \"first\": \"2026-01-02T08:00:00+00:00\"}"),
                json!("2026-01-02T08:00:00+00:00"),
                json!("tm-id-000028"),
                json!(at),
            ],
        ),
        ("INSERT INTO schedule_names VALUES ('sc-id-000027', 'Plant watering')".into(), vec![]),
    ])
    .await;
    ids.set_next(32);
    d.token.store(3, std::sync::atomic::Ordering::SeqCst);
    let (same, named) = replay("schedules_reminders_timers", &d, &["app#2"]).await;
    assert_eq!((same, named), (20, 2));
    tables(
        "schedules_reminders_timers",
        &d,
        &[
            "timers",
            "timer_names",
            "timer_repeats",
            "timer_deliveries",
            "schedule_names",
            "journal",
        ],
        "id-000032",
    )
    .await;
    // schedules: every column but `seq` (the recording's rows were written by the chat turn)
    let want = fixture("daemon/rpc_transcripts.json")["scenarios"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["name"] == "schedules_reminders_timers")
        .unwrap()["tables"]["schedules"]["rows"]
        .clone();
    assert_eq!(json!(d.table("schedules").await), want);
    // the seams: the timer's banner, its timer.changed events (DMN-548)
    let calls = d.stand.calls.take();
    assert_eq!(calls.len(), 1, "{calls:?}");
    assert_eq!(calls[0]["notify"]["title"], "Tea");
    let topics: Vec<Value> = d
        .published
        .lock()
        .unwrap()
        .iter()
        .map(|p| p["topic"].clone())
        .collect();
    assert!(topics.contains(&json!("timer.fired")) && topics.contains(&json!("timer.changed")));
    d.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn settings_import_timeline_persona() {
    let tmp = tempfile::tempdir().unwrap();
    let ids = Seq::new("id");
    let mut d = Daemon::start(tmp.path(), "30", "id", fake_clock(), ids.clone()).await;
    let config = Arc::new(ferb_core::config::Config::new(&d.home));
    areas_a::register_settings(&mut d.server, config, Arc::new(Settings));
    let project = d.user.join(".claude/projects/invented-project");
    std::fs::create_dir_all(&project).unwrap();
    let rows = [
        json!({"type": "user", "timestamp": "2025-12-30T10:00:00Z", "message": {"role": "user", "content": "I keep my invented notes in ~/notes"}}),
        json!({"type": "assistant", "timestamp": "2025-12-30T10:00:05Z", "message": {"role": "assistant", "content": [{"type": "text", "text": "Noted."}]}}),
    ];
    let text: String = rows
        .iter()
        .map(|r| ferb_py::json::dumps(r) + "\n")
        .collect();
    std::fs::write(project.join("session-1.jsonl"), text).unwrap();
    // the journal's newest rows as the scenario left them before `timeline.get` (the response shows them)
    let recorded = fixture("daemon/rpc_transcripts.json")["scenarios"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["name"] == "settings_setup_about")
        .unwrap()["steps"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|s| s["lines"].as_array())
        .flatten()
        .filter_map(|l| l["recv"].as_str())
        .find(|l| l.contains("\"id\": 75,"))
        .map(|l| serde_json::from_str::<Value>(l).unwrap())
        .unwrap();
    let mut seed = vec![(
        "INSERT INTO journal (id, at, kind, data_json, origin) VALUES ('seed-0', ?, 'daemon.started', '{}', 'ferb')"
            .to_owned(),
        vec![json!("2026-01-01T09:00:00+00:00")],
    )];
    for (n, e) in recorded["result"]["entries"]
        .as_array()
        .unwrap()
        .iter()
        .enumerate()
    {
        seed.push((
            "INSERT INTO journal (id, at, kind, data_json, origin) VALUES (?, ?, ?, ?, 'ferb')"
                .into(),
            vec![
                json!(format!("seed-{}", n + 1)),
                e["at"].clone(),
                e["kind"].clone(),
                json!(ferb_py::json::dumps(&e["data"])),
            ],
        ));
    }
    d.sql(seed).await;
    let (same, named) = replay("settings_setup_about", &d, &[]).await;
    assert_eq!((same, named), (15, 1));
    d.close().await;
}

#[test]
fn every_method_of_group_a_is_registered_once() {
    let mut seen = std::collections::BTreeSet::new();
    for m in areas_a::METHODS {
        assert!(seen.insert(m), "{m} twice");
        assert!(ferb_rpc::auth::method(m).is_some(), "{m} is a cli method");
    }
    assert_eq!(seen.len(), 26);
    let _ = Path::new("");
}
