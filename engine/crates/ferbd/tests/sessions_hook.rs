//! HAND-662 through the real programs: Claude Code's hook command (`ferb agent-hook claude <Event>`, FERB.app's
//! `bin/ferb`) against the real daemon on a temporary home. It never slows Claude Code down: FERB off, missing,
//! hung or stopping means no output and exit 0 within its short bound; only a held permission request waits, for the
//! owner's answer, and FERB stopping meanwhile leaves Claude Code's own dialog to decide.

mod support;

use std::io::Write;
use std::os::unix::net::UnixListener;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use support::*;

const FERB: &str = env!("CARGO_BIN_EXE_ferb");
const SID: &str = "5e5510a5-0000-4000-8000-00000000c0de";
/// The hook's own bound for anything but a held request (agent_hook::QUICK_S), and the slack a loaded machine needs.
const QUICK: Duration = Duration::from_millis(2000 + 1500);

fn hook(home: &std::path::Path, event: Value) -> Child {
    let name = event["hook_event_name"].as_str().unwrap().to_owned();
    let mut c = Command::new(FERB)
        .args(["agent-hook", "claude", &name])
        .env_clear()
        .env("FERB_HOME", home)
        .env("PATH", "/usr/bin:/bin")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    c.stdin
        .take()
        .unwrap()
        .write_all(event.to_string().as_bytes())
        .unwrap();
    c
}

fn finish(c: Child) -> (i32, String, Duration) {
    let t = Instant::now();
    let out = c.wait_with_output().unwrap();
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).trim().to_owned(),
        t.elapsed(),
    )
}

fn event(name: &str, extra: Value) -> Value {
    let mut v =
        json!({"hook_event_name": name, "session_id": SID, "cwd": "/p/app", "transcript_path": ""});
    if let (Value::Object(m), Value::Object(e)) = (&mut v, extra) {
        m.extend(e);
    }
    v
}

fn call(run: &Run, method: &str, params: Value) -> Value {
    let mut c = Raw::connect(&run.socket()).unwrap();
    c.hello("cli").unwrap();
    c.send(&json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}).to_string());
    serde_json::from_str(&c.recv().expect("an answer")).unwrap()
}

