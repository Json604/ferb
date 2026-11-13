//! Installed-software facts (`ferb/sense/software.py`): signature, Gatekeeper assessment, entitlements, download
//! origin, privacy grants, launch jobs and login items of an app, without launching it or assigning intent. Running
//! `codesign`, `spctl` and `xattr` is the caller's (`run`, `xattr` seams; the Mac's are in [`crate::mac`]); nothing
//! here asks for authorization (`sfltool dumpbtm` is never started).

use std::path::Path;

use serde_json::{Map, Value};

use crate::paths;

/// `str.splitlines()` (Python's line boundaries).
pub fn py_splitlines(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut it = text.char_indices().peekable();
    while let Some((i, c)) = it.next() {
        let boundary = matches!(
            c,
            '\n' | '\r'
                | '\u{b}'
                | '\u{c}'
                | '\u{1c}'
                | '\u{1d}'
                | '\u{1e}'
                | '\u{85}'
                | '\u{2028}'
                | '\u{2029}'
        );
        if boundary {
            out.push(&text[start..i]);
            let mut next = i + c.len_utf8();
            if c == '\r' {
                if let Some((j, '\n')) = it.peek().copied() {
                    it.next();
                    next = j + 1;
                }
            }
            start = next;
        }
    }
    if start < text.len() {
        out.push(&text[start..]);
    }
    out
}

