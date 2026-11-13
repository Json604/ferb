//! The before-and-after record of the files a task changes (L39; ferb/act/fs/changes.py): before a call that can
//! change files, a copy of each existing file it names and a listing of each folder it names or writes into; at the
//! step's check, what changed, as an old-vs-new text diff the checker reads. Code records and compares facts; the
//! checker judges (rules 5, 6). Every file and folder is opened without following a link, the path it really opened
//! (F_GETPATH) is checked, and only then is it cloned, read or listed, so a link swapped in between can't redirect it.
//!
//! Seams: the protection lists' read check ([`crate::Closed`]), PDF text through PDFKit ([`PdfText`]), the owner's home
//! (injected, never read from the environment). Test points (`crate::hooks`): `changes.access <path>` before each
//! `os.access`, `changes.skip <path>` after each `_skip`, `changes.open_dir <cwd|fd>|<path>` after a folder is opened,
//! `changes.read <path>` / `changes.read_done <path>` around `_read`.

use std::cell::RefCell;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::{Arc, Mutex, OnceLock};

use regex::Regex;
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

use crate::difflib::{splitlines, str_ratio, unified_diff, SequenceMatcher};
use crate::hooks;
use crate::pypath as pp;
use crate::recovery;
use crate::sys::{self, At, Fd, St};
use crate::Closed;

/// A made file is shown against a file at least this alike (git's default copy/rename bar).
pub const SIMILAR: f64 = 0.5;
/// Display only: characters of extracted text compared per side.
pub const TEXT_MAX: usize = 2_000_000;
pub const CONTEXT_LINES: usize = 2;
/// Display only: a file larger than this is not read for a diff.
pub const FILE_MAX: i64 = 100_000_000;
pub const PAIR_READS: usize = 3;
pub const PAIR_SIZE: i64 = 4;
pub const FOLDER_FACTS: usize = 50;
pub const WALK_MAX: usize = 5_000;
pub const WALK_DEPTH: usize = 32;
pub const PREVIEW_CHARS: usize = 12_000;
pub const DOCUMENTS: [&str; 4] = [".pdf", ".docx", ".docm", ".dotx"];
pub const UNREADABLE: &str = "FERB can't read it";
pub const UNCHANGED: &str = "content unchanged (the same bytes as before the step)";
pub const HISTORY_NOTE: &str =
    "a shell history file: what is typed in a terminal is never shown (rule 14)";
pub const INDEX: &str = "index.json";

/// A PDF annotation: (page, type, its text).
pub type Annotation = (i64, String, String);
/// What a file says: (text, PDF annotations).
pub type Text = (String, Vec<Annotation>);
/// A reader of what a file says (`read`/`base` in `describe`).
pub type ReadFn<'a> = &'a dyn Fn(&str) -> Option<Text>;
/// A pairing candidate: (rank, original, readable, current, real).
type Candidate<R> = (R, String, String, bool, Option<String>);

/// PDFKit's reading of a PDF's bytes (changes.py `_pdf`): its text layer page by page ("[page N]\n<text>", joined by
/// newlines) and its annotations. None: no PDF FERB can read (or PDFKit is unavailable).
pub trait PdfText: Send + Sync {
    fn text(&self, data: &[u8]) -> Option<Text>;
}

// ---------- what a command's text names ----------

/// Python's `\s` for str patterns (every character `str.isspace` accepts).
const SP: &str = r"\t\n\x0B\x0C\r\x1C-\x1F \x{85}\x{A0}\x{1680}\x{2000}-\x{200A}\x{2028}\x{2029}\x{202F}\x{205F}\x{3000}";

struct Patterns {
    assign: fancy_regex::Regex,
    cd: fancy_regex::Regex,
    tokens: Regex,
    inner: fancy_regex::Regex,
    file_name: Regex,
    suffix: Regex,
    var: Regex,
    embedded: Regex,
    glob: Regex,
    escape: Regex,
}

