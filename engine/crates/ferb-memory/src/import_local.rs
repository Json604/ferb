//! What the owner typed into the coding assistants on this Mac, from their prompt-history files (IMP-16, MEM-432):
//! `~/.claude/history.jsonl` (Claude Code: `display`, `timestamp` in ms, `sessionId`, `project`) and
//! `~/.codex/history.jsonl` (Codex: `text`, `ts` in s, `session_id`). Unlike session transcripts these hold only the
//! owner's own words, are a few MB, and Claude Code does not delete them after 30 days. Codex's lines carry no
//! folder: it is read from the first line of the session's own file while that file still exists (only its start is
//! looked at, never the whole rollout).
//!
//! What is read becomes one conversation per session per day (its id `<session>@<day>`), so a session that grows
//! only changes today's conversation and the import's progress by part hash stays incremental with no new state
//! (MEM-433). Lines that say nothing by their shape alone are dropped (an empty line, a bare slash command, an
//! interrupted-turn marker), and so is an exact repeat of a line typed earlier in the window: a structural filter,
//! never a judgement of what a line means (AGENTS.md rule 4).
//!
//! Also the shared rows of the work log (`rows`), which keep the session, folder and time of every line.

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::import_sources::{
    self, display, finish, glob, name_of, owner_words, sorted, Conversation, Message,
};
use crate::pystr;
use crate::pyval;
use crate::times::{self, DateTime, LocalZone};
use crate::Error;

/// The two prompt-history files: (source, path under the home folder, text key, time key, session key).
pub const FILES: [(&str, &str, &str, &str, &str); 2] = [
    (
        "claude_code",
        ".claude/history.jsonl",
        "display",
        "timestamp",
        "sessionId",
    ),
    ("codex", ".codex/history.jsonl", "text", "ts", "session_id"),
];
/// How much of a Codex session file's first line is looked at for its folder.
pub const HEAD_BYTES: usize = 65_536;

/// One line the owner typed: when, in which session and folder, and the words.
#[derive(Clone, Debug, PartialEq)]
pub struct Line {
    pub source: String,
    pub at: DateTime,
    pub session: String,
    pub folder: String,
    pub text: String,
}

/// The prompt-history file of `source` under `home`.
pub fn history_file(home: &Path, source: &str) -> Option<PathBuf> {
    FILES.iter().find(|f| f.0 == source).map(|f| home.join(f.1))
}

/// Which coding assistants have a prompt history on this Mac (what the setup question names).
pub fn found(home: &Path) -> Vec<&'static str> {
    FILES
        .iter()
        .filter(|f| home.join(f.1).is_file())
        .map(|f| f.0)
        .collect()
}

/// The folder's own name (the last part of a path, either separator).
fn base(path: &str) -> String {
    path.trim_end_matches(['/', '\\'])
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or("")
        .to_owned()
}

/// A Codex session id in a rollout's file name (`rollout-<time>-<uuid>.jsonl`).
fn session_in_name(name: &str) -> Option<String> {
    let bytes = name.as_bytes();
    let shape = [8usize, 4, 4, 4, 12];
    let total = 36;
    if bytes.len() < total {
        return None;
    }
    'outer: for start in (0..=bytes.len() - total).rev() {
        let mut at = start;
        for (n, len) in shape.iter().enumerate() {
            for _ in 0..*len {
                if !bytes[at].is_ascii_hexdigit() || bytes[at].is_ascii_uppercase() {
                    continue 'outer;
                }
                at += 1;
            }
            if n < 4 {
                if bytes[at] != b'-' {
                    continue 'outer;
                }
                at += 1;
            }
        }
        return Some(name[start..start + total].to_owned());
    }
    None
}

/// The `cwd` a session file's header line names, from its first `HEAD_BYTES` only.
fn header_cwd(path: &Path) -> Option<String> {
    let mut head = vec![0u8; HEAD_BYTES];
    let mut f = fs::File::open(path).ok()?;
    let mut n = 0;
    while n < HEAD_BYTES {
        match f.read(&mut head[n..]) {
            Ok(0) => break,
            Ok(k) => n += k,
            Err(_) => return None,
        }
    }
    head.truncate(n);
    let first = match head.iter().position(|b| *b == b'\n') {
        Some(i) => &head[..i],
        None => &head[..],
    };
    let text = String::from_utf8_lossy(first);
    if let Ok(Value::Object(m)) = serde_json::from_str::<Value>(&text) {
        let cwd = m
            .get("payload")
            .and_then(|p| p.get("cwd"))
            .or_else(|| m.get("cwd"));
        return cwd.and_then(Value::as_str).map(str::to_owned);
    }
    // a header longer than what was read: the first "cwd" string in it
    let at = text.find("\"cwd\"")?;
    let rest = text[at + 5..].trim_start().strip_prefix(':')?.trim_start();
    let rest = rest.strip_prefix('"')?;
    let mut raw = String::from("\"");
    let mut escaped = false;
    for c in rest.chars() {
        raw.push(c);
        if escaped {
            escaped = false;
        } else if c == '\\' {
            escaped = true;
        } else if c == '"' {
            break;
        }
    }
    serde_json::from_str::<String>(&raw).ok()
}

/// Codex session id -> the folder it ran in, from the header of each session file still on this Mac.
pub fn codex_folders(home: &Path) -> Vec<(String, String)> {
    let root = home.join(".codex");
    let mut files = glob(&root, "sessions/**/rollout-*");
    files.extend(glob(&root, "archived_sessions/**/rollout-*"));
    let mut out: Vec<(String, String)> = Vec::new();
    for path in sorted(files) {
        let Some(id) = session_in_name(&name_of(&path)) else {
            continue;
        };
        if out.iter().any(|(k, _)| *k == id) {
            continue;
        }
        let folder = header_cwd(&path).map(|c| base(&c)).unwrap_or_default();
        out.push((id, folder));
    }
    out
}