/// `bytes.decode(errors="replace")`.
pub fn decode(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// A plist value as Python's `plistlib.loads` gives it, written as JSON (dates as ISO text, data as hex).
pub fn plist_json(v: &plist::Value) -> Value {
    match v {
        plist::Value::Array(a) => Value::Array(a.iter().map(plist_json).collect()),
        plist::Value::Dictionary(d) => {
            Value::Object(d.iter().map(|(k, v)| (k.clone(), plist_json(v))).collect())
        }
        plist::Value::Boolean(b) => Value::Bool(*b),
        plist::Value::Data(d) => Value::from(hex::encode(d)),
        plist::Value::Date(d) => Value::from(d.to_xml_format()),
        plist::Value::Real(f) => Value::from(*f),
        plist::Value::Integer(i) => i
            .as_signed()
            .map(Value::from)
            .or_else(|| i.as_unsigned().map(Value::from))
            .unwrap_or(Value::Null),
        plist::Value::String(s) => Value::from(s.clone()),
        plist::Value::Uid(u) => Value::from(u.get()),
        _ => Value::Null,
    }
}

/// `plistlib.loads(bytes)`.
pub fn plist_loads(bytes: &[u8]) -> Result<plist::Value, String> {
    plist::Value::from_reader(std::io::Cursor::new(bytes)).map_err(|e| e.to_string())
}

fn unquote(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let (Some(h), Some(l)) = (
                (b[i + 1] as char).to_digit(16),
                (b[i + 2] as char).to_digit(16),
            ) {
                out.push((h * 16 + l) as u8);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// `urlparse(raw).path` for a `file:` URL.
fn url_path(raw: &str) -> String {
    let rest = &raw["file:".len()..];
    let rest = match rest.strip_prefix("//") {
        Some(r) => r.find(['/', '?', '#']).map_or("", |i| &r[i..]),
        None => rest,
    };
    let rest = rest.split('#').next().unwrap_or("");
    let rest = rest.split('?').next().unwrap_or("");
    // urlparse's params: ";..." after the last "/"
    let last = rest.rfind('/').unwrap_or(0);
    match rest[last..].find(';') {
        Some(j) => rest[..last + j].to_string(),
        None => rest.to_string(),
    }
}

fn inside(candidate: &str, bundle: &Path) -> bool {
    candidate.starts_with('/') && paths::within(candidate, &paths::text(bundle))
}

const LOGIN_KEYS: [&str; 11] = [
    "UUID",
    "Name",
    "Developer Name",
    "Team Identifier",
    "Type",
    "Flags",
    "Disposition",
    "Identifier",
    "URL",
    "Executable Path",
    "Parent Identifier",
];

/// software.py:72 `login_items`: match an already-collected background-task inventory; without one, say it wasn't
/// read (passive observation never starts an authorization-capable utility).
pub fn login_items(path: &Path, bundle_id: &str, inventory: Option<&[u8]>) -> Value {
    let mut out = Map::new();
    let Some(inventory) = inventory else {
        out.insert("status".into(), Value::from("unavailable"));
        out.insert(
            "reason".into(),
            Value::from("no_supported_noninteractive_source"),
        );
        out.insert(
            "detail".into(),
            Value::from(
                "Background-task inventory was not read; passive observation does not request administrator authorization",
            ),
        );
        return Value::Object(out);
    };
    let text = decode(inventory);
    let mut records: Vec<Map<String, Value>> = Vec::new();
    let mut current: Map<String, Value> = Map::new();
    for line in py_splitlines(&text) {
        let line = ferb_py::value::strip(line);
        let (key, sep, value) = match line.split_once(':') {
            Some((k, v)) => (k, true, v),
            None => (line, false, ""),
        };
        if key == "UUID" && !current.is_empty() {
            records.push(std::mem::take(&mut current));
        }
        if sep && LOGIN_KEYS.contains(&key) {
            current.insert(key.into(), Value::from(ferb_py::value::strip(value)));
        }
    }
    if !current.is_empty() {
        records.push(current);
    }
    if records.is_empty() {
        out.insert("status".into(), Value::from("unsupported_or_empty"));
        out.insert(
            "detail".into(),
            Value::from("No recognizable background-task records"),
        );
        return Value::Object(out);
    }
    let get = |r: &Map<String, Value>, k: &str| {
        r.get(k).and_then(Value::as_str).unwrap_or("").to_string()
    };
    let associated = |r: &Map<String, Value>| {
        if r.get("Identifier") == Some(&Value::from(bundle_id))
            || r.get("Parent Identifier") == Some(&Value::from(bundle_id))
        {
            return true;
        }
        ["Executable Path", "URL"].iter().any(|key| {
            let raw = get(r, key);
            let candidate = if raw.starts_with("file:") {
                unquote(&url_path(&raw))
            } else {
                raw
            };
            !candidate.is_empty() && inside(&candidate, path)
        })
    };
    out.insert("status".into(), Value::from("read"));
    out.insert(
        "items".into(),
        Value::Array(
            records
                .iter()
                .filter(|r| associated(r))
                .map(|r| Value::Object(r.clone()))
                .collect(),
        ),
    );
    out.insert(
        "association".into(),
        Value::from(
            "exact app or parent identifier, or path inside app bundle; not proof of installer causation",
        ),
    );
    Value::Object(out)
}

/// The launch-job folders (software.py:30), under `home`.
pub fn job_folders(home: &Path) -> Vec<std::path::PathBuf> {
    vec![
        home.join("Library/LaunchAgents"),
        "/Library/LaunchAgents".into(),
        "/Library/LaunchDaemons".into(),
    ]
}

/// The TCC databases (software.py:25), under `home`.
pub fn tcc_databases(home: &Path) -> Vec<std::path::PathBuf> {
    vec![
        home.join("Library/Application Support/com.apple.TCC/TCC.db"),
        "/Library/Application Support/com.apple.TCC/TCC.db".into(),
    ]
}

fn plists(folder: &Path) -> Vec<std::path::PathBuf> {
    let mut files: Vec<std::path::PathBuf> = std::fs::read_dir(folder)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .map(|n| n.as_encoded_bytes())
                .is_some_and(|n| n.ends_with(b".plist"))
        })
        .collect();
    files.sort();
    files
}

/// software.py:55 `associated_jobs`: launch jobs whose program is inside this app bundle.
///
/// Fixed against Python (ENG-6): a plist whose root isn't a dictionary raised `AttributeError` out of Python's
/// loop (only `OSError`, `ValueError`, `TypeError` and an invalid file were caught), which failed the whole software
/// watcher; here such a file is skipped like any other unreadable one.
pub fn associated_jobs(path: &Path, folders: &[std::path::PathBuf]) -> Vec<Value> {
    let mut jobs = Vec::new();
    for folder in folders {
        for file in plists(folder) {
            let Ok(bytes) = std::fs::read(&file) else {
                continue;
            };
            let Ok(plist::Value::Dictionary(data)) = plist_loads(&bytes) else {
                continue;
            };
            let args = match data.get("ProgramArguments") {
                None => Some(Vec::new()),
                Some(plist::Value::Array(a)) => Some(a.clone()),
                Some(_) => None,
            };
            let program: Option<plist::Value> = match data.get("Program") {
                Some(p) if truthy_plist(p) => Some(p.clone()),
                _ => match &args {
                    Some(a) if !a.is_empty() => Some(a[0].clone()),
                    Some(_) => Some(plist::Value::String(String::new())),
                    None => None, // `args[0]` of a non-list: Python's TypeError or a wrong element
                },
            };
            let Some(plist::Value::String(program)) = program else {
                continue; // Path(non-text): TypeError, skipped
            };
            if !program.is_empty() && paths::within(&paths::normal(&program), &paths::text(path)) {
                let mut job = Map::new();
                job.insert("path".into(), Value::from(paths::text(&file)));
                job.insert(
                    "label".into(),
                    data.get("Label").map_or(Value::Null, plist_json),
                );
                job.insert("program".into(), Value::from(program));
                job.insert(
                    "association".into(),
                    Value::from("executable inside this app bundle"),
                );
                jobs.push(Value::Object(job));
            }
        }
    }
    jobs
}

fn truthy_plist(v: &plist::Value) -> bool {
    match v {
        plist::Value::String(s) => !s.is_empty(),
        plist::Value::Array(a) => !a.is_empty(),
        plist::Value::Dictionary(d) => !d.is_empty(),
        plist::Value::Boolean(b) => *b,
        plist::Value::Integer(i) => i.as_signed() != Some(0),
        plist::Value::Real(f) => *f != 0.0,
        plist::Value::Data(d) => !d.is_empty(),
        _ => true,
    }
}

/// software.py:34 `change_stamp`: what changes when an app's privacy grants or launch jobs can (stat calls only).
pub fn change_stamp(
    databases: &[std::path::PathBuf],
    folders: &[std::path::PathBuf],
) -> Vec<String> {
    use std::os::unix::fs::MetadataExt;
    let mut out = Vec::new();
    for db in databases {
        let mut wal = db.clone().into_os_string();
        wal.push("-wal");
        for file in [db.clone(), wal.into()] {
            match std::fs::metadata(&file) {
                Ok(m) => out.push(format!(
                    "{}|{}|{}",
                    paths::text(&file),
                    m.mtime() as i128 * 1_000_000_000 + m.mtime_nsec() as i128,
                    m.len()
                )),
                Err(_) => out.push(format!("{}|None", paths::text(&file))),
            }
        }
    }
    for folder in folders {
        match std::fs::metadata(folder) {
            Ok(m) => {
                let mut entries: Vec<String> = Vec::new();
                for f in plists(folder) {
                    match std::fs::metadata(&f) {
                        Ok(fm) => entries.push(format!(
                            "{}:{}",
                            f.file_name()
                                .map(|n| n.to_string_lossy().into_owned())
                                .unwrap_or_default(),
                            fm.mtime() as i128 * 1_000_000_000 + fm.mtime_nsec() as i128
                        )),
                        Err(_) => {
                            out.push(format!("{}|None", paths::text(folder)));
                            entries.clear();
                            break;
                        }
                    }
                }
                out.push(format!(
                    "{}|{}|{}",
                    paths::text(folder),
                    m.mtime() as i128 * 1_000_000_000 + m.mtime_nsec() as i128,
                    entries.join(",")
                ));
            }
            Err(_) => out.push(format!("{}|None", paths::text(folder))),
        }
    }
    out
}

/// What a command answered: exit code, stdout, stderr.
pub type Answer = (i32, Vec<u8>, Vec<u8>);

/// Python's exception type name for an OS error.
pub fn os_error_name(e: &std::io::Error) -> &'static str {
    match e.kind() {
        std::io::ErrorKind::NotFound => "FileNotFoundError",
        std::io::ErrorKind::PermissionDenied => "PermissionError",
        _ => "OSError",
    }
}

