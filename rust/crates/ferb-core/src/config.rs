//! config.toml: parsing and persistence (ferb/config.py). Never infers intent or changes an owner's value silently.
//!
//! - Refuses a config (or FERB's home) that others can read or write: allowed origins and policy live here.
//! - Writes are atomic: a 0600 temp file in the same folder, fsync, rename over the original.
//! - Reads are cached (DMN-12): `get` parses config.toml once and again only when the file (or its or the home's
//!   owner or mode) changes, so an owner's edit while FERB runs applies at the next read, and a loosened mode is
//!   still refused.
//!
//! Python edits the file through tomlkit, which keeps the owner's comments and layout. Here `toml_edit` parses the
//! file and says where each table, key and value sits; an edit then changes only that part of the text, following
//! what tomlkit does for the same edit ([`Document`]). Shapes tomlkit handles in ways not reproduced here are
//! refused with [`ConfigError::Unsupported`] before anything is written.
//!
//! Seam: `ensure_privacy_defaults` needs the protection lists (ferb/policy/protection.py) and the excluded paths
//! (ferb/sense/exclusions.py); those ports pass them in as [`PrivacyDefaults`].

use std::collections::BTreeSet;
use std::io::Write;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};
use toml_edit::{ImDocument, Item, Table};

use crate::types::Dict;

/// config.py:23: `[privacy] exclude_apps` starts with password managers only; the owner may edit or empty it.
pub const DEFAULT_EXCLUDE_APPS: &[&str] = &[
    "com.1password.1password",
    "com.agilebits.onepassword7",
    "com.bitwarden.desktop",
    "com.apple.Passwords",
    "com.apple.keychainaccess",
];

/// config.py:26-57: the owner's decisions applied to config.toml once each, in this order, each journaled
/// (`config.upgraded`) by the daemon.
pub const LIMITS_DECIDED: &str = "download-limits-2026-10-09";
pub const DOWNLOADS_DECIDED: &str = "download-limits-2026-10-31"; // HAND-210, HAND-236
pub const FEEDBACK_DECIDED: &str = "feedback-relay-2026-11-01"; // DMN-8, DMN-31
pub const VOICE_DECIDED: &str = "no-voice-2026-11-04"; // VOICE-410
pub const TEST_MODE_DECIDED: &str = "no-test-mode-2026-11-10"; // DMN-582
pub const UPGRADES: &[&str] = &[
    LIMITS_DECIDED,
    DOWNLOADS_DECIDED,
    FEEDBACK_DECIDED,
    VOICE_DECIDED,
    TEST_MODE_DECIDED,
];

/// (section, key, FERB's old value, the decided value).
const FERB_WRITTEN: &[(&str, &str, i64, i64)] = &[
    ("browser", "download_max_mb", 100, 512),
    ("browser", "download_task_max_files", 20, 50),
    ("browser", "download_task_max_mb", 500, 4096),
    ("browser", "download_total_mb", 2048, 20480),
    ("browser", "task_max_tabs", 20, 50),
];
const RETIRED: &[(&str, &str)] = &[("retention", "download_days")];
const DOWNLOAD_FILES: (&str, i64, i64) = ("download_task_max_files", 50, 300);
/// (the MB key, its GB key, the 2026-10-09 value).
const DOWNLOAD_MB_TO_GB: &[(&str, &str, i64)] = &[
    ("download_task_max_mb", "download_task_max_gb", 4096),
    ("download_total_mb", "download_total_gb", 20480),
];
const DOWNLOAD_RETIRED: &[&str] = &["download_max_mb"];
const FEEDBACK_RETIRED: &[(&str, &str)] = &[("feedback", "address")];
const VOICE_RETIRED: &[(&str, &str)] = &[("voice", "tts"), ("voice", "voice"), ("voice", "rate")];
const TEST_MODE_RETIRED: &[(&str, &str)] = &[("test_mode", "state")];

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    /// config.py:60 `InsecureConfig`.
    #[error("{0}")]
    Insecure(String),
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("config.toml can't be read as TOML: {0}")]
    Parse(String),
    /// An edit whose result tomlkit shapes in a way this port doesn't reproduce; nothing was written.
    #[error("config.toml: {0}")]
    Unsupported(String),
    /// config.py:219.
    #[error("no config upgrade '{0}'")]
    NoUpgrade(String),
}

type Result<T> = std::result::Result<T, ConfigError>;

