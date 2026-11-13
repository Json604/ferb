//! Reading other assistants' records into one normalized form, in memory only (`import_sources.py`, MEM-400..415):
//! ChatGPT and claude.ai data exports, Gemini's Takeout, Claude Code's and Codex CLI's local transcripts. Nothing
//! read is persisted or logged here; every text is cut and redacted before it leaves (`finish`).
//!
//! Python's behavior is kept, with two deliberate differences (journal): the Gemini HTML parsers are closed after
//! feeding, so text after the last tag is read (Python drops it when it holds a `&` near its end), and
//! `window_start` gives up on a date before year 1 instead of looping forever. A compressed Codex rollout
//! (`.jsonl.zst`) is read through the [`Unzstd`] seam; without a decoder it is skipped.

use std::cmp::Ordering;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::html::{self, Handler};
use crate::pystr;
use crate::pyval::{self, Crash};
use crate::times::{self, DateTime, LocalZone};
use crate::zip;
use crate::Error;

/// The sources and their names (`SOURCES`).
pub const SOURCES: [(&str, &str); 5] = [
    ("chatgpt", "ChatGPT"),
    ("claude", "Claude"),
    ("claude_code", "Claude Code"),
    ("codex", "Codex CLI"),
    ("gemini", "Gemini"),
];
/// Read from a file the owner gives; the other two from this Mac.
pub const EXPORTS: [&str; 3] = ["chatgpt", "claude", "gemini"];
pub const WINDOW_MONTHS: u32 = 3;
pub const OWNER_CHARS: usize = 6000;
pub const ASSISTANT_CHARS: usize = 700;
pub const CLAIM_CHARS: usize = 6000;
pub const MEMBER_LIMIT: u64 = 1 << 30;
pub const CODEX_INTERACTIVE: [&str; 2] = ["cli", "vscode"];

/// The display name of a source.
pub fn source_name(source: &str) -> Option<&'static str> {
    SOURCES.iter().find(|(k, _)| *k == source).map(|(_, v)| *v)
}

/// One message: role "owner", "assistant" or "tool".
#[derive(Clone, Debug)]
pub struct Message {
    pub role: String,
    pub text: String,
    pub at: Option<DateTime>,
}

impl PartialEq for Message {
    fn eq(&self, other: &Message) -> bool {
        self.role == other.role
            && self.text == other.text
            && match (&self.at, &other.at) {
                (None, None) => true,
                (Some(a), Some(b)) => times::equal(a, b),
                _ => false,
            }
    }
}

impl Message {
    pub fn new(role: &str, text: impl Into<String>, at: Option<DateTime>) -> Message {
        Message {
            role: role.to_owned(),
            text: text.into(),
            at,
        }
    }
}

/// One conversation (or an assistant's saved memory: `claims`).
#[derive(Clone, Debug, PartialEq)]
pub struct Conversation {
    pub source: String,
    pub id: String,
    pub title: String,
    pub messages: Vec<Message>,
    pub claims: bool,
}

impl Conversation {
    pub fn new(source: &str, id: impl Into<String>, title: impl Into<String>) -> Conversation {
        Conversation {
            source: source.to_owned(),
            id: id.into(),
            title: title.into(),
            messages: Vec::new(),
            claims: false,
        }
    }

    /// `last`: the latest message time.
    pub fn last(&self) -> Option<&DateTime> {
        let mut best: Option<&DateTime> = None;
        for at in self.messages.iter().filter_map(|m| m.at.as_ref()) {
            if best.is_none_or(|b| times::instant(at) > times::instant(b)) {
                best = Some(at);
            }
        }
        best
    }
}

/// `window_start`: three calendar months before `now` (the same day, or the month's last day). `None` before
/// year 1 (Python loops forever there).
pub fn window_start(now: &DateTime) -> Option<DateTime> {
    let mut month = now.month as i64 - i64::from(WINDOW_MONTHS);
    let mut year = i64::from(now.year);
    while month < 1 {
        month += 12;
        year -= 1;
    }
    if !(1..=9999).contains(&year) {
        return None;
    }
    let day = now.day.min(times::days_in_month(year, month as u32));
    Some(DateTime {
        year: year as i32,
        month: month as u32,
        day,
        ..*now
    })
}

/// `cut`: within `limit` characters, cut before the word that crosses it, the cut said in the text.
pub fn cut(text: &str, limit: usize) -> String {
    let text = pystr::strip(text);
    let len = pystr::char_len(text);
    if len <= limit {
        return text.to_owned();
    }
    let head = pystr::prefix(text, limit);
    let space = head.rfind([' ', '\n', '\t']);
    let head = match space {
        Some(byte) if head[..byte].chars().count() > limit / 2 => &head[..byte],
        _ => "",
    };
    format!(
        "{} [… {} more characters not read]",
        pystr::rstrip(head),
        len - pystr::char_len(head)
    )
}

const NAME_REST: fn(char) -> bool =
    |c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-';

/// The tag opened at `i` (`<` there): `<[a-z][a-z0-9_\-]{1,40}(?:\s[^<>]*)?>`, as (name, end).
fn open_tag(chars: &[char], i: usize) -> Option<(String, usize)> {
    if chars.get(i) != Some(&'<') || !chars.get(i + 1)?.is_ascii_lowercase() {
        return None;
    }
    let mut j = i + 2;
    while j < chars.len() && j - (i + 1) < 41 && NAME_REST(chars[j]) {
        j += 1;
    }
    if j - (i + 1) < 2 {
        return None;
    }
    let name: String = chars[i + 1..j].iter().collect();
    match chars.get(j) {
        Some('>') => Some((name, j + 1)),
        Some(&c) if pystr::is_space(c) => {
            let mut k = j + 1;
            while k < chars.len() && chars[k] != '<' && chars[k] != '>' {
                k += 1;
            }
            (chars.get(k) == Some(&'>')).then_some((name, k + 1))
        }
        _ => None,
    }
}

