//! What the archived sessions feature left on the owner's Mac, taken away (kb HAND-717, HAND-718; engine/ARCHIVE-sessions.md).
//!
//! FERB's sessions (2026-11-22..24) put a command hook for seven events into Claude Code's settings
//! (`~/.claude/settings.json`: `<FERB.app>/Contents/Resources/bin/ferb agent-hook claude <Event>`), kept its own files in
//! `~/.ferb/sessions/` and its switch in config.toml's `[sessions]`. Every daemon start and `ferb uninstall` take them
//! away again ([`retire_sessions`], [`strip_file`]). Only FERB's own hook entries are touched: every other key, hook
//! and order in Claude Code's file stays as it was. `ferb agent-hook` itself stays, doing nothing (ferbd
//! `agent_hook.rs`): a Claude Code session started before the cleanup still calls it until it restarts.

use std::path::Path;

use serde_json::{json, Map, Value};

/// What followed FERB's program in its hook command.
pub const SUBCOMMAND: &str = "agent-hook";

/// The folder in FERB's home the sessions kept their files in.
pub const SESSIONS_FOLDER: &str = "sessions";

/// The config table the sessions kept their switch (`enabled`) and added apps (`chat_apps`) in.
pub const SESSIONS_SECTION: &str = "sessions";

/// Whether a hook command is one of FERB's (any FERB.app's, wherever it was installed): its program is a `ferb`, and
/// `agent-hook claude` follows it.
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

/// Drops FERB's hooks from one event's list (and a group left with none); true when something was dropped.
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

/// Claude Code's settings without FERB's hooks; None when there were none (nothing to write). An event left with no
/// hooks goes, and so does `hooks` left empty. The file is written as Claude Code writes it (two spaces), ending in a
/// newline only if it did. A file that isn't a JSON object is left alone and said.
pub fn remove(text: &str) -> Result<Option<String>, String> {
    if text.trim().is_empty() {
        return Ok(None);
    }
    let mut m: Map<String, Value> = match serde_json::from_str::<Value>(text) {
        Ok(Value::Object(m)) => m,
        Ok(_) => {
            return Err(
                "Claude Code's settings file isn't a settings object, so FERB left it alone."
                    .into(),
            )
        }
        Err(_) => {
            return Err(
                "Claude Code's settings file couldn't be read as JSON, so FERB left it alone."
                    .into(),
            )
        }
    };
    let mut changed = false;
    if let Some(Value::Object(hooks)) = m.get_mut("hooks") {
        let names: Vec<String> = hooks.keys().cloned().collect();
        for name in names {
            if let Some(Value::Array(groups)) = hooks.get_mut(&name) {
                if strip(groups) {
                    changed = true;
                    if groups.is_empty() {
                        hooks.shift_remove(&name);
                    }
                }
            }
        }
        if changed && hooks.is_empty() {
            m.shift_remove("hooks");
        }
    }
    if !changed {
        return Ok(None);
    }
    let mut out = serde_json::to_string_pretty(&Value::Object(m)).map_err(|e| e.to_string())?;
    if text.ends_with('\n') {
        out.push('\n');
    }
    Ok(Some(out))
}

/// Takes FERB's hooks out of the settings file at `path`: true when it had some and the file was written. No file:
/// false. A symlinked file is written where it points, so the link stays.
pub fn strip_file(path: &Path) -> Result<bool, String> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(e.to_string()),
    };
    match remove(&text)? {
        Some(new) => {
            let target = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
            write_atomic(&target, &new)?;
            Ok(true)
        }
        None => Ok(false),
    }
}

/// Writes a file whole or not at all (a temporary file beside it, then a rename), keeping its permissions.
pub fn write_atomic(path: &Path, text: &str) -> Result<(), String> {
    let dir = path.parent().ok_or("no folder")?;
    let tmp = dir.join(format!(
        ".{}.ferb-{}",
        path.file_name()
            .map(|n| n.to_string_lossy())
            .unwrap_or_default(),
        std::process::id()
    ));
    std::fs::write(&tmp, text).map_err(|e| e.to_string())?;
    if let Ok(m) = std::fs::metadata(path) {
        let _ = std::fs::set_permissions(&tmp, m.permissions());
    }
    std::fs::rename(&tmp, path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        e.to_string()
    })
}

/// The daemon's start: FERB's hooks out of Claude Code's settings (`user_home/.claude/settings.json`), the sessions'
/// folder out of FERB's `home`, and `[sessions]` out of config.toml. What it changed, for the journal; None when there
/// was nothing left to take away (every start after the first), so it is journaled once.
pub fn retire_sessions(
    home: &Path,
    user_home: &Path,
    config: &ferb_core::config::Config,
) -> Option<Value> {
    let mut out = Map::new();
    let mut errors = Vec::new();
    let settings = user_home.join(".claude").join("settings.json");
    match strip_file(&settings) {
        Ok(true) => {
            out.insert("hooks_removed".into(), json!(settings.to_string_lossy()));
        }
        Ok(false) => {}
        Err(e) => errors.push(json!({"what": "hooks", "error": e})),
    }
    let folder = home.join(SESSIONS_FOLDER);
    if folder.is_dir() {
        match std::fs::remove_dir_all(&folder) {
            Ok(()) => {
                out.insert("folder_removed".into(), json!(folder.to_string_lossy()));
            }
            Err(e) => errors.push(json!({"what": "folder", "error": e.to_string()})),
        }
    }
    match config.section(SESSIONS_SECTION) {
        Ok(table) if !table.is_empty() => {
            let keys: Vec<(&str, &str)> = table
                .keys()
                .map(|k| (SESSIONS_SECTION, k.as_str()))
                .collect();
            match config.retire(&keys) {
                Ok(changes) if !changes.is_empty() => {
                    out.insert("config".into(), Value::Array(changes));
                }
                Ok(_) => {}
                Err(e) => errors.push(json!({"what": "config", "error": e.to_string()})),
            }
        }
        Ok(_) => {}
        Err(e) => errors.push(json!({"what": "config", "error": e.to_string()})),
    }
    if !errors.is_empty() {
        out.insert("errors".into(), Value::Array(errors));
    }
    (!out.is_empty()).then_some(Value::Object(out))
}