fn rx() -> &'static Patterns {
    static P: OnceLock<Patterns> = OnceLock::new();
    P.get_or_init(|| {
        let f = |p: String| fancy_regex::Regex::new(&p).expect("pattern");
        let r = |p: String| Regex::new(&p).expect("pattern");
        Patterns {
            assign: f(format!(
                r#"(?:^|(?<=[{SP};&|(]))(?:export[{SP}]+|local[{SP}]+|readonly[{SP}]+|declare[{SP}]+)?([A-Za-z_][A-Za-z0-9_]*)=("(?:[^"\\\n]|\\.)*"|'[^'\n]*'|[^{SP};&|()<>'"`]*)"#
            )),
            cd: f(format!(
                r#"(?:^|(?<=[{SP};&|(]))cd[{SP}]+("(?:[^"\\\n]|\\.)*"|'[^'\n]*'|[^{SP};&|()<>'"`]+)"#
            )),
            tokens: r(format!(r#""((?:[^"\\\n]|\\.)*)"|'([^'\n]*)'|([^{SP}'"`;|&<>(){{}}]+)"#)),
            inner: f(r#"(?<=['"])(~?/[^'"\n]+)(?=['"])"#.to_string()),
            file_name: r(format!(r"\A[^/{SP}]+\.[A-Za-z0-9]{{1,8}}\z")),
            suffix: r(r"\A\.[A-Za-z0-9]{1,16}\z".to_string()),
            var: r(r"\$\{([A-Za-z_][A-Za-z0-9_]*)\}|\$([A-Za-z_][A-Za-z0-9_]*)".to_string()),
            embedded: r(r"(?:^|[=:,(])((?:~|/)[^,;:)]*)$".to_string()),
            glob: r(r"[*?\[]".to_string()),
            escape: r(r"\\(.)".to_string()),
        }
    })
}

fn unescape(text: &str) -> String {
    rx().escape.replace_all(text, "$1").into_owned()
}

fn unquote(word: &str) -> String {
    let n = word.chars().count();
    if n >= 2 && word.starts_with('\'') && word.ends_with('\'') {
        return word[1..word.len() - 1].to_string();
    }
    if n >= 2 && word.starts_with('"') && word.ends_with('"') {
        return unescape(&word[1..word.len() - 1]);
    }
    word.to_string()
}

/// `_expand`: the command's own literal variables put in, ~ expanded; None when a variable stays unknown or a command
/// substitution is in it.
fn expand(text: &str, values: &HashMap<String, String>, home: &str) -> Option<String> {
    if text.contains("$(") || text.contains('`') {
        return None;
    }
    let mut text = text.to_string();
    for _ in 0..4 {
        let done = rx()
            .var
            .replace_all(&text, |c: &regex::Captures| {
                let name = c
                    .get(1)
                    .or_else(|| c.get(2))
                    .map(|m| m.as_str())
                    .unwrap_or("");
                values
                    .get(name)
                    .cloned()
                    .unwrap_or_else(|| "\0".to_string())
            })
            .into_owned();
        if done == text {
            break;
        }
        text = done;
    }
    if text.contains('\0') || text.contains('$') {
        return None;
    }
    if text == "~" || text.starts_with("~/") {
        text = format!("{home}{}", &text[1..]);
    }
    Some(text)
}

/// `named(text, cwd, home)`: every path written in a command's or script's text, as the text spells it.
///
/// One deliberate difference from Python: for a word holding an embedded path (`--out=/x/y`), Python iterates the
/// set `{word, embedded}` in its hash-seeded order, so which of the two comes first changes from run to run; here the
/// word always comes first.
pub fn named(text: &str, cwd: &str, home: &str) -> Vec<String> {
    let p = rx();
    let mut values: HashMap<String, String> = HashMap::new();
    for m in p.assign.captures_iter(text).flatten() {
        let raw = m.get(2).map(|g| g.as_str()).unwrap_or("");
        if let Some(value) = expand(&unquote(raw), &values, home) {
            values.insert(
                m.get(1).map(|g| g.as_str()).unwrap_or("").to_string(),
                value,
            );
        }
    }
    let cds: Vec<(usize, String)> =
        p.cd.captures_iter(text)
            .flatten()
            .map(|m| {
                (
                    m.get(0).map(|g| g.start()).unwrap_or(0),
                    unquote(m.get(1).map(|g| g.as_str()).unwrap_or("")),
                )
            })
            .collect();
    let mut words: Vec<(usize, String)> = Vec::new();
    for m in p.tokens.captures_iter(text) {
        let start = m.get(0).map(|g| g.start()).unwrap_or(0);
        if let Some(g) = m.get(1) {
            words.push((start, unescape(g.as_str())));
        } else {
            let g = m
                .get(2)
                .or_else(|| m.get(3))
                .map(|g| g.as_str())
                .unwrap_or("");
            words.push((start, g.to_string()));
        }
    }
    for m in p.inner.captures_iter(text).flatten() {
        if let Some(g) = m.get(1) {
            words.push((
                m.get(0).map(|x| x.start()).unwrap_or(0),
                g.as_str().to_string(),
            ));
        }
    }
    let mut found: Vec<String> = Vec::new();
    for (start, raw) in &words {
        let mut base = pp::norm(cwd);
        for (at, where_) in &cds {
            if at >= start {
                break;
            }
            if let Some(moved) = expand(where_, &values, home) {
                base = if !moved.starts_with('/') {
                    pp::join(&base, &moved)
                } else {
                    pp::norm(&moved)
                };
            }
        }
        let mut pieces = vec![raw.clone()];
        if let Some(e) = p.embedded.captures(raw).and_then(|c| c.get(1)) {
            if e.as_str() != raw {
                pieces.push(e.as_str().to_string());
            }
        }
        for piece in pieces {
            let Some(mut path) = expand(ferb_py::value::strip(&piece), &values, home) else {
                continue;
            };
            if path.is_empty()
                || path.contains('\n')
                || path.contains('\0')
                || path.chars().count() > 1024
            {
                continue;
            }
            if let Some(g) = p.glob.find(&path) {
                let head_all = &path[..g.start()];
                match head_all.rfind('/') {
                    None => {
                        found.push(base.clone());
                        continue;
                    }
                    Some(i) => {
                        let head = &head_all[..i];
                        path = if head.is_empty() {
                            "/".into()
                        } else {
                            head.to_string()
                        };
                    }
                }
            } else if !path.contains('/') && !p.file_name.is_match(&path) {
                continue;
            }
            found.push(if path.starts_with('/') {
                pp::norm(&path)
            } else {
                pp::join(&base, &path)
            });
        }
    }
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for f in found {
        let n = pp::norm(&pp::normpath(&f));
        if seen.insert(n.clone()) {
            out.push(n);
        }
    }
    out
}

// ---------- the record ----------

#[derive(Clone, Debug, PartialEq)]
struct FileRec {
    existed: bool,
    size: i64,
    mtime_ns: i64,
    ino: i64,
    copy: Option<String>,
    no_copy: String,
    real: String,
    history: bool,
    dev: i64,
}

impl FileRec {
    fn new(existed: bool) -> FileRec {
        FileRec {
            existed,
            size: 0,
            mtime_ns: 0,
            ino: 0,
            copy: None,
            no_copy: String::new(),
            real: String::new(),
            history: false,
            dev: 0,
        }
    }

    fn to_json(&self) -> Value {
        json!({"existed": self.existed, "size": self.size, "mtime_ns": self.mtime_ns, "ino": self.ino,
               "copy": self.copy, "no_copy": self.no_copy, "real": self.real, "history": self.history,
               "dev": self.dev})
    }

    /// `_File(**v)`: only its own fields, `existed` required.
    fn from_json(v: &Value) -> Option<FileRec> {
        let m = v.as_object()?;
        let known = [
            "existed", "size", "mtime_ns", "ino", "copy", "no_copy", "real", "history", "dev",
        ];
        if m.keys().any(|k| !known.contains(&k.as_str())) {
            return None;
        }
        let mut f = FileRec::new(m.get("existed")?.as_bool()?);
        let int = |k: &str, d: i64| -> Option<i64> {
            match m.get(k) {
                None => Some(d),
                Some(v) => v.as_i64(),
            }
        };
        f.size = int("size", 0)?;
        f.mtime_ns = int("mtime_ns", 0)?;
        f.ino = int("ino", 0)?;
        f.dev = int("dev", 0)?;
        f.copy = match m.get("copy") {
            None | Some(Value::Null) => None,
            Some(v) => Some(v.as_str()?.to_string()),
        };
        f.no_copy = match m.get("no_copy") {
            None => String::new(),
            Some(v) => v.as_str()?.to_string(),
        };
        f.real = match m.get("real") {
            None => String::new(),
            Some(v) => v.as_str()?.to_string(),
        };
        f.history = match m.get("history") {
            None => false,
            Some(v) => v.as_bool()?,
        };
        Some(f)
    }
}

/// A dict in insertion order.
#[derive(Clone, Debug, Default)]
struct Files {
    order: Vec<String>,
    map: HashMap<String, FileRec>,
}

impl Files {
    fn get(&self, k: &str) -> Option<&FileRec> {
        self.map.get(k)
    }
    fn contains(&self, k: &str) -> bool {
        self.map.contains_key(k)
    }
    fn insert(&mut self, k: String, v: FileRec) {
        if !self.map.contains_key(&k) {
            self.order.push(k.clone());
        }
        self.map.insert(k, v);
    }
    fn iter(&self) -> impl Iterator<Item = (&String, &FileRec)> {
        self.order.iter().map(move |k| (k, &self.map[k]))
    }
}

#[derive(Clone, Debug)]
struct Record {
    task_id: String,
    tree_id: String,
    files: Files,
    /// path -> name -> [kind, size, mtime, ino], both in insertion order.
    folders: Map<String, Value>,
    named: BTreeSet<String>,
    n: i64,
}

impl Record {
    fn new(task_id: &str, tree_id: &str) -> Record {
        Record {
            task_id: task_id.into(),
            tree_id: tree_id.into(),
            files: Files::default(),
            folders: Map::new(),
            named: BTreeSet::new(),
            n: 0,
        }
    }

    /// The index text `_save` writes.
    fn dumps(&self) -> String {
        let mut files = Map::new();
        for (k, f) in self.files.iter() {
            files.insert(k.clone(), f.to_json());
        }
        let mut top = Map::new();
        top.insert("task_id".into(), json!(self.task_id));
        top.insert("tree_id".into(), json!(self.tree_id));
        top.insert("n".into(), json!(self.n));
        top.insert("folders".into(), Value::Object(self.folders.clone()));
        top.insert("named".into(), json!(self.named.iter().collect::<Vec<_>>()));
        top.insert("files".into(), Value::Object(files));
        ferb_py::json::dumps(&Value::Object(top))
    }

    fn listing(&self, folder: &str) -> Option<&Map<String, Value>> {
        self.folders.get(folder).and_then(Value::as_object)
    }
}

/// One file that changed during the task, as facts (changes.py `Change`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Change {
    pub path: String,
    pub what: String,
    pub before: Option<String>,
    pub source: Option<String>,
    pub note: String,
    pub content: bool,
    pub size: Option<i64>,
    pub mtime_ns: Option<i64>,
    pub named: bool,
    pub before_real: Option<String>,
    pub original: bool,
}

impl Change {
    pub fn new(path: &str, what: &str) -> Change {
        Change {
            path: path.into(),
            what: what.into(),
            before: None,
            source: None,
            note: String::new(),
            content: true,
            size: None,
            mtime_ns: None,
            named: false,
            before_real: None,
            original: false,
        }
    }

    /// `dataclasses.asdict(change)`.
    pub fn to_json(&self) -> Value {
        json!({"path": self.path, "what": self.what, "before": self.before, "source": self.source,
               "note": self.note, "content": self.content, "size": self.size, "mtime_ns": self.mtime_ns,
               "named": self.named, "before_real": self.before_real, "original": self.original})
    }

    fn fact(path: &str, what: &str, st: Option<&St>) -> Change {
        Change {
            content: false,
            size: st.map(|s| s.size),
            mtime_ns: st.map(|s| s.mtime_ns() as i64),
            ..Change::new(path, what)
        }
    }
}

/// What `before` counted for the call's record.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counts {
    pub copied: i64,
    pub not_copied: i64,
    pub folders: i64,
    pub protected: i64,
}

impl Counts {
    /// The Python dict, in its order.
    pub fn to_json(&self) -> Value {
        json!({"copied": self.copied, "not_copied": self.not_copied, "folders": self.folders,
               "protected": self.protected})
    }
}

fn kind_code(st: &St) -> i64 {
    if st.is_reg() {
        1
    } else if st.is_dir() {
        2
    } else {
        3
    }
}

fn what_now(st: &St, path: &str) -> String {
    if st.is_lnk() {
        return match sys::readlink(path) {
            Ok(t) => format!("now a link to {t}"),
            Err(_) => "now a link".into(),
        };
    }
    if st.is_dir() {
        "now a folder".into()
    } else {
        "now a special file (not a plain file)".into()
    }
}

/// `_history_name`: *_history, *.history, .histfile, .lesshst (any case).
fn history_name(name: &str) -> bool {
    let low = crate::casefold(name);
    low.starts_with('.')
        && (low.ends_with("_history")
            || low.ends_with(".history")
            || low == ".histfile"
            || low == ".lesshst")
}

const NOFOLLOW: i32 = sys::O_RDONLY | sys::O_NOFOLLOW | sys::O_NONBLOCK | sys::O_CLOEXEC;

/// `_open`: an fd for the path itself, never through a link at its last name; None when it can't be opened so.
fn open_nofollow(path: &str, directory: bool, dir_fd: Option<i32>) -> Option<Fd> {
    let at = dir_fd.map(At::Dir).unwrap_or(At::Cwd);
    let fd = sys::open(
        at,
        path,
        NOFOLLOW | if directory { sys::O_DIRECTORY } else { 0 },
        0,
    )
    .ok()?;
    if directory {
        let how = if dir_fd.is_some() { "fd" } else { "cwd" };
        hooks::at(&format!("changes.open_dir {how}|{path}"));
    }
    Some(fd)
}

fn access(path: &str, mode: i32, follow: bool) -> bool {
    hooks::at(&format!("changes.access {path}"));
    sys::access(path, mode, follow)
}

fn identity(path: &str) -> Option<(i64, i64)> {
    sys::stat(path).ok().map(|s| (s.dev, s.ino as i64))
}

fn kind_of(path: &str) -> &'static str {
    let suffix = lower(&pp::suffix(path));
    if suffix == ".pdf" {
        "pdf"
    } else if DOCUMENTS.contains(&suffix.as_str()) {
        "docx"
    } else {
        "text"
    }
}

fn lower(s: &str) -> String {
    ferb_py::text::lower(s)
}

/// `_key`: the first 24 hex digits of the text's SHA-256.
fn key(text: &str) -> String {
    hex::encode(Sha256::digest(text.as_bytes()))[..24].to_string()
}

/// A listed folder now (changes.py `_Scan`).
struct Scan {
    where_: String,
    dev: i64,
    entries: Vec<(String, St)>,
    index: HashMap<String, usize>,
    fd: Option<Fd>,
}

impl Scan {
    fn get(&self, name: &str) -> Option<&St> {
        self.index.get(name).map(|i| &self.entries[*i].1)
    }
    fn fd(&self) -> Option<i32> {
        self.fd.as_ref().map(Fd::raw)
    }
}

/// One check's walk (changes.py `_Look`).
struct Look {
    seen: HashSet<String>,
    out: Vec<Change>,
    moved: HashMap<(i64, i64), String>,
    renamed: HashSet<String>,
    held: HashSet<String>,
    names: usize,
    entries: usize,
    more: HashMap<String, i64>,
    stopped: HashSet<String>,
    deep: HashSet<String>,
}