/// `owner_words`: a turn's typed words: injected blocks (`<tag>…</tag>`) dropped, pasted ones named; a turn that
/// only opens such a block holds none of the owner's words.
pub fn owner_words(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '<' {
            if let Some((name, after)) = open_tag(&chars, i) {
                let close: Vec<char> = format!("</{name}>").chars().collect();
                let found = (after..=chars.len().saturating_sub(close.len())).find(|&p| {
                    p + close.len() <= chars.len() && chars[p..p + close.len()] == close[..]
                });
                if let Some(p) = found {
                    out.push_str(if name.contains("paste") {
                        " [pasted text, not read] "
                    } else {
                        " "
                    });
                    i = p + close.len();
                    continue;
                }
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    let lead: Vec<char> = out.chars().collect();
    let mut s = 0;
    while s < lead.len() && pystr::is_space(lead[s]) {
        s += 1;
    }
    if open_tag(&lead, s).is_some() && !pystr::strip(&out).starts_with("[pasted") {
        return String::new();
    }
    // [ \t]{2,} -> " "
    let mut squeezed = String::with_capacity(out.len());
    let mut run = String::new();
    for c in out.chars() {
        if c == ' ' || c == '\t' {
            run.push(c);
            continue;
        }
        squeezed.push_str(if run.chars().count() >= 2 { " " } else { &run });
        run.clear();
        squeezed.push(c);
    }
    squeezed.push_str(if run.chars().count() >= 2 { " " } else { &run });
    pystr::strip(&squeezed).to_owned()
}

/// What `finish` and the readers need beyond the files.
pub struct ReadOptions<'a> {
    pub now: DateTime,
    pub redact: &'a dyn Fn(&str) -> String,
    /// Where `~/.claude` and `~/.codex` are (default: `$HOME`).
    pub home: Option<&'a Path>,
    /// This device's zone (Gemini's local times and day grouping).
    pub zone: &'a dyn LocalZone,
    /// A Zstandard decoder for compressed Codex rollouts.
    pub unzstd: Option<&'a dyn Unzstd>,
}

/// Decompresses a `.jsonl.zst` rollout: what could be read, and whether the stream was damaged after it.
pub trait Unzstd {
    fn decompress(&self, data: &[u8]) -> (Vec<u8>, bool);
}

/// `finish`: the window, the caps, redaction, only conversations holding an owner message (or saved claims) in
/// the window; oldest first by their last message.
pub fn finish(
    conversations: Vec<Conversation>,
    since: &DateTime,
    redact: &dyn Fn(&str) -> String,
) -> Result<Vec<Conversation>, Error> {
    let mut out = Vec::new();
    for conv in conversations {
        let mut kept = Vec::new();
        for m in &conv.messages {
            let Some(at) = &m.at else { continue };
            if m.role == "tool"
                || times::compare(at, since)? == Ordering::Less
                || pystr::strip(&m.text).is_empty()
            {
                continue;
            }
            let limit = if m.role == "owner" {
                OWNER_CHARS
            } else if conv.claims {
                CLAIM_CHARS
            } else {
                ASSISTANT_CHARS
            };
            let text = pystr::strip(&redact(&cut(&m.text, limit))).to_owned();
            if !text.is_empty() {
                kept.push(Message::new(&m.role, text, Some(*at)));
            }
        }
        if kept.is_empty() || !(conv.claims || kept.iter().any(|m| m.role == "owner")) {
            continue;
        }
        out.push(Conversation {
            source: conv.source,
            id: conv.id,
            title: redact(&cut(&conv.title, 200)),
            messages: kept,
            claims: conv.claims,
        });
    }
    let key = |c: &Conversation| times::instant(c.last().unwrap_or(since));
    out.sort_by(|a, b| key(a).cmp(&key(b)).then_with(|| a.id.cmp(&b.id)));
    Ok(out)
}

fn unreadable(msg: String) -> Error {
    Error::Unreadable(msg)
}

/// `read`: the source's conversations in the window, normalized and redacted. `path`: the export the owner gave
/// (a zip, its folder or the JSON/HTML file), or for the local sources a folder in place of the usual one.
pub fn read(
    source: &str,
    path: Option<&Path>,
    opts: &ReadOptions<'_>,
) -> Result<Vec<Conversation>, Error> {
    let since = window_start(&opts.now).ok_or(Crash("ValueError"))?;
    if EXPORTS.contains(&source) && path.is_none() {
        return Err(unreadable(format!(
            "{} is imported from its data export: choose the file",
            source_name(source).unwrap_or(source)
        )));
    }
    let home = match opts.home {
        Some(h) => h.to_path_buf(),
        None => PathBuf::from(std::env::var_os("HOME").unwrap_or_default()),
    };
    let expanded = path.map(expanduser);
    let path = expanded.as_deref();
    let convs = match source {
        "chatgpt" => chatgpt(
            &files(
                path.unwrap_or(Path::new("")),
                &["conversations"],
                &[".json"],
            )?,
            &opts.now,
        )?,
        "claude" => claude(
            &files(
                path.unwrap_or(Path::new("")),
                &["conversations", "projects", "memories", "memory"],
                &[".json"],
            )?,
            &opts.now,
        )?,
        "gemini" => gemini(
            &files(
                path.unwrap_or(Path::new("")),
                &["myactivity", "my activity"],
                &[".json", ".html"],
            )?,
            opts.zone,
        )?,
        "claude_code" => claude_code(
            &path.map_or_else(|| home.join(".claude").join("projects"), Path::to_path_buf),
        )?,
        "codex" => codex(
            &path.map_or_else(|| home.join(".codex"), Path::to_path_buf),
            opts.unzstd,
        )?,
        _ => {
            return Err(unreadable(format!(
                "unknown source {}: one of {}",
                pystr::repr(source),
                SOURCES
                    .iter()
                    .map(|(k, _)| *k)
                    .collect::<Vec<_>>()
                    .join(", ")
            )))
        }
    };
    finish(convs, &since, opts.redact)
}

// ---------- files ----------

fn name_of(path: &Path) -> String {
    path.file_name()
        .map_or_else(String::new, |n| n.to_string_lossy().into_owned())
}

/// `Path.expanduser()` for `~` and `~/…` (from `$HOME`, as Python reads it first); `~user` is left as it is.
fn expanduser(path: &Path) -> PathBuf {
    let text = path.to_string_lossy();
    match text.strip_prefix('~') {
        Some(rest) if rest.is_empty() || rest.starts_with('/') => match std::env::var_os("HOME") {
            Some(h) => PathBuf::from(format!("{}{rest}", PathBuf::from(h).to_string_lossy())),
            None => path.to_path_buf(),
        },
        _ => path.to_path_buf(),
    }
}

/// `str(path)`.
fn display(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// Python's `Path` ordering: by components.
fn path_cmp(a: &Path, b: &Path) -> Ordering {
    let parts = |p: &Path| -> Vec<String> {
        p.components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect()
    };
    parts(a).cmp(&parts(b))
}

/// The directories `**` visits from `dir`: itself and every directory below it not reached through a link.
fn walk_dirs(dir: &Path) -> Vec<PathBuf> {
    let mut out = vec![dir.to_path_buf()];
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(entries) = fs::read_dir(&d) else {
            continue;
        };
        let mut subdirs = Vec::new();
        for e in entries.flatten() {
            if e.file_type().is_ok_and(|t| t.is_dir()) {
                subdirs.push(e.path());
            }
        }
        for s in subdirs.iter().rev() {
            stack.push(s.clone());
        }
        out.extend(subdirs);
    }
    out
}

/// `fnmatch` of a name against a pattern of literal text and `*`.
fn fnmatch(name: &str, pattern: &str) -> bool {
    let parts: Vec<&str> = pattern.split('*').collect();
    if parts.len() == 1 {
        return name == pattern;
    }
    let (first, last) = (parts[0], parts[parts.len() - 1]);
    if !name.starts_with(first) || name.len() < first.len() + last.len() || !name.ends_with(last) {
        return false;
    }
    let mut at = first.len();
    let end = name.len() - last.len();
    for mid in &parts[1..parts.len() - 1] {
        match name[at..end].find(mid) {
            Some(p) => at += p + mid.len(),
            None => return false,
        }
    }
    true
}

/// `Path.glob(pattern)` for patterns of literal parts, `*` parts and `**` (Python 3.12's selectors).
fn glob(root: &Path, pattern: &str) -> Vec<PathBuf> {
    fn select(dir: &Path, parts: &[&str], out: &mut Vec<PathBuf>) {
        let Some((part, rest)) = parts.split_first() else {
            out.push(dir.to_path_buf());
            return;
        };
        if *part == "**" {
            let mut rest = rest;
            while rest.first() == Some(&"**") {
                rest = &rest[1..];
            }
            for d in walk_dirs(dir) {
                select(&d, rest, out);
            }
            return;
        }
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if !rest.is_empty() && !e.path().is_dir() {
                continue;
            }
            if fnmatch(&name, part) {
                select(&dir.join(&name), rest, out);
            }
        }
    }
    let mut out = Vec::new();
    if root.is_dir() {
        let parts: Vec<&str> = pattern.split('/').collect();
        select(root, &parts, &mut out);
    }
    out
}