const SIGNATURE_KEYS: [&str; 5] = [
    "Authority",
    "TeamIdentifier",
    "Identifier",
    "Signature",
    "Runtime Version",
];

/// The seams `app_security_facts` reads through.
pub struct Readers<'a> {
    /// run a program with arguments, capturing its output
    pub run: &'a dyn Fn(&[String]) -> std::io::Result<Answer>,
    /// an extended attribute's bytes (`Err`: none or unreadable)
    pub xattr: &'a dyn Fn(&Path, &str) -> std::io::Result<Vec<u8>>,
    pub privacy: &'a dyn Fn(&str, &Path) -> Value,
    pub jobs: &'a dyn Fn(&Path) -> Value,
}

/// software.py:118 `app_security_facts`.
pub fn app_security_facts(path: &Path, bundle_id: &str, r: &Readers<'_>) -> Value {
    let p = paths::text(path);
    let command = |args: &[&str]| -> Answer {
        let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        match (r.run)(&args) {
            Ok(a) => a,
            Err(e) => (-1, Vec::new(), os_error_name(&e).as_bytes().to_vec()),
        }
    };
    let both = |out: &[u8], err: &[u8]| decode(&[out, err].concat());
    let (code, out, err) = command(&["/usr/bin/codesign", "-dv", "--verbose=4", &p]);
    let mut signature = Map::new();
    signature.insert("exit_code".into(), Value::from(code));
    for line in py_splitlines(&both(&out, &err)) {
        if let Some((key, value)) = line.split_once('=') {
            if SIGNATURE_KEYS.contains(&key) {
                if key == "Authority" {
                    let list = signature
                        .entry("Authority")
                        .or_insert_with(|| Value::Array(Vec::new()));
                    if let Value::Array(a) = list {
                        a.push(Value::from(value));
                    }
                } else {
                    signature.insert(key.into(), Value::from(value));
                }
            }
        }
    }
    let (code, out, err) = command(&[
        "/usr/sbin/spctl",
        "--assess",
        "--type",
        "execute",
        "--verbose=4",
        &p,
    ]);
    let mut assessment = Map::new();
    assessment.insert("accepted".into(), Value::Bool(code == 0));
    assessment.insert("exit_code".into(), Value::from(code));
    assessment.insert("detail".into(), Value::from(both(&out, &err)));
    let (code, out, _) = command(&["/usr/bin/codesign", "-d", "--entitlements", ":-", &p]);
    let entitlements = if code == 0 && !out.is_empty() {
        match plist_loads(&out) {
            Ok(v) => plist_json(&v),
            Err(_) => Value::Object(
                [("status".to_string(), Value::from("unreadable"))]
                    .into_iter()
                    .collect(),
            ),
        }
    } else {
        Value::Object(Map::new())
    };
    let mut attributes = Map::new();
    for name in [
        "com.apple.quarantine",
        "com.apple.metadata:kMDItemWhereFroms",
    ] {
        let value = match (r.xattr)(path, name) {
            Err(_) => Value::Null,
            Ok(v) if name.contains("WhereFroms") && !v.is_empty() => match plist_loads(&v) {
                Ok(p) => plist_json(&p),
                Err(_) => Value::Null,
            },
            Ok(v) => Value::from(decode(&v)),
        };
        attributes.insert(name.into(), value);
    }
    let mut o = Map::new();
    o.insert("bundle_id".into(), Value::from(bundle_id));
    o.insert("path".into(), Value::from(p.clone()));
    o.insert("signature".into(), Value::Object(signature));
    o.insert("assessment".into(), Value::Object(assessment));
    o.insert("entitlements".into(), entitlements);
    o.insert("download".into(), Value::Object(attributes));
    o.insert("privacy".into(), (r.privacy)(bundle_id, path));
    o.insert("launch_jobs".into(), (r.jobs)(path));
    o.insert("login_items".into(), login_items(path, bundle_id, None));
    Value::Object(o)
}