/// The before-and-after record (changes.py `Changes`).
pub struct Changes {
    pub root: String,
    closed: Closed,
    pub home: String,
    sessions: [String; 2],
    fish: String,
    pdf: Option<Arc<dyn PdfText>>,
    records: Mutex<HashMap<String, Arc<Mutex<Record>>>>,
    saved: Mutex<HashMap<String, String>>,
    locks: Mutex<HashMap<String, Arc<Mutex<()>>>>,
}

/// One call's view: the history files' identities, enumerated afresh per `before` and `changes` call.
struct Call<'a> {
    c: &'a Changes,
    hist: RefCell<Option<HashSet<(i64, i64)>>>,
}

impl Changes {
    /// `Changes(root, closed, home)`: `closed(path)` is whether FERB's shell sandbox denies reading the path.
    pub fn new(root: &str, closed: Option<Closed>, home: &str) -> Changes {
        let home = pp::norm(home);
        Changes {
            root: pp::norm(root),
            closed: closed.unwrap_or_else(|| Arc::new(|_: &std::path::Path| false)),
            sessions: [
                pp::join(&home, ".zsh_sessions"),
                pp::join(&home, ".bash_sessions"),
            ],
            fish: pp::join_all(&home, [".local", "share", "fish", "fish_history"]),
            home,
            pdf: None,
            records: Mutex::new(HashMap::new()),
            saved: Mutex::new(HashMap::new()),
            locks: Mutex::new(HashMap::new()),
        }
    }

    /// PDFKit's reading, for PDFs (None: no PDF text, as Python without PyObjC).
    pub fn with_pdf(mut self, pdf: Arc<dyn PdfText>) -> Changes {
        self.pdf = Some(pdf);
        self
    }

    fn pdf(&self) -> Option<&dyn PdfText> {
        self.pdf.as_deref()
    }

    /// Where one task's copies and index live.
    pub fn folder(&self, tree_id: &str, task_id: &str) -> String {
        pp::join_all(&self.root, [key(tree_id).as_str(), key(task_id).as_str()])
    }

    fn lock(&self, task_id: &str) -> Arc<Mutex<()>> {
        let mut locks = self.locks.lock().unwrap_or_else(|e| e.into_inner());
        locks.entry(task_id.to_string()).or_default().clone()
    }

    fn call(&self) -> Call<'_> {
        Call {
            c: self,
            hist: RefCell::new(None),
        }
    }

    fn record(&self, task_id: &str, tree_id: Option<&str>) -> Option<Arc<Mutex<Record>>> {
        if let Some(r) = self
            .records
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(task_id)
        {
            return Some(r.clone());
        }
        let tree_id = tree_id?;
        let index = pp::join(&self.folder(tree_id, task_id), INDEX);
        let rec = match std::fs::read(&index)
            .ok()
            .and_then(|b| String::from_utf8(b).ok())
            .and_then(|text| load_record(&text).map(|r| (text, r)))
        {
            Some((text, rec)) => {
                self.saved
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .insert(task_id.to_string(), text);
                rec
            }
            None => Record::new(task_id, tree_id),
        };
        let mut records = self.records.lock().unwrap_or_else(|e| e.into_inner());
        Some(
            records
                .entry(task_id.to_string())
                .or_insert_with(|| Arc::new(Mutex::new(rec)))
                .clone(),
        )
    }

    fn save(&self, rec: &Record) {
        let text = rec.dumps();
        let mut saved = self.saved.lock().unwrap_or_else(|e| e.into_inner());
        if saved.get(&rec.task_id) == Some(&text) {
            return;
        }
        let folder = self.folder(&rec.tree_id, &rec.task_id);
        let tmp = pp::join(&folder, &format!("{INDEX}.tmp"));
        if std::fs::write(&tmp, &text).is_ok()
            && std::fs::rename(&tmp, pp::join(&folder, INDEX)).is_ok()
        {
            saved.insert(rec.task_id.clone(), text);
        }
    }

    /// The task's record as its index text (None: no record in memory), for a test or a trace.
    pub fn record_text(&self, task_id: &str) -> Option<String> {
        let r = self
            .records
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(task_id)
            .cloned()?;
        let rec = r.lock().unwrap_or_else(|e| e.into_inner());
        Some(rec.dumps())
    }

    fn under_root(&self, p: &str) -> bool {
        pp::is_relative_to(p, &self.root)
    }

    /// `_skip`: never recorded or read: the root, /dev, a Trash, FERB's own change store, and what the shell's
    /// sandbox denies to reads, as the path is written and as it really resolves.
    fn skip(&self, path: &str) -> bool {
        let r = self.skip_inner(path);
        hooks::at(&format!("changes.skip {path}"));
        r
    }

    fn skip_inner(&self, path: &str) -> bool {
        let path = pp::norm(path);
        if path == "/" || dev_or_trash(&path) {
            return true;
        }
        if path.contains('\0') {
            return true;
        }
        let resolved = pp::norm(&pp::realpath(&path));
        let mut forms = vec![path.clone()];
        if resolved != path {
            forms.push(resolved);
        }
        forms
            .iter()
            .any(|p| self.under_root(p) || (self.closed)(std::path::Path::new(p)))
    }

    /// `_closed_as_written`: `_skip` for a recorded path as it was written, without resolving it.
    fn closed_as_written(&self, path: &str) -> bool {
        let path = pp::norm(path);
        dev_or_trash(&path) || self.under_root(&path) || (self.closed)(std::path::Path::new(&path))
    }

    /// `_opened`: the path an open fd really is, when it isn't skipped.
    fn opened(&self, fd: i32) -> Option<String> {
        let where_ = sys::opened_path(fd).ok()?;
        if self.skip(&where_) {
            None
        } else {
            Some(where_)
        }
    }

    // ---------- before a call ----------

    /// Before a call that may change these paths (each named exactly by the call). Never fails for one path.
    pub fn before(&self, task_id: &str, tree_id: &str, paths: &[String]) -> Counts {
        let lock = self.lock(task_id);
        let _held = lock.lock().unwrap_or_else(|e| e.into_inner());
        let rec = self
            .record(task_id, Some(tree_id))
            .expect("a tree id makes a record");
        let mut rec = rec.lock().unwrap_or_else(|e| e.into_inner());
        let call = self.call();
        let folder = self.folder(tree_id, task_id);
        make_dirs(&folder);
        let mut counts = Counts::default();
        for raw in paths {
            let Some(path) = real(raw, &self.home) else {
                continue;
            };
            if self.skip(&path) {
                counts.protected += if path == "/" { 0 } else { 1 };
                continue;
            }
            match sys::lstat(&path) {
                Err(e) if e.errno == libc::ENOENT => {
                    if let Some(near) = nearest(&path) {
                        if near != "/" && access(&near, libc::W_OK, true) {
                            rec.named.insert(path.clone());
                            call.list(&mut rec, &near, &mut counts);
                        }
                    }
                }
                Err(_) => {}
                Ok(st) if st.is_dir() => {
                    call.list(&mut rec, &path, &mut counts);
                }
                Ok(st) if st.is_reg() => {
                    call.list(&mut rec, &pp::parent(&path), &mut counts);
                    call.file(&mut rec, &folder, &path, &mut counts);
                }
                Ok(_) => {}
            }
        }
        self.save(&rec);
        counts
    }

    /// A loop round starts its own record. Returns what couldn't be removed.
    pub fn reset(&self, task_id: &str) -> Vec<String> {
        let lock = self.lock(task_id);
        let _held = lock.lock().unwrap_or_else(|e| e.into_inner());
        let rec = self
            .records
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(task_id);
        self.saved
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(task_id);
        match rec {
            Some(r) => {
                let tree = r.lock().unwrap_or_else(|e| e.into_inner()).tree_id.clone();
                remove_tree(&self.folder(&tree, task_id))
            }
            None => Vec::new(),
        }
    }

    // ---------- at the check ----------

    /// What changed since the task first touched each recorded file or folder.
    pub fn changes(&self, task_id: &str, tree_id: Option<&str>) -> Vec<Change> {
        let lock = self.lock(task_id);
        let _held = lock.lock().unwrap_or_else(|e| e.into_inner());
        let Some(rec) = self.record(task_id, tree_id) else {
            return Vec::new();
        };
        let rec = rec.lock().unwrap_or_else(|e| e.into_inner());
        let call = self.call();
        let folder = self.folder(&rec.tree_id, task_id);
        let mut scans: HashMap<String, Option<Scan>> = HashMap::new();
        for where_ in rec.folders.keys() {
            scans.insert(where_.clone(), call.scan(where_, true));
        }
        call.check(&rec, &folder, &scans)
        // the scans' fds close when `scans` drops
    }

    /// What a current file says, opened without following a link and only when the file it opens isn't closed.
    pub fn text_of(&self, path: &str) -> Option<Text> {
        read_text(path, Some(&|opened: &str| self.skip(opened)), self.pdf())
    }

    /// One change as the checker reads it (the module's `describe`, through `text_of`).
    pub fn describe(&self, change: &Change) -> String {
        let mut change = change.clone();
        if let Some(br) = &change.before_real {
            if self.closed_as_written(br) {
                change.before = None;
                change.note = "its copy came from a path closed to FERB's shell".into();
            }
        }
        let read = |p: &str| self.text_of(p);
        if let Some(before) = change.before.clone() {
            if !pp::parents(&before).iter().any(|p| pp::same(p, &self.root)) {
                let old = self.text_of(&before);
                return match old {
                    None => {
                        let mut c = change.clone();
                        c.before = None;
                        c.source = None;
                        describe(&c, &read, None, self.pdf())
                    }
                    Some(old) => describe(
                        &change,
                        &read,
                        Some(&|_: &str| Some(old.clone())),
                        self.pdf(),
                    ),
                };
            }
        }
        describe(&change, &read, None, self.pdf())
    }

    /// GATE-539: this task tree made `path` (by its records, never by what the path is).
    pub fn made_in_tree(&self, tree_id: &str, path: &str) -> bool {
        let Some(target) = real(path, &self.home) else {
            return false;
        };
        let mut recs: Vec<(Map<String, Value>, BTreeSet<String>)> = Vec::new();
        let mut seen = HashSet::new();
        let all: Vec<Arc<Mutex<Record>>> = self
            .records
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .values()
            .cloned()
            .collect();
        for r in all {
            let r = r.lock().unwrap_or_else(|e| e.into_inner());
            if r.tree_id == tree_id {
                seen.insert(r.task_id.clone());
                recs.push((r.folders.clone(), r.named.clone()));
            }
        }
        for index in indexes(&pp::join(&self.root, &key(tree_id))) {
            let Some(data) = std::fs::read_to_string(&index)
                .ok()
                .and_then(|t| ferb_py::json::loads(&t).ok())
            else {
                continue;
            };
            let (Some(task), Some(folders)) = (
                data.get("task_id").and_then(Value::as_str),
                data.get("folders").and_then(Value::as_object),
            ) else {
                continue;
            };
            if data.get("tree_id").is_none() {
                continue;
            }
            if !seen.contains(task) {
                let named = data
                    .get("named")
                    .and_then(Value::as_array)
                    .map(|a| {
                        a.iter()
                            .filter_map(Value::as_str)
                            .map(String::from)
                            .collect()
                    })
                    .unwrap_or_default();
                recs.push((folders.clone(), named));
            }
        }
        let parent = pp::parent(&target);
        let name = pp::name(&target);
        let listed: Vec<&Value> = recs.iter().filter_map(|(f, _)| f.get(&parent)).collect();
        if listed
            .iter()
            .any(|names| names.as_object().is_some_and(|m| m.contains_key(&name)))
        {
            return false;
        }
        !listed.is_empty() || recs.iter().any(|(_, named)| named.contains(&target))
    }

    /// The tree ended: its copies and records go. Returns what couldn't be removed.
    pub fn forget(&self, tree_id: &str) -> Vec<String> {
        {
            let mut records = self.records.lock().unwrap_or_else(|e| e.into_inner());
            let gone: Vec<String> = records
                .iter()
                .filter(|(_, r)| r.lock().unwrap_or_else(|e| e.into_inner()).tree_id == tree_id)
                .map(|(t, _)| t.clone())
                .collect();
            let mut saved = self.saved.lock().unwrap_or_else(|e| e.into_inner());
            for t in gone {
                records.remove(&t);
                saved.remove(&t);
            }
        }
        remove_tree(&pp::join(&self.root, &key(tree_id)))
    }

    /// At FERB's start: what a stopped daemon left, except the records of trees that will resume.
    pub fn sweep(&self, keep: &dyn Fn(&str) -> bool) -> Vec<String> {
        let mut failed = Vec::new();
        self.records
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
        self.saved.lock().unwrap_or_else(|e| e.into_inner()).clear();
        if !pp::is_dir(&self.root) {
            return failed;
        }
        let Ok(dir) = std::fs::read_dir(&self.root) else {
            return failed;
        };
        for entry in dir.flatten() {
            let child = pp::join(&self.root, &entry.file_name().to_string_lossy());
            if pp::is_dir(&child) && !pp::is_symlink(&child) {
                if trees_in(&child).iter().any(|t| !t.is_empty() && keep(t)) {
                    continue;
                }
                failed.extend(remove_tree(&child));
            } else if let Err(e) = std::fs::remove_file(&child) {
                if e.kind() != std::io::ErrorKind::NotFound {
                    failed.push(child);
                }
            }
        }
        failed
    }
}

