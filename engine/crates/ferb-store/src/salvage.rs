//! The check of FERB's home before setup or the daemon use what an older FERB left there: a
//! port of `ferb/store/salvage.py` (decisions: STORE-1, STORE-2, STORE-3, STORE-4, DMN-10).
//! The owner's data that can't be read is never deleted: it is moved aside with a timestamp,
//! FERB starts that file fresh, and the move is kept in `set-aside.json` until the daemon has
//! journaled it and told the owner once. A database a newer FERB migrated isn't damaged
//! (`Error::NewerStore`, nothing changed). An error that isn't damage (the file locked,
//! unreadable, a disk error) is returned as it is: never a reason to move anything.
//!
//! Seams (`Seams`): the clock (`datetime.now`), and whether `config.toml` parses, which is the
//! config port's to say (Python asks `tomlkit`; its message is shown to the owner); the pause
//! before damage is checked again, and who is told of a search index rebuilt in place.
//!
//! Fixed against Python (decisions: STORE-429, STORE-430, STORE-431; 2026-11-21, the owner's
//! database set aside twice while it was healthy): every check runs on its own connection, one
//! statement, so it reads one snapshot (FTS5 keeps a table's segment list from the first
//! statement of a connection and read it against the second's rows, reporting a database another
//! process was writing as corrupt); damage counts only when a second check after a pause finds it
//! again; damage only in full-text indexes (data derived from the tables) is rebuilt in place,
//! never a reason to move the database; and `ferb setup` never checks the database (Python's
//! setup didn't either: only `check_setup`).

use std::{
    fs,
    io::{self, Write},
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    time::Duration,
};

use rusqlite::{Connection, OpenFlags, OptionalExtension};
use serde_json::{json, Map, Value};

use crate::{
    known_schema,
    pyjson::{self, Style},
    pytime::{DateTime, Timespec},
    pyval::{prefix, squeeze},
    Error, Result, BUSY_TIMEOUT_MS,
};

/// Moves the owner hasn't been told about yet (`salvage.py:34`; decision: STORE-2).
pub const PENDING: &str = "set-aside.json";
/// The build whose full integrity check passed (`salvage.py:35`; decision: STORE-4).
pub const CHECKED: &str = "run/store-checked.json";
pub const STORE: &str = "ferb.db";
pub const CONFIG: &str = "config.toml";
pub const SECRET_NAMES: &str = "owner-secret-handles.json";
pub const PERSONA: &str = "persona.json";
/// SQLite's own files beside the database, moved with it (`salvage.py:37`).
pub const STORE_COMPANIONS: [&str; 2] = ["-wal", "-shm"];
/// The primary result codes that mean damage, not a passing problem (`salvage.py:41`).
const SQLITE_CORRUPT: i32 = 11;
const SQLITE_NOTADB: i32 = 26;
/// Problems a check reports (and FERB keeps as the reason) (`salvage.py:42`).
const FOUND_SHOWN: u32 = 5;
/// Problems read to tell whether damage is only in full-text indexes; a check that reports this
/// many may have more, so it isn't (decision: STORE-431).
const FOUND_READ: usize = 100;
/// How long FERB waits before it checks damage again (decision: STORE-430).
pub const RECHECK_AFTER: Duration = Duration::from_secs(2);
/// The full-text modules whose indexes are data derived from the tables, rebuilt by SQLite's
/// `'rebuild'` command (decision: STORE-431).
const FTS_MODULES: [&str; 3] = ["fts3", "fts4", "fts5"];

/// One move, as `set-aside.json` and the journal keep it: `file`, `moved_to`, `why`, `at`.
pub type Entry = Map<String, Value>;

/// What the check takes from outside the store.
pub struct Seams<'a> {
    /// `datetime.now()`: local wall time, naive (`DateTime::now_local`).
    pub now: &'a dyn Fn() -> DateTime,
    /// Why `config.toml`'s text doesn't parse as TOML (the parser's message), or `None` when it
    /// does (`tomlkit.parse`, `salvage.py:65`).
    pub toml_error: &'a dyn Fn(&str) -> Option<String>,
    /// `thread::sleep`: the pause before damage found is checked again (decision: STORE-430).
    pub pause: &'a dyn Fn(Duration),
    /// Told of full-text indexes rebuilt in place (the tables, then the damage SQLite reported),
    /// for the daemon's journal (decision: STORE-431).
    pub rebuilt: &'a dyn Fn(&[String], &str),
}