/// Every line typed into `source` between `since` and `now`, oldest first. A missing file reads as nothing.
pub fn lines(home: &Path, source: &str, since: &DateTime, now: &DateTime) -> Vec<Line> {
    let Some(&(_, rel, text_key, time_key, session_key)) = FILES.iter().find(|f| f.0 == source)
    else {
        return Vec::new();
    };
    let Ok(bytes) = fs::read(home.join(rel)) else {
        return Vec::new();
    };
    let folders = if source == "codex" {
        codex_folders(home)
    } else {
        Vec::new()
    };
    let (lo, hi) = (times::instant(since), times::instant(now));
    let mut out = Vec::new();
    for raw in String::from_utf8_lossy(&bytes).lines() {
        let Ok(Value::Object(r)) = serde_json::from_str::<Value>(raw) else {
            continue;
        };
        let Some(Value::String(text)) = r.get(text_key) else {
            continue;
        };
        let Some(t) = r.get(time_key).and_then(Value::as_f64) else {
            continue;
        };
        let Some(at) = times::from_timestamp_utc(if t > 1e11 { t / 1000.0 } else { t }) else {
            continue;
        };
        let i = times::instant(&at);
        if i < lo || i > hi {
            continue;
        }
        let session = match r.get(session_key) {
            Some(v) if pyval::truthy(v) => pyval::to_str(v),
            _ => times::ymd(&at),
        };
        let folder = match r.get("project").and_then(Value::as_str) {
            Some(p) if !base(p).is_empty() => base(p),
            _ => folders
                .iter()
                .find(|(k, _)| *k == session)
                .map(|(_, f)| f.clone())
                .unwrap_or_default(),
        };
        out.push(Line {
            source: source.to_owned(),
            at,
            session,
            folder,
            text: text.clone(),
        });
    }
    out.sort_by_key(|l| times::instant(&l.at));
    out
}

/// A line that says nothing by its shape alone: empty once tags are dropped, a bare slash command (`/clear`), or the
/// tool's interrupted-turn marker.
pub fn noise(words: &str) -> bool {
    let w = pystr::strip(words);
    if w.is_empty() || w.starts_with("[Request interrupted") {
        return true;
    }
    if let Some(rest) = w.strip_prefix('/') {
        return !rest.is_empty()
            && rest
                .chars()
                .all(|c| c.is_alphanumeric() || c == '_' || c == ':' || c == '-');
    }
    false
}

/// The lines as the import reads them: the owner's words, noise and exact repeats dropped (the first time a line
/// was typed in the window is kept), with their session, folder and day.
pub fn kept(lines: &[Line], zone: &dyn LocalZone) -> Vec<(Line, String)> {
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut out = Vec::new();
    for l in lines {
        let words = owner_words(&l.text);
        if noise(&words) || !seen.insert(words.clone()) {
            continue;
        }
        let day =
            times::astimezone(&l.at, zone).map_or_else(|| times::ymd(&l.at), |d| times::ymd(&d));
        out.push((
            Line {
                text: words,
                ..l.clone()
            },
            day,
        ));
    }
    out
}

/// The lines as conversations, one per session per day, titled by the folder ("in the folder X").
pub fn conversations(lines: &[Line], zone: &dyn LocalZone) -> Vec<Conversation> {
    let mut out: Vec<Conversation> = Vec::new();
    for (l, day) in kept(lines, zone) {
        let id = format!("{}@{day}", l.session);
        let at = match out.iter().position(|c| c.id == id && c.source == l.source) {
            Some(i) => i,
            None => {
                let mut c = Conversation::new(
                    &l.source,
                    id,
                    if l.folder.is_empty() {
                        String::new()
                    } else {
                        format!("in the folder {}", l.folder)
                    },
                );
                c.place = l.folder.clone();
                out.push(c);
                out.len() - 1
            }
        };
        if out[at].place.is_empty() && !l.folder.is_empty() {
            out[at].place = l.folder.clone();
            out[at].title = format!("in the folder {}", l.folder);
        }
        out[at]
            .messages
            .push(Message::new("owner", l.text, Some(l.at)));
    }
    out
}

/// Claude Code's own saved-memory notes (`~/.claude/projects/*/memory/*.md`), as the assistant's claims (IMP-29).
pub fn claude_code_notes(home: &Path) -> Vec<Conversation> {
    import_sources::claude_code_notes(&home.join(".claude").join("projects"))
}

/// `read` for the coding assistants with no folder given: the prompt history in the window (and Claude Code's saved
/// notes), cut and redacted by `finish` like every source.
pub fn read(
    source: &str,
    home: &Path,
    now: &DateTime,
    since: &DateTime,
    zone: &dyn LocalZone,
    redact: &dyn Fn(&str) -> String,
) -> Result<Vec<Conversation>, Error> {
    let file = history_file(home, source).unwrap_or_default();
    if !file.is_file() {
        return Err(Error::Unreadable(format!(
            "no {} prompt history at {}",
            import_sources::source_name(source).unwrap_or(source),
            display(&file)
        )));
    }
    let mut convs = conversations(&lines(home, source, since, now), zone);
    if source == "claude_code" {
        convs.extend(claude_code_notes(home));
    }
    finish(convs, since, redact)
}