fn dev_or_trash(path: &str) -> bool {
    let t = pp::tail(path);
    (pp::is_abs(path) && !path.starts_with("//") && t.first() == Some(&"dev"))
        || t.iter().any(|p| *p == ".Trash" || *p == ".Trashes")
}

fn load_record(text: &str) -> Option<Record> {
    let data = ferb_py::json::loads(text).ok()?;
    let m = data.as_object()?;
    let mut rec = Record::new(m.get("task_id")?.as_str()?, m.get("tree_id")?.as_str()?);
    for (k, v) in m.get("files")?.as_object()? {
        rec.files.insert(k.clone(), FileRec::from_json(v)?);
    }
    rec.folders = m.get("folders")?.as_object()?.clone();
    if let Some(named) = m.get("named") {
        for n in named.as_array()? {
            rec.named.insert(n.as_str()?.to_string());
        }
    }
    rec.n = m.get("n")?.as_i64()?;
    Some(rec)
}

/// `Path.mkdir(mode=0o700, parents=True, exist_ok=True)`: the folders above made with the default mode.
fn make_dirs(folder: &str) {
    let parent = pp::parent(folder);
    if parent != folder && !pp::exists(&parent) {
        make_dirs_default(&parent);
    }
    let _ = sys::mkdir_at(At::Cwd, folder, 0o700);
}

fn make_dirs_default(folder: &str) {
    let parent = pp::parent(folder);
    if parent != folder && !pp::exists(&parent) {
        make_dirs_default(&parent);
    }
    let _ = sys::mkdir_at(At::Cwd, folder, 0o777);
}

/// The `*/index.json` files of a tree folder.
fn indexes(folder: &str) -> Vec<String> {
    let Ok(dir) = std::fs::read_dir(folder) else {
        return Vec::new();
    };
    dir.flatten()
        .map(|e| pp::join_all(folder, [e.file_name().to_string_lossy().as_ref(), INDEX]))
        .filter(|p| pp::lexists(p))
        .collect()
}

/// `_trees_in`: the task trees a tree folder's records belong to.
fn trees_in(folder: &str) -> HashSet<String> {
    let mut out = HashSet::new();
    for index in indexes(folder) {
        let Some(data) = std::fs::read_to_string(&index)
            .ok()
            .and_then(|t| ferb_py::json::loads(&t).ok())
        else {
            continue;
        };
        let Some(m) = data.as_object() else { continue };
        out.insert(match m.get("tree_id") {
            None => String::new(),
            Some(Value::String(s)) => s.clone(),
            Some(other) => ferb_py::value::py_str(other),
        });
    }
    out
}

/// `_nearest`: the nearest folder above a missing path that exists.
fn nearest(path: &str) -> Option<String> {
    pp::parents(path)
        .into_iter()
        .find(|p| pp::is_dir(p) && !pp::is_symlink(p))
}

/// `_remove`: a folder of FERB's copies, a locked copy unlocked first (shutil.rmtree with a retry). Returns the paths
/// that stayed.
pub fn remove_tree(folder: &str) -> Vec<String> {
    let mut failed = Vec::new();
    // shutil.rmtree refuses a link (its onexc retry then only asks islink): nothing is removed or reported
    if pp::exists(folder) && !pp::is_symlink(folder) {
        rmtree(folder, &mut failed);
    }
    failed
}

fn retry(path: &str, f: &dyn Fn(&str) -> std::io::Result<()>, failed: &mut Vec<String>) {
    if f(path).is_err() {
        recovery::unlock(path);
        if f(path).is_err() {
            failed.push(path.to_string());
        }
    }
}

fn rmtree(folder: &str, failed: &mut Vec<String>) {
    if let Ok(dir) = std::fs::read_dir(folder) {
        for entry in dir.flatten() {
            let path = pp::join(folder, &entry.file_name().to_string_lossy());
            let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
            if is_dir {
                rmtree(&path, failed);
            } else {
                retry(&path, &|p| std::fs::remove_file(p), failed);
            }
        }
    }
    retry(folder, &|p| std::fs::remove_dir(p), failed);
}

/// `real(path)`: the file a path really reaches: its folder's links followed, the name kept. None for a NUL in it.
pub fn real(path: &str, home: &str) -> Option<String> {
    if path.contains('\0') {
        return None;
    }
    let p = pp::norm(&pp::os_expanduser(&pp::norm(path), home));
    let name = pp::name(&p);
    Some(if !name.is_empty() {
        pp::join(&pp::norm(&pp::realpath(&pp::parent(&p))), &name)
    } else {
        pp::norm(&pp::realpath(&p))
    })
}

