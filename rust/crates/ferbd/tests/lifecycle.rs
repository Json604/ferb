//! The real program's start and stop (main.py `run`, `start`, `shutdown`): SIGTERM ends it cleanly with exit 0; a
//! second daemon on the same home refuses; a restart serves again; the stop marker outlives a restart; what the
//! ledger had in flight is unknown after one (`mark_unknown_after_restart`); `ping` is answered while starting.

mod support;

use serde_json::{json, Value};
use support::*;

fn call(run: &Run, method: &str, params: Value) -> Value {
    let mut c = Raw::connect(&run.socket()).unwrap();
    c.hello("cli").unwrap();
    c.send(&json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}).to_string());
    serde_json::from_str(&c.recv().expect("an answer")).unwrap()
}

#[test]
fn start_stop_restart_and_one_daemon_per_home() {
    let tmp = root();
    let mut run = Run::start(tmp.path(), "01", json!({}));
    let ping = call(&run, "ping", json!({}));
    assert_eq!(ping["result"]["role"], "cli");
    assert_eq!(ping["result"]["stopped"], false);

    // a second daemon on the same home refuses, and changes nothing
    let second = std::process::Command::new(BIN)
        .arg("daemon")
        .env_clear()
        .env("FERB_HOME", &run.home)
        .env("HOME", &run.user)
        .env("FERBD_STAND_INS", &run.stand)
        .output()
        .unwrap();
    assert_eq!(second.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&second.stderr).contains("another daemon is answering"),
        "{}",
        String::from_utf8_lossy(&second.stderr)
    );
    assert_eq!(call(&run, "ping", json!({}))["result"]["role"], "cli");

    // SIGTERM: a clean exit (0), the socket gone, `daemon.stopped` journaled
    let (status, err) = run.stop();
    assert_eq!(status.code(), Some(0), "{err}");
    assert!(!run.socket().exists());
    let db = rusqlite::Connection::open(run.home.join("ferb.db")).unwrap();
    let kinds: Vec<String> = db
        .prepare("SELECT kind FROM journal ORDER BY seq")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert!(kinds.contains(&"daemon.started".to_string()), "{kinds:?}");
    assert_eq!(kinds.last().map(String::as_str), Some("daemon.stopped"));

    // restart: it serves again on the same home
    run.spawn();
    assert_eq!(call(&run, "ping", json!({}))["result"]["role"], "cli");
    let (status, _) = run.stop();
    assert_eq!(status.code(), Some(0));
}

#[test]
fn the_stop_and_the_ledger_survive_a_restart() {
    let tmp = root();
    let mut run = Run::start(tmp.path(), "02", json!({}));
    let engaged = call(&run, "stop.engage", json!({}));
    assert_eq!(engaged["result"]["tokens_revoked"], 0, "{engaged}");
    assert!(run.home.join("STOPPED").exists());
    // an action in flight when the daemon goes (the orchestrator's ledger)
    run.control(json!({"do": "sql", "statements": [[
        "INSERT INTO action_ledger (id, task_id, capability, target_json, payload_hash, state, created_at, updated_at) \
         VALUES ('intent-1', 'task-1', 'fs.write', '{}', 'h', 'executing', '2026-01-01T09:00:00+00:00', \
         '2026-01-01T09:00:00+00:00')", []]]}));
    let (status, _) = run.stop();
    assert_eq!(status.code(), Some(0));

    run.spawn();
    assert_eq!(call(&run, "ping", json!({}))["result"]["stopped"], true);
    let rows = run.table("action_ledger");
    let row = rows
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r.as_array().unwrap().iter().any(|c| c == "intent-1"))
        .unwrap();
    assert!(
        row.as_array().unwrap().iter().any(|c| c == "unknown"),
        "{row}"
    );
    let journal = run.table("journal");
    let kinds: Vec<&str> = journal
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r[3].as_str().unwrap_or(""))
        .collect();
    assert!(kinds.contains(&"ledger.unknown_after_restart"), "{kinds:?}");
    assert!(kinds.contains(&"stop.still_engaged"), "{kinds:?}");

    // lifted: the marker goes, and a restart starts unstopped
    assert_eq!(
        call(&run, "stop.lift", json!({}))["result"]["stopped"],
        false
    );
    assert!(!run.home.join("STOPPED").exists());
    let _ = run.stop();
    run.spawn();
    assert_eq!(call(&run, "ping", json!({}))["result"]["stopped"], false);
}

#[test]
fn the_supervisors_methods_refuse_plainly_in_this_build() {
    let tmp = root();
    let run = Run::start(tmp.path(), "03", json!({}));
    let r = call(&run, "chat.status", json!({}));
    assert_eq!(r["error"]["code"], -32004, "{r}");
    assert!(r["error"]["message"]
        .as_str()
        .unwrap()
        .starts_with("not available in this build"));
}