fn unsupported<T>(what: impl Into<String>) -> Result<T> {
    Err(ConfigError::Unsupported(what.into()))
}

/// config.py:64.
fn check_owner_only(path: &Path) -> Result<()> {
    let st = std::fs::metadata(path)?;
    // SAFETY: getuid has no preconditions and cannot fail.
    if st.uid() != unsafe { libc::getuid() } {
        return Err(ConfigError::Insecure(format!(
            "{} is not owned by this user",
            path.display()
        )));
    }
    let mode = st.mode() & 0o7777;
    if mode & 0o077 != 0 {
        return Err(ConfigError::Insecure(format!(
            "{} has mode 0o{mode:o}; others must have no access",
            path.display()
        )));
    }
    Ok(())
}

/// config.py:72: write `text` to `path` (inside `home`) atomically and owner-only: a 0600 temp file in the same
/// folder, fsync, rename over the original; `home` is made 0700. A failure removes the temp file.
pub fn write_private(home: &Path, path: &Path, text: &str) -> std::io::Result<()> {
    std::fs::create_dir_all(home)?;
    std::fs::set_permissions(home, std::fs::Permissions::from_mode(0o700))?;
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let suffix = path
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default();
    let mut tmp = tempfile::Builder::new()
        .prefix(&format!(".{stem}."))
        .suffix(&suffix)
        .tempfile_in(home)?;
    tmp.write_all(text.as_bytes())?;
    tmp.flush()?;
    tmp.as_file().sync_all()?;
    std::fs::set_permissions(tmp.path(), std::fs::Permissions::from_mode(0o600))?;
    tmp.persist(path).map_err(|e| e.error)?; // atomic on the same filesystem
    Ok(())
}

// ---------- values as tomlkit writes them ----------

fn basic_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{8}' => out.push_str("\\b"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\u{c}' => out.push_str("\\f"),
            '\r' => out.push_str("\\r"),
            c if (c as u32) < 0x20 || c as u32 == 0x7f => {
                out.push_str(&format!("\\u{:04x}", c as u32))
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// A value as tomlkit writes a new one: basic strings, Python's float text, arrays on one line.
fn encode(value: &Value) -> Result<String> {
    Ok(match value {
        Value::String(s) => basic_string(s),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) if n.is_f64() => {
            ferb_py::json::python_float(n.as_f64().unwrap_or_default())
        }
        Value::Number(n) => n.to_string(),
        Value::Array(items) => {
            let parts = items.iter().map(encode).collect::<Result<Vec<_>>>()?;
            format!("[{}]", parts.join(", "))
        }
        Value::Null => return unsupported("a setting can't be set to nothing"),
        Value::Object(_) => {
            return unsupported("writing a table as a setting's value is not ported")
        }
    })
}

fn encode_key(key: &str) -> String {
    let bare = !key.is_empty()
        && key
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');
    if bare {
        key.to_string()
    } else {
        basic_string(key)
    }
}

fn plain(item: &Item) -> Value {
    match item {
        Item::None => Value::Null,
        Item::Value(v) => plain_value(v),
        Item::Table(t) => Value::Object(t.iter().map(|(k, v)| (k.to_string(), plain(v))).collect()),
        Item::ArrayOfTables(a) => Value::Array(
            a.iter()
                .map(|t| Value::Object(t.iter().map(|(k, v)| (k.to_string(), plain(v))).collect()))
                .collect(),
        ),
    }
}

fn plain_value(value: &toml_edit::Value) -> Value {
    use toml_edit::Value as T;
    match value {
        T::String(s) => json!(s.value()),
        T::Integer(i) => json!(i.value()),
        T::Float(f) => json!(f.value()),
        T::Boolean(b) => json!(b.value()),
        T::Datetime(d) => json!(d.value().to_string()),
        T::Array(a) => Value::Array(a.iter().map(plain_value).collect()),
        T::InlineTable(t) => Value::Object(
            t.iter()
                .map(|(k, v)| (k.to_string(), plain_value(v)))
                .collect(),
        ),
    }
}

/// Python's `==` between a config value and a whole number FERB once wrote (100 == 100.0).
fn equals_int(value: Option<&Value>, n: i64) -> bool {
    match value {
        Some(Value::Number(v)) => {
            v.as_i64() == Some(n) || (v.is_f64() && v.as_f64() == Some(n as f64))
        }
        _ => false,
    }
}

// ---------- the document ----------