/// What the owner reads for each file FERB may set aside (`salvage.py:39-40`).
fn what(file: &str) -> &str {
    match file {
        STORE => "its database",
        CONFIG => "its settings file (config.toml)",
        SECRET_NAMES => "its list of saved secret names (the secrets stay in your Keychain)",
        other => other,
    }
}

/// `this_build` (`salvage.py:47-51`): this FERB's build, for the once-per-build full check: its
/// version, FERB.app's build number (`FERB_APP_BUILD`; a checkout's: none) and the newest
/// schema it knows.
pub fn this_build(version: &str) -> String {
    let app = std::env::var("FERB_APP_BUILD")
        .ok()
        .filter(|b| !b.is_empty())
        .unwrap_or_else(|| "checkout".to_owned());
    format!("{version}+{app}+{}", known_schema())
}

/// `check_home` (`salvage.py:54-58`; decision: STORE-1): the daemon's check at each start,
/// before it opens anything: the database first (a newer one is `NewerStore` before anything
/// is changed), then the owner's files. Returns what it set aside now.
pub fn check_home(home: &Path, build: &str, seams: &Seams<'_>) -> Result<Vec<Entry>> {
    let mut moved = check_store(home, build, seams)?;
    moved.extend(check_files(home, seams)?);
    Ok(moved)
}

/// `ferb setup`'s check (the Python CLI's, `ferb/cli/__init__.py:223-231`; decisions: STORE-1,
/// STORE-3, STORE-429): a database a newer FERB migrated refuses the setup (`NewerStore`, nothing
/// changed), then the owner's files. It never checks or moves the database: setup runs while the
/// daemon may be writing it, and the daemon checks it at its start, alone (under its lock).
pub fn check_setup(home: &Path, seams: &Seams<'_>) -> Result<Vec<Entry>> {
    refuse_newer(home)?;
    check_files(home, seams)
}

/// `_short` (`salvage.py:228-229`) of an error's text; `name` stands for the error's type when
/// it says nothing.
fn short(message: &str, name: &str) -> String {
    let text = squeeze(message);
    prefix(if text.is_empty() { name } else { &text }, 200).to_owned()
}

/// `check_files` (`salvage.py:61-80`): config.toml and the saved secret names, moved aside
/// when they can't be parsed; persona.json (FERB's own: it scans this Mac again) deleted then.
pub fn check_files(home: &Path, seams: &Seams<'_>) -> Result<Vec<Entry>> {
    let mut moved = Vec::new();
    let config = home.join(CONFIG);
    if config.is_file() {
        let bytes = fs::read(&config)?;
        let bad = match pyjson::decode_utf8(&bytes) {
            // text that isn't UTF-8 is a ValueError too
            Err(why) => Some(short(&why, "UnicodeDecodeError")),
            Ok(text) => (seams.toml_error)(text).map(|why| short(&why, "ParseError")),
        };
        if let Some(why) = bad {
            let why = format!("it doesn't parse ({why})");
            moved.push(set_aside(home, CONFIG, &why, seams.now, &[])?);
        }
    }
    for name in [SECRET_NAMES, PERSONA] {
        let path = home.join(name);
        if !path.is_file() {
            continue;
        }
        let bytes = fs::read(&path)?;
        let bad = match pyjson::decode_utf8(&bytes) {
            Err(why) => Some(short(&why, "UnicodeDecodeError")),
            Ok(text) => pyjson::check(text)
                .err()
                .map(|why| short(&why, "JSONDecodeError")),
        };
        if let Some(why) = bad {
            if name == PERSONA {
                fs::remove_file(&path)?; // FERB's own: it scans this Mac again
            } else {
                let why = format!("it isn't JSON ({why})");
                moved.push(set_aside(home, name, &why, seams.now, &[])?);
            }
        }
    }
    Ok(moved)
}

