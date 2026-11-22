//! A site's data export, checked before and after FERB's reader with plain words for the ways it goes wrong
//! (MEM-438): an unfinished download, an empty file, a web page saved where a zip should be (an expired link), a
//! damaged zip, another site's export, an export with nothing from the window. And claude.ai's export as it comes
//! since late 2026: the mail brings a small `manifest-….json` listing one link per category, each downloading one
//! zip (`conversations-000.zip`, `memories-000.zip`, `projects-000.zip` and others the import doesn't use). Exactly
//! the parts the manifest names for conversations, memories and projects are read, never whatever else lies in the
//! folder; a part not downloaded is named, with whether its link (24 hours) has expired.
//!
//! The readers themselves are unchanged (`import_sources`); these checks run around them.

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::import_sources::{self, display, files, name_of, source_name, Conversation};
use crate::times::{self, DateTime};
use crate::{zip, Error};

/// Where each site's export is requested (said with every message that sends the owner there).
pub fn where_(source: &str) -> &'static str {
    match source {
        "chatgpt" => "ChatGPT > Settings > Data controls > Export data (the link comes by mail and stops working after a day)",
        "claude" => "claude.ai > Settings > Privacy > Export data (the links come by mail and stop working after a day)",
        "gemini" => "takeout.google.com > deselect all > My Activity > only Gemini Apps (the file comes by mail or to Drive)",
        _ => "",
    }
}

/// A download a browser hasn't finished.
pub const UNFINISHED: [&str; 5] = [".crdownload", ".part", ".download", ".partial", ".tmp"];
/// The claude.ai export's categories the import reads; the others (frames: the owner's artifacts; light_metadata:
/// login history and invoices; feedback: the site's own text) are never needed (IMP-32).
pub const NEEDED: [&str; 3] = ["conversations", "memories", "projects"];
/// How long a claude.ai manifest's links work.
pub const LINK_HOURS: i64 = 24;

fn unusable(msg: String) -> Error {
    Error::Unreadable(msg)
}

fn label(source: &str) -> &str {
    source_name(source).unwrap_or(source)
}

fn suffix_lower(path: &Path) -> String {
    let name = name_of(path).to_lowercase();
    match name.rfind('.') {
        Some(i) if i > 0 => name[i..].to_owned(),
        _ => String::new(),
    }
}

/// Stops with plain words when `path` plainly isn't a usable export of `source`.
pub fn check(source: &str, path: &Path) -> Result<(), Error> {
    let l = label(source);
    if !path.exists() {
        return Err(unusable(format!(
            "{} doesn't exist. {l}'s export: {}.",
            display(path),
            where_(source)
        )));
    }
    if !path.is_file() {
        return Ok(());
    }
    let name = name_of(path);
    if UNFINISHED.contains(&suffix_lower(path).as_str()) {
        return Err(unusable(format!(
            "{name} is a download that hasn't finished. Wait for it to finish, or download {l}'s export again."
        )));
    }
    let data = fs::read(path)?;
    if data.is_empty() {
        return Err(unusable(format!(
            "{name} is empty. Download {l}'s export again."
        )));
    }
    if suffix_lower(path) == ".zip" {
        if !zip::is_zip(&data) {
            return Err(unusable(format!(
                "{name} isn't a complete zip file: the download was cut short, or the export link had expired and a \
                 web page was saved instead. Ask {l} for a new export ({}).",
                where_(source)
            )));
        }
        if zip::members(&data).is_err() {
            return Err(damaged(&name, l));
        }
    }
    Ok(())
}

fn damaged(name: &str, l: &str) -> Error {
    unusable(format!(
        "{name} is damaged (part of it can't be read): download {l}'s export again."
    ))
}

/// The manifest at `path` (the file itself, or the newest `manifest-*.json` in a folder): its folder, file name and
/// value.
pub fn manifest(path: &Path) -> Option<(PathBuf, String, Value)> {
    let candidates: Vec<PathBuf> = if path.is_file() && suffix_lower(path) == ".json" {
        vec![path.to_path_buf()]
    } else if path.is_dir() {
        let mut found: Vec<(std::time::SystemTime, PathBuf)> = fs::read_dir(path)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .filter(|p| {
                let n = name_of(p).to_lowercase();
                n.starts_with("manifest-") && n.ends_with(".json") && p.is_file()
            })
            .map(|p| {
                (
                    fs::metadata(&p)
                        .and_then(|m| m.modified())
                        .unwrap_or(std::time::UNIX_EPOCH),
                    p,
                )
            })
            .collect();
        found.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
        found.into_iter().map(|(_, p)| p).collect()
    } else {
        Vec::new()
    };
    for file in candidates {
        let Ok(bytes) = fs::read(&file) else { continue };
        let bytes = bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(&bytes);
        let Ok(value) = serde_json::from_slice::<Value>(bytes) else {
            continue;
        };
        if value.get("data_files").is_some_and(Value::is_array) {
            let folder = file.parent().map(Path::to_path_buf).unwrap_or_default();
            return Some((folder, name_of(&file), value));
        }
    }
    None
}