/// config.toml's text, for a caller that changes and saves it (Python: a tomlkit document). Each edit changes
/// only the text it must, as tomlkit does:
/// - a value is replaced where it stands (its comment stays);
/// - a new key goes after the last line of its table that isn't blank;
/// - a new table goes at the end, after one blank line;
/// - a removed key takes its whole line (or lines) with it;
/// - a removed table goes from its header to the next header.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Document {
    text: String,
}

struct Header {
    start: usize,
    /// Just past the header line's newline (or the end of the text).
    line_end: usize,
}

fn headers(text: &str, table: &Table, out: &mut Vec<Header>) {
    for (_, item) in table.iter() {
        let mut add = |t: &Table| {
            if let (false, Some(span)) = (t.is_dotted(), t.span()) {
                let line_end = text[span.start..]
                    .find('\n')
                    .map_or(text.len(), |i| span.start + i + 1);
                out.push(Header {
                    start: span.start,
                    line_end,
                });
            }
            headers(text, t, out);
        };
        match item {
            Item::Table(t) => add(t),
            Item::ArrayOfTables(a) => a.iter().for_each(add),
            _ => {}
        }
    }
}

/// Where a `[section]` written once with its own header sits: its header line and its body.
struct Section<'a> {
    table: &'a Table,
    header_start: usize,
    body: std::ops::Range<usize>,
}