/// `check_store` (`salvage.py:83-96`; decisions: STORE-4, STORE-430, STORE-431): ferb.db kept
/// when SQLite's check finds nothing; set aside with its WAL when it finds damage that a second
/// check after a pause finds again and that isn't only in full-text indexes (those are rebuilt in
/// place and checked again); `NewerStore` when a newer FERB migrated it. `build`: this FERB's
/// build, which runs the full check once ("" for none).
pub fn check_store(home: &Path, build: &str, seams: &Seams<'_>) -> Result<Vec<Entry>> {
    let path = home.join(STORE);
    if !path.is_file() || fs::metadata(&path)?.len() == 0 {
        return Ok(Vec::new()); // nothing yet: Db::open makes it
    }
    let full = !build.is_empty() && checked(home).as_deref() != Some(build);
    let passed = |checked_fully: bool| -> Result<Vec<Entry>> {
        if checked_fully && !build.is_empty() {
            mark_checked(home, build, seams.now)?;
        }
        Ok(Vec::new())
    };
    if damage(&path, full)?.is_none() {
        return passed(full);
    }
    // decision: STORE-430: only damage a second check finds, on new connections, counts
    (seams.pause)(RECHECK_AFTER);
    let Some(found) = damage(&path, true)? else {
        return passed(true);
    };
    // decision: STORE-431: damage only in full-text indexes is rebuilt, never a set-aside
    if let Some(tables) = only_in_indexes(&path)? {
        if rebuild(&path, &tables)? && damage(&path, true)?.is_none() {
            (seams.rebuilt)(&tables, &found);
            return passed(true);
        }
    }
    Ok(vec![set_aside(
        home,
        STORE,
        &found,
        seams.now,
        &STORE_COMPANIONS,
    )?])
}

/// `refuse_newer` (`salvage.py:99-106`; decision: STORE-3): setup's check, before it installs
/// anything: `NewerStore` when a newer FERB migrated the database. Reads only the schema
/// version; a database that can't be read is the daemon's to check.
pub fn refuse_newer(home: &Path) -> Result<()> {
    let path = home.join(STORE);
    if !path.is_file() {
        return Ok(());
    }
    let read = || -> Result<()> { newer(&open(&path)?) };
    match read() {
        Err(Error::Sqlite(_)) => Ok(()),
        other => other,
    }
}

/// `damage` (`salvage.py:109-123`): what SQLite finds wrong with the database (its own words),
/// or `None` when nothing. quick_check; integrity_check when `full`, and to confirm a failed
/// quick_check. `NewerStore`, and any error that isn't damage, is returned as the error.
///
/// Each check is one statement on a connection of its own, so each reads one snapshot of a
/// database another process may be writing. Fixed against Python, which ran both on one
/// connection (decision: STORE-430): FTS5's own check keeps the segment list it read in the first
/// statement and reads it against the second statement's rows, so a segment merged in between
/// reads as "fts5: corruption found reading blob" or "checksum mismatch" (seen 2026-11-21).
pub fn damage(path: &Path, full: bool) -> Result<Option<String>> {
    if !path.is_file() {
        return Err(Error::Io(io::Error::new(
            io::ErrorKind::NotFound,
            path.display().to_string(),
        )));
    }
    let read = || -> Result<Option<String>> {
        let mut found = check(&open(path)?, "quick_check")?;
        if found.is_some() || full {
            found = check(&open(path)?, "integrity_check")?;
        }
        if found.is_none() {
            newer(&open(path)?)?;
        }
        Ok(found)
    };
    match read() {
        Err(Error::Sqlite(e)) => {
            if is_damage(&e) {
                let message = match &e {
                    rusqlite::Error::SqliteFailure(failure, text) => {
                        text.clone().unwrap_or_else(|| failure.to_string())
                    }
                    other => other.to_string(),
                };
                Ok(Some(format!(
                    "SQLite: {}",
                    short(&message, "DatabaseError")
                )))
            } else {
                Err(Error::Sqlite(e))
            }
        }
        other => other,
    }
}

