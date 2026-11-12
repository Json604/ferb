//! FERB's store foundation: the SQLite connection and the schema migrations.
//!
//! A port of `ferb/store/db.py` (Python lines at `63369cb3`). Either daemon can open a store the
//! other made: the same pragmas, the same migration files (embedded from `ferb/store/migrations`
//! at build time, decision: ENG-4) and the same `schema_version` rows.
//!
//! Not ported here: `swap` (`db.py:236`), which belongs with the parts that call it.

mod migrate;

use std::{
    fs, io,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
};

use rusqlite::{Connection, OpenFlags};

pub use migrate::{known_schema, newer_than_build, split_sql, Migration};

/// How long a connection waits for another's write lock (`db.py:15`; decision: STORE-409).
pub const BUSY_TIMEOUT_MS: u32 = 5000;
/// The size the WAL file is cut back to after a checkpoint (`db.py:17`; decision: STORE-409).
pub const JOURNAL_SIZE_LIMIT: u64 = 64 * 1024 * 1024;
/// Free pages handed back per slice and per hourly pass (`db.py:20-21`; decision: STORE-409).
pub const VACUUM_SLICE_PAGES: u32 = 512;
pub const VACUUM_PASS_PAGES: u32 = 8192;
/// The free disk a compaction needs, as a multiple of the used bytes (`db.py:25`; decision: STORE-416).
pub const COMPACT_ROOM: f64 = 2.25;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Python's `MigrationError` (`db.py:28`): the migrations can't be applied safely.
    #[error("{0}")]
    Migration(String),
    /// Python's `NewerStore` (`db.py:32`): a newer FERB migrated this database. Nothing was
    /// changed (decision: STORE-3).
    #[error("{}", newer_than_build(*.newest, *.known))]
    NewerStore { newest: i64, known: i64 },
    /// Python's `ValueError` from `_split_sql` (`db.py:275`).
    #[error("incomplete SQL statement at end of migration: {0}")]
    IncompleteSql(String),
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
    #[error(transparent)]
    Io(#[from] io::Error),
}

impl Error {
    /// Whether Python would raise a `MigrationError` (`NewerStore` is one).
    pub fn is_migration_error(&self) -> bool {
        matches!(self, Error::Migration(_) | Error::NewerStore { .. })
    }
}

pub type Result<T> = std::result::Result<T, Error>;

/// What `truncate_wal` left behind when a reader kept the WAL from being emptied (`db.py:95`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WalKept {
    pub wal_frames: i64,
    pub checkpointed: i64,
}

/// What `compact` did (`db.py:136-138`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Compaction {
    pub compacted: bool,
    pub needed: u64,
    pub free: u64,
}

type OnWalKept = Box<dyn Fn(&str, WalKept) + Send>;

/// One connection to a store (Python's `Database`, `db.py:55`).
pub struct Db {
    conn: Connection,
    path: PathBuf,
    /// Told when the WAL couldn't be emptied after a forget (`db.py:73`).
    pub on_wal_kept: OnWalKept,
}

impl Db {
    /// Open (or create) the store at `path` as `Database.__init__` does (`db.py:58-73`): parent
    /// folders made, autocommit, then the pragmas in Python's order, then the file set to 0600.
    /// decisions: STORE-409, STORE-417.
    pub fn open(path: impl AsRef<Path>) -> Result<Db> {
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let new = fs::metadata(&path).map_or(true, |m| m.len() == 0);
        // Python's sqlite3.connect: read-write, create, the path never read as a URI.
        let flags = OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_CREATE
            | OpenFlags::SQLITE_OPEN_NO_MUTEX;
        let conn = Connection::open_with_flags(&path, flags)?;
        conn.execute_batch(&format!("PRAGMA busy_timeout={BUSY_TIMEOUT_MS}"))?;
        if new {
            // only before the first table: an older store gets it by `compact`
            conn.execute_batch("PRAGMA auto_vacuum=INCREMENTAL")?;
        }
        conn.execute_batch("PRAGMA journal_mode=WAL")?;
        conn.execute_batch(&format!("PRAGMA journal_size_limit={JOURNAL_SIZE_LIMIT}"))?;
        conn.execute_batch("PRAGMA foreign_keys=ON")?;
        conn.execute_batch("PRAGMA secure_delete=ON")?; // "forget" must really erase
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
        Ok(Db {
            conn,
            path,
            on_wal_kept: Box::new(|_, _| {}),
        })
    }