fn sorted(mut paths: Vec<PathBuf>) -> Vec<PathBuf> {
    paths.sort_by(|a, b| path_cmp(a, b));
    paths
}

/// `_files`: (name, bytes) of the export's files whose name (or, for claude.ai's 2026 layout, folder) starts with
/// one of `stems`: from a zip without extracting it, a folder (searched, zips inside read), or the file itself.
pub fn files(
    path: &Path,
    stems: &[&str],
    suffixes: &[&str],
) -> Result<Vec<(String, Vec<u8>)>, Error> {
    let wanted = |name: &str| -> bool {
        let parts: Vec<String> = name
            .replace('\\', "/")
            .split('/')
            .filter(|x| !x.is_empty())
            .map(pystr::lower)
            .collect();
        let Some(last) = parts.last() else {
            return false;
        };
        suffixes.iter().any(|s| last.ends_with(s))
            && parts[parts.len().saturating_sub(2)..]
                .iter()
                .any(|x| stems.iter().any(|s| x.starts_with(s)))
    };
    let mut found: Vec<(String, Vec<u8>)> = Vec::new();
    let from_zip =
        |data: &[u8], where_: &str, found: &mut Vec<(String, Vec<u8>)>| -> Result<(), Error> {
            for m in zip::members(data)? {
                if m.is_dir() || !wanted(&m.name) {
                    continue;
                }
                if m.file_size > MEMBER_LIMIT {
                    return Err(unreadable(format!(
                        "{} in the export is larger than 1 GiB",
                        m.name
                    )));
                }
                found.push((format!("{where_}/{}", m.name), zip::read(data, &m)?));
            }
            Ok(())
        };
    if !path.exists() {
        return Err(unreadable(format!("{} doesn't exist", display(path))));
    }
    if path.is_file() {
        let data = fs::read(path)?;
        if zip::is_zip(&data) {
            from_zip(&data, &name_of(path), &mut found)?;
        } else {
            found.push((name_of(path), data));
        }
    } else if path.is_dir() {
        for p in sorted(glob(path, "**/*")) {
            let rel = display(p.strip_prefix(path).unwrap_or(&p));
            if !p.is_file() {
                continue;
            }
            if pystr::lower(suffix(&name_of(&p))) == ".zip" {
                let data = fs::read(&p)?;
                if zip::is_zip(&data) {
                    from_zip(&data, &rel, &mut found)?;
                    continue;
                }
                if wanted(&rel) {
                    found.push((rel, data));
                }
            } else if wanted(&rel) {
                found.push((rel, fs::read(&p)?));
            }
        }
    }
    if found.is_empty() {
        return Err(unreadable(format!(
            "no {} file in {}: is it the right export?",
            stems.join(" / "),
            name_of(path)
        )));
    }
    Ok(found)
}

/// `_json`: the file as JSON (a UTF-8 BOM allowed), else unreadable.
fn json(name: &str, data: &[u8]) -> Result<Value, Error> {
    let data = data.strip_prefix(b"\xef\xbb\xbf").unwrap_or(data);
    let text = std::str::from_utf8(data).map_err(|_| unreadable(format!("{name} isn't JSON")))?;
    serde_json::from_str(text).map_err(|_| unreadable(format!("{name} isn't JSON")))
}

/// `_unix`: a Unix time in seconds as an aware UTC time, or `None`.
pub fn unix(value: Option<&Value>) -> Option<DateTime> {
    match value {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) if s.is_empty() => None,
        Some(v) => times::from_timestamp_utc(pyval::float(v)?),
    }
}

/// `_iso`: an ISO time ("Z" read as UTC; a naive one taken as UTC), or `None`.
pub fn iso(value: Option<&Value>) -> Option<DateTime> {
    let Some(Value::String(s)) = value else {
        return None;
    };
    if s.is_empty() {
        return None;
    }
    let mut at = times::parse_iso(&s.replace('Z', "+00:00"))?;
    if at.offset.is_none() {
        at.offset = Some((0, 0));
    }
    Some(at)
}

/// The lines of a JSON Lines text as Python's text mode reads them (`\r\n` and `\r` end a line too).
fn universal_lines(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut line = String::new();
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\r' => {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                line.push('\n');
                out.push(std::mem::take(&mut line));
            }
            '\n' => {
                line.push('\n');
                out.push(std::mem::take(&mut line));
            }
            c => line.push(c),
        }
    }
    if !line.is_empty() {
        out.push(line);
    }
    out
}

/// `_jsonl`: a JSON Lines file's objects (lines that aren't JSON objects skipped); a compressed one through
/// `unzstd` (what was read before damage stays); an unreadable file yields nothing.
fn jsonl(path: &Path, unzstd: Option<&dyn Unzstd>) -> Vec<serde_json::Map<String, Value>> {
    let Ok(bytes) = fs::read(path) else {
        return Vec::new();
    };
    let text = if path.extension().is_some_and(|e| e == "zst") {
        let Some(z) = unzstd else { return Vec::new() };
        let (bytes, damaged) = z.decompress(&bytes);
        let mut text = String::from_utf8_lossy(&bytes).into_owned();
        if damaged {
            // a line the damage cut short was never yielded
            match text.rfind(['\n', '\r']) {
                Some(i) => text.truncate(i + 1),
                None => text.clear(),
            }
        }
        text
    } else {
        String::from_utf8_lossy(&bytes).into_owned()
    };
    universal_lines(&text)
        .into_iter()
        .filter(|l| !pystr::strip(l).is_empty())
        .filter_map(|l| match serde_json::from_str::<Value>(&l) {
            Ok(Value::Object(m)) => Some(m),
            _ => None,
        })
        .collect()
}

// ---------- ChatGPT ----------

fn chatgpt_path(conv: &Value) -> Result<Vec<Value>, Error> {
    let mapping = match pyval::get(conv, "mapping")? {
        Some(v) if pyval::truthy(v) => v,
        _ => return Ok(Vec::new()),
    };
    let Value::Object(map) = mapping else {
        return Err(Crash("TypeError").into());
    };
    let mut node_id = pyval::get(conv, "current_node")?.cloned();
    let mut path = Vec::new();
    let mut seen = std::collections::HashSet::new();
    while let Some(Value::String(id)) = &node_id {
        let Some(node) = map.get(id) else { break };
        if !seen.insert(id.clone()) {
            break;
        }
        path.push(node.clone());
        node_id = pyval::get(node, "parent")?.cloned();
    }
    if !path.is_empty() {
        path.reverse();
        return Ok(path);
    }
    let mut nodes = Vec::new();
    for n in map.values().filter(|n| n.is_object()) {
        let msg = pyval::obj_or_empty(pyval::get(n, "message")?)?;
        let key = match msg.get("create_time") {
            Some(v) if pyval::truthy(v) => pyval::float(v).ok_or(Crash("ValueError"))?,
            _ => 0.0,
        };
        nodes.push((key, n.clone()));
    }
    nodes.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(Ordering::Equal));
    Ok(nodes.into_iter().map(|(_, n)| n).collect())
}

