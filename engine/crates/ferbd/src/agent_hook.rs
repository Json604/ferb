//! `ferb agent-hook claude <Event>` (kb HAND-666, HAND-662): the command FERB puts in Claude Code's settings for each hook
//! event. It reads the event's JSON from stdin, hands it to the daemon (`sessions.hook`, the owner's uid's `cli` role)
//! with its own ancestors (so the daemon knows which Claude Code process asks), and prints the daemon's reply, which is
//! empty except for a held permission request the owner answered.
//!
//! It never slows Claude Code down: FERB not running, starting, busy or failing means no output and exit 0 within
//! [`QUICK_S`] (Claude Code then does exactly what it does without FERB). Only `PermissionRequest` waits longer: for
//! the owner's answer, while Claude Code's own dialog stays up beside it (HAND-662); the wait ends with his answer, with
//! the card withdrawn, with FERB stopping (the connection closes: no output), or with Claude Code's own hook timeout,
//! which ends this process (and the daemon drops the card: `sessions.hook` ends with its caller).

use std::io::{Read, Write};

use ferb_device::cli::client::Client;
use ferb_device::cli::Rpc;
use serde_json::{json, Value};

/// The subcommand.
pub const COMMAND: &str = "agent-hook";
/// How long anything but a held permission request may take, start to end (connecting, the daemon's answer).
pub const QUICK_S: f64 = 2.0;
/// The most of stdin read (a hook's JSON; a tool's input can be large).
pub const MAX_INPUT: u64 = 4 * 1024 * 1024;

/// Runs the hook; always exits 0.
pub fn main(args: &[String]) -> i32 {
    let event_arg = args.get(2).cloned().unwrap_or_default();
    let mut input = String::new();
    if std::io::stdin()
        .take(MAX_INPUT)
        .read_to_string(&mut input)
        .is_err()
    {
        return 0;
    }
    let Ok(event) = serde_json::from_str::<Value>(&input) else {
        return 0;
    };
    let name = event
        .get("hook_event_name")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .unwrap_or(event_arg);
    let held = name == ferb_sessions::claude::HELD;
    let reply = report(&event, held).unwrap_or_default();
    if !reply.is_empty() {
        let mut out = std::io::stdout();
        let _ = out.write_all(reply.as_bytes());
        let _ = out.write_all(b"\n");
        let _ = out.flush();
    }
    0
}

fn report(event: &Value, held: bool) -> Option<String> {
    let start = std::time::Instant::now();
    let (home, _) = crate::home::resolve().ok()?;
    let sock = ferb_core::paths::socket_path(&home);
    // connecting and the hello: quick, whatever the event
    let mut c = Client::connect(&sock, "cli", None, QUICK_S).ok()?;
    // a held request waits for the owner (no limit of FERB's own: Claude Code's hook timeout ends it); anything else
    // gets what is left of QUICK_S, start to end
    let left = (QUICK_S - start.elapsed().as_secs_f64()).max(0.05);
    c.set_timeout(if held { None } else { Some(left) });
    let ancestors = ancestors();
    let r = c
        .call(
            "sessions.hook",
            Some(json!({"event": event, "ancestors": ancestors})),
        )
        .ok()?;
    r.get("reply")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

#[cfg(target_os = "macos")]
fn ancestors() -> Vec<i32> {
    ferb_platform_macos::sessions::my_ancestors()
}

#[cfg(not(target_os = "macos"))]
fn ancestors() -> Vec<i32> {
    Vec::new()
}