    pub fn conn(&self) -> &Connection {
        &self.conn
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// One write transaction (`db.py:75-90`; decision: STORE-415): `BEGIN IMMEDIATE`, committed
    /// when `block` returns `Ok`, rolled back whole on `Err` or a panic. Inside a transaction
    /// already, the block is part of that one.
    pub fn transaction<T, E>(
        &self,
        block: impl FnOnce(&Connection) -> std::result::Result<T, E>,
    ) -> std::result::Result<T, E>
    where
        E: From<rusqlite::Error>,
    {
        if !self.conn.is_autocommit() {
            return block(&self.conn);
        }
        self.conn.execute_batch("BEGIN IMMEDIATE")?;
        let guard = Rollback(&self.conn);
        let value = block(&self.conn)?;
        self.conn.execute_batch("COMMIT")?;
        drop(guard);
        Ok(value)
    }

    /// Empty the WAL, waiting out readers up to the busy timeout; `on_wal_kept` is told what was
    /// left. Returns whether it was emptied (`db.py:92-99`; decision: STORE-409).
    pub fn truncate_wal(&self, after: &str) -> Result<bool> {
        let (busy, wal_frames, checkpointed) = self.wal_checkpoint_truncate()?;
        if busy != 0 {
            (self.on_wal_kept)(
                after,
                WalKept {
                    wal_frames,
                    checkpointed,
                },
            );
        }
        Ok(busy == 0)
    }

    /// The routine checkpoint: TRUNCATE without waiting, the busy timeout put back afterwards
    /// (`db.py:101-111`; decision: STORE-409). Returns whether the WAL was emptied.
    pub fn checkpoint(&self) -> Result<bool> {
        let wait: i64 = self.pragma("busy_timeout")?;
        self.conn.execute_batch("PRAGMA busy_timeout=0")?;
        let result = self.wal_checkpoint_truncate();
        self.conn
            .execute_batch(&format!("PRAGMA busy_timeout={wait}"))?;
        Ok(result?.0 == 0)
    }

    fn wal_checkpoint_truncate(&self) -> Result<(i64, i64, i64)> {
        Ok(self
            .conn
            .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?))
            })?)
    }

    fn pragma(&self, name: &str) -> Result<i64> {
        Ok(self
            .conn
            .query_row(&format!("PRAGMA {name}"), [], |r| r.get(0))?)
    }

    /// Whether the store hands free pages back by itself (`db.py:113-116`).
    pub fn incremental(&self) -> Result<bool> {
        Ok(self.pragma("auto_vacuum")? == 2)
    }

    /// Up to `pages` free pages handed back to the disk; returns how many are left
    /// (`db.py:118-122`).
    pub fn vacuum_slice(&self, pages: u32) -> Result<i64> {
        self.conn
            .execute_batch(&format!("PRAGMA incremental_vacuum({pages})"))?;
        self.pragma("freelist_count")
    }

    /// The free disk `compact` needs (`db.py:222-227`; decision: STORE-416).
    pub fn compaction_needs(&self) -> Result<u64> {
        let used = self.pragma("page_count")? - self.pragma("freelist_count")?;
        Ok(((used * self.pragma("page_size")?) as f64 * COMPACT_ROOM) as u64)
    }

    /// Shrink the file to what it holds and switch it to incremental auto-vacuum, only when
    /// `free` (the free bytes of the folder's volume; see `disk_free`) covers `compaction_needs`
    /// (`db.py:124-138`; decision: STORE-416).
    pub fn compact(&self, free: impl FnOnce(&Path) -> io::Result<u64>) -> Result<Compaction> {
        self.truncate_wal("store.compact")?;
        let needed = self.compaction_needs()?;
        let room = free(self.path.parent().unwrap_or(Path::new(".")))?;
        if room < needed {
            return Ok(Compaction {
                compacted: false,
                needed,
                free: room,
            });
        }
        self.conn.execute_batch("PRAGMA auto_vacuum=INCREMENTAL")?;
        self.conn.execute_batch("VACUUM")?;
        self.truncate_wal("store.compact")?;
        Ok(Compaction {
            compacted: true,
            needed,
            free: room,
        })
    }

    /// Close the connection (`db.py:218`); dropping a `Db` does the same without the error.
    pub fn close(self) -> Result<()> {
        self.conn.close().map_err(|(_, e)| Error::Sqlite(e))
    }
}

/// Rolls back on drop while a transaction is still open: the `Err` and panic paths of
/// `Db::transaction`. SQLite may have rolled back already; then there is nothing to do.
struct Rollback<'a>(&'a Connection);

impl Drop for Rollback<'_> {
    fn drop(&mut self) {
        if !self.0.is_autocommit() {
            let _ = self.0.execute_batch("ROLLBACK");
        }
    }
}

/// The bytes a store takes on disk: its database file and its WAL (`db.py:50-52`).
pub fn store_size(path: &Path) -> u64 {
    let mut wal = path.as_os_str().to_owned();
    wal.push("-wal");
    [path, Path::new(&wal)]
        .iter()
        .filter_map(|p| fs::metadata(p).ok())
        .map(|m| m.len())
        .sum()
}

/// The free bytes of the volume holding `folder`, as Python's `shutil.disk_usage(folder).free`
/// (the default of `Database.compact`, `db.py:124`).
pub fn disk_free(folder: &Path) -> io::Result<u64> {
    use std::os::unix::ffi::OsStrExt;
    let path = std::ffi::CString::new(folder.as_os_str().as_bytes())?;
    let mut stat = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    // SAFETY: `path` is a NUL-terminated string and `stat` is writable memory of the right size;
    // statvfs fills it when it returns 0.
    let stat = unsafe {
        if libc::statvfs(path.as_ptr(), stat.as_mut_ptr()) != 0 {
            return Err(io::Error::last_os_error());
        }
        stat.assume_init()
    };
    Ok(stat.f_bavail as u64 * stat.f_frsize as u64)
}