/// `set_aside` (`salvage.py:126-142`; decisions: STORE-1, STORE-2): move `name` (and its
/// companions, under the same new name) aside in FERB's home, never over another file, and
/// record it for the owner. Returns the record.
pub fn set_aside(
    home: &Path,
    name: &str,
    why: &str,
    now: &dyn Fn() -> DateTime,
    companions: &[&str],
) -> Result<Entry> {
    let when = now();
    let base = format!("{name}.broken-{}", when.compact_minute());
    let taken = |target: &str| {
        std::iter::once("")
            .chain(companions.iter().copied())
            .any(|suffix| home.join(format!("{target}{suffix}")).exists())
    };
    let (mut target, mut n) = (base.clone(), 1);
    while taken(&target) {
        n += 1;
        target = format!("{base}-{n}");
    }
    fs::rename(home.join(name), home.join(&target))?;
    for suffix in companions {
        let beside = home.join(format!("{name}{suffix}"));
        if beside.exists() {
            fs::rename(beside, home.join(format!("{target}{suffix}")))?;
        }
    }
    let mut entry = Map::new();
    entry.insert("file".to_owned(), json!(name));
    entry.insert(
        "moved_to".to_owned(),
        json!(home.join(&target).to_string_lossy()),
    );
    entry.insert("why".to_owned(), json!(why));
    entry.insert("at".to_owned(), json!(when.isoformat(Timespec::Seconds)));
    let mut all = pending(home);
    all.push(entry.clone());
    write_pending(home, &all)?;
    Ok(entry)
}

/// `pending` (`salvage.py:145-151`): the moves the owner hasn't been told about yet (a record
/// that can't be read is FERB's own: read as none).
pub fn pending(home: &Path) -> Vec<Entry> {
    let Ok(text) = fs::read_to_string(home.join(PENDING)) else {
        return Vec::new();
    };
    let Ok(Value::Array(data)) = pyjson::loads(&text) else {
        return Vec::new();
    };
    data.into_iter()
        .filter_map(|e| match e {
            Value::Object(map) if map.get("moved_to").is_some_and(Value::is_string) => Some(map),
            _ => None,
        })
        .collect()
}

/// `told` (`salvage.py:154-161`; decision: STORE-2): these moves were journaled and the owner
/// told: they leave the record.
pub fn told(home: &Path, entries: &[Entry]) -> Result<()> {
    let done: Vec<&Value> = entries.iter().filter_map(|e| e.get("moved_to")).collect();
    let left: Vec<Entry> = pending(home)
        .into_iter()
        .filter(|e| !done.contains(&&e["moved_to"]))
        .collect();
    if left.is_empty() {
        match fs::remove_file(home.join(PENDING)) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e.into()),
            _ => Ok(()),
        }
    } else {
        write_pending(home, &left)
    }
}

fn field(entry: &Entry, key: &str) -> String {
    entry
        .get(key)
        .map_or_else(|| "None".to_owned(), crate::pyval::py_str)
}

/// `notice` (`salvage.py:164-168`): what the owner reads, once, about one move.
pub fn notice(entry: &Entry) -> String {
    let file = field(entry, "file");
    format!(
        "FERB couldn't read {}: {}. It moved the file aside to {} (nothing was deleted) and started a new one.",
        what(&file),
        field(entry, "why"),
        field(entry, "moved_to")
    )
}

/// `describe` (`salvage.py:171-174`): setup's lines for what it set aside.
pub fn describe(entries: &[Entry]) -> Vec<String> {
    entries
        .iter()
        .map(|entry| {
            format!(
                "{} couldn't be read ({}); moved aside to {}, starting a new one.",
                field(entry, "file"),
                field(entry, "why"),
                field(entry, "moved_to")
            )
        })
        .collect()
}

// ---------- internals ----------

/// `_open` (`salvage.py:179-193`): a read-only connection to an existing database (never
/// creates one): it never checkpoints the WAL into the file or deletes it, so a file set aside
/// keeps exactly what it held.
fn open(path: &Path) -> Result<Connection> {
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    conn.busy_timeout(Duration::from_millis(u64::from(BUSY_TIMEOUT_MS)))?;
    Ok(conn)
}