fn chatgpt_text(content: &serde_json::Map<String, Value>) -> Result<String, Error> {
    match content.get("content_type").and_then(Value::as_str) {
        Some("text" | "multimodal_text") => {
            let parts: Vec<String> = pyval::iter_or_empty(content.get("parts"))?
                .into_iter()
                .filter_map(|p| match p {
                    Value::String(s) => Some(s),
                    _ => None,
                })
                .collect();
            Ok(parts.join("\n"))
        }
        Some("code" | "execution_output") => Ok(pyval::str_or_empty(content.get("text"))),
        _ => Ok(String::new()),
    }
}

fn lt(a: &Option<DateTime>, b: &DateTime) -> Result<bool, Error> {
    match a {
        Some(a) => Ok(times::compare(a, b)? == Ordering::Less),
        None => Err(Crash("TypeError").into()),
    }
}

/// The ChatGPT reader (`chatgpt`).
pub fn chatgpt(files: &[(String, Vec<u8>)], now: &DateTime) -> Result<Vec<Conversation>, Error> {
    let mut out = Vec::new();
    let mut instructions: Vec<(String, Message)> = Vec::new();
    let mut snapshot: Option<Message> = None;
    let mut writes: Vec<Message> = Vec::new();
    for (name, data) in files {
        let mut convs = json(name, data)?;
        if let Value::Object(m) = &convs {
            if let Some(Value::Array(a)) = m.get("conversations") {
                convs = Value::Array(a.clone());
            }
        }
        let Value::Array(convs) = convs else {
            return Err(unreadable(format!(
                "{name} isn't ChatGPT's list of conversations"
            )));
        };
        for conv in &convs {
            if !conv.is_object() {
                continue;
            }
            let at_conv =
                unix(pyval::get(conv, "update_time")?).or_else(|| unix(conv.get("create_time")));
            let id = ["conversation_id", "id"]
                .iter()
                .filter_map(|k| conv.get(*k))
                .find(|v| pyval::truthy(v))
                .map_or(String::new(), pyval::to_str);
            let mut c = Conversation::new("chatgpt", id, pyval::str_or_empty(conv.get("title")));
            for node in chatgpt_path(conv)? {
                let msg = match pyval::get(&node, "message")? {
                    Some(v) if pyval::truthy(v) => v.clone(),
                    _ => Value::Object(Default::default()),
                };
                let Value::Object(msg) = msg else { continue };
                let meta = pyval::obj_or_empty(msg.get("metadata"))?;
                let content = pyval::obj_or_empty(msg.get("content"))?;
                let author = pyval::obj_or_empty(msg.get("author"))?;
                let role = author.get("role");
                let role_str = role.and_then(Value::as_str);
                let at = unix(msg.get("create_time")).or(at_conv);
                let about = match meta.get("user_context_message_data") {
                    Some(v) if pyval::truthy(v) => v.clone(),
                    _ => Value::Object(Default::default()),
                };
                let ctype = content.get("content_type").and_then(Value::as_str);
                if ctype == Some("model_editable_context") {
                    let text = pystr::strip(&pyval::str_or_empty(content.get("model_set_context")))
                        .to_owned();
                    if let Some(at) = at {
                        if !text.is_empty() {
                            let newer = match &snapshot {
                                None => true,
                                Some(s) => lt(&s.at.or(Some(at)), &at)?,
                            };
                            if newer {
                                snapshot = Some(Message::new("assistant", text, Some(at)));
                            }
                        }
                    }
                    continue;
                }
                if role_str == Some("assistant")
                    && msg.get("recipient").and_then(Value::as_str) == Some("bio")
                {
                    let text = chatgpt_text(content)?;
                    if at.is_some() && !pystr::strip(&text).is_empty() {
                        writes.push(Message::new("assistant", text, at));
                    }
                    continue;
                }
                let about_flag = match &about {
                    Value::Object(a) => {
                        a.get("about_user_message").is_some_and(pyval::truthy)
                            || a.get("about_model_message").is_some_and(pyval::truthy)
                    }
                    _ => false,
                };
                if ctype == Some("user_editable_context") || about_flag {
                    let mine = ["user_profile", "user_instructions"]
                        .iter()
                        .map(|k| pyval::str_or_empty(content.get(*k)))
                        .collect::<Vec<_>>()
                        .join("\n");
                    let mut text = pystr::strip(&mine).to_owned();
                    if text.is_empty() {
                        let about_obj = pyval::obj_or_empty(Some(&about))?;
                        let theirs = ["about_user_message", "about_model_message"]
                            .iter()
                            .map(|k| pyval::str_or_empty(about_obj.get(*k)))
                            .collect::<Vec<_>>()
                            .join("\n");
                        text = pystr::strip(&theirs).to_owned();
                    }
                    if let Some(at) = at {
                        if !text.is_empty() {
                            match instructions.iter_mut().find(|(k, _)| *k == text) {
                                Some((_, m)) => {
                                    if lt(&m.at, &at)? {
                                        *m = Message::new("owner", text.clone(), Some(at));
                                    }
                                }
                                None => instructions
                                    .push((text.clone(), Message::new("owner", text, Some(at)))),
                            }
                        }
                    }
                    continue;
                }
                let hidden = meta
                    .get("is_visually_hidden_from_conversation")
                    .is_some_and(pyval::truthy);
                let mapped = match role_str {
                    Some("user") => "owner",
                    Some("assistant") => "assistant",
                    Some("tool") => "tool",
                    _ => continue,
                };
                if hidden {
                    continue;
                }
                let text = chatgpt_text(content)?;
                let role_ = if mapped == "assistant" && ctype == Some("execution_output") {
                    "tool"
                } else {
                    mapped
                };
                c.messages.push(Message::new(role_, text, at));
            }
            if !c.messages.is_empty() {
                out.push(c);
            }
        }
    }
    let by_time = |a: &Message, b: &Message| -> Ordering {
        times::instant(a.at.as_ref().unwrap_or(now))
            .cmp(&times::instant(b.at.as_ref().unwrap_or(now)))
    };
    if snapshot.is_some() || !writes.is_empty() {
        writes.sort_by(by_time);
        let mut msgs: Vec<Message> = snapshot.into_iter().collect();
        msgs.extend(writes);
        out.push(Conversation {
            source: "chatgpt".into(),
            id: "saved-memory".into(),
            title: "ChatGPT's saved memory".into(),
            messages: msgs,
            claims: true,
        });
    }
    if !instructions.is_empty() {
        let mut msgs: Vec<Message> = instructions.into_iter().map(|(_, m)| m).collect();
        msgs.sort_by(by_time);
        out.push(Conversation {
            source: "chatgpt".into(),
            id: "custom-instructions".into(),
            title: "ChatGPT custom instructions (about you)".into(),
            messages: msgs,
            claims: false,
        });
    }
    Ok(out)
}

// ---------- claude.ai ----------

fn claude_text(msg: &Value) -> Result<String, Error> {
    if let Some(Value::Array(blocks)) = pyval::get(msg, "content")? {
        if !blocks.is_empty() {
            let text = blocks
                .iter()
                .filter(|b| b.get("type").and_then(Value::as_str) == Some("text"))
                .map(|b| pyval::str_or_empty(b.get("text")))
                .collect::<Vec<_>>()
                .join("\n");
            if !pystr::strip(&text).is_empty() {
                return Ok(text);
            }
        }
    }
    Ok(pyval::str_or_empty(msg.get("text")))
}

