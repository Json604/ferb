//! FERB's entries in Claude Code's settings (`~/.claude/settings.json`), added after the owner's Yes and removed on
//! his No, the Settings switch or an uninstall (kb HAND-666, HAND-666). Only FERB's own entries are ever touched: each is a
//! command hook whose program is FERB's `ferb` with `agent-hook claude` after it, so an older FERB.app's entries are
//! found too. Every other key, hook and order in the file stays as it was. A file that isn't a JSON object is left
//! alone and said.
//!
//! The functions take the file's text and return the new text (None: nothing to write); the daemon reads and
//! writes the file.

use serde_json::{json, Map, Value};

use crate::claude::HOOK_EVENTS;

/// What follows FERB's program in its hook command.
pub const SUBCOMMAND: &str = "agent-hook";

/// FERB's hook command for `event`.
pub fn command(program: &str, event: &str) -> String {
    format!("{} {SUBCOMMAND} claude {event}", quote(program))
}

fn quote(path: &str) -> String {
    if path
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || "/._-+".contains(c))
    {
        path.to_owned()
    } else {
        format!("'{}'", path.replace('\'', "'\\''"))
    }
}

/// Whether a hook command is one of FERB's (any FERB.app's).
pub fn is_ferbs(command: &str) -> bool {
    let c = command.trim();
    let (prog, rest) = if let Some(r) = c.strip_prefix('\'') {
        match r.split_once('\'') {
            Some((p, rest)) => (p.to_owned(), rest),
            None => return false,
        }
    } else {
        match c.split_once(' ') {
            Some((p, rest)) => (p.to_owned(), rest),
            None => return false,
        }
    };
    let name = prog.rsplit('/').next().unwrap_or("");
    name == "ferb"
        && rest
            .trim_start()
            .starts_with(&format!("{SUBCOMMAND} claude"))
}

fn hook_is_ferbs(h: &Value) -> bool {
    h.get("type").and_then(Value::as_str) == Some("command")
        && h.get("command")
            .and_then(Value::as_str)
            .is_some_and(is_ferbs)
}

fn parse(text: Option<&str>) -> Result<Map<String, Value>, String> {
    let t = text.unwrap_or("").trim();
    if t.is_empty() {
        return Ok(Map::new());
    }
    match serde_json::from_str::<Value>(t) {
        Ok(Value::Object(m)) => Ok(m),
        Ok(_) => Err(
            "Claude Code's settings file isn't a settings object, so FERB left it alone.".into(),
        ),
        Err(_) => Err(
            "Claude Code's settings file couldn't be read as JSON, so FERB left it alone.".into(),
        ),
    }
}

fn write(m: &Map<String, Value>) -> String {
    let mut out = serde_json::to_string_pretty(&Value::Object(m.clone())).unwrap_or_default();
    out.push('\n');
    out
}

/// Drops FERB's hooks from one event's list; true when something was dropped.
fn strip(groups: &mut Vec<Value>) -> bool {
    let before = groups.clone();
    for g in groups.iter_mut() {
        if let Some(Value::Array(hs)) = g.get_mut("hooks") {
            hs.retain(|h| !hook_is_ferbs(h));
        }
    }
    groups.retain(|g| {
        g.get("hooks")
            .and_then(Value::as_array)
            .is_none_or(|hs| !hs.is_empty())
    });
    *groups != before
}

/// The settings with FERB's hooks for every event it listens to, pointing at `program`. None when they are already
/// exactly there.
pub fn install(text: Option<&str>, program: &str) -> Result<Option<String>, String> {
    let mut m = parse(text)?;
    let original = m.clone();
    let hooks = m.entry("hooks").or_insert_with(|| json!({}));
    let Value::Object(hooks) = hooks else {
        return Err(
            "Claude Code's settings have hooks FERB can't read, so FERB left them alone.".into(),
        );
    };
    for event in HOOK_EVENTS {
        let list = hooks.entry(event).or_insert_with(|| json!([]));
        let Value::Array(groups) = list else {
            return Err(
                "Claude Code's settings have hooks FERB can't read, so FERB left them alone."
                    .into(),
            );
        };
        strip(groups);
        groups.push(json!({"hooks": [{"type": "command", "command": command(program, event)}]}));
    }
    if m == original {
        return Ok(None);
    }
    Ok(Some(write(&m)))
}

/// The settings without FERB's hooks. None when there were none.
pub fn remove(text: Option<&str>) -> Result<Option<String>, String> {
    let mut m = parse(text)?;
    let mut changed = false;
    if let Some(Value::Object(hooks)) = m.get_mut("hooks") {
        let names: Vec<String> = hooks.keys().cloned().collect();
        for name in names {
            if let Some(Value::Array(groups)) = hooks.get_mut(&name) {
                if strip(groups) {
                    changed = true;
                    if groups.is_empty() {
                        hooks.remove(&name);
                    }
                }
            }
        }
        if changed && hooks.is_empty() {
            m.remove("hooks");
        }
    }
    Ok(changed.then(|| write(&m)))
}

/// Whether every one of FERB's hooks is there, pointing at `program`.
pub fn installed(text: Option<&str>, program: &str) -> bool {
    let Ok(m) = parse(text) else {
        return false;
    };
    HOOK_EVENTS.iter().all(|event| {
        let want = command(program, event);
        m.get("hooks")
            .and_then(|h| h.get(*event))
            .and_then(Value::as_array)
            .is_some_and(|groups| {
                groups.iter().any(|g| {
                    g.get("hooks").and_then(Value::as_array).is_some_and(|hs| {
                        hs.iter().any(|h| {
                            h.get("command").and_then(Value::as_str) == Some(want.as_str())
                        })
                    })
                })
            })
    })
}
