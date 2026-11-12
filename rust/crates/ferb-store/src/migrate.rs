//! The migration runner: a port of `Database.migrate` and `_split_sql` (`ferb/store/db.py:140-216`,
//! `262-276`). decisions: STORE-414, STORE-3, ENG-4.

use std::{
    borrow::Cow,
    collections::{BTreeMap, HashMap},
    ffi::CString,
    fs, io,
    path::Path,
};

use rusqlite::params;
use sha2::{Digest, Sha256};

use crate::{Db, Error, Result};

include!(concat!(env!("OUT_DIR"), "/migrations.rs"));

/// One migration file: its name (`NNNN_*.sql`) and its exact bytes.
#[derive(Debug, Clone)]
pub struct Migration {
    pub name: Cow<'static, str>,
    pub bytes: Cow<'static, [u8]>,
}

impl Migration {
    /// The files of `ferb/store/migrations` as they were when this crate was built.
    pub fn embedded() -> Vec<Migration> {
        let file = |&(name, bytes)| Migration {
            name: Cow::Borrowed(name),
            bytes: Cow::Borrowed(bytes),
        };
        EMBEDDED.iter().map(file).collect()
    }

    /// The `[0-9][0-9][0-9][0-9]_*.sql` files of `folder`, sorted by name (`db.py:158`).
    pub fn from_dir(folder: &Path) -> io::Result<Vec<Migration>> {
        let mut names = Vec::new();
        for entry in fs::read_dir(folder)? {
            if let Ok(name) = entry?.file_name().into_string() {
                if version_of(&name).is_some() {
                    names.push(name);
                }
            }
        }
        names.sort();
        let read = |name: String| {
            let bytes = fs::read(folder.join(&name))?;
            Ok(Migration {
                name: Cow::Owned(name),
                bytes: Cow::Owned(bytes),
            })
        };
        names.into_iter().map(read).collect()
    }

    fn sha256(&self) -> String {
        hex::encode(Sha256::digest(&self.bytes))
    }
}

/// The number of a file named `NNNN_*.sql`, or None for any other name.
fn version_of(name: &str) -> Option<i64> {
    let b = name.as_bytes();
    let matches = b.len() >= 9
        && b[..4].iter().all(u8::is_ascii_digit)
        && b[4] == b'_'
        && name.ends_with(".sql");
    matches.then(|| name[..4].parse().unwrap())
}

/// The newest schema version this build's migration files reach (`db.py:37-40`).
pub fn known_schema() -> i64 {
    EMBEDDED
        .iter()
        .filter_map(|(name, _)| version_of(name))
        .max()
        .unwrap_or(0)
}

/// What the owner reads about a database a newer FERB migrated (`db.py:43-47`).
pub fn newer_than_build(newest: i64, known: i64) -> String {
    format!(
        "FERB's database was last used by a newer FERB (schema {newest}; this FERB knows up to {known}). Install \
         the newer FERB; nothing was changed"
    )
}

impl Db {
    /// The newest recorded schema version, 0 on an empty store (`db.py:140-143`).
    pub fn schema_version(&self) -> Result<i64> {
        self.version_table()?;
        let newest: Option<i64> =
            self.conn()
                .query_row("SELECT MAX(version) FROM schema_version", [], |r| r.get(0))?;
        Ok(newest.unwrap_or(0))
    }