/// software.py:9 `privacy_facts` for one TCC database: its grants for this app (by bundle id or path).
#[cfg(feature = "store")]
pub fn privacy_facts(bundle_id: &str, path: &Path, database: &Path) -> Value {
    use rusqlite::{Connection, OpenFlags};
    let mut o = Map::new();
    let read = || -> rusqlite::Result<Value> {
        let db = Connection::open_with_flags(
            database,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        let cols: Vec<String> = {
            let mut st = db.prepare("PRAGMA table_info(access)")?;
            let r = st.query_map([], |r| r.get::<_, String>(1))?;
            r.collect::<rusqlite::Result<_>>()?
        };
        let mut out = Map::new();
        if !["service", "client", "auth_value", "last_modified"]
            .iter()
            .all(|c| cols.iter().any(|x| x == c))
        {
            out.insert("status".into(), Value::from("unsupported_schema"));
            return Ok(Value::Object(out));
        }
        let mut st = db.prepare(
            "SELECT service,client,auth_value,last_modified FROM access WHERE client=? OR client=? ORDER BY service",
        )?;
        let grants: Vec<Value> = st
            .query_map(rusqlite::params![bundle_id, paths::text(path)], |r| {
                let mut g = Map::new();
                for (i, k) in ["service", "client", "auth_value", "last_modified"]
                    .iter()
                    .enumerate()
                {
                    g.insert(
                        k.to_string(),
                        sql_json(r.get::<_, rusqlite::types::Value>(i)?),
                    );
                }
                Ok(Value::Object(g))
            })?
            .collect::<rusqlite::Result<_>>()?;
        out.insert("status".into(), Value::from("read"));
        out.insert("grants".into(), Value::Array(grants));
        Ok(Value::Object(out))
    };
    match read() {
        Ok(v) => v,
        Err(e) => {
            o.insert("status".into(), Value::from("unavailable"));
            o.insert("error".into(), Value::from(sqlite_error_name(&e)));
            o.insert("requires".into(), Value::from("Full Disk Access"));
            Value::Object(o)
        }
    }
}

/// software.py:10: both TCC databases (the user's and the system's).
#[cfg(feature = "store")]
pub fn privacy_both(bundle_id: &str, path: &Path, home: &Path) -> Value {
    let dbs = tcc_databases(home);
    let mut o = Map::new();
    o.insert("user".into(), privacy_facts(bundle_id, path, &dbs[0]));
    o.insert("system".into(), privacy_facts(bundle_id, path, &dbs[1]));
    Value::Object(o)
}

#[cfg(feature = "store")]
fn sql_json(v: rusqlite::types::Value) -> Value {
    use rusqlite::types::Value as S;
    match v {
        S::Null => Value::Null,
        S::Integer(i) => Value::from(i),
        S::Real(f) => Value::from(f),
        S::Text(t) => Value::from(t),
        S::Blob(b) => Value::from(hex::encode(b)),
    }
}

/// Python's `sqlite3` exception class for an SQLite error.
#[cfg(feature = "store")]
pub fn sqlite_error_name(e: &rusqlite::Error) -> &'static str {
    use rusqlite::ErrorCode as C;
    match e {
        rusqlite::Error::SqliteFailure(f, _) => match f.code {
            C::NotADatabase | C::DatabaseCorrupt => "DatabaseError",
            C::ConstraintViolation => "IntegrityError",
            C::TooBig => "DataError",
            C::ApiMisuse => "InterfaceError",
            _ => "OperationalError",
        },
        _ => "DatabaseError",
    }
}