/// The part zips of a claude.ai export that came as a manifest or as loose part zips: exactly the files the manifest
/// names for the categories the import reads. `None` for an export of the older kind (one zip, or its unpacked
/// folder).
pub fn claude_parts(path: &Path, now: &DateTime) -> Result<Option<Vec<PathBuf>>, Error> {
    let wanted: Vec<(String, PathBuf)> = match manifest(path) {
        None => {
            if !path.is_dir() || path.join("conversations.json").exists() {
                return Ok(None);
            }
            let mut loose: Vec<(String, PathBuf)> = fs::read_dir(path)
                .into_iter()
                .flatten()
                .flatten()
                .map(|e| e.path())
                .filter_map(|p| {
                    let n = name_of(&p);
                    let (cat, rest) = n.split_once('-')?;
                    let digits = rest.strip_suffix(".zip")?;
                    (NEEDED.contains(&cat)
                        && digits.len() == 3
                        && digits.bytes().all(|b| b.is_ascii_digit()))
                    .then(|| (cat.to_owned(), p))
                })
                .collect();
            loose.sort_by(|a, b| a.1.cmp(&b.1));
            if !loose.iter().any(|(c, _)| c == "conversations") {
                return Ok(None);
            }
            loose
        }
        Some((folder, name, value)) => {
            let wanted: Vec<(String, PathBuf)> = value["data_files"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|d| {
                    let cat = d.get("category")?.as_str()?;
                    let file = d.get("filename")?.as_str()?;
                    let file = Path::new(file).file_name()?.to_string_lossy().into_owned();
                    (NEEDED.contains(&cat) && !file.is_empty())
                        .then(|| (cat.to_owned(), folder.join(file)))
                })
                .collect();
            if !wanted.iter().any(|(c, _)| c == "conversations") {
                return Err(unusable(format!(
                    "{name} lists no conversations file: it isn't a complete claude.ai export. Ask for a new one ({}).",
                    where_("claude")
                )));
            }
            let missing: Vec<String> = wanted
                .iter()
                .filter(|(_, p)| !p.exists())
                .map(|(_, p)| name_of(p))
                .collect();
            if !missing.is_empty() {
                let made = import_sources::iso(value.get("created_at"));
                let hours = made
                    .map(|m| (times::instant(now) - times::instant(&m)) / 3_600_000_000)
                    .unwrap_or(0);
                if made.is_some() && hours >= LINK_HOURS {
                    return Err(unusable(format!(
                        "claude.ai's export came as a list of links ({name}) and {} of its parts were never \
                         downloaded: {}. The links stop working {LINK_HOURS} hours after they're made, so they have \
                         expired: ask for a new export ({}) and download every part when the mail arrives.",
                        missing.len(),
                        missing.join(", "),
                        where_("claude")
                    )));
                }
                let left = if made.is_some() {
                    format!(
                        "; the links stop working in about {} hours",
                        LINK_HOURS - hours
                    )
                } else {
                    String::new()
                };
                return Err(unusable(format!(
                    "claude.ai's export came as a list of links ({name}) and {} of its parts aren't downloaded yet: \
                     {}. Open each of those links in the browser signed in to claude.ai, let each download finish, \
                     and keep the files beside the manifest{left}. A link may work only once.",
                    missing.len(),
                    missing.join(", ")
                )));
            }
            wanted
        }
    };
    for (_, part) in &wanted {
        check("claude", part)?;
    }
    Ok(Some(wanted.into_iter().map(|(_, p)| p).collect()))
}

/// What a conversation of each site's export holds that the others' don't.
const SHAPE: [(&str, &str); 2] = [("chatgpt", "mapping"), ("claude", "chat_messages")];

/// Another site whose export this file is, judged by what its first conversation holds.
pub fn looks_like(path: &Path, source: &str) -> Option<&'static str> {
    let found = files(path, &["conversations"], &[".json"]).ok()?;
    let (_, data) = found.first()?;
    let data = data.strip_prefix(b"\xef\xbb\xbf").unwrap_or(data);
    let value: Value = serde_json::from_slice(data).ok()?;
    let list = match &value {
        Value::Object(m) => m.get("conversations").cloned().unwrap_or(Value::Null),
        v => v.clone(),
    };
    let first = list.as_array()?.first()?.as_object()?.clone();
    SHAPE
        .iter()
        .find(|(s, field)| *s != source && first.contains_key(*field))
        .map(|(s, _)| *s)
}

/// Why an export gave nothing from the window: another site's export, or nothing recent.
pub fn nothing(source: &str, path: &Path) -> Error {
    let l = label(source);
    let name = name_of(path);
    if let Some(other) = looks_like(path, source) {
        return unusable(format!(
            "{name} looks like {}'s export, not {l}'s.",
            label(other)
        ));
    }
    unusable(format!(
        "{name} holds nothing from the last month that {l}'s export would hold. Is it {l}'s export, and a recent \
         one? ({})",
        where_(source)
    ))
}

/// A damaged zip met while reading (`BadZipFile`, a cut-short member) said plainly.
pub fn read_error(source: &str, path: &Path, e: Error) -> Error {
    match e {
        Error::Crash("BadZipFile" | "EOFError" | "error") => damaged(&name_of(path), label(source)),
        other => other,
    }
}

/// The newest owner or assistant message of an export's conversations (its saved memories aside): how recent the
/// export is, kept so an older export says so (MEM-438).
pub fn newest(conversations: &[Conversation]) -> Option<DateTime> {
    conversations
        .iter()
        .filter(|c| !c.claims)
        .filter_map(|c| c.last().copied())
        .max_by_key(times::instant)
}
