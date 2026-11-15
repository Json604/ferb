//! Today's searchable activity log (`ferb/sense/daily.py`): one JSON row per line, emptied when the day changes,
//! searched by shared terms. Long-term memory is stored elsewhere. It never turns observations into authorization and
//! keeps one copy of each row (a correction or a forget rewrites the file atomically).

use std::collections::HashMap;
use std::fs::{self, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use ferb_clock::zone::Zone;
use ferb_core::when::{isoformat, DateTime};
use ferb_py::text::is_alnum;
use ferb_sanitize::parity::casefold;
use serde_json::{Map, Value};

use crate::observation::dumps;

/// daily.py:18 `terms`: `\w+` runs (Python's `\w`: alphanumeric or `_`) of the casefolded text, longer than two characters.
pub fn terms(text: &str) -> Vec<String> {
    let folded = casefold(text);
    let mut out: Vec<String> = Vec::new();
    let mut current = String::new();
    let flush = |current: &mut String, out: &mut Vec<String>| {
        if current.chars().count() > 2 && !out.contains(current) {
            out.push(current.clone());
        }
        current.clear();
    };
    for c in folded.chars() {
        if c == '_' || is_alnum(c) {
            current.push(c);
        } else {
            flush(&mut current, &mut out);
        }
    }
    flush(&mut current, &mut out);
    out
}

/// daily.py:22 `values`: a row's values as text, never its keys.
pub fn values(node: &Value, out: &mut Vec<String>) {
    match node {
        Value::Object(m) => m.values().for_each(|v| values(v, out)),
        Value::Array(a) => a.iter().for_each(|v| values(v, out)),
        Value::String(s) => out.push(s.clone()),
        Value::Number(n) => out.push(ferb_py::json::number(n)),
        _ => {}
    }
}

/// daily.py:48 `without_queries`: every web address without its query string or fragment.
pub fn without_queries(node: &Value) -> Value {
    match node {
        Value::Object(m) => Value::Object(
            m.iter()
                .map(|(k, v)| (k.clone(), without_queries(v)))
                .collect(),
        ),
        Value::Array(a) => Value::Array(a.iter().map(without_queries).collect()),
        Value::String(s) if s.starts_with("http://") || s.starts_with("https://") => {
            let s = s.split('#').next().unwrap_or("");
            Value::from(s.split('?').next().unwrap_or(""))
        }
        other => other.clone(),
    }
}

struct Index {
    inode: u64,
    generation: u64,
    offset: u64,
    rows: Vec<u64>,
    days: Vec<String>,
    postings: HashMap<String, Vec<u32>>,
}

struct State {
    index: Option<Index>,
    generation: u64,
}

type Now = Box<dyn Fn() -> DateTime + Send + Sync>;

/// daily.py:61.
pub struct DailyActivity {
    pub path: PathBuf,
    now: Now,
    zone: Zone,
    state: Mutex<State>,
}

fn parse_row(bytes: &[u8]) -> Option<Map<String, Value>> {
    let text = std::str::from_utf8(bytes).ok()?;
    match ferb_py::json::loads(text) {
        Ok(Value::Object(m)) => Some(m),
        _ => None,
    }
}

/// Python's text-mode line iteration (universal newlines): lines end at "\n", "\r\n" or "\r".
fn text_lines(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let bytes = text.as_bytes();
    let mut start = 0;
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\n' => {
                out.push(&text[start..=i]);
                start = i + 1;
            }
            b'\r' => {
                let end = if bytes.get(i + 1) == Some(&b'\n') {
                    i + 1
                } else {
                    i
                };
                out.push(&text[start..=end]);
                start = end + 1;
                i = end;
            }
            _ => {}
        }
        i += 1;
    }
    if start < bytes.len() {
        out.push(&text[start..]);
    }
    out
}