impl Call<'_> {
    fn history_ids(&self) -> HashSet<(i64, i64)> {
        if let Some(h) = self.hist.borrow().as_ref() {
            return h.clone();
        }
        let mut ids = HashSet::new();
        let add = |ids: &mut HashSet<(i64, i64)>, path: &str| {
            let Ok(mut st) = sys::lstat(path) else { return };
            if st.is_lnk() {
                ids.insert((st.dev, st.ino as i64));
                match sys::stat(path) {
                    Ok(s) => st = s,
                    Err(_) => return,
                }
            }
            if st.is_reg() {
                ids.insert((st.dev, st.ino as i64));
            }
        };
        let home = &self.c.home;
        if let Ok(fd) = sys::open(
            At::Cwd,
            home,
            sys::O_RDONLY | sys::O_DIRECTORY | sys::O_CLOEXEC,
            0,
        ) {
            if let Ok(entries) = sys::scandir(fd.raw()) {
                for e in entries {
                    if history_name(&e.name) {
                        add(&mut ids, &scandir_path(home, &e.name));
                    }
                }
            }
        }
        for folder in &self.c.sessions {
            if let Ok(fd) = sys::open(
                At::Cwd,
                folder,
                sys::O_RDONLY | sys::O_DIRECTORY | sys::O_CLOEXEC,
                0,
            ) {
                if let Ok(entries) = sys::scandir(fd.raw()) {
                    for e in entries {
                        add(&mut ids, &scandir_path(folder, &e.name));
                    }
                }
            }
        }
        add(&mut ids, &self.c.fish);
        *self.hist.borrow_mut() = Some(ids.clone());
        ids
    }

    /// `_history_st`.
    fn history_st(&self, st: &St, opened: Option<&str>) -> bool {
        if self.history_ids().contains(&(st.dev, st.ino as i64)) {
            return true;
        }
        let sessions: HashSet<(i64, i64)> =
            self.c.sessions.iter().filter_map(|p| identity(p)).collect();
        let Some(opened) = opened else { return false };
        !sessions.is_empty()
            && pp::parents(opened)
                .iter()
                .any(|p| identity(p).is_some_and(|i| sessions.contains(&i)))
    }

    /// `_history`: a path names a history file.
    fn history(&self, path: &str) -> bool {
        if let Some(fd) = open_nofollow(path, false, None) {
            let Ok(st) = sys::fstat(fd.raw()) else {
                return false;
            };
            if !st.is_reg() {
                return false;
            }
            return match sys::opened_path(fd.raw()) {
                Ok(o) => self.history_st(&st, Some(&o)),
                Err(_) => false,
            };
        }
        if let Ok(st) = sys::lstat(path) {
            if st.is_reg() {
                return self.history_st(&st, None);
            }
        }
        let Some(folder) = identity(&pp::parent(path)) else {
            return false;
        };
        let sessions: Vec<Option<(i64, i64)>> =
            self.c.sessions.iter().map(|p| identity(p)).collect();
        if sessions.contains(&Some(folder)) {
            return true;
        }
        if Some(folder) == identity(&pp::parent(&self.c.fish)) {
            return crate::casefold(&pp::name(path)) == pp::name(&self.c.fish);
        }
        Some(folder) == identity(&self.c.home) && history_name(&pp::name(path))
    }

    /// `_scan`: a folder's own entries (lstat), listed through an fd opened without following a link.
    fn scan(&self, folder: &str, keep: bool) -> Option<Scan> {
        let fd = open_nofollow(folder, true, None)?;
        let where_ = self.c.opened(fd.raw())?;
        let list = sys::scandir(fd.raw()).ok()?;
        let mut entries = Vec::new();
        let mut index = HashMap::new();
        for e in list {
            let Ok(st) = sys::stat_at(At::Dir(fd.raw()), &e.name, false) else {
                continue;
            };
            index.insert(e.name.clone(), entries.len());
            entries.push((e.name, st));
        }
        let dev = sys::fstat(fd.raw()).ok()?.dev;
        Some(Scan {
            where_,
            dev,
            entries,
            index,
            fd: if keep { Some(fd) } else { None },
        })
    }

    /// `_list`: the folder's own entries, once per task.
    fn list(&self, rec: &mut Record, folder: &str, counts: &mut Counts) -> bool {
        if rec.folders.contains_key(folder) {
            return true;
        }
        if self.c.skip(folder) || !access(folder, libc::W_OK, true) {
            return false;
        }
        let Some(scan) = self.scan(folder, false) else {
            return false;
        };
        let mut listing = Map::new();
        for (name, st) in &scan.entries {
            listing.insert(
                name.clone(),
                json!([kind_code(st), st.size, st.mtime_ns() as i64, st.ino as i64]),
            );
        }
        rec.folders
            .insert(folder.to_string(), Value::Object(listing));
        counts.folders += 1;
        true
    }

    /// `_file`: record one named plain file, cloned from the fd checked as the file it really is.
    fn file(&self, rec: &mut Record, folder: &str, path: &str, counts: &mut Counts) {
        if rec.files.contains(path) || self.c.skip(path) {
            return;
        }
        let Some(fd) = open_nofollow(path, false, None) else {
            self.unreadable(rec, path, counts);
            return;
        };
        let Ok(st) = sys::fstat(fd.raw()) else { return };
        let opened = self.c.opened(fd.raw());
        if !st.is_reg() || opened.is_none() {
            counts.protected += if opened.is_some() { 0 } else { 1 };
            return;
        }
        let opened = opened.unwrap_or_default();
        if !(access(&opened, libc::W_OK, true) || access(&pp::parent(&opened), libc::W_OK, true)) {
            return;
        }
        rec.n += 1;
        let suffix = pp::suffix(path);
        let name = format!(
            "c{}{}",
            rec.n,
            if rx().suffix.is_match(&suffix) {
                suffix.as_str()
            } else {
                ""
            }
        );
        let mut entry = FileRec::new(true);
        entry.size = st.size;
        entry.mtime_ns = st.mtime_ns() as i64;
        entry.ino = st.ino as i64;
        entry.real = opened.clone();
        entry.history = self.history_st(&st, Some(&opened));
        entry.dev = st.dev;
        if !entry.history {
            let Ok(dir) = sys::open(
                At::Cwd,
                folder,
                sys::O_RDONLY | sys::O_DIRECTORY | sys::O_CLOEXEC,
                0,
            ) else {
                return;
            };
            hooks::at(&format!("changes.open_dir cwd|{folder}"));
            match recovery::fclone(fd.raw(), dir.raw(), &name) {
                Ok(()) => {
                    recovery::unlock(&pp::join(folder, &name));
                    entry.copy = Some(name);
                    counts.copied += 1;
                }
                Err(e) => {
                    entry.no_copy = if e.errno == libc::EXDEV || e.errno == libc::ENOTSUP {
                        "this volume can't make an instant copy".into()
                    } else {
                        format!("FERB couldn't copy it ({})", e.strerror_or_str())
                    };
                    counts.not_copied += 1;
                }
            }
        }
        rec.files.insert(path.to_string(), entry);
    }

    /// `_unreadable`: a named plain file FERB can't open: its facts from lstat, no copy.
    fn unreadable(&self, rec: &mut Record, path: &str, counts: &mut Counts) {
        let Ok(st) = sys::lstat(path) else { return };
        if !st.is_reg()
            || !(access(path, libc::W_OK, false) || access(&pp::parent(path), libc::W_OK, true))
        {
            return;
        }
        let mut f = FileRec::new(true);
        f.size = st.size;
        f.mtime_ns = st.mtime_ns() as i64;
        f.ino = st.ino as i64;
        f.no_copy = UNREADABLE.into();
        f.history = self.history_st(&st, None);
        f.dev = st.dev;
        rec.files.insert(path.to_string(), f);
        counts.not_copied += 1;
    }

    fn check(
        &self,
        rec: &Record,
        folder: &str,
        scans: &HashMap<String, Option<Scan>>,
    ) -> Vec<Change> {
        let mut look = Look {
            seen: rec.files.order.iter().cloned().collect(),
            out: Vec::new(),
            moved: HashMap::new(),
            renamed: HashSet::new(),
            held: HashSet::new(),
            names: 0,
            entries: 0,
            more: HashMap::new(),
            stopped: HashSet::new(),
            deep: HashSet::new(),
        };
        let mut found: Vec<(String, Option<Change>)> = Vec::new();
        for (key, f) in rec.files.iter() {
            let compared = self.compare(key, f, folder);
            if f.existed && gone(rec, key, f, scans) {
                look.moved.insert((f.dev, f.ino), key.clone());
                if compared.as_ref().is_none_or(|c| c.what != "removed") {
                    look.held.insert(key.clone());
                }
            }
            found.push((key.clone(), compared));
        }
        let recorded: HashSet<(i64, i64)> = rec
            .files
            .iter()
            .filter(|(_, f)| f.existed)
            .map(|(_, f)| (f.dev, f.ino))
            .collect();
        for (where_, entries) in rec.folders.iter() {
            let entries = entries.as_object().cloned().unwrap_or_default();
            let scan = scans.get(where_).and_then(Option::as_ref);
            let Some(scan) = scan else {
                match sys::lstat(where_) {
                    Ok(here) => look.out.push(Change {
                        content: false,
                        note: what_now(&here, where_),
                        ..Change::new(where_, "modified")
                    }),
                    Err(e) if e.errno == libc::ENOENT => look.out.push(Change {
                        content: false,
                        note: "the folder is gone".into(),
                        ..Change::new(where_, "removed")
                    }),
                    Err(_) => {}
                }
                continue;
            };
            let mut names: BTreeSet<&String> = entries.keys().collect();
            names.extend(scan.entries.iter().map(|(n, _)| n));
            for name in names {
                let path = pp::join(where_, name);
                if look.seen.contains(&path) {
                    continue;
                }
                look.seen.insert(path.clone());
                let old = entries.get(name).and_then(Value::as_array);
                let oldv: Option<Vec<i64>> =
                    old.map(|a| a.iter().map(|v| v.as_i64().unwrap_or(i64::MIN)).collect());
                let st = scan.get(name);
                if let Some(o) = &oldv {
                    let ino = o.get(3).copied().unwrap_or(i64::MIN);
                    let known = recorded.contains(&(scan.dev, ino)) || recorded.contains(&(0, ino));
                    if known && st.is_none_or(|s| s.ino as i64 == ino) {
                        continue;
                    }
                }
                let real_path = pp::join(&scan.where_, name);
                match st {
                    None => {
                        if oldv.as_ref().is_some_and(|o| o.first() == Some(&1)) {
                            self.unnamed(
                                &mut look,
                                where_,
                                &real_path,
                                || Change::fact(&path, "removed", None),
                                true,
                            );
                        }
                    }
                    Some(st) if st.is_dir() && oldv.is_none() => {
                        self.made_under(&mut look, rec, folder, &path, where_, scan.fd());
                    }
                    Some(st) if st.is_reg() && oldv.is_none() => {
                        let now = *st;
                        self.made(
                            &mut look,
                            rec,
                            folder,
                            &path,
                            &real_path,
                            where_,
                            st.dev,
                            st.ino as i64,
                            &|| Ok(now),
                        );
                    }
                    Some(st)
                        if st.is_reg()
                            && Some(vec![1, st.size, st.mtime_ns() as i64, st.ino as i64])
                                != oldv =>
                    {
                        let changed = *st;
                        self.unnamed(
                            &mut look,
                            where_,
                            &real_path,
                            || Change::fact(&path, "modified", Some(&changed)),
                            false,
                        );
                    }
                    _ => {}
                }
            }
        }
        let missing: Vec<String> = rec
            .named
            .iter()
            .filter(|k| !look.seen.contains(*k))
            .cloned()
            .collect();
        for key in missing {
            self.named_now(&mut look, rec, folder, &key);
        }
        for where_ in rec.folders.keys() {
            let mut parts = Vec::new();
            let more = look.more.get(where_).copied().unwrap_or(0);
            if more > 0 {
                parts.push(format!(
                    "and {} more new or changed files here and in folders made here, not listed",
                    commas(more)
                ));
            }
            if look.stopped.contains(where_) {
                parts.push(format!(
                    "more than {} entries in folders made during the step (and files changed in place): FERB stopped \
                     looking there",
                    commas(WALK_MAX as i64)
                ));
            }
            if look.deep.contains(where_) {
                parts.push(format!(
                    "folders nested more than {WALK_DEPTH} deep were not walked"
                ));
            }
            if !parts.is_empty() {
                look.out.push(Change {
                    content: false,
                    note: parts.join("; "),
                    ..Change::new(where_, "modified")
                });
            }
        }
        let mut out: Vec<Change> = found
            .into_iter()
            .filter_map(|(key, c)| {
                let c = c?;
                (!(look.renamed.contains(&key) && !look.held.contains(&key))).then_some(c)
            })
            .collect();
        out.append(&mut look.out);
        let all = out.clone();
        out.into_iter()
            .map(|c| {
                if c.what == "created" && c.content {
                    self.paired(c, &all, folder, rec)
                } else {
                    c
                }
            })
            .collect()
    }

    fn unnamed(
        &self,
        look: &mut Look,
        where_: &str,
        real_path: &str,
        change: impl FnOnce() -> Change,
        shown: bool,
    ) {
        if look.names < FOLDER_FACTS {
            if !self.c.closed_as_written(real_path) {
                look.names += 1;
                look.out.push(change());
            }
            return;
        }
        if !shown {
            if look.entries >= WALK_MAX {
                look.stopped.insert(where_.to_string());
                return;
            }
            look.entries += 1;
            if self.c.closed_as_written(real_path) {
                return;
            }
        }
        *look.more.entry(where_.to_string()).or_insert(0) += 1;
    }

    #[allow(clippy::too_many_arguments)]
    fn made(
        &self,
        look: &mut Look,
        rec: &Record,
        folder: &str,
        path: &str,
        real_path: &str,
        where_: &str,
        dev: i64,
        ino: i64,
        stat_of: &dyn Fn() -> sys::Res<St>,
    ) {
        let was = look
            .moved
            .get(&(dev, ino))
            .or_else(|| look.moved.get(&(0, ino)))
            .cloned();
        if was.is_none() && !rec.named.contains(path) {
            if look.names >= FOLDER_FACTS {
                *look.more.entry(where_.to_string()).or_insert(0) += 1;
                return;
            }
            if self.c.closed_as_written(real_path) {
                return;
            }
            let Ok(st) = stat_of() else { return };
            if st.is_reg() {
                look.names += 1;
                look.out.push(Change::fact(path, "created", Some(&st)));
            }
            return;
        }
        if self.c.skip(path) || self.c.closed_as_written(real_path) {
            return;
        }
        let Ok(st) = stat_of() else { return };
        if !st.is_reg() {
            return;
        }
        if let Some(was) = was {
            if look.moved.remove(&(dev, ino)).is_none() {
                look.moved.remove(&(0, ino));
            }
            look.renamed.insert(was.clone());
            let held = look.held.contains(&was);
            let f = rec
                .files
                .get(&was)
                .expect("a moved file is recorded")
                .clone();
            look.out
                .push(self.renamed(path, &st, &was, &f, folder, held));
        } else {
            look.out.push(self.new_file(rec, path, &st));
        }
    }

    fn named_now(&self, look: &mut Look, rec: &Record, folder: &str, path: &str) {
        if self.c.skip(path) {
            return;
        }
        let Some(fd) = open_nofollow(path, false, None) else {
            let Ok(st) = sys::lstat(path) else { return };
            if st.is_reg() {
                look.out.push(Change {
                    content: false,
                    named: true,
                    size: Some(st.size),
                    mtime_ns: Some(st.mtime_ns() as i64),
                    note: UNREADABLE.into(),
                    ..Change::new(path, "created")
                });
            }
            return;
        };
        let Ok(st) = sys::fstat(fd.raw()) else { return };
        let Some(opened) = self.c.opened(fd.raw()) else {
            return;
        };
        if !st.is_reg() {
            return;
        }
        look.seen.insert(path.to_string());
        self.made(
            look,
            rec,
            folder,
            path,
            &opened,
            &pp::parent(path),
            st.dev,
            st.ino as i64,
            &|| Ok(st),
        );
    }

    fn new_file(&self, rec: &Record, path: &str, st: &St) -> Change {
        if !rec.named.contains(path) {
            return Change::fact(path, "created", Some(st));
        }
        let made = Change {
            size: Some(st.size),
            mtime_ns: Some(st.mtime_ns() as i64),
            named: true,
            ..Change::new(path, "created")
        };
        if self.history(path) {
            return Change {
                content: false,
                note: HISTORY_NOTE.into(),
                ..made
            };
        }
        if open_nofollow(path, false, None).is_none() {
            return Change {
                content: false,
                note: UNREADABLE.into(),
                ..made
            };
        }
        made
    }

    fn renamed(
        &self,
        path: &str,
        st: &St,
        was: &str,
        f: &FileRec,
        folder: &str,
        held: bool,
    ) -> Change {
        let made = Change {
            source: Some(was.into()),
            named: true,
            size: Some(st.size),
            mtime_ns: Some(st.mtime_ns() as i64),
            ..Change::new(path, "renamed")
        };
        if f.history || self.history(path) {
            return Change {
                content: false,
                note: HISTORY_NOTE.into(),
                ..made
            };
        }
        if f.no_copy == UNREADABLE {
            return Change {
                content: false,
                note: UNREADABLE.into(),
                ..made
            };
        }
        if !f.real.is_empty() && self.c.closed_as_written(&f.real) {
            return Change {
                content: false,
                note: "closed to FERB's shell now".into(),
                ..made
            };
        }
        let copy = f.copy.as_ref().map(|c| pp::join(folder, c));
        if (st.size, st.mtime_ns() as i64) == (f.size, f.mtime_ns)
            || copy.as_ref().is_some_and(|c| same(path, c))
        {
            return Change {
                content: false,
                note: UNCHANGED.into(),
                original: held,
                ..made
            };
        }
        Change {
            before: copy,
            note: f.no_copy.clone(),
            before_real: (!f.real.is_empty()).then(|| f.real.clone()),
            ..made
        }
    }

    fn made_under(
        &self,
        look: &mut Look,
        rec: &Record,
        folder: &str,
        top: &str,
        where_: &str,
        parent: Option<i32>,
    ) {
        if look.entries >= WALK_MAX {
            look.stopped.insert(where_.to_string());
            return;
        }
        look.entries += 1;
        let Some(parent) = parent else { return };
        let Some(fd) = open_nofollow(&pp::name(top), true, Some(parent)) else {
            return;
        };
        let Some(opened) = self.c.opened(fd.raw()) else {
            return;
        };
        type Item = (String, bool, bool, i64);
        let mut stack: Vec<(String, String, Fd, i64, std::vec::IntoIter<Item>)> = Vec::new();
        let enter = |stack: &mut Vec<(String, String, Fd, i64, std::vec::IntoIter<Item>)>,
                     path: String,
                     real_path: String,
                     dir: Fd| {
            let Ok(dev) = sys::fstat(dir.raw()).map(|s| s.dev) else {
                return;
            };
            let Ok(list) = sys::scandir(dir.raw()) else {
                return;
            };
            let mut items: Vec<Item> = list
                .into_iter()
                .map(|e| {
                    let (is_dir, is_file) = entry_kind(dir.raw(), &e);
                    (e.name, is_dir, is_file, e.ino as i64)
                })
                .collect();
            items.sort();
            stack.push((path, real_path, dir, dev, items.into_iter()));
        };
        enter(&mut stack, top.to_string(), opened, fd);
        while let Some(last) = stack.last_mut() {
            let item = last.4.next();
            let Some((name, is_dir, is_file, ino)) = item else {
                stack.pop();
                continue;
            };
            if look.entries >= WALK_MAX {
                look.stopped.insert(where_.to_string());
                return;
            }
            look.entries += 1;
            let (path, real_path, dir_raw, dev) =
                (last.0.clone(), last.1.clone(), last.2.raw(), last.3);
            if is_dir {
                if stack.len() >= WALK_DEPTH {
                    look.deep.insert(where_.to_string());
                    continue;
                }
                let Some(sub) = open_nofollow(&name, true, Some(dir_raw)) else {
                    continue;
                };
                let Some(sub_real) = self.c.opened(sub.raw()) else {
                    continue;
                };
                enter(&mut stack, pp::join(&path, &name), sub_real, sub);
            } else if is_file {
                let key = pp::join(&path, &name);
                if look.seen.contains(&key) {
                    continue;
                }
                look.seen.insert(key.clone());
                let n = name.clone();
                self.made(
                    look,
                    rec,
                    folder,
                    &key,
                    &pp::join(&real_path, &name),
                    where_,
                    dev,
                    ino,
                    &move || sys::stat_at(At::Dir(dir_raw), &n, false),
                );
            }
        }
    }

    fn compare(&self, path: &str, f: &FileRec, folder: &str) -> Option<Change> {
        let st = match sys::lstat(path) {
            Ok(st) => Some(st),
            Err(e) if e.errno == libc::ENOENT => None,
            Err(_) => return None,
        };
        let copy = f.copy.as_ref().map(|c| pp::join(folder, c));
        let named = |c: Change| Change { named: true, ..c };
        if let Some(st) = &st {
            if !st.is_reg() {
                if !f.existed {
                    return None;
                }
                return Some(named(Change {
                    content: false,
                    note: what_now(st, path),
                    ..Change::new(path, "modified")
                }));
            }
        }
        if !f.real.is_empty() && self.c.closed_as_written(&f.real) {
            return Some(named(Change {
                content: false,
                note: "closed to FERB's shell now".into(),
                ..Change::new(path, "modified")
            }));
        }
        let before_real = (!f.real.is_empty()).then(|| f.real.clone());
        if !f.existed {
            let st = st?;
            return Some(named(Change {
                content: !f.history,
                size: Some(st.size),
                mtime_ns: Some(st.mtime_ns() as i64),
                note: if f.history {
                    HISTORY_NOTE.into()
                } else {
                    String::new()
                },
                ..Change::new(path, "created")
            }));
        }
        let history = f.history || (st.is_some() && self.history(path));
        if history || f.no_copy == UNREADABLE {
            let note = if history { HISTORY_NOTE } else { UNREADABLE };
            let Some(st) = st else {
                return Some(named(Change {
                    content: false,
                    note: note.into(),
                    ..Change::new(path, "removed")
                }));
            };
            if (st.size, st.mtime_ns() as i64, st.ino as i64) == (f.size, f.mtime_ns, f.ino) {
                return None;
            }
            return Some(named(Change {
                content: false,
                size: Some(st.size),
                mtime_ns: Some(st.mtime_ns() as i64),
                note: note.into(),
                ..Change::new(path, "modified")
            }));
        }
        let Some(st) = st else {
            return Some(named(Change {
                before: copy,
                note: f.no_copy.clone(),
                before_real,
                ..Change::new(path, "removed")
            }));
        };
        if (st.size, st.mtime_ns() as i64, st.ino as i64) == (f.size, f.mtime_ns, f.ino) {
            return None;
        }
        if copy.as_ref().is_some_and(|c| same(path, c)) {
            return None;
        }
        Some(named(Change {
            before: copy,
            note: f.no_copy.clone(),
            size: Some(st.size),
            mtime_ns: Some(st.mtime_ns() as i64),
            before_real,
            ..Change::new(path, "modified")
        }))
    }

    fn paired(&self, made: Change, all: &[Change], folder: &str, rec: &Record) -> Change {
        let path = made.path.clone();
        let kind = kind_of(&path);
        let suffix = lower(&pp::suffix(&path));
        let Ok(size) = sys::lstat(&path).map(|s| s.size) else {
            return made;
        };
        let my_stem = lower(&pp::stem(&path));
        let likeness = |other: &str| str_ratio(&lower(&pp::stem(other)), &my_stem);
        let in_range =
            |n: i64| (size as f64) / (PAIR_SIZE as f64) <= n as f64 && n <= size * PAIR_SIZE;
        // (rank, original, readable, current, real)
        let mut own: Vec<Candidate<(i64, f64)>> = Vec::new();
        for (key, f) in rec.files.iter() {
            let Some(copy) = &f.copy else { continue };
            if *key != made.path
                && kind_of(key) == kind
                && in_range(f.size)
                && !(!f.real.is_empty() && self.c.closed_as_written(&f.real))
            {
                let alike = likeness(key);
                let rank = if alike >= SIMILAR {
                    (0, -alike)
                } else {
                    (1, (f.size - size).abs() as f64)
                };
                own.push((
                    rank,
                    key.clone(),
                    pp::join(folder, copy),
                    false,
                    (!f.real.is_empty()).then(|| f.real.clone()),
                ));
            }
        }
        own.sort_by(|a, b| {
            a.0 .0
                .cmp(&b.0 .0)
                .then(
                    a.0 .1
                        .partial_cmp(&b.0 .1)
                        .unwrap_or(std::cmp::Ordering::Equal),
                )
                .then(a.1.cmp(&b.1))
        });
        let mut picks: Vec<(String, String, bool, Option<String>)> = own
            .into_iter()
            .take(PAIR_READS)
            .map(|c| (c.1, c.2, c.3, c.4))
            .collect();
        let changed: HashSet<&str> = all.iter().map(|c| c.path.as_str()).collect();
        let mut siblings: Vec<(f64, String, String, bool, Option<String>)> = Vec::new();
        let parent = pp::parent(&path);
        if let Some(dir) = open_nofollow(&parent, true, None) {
            if self.c.opened(dir.raw()).is_some() {
                if let Ok(entries) = sys::scandir(dir.raw()) {
                    for e in entries {
                        let sibling = pp::join(&parent, &e.name);
                        if lower(&pp::suffix(&sibling)) != suffix
                            || changed.contains(sibling.as_str())
                            || sibling == path
                            || rec.files.contains(&sibling)
                            || !entry_kind(dir.raw(), &e).1
                        {
                            continue;
                        }
                        let Ok(est) = sys::stat_at(At::Dir(dir.raw()), &e.name, false) else {
                            break; // Python: an OSError ends the listing
                        };
                        if in_range(est.size) {
                            let alike = likeness(&sibling);
                            if alike >= SIMILAR {
                                siblings.push((alike, sibling.clone(), sibling, true, None));
                            }
                        }
                    }
                }
            }
        }
        siblings.sort_by(|a, b| {
            b.0.partial_cmp(&a.0)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(a.1.cmp(&b.1))
        });
        let room = PAIR_READS.saturating_sub(picks.len());
        picks.extend(
            siblings
                .into_iter()
                .take(room)
                .map(|c| (c.1, c.2, c.3, c.4)),
        );
        if picks.is_empty() {
            return made;
        }
        let Some(new) = self.c.text_of(&path) else {
            return made;
        };
        if new.0.is_empty() {
            return made;
        }
        let lines = splitlines(&new.0);
        let mut best: Option<(f64, String, String, Option<String>)> = None;
        for (original, source, current, real_from) in picks {
            if current && (self.c.skip(&source) || self.history(&source)) {
                continue;
            }
            let old = if current {
                self.c.text_of(&source)
            } else {
                read_text(&source, None, self.c.pdf())
            };
            let Some(old) = old else { continue };
            if old.0.is_empty() {
                continue;
            }
            let old_lines = splitlines(&old.0);
            let mut m = SequenceMatcher::new(&old_lines, &lines, false);
            if m.real_quick_ratio() < SIMILAR || m.quick_ratio() < SIMILAR {
                continue;
            }
            let ratio = m.ratio();
            if ratio >= SIMILAR && best.as_ref().is_none_or(|b| ratio > b.0) {
                best = Some((ratio, original, source, real_from));
            }
        }
        match best {
            None => made,
            Some((_, original, source, real_from)) => Change {
                before: Some(source),
                source: Some(original),
                before_real: real_from,
                ..made
            },
        }
    }
}

