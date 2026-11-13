//! L12's front-window capture and DMN-5's disturbance sentinel on this Mac, through the real binary on a temp home
//! (ignored: run it by name with `--ignored`; it reads the owner's frontmost window). Nothing is typed, clicked or
//! opened in the owner's apps; no browser, model provider, Keychain or launchctl. The picture stays in the temp home's
//! evidence store and goes with it; the report says only which facts and fields there were, never their words.
//!
//! 1. One capture as a message would get it (`front_window`): facts' keys, whether a redacted picture was kept.
//! 2. One disturbance sample around a harmless on-screen tool: the observer starts for the task, `shell.run` runs
//!    `/usr/bin/true` in FERB's sandbox (its capability declares `on_screen`, so the desktop is sampled before and
//!    after it), the observer ends; the spans it wrote are counted by kind, and its summary's counts read.

mod shim;
mod support;

use serde_json::{json, Map, Value};
use shim::{all_scopes, Shim};
use support::*;

#[test]
#[ignore]
fn real_front_window_and_one_disturbance_sample() {
    let tmp = root();
    let run = Run::start(tmp.path(), "01", json!({"ids": "id", "mac": true}));
    let mut report = Map::new();

    // 1. the front window at send
    let seen = run.control(json!({"do": "front_window", "request_id": "req-real-1"}));
    report.insert("front_window".into(), seen);
    let kept = std::fs::read_dir(run.home.join("evidence"))
        .map(|d| d.count())
        .unwrap_or(0);
    report.insert("evidence_day_folders_in_temp_home".into(), json!(kept));

    // 2. one disturbance sample around a harmless on-screen tool
    let token = run.control(
        json!({"do": "issue", "task_id": "task-1", "worker_id": "w-1", "identity": "owner",
               "scopes": all_scopes()}),
    );
    run.control(json!({"do": "hold_task", "task_id": "task-1"}));
    std::fs::create_dir_all(run.home.join("workspace")).unwrap();
    run.control(json!({"do": "sentinel_start", "tree": "task-1"}));
    let mut shim = Shim::open_waiting(&run, token.as_str().unwrap(), 120);
    let ran = shim.call("shell.run", json!({"command": "/usr/bin/true"}));
    report.insert(
        "shell_run".into(),
        json!({"is_error": ran["is_error"], "exit": ran["content"]["exit_code"],
               "screen_fact_keys": ran["content"].get("screen").and_then(Value::as_object)
                   .map(|m| m.keys().cloned().collect::<Vec<_>>())}),
    );
    run.control(json!({"do": "sentinel_finish", "tree": "task-1"}));
    let spans = run.control(json!({"do": "table", "name": "spans"}));
    let mut kinds: std::collections::BTreeMap<String, i64> = Default::default();
    let mut summary = Value::Null;
    let mut unavailable: Vec<String> = Vec::new();
    for row in spans.as_array().cloned().unwrap_or_default() {
        let kind = row[3].as_str().unwrap_or("").to_string();
        if !kind.starts_with("disturbance") {
            continue;
        }
        *kinds.entry(kind.clone()).or_default() += 1;
        let out: Value = row[8]
            .as_str()
            .and_then(|s| serde_json::from_str(s).ok())
            .unwrap_or(Value::Null);
        if kind == "disturbance.summary" {
            if let Some(u) = out["unavailable"].as_object() {
                unavailable = u.keys().cloned().collect();
            }
            summary = json!({"samples": out["samples"], "changes": out["changes"], "errors": out["errors"],
                             "on_screen_calls": out["on_screen_calls"],
                             "screen_changes": out["screen_changes"].as_array().map(Vec::len)});
        }
    }
    report.insert("disturbance_spans".into(), json!(kinds));
    report.insert("disturbance_summary".into(), summary);
    report.insert("unavailable_fields".into(), json!(unavailable));
    eprintln!("REAL-MACHINE REPORT {}", Value::Object(report));
}