impl Document {
    pub fn parse(text: &str) -> Result<Self> {
        let doc = Document {
            text: text.to_string(),
        };
        doc.parsed()?;
        Ok(doc)
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    fn parsed(&self) -> Result<ImDocument<&str>> {
        ImDocument::parse(self.text.as_str()).map_err(|e| ConfigError::Parse(e.to_string()))
    }

    /// The plain values (tomlkit's `unwrap()`). A TOML date or time is given as its text.
    pub fn values(&self) -> Result<Dict> {
        match plain(self.parsed()?.as_item()) {
            Value::Object(values) => Ok(values),
            _ => Ok(Dict::new()),
        }
    }

    fn value(&self, section: &str, key: &str) -> Result<Option<Value>> {
        Ok(self
            .values()?
            .get(section)
            .and_then(|t| t.get(key))
            .cloned())
    }

    /// The section's table when it can be edited here; Ok(None) when the file has no such key.
    fn section<'a>(&self, doc: &'a ImDocument<&str>, section: &str) -> Result<Option<Section<'a>>> {
        let table = match doc.as_table().get(section) {
            None => return Ok(None),
            Some(Item::Table(t)) if !t.is_implicit() && !t.is_dotted() && t.span().is_some() => t,
            Some(_) => {
                return unsupported(format!(
                    "[{section}] is not a table written with its own header"
                ))
            }
        };
        let mut all = Vec::new();
        headers(&self.text, doc.as_table(), &mut all);
        all.sort_by_key(|h| h.start);
        let start = table.span().map_or(0, |s| s.start);
        let at = all
            .iter()
            .position(|h| h.start == start)
            .expect("the table's own header");
        let end = all.get(at + 1).map_or(self.text.len(), |h| h.start);
        Ok(Some(Section {
            table,
            header_start: start,
            body: all[at].line_end..end,
        }))
    }

    fn splice(&mut self, range: std::ops::Range<usize>, with: &str) {
        self.text.replace_range(range, with);
    }

    /// config.py:151 `doc[section][key] = value`.
    pub fn set(&mut self, section: &str, key: &str, value: &Value) -> Result<()> {
        let written = encode(value)?;
        let doc = self.parsed()?;
        let Some(found) = self.section(&doc, section)? else {
            let before = &self.text;
            let gap = if before.is_empty() {
                ""
            } else if !before.ends_with('\n') {
                "\n"
            } else {
                let last_line = &before[..before.len() - 1];
                if last_line.is_empty() || last_line.ends_with('\n') || before.trim().is_empty() {
                    ""
                } else {
                    "\n"
                }
            };
            let added = format!(
                "{gap}[{}]\n{} = {written}\n",
                encode_key(section),
                encode_key(key)
            );
            self.text.push_str(&added);
            return Ok(());
        };
        match found.table.get(key) {
            Some(Item::Value(old)) => {
                let Some(span) = old.span() else {
                    return unsupported(format!("{section}.{key} can't be located"));
                };
                self.splice(span, &written);
            }
            Some(_) => return unsupported(format!("{section}.{key} is a table, not a value")),
            None => {
                let body = &self.text[found.body.clone()];
                let mut at = found.body.start;
                let mut offset = found.body.start;
                for line in body.split_inclusive('\n') {
                    offset += line.len();
                    if !line.trim().is_empty() {
                        at = offset;
                    }
                }
                let newline = if at > 0 && !self.text[..at].ends_with('\n') {
                    "\n"
                } else {
                    ""
                };
                self.splice(
                    at..at,
                    &format!("{newline}{} = {written}\n", encode_key(key)),
                );
            }
        }
        Ok(())
    }

    /// `del doc[section][key]`: false when the key (or its table) isn't there.
    pub fn remove(&mut self, section: &str, key: &str) -> Result<bool> {
        if self.value(section, key)?.is_none() {
            return Ok(false);
        }
        let doc = self.parsed()?;
        let Some(found) = self.section(&doc, section)? else {
            return Ok(false);
        };
        let Some((name, item)) = found.table.get_key_value(key) else {
            return Ok(false);
        };
        let (Some(key_span), Item::Value(value)) = (name.span(), item) else {
            return unsupported(format!("{section}.{key} is not a plain key with a value"));
        };
        let Some(value_span) = value.span() else {
            return unsupported(format!("{section}.{key} can't be located"));
        };
        let start = self.text[..key_span.start].rfind('\n').map_or(0, |i| i + 1);
        let end = self.text[value_span.end..]
            .find('\n')
            .map_or(self.text.len(), |i| value_span.end + i + 1);
        self.splice(start..end, "");
        Ok(true)
    }

    /// `del doc[section]` for a table with nothing left in it: from its header to the next header.
    fn remove_empty_table(&mut self, section: &str) -> Result<()> {
        let doc = self.parsed()?;
        if let Some(found) = self.section(&doc, section)? {
            if found.table.is_empty() {
                self.splice(found.header_start..found.body.end, "");
            }
        }
        Ok(())
    }

    /// `doc[section][key].extend(items)` on an array; the array's own layout (one line, or one item a line with or
    /// without a trailing comma) is kept.
    pub fn extend(&mut self, section: &str, key: &str, items: &[String]) -> Result<()> {
        if items.is_empty() {
            return Ok(());
        }
        let doc = self.parsed()?;
        let array = match self.section(&doc, section)?.and_then(|s| s.table.get(key)) {
            Some(Item::Value(toml_edit::Value::Array(a))) => a,
            _ => return unsupported(format!("{section}.{key} is not a list")),
        };
        let Some(span) = array.span() else {
            return unsupported(format!("{section}.{key} can't be located"));
        };
        let (open, close) = (span.start, span.end - 1);
        let items: Vec<String> = items.iter().map(|s| basic_string(s)).collect();
        let multiline = self.text[open..close].contains('\n');
        let lines = |indent: &str| {
            items
                .iter()
                .map(|i| format!("\n{indent}{i}"))
                .collect::<Vec<_>>()
                .join(",")
        };
        let last = array.iter().last().and_then(toml_edit::Value::span);
        let replaced = match (last, multiline) {
            (None, false) => (open + 1..open + 1, items.join(", ")),
            (None, true) => {
                let body = &self.text[open + 1..close];
                let cut = body.rfind('\n').unwrap_or(0);
                let comment = body[..cut]
                    .rsplit('\n')
                    .next()
                    .filter(|l| l.trim_start().starts_with('#'));
                let indent = comment.map_or("    ", |l| &l[..l.len() - l.trim_start().len()]);
                (open + 1 + cut..open + 1 + cut, lines(indent))
            }
            (Some(last), false) => {
                let after = self.text[last.end..close].replace(',', "");
                (last.end..close, format!(", {}{after}", items.join(", ")))
            }
            (Some(last), true) => {
                let line_start = self.text[..last.start].rfind('\n').map_or(0, |i| i + 1);
                let indent = &self.text[line_start..last.start];
                if !indent.trim().is_empty() {
                    (last.end..last.end, format!(", {}", items.join(", "))) // items share a line
                } else {
                    let after = &self.text[last.end..close];
                    let (same_line, rest) = after.split_at(after.find('\n').unwrap_or(after.len()));
                    let comma = same_line
                        .split('#')
                        .next()
                        .unwrap_or_default()
                        .contains(',');
                    let same_line = if comma {
                        same_line.replacen(',', "", 1)
                    } else {
                        same_line.to_string()
                    };
                    let new = if rest.contains('#') {
                        // comment lines follow the last item: the new items go after them
                        let cut = rest.rfind('\n').unwrap_or(0);
                        format!(
                            ",{same_line}{}{}{}",
                            &rest[..cut],
                            lines(indent),
                            &rest[cut..]
                        )
                    } else {
                        format!(
                            ",{same_line}{}{}{rest}",
                            lines(indent),
                            if comma { "," } else { "" }
                        )
                    };
                    (last.end..close, new)
                }
            }
        };
        self.splice(replaced.0, &replaced.1);
        Ok(())
    }
}

