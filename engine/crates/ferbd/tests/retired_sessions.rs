//! The archived sessions (kb HAND-717, HAND-718) through the real programs: `ferb agent-hook …`, which a Claude Code
//! session started before the cleanup still calls, reads its event and says nothing with exit 0; and the real daemon's
//! start takes FERB's hooks out of Claude Code's settings (nothing else), the sessions' folder and `[sessions]`, and
//! journals it once.

mod support;

use std::io::Write;
use std::process::{Command, Stdio};

use serde_json::{json, Value};
use support::*;

const FERB: &str = env!("CARGO_BIN_EXE_ferb");

fn retired_fixture(name: &str) -> String {
    std::fs::read_to_string(repo().join("engine/fixtures/retired-sessions").join(name)).unwrap()
}

// decision: HAND-718
#[test]
fn the_hook_reads_its_event_and_says_nothing_with_exit_0() {
    let tmp = root();
    let home = tmp.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    let big = "x".repeat(2 * 1024 * 1024);
    for (args, input) in [
        (
            vec!["agent-hook", "claude", "UserPromptSubmit"],
            json!({"hook_event_name": "UserPromptSubmit", "prompt": "hello", "session_id": "s1"})
                .to_string(),
        ),
        (
            vec!["agent-hook", "claude", "PermissionRequest"],
            json!({"hook_event_name": "PermissionRequest", "tool_input": {"command": big}})
                .to_string(),
        ),
        (vec!["agent-hook"], "not json".to_string()),
        (vec!["agent-hook", "claude", "Stop"], String::new()),
    ] {
        let mut c = Command::new(FERB)
            .args(&args)
            .env_clear()
            .env("FERB_HOME", &home)
            .env("HOME", tmp.path())
            .env("PATH", "/usr/bin:/bin")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let mut stdin = c.stdin.take().unwrap();
        stdin.write_all(input.as_bytes()).unwrap();
        drop(stdin);
        let out = c.wait_with_output().unwrap();
        assert_eq!(out.status.code(), Some(0), "{args:?}");
        assert!(out.stdout.is_empty(), "{args:?}: {:?}", out.stdout);
        assert!(
            out.stderr.is_empty(),
            "{args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    // it made nothing in FERB's home
    assert_eq!(std::fs::read_dir(&home).unwrap().count(), 0);
}

fn retired_rows(run: &Run) -> Vec<Value> {
    run.table("journal")
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter(|r| r[3] == "sessions.retired")
        .collect()
}

// decision: HAND-718
#[test]
fn the_daemon_s_start_takes_away_what_the_sessions_left_once() {
    let tmp = root();
    let (home, user) = Run::paths(tmp.path(), "01");
    std::fs::create_dir_all(home.join("sessions")).unwrap();
    std::fs::write(home.join("sessions/chat-layouts.json"), "{}").unwrap();
    std::fs::create_dir_all(user.join(".claude")).unwrap();
    let settings = user.join(".claude/settings.json");
    std::fs::write(&settings, retired_fixture("settings-before.json")).unwrap();
    let config = home.join("config.toml");
    std::fs::write(&config, "[sessions]\nenabled = true\n").unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&config, std::fs::Permissions::from_mode(0o600)).unwrap();
    }

    let mut run = Run::start(tmp.path(), "01", json!({}));
    assert_eq!(
        std::fs::read_to_string(&settings).unwrap(),
        retired_fixture("settings-after.json")
    );
    assert!(!home.join("sessions").exists());
    assert!(!std::fs::read_to_string(&config)
        .unwrap()
        .contains("[sessions]"));
    let rows = retired_rows(&run);
    assert_eq!(rows.len(), 1, "{rows:?}");

    // the next start finds nothing left and journals nothing
    run.stop();
    run.spawn();
    assert_eq!(retired_rows(&run).len(), 1);
    assert_eq!(
        std::fs::read_to_string(&settings).unwrap(),
        retired_fixture("settings-after.json")
    );
}

/// The sessions' calls are gone: no role has them, so an older FERB.app or phone asking is refused as for any unknown
/// method, and the settings list no `sessions.enabled`.
// decision: HAND-717
#[test]
fn the_sessions_calls_and_setting_are_gone() {
    let tmp = root();
    let run = Run::start(tmp.path(), "01", json!({}));
    let mut c = Raw::connect(&run.socket()).unwrap();
    c.hello("cli").unwrap();
    for (id, method) in [
        (1, "sessions.list"),
        (2, "sessions.hook"),
        (3, "sessions.remove_hooks"),
    ] {
        c.send(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": {}}).to_string());
        let answer: Value = serde_json::from_str(&c.recv().expect("an answer")).unwrap();
        assert!(answer.get("result").is_none(), "{method}: {answer}");
        // no role has the method any more: refused before any handler (-32001, as every unknown method)
        assert!(ferb_rpc::auth::method(method).is_none(), "{method}");
        assert_eq!(answer["error"]["code"], json!(-32001), "{method}: {answer}");
    }
    c.send(&json!({"jsonrpc": "2.0", "id": 4, "method": "settings.get", "params": {}}).to_string());
    let answer: Value = serde_json::from_str(&c.recv().expect("an answer")).unwrap();
    assert!(answer["result"]["values"].is_object(), "{answer}");
    assert!(answer["result"]["values"].get("sessions.enabled").is_none());
}
