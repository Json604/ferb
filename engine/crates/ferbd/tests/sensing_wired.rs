//! Sensing's wiring in the daemon (areas/sensing.py `_build_sense`): perception built always on the trail's thread
//! (its status in `sense.status`, its batcher's loop as the `jev_batches` watcher, typed text expired at the start),
//! the learners' spans and usage, and the Automation answer the browsers' scripted tabs read.

mod support;

use std::rc::Rc;
use std::sync::mpsc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use support::*;

fn rpc(c: &mut Raw, id: i64, method: &str, params: Value) -> Value {
    c.send(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}).to_string());
    let line = c.recv().expect("an answer");
    let v: Value = serde_json::from_str(&line).unwrap();
    assert!(v.get("error").is_none(), "{method}: {line}");
    v["result"].clone()
}

/// sensing.py:151-159: `sense.status` carries perception's status (its batches, unsent rows, batch age, triage
/// measurements) and the batcher's watcher; nothing runs for Jev (SENSE-2, SENSE-3).
// decision: SENSE-3
#[test]
fn sense_status_carries_perceptions_status() {
    let tmp = root();
    let run = Run::start(tmp.path(), "70", json!({"group_a": true}));
    let mut c = Raw::connect(&run.socket()).unwrap();
    c.hello("cli").unwrap();
    let until = Instant::now() + Duration::from_secs(10);
    let status = loop {
        let s = rpc(&mut c, 1, "sense.status", json!({}));
        if s["watchers"]
            .as_array()
            .is_some_and(|w| w.contains(&json!("jev_batches")))
            || Instant::now() >= until
        {
            break s;
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    assert_eq!(status["watchers"], json!(["jev_batches"]), "{status}");
    assert_eq!(status["owner_input"], json!(false));
    assert_eq!(status["folder_problems"], json!([]));
    let p = &status["perception"];
    assert_eq!(p["batches"], json!({}), "{status}");
    assert_eq!(p["unsent_events"], json!(0));
    assert_eq!(p["batch_age_s"], json!(60.0));
    assert_eq!(p["notification_triage"]["mode"], json!("shadow"));
    assert_eq!(p["now_playing"]["status"], json!("pending_owner_probe"));
}

/// perception.py:131 (D14/P8), run at sensing's build (sensing.py:68): typed text older than 7 days keeps only its
/// source ids; a newer row is left as it is.
// decision: SENSE-401
#[test]
fn typed_text_past_seven_days_expires_at_the_start() {
    let tmp = root();
    let mut run = Run::start(tmp.path(), "71", json!({"group_a": true}));
    run.stop();
    {
        let db = rusqlite::Connection::open(run.home.join("ferb.db")).unwrap();
        for (id, at) in [
            ("old-1", "2025-12-20T09:00:00+00:00"),
            ("new-1", "2025-12-31T09:00:00+00:00"),
        ] {
            db.execute(
                "INSERT INTO trail (id, at, day, device_id, actor, task_id, source, data_json) \
                 VALUES (?, ?, ?, 'dev', 'owner', NULL, 'typed_text', ?)",
                rusqlite::params![
                    id,
                    at,
                    &at[..10],
                    json!({"bundle_id": "com.apple.TextEdit", "app": "TextEdit", "text": "hello there",
                           "field_id": "f1"})
                    .to_string()
                ],
            )
            .unwrap();
        }
    }
    run.spawn();
    let until = Instant::now() + Duration::from_secs(10);
    loop {
        let rows = run.table("trail");
        let data = |id: &str| -> Value {
            let row = rows
                .as_array()
                .unwrap()
                .iter()
                .find(|r| r.as_array().unwrap().iter().any(|v| v == id))
                .cloned()
                .unwrap();
            let text = row
                .as_array()
                .unwrap()
                .iter()
                .filter_map(Value::as_str)
                .find(|s| s.starts_with('{'))
                .unwrap()
                .to_string();
            serde_json::from_str(&text).unwrap()
        };
        let old = data("old-1");
        if old.get("content_expired") == Some(&json!(true)) {
            assert_eq!(
                old,
                json!({"bundle_id": "com.apple.TextEdit", "app": "TextEdit", "field_id": "f1",
                       "content_expired": true})
            );
            assert_eq!(data("new-1")["text"], json!("hello there"));
            break;
        }
        assert!(Instant::now() < until, "the old row didn't expire: {old}");
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// memory.py:763-769 `_record_learning_usage`: a learner's usage is a `model.usage` span naming its role, with the
/// event's cost; a call's span (`model.learning`) is finished with its output.
#[test]
fn a_learners_usage_is_a_model_usage_span_with_its_role() {
    use ferb_learning::calls::Spans;
    let tmp = root();
    let db = ferb_store::Db::open(tmp.path().join("ferb.db")).unwrap();
    db.migrate().unwrap();
    let db = Rc::new(db);
    let spans = ferb_daemon::areas_a::memory::LearningSpans::over(
        db.clone(),
        Arc::new(ferb_clock::clock::FakeClock::new(None)),
        Arc::new(ferb_core::ids::SequentialIds::new("s")),
        Arc::new(ferb_sanitize::redact::Redactor::new(&[], &[])),
    );
    spans.usage(
        "distill",
        "codex",
        &json!({"cost_usd": 0.25, "input_tokens": 10}),
    );
    let id = spans.start(
        "model.learning",
        None,
        json!({"role": "distill", "call": "c1"}),
        None,
    );
    assert!(!id.is_empty());
    spans.finish(&id, Some(json!({"outcome": "answered"})), None);
    let rows: Vec<(String, String, Option<String>, Option<f64>)> = {
        let mut st = db
            .conn()
            .prepare(
                "SELECT kind, input_json, output_json, cost_usd FROM spans ORDER BY started_at, id",
            )
            .unwrap();
        st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap()
    };
    assert_eq!(rows.len(), 2, "{rows:?}");
    let usage = rows.iter().find(|r| r.0 == "model.usage").unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&usage.1).unwrap(),
        json!({"runtime": "codex", "role": "distill"})
    );
    assert_eq!(usage.3, Some(0.25));
    assert_eq!(
        serde_json::from_str::<Value>(usage.2.as_deref().unwrap()).unwrap()["input_tokens"],
        json!(10)
    );
    let call = rows.iter().find(|r| r.0 == "model.learning").unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(call.2.as_deref().unwrap()).unwrap(),
        json!({"outcome": "answered"})
    );
}

/// desktop.py:362 `_automation_granted`: while the check runs the answer is no (the watcher skips that sample);
/// once it answered, that answer is kept and no new check starts.
#[test]
fn automation_is_no_while_its_check_runs_then_kept() {
    let a = ferbd::compose::loops::AutomationGranted::default();
    let (release, wait) = mpsc::channel::<()>();
    let wait = std::sync::Mutex::new(wait);
    assert!(!a.granted("com.apple.Safari", move || {
        let _ = wait.lock().unwrap().recv();
        true
    }));
    // still running: no, and no second check
    assert!(!a.granted("com.apple.Safari", || panic!("a second check")));
    release.send(()).unwrap();
    let until = Instant::now() + Duration::from_secs(5);
    while !a.granted("com.apple.Safari", || panic!("a second check")) {
        assert!(Instant::now() < until, "the check's answer never came");
        std::thread::sleep(Duration::from_millis(10));
    }
    // kept: answered without a check
    assert!(a.granted("com.apple.Safari", || panic!("a new check")));
}

/// The banner handle the notifier keeps between posting and a failure reads back to the same banner.
#[test]
fn a_banner_handle_reads_back() {
    let b = ferb_sense::perception::Banner {
        title: "FERB".into(),
        body: "Done".into(),
        poster: "com.apple.ScriptEditor2".into(),
        at: "2026-01-01T09:00:00+00:00".into(),
    };
    let v = ferbd::compose::loops::banner_json(&b);
    assert_eq!(ferbd::compose::loops::banner_of(&v), Some(b));
    assert_eq!(
        ferbd::compose::loops::banner_of(&json!({"title": "x"})),
        None
    );
}