/// `DirEntry.is_dir/is_file(follow_symlinks=False)` on an fd listing: the entry's d_type, else an lstat.
fn entry_kind(dir: i32, e: &sys::Entry) -> (bool, bool) {
    match e.dtype {
        libc::DT_DIR => (true, false),
        libc::DT_REG => (false, true),
        libc::DT_UNKNOWN => match sys::stat_at(At::Dir(dir), &e.name, false) {
            Ok(st) => (st.is_dir(), st.is_reg()),
            Err(_) => (false, false),
        },
        _ => (false, false),
    }
}

/// `os.scandir(path)`'s `entry.path`.
fn scandir_path(folder: &str, name: &str) -> String {
    if folder.ends_with('/') {
        format!("{folder}{name}")
    } else {
        format!("{folder}/{name}")
    }
}

/// `_gone`: a recorded file's own name no longer holds it.
fn gone(rec: &Record, path: &str, f: &FileRec, scans: &HashMap<String, Option<Scan>>) -> bool {
    let parent = pp::parent(path);
    let old = rec.listing(&parent);
    let scan = scans.get(&parent).and_then(Option::as_ref);
    if let (Some(old), Some(scan)) = (old, scan) {
        let names: Vec<&String> = old
            .iter()
            .filter(|(_, e)| {
                let a = e.as_array();
                a.and_then(|a| a.first()).and_then(Value::as_i64) == Some(1)
                    && a.and_then(|a| a.get(3)).and_then(Value::as_i64) == Some(f.ino)
            })
            .map(|(n, _)| n)
            .collect();
        let own = pp::name(path);
        let name = if names.iter().any(|n| **n == own) || names.is_empty() {
            own
        } else {
            names[0].clone()
        };
        return scan.get(&name).is_none_or(|st| st.ino as i64 != f.ino);
    }
    match sys::lstat(path) {
        Ok(st) => st.ino as i64 != f.ino,
        Err(e) if e.errno == libc::ENOENT => true,
        Err(_) => false,
    }
}