/// `bytes.splitlines(keepends=True)`.
fn byte_lines(data: &[u8]) -> Vec<&[u8]> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut i = 0;
    while i < data.len() {
        match data[i] {
            b'\n' => {
                out.push(&data[start..=i]);
                start = i + 1;
            }
            b'\r' => {
                let end = if data.get(i + 1) == Some(&b'\n') {
                    i + 1
                } else {
                    i
                };
                out.push(&data[start..=end]);
                start = end + 1;
                i = end;
            }
            _ => {}
        }
        i += 1;
    }
    if start < data.len() {
        out.push(&data[start..]);
    }
    out
}

fn source_id(row: &Map<String, Value>) -> Option<&Value> {
    if row.contains_key("source_id") {
        row.get("source_id")
    } else {
        row.get("id")
    }
}

impl DailyActivity {
    /// `now`: the daemon's clock; `zone`: the Mac's (Python's `astimezone()`).
    pub fn new(
        path: PathBuf,
        now: impl Fn() -> DateTime + Send + Sync + 'static,
        zone: Zone,
    ) -> Self {
        DailyActivity {
            path,
            now: Box::new(now),
            zone,
            state: Mutex::new(State {
                index: None,
                generation: 0,
            }),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn today(&self) -> String {
        self.zone.date(&(self.now)()).format("%Y-%m-%d").to_string()
    }

    fn rows(&self) -> Vec<Map<String, Value>> {
        let Ok(bytes) = fs::read(&self.path) else {
            return Vec::new();
        };
        let text = String::from_utf8_lossy(&bytes);
        text_lines(&text)
            .into_iter()
            .filter_map(|l| parse_row(l.as_bytes()))
            .collect()
    }

    fn rotate(&self) -> String {
        let today = self.today();
        let first = self.rows().into_iter().next();
        if let Some(first) = first {
            if first.get("day") != Some(&Value::from(today.clone())) {
                let mut s = self.lock();
                s.generation += 1;
                let _ = fs::write(&self.path, "");
            }
        }
        today
    }

    /// daily.py:89: empty the file when it still holds an earlier day's rows.
    pub fn roll_over(&self) {
        self.rotate();
    }

    /// daily.py:93.
    pub fn record(&self, event: &Map<String, Value>) -> std::io::Result<()> {
        let day = self.rotate();
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut row = event.clone();
        row.insert("day".into(), Value::from(day));
        let when = self.zone.local(&(self.now)());
        row.insert("when".into(), Value::from(isoformat(&when)));
        if event.get("id").is_some_and(ferb_py::value::truthy) {
            row.insert("source_id".into(), event["id"].clone());
        }
        let mut separator = "";
        if let Ok(mut f) = fs::File::open(&self.path) {
            if f.metadata()?.len() > 0 {
                f.seek(SeekFrom::End(-1))?;
                let mut last = [0u8; 1];
                f.read_exact(&mut last)?;
                if last[0] != b'\n' {
                    separator = "\n";
                }
            }
        }
        let mut out = OpenOptions::new()
            .append(true)
            .create(true)
            .mode(0o600)
            .open(&self.path)?;
        out.write_all(format!("{separator}{}\n", dumps(&Value::Object(row))).as_bytes())
    }

    fn rewrite(
        &self,
        transform: &dyn Fn(Map<String, Value>) -> Option<Map<String, Value>>,
    ) -> std::io::Result<()> {
        let mut s = self.lock();
        if !self.path.exists() {
            return Ok(());
        }
        let dir = self.path.parent().unwrap_or(Path::new("."));
        let mut name = [0u8; 6];
        getrandom::fill(&mut name).map_err(|e| std::io::Error::other(e.to_string()))?;
        let temporary = dir.join(format!(".activity-{}", hex::encode(name)));
        let result = (|| {
            let mut out = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&temporary)?;
            for row in self.rows() {
                if let Some(updated) = transform(row) {
                    out.write_all(format!("{}\n", dumps(&Value::Object(updated))).as_bytes())?;
                }
            }
            out.flush()?;
            out.sync_all()?;
            s.generation += 1;
            fs::rename(&temporary, &self.path)
        })();
        let _ = fs::remove_file(&temporary);
        result
    }

    /// daily.py:131.
    pub fn forget_sources(&self, ids: &[String]) -> std::io::Result<()> {
        if ids.is_empty() {
            return Ok(());
        }
        self.rewrite(&|row| {
            let hit = matches!(source_id(&row), Some(Value::String(s)) if ids.contains(s));
            (!hit).then_some(row)
        })
    }

    /// daily.py:135.
    pub fn redact_source(&self, source: &str, data: &Value) -> std::io::Result<()> {
        self.rewrite(&|row| {
            if matches!(source_id(&row), Some(Value::String(s)) if s == source) {
                let mut out: Map<String, Value> =
                    row.into_iter().filter(|(k, _)| k != "where").collect();
                out.insert("source_id".into(), Value::from(source));
                out.insert("data".into(), data.clone());
                Some(out)
            } else {
                Some(row)
            }
        })
    }

    /// daily.py:142: today's rows sharing the most terms with the query, best first (the more recent among near
    /// equals). Read-only.
    pub fn search(&self, query: &str, limit: usize) -> Vec<Map<String, Value>> {
        let day = self.today();
        let wanted = terms(query);
        if wanted.is_empty() {
            return Vec::new();
        }
        let mut s = self.lock();
        let generation = s.generation;
        let Some(index) = self.refresh(&mut s.index, generation) else {
            return Vec::new();
        };
        let mut scores: HashMap<u32, u32> = HashMap::new();
        for term in &wanted {
            for row in index.postings.get(term).into_iter().flatten() {
                *scores.entry(*row).or_default() += 1;
            }
        }
        let count = index.rows.len().max(1) as f64;
        let mut best: Vec<(f64, u32)> = scores
            .iter()
            .filter(|(row, _)| index.days[**row as usize] == day)
            .map(|(row, score)| (f64::from(*score) + 0.5 * f64::from(*row) / count, *row))
            .collect();
        best.sort_by(|a, b| b.0.total_cmp(&a.0).then(b.1.cmp(&a.1)));
        let offsets: Vec<u64> = best
            .iter()
            .take(limit.max(1))
            .map(|(_, row)| index.rows[*row as usize])
            .collect();
        let Ok(bytes) = fs::read(&self.path) else {
            return Vec::new();
        };
        let mut found = Vec::new();
        for offset in offsets {
            let rest = bytes.get(offset as usize..).unwrap_or(&[]);
            let end = rest
                .iter()
                .position(|b| *b == b'\n')
                .map_or(rest.len(), |i| i + 1);
            if let Some(row) = parse_row(&rest[..end]) {
                found.push(row);
            }
        }
        found
    }

    fn refresh<'a>(&self, slot: &'a mut Option<Index>, generation: u64) -> Option<&'a Index> {
        let Ok(meta) = fs::metadata(&self.path) else {
            *slot = None;
            return None;
        };
        let stale = match slot {
            None => true,
            Some(i) => i.inode != meta.ino() || i.generation != generation || meta.len() < i.offset,
        };
        if stale {
            *slot = Some(Index {
                inode: meta.ino(),
                generation,
                offset: 0,
                rows: Vec::new(),
                days: Vec::new(),
                postings: HashMap::new(),
            });
        }
        let index = slot.as_mut().expect("an index");
        if meta.len() > index.offset {
            let mut data = Vec::new();
            if let Ok(mut f) = fs::File::open(&self.path) {
                let _ = f.seek(SeekFrom::Start(index.offset));
                let _ = f.take(meta.len() - index.offset).read_to_end(&mut data);
            }
            let end = data.iter().rposition(|b| *b == b'\n').map_or(0, |i| i + 1);
            let mut position = index.offset;
            for line in byte_lines(&data[..end]) {
                if let Some(row) = parse_row(line) {
                    let number = index.rows.len() as u32;
                    index.rows.push(position);
                    index.days.push(crate::observation::py_str(row.get("day")));
                    let mut texts = Vec::new();
                    values(&Value::Object(row), &mut texts);
                    for term in terms(&texts.join(" ")) {
                        index.postings.entry(term).or_default().push(number);
                    }
                }
                position += line.len() as u64;
            }
            index.offset += end as u64;
        }
        Some(index)
    }
}
