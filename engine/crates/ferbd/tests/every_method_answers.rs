//! Every RPC method of the method table through the real program (audit of 2026-11-16): the table itself is Python's
//! plus the named additions (ferb-rpc tests/golden.rs against engine/fixtures/rpc/table.json); here each method of it
//! has a handler in the composed `ferbd daemon`, and the ones that only read answer a well-formed call with a result
//! over the socket, as FERB.app's connection (role `cli`, the stand-ins' FERB.app check passing).
//!
//! Methods that act (send, pair, open a browser or System Settings, sign in, stop, send a report) are only checked to
//! be registered here; the chat, setup and records tests drive them with their own stand-ins.

mod shim;
mod support;

use std::collections::BTreeSet;

use serde_json::{json, Value};
use support::*;

/// Read-only methods and a well-formed call of each.
fn reads() -> Vec<(&'static str, Value)> {
    vec![
        ("ping", json!({})),
        ("about.get", json!({})),
        ("chat.status", json!({})),
        ("chat.attach", json!({})),
        ("chat.entries.days", json!({})),
        ("chat.entries.page", json!({})),
        ("chat.history.days", json!({})),
        ("chat.history.page", json!({"day": "2026-01-01"})),
        ("conversations.list", json!({})),
        ("data.tables", json!({})),
        ("identity.check", json!({})),
        ("jev.stats", json!({})),
        ("memory.list", json!({})),
        ("memory.profile", json!({})),
        ("memory.repeats", json!({})),
        ("memory.import.status", json!({})),
        ("memory.learning", json!({"op": "backlog"})),
        ("notices.list", json!({})),
        ("notifications.categories", json!({})),
        ("projects.list", json!({})),
        ("ps.list", json!({})),
        ("recovery.list", json!({})),
        ("reminders.list", json!({})),
        ("results.ack", json!({"task_ids": []})),
        ("runtimes.status", json!({})),
        ("schedules.list", json!({})),
        ("screen_use.status", json!({})),
        ("secrets.saved", json!({})),
        ("seen.list", json!({})),
        ("sense.status", json!({})),
        ("settings.get", json!({})),
        ("tasks.list", json!({})),
        ("phone.list", json!({})),
        ("phone.upload_folder", json!({})),
        ("timeline.get", json!({})),
        ("timers.states", json!({"ids": []})),
    ]
}

fn call(run: &Run, method: &str, params: Value) -> Value {
    let mut c = Raw::connect(&run.socket()).unwrap();
    c.hello("cli").unwrap();
    c.send(&json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}).to_string());
    serde_json::from_str(&c.recv().expect("an answer")).unwrap()
}

// decision: ENG-19
#[test]
fn every_method_of_the_table_has_a_handler_and_the_reads_answer() {
    let tmp = root();
    let mut run = Run::start(tmp.path(), "01", json!({}));
    let got = run.control(json!({"do": "methods"}));
    let names = |v: &Value| -> BTreeSet<String> {
        v.as_array()
            .unwrap()
            .iter()
            .map(|n| n.as_str().unwrap().to_string())
            .collect()
    };
    let (unary, streams) = (names(&got[0]), names(&got[1]));
    let mut problems: Vec<String> = Vec::new();
    for m in ferb_rpc::auth::METHODS {
        if !unary.contains(m.name) && !streams.contains(m.name) {
            problems.push(format!("{}: in the method table, no handler", m.name));
        }
    }
    // and no handler the table doesn't let anyone call
    let table: BTreeSet<&str> = ferb_rpc::auth::METHODS.iter().map(|m| m.name).collect();
    for n in unary.iter().chain(&streams) {
        if !table.contains(n.as_str()) {
            problems.push(format!("{n}: a handler no role may call"));
        }
    }
    for (method, params) in reads() {
        let v = call(&run, method, params.clone());
        if v.get("result").is_none() {
            problems.push(format!("{method} {params}: {v}"));
        }
    }
    run.stop();
    assert!(
        problems.is_empty(),
        "{} problems:\n{}",
        problems.len(),
        problems.join("\n")
    );
}

/// rpc.py:112 `_update_prepare` (fixture scenario `update_prepare`, never replayed through the program before the audit
/// of 2026-11-16): while a command of a task runs the answer is not ready and names it; with `even_if_busy` it is
/// ready, journaled, and the daemon then stops by itself (exit 0: FERB's own stop, launchd leaves it stopped, DMN-619).
#[test]
fn update_prepare_waits_for_running_work_then_stops_the_daemon() {
    let tmp = root();
    let (mut run, mut shim) = shim::started(tmp.path(), "02");
    let started = shim.call("shell.start", json!({"command": "sleep 5"}));
    assert_eq!(started["is_error"], false, "{started}");
    let busy = call(&run, "update.prepare", json!({"version": "9.9.9"}));
    assert_eq!(busy["result"]["ready"], false, "{busy}");
    assert_eq!(
        busy["result"]["running"].as_array().map(Vec::len),
        Some(1),
        "{busy}"
    );
    let ready = call(
        &run,
        "update.prepare",
        json!({"version": "9.9.9", "even_if_busy": true}),
    );
    assert_eq!(ready["result"]["ready"], true, "{ready}");
    let mut child = run.child.take().unwrap();
    let until = std::time::Instant::now() + std::time::Duration::from_secs(30);
    let status = loop {
        if let Some(s) = child.try_wait().unwrap() {
            break s;
        }
        assert!(std::time::Instant::now() < until, "the daemon didn't stop");
        std::thread::sleep(std::time::Duration::from_millis(50));
    };
    assert_eq!(status.code(), Some(0));
    let db = rusqlite::Connection::open(run.home.join("ferb.db")).unwrap();
    let note: String = db
        .query_row(
            "SELECT data_json FROM journal WHERE kind = 'update.prepared'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(note.contains("9.9.9"), "{note}");
}