/// Python's `{n:,}`.
pub fn commas(n: i64) -> String {
    let digits = n.unsigned_abs().to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    if n < 0 {
        format!("-{out}")
    } else {
        out
    }
}

/// `_read`: a plain file's bytes, opened without following a link and non-blocking, checked as a plain file before
/// any read; with `closed`, only when the file it really opens isn't closed. None when it is a link, not a plain
/// file, closed, larger than FILE_MAX, or unreadable.
pub fn read_file(
    path: &str,
    closed: Option<&dyn Fn(&str) -> bool>,
    limit: Option<usize>,
) -> Option<Vec<u8>> {
    hooks::at(&format!("changes.read {path}"));
    let out = read_inner(path, closed, limit);
    hooks::at(&format!("changes.read_done {path}"));
    out
}

fn read_inner(
    path: &str,
    closed: Option<&dyn Fn(&str) -> bool>,
    limit: Option<usize>,
) -> Option<Vec<u8>> {
    let fd = sys::open(At::Cwd, path, NOFOLLOW, 0).ok()?;
    let st = sys::fstat(fd.raw()).ok()?;
    if !st.is_reg() || st.size > FILE_MAX {
        return None;
    }
    if let Some(closed) = closed {
        if closed(&sys::opened_path(fd.raw()).ok()?) {
            return None;
        }
    }
    let mut left = limit.unwrap_or(st.size as usize + 1);
    let mut out = Vec::new();
    while left > 0 {
        let block = sys::read(fd.raw(), left.min(1 << 20)).ok()?;
        if block.is_empty() {
            break;
        }
        left = left.saturating_sub(block.len());
        out.extend_from_slice(&block);
    }
    Some(out)
}