// ---------- the config ----------

/// The file's identity, size, change times and owner/mode, and the home's owner/mode (config.py:100).
type Stamp = [u64; 8];

/// The lists `ensure_privacy_defaults` writes that belong to later ports (config.py:277-278).
#[derive(Debug, Clone, Default)]
pub struct PrivacyDefaults {
    /// ferb/policy/protection.py DEFAULT_NEVER_READ
    pub never_read: Vec<String>,
    /// ferb/policy/protection.py DEFAULT_NEVER_WRITE
    pub never_write: Vec<String>,
    /// ferb/policy/protection.py DEFAULT_TERMINAL_APPS
    pub terminal_apps: Vec<String>,
    /// ferb/sense/exclusions.py DEFAULT_EXCLUDE_PATHS
    pub exclude_paths: Vec<String>,
}

pub struct Config {
    pub home: PathBuf,
    pub path: PathBuf,
    /// What the file and the home looked like when parsed, and the parsed values: `get` re-parses only on a change.
    cached: Mutex<Option<(Stamp, Arc<Dict>)>>,
    parses: AtomicU64,
}

fn change(key: String, from: Option<Value>, to: Value) -> Value {
    let mut row = Dict::new();
    row.insert("key".to_string(), json!(key));
    if let Some(from) = from {
        row.insert("from".to_string(), from);
    }
    row.insert("to".to_string(), to);
    Value::Object(row)
}

impl Config {
    pub fn new(home: &Path) -> Self {
        Config {
            home: home.to_path_buf(),
            path: home.join("config.toml"),
            cached: Mutex::new(None),
            parses: AtomicU64::new(0),
        }
    }

    /// How many times the file was read and parsed (DMN-12: once, and again only when it changes).
    pub fn parses(&self) -> u64 {
        self.parses.load(Ordering::Relaxed)
    }