/// `dict(value or {})`'s values (a dict's, or a list of pairs').
fn dict_values(value: Option<&Value>) -> Result<Vec<Value>, Error> {
    match value {
        None => Ok(Vec::new()),
        Some(v) if !pyval::truthy(v) => Ok(Vec::new()),
        Some(Value::Object(m)) => Ok(m.values().cloned().collect()),
        Some(Value::Array(items)) => {
            let mut keys: Vec<String> = Vec::new();
            let mut values: Vec<Value> = Vec::new();
            for item in items {
                let (k, v) = match item {
                    Value::Array(pair) if pair.len() == 2 => {
                        if pair[0].is_array() || pair[0].is_object() {
                            return Err(Crash("TypeError").into());
                        }
                        (pyval::repr(&pair[0]), pair[1].clone())
                    }
                    Value::String(s) if s.chars().count() == 2 => {
                        let mut cs = s.chars();
                        let k = cs.next().map(String::from).unwrap_or_default();
                        let v = cs.next().map(String::from).unwrap_or_default();
                        (pystr::repr(&k), Value::String(v))
                    }
                    Value::Array(_) | Value::String(_) | Value::Object(_) => {
                        return Err(Crash("ValueError").into())
                    }
                    _ => return Err(Crash("TypeError").into()),
                };
                match keys.iter().position(|x| *x == k) {
                    Some(i) => values[i] = v,
                    None => {
                        keys.push(k);
                        values.push(v);
                    }
                }
            }
            Ok(values)
        }
        Some(Value::String(_)) => Err(Crash("ValueError").into()),
        Some(_) => Err(Crash("TypeError").into()),
    }
}

/// The claude.ai reader (`claude`).
pub fn claude(files: &[(String, Vec<u8>)], now: &DateTime) -> Result<Vec<Conversation>, Error> {
    let mut out = Vec::new();
    for (name, data) in files {
        let base = pystr::lower(name.rsplit('/').next().unwrap_or(name));
        let value = json(name, data)?;
        let lname = pystr::lower(name);
        if base.starts_with("conversations") {
            let Value::Array(convs) = &value else {
                return Err(unreadable(format!(
                    "{name} isn't Claude's list of conversations"
                )));
            };
            for conv in convs {
                if !conv.is_object() {
                    continue;
                }
                let mut c = Conversation::new(
                    "claude",
                    pyval::str_or_empty(conv.get("uuid")),
                    pyval::str_or_empty(conv.get("name")),
                );
                for msg in pyval::iter_or_empty(conv.get("chat_messages"))? {
                    if !msg.is_object() {
                        continue;
                    }
                    let sender = msg.get("sender").map_or("None".to_owned(), pyval::to_str);
                    let role = match sender.as_str() {
                        "human" => "owner",
                        "assistant" => "assistant",
                        _ => continue,
                    };
                    let mut text = claude_text(&msg)?;
                    let attached = msg.get("attachments").is_some_and(pyval::truthy)
                        || msg.get("files").is_some_and(pyval::truthy);
                    if role == "owner" && attached {
                        text.push_str("\n[attached file, not read]");
                    }
                    c.messages
                        .push(Message::new(role, text, iso(msg.get("created_at"))));
                }
                if !c.messages.is_empty() {
                    out.push(c);
                }
            }
        } else if lname.contains("projects") && (value.is_array() || value.is_object()) {
            let projects = match &value {
                Value::Array(a) => a.clone(),
                v => vec![v.clone()],
            };
            for project in &projects {
                if !project.is_object() {
                    continue;
                }
                let at = iso(project.get("updated_at")).or_else(|| iso(project.get("created_at")));
                let joined = ["description", "prompt_template"]
                    .iter()
                    .map(|k| pyval::str_or_empty(project.get(*k)))
                    .collect::<Vec<_>>()
                    .join("\n");
                let text = pystr::strip(&joined);
                if !text.is_empty() {
                    let mut c = Conversation::new(
                        "claude",
                        format!("project-{}", pyval::str_or_empty(project.get("uuid"))),
                        format!(
                            "Claude project: {}",
                            pyval::str_or_empty(project.get("name"))
                        ),
                    );
                    c.messages.push(Message::new("owner", text, at));
                    out.push(c);
                }
            }
        } else if lname.contains("memor") {
            let mut claims = Vec::new();
            let records = match &value {
                Value::Array(a) => a.clone(),
                v => vec![v.clone()],
            };
            for record in &records {
                if !record.is_object() {
                    continue;
                }
                let mut texts = vec![record
                    .get("conversations_memory")
                    .cloned()
                    .unwrap_or(Value::Null)];
                texts.extend(dict_values(record.get("project_memories"))?);
                for t in texts {
                    if let Value::String(s) = t {
                        if !pystr::strip(&s).is_empty() {
                            claims.push(Message::new("assistant", s, Some(*now)));
                        }
                    }
                }
                for f in pyval::iter_or_empty(record.get("memory_files"))? {
                    if let Some(Value::String(content)) = f.get("content") {
                        if f.is_object() && !pystr::strip(content).is_empty() {
                            let at = iso(f.get("updated_at")).unwrap_or(*now);
                            claims.push(Message::new("assistant", content.clone(), Some(at)));
                        }
                    }
                }
            }
            if !claims.is_empty() {
                out.push(Conversation {
                    source: "claude".into(),
                    id: format!("memory-{}", name.rsplit('/').next().unwrap_or(name)),
                    title: "Claude's saved memory".into(),
                    messages: claims,
                    claims: true,
                });
            }
        }
    }
    Ok(out)
}

// ---------- Gemini ----------

/// Takeout's MyActivity.html: each `outer-cell` holds the text pieces of its content cells.
#[derive(Default)]
struct ActivityHtml {
    cells: Vec<Vec<String>>,
    depth: i64,
    outer: Option<i64>,
}

impl Handler for ActivityHtml {
    fn start(&mut self, tag: &str, attrs: &[(String, Option<String>)]) {
        if tag == "div" {
            self.depth += 1;
            let class = attrs
                .iter()
                .rev()
                .find(|(k, _)| k == "class")
                .and_then(|(_, v)| v.clone())
                .unwrap_or_default();
            if pystr::split_ws(&class).any(|c| c == "outer-cell") {
                self.outer = Some(self.depth);
                self.cells.push(Vec::new());
            }
        } else if (tag == "br" || tag == "p") && self.outer.is_some() {
            if let Some(cell) = self.cells.last_mut() {
                cell.push("\n".into());
            }
        }
    }

    fn end(&mut self, tag: &str) {
        if tag == "div" {
            if self.outer == Some(self.depth) {
                self.outer = None;
            }
            self.depth -= 1;
        }
    }

    fn data(&mut self, data: &str) {
        if self.outer.is_some() {
            if let Some(cell) = self.cells.last_mut() {
                cell.push(data.to_owned());
            }
        }
    }
}

#[derive(Default)]
struct TextParts {
    parts: Vec<String>,
}

impl Handler for TextParts {
    fn start(&mut self, tag: &str, _attrs: &[(String, Option<String>)]) {
        if matches!(tag, "br" | "p" | "li" | "div") {
            self.parts.push("\n".into());
        }
    }