fn until(what: &str, mut f: impl FnMut() -> bool) {
    let end = Instant::now() + Duration::from_secs(20);
    while !f() {
        assert!(Instant::now() < end, "{what}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn card(run: &Run) -> Option<Value> {
    let t = call(
        run,
        "sessions.timeline",
        json!({"session": format!("claude:{SID}")}),
    );
    t["result"]["cards"]
        .as_array()
        .and_then(|c| c.first().cloned())
}

// decision: HAND-662
#[test]
fn the_hook_reports_quickly_and_a_held_permission_waits_only_for_the_owner() {
    let tmp = root();
    let mut run = Run::start(tmp.path(), "01", json!({}));
    // off: answered at once, nothing kept
    let (code, out, took) = finish(hook(&run.home, event("SessionStart", json!({}))));
    assert_eq!((code, out.as_str()), (0, ""));
    assert!(took < QUICK, "{took:?}");
    assert_eq!(
        call(&run, "sessions.list", json!({}))["result"]["sessions"],
        json!([])
    );
    // the owner's Yes (FERB.app's switch)
    let set = call(
        &run,
        "settings.set",
        json!({"key": "sessions.enabled", "value": true}),
    );
    assert_eq!(set["result"]["value"], true, "{set}");
    let (code, out, _) = finish(hook(&run.home, event("SessionStart", json!({}))));
    assert_eq!((code, out.as_str()), (0, ""));
    let list = call(&run, "sessions.list", json!({}));
    assert_eq!(
        list["result"]["sessions"][0]["id"],
        format!("claude:{SID}"),
        "{list}"
    );
    // a held permission request: the owner answers from FERB (here FERB.app's sessions.answer), the hook prints it
    let held = hook(
        &run.home,
        event(
            "PermissionRequest",
            json!({"tool_name": "Bash", "tool_input": {"command": "rm -rf build"}}),
        ),
    );
    until("the card", || card(&run).is_some());
    let c = card(&run).unwrap();
    assert_eq!(c["detail"], "rm -rf build");
    let answered = call(
        &run,
        "sessions.answer",
        json!({"session": format!("claude:{SID}"), "card": c["id"], "choice": "deny"}),
    );
    assert_eq!(answered["result"]["answered"], true, "{answered}");
    let (code, out, _) = finish(held);
    assert_eq!(code, 0);
    let reply: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(reply["hookSpecificOutput"]["decision"]["behavior"], "deny");
    // held again, and FERB stops meanwhile: no output, exit 0 (Claude Code's own dialog decides)
    let held = hook(
        &run.home,
        event(
            "PermissionRequest",
            json!({"tool_name": "Bash", "tool_input": {"command": "make"}}),
        ),
    );
    until("the second card", || card(&run).is_some());
    run.stop();
    let (code, out, took) = finish(held);
    assert_eq!((code, out.as_str()), (0, ""));
    assert!(took < QUICK, "{took:?}");
    // FERB not running at all: the same
    let (code, out, took) = finish(hook(
        &run.home,
        event("Stop", json!({"last_assistant_message": "done"})),
    ));
    assert_eq!((code, out.as_str()), (0, ""));
    assert!(took < QUICK, "{took:?}");
}

/// A FERB that takes the connection and never answers (hung, busy, or still starting) costs Claude Code at most the
/// hook's short bound, held request or not.
// decision: HAND-662
#[test]
fn a_hung_ferb_costs_claude_code_at_most_the_short_bound() {
    let tmp = root();
    let home = tmp.path().join("h");
    std::fs::create_dir_all(home.join("run")).unwrap();
    let listener = UnixListener::bind(home.join("run/ferb.sock")).unwrap();
    let keep = std::thread::spawn(move || {
        let mut held = Vec::new();
        for s in listener.incoming().take(2) {
            held.push(s.unwrap());
        }
        std::thread::sleep(Duration::from_secs(8));
        drop(held);
    });
    for name in ["Notification", "PermissionRequest"] {
        let t = Instant::now();
        let (code, out, _) = finish(hook(
            &home,
            event(
                name,
                json!({"tool_name": "Bash", "tool_input": {"command": "x"}}),
            ),
        ));
        assert_eq!((code, out.as_str()), (0, ""), "{name}");
        assert!(t.elapsed() < QUICK, "{name}: {:?}", t.elapsed());
    }
    drop(keep);
    // what isn't JSON is ignored at once
    let mut c = Command::new(FERB)
        .args(["agent-hook", "claude", "Stop"])
        .env_clear()
        .env("FERB_HOME", &home)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    c.stdin.take().unwrap().write_all(b"not json").unwrap();
    let (code, out, _) = finish(c);
    assert_eq!((code, out.as_str()), (0, ""));
}

// decision: HAND-702 — the hand-over turn's own frames say what became of it: a plan with its task, a question, a
// failure; a plain reply only when nothing else came
#[test]
fn a_hand_over_turn_s_frames_say_what_became_of_it() {
    use ferb_daemon::sessions::Hand;
    use ferbd::compose::sessions::hand_of;
    use serde_json::json;
    let mut said = None;
    assert_eq!(
        hand_of(
            &json!({"type": "plan", "task_id": "t1", "goal": "g"}),
            &mut said
        ),
        Some(Hand::Started("t1".into()))
    );
    assert_eq!(
        hand_of(
            &json!({"type": "plan", "task_id": "t1", "preview": true}),
            &mut said
        ),
        None
    );
    assert_eq!(
        hand_of(
            &json!({"type": "clarify", "text": "What about?"}),
            &mut said
        ),
        Some(Hand::Asked("What about?".into()))
    );
    assert_eq!(
        hand_of(
            &json!({"type": "reply", "text": "I couldn't start that.", "status": "failed"}),
            &mut said
        ),
        Some(Hand::Failed("I couldn't start that.".into()))
    );
    assert_eq!(
        hand_of(&json!({"type": "reply", "text": "It's idle."}), &mut said),
        None
    );
    assert_eq!(said.as_deref(), Some("It's idle."));
    assert_eq!(hand_of(&json!({"type": "ack"}), &mut said), None);
}