    fn stamp(&self) -> Result<Option<Stamp>> {
        let st = match std::fs::metadata(&self.path) {
            Ok(st) => st,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        let home = std::fs::metadata(&self.home)?;
        let nanos = |s: i64, n: i64| {
            (s as u64)
                .wrapping_mul(1_000_000_000)
                .wrapping_add(n as u64)
        };
        Ok(Some([
            st.ino(),
            st.size(),
            nanos(st.mtime(), st.mtime_nsec()),
            nanos(st.ctime(), st.ctime_nsec()),
            u64::from(st.uid()),
            u64::from(st.mode()),
            u64::from(home.uid()),
            u64::from(home.mode()),
        ]))
    }

    fn values(&self) -> Result<Arc<Dict>> {
        let Some(stamp) = self.stamp()? else {
            return Ok(Arc::new(Dict::new()));
        };
        if let Some((seen, values)) = self.cached.lock().expect("config cache").as_ref() {
            if *seen == stamp {
                return Ok(values.clone());
            }
        }
        let values = Arc::new(self.load()?.values()?); // checks the owner and mode, then parses
        *self.cached.lock().expect("config cache") = Some((stamp, values.clone()));
        Ok(values)
    }

    /// config.py:126: a freshly parsed document, for a caller that changes and saves it.
    pub fn load(&self) -> Result<Document> {
        if !self.path.exists() {
            return Ok(Document::default());
        }
        check_owner_only(&self.home)?;
        check_owner_only(&self.path)?;
        self.parses.fetch_add(1, Ordering::Relaxed);
        // Python reads the file in text mode: every \r\n and lone \r arrives as \n, and is saved that way
        Document::parse(
            &std::fs::read_to_string(&self.path)?
                .replace("\r\n", "\n")
                .replace('\r', "\n"),
        )
    }

    /// config.py:134.
    pub fn save(&self, doc: &Document) -> Result<()> {
        let values = doc.values()?;
        write_private(&self.home, &self.path, doc.text())?;
        *self.cached.lock().expect("config cache") =
            self.stamp()?.map(|stamp| (stamp, Arc::new(values)));
        Ok(())
    }

    /// config.py:138: a plain value; None when the table or the key isn't there.
    pub fn get(&self, section: &str, key: &str) -> Result<Option<Value>> {
        Ok(self
            .values()?
            .get(section)
            .and_then(Value::as_object)
            .and_then(|t| t.get(key))
            .cloned())
    }

    /// config.py:146: one table's plain values (empty when there is none).
    pub fn section(&self, section: &str) -> Result<Dict> {
        Ok(self
            .values()?
            .get(section)
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default())
    }

    /// config.py:151.
    pub fn set(&self, section: &str, key: &str, value: &Value) -> Result<()> {
        let mut doc = self.load()?;
        doc.set(section, key, value)?;
        self.save(&doc)
    }

    /// config.py:158: delete these keys of one table in one atomic write (nothing is written when none is there).
    pub fn remove(&self, section: &str, keys: &[&str]) -> Result<()> {
        let mut doc = self.load()?;
        let mut removed = false;
        for key in keys {
            removed |= doc.remove(section, key)?;
        }
        if removed {
            self.save(&doc)?;
        }
        Ok(())
    }

    /// config.py:169: the limits decision (LIMITS_DECIDED), all or nothing in one atomic write. Returns what
    /// changed, for the daemon to journal after the write.
    pub fn upgrade_ferb_written(&self) -> Result<Vec<Value>> {
        let mut doc = self.load()?;
        let mut changed = Vec::new();
        for (section, key, old, new) in FERB_WRITTEN {
            if equals_int(doc.value(section, key)?.as_ref(), *old) {
                doc.set(section, key, &json!(new))?;
                changed.push(change(
                    format!("{section}.{key}"),
                    Some(json!(old)),
                    json!(new),
                ));
            }
        }
        for (section, key) in RETIRED {
            if let Some(value) = doc.value(section, key)? {
                doc.remove(section, key)?;
                changed.push(change(format!("{section}.{key}"), Some(value), Value::Null));
            }
        }
        if !changed.is_empty() {
            self.save(&doc)?;
        }
        Ok(changed)
    }

    /// config.py:192: add to a list the owner keeps in config.toml each of `entries` it lacks, in one atomic write;
    /// the owner's own entries and order stay. Nothing is written when the list isn't there or isn't a list.
    pub fn add_defaults(&self, section: &str, key: &str, entries: &[&str]) -> Result<Vec<String>> {
        let mut doc = self.load()?;
        let Some(Value::Array(current)) = doc.value(section, key)? else {
            return Ok(Vec::new());
        };
        let added: Vec<String> = entries
            .iter()
            .filter(|e| !current.iter().any(|c| c == **e))
            .map(|e| e.to_string())
            .collect();
        if !added.is_empty() {
            doc.extend(section, key, &added)?;
            self.save(&doc)?;
        }
        Ok(added)
    }

    /// config.py:207: one of UPGRADES, by name (the daemon applies each once and journals what it changed).
    pub fn upgrade(&self, name: &str) -> Result<Vec<Value>> {
        match name {
            LIMITS_DECIDED => self.upgrade_ferb_written(),
            DOWNLOADS_DECIDED => self.upgrade_downloads(),
            FEEDBACK_DECIDED => self.retire(FEEDBACK_RETIRED),
            VOICE_DECIDED => self.retire(VOICE_RETIRED),
            TEST_MODE_DECIDED => self.retire(TEST_MODE_RETIRED),
            _ => Err(ConfigError::NoUpgrade(name.to_string())),
        }
    }

    /// config.py:221: remove keys no build reads any more, in one atomic write; a table left empty goes too. The
    /// old value is not returned: a retired key may have held an address.
    pub fn retire(&self, keys: &[(&str, &str)]) -> Result<Vec<Value>> {
        let mut doc = self.load()?;
        let mut changed = Vec::new();
        for (section, key) in keys {
            if doc.remove(section, key)? {
                changed.push(change(format!("{section}.{key}"), None, Value::Null));
                doc.remove_empty_table(section)?;
            }
        }
        if !changed.is_empty() {
            self.save(&doc)?;
        }
        Ok(changed)
    }

    /// config.py:237 (HAND-210, HAND-236), all or nothing in one atomic write: the per-file key goes; the files
    /// limit still at 2026-10-09's 50 becomes 300; each MB key goes, and an owner's own value (anything but
    /// 2026-10-09's) carries over to its GB key, converted (MiB to decimal GB).
    pub fn upgrade_downloads(&self) -> Result<Vec<Value>> {
        let mut doc = self.load()?;
        let mut changed = Vec::new();
        if !doc.values()?.contains_key("browser") {
            return Ok(changed);
        }
        let (key, old, new) = DOWNLOAD_FILES;
        if equals_int(doc.value("browser", key)?.as_ref(), old) {
            doc.set("browser", key, &json!(new))?;
            changed.push(change(
                format!("browser.{key}"),
                Some(json!(old)),
                json!(new),
            ));
        }
        for (key, gb_key, decided) in DOWNLOAD_MB_TO_GB {
            let Some(value) = doc.value("browser", key)? else {
                continue;
            };
            doc.remove("browser", key)?;
            changed.push(change(
                format!("browser.{key}"),
                Some(value.clone()),
                Value::Null,
            ));
            let size = value
                .as_f64()
                .filter(|v| *v >= 0.0 && !equals_int(Some(&value), *decided));
            if let Some(mb) = size {
                // Python's round(x, 3) and this formatting both round the exact binary value, halves to even
                let gb: f64 = format!("{:.3}", mb * 1024.0 * 1024.0 / 1e9)
                    .parse()
                    .unwrap_or(f64::NAN);
                let before = doc.value("browser", gb_key)?.unwrap_or(Value::Null);
                changed.push(change(format!("browser.{gb_key}"), Some(before), json!(gb)));
                doc.set("browser", gb_key, &json!(gb))?;
            }
        }
        for key in DOWNLOAD_RETIRED {
            if let Some(value) = doc.value("browser", key)? {
                doc.remove("browser", key)?;
                changed.push(change(format!("browser.{key}"), Some(value), Value::Null));
            }
        }
        if !changed.is_empty() {
            self.save(&doc)?;
        }
        Ok(changed)
    }

    /// config.py:271: `[privacy] exclude_apps`, else the defaults (an emptied list is kept).
    pub fn exclude_apps(&self) -> Result<BTreeSet<String>> {
        match self.get("privacy", "exclude_apps")? {
            None => Ok(DEFAULT_EXCLUDE_APPS.iter().map(|s| s.to_string()).collect()),
            Some(Value::Array(apps)) => Ok(apps
                .iter()
                .filter_map(|a| a.as_str().map(str::to_string))
                .collect()),
            Some(_) => unsupported("[privacy] exclude_apps is not a list"),
        }
    }

    /// config.py:274: write the defaults into config.toml once, so the owner sees and edits them there; a value
    /// the owner set (an emptied list included) is never replaced. One atomic write per missing key, as Python.
    pub fn ensure_privacy_defaults(&self, lists: &PrivacyDefaults) -> Result<()> {
        let defaults: Vec<(&str, &str, Value)> = vec![
            ("privacy", "exclude_apps", json!(DEFAULT_EXCLUDE_APPS)),
            ("privacy", "exclude_domains", json!([])),
            ("privacy", "exclude_paths", json!(lists.exclude_paths)),
            ("sense", "window_titles", json!(true)),
            ("sense", "watch_folders", json!([])),
            ("sense", "idle_after_s", json!(120)),
            ("sense", "trail_retention_days", json!(30)),
            ("retention", "trace_days", json!(30)),
            ("retention", "decision_days", json!(30)),
            ("retention", "evidence_days", json!(7)),
            ("retention", "recovery_days", json!(7)),
            ("retention", "resource_days", json!(30)),
            ("retention", "operational_days", json!(90)),
            ("browser", "download_task_max_files", json!(300)),
            ("browser", "download_task_max_gb", json!(32)),
            ("browser", "download_total_gb", json!(20)),
            ("browser", "download_min_free_gb", json!(10)),
            ("browser", "task_max_tabs", json!(50)),
            ("protection", "never_read", json!(lists.never_read)),
            ("protection", "never_write", json!(lists.never_write)),
            ("protection", "terminal_apps", json!(lists.terminal_apps)),
            ("protection", "local_ports", json!([])),
        ];
        for (section, key, value) in defaults {
            if self.get(section, key)?.is_none() {
                self.set(section, key, &value)?;
            }
        }
        Ok(())
    }
}