/// SQLite's primary result code says damage (CORRUPT, NOTADB), not a passing problem.
fn is_damage(e: &rusqlite::Error) -> bool {
    let code = match e {
        rusqlite::Error::SqliteFailure(failure, _) => failure.extended_code & 0xFF,
        _ => 0,
    };
    code == SQLITE_CORRUPT || code == SQLITE_NOTADB
}

/// What `PRAGMA integrity_check` (of `table` alone when given) reports, up to `limit` lines;
/// empty when it says ok.
fn problems(conn: &Connection, table: Option<&str>, limit: usize) -> Result<Vec<String>> {
    let sql = match table {
        Some(name) => format!("PRAGMA integrity_check('{}')", name.replace('\'', "''")),
        None => format!("PRAGMA integrity_check({limit})"),
    };
    let mut statement = conn.prepare(&sql)?;
    let rows: Vec<String> = statement
        .query_map([], |r| r.get(0))?
        .take(limit)
        .collect::<rusqlite::Result<_>>()?;
    Ok(if rows == ["ok"] { Vec::new() } else { rows })
}

/// The database's full-text tables (decision: STORE-431): virtual tables whose module, as their
/// `CREATE VIRTUAL TABLE ... USING <module>` names it, is FTS3, FTS4 or FTS5.
fn fts_tables(conn: &Connection) -> Result<Vec<String>> {
    let mut statement = conn.prepare(
        "SELECT name, sql FROM sqlite_master WHERE type = 'table' AND sql LIKE 'CREATE VIRTUAL TABLE%' ORDER BY name",
    )?;
    let rows: Vec<(String, String)> = statement
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;
    Ok(rows
        .into_iter()
        .filter(|(_, sql)| {
            let lower = sql.to_ascii_lowercase();
            lower.find(" using ").is_some_and(|at| {
                let module: String = lower[at + 7..]
                    .trim_start()
                    .chars()
                    .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                    .collect();
                FTS_MODULES.contains(&module.as_str())
            })
        })
        .map(|(name, _)| name)
        .collect())
}

/// The full-text tables to rebuild when every problem SQLite's check reports is one of theirs
/// (each table checked alone, on connections of their own, reports exactly those lines);
/// `None` when any problem is elsewhere (a table, an index, the file, a shadow table's pages),
/// or SQLite can't tell (decision: STORE-431).
fn only_in_indexes(path: &Path) -> Result<Option<Vec<String>>> {
    let read = || -> Result<Option<Vec<String>>> {
        let all = problems(&open(path)?, None, FOUND_READ)?;
        if all.is_empty() || all.len() >= FOUND_READ {
            return Ok(None);
        }
        let mut theirs = Vec::new();
        let mut tables = Vec::new();
        for table in fts_tables(&open(path)?)? {
            let found = problems(&open(path)?, Some(&table), FOUND_READ)?;
            if !found.is_empty() {
                theirs.extend(found);
                tables.push(table);
            }
        }
        let every = all.iter().all(|line| theirs.contains(line));
        Ok((every && !tables.is_empty()).then_some(tables))
    };
    match read() {
        Err(Error::Sqlite(e)) if is_damage(&e) => Ok(None),
        other => other,
    }
}

/// SQLite's `'rebuild'` of each full-text table, from the rows it indexes, in one transaction
/// on a read-write connection (decision: STORE-431). False when SQLite refuses for damage or
/// another reason of the file's (the database is then set aside as before); a lock, I/O or
/// permission error is returned as it is.
fn rebuild(path: &Path, tables: &[String]) -> Result<bool> {
    let run = || -> rusqlite::Result<()> {
        let mut conn = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        conn.busy_timeout(Duration::from_millis(u64::from(BUSY_TIMEOUT_MS)))?;
        let tx = conn.transaction()?;
        for table in tables {
            let name = format!("\"{}\"", table.replace('"', "\"\""));
            tx.execute_batch(&format!("INSERT INTO {name}({name}) VALUES('rebuild')"))?;
        }
        tx.commit()?;
        conn.close().map_err(|(_, e)| e)
    };
    match run() {
        Ok(()) => Ok(true),
        Err(e) if is_damage(&e) || is_refusal(&e) => Ok(false),
        Err(e) => Err(Error::Sqlite(e)),
    }
}