/// `_same`: whether a current plain file has the same bytes as FERB's copy (both non-blocking, never through a link).
pub fn same(a: &str, b: &str) -> bool {
    let Ok(fa) = sys::open(At::Cwd, a, NOFOLLOW, 0) else {
        return false;
    };
    let Ok(fb) = sys::open(At::Cwd, b, NOFOLLOW, 0) else {
        return false;
    };
    let (Ok(sa), Ok(sb)) = (sys::fstat(fa.raw()), sys::fstat(fb.raw())) else {
        return false;
    };
    if !(sa.is_reg() && sb.is_reg()) || sa.size != sb.size {
        return false;
    }
    loop {
        let (Ok(x), Ok(y)) = (sys::read(fa.raw(), 1 << 20), sys::read(fb.raw(), 1 << 20)) else {
            return false;
        };
        if x != y {
            return false;
        }
        if x.is_empty() {
            return true;
        }
    }
}

/// `_text`: what a plain file says, for comparing. None for a file with no text FERB can read.
pub fn read_text(
    path: &str,
    closed: Option<&dyn Fn(&str) -> bool>,
    pdf: Option<&dyn PdfText>,
) -> Option<Text> {
    let suffix = lower(&pp::suffix(path));
    if DOCUMENTS.contains(&suffix.as_str()) {
        let data = read_file(path, closed, None)?;
        if suffix == ".pdf" {
            return pdf?.text(&data);
        }
        return crate::docx::docx(&data).map(|t| (t, Vec::new()));
    }
    let data = read_file(path, closed, Some(TEXT_MAX * 4 + 1))?;
    if data[..data.len().min(8192)].contains(&0) {
        return None;
    }
    String::from_utf8(data).ok().map(|t| (t, Vec::new()))
}

/// The module's `text_of`: what a file says (no link followed).
pub fn text_of(path: &str, pdf: Option<&dyn PdfText>) -> Option<Text> {
    read_text(path, None, pdf)
}

fn annotations(old: &[Annotation], new: &[Annotation]) -> String {
    fn minus(a: &[Annotation], b: &[Annotation]) -> Vec<Annotation> {
        let mut left: HashMap<&Annotation, i64> = HashMap::new();
        for x in b {
            *left.entry(x).or_insert(0) += 1;
        }
        let mut out = Vec::new();
        for x in a {
            let n = left.entry(x).or_insert(0);
            if *n > 0 {
                *n -= 1;
            } else {
                out.push(x.clone());
            }
        }
        out
    }
    let shown = |mut items: Vec<Annotation>| -> String {
        items.sort();
        items
            .iter()
            .map(|(p, t, x)| {
                let text = if x.is_empty() {
                    String::new()
                } else {
                    format!(" {}", ferb_py::json::dumps_utf8(&Value::String(x.clone())))
                };
                format!("page {p}: {t}{text}")
            })
            .collect::<Vec<_>>()
            .join("; ")
    };
    let (added, removed) = (minus(new, old), minus(old, new));
    let mut parts = Vec::new();
    if !added.is_empty() {
        let n = added.len();
        parts.push(format!(
            "{n} annotation{} added ({})",
            if n != 1 { "s" } else { "" },
            shown(added)
        ));
    }
    if !removed.is_empty() {
        let n = removed.len();
        parts.push(format!(
            "{n} annotation{} removed ({})",
            if n != 1 { "s" } else { "" },
            shown(removed)
        ));
    }
    parts.join(", ")
}

/// `_when`: ", modified <local time, to the second>".
fn when(ns: Option<i64>) -> String {
    let Some(ns) = ns else { return String::new() };
    let d = ns as f64 / 1e9;
    let (mut secs, frac) = (d.trunc(), d.fract());
    let mut us = round_half_even(frac * 1e6);
    if us >= 1e6 {
        us -= 1e6;
        secs += 1.0;
    } else if us < 0.0 {
        secs -= 1.0;
    }
    let _ = us;
    use chrono::{Local, Offset, TimeZone};
    let Some(t) = Local.timestamp_opt(secs as i64, 0).single() else {
        return String::new();
    };
    let off = t.offset().fix().local_minus_utc();
    let (sign, abs) = if off < 0 { ('-', -off) } else { ('+', off) };
    let mut zone = format!("{sign}{:02}:{:02}", abs / 3600, abs % 3600 / 60);
    if abs % 60 != 0 {
        zone.push_str(&format!(":{:02}", abs % 60));
    }
    format!(", modified {}{zone}", t.format("%Y-%m-%dT%H:%M:%S"))
}

fn round_half_even(x: f64) -> f64 {
    let r = x.round();
    if (x - r).abs() == 0.5 {
        2.0 * (x / 2.0).round()
    } else {
        r
    }
}

fn char_prefix(s: &str, n: usize) -> &str {
    match s.char_indices().nth(n) {
        Some((i, _)) => &s[..i],
        None => s,
    }
}

/// One change as the checker reads it (changes.py `describe`). `read` reads the current file; `base` reads `before`
/// when it isn't FERB's own copy (None: it is, read as is).
pub fn describe(
    change: &Change,
    read: &dyn Fn(&str) -> Option<Text>,
    base: Option<ReadFn<'_>>,
    pdf: Option<&dyn PdfText>,
) -> String {
    let path = &change.path;
    let source = change.source.clone().unwrap_or_else(|| "None".into());
    let verb = match change.what.as_str() {
        "created" => "new".to_string(),
        "removed" => "removed".to_string(),
        "modified" => "changed".to_string(),
        "renamed" => format!("renamed from {source}"),
        other => other.to_string(),
    };
    let size = change
        .size
        .map(|s| format!(", {} bytes", commas(s)))
        .unwrap_or_default();
    if change.original {
        return format!(
            "{path}: holds the original of {source} (the file a call named, renamed here during the step; {source} \
             now holds a new file){size}{}; {}",
            when(change.mtime_ns),
            change.note
        );
    }
    if !change.content && change.named {
        let note = if change.note.is_empty() {
            "not a plain file now"
        } else {
            &change.note
        };
        return format!(
            "{path}: {verb}, named by the call{size}{}; {note}{}",
            when(change.mtime_ns),
            if change.note == UNCHANGED {
                ""
            } else {
                "; not read"
            }
        );
    }
    if !change.content {
        let how = if !change.note.is_empty() {
            change.note.clone()
        } else if change.what != "modified" || change.before.is_none() {
            "not shown: no call of this step named it; it may be the owner's or another task's"
                .into()
        } else {
            "not shown".into()
        };
        return format!(
            "{path}: {verb} during this step{size}{} ({how})",
            when(change.mtime_ns)
        );
    }
    if change.what == "removed" {
        let head = format!("{path}: removed (no longer at this path)");
        return if change.note.is_empty() {
            head
        } else {
            format!("{head}; {}", change.note)
        };
    }
    if change.what == "created" && change.source.is_none() {
        let Some(new) = read(path) else {
            return format!("{path}: new, named by this step (no text FERB can read)");
        };
        let cut = !DOCUMENTS.contains(&lower(&pp::suffix(path)).as_str())
            && new.0.chars().count() > PREVIEW_CHARS;
        let body = if cut {
            format!(
                "{}\n[… the file holds the rest: read it with fs_read]",
                char_prefix(&new.0, PREVIEW_CHARS)
            )
        } else {
            new.0.clone()
        };
        let notes = if new.1.is_empty() {
            String::new()
        } else {
            format!("\nPDF annotations: {}", annotations(&[], &new.1))
        };
        return format!(
            "{path}: new, named by this step. Its text{}:\n{body}{notes}",
            if cut { " (the first part)" } else { "" }
        );
    }
    let Some(before) = &change.before else {
        let note = if change.note.is_empty() {
            "FERB kept no copy from before"
        } else {
            &change.note
        };
        return format!("{path}: {verb}; no diff: {note}");
    };
    let old = match base {
        Some(b) => b(before),
        None => read_text(before, None, pdf),
    };
    let new = read(path);
    let head = match change.what.as_str() {
        "created" => format!(
            "{path}: new, named by this step; compared with {source} as it was before the step"
        ),
        "renamed" => format!(
            "{path}: {verb}, named by this step; compared with FERB's copy from before the step"
        ),
        _ => format!(
            "{path}: changed, named by this step; compared with FERB's copy from before the step"
        ),
    };
    let (Some(old), Some(new)) = (old, new) else {
        let mut sizes = String::new();
        let before_size = if base.is_none() {
            sys::stat(before).map(|s| format!("{} bytes before, ", commas(s.size)))
        } else {
            Ok(String::new())
        };
        if let Ok(b) = before_size {
            if let Ok(now) = sys::lstat(path) {
                sizes = format!(" ({b}{} now)", commas(now.size));
            }
        }
        return format!("{head}\nNo text FERB can read in it{sizes}; its bytes changed.");
    };
    if old.0.chars().count() > TEXT_MAX || new.0.chars().count() > TEXT_MAX {
        return format!("{head}\nIts text is too long to compare here; read the file itself.");
    }
    let lines = unified_diff(
        &splitlines(&old.0),
        &splitlines(&new.0),
        "before",
        "now",
        CONTEXT_LINES,
    );
    let notes = annotations(&old.1, &new.1);
    if lower(&pp::suffix(path)) == ".pdf" {
        let layer = if lines.is_empty() {
            "text layer unchanged"
        } else {
            "text layer changed"
        };
        let lead = if notes.is_empty() {
            "no annotations added or removed, ".to_string()
        } else {
            format!("{notes}, ")
        };
        let mut out = format!("{head}\nPDF: {lead}{layer}");
        if !lines.is_empty() {
            out.push('\n');
            out.push_str(&lines.join("\n"));
        }
        return out;
    }
    if lines.is_empty() {
        return format!(
            "{head}\nIts text is unchanged (only its formatting or other bytes changed)."
        );
    }
    format!("{head}\n{}", lines.join("\n"))
}

/// `shown(path)`: a path as the owner writes it (~ for the home folder), for an owner-facing line.
pub fn shown(path: &str, home: &str) -> String {
    if pp::is_relative_to(path, home) && !pp::same(path, home) {
        format!("~/{}", pp::relative_to(path, home))
    } else {
        pp::norm(path)
    }
}