    /// `db.py:145-149`: the table as Python creates it; an install migrated before checksums
    /// were recorded gets the column.
    fn version_table(&self) -> Result<()> {
        let conn = self.conn();
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS schema_version (version INTEGER NOT NULL, sha256 TEXT)",
        )?;
        let has_sum: bool = conn.query_row(
            "SELECT count(*) FROM pragma_table_info('schema_version') WHERE name = 'sha256'",
            [],
            |r| r.get(0),
        )?;
        if !has_sum {
            conn.execute_batch("ALTER TABLE schema_version ADD COLUMN sha256 TEXT")?;
        }
        Ok(())
    }

    /// Apply this build's embedded migrations; returns the versions applied.
    pub fn migrate(&self) -> Result<Vec<i64>> {
        self.migrate_files(&Migration::embedded())
    }

    /// Apply the migrations of `folder` (Python's `migrate(folder)`).
    pub fn migrate_dir(&self, folder: &Path) -> Result<Vec<i64>> {
        self.migrate_files(&Migration::from_dir(folder)?)
    }

    /// Apply every file not yet applied, in order, each in its own `BEGIN IMMEDIATE`
    /// transaction, recording its checksum (`db.py:151-216`; decision: STORE-414).
    /// Refused with nothing applied: two files with one number; a new file numbered below the
    /// newest applied one; an applied file whose content changed; a database migrated by newer
    /// code (`Error::NewerStore`, decision: STORE-3). A recorded version whose file is gone is
    /// fine below the newest, and a row without a checksum gets its file's on this run.
    /// `files` must be sorted by name, as `Migration::embedded` and `from_dir` give them.
    pub fn migrate_files(&self, files: &[Migration]) -> Result<Vec<i64>> {
        let conn = self.conn();
        let mut by_version: BTreeMap<i64, &Migration> = BTreeMap::new();
        for file in files {
            let Some(version) = version_of(&file.name) else {
                continue;
            };
            if let Some(first) = by_version.get(&version) {
                return Err(Error::Migration(format!(
                    "two migrations are numbered {version:04}: {}, {}",
                    first.name, file.name
                )));
            }
            by_version.insert(version, file);
        }
        self.version_table()?;
        let mut recorded: HashMap<i64, Option<String>> = HashMap::new();
        {
            let mut rows = conn.prepare("SELECT version, sha256 FROM schema_version")?;
            for row in rows.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))? {
                let (version, sum) = row?;
                recorded.insert(version, sum);
            }
        }
        let newest = recorded.keys().copied().max().unwrap_or(0);
        let known = by_version.keys().copied().max().unwrap_or(0);
        if newest > known {
            return Err(Error::NewerStore { newest, known });
        }
        let sums: BTreeMap<i64, String> =
            by_version.iter().map(|(v, f)| (*v, f.sha256())).collect();
        for (version, file) in &by_version {
            match recorded.get(version) {
                Some(Some(sum)) if *sum != sums[version] => {
                    return Err(Error::Migration(format!(
                        "migration {} changed after it was applied; add a new migration instead",
                        file.name
                    )));
                }
                None if *version < newest => {
                    return Err(Error::Migration(format!(
                        "migration {} is numbered below the newest applied ({newest:04}) and would never run on \
                         existing installs; renumber it above {newest:04}",
                        file.name
                    )));
                }
                _ => {}
            }
        }
        for (version, sum) in &recorded {
            if sum.is_none() && by_version.contains_key(version) {
                conn.execute(
                    "UPDATE schema_version SET sha256 = ? WHERE version = ?",
                    params![sums[version], version],
                )?;
            }
        }
        let mut applied = Vec::new();
        for (version, file) in &by_version {
            if recorded.contains_key(version) {
                continue;
            }
            // the write lock first: no read in a migration goes stale
            conn.execute_batch("BEGIN IMMEDIATE")?;
            let run = || -> Result<()> {
                let script = std::str::from_utf8(&file.bytes)
                    .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
                for statement in split_sql(script)? {
                    run_statement(conn, &statement)?;
                }
                conn.execute(
                    "INSERT INTO schema_version (version, sha256) VALUES (?, ?)",
                    params![version, sums[version]],
                )?;
                conn.execute_batch("COMMIT")?;
                Ok(())
            };
            if let Err(error) = run() {
                // Python rolls back unconditionally (`db.py:212`); here the real error is kept
                // when SQLite has rolled back already.
                if !conn.is_autocommit() {
                    conn.execute_batch("ROLLBACK")?;
                }
                return Err(error);
            }
            applied.push(*version);
        }
        Ok(applied)
    }
}

/// Run one statement to its end, as Python's `conn.execute` does for a statement of a migration.
fn run_statement(conn: &rusqlite::Connection, statement: &str) -> rusqlite::Result<()> {
    let mut prepared = conn.prepare(statement)?;
    let mut rows = prepared.raw_query();
    while rows.next()?.is_some() {}
    Ok(())
}

/// Split a migration into statements as `_split_sql` does (`db.py:262-276`): line by line, a
/// statement ends where SQLite says the text so far is complete (so triggers with inner
/// semicolons stay whole); comment lines between statements are dropped.
pub fn split_sql(script: &str) -> Result<Vec<String>> {
    // Python's read_text turns \r\n and \r into \n; str.splitlines also breaks at these others.
    let script = script.replace("\r\n", "\n").replace('\r', "\n");
    let line_end = |c: char| {
        matches!(
            c,
            '\n' | '\x0b' | '\x0c' | '\x1c'..='\x1e' | '\u{85}' | '\u{2028}' | '\u{2029}'
        )
    };
    let (mut statements, mut buf) = (Vec::new(), String::new());
    for line in script.split_inclusive(line_end) {
        if line.trim().starts_with("--") && buf.trim().is_empty() {
            continue;
        }
        buf += line;
        if complete(&buf) {
            if !buf.trim().is_empty() {
                statements.push(buf.trim().to_string());
            }
            buf.clear();
        }
    }
    if !buf.trim().is_empty() {
        return Err(Error::IncompleteSql(buf.trim().chars().take(60).collect()));
    }
    Ok(statements)
}

/// Python's `sqlite3.complete_statement`: SQLite's own `sqlite3_complete`.
fn complete(sql: &str) -> bool {
    let Ok(text) = CString::new(sql) else {
        return false;
    };
    // SAFETY: `text` is a valid NUL-terminated string that outlives the call; sqlite3_complete
    // only reads it.
    unsafe { rusqlite::ffi::sqlite3_complete(text.as_ptr()) != 0 }
}