/// SQLite's plain refusal (SQLITE_ERROR, e.g. a module that can't rebuild), not a lock, I/O or
/// permission error.
fn is_refusal(e: &rusqlite::Error) -> bool {
    match e {
        rusqlite::Error::SqliteFailure(failure, _) => failure.extended_code & 0xFF == 1,
        _ => false,
    }
}

/// `_check` (`salvage.py:196-198`).
fn check(conn: &Connection, pragma: &str) -> Result<Option<String>> {
    let mut statement = conn.prepare(&format!("PRAGMA {pragma}({FOUND_SHOWN})"))?;
    let rows: Vec<String> = statement
        .query_map([], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    if rows == ["ok"] {
        return Ok(None);
    }
    Ok(Some(format!(
        "SQLite's {}: {}",
        pragma.replace('_', " "),
        prefix(&rows.join("; "), 400)
    )))
}

/// `_newer` (`salvage.py:201-206`).
fn newer(conn: &Connection) -> Result<()> {
    let table: Option<i64> = conn
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'schema_version'",
            [],
            |r| r.get(0),
        )
        .optional()?;
    if table.is_none() {
        return Ok(());
    }
    let newest: Option<i64> =
        conn.query_row("SELECT MAX(version) FROM schema_version", [], |r| r.get(0))?;
    let (newest, known) = (newest.unwrap_or(0), known_schema());
    if newest > known {
        return Err(Error::NewerStore { newest, known });
    }
    Ok(())
}

/// `_checked` (`salvage.py:209-214`).
fn checked(home: &Path) -> Option<String> {
    let text = fs::read_to_string(home.join(CHECKED)).ok()?;
    let data = pyjson::loads(&text).ok()?;
    let map = data.as_object()?;
    Some(
        map.get("build")
            .map_or_else(|| "None".to_owned(), crate::pyval::py_str),
    )
}

/// `_mark_checked` (`salvage.py:217-220`).
fn mark_checked(home: &Path, build: &str, now: &dyn Fn() -> DateTime) -> Result<()> {
    let path = home.join(CHECKED);
    let text = pyjson::dumps(&json!({"build": build, "at": now().isoformat(Timespec::Seconds)}));
    let folder = path.parent().unwrap_or(home).to_path_buf();
    write_private(&folder, &path, &format!("{text}\n"))?;
    Ok(())
}

/// `_write_pending` (`salvage.py:223-225`).
fn write_pending(home: &Path, entries: &[Entry]) -> Result<()> {
    let list = Value::Array(entries.iter().cloned().map(Value::Object).collect());
    let text = pyjson::dumps_with(
        &list,
        Style {
            ascii: false,
            sort_keys: false,
            indent: Some(2),
        },
    );
    write_private(home, &home.join(PENDING), &format!("{text}\n"))?;
    Ok(())
}

/// `ferb.config.write_private` (`ferb/config.py:72-87`), the part the store needs: `text`
/// written to `path` (inside `folder`) atomically and owner-only: a 0600 temporary file in the
/// same folder, fsync, rename over the original; `folder` is made 0700.
fn write_private(folder: &Path, path: &Path, text: &str) -> io::Result<()> {
    fs::create_dir_all(folder)?;
    fs::set_permissions(folder, fs::Permissions::from_mode(0o700))?;
    let stem = path.file_stem().unwrap_or_default().to_string_lossy();
    let suffix = path
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default();
    let mut attempt = 0u32;
    let (tmp, mut file): (PathBuf, fs::File) = loop {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.subsec_nanos());
        let name = format!(
            ".{stem}.{:x}{nanos:08x}{attempt:x}{suffix}",
            std::process::id()
        );
        let tmp = folder.join(name);
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&tmp)
        {
            Ok(file) => break (tmp, file),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists && attempt < 100 => attempt += 1,
            Err(e) => return Err(e),
        }
    };
    let written = (|| {
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
        fs::set_permissions(&tmp, fs::Permissions::from_mode(0o600))?;
        fs::rename(&tmp, path) // atomic on the same filesystem
    })();
    if written.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    written
}