    fn end(&mut self, _tag: &str) {}

    fn data(&mut self, data: &str) {
        self.parts.push(data.to_owned());
    }
}

/// The text pieces `_Text` collects from `text` (fed, then closed when `close`).
pub fn text_parts(text: &str, close: bool) -> Vec<String> {
    let mut p = TextParts::default();
    html::parse(text, &mut p, close);
    p.parts
}

/// The outer cells `_ActivityHTML` collects from `text`.
pub fn activity_cells(text: &str, close: bool) -> Vec<Vec<String>> {
    let mut p = ActivityHtml::default();
    html::parse(text, &mut p, close);
    p.cells
}

/// `re.sub(r"\n\s*\n+", "\n", text)`.
fn fold_blank_lines(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '\n' {
            let mut e = i + 1;
            while e < chars.len() && pystr::is_space(chars[e]) {
                e += 1;
            }
            if let Some(last) = (i + 1..e).rev().find(|&p| chars[p] == '\n') {
                out.push('\n');
                i = last + 1;
                continue;
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

/// `_strip_html` (with the parser closed: the Rust reader's fix).
pub fn strip_html(text: &str) -> String {
    strip_html_with(text, true)
}

/// `_strip_html`, the parser closed or (as Python leaves it) not.
pub fn strip_html_with(text: &str, close: bool) -> String {
    pystr::strip(&fold_blank_lines(&text_parts(text, close).concat())).to_owned()
}

/// `_prompt`: the title after Takeout's English "Prompted" marker, else `None`.
pub fn prompt(title: &str) -> Option<String> {
    let t = pystr::strip(title);
    let (head, rest) = t.split_once(' ').unwrap_or((t, ""));
    let rest = pystr::strip(rest);
    (head == "Prompted" && !rest.is_empty()).then(|| rest.to_owned())
}

/// `_HTML_TIME.search(text)`: (group 1, group 2, end), in characters of `chars`.
fn html_time(chars: &[char]) -> Option<(String, Option<String>, usize)> {
    let n = chars.len();
    let dec = |i: usize| chars.get(i).copied().and_then(pystr::decimal).is_some();
    let lit = |i: usize, s: &str| -> Option<usize> {
        let mut j = i;
        for c in s.chars() {
            if chars.get(j) != Some(&c) {
                return None;
            }
            j += 1;
        }
        Some(j)
    };
    // \d{a,b} followed by what `next` accepts (backtracking over the count)
    let digits = |i: usize, lo: usize, hi: usize| -> Vec<usize> {
        let mut run = 0;
        while run < hi && dec(i + run) {
            run += 1;
        }
        (lo..=run).rev().map(|k| i + k).collect()
    };
    for start in 0..n {
        if !chars[start].is_ascii_uppercase() {
            continue;
        }
        let try_at = || -> Option<(usize, usize)> {
            let mut lowers = 0;
            while lowers < 8
                && chars
                    .get(start + 1 + lowers)
                    .is_some_and(char::is_ascii_lowercase)
            {
                lowers += 1;
            }
            for l in (2..=lowers).rev() {
                let Some(p) = lit(start + 1 + l, " ") else {
                    continue;
                };
                for p in digits(p, 1, 2) {
                    let Some(p) = lit(p, ", ") else { continue };
                    if digits(p, 4, 4).is_empty() {
                        continue;
                    }
                    let Some(p) = lit(p + 4, ", ") else { continue };
                    for p in digits(p, 1, 2) {
                        let Some(p) = lit(p, ":") else { continue };
                        if digits(p, 2, 2).is_empty() {
                            continue;
                        }
                        let Some(p) = lit(p + 2, ":") else { continue };
                        if digits(p, 2, 2).is_empty() {
                            continue;
                        }
                        let p = p + 2;
                        let mut options = Vec::new();
                        if chars.get(p).is_some_and(|&c| pystr::is_space(c)) {
                            options.push(p + 1);
                        }
                        options.push(p);
                        for q in options {
                            if matches!(chars.get(q), Some('A' | 'P'))
                                && chars.get(q + 1) == Some(&'M')
                            {
                                return Some((start, q + 2));
                            }
                        }
                    }
                }
            }
            None
        };
        let Some((s, e)) = try_at() else { continue };
        let group1: String = chars[s..e].iter().collect();
        // (?:\s+(GMT[+-]\d{1,2}(?::\d{2})?|[A-Z]{2,5}))?
        let mut w = e;
        while w < n && pystr::is_space(chars[w]) {
            w += 1;
        }
        let mut zone = None;
        if w > e {
            if lit(w, "GMT").is_some() && matches!(chars.get(w + 3), Some('+' | '-')) {
                let d = digits(w + 4, 1, 2);
                if let Some(&after) = d.first() {
                    let mut end = after;
                    if chars.get(after) == Some(&':') && !digits(after + 1, 2, 2).is_empty() {
                        end = after + 3;
                    }
                    zone = Some(end);
                }
            }
            if zone.is_none() {
                let mut k = 0;
                while k < 5 && chars.get(w + k).is_some_and(char::is_ascii_uppercase) {
                    k += 1;
                }
                if k >= 2 {
                    zone = Some(w + k);
                }
            }
        }
        return Some(match zone {
            Some(z) => (group1, Some(chars[w..z].iter().collect()), z),
            None => (group1, None, e),
        });
    }
    None
}

/// `datetime.strptime(text, "%b %d, %Y, %I:%M:%S %p")` (naive), `None` where Python raises `ValueError`.
fn strptime_takeout(text: &str) -> Option<DateTime> {
    const MONTHS: [&str; 12] = [
        "jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec",
    ];
    let c: Vec<char> = text.chars().collect();
    let month = MONTHS.iter().position(|m| {
        c.len() >= 3 && c[..3].iter().collect::<String>().to_ascii_lowercase() == *m
    })? as u32
        + 1;
    let mut i = 3;
    let ws = |i: &mut usize| -> bool {
        let s = *i;
        while c.get(*i).is_some_and(|&x| pystr::is_space(x)) {
            *i += 1;
        }
        *i > s
    };
    let dval = |x: char| pystr::decimal(x).map(i64::from);
    let ascii = |x: Option<&char>, set: &str| x.is_some_and(|ch| set.contains(*ch));
    if !ws(&mut i) {
        return None;
    }
    // %d: 3[01]|[12]\d|0[1-9]|[1-9]| [1-9], then ","
    let day = if c.get(i) == Some(&'3') && ascii(c.get(i + 1), "01") && c.get(i + 2) == Some(&',') {
        i += 2;
        30 + dval(c[i - 1])?
    } else if ascii(c.get(i), "12")
        && c.get(i + 1).copied().and_then(pystr::decimal).is_some()
        && c.get(i + 2) == Some(&',')
    {
        i += 2;
        dval(c[i - 2])? * 10 + dval(c[i - 1])?
    } else if c.get(i) == Some(&'0')
        && ascii(c.get(i + 1), "123456789")
        && c.get(i + 2) == Some(&',')
    {
        i += 2;
        dval(c[i - 1])?
    } else if ascii(c.get(i), "123456789") && c.get(i + 1) == Some(&',') {
        i += 1;
        dval(c[i - 1])?
    } else {
        return None;
    };
    if c.get(i) != Some(&',') {
        return None;
    }
    i += 1;
    if !ws(&mut i) {
        return None;
    }
    let mut year = 0;
    for k in 0..4 {
        year = year * 10 + dval(*c.get(i + k)?)?;
    }
    i += 4;
    if c.get(i) != Some(&',') {
        return None;
    }
    i += 1;
    if !ws(&mut i) {
        return None;
    }
    // %I: 1[0-2]|0[1-9]|[1-9], then ":"
    let hour = if c.get(i) == Some(&'1') && ascii(c.get(i + 1), "012") && c.get(i + 2) == Some(&':')
    {
        i += 2;
        10 + dval(c[i - 1])?
    } else if c.get(i) == Some(&'0')
        && ascii(c.get(i + 1), "123456789")
        && c.get(i + 2) == Some(&':')
    {
        i += 2;
        dval(c[i - 1])?
    } else if ascii(c.get(i), "123456789") && c.get(i + 1) == Some(&':') {
        i += 1;
        dval(c[i - 1])?
    } else {
        return None;
    };
    i += 1;
    // %M: [0-5]\d|\d, then ":"
    let minute = if ascii(c.get(i), "012345")
        && c.get(i + 1).copied().and_then(pystr::decimal).is_some()
        && c.get(i + 2) == Some(&':')
    {
        i += 2;
        dval(c[i - 2])? * 10 + dval(c[i - 1])?
    } else if c.get(i).copied().and_then(pystr::decimal).is_some() && c.get(i + 1) == Some(&':') {
        i += 1;
        dval(c[i - 1])?
    } else {
        return None;
    };
    i += 1;
    // %S: 6[0-1]|[0-5]\d|\d, then \s+
    let next_ws = |j: usize| c.get(j).is_some_and(|&x| pystr::is_space(x));
    let second = if c.get(i) == Some(&'6') && ascii(c.get(i + 1), "01") && next_ws(i + 2) {
        i += 2;
        60 + dval(c[i - 1])?
    } else if ascii(c.get(i), "012345")
        && c.get(i + 1).copied().and_then(pystr::decimal).is_some()
        && next_ws(i + 2)
    {
        i += 2;
        dval(c[i - 2])? * 10 + dval(c[i - 1])?
    } else if c.get(i).copied().and_then(pystr::decimal).is_some() && next_ws(i + 1) {
        i += 1;
        dval(c[i - 1])?
    } else {
        return None;
    };
    ws(&mut i);
    let ampm: String = c
        .get(i..i + 2)?
        .iter()
        .collect::<String>()
        .to_ascii_lowercase();
    if c.len() != i + 2 || !(ampm == "am" || ampm == "pm") {
        return None;
    }
    let mut hour = hour;
    if ampm == "pm" && hour != 12 {
        hour += 12;
    } else if ampm == "am" && hour == 12 {
        hour = 0;
    }
    DateTime::new(
        i32::try_from(year).ok()?,
        month,
        u32::try_from(day).ok()?,
        u32::try_from(hour).ok()?,
        u32::try_from(minute).ok()?,
        u32::try_from(second).ok()?,
        0,
        None,
    )
    .ok()
}

/// The Gemini reader (`gemini`): a day's prompts read as one conversation.
pub fn gemini(
    files: &[(String, Vec<u8>)],
    zone: &dyn LocalZone,
) -> Result<Vec<Conversation>, Error> {
    let mut entries: Vec<(DateTime, String, String)> = Vec::new();
    for (name, data) in files {
        if pystr::lower(name).ends_with(".json") {
            let Value::Array(records) = json(name, data)? else {
                return Err(unreadable(format!("{name} isn't Takeout's activity list")));
            };
            for r in &records {
                if !r.is_object() {
                    continue;
                }
                let mut p = prompt(&pyval::str_or_empty(r.get("title")));
                let at = iso(r.get("time"));
                let mut details: Vec<(String, String)> = Vec::new();
                for x in pyval::iter_or_empty(r.get("details"))? {
                    if x.is_object() {
                        let k = x.get("name").map_or("None".to_owned(), pyval::to_str);
                        let v = pyval::str_or_empty(x.get("value"));
                        match details.iter_mut().find(|(dk, _)| *dk == k) {
                            Some(slot) => slot.1 = v,
                            None => details.push((k, v)),
                        }
                    }
                }
                let detail = |k: &str| {
                    details
                        .iter()
                        .find(|(dk, _)| dk == k)
                        .map(|(_, v)| v.clone())
                };
                if p.is_none() {
                    if let Some(req) = detail("Request").filter(|s| !s.is_empty()) {
                        p = Some(req);
                    }
                }
                let (Some(p), Some(at)) = (p, at) else {
                    continue;
                };
                let items: Vec<String> = pyval::iter_or_empty(r.get("safeHtmlItem"))?
                    .iter()
                    .filter(|h| h.is_object())
                    .map(|h| strip_html(&pyval::str_or_empty(h.get("html"))))
                    .collect();
                let mut reply = items.join(" ");
                if reply.is_empty() {
                    reply = strip_html(&detail("Response").unwrap_or_default());
                }
                entries.push((at, html::unescape(&p), reply));
            }
        } else {
            let text = String::from_utf8_lossy(data);
            for cell in activity_cells(&text, true) {
                let text = html::unescape(&cell.concat()).replace('\u{a0}', " ");
                let chars: Vec<char> = text.chars().collect();
                let m = html_time(&chars);
                let lines: Vec<&str> = text
                    .split('\n')
                    .map(pystr::strip)
                    .filter(|l| !l.is_empty())
                    .collect();
                let Some((group1, group2, end)) = m else {
                    continue;
                };
                if lines.is_empty() {
                    continue;
                }
                let Some(p) = lines.iter().find_map(|l| prompt(l)) else {
                    continue;
                };
                let Some(at) = strptime_takeout(&group1.replace('\u{202f}', " ")) else {
                    continue;
                };
                let at = if matches!(group2.as_deref(), Some("UTC" | "GMT")) {
                    DateTime {
                        offset: Some((0, 0)),
                        ..at
                    }
                } else {
                    times::astimezone(&at, zone).ok_or(Crash("OverflowError"))?
                };
                let reply = pystr::strip(&chars[end..].iter().collect::<String>()).to_owned();
                entries.push((at, p, reply));
            }
        }
    }
    entries.sort_by(|a, b| {
        times::instant(&a.0)
            .cmp(&times::instant(&b.0))
            .then_with(|| a.1.cmp(&b.1))
            .then_with(|| a.2.cmp(&b.2))
    });
    let mut by_day: Vec<Conversation> = Vec::new();
    for (at, p, reply) in entries {
        let local = times::astimezone(&at, zone).ok_or(Crash("OverflowError"))?;
        let day = times::ymd(&local);
        let id = format!("gemini-{day}");
        let idx = match by_day.iter().position(|c| c.id == id) {
            Some(i) => i,
            None => {
                by_day.push(Conversation::new(
                    "gemini",
                    id,
                    format!("Gemini prompts on {day}"),
                ));
                by_day.len() - 1
            }
        };
        let conv = &mut by_day[idx];
        conv.messages.push(Message::new("owner", p, Some(at)));
        if !reply.is_empty() {
            let later = times::plus(&at, 1_000_000).ok_or(Crash("OverflowError"))?;
            conv.messages
                .push(Message::new("assistant", reply, Some(later)));
        }
    }
    Ok(by_day)
}

// ---------- Claude Code ----------

fn cc_text(content: Option<&Value>) -> (String, bool) {
    match content {
        Some(Value::String(s)) => (s.clone(), false),
        Some(Value::Array(blocks)) => {
            if blocks
                .iter()
                .any(|b| b.get("type").and_then(Value::as_str) == Some("tool_result"))
            {
                return (String::new(), true);
            }
            let text = blocks
                .iter()
                .filter(|b| b.get("type").and_then(Value::as_str) == Some("text"))
                .map(|b| pyval::str_or_empty(b.get("text")))
                .collect::<Vec<_>>()
                .join("\n");
            (text, false)
        }
        _ => (String::new(), false),
    }
}

/// `PurePath.suffix`.
fn suffix(name: &str) -> &str {
    match name.rfind('.') {
        Some(i) if i > 0 && i < name.len() - 1 => &name[i..],
        _ => "",
    }
}

/// `PurePath.stem`.
fn stem(path: &Path) -> String {
    let name = name_of(path);
    match name.rfind('.') {
        Some(i) if i > 0 && i < name.len() - 1 => name[..i].to_owned(),
        _ => name,
    }
}

/// The Claude Code reader (`claude_code`).
pub fn claude_code(root: &Path) -> Result<Vec<Conversation>, Error> {
    if !root.is_dir() {
        return Err(unreadable(format!(
            "no Claude Code transcripts at {}",
            display(root)
        )));
    }
    let mut out = Vec::new();
    for path in sorted(glob(root, "*/*.jsonl")) {
        let id = stem(&path);
        if id.starts_with("agent-") {
            continue;
        }
        let mut c = Conversation::new("claude_code", id, "");
        let mut programmatic = false;
        for r in jsonl(&path, None) {
            let r = Value::Object(r);
            if pyval::str_or_empty(r.get("entrypoint")).starts_with("sdk") {
                programmatic = true;
                break;
            }
            let kind = r.get("type").and_then(Value::as_str);
            if kind == Some("ai-title") {
                if let Some(Value::String(t)) = r.get("aiTitle") {
                    c.title = t.clone();
                }
            }
            if !matches!(kind, Some("user" | "assistant"))
                || r.get("isSidechain").is_some_and(pyval::truthy)
                || r.get("isMeta").is_some_and(pyval::truthy)
            {
                continue;
            }
            let message = pyval::obj_or_empty(r.get("message"))?;
            let (text, tool) = cc_text(message.get("content"));
            let at = iso(r.get("timestamp"));
            if kind == Some("assistant") {
                c.messages.push(Message::new("assistant", text, at));
            } else if tool {
                c.messages.push(Message::new("tool", "", at));
            } else if !r.get("isCompactSummary").is_some_and(pyval::truthy)
                && r.get("turnOrigin")
                    .is_none_or(|v| v.as_str() == Some("human"))
            {
                c.messages
                    .push(Message::new("owner", owner_words(&text), at));
            }
        }
        if !programmatic && !c.messages.is_empty() {
            out.push(c);
        }
    }
    for note in sorted(glob(root, "*/memory/*.md")) {
        let Ok(meta) = fs::metadata(&note) else {
            continue;
        };
        let Ok(bytes) = fs::read(&note) else { continue };
        // os.stat's st_mtime: seconds + nanoseconds * 1e-9, as a float
        let mtime = meta
            .modified()
            .ok()
            .map(|t| match t.duration_since(std::time::UNIX_EPOCH) {
                Ok(d) => d.as_secs() as f64 + f64::from(d.subsec_nanos()) * 1e-9,
                Err(e) => {
                    -(e.duration().as_secs() as f64 + f64::from(e.duration().subsec_nanos()) * 1e-9)
                }
            });
        let Some(at) = mtime.and_then(times::from_timestamp_utc) else {
            continue;
        };
        let project = note
            .parent()
            .and_then(Path::parent)
            .map_or_else(String::new, name_of);
        let s = stem(&note);
        out.push(Conversation {
            source: "claude_code".into(),
            id: format!("memory-{project}-{s}"),
            title: format!("Claude Code memory note {s}"),
            messages: vec![Message::new(
                "assistant",
                String::from_utf8_lossy(&bytes)
                    .replace("\r\n", "\n")
                    .replace('\r', "\n"),
                Some(at),
            )],
            claims: true,
        });
    }
    Ok(out)
}

// ---------- Codex CLI ----------

/// The Codex CLI reader (`codex`): interactive sessions only.
pub fn codex(root: &Path, unzstd: Option<&dyn Unzstd>) -> Result<Vec<Conversation>, Error> {
    let found = |pattern: &str| -> Vec<PathBuf> {
        let mut v = glob(root, &format!("{pattern}.jsonl"));
        v.extend(glob(root, &format!("{pattern}.jsonl.zst")));
        sorted(v)
    };
    let mut files = found("sessions/**/rollout-*");
    files.extend(found("archived_sessions/**/rollout-*"));
    if files.is_empty() && root.is_dir() {
        files = found("**/rollout-*");
    }
    if !root.is_dir() {
        return Err(unreadable(format!(
            "no Codex sessions at {}",
            display(root)
        )));
    }
    let mut out = Vec::new();
    for path in files {
        let name = name_of(&path);
        let id = name.strip_suffix(".zst").unwrap_or(&name);
        let id = id.strip_suffix(".jsonl").unwrap_or(id);
        let id = id.strip_prefix("rollout-").unwrap_or(id);
        let mut c = Conversation::new("codex", id, "");
        let mut interactive: Option<bool> = None;
        for r in jsonl(&path, unzstd) {
            let r = Value::Object(r);
            let payload = match r.get("payload") {
                Some(Value::Object(p)) => Value::Object(p.clone()),
                _ => Value::Object(Default::default()),
            };
            let kind = r.get("type").and_then(Value::as_str);
            if kind == Some("session_meta") {
                if interactive.is_none() {
                    let source = payload.get("source").and_then(Value::as_str);
                    interactive = Some(source.is_some_and(|s| CODEX_INTERACTIVE.contains(&s)));
                    if let Some(v) = payload.get("id").filter(|v| pyval::truthy(v)) {
                        c.id = pyval::to_str(v);
                    }
                }
                continue;
            }
            if kind != Some("response_item")
                || payload.get("type").and_then(Value::as_str) != Some("message")
            {
                continue;
            }
            let role = payload.get("role").and_then(Value::as_str);
            let text = pyval::iter_or_empty(payload.get("content"))?
                .iter()
                .filter(|b| {
                    matches!(
                        b.get("type").and_then(Value::as_str),
                        Some("input_text" | "output_text")
                    )
                })
                .map(|b| pyval::str_or_empty(b.get("text")))
                .collect::<Vec<_>>()
                .join("\n");
            let at = iso(r.get("timestamp"));
            if role == Some("assistant") {
                c.messages.push(Message::new("assistant", text, at));
            } else if role == Some("user") {
                let meta = pyval::obj_or_empty(r.get("metadata"))?;
                if meta
                    .get("inherited_user_message")
                    .is_some_and(pyval::truthy)
                {
                    continue;
                }
                if pystr::lstrip(&text).starts_with("# AGENTS.md instructions") {
                    continue;
                }
                c.messages
                    .push(Message::new("owner", owner_words(&text), at));
            }
        }
        if interactive == Some(true) && !c.messages.is_empty() {
            out.push(c);
        }
    }
    Ok(out)
}
