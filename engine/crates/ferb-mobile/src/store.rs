//! The phone's own copy (kb PHN-448, PHN-408, ENG-10): one SQLCipher database holding what the phone fetched of each
//! conversation (every entry's newest copy, tombstones included), the cursor it reads changes from, and the outbox
//! (messages only: answers are never queued, PHN-448). Kept until "Forget this Mac", which deletes the files.
//!
//! The key is the app's (a random 32-byte key wrapped by a non-exportable Keystore or Keychain key, slice S9): this
//! crate takes it as bytes at open, hands it to SQLCipher as a raw key and zeroizes its own copies; it never writes it
//! anywhere. Nothing secret is stored: a hidden answer's value never reaches this file (it is never queued), and
//! entries are the engine's redacted bodies.
//!
//! The schema is the phone's own, versioned by `PRAGMA user_version` with its own migrations (`MIGRATIONS`): an older
//! file is upgraded at open, a newer one (from a later app) is refused rather than misread.

use std::path::{Path, PathBuf};

use rusqlite::{params, Connection, OptionalExtension};
use serde_json::Value;
use zeroize::{Zeroize, Zeroizing};

use crate::entry::ChatEntry;

/// The schema's migrations, in order: `MIGRATIONS[n]` takes a file from version n to n + 1.
pub const MIGRATIONS: [&str; 2] = [
    // v1: what was fetched, where to read from, the outbox
    "CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
     CREATE TABLE conversations (
         id TEXT PRIMARY KEY,
         epoch TEXT,
         after_seq INTEGER,
         older TEXT NOT NULL DEFAULT 'unknown',
         before_pos INTEGER,
         updated_at TEXT NOT NULL DEFAULT ''
     );
     CREATE TABLE entries (
         conversation_id TEXT NOT NULL,
         id TEXT NOT NULL,
         pos INTEGER NOT NULL,
         seq INTEGER NOT NULL,
         rev INTEGER NOT NULL,
         deleted INTEGER NOT NULL,
         json TEXT NOT NULL,
         PRIMARY KEY (conversation_id, id)
     );
     CREATE INDEX entries_by_pos ON entries (conversation_id, pos);
     CREATE TABLE outbox (
         id TEXT PRIMARY KEY,
         n INTEGER NOT NULL UNIQUE,
         text TEXT NOT NULL,
         created_at TEXT NOT NULL,
         state TEXT NOT NULL,
         attempts INTEGER NOT NULL DEFAULT 0,
         reason TEXT,
         request_id TEXT,
         entry_id TEXT
     );",
    // v2: entries whose words a reply cut, to read whole (`chat.entries.get`); the conversation's read mark
    "CREATE TABLE pending_gets (
         conversation_id TEXT NOT NULL,
         id TEXT NOT NULL,
         PRIMARY KEY (conversation_id, id)
     );
     ALTER TABLE conversations ADD COLUMN read_pos INTEGER;
     ALTER TABLE conversations ADD COLUMN read_mark_sent INTEGER;",
];

/// The schema version this build writes.
pub const VERSION: i64 = MIGRATIONS.len() as i64;

/// The raw key's length (SQLCipher's 256-bit key).
pub const KEY_BYTES: usize = 32;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StoreError {
    /// The key is not 32 bytes.
    BadKey,
    /// The file is not this key's (or not a database).
    WrongKey,
    /// The file was written by a newer app (its schema version is above this build's).
    Newer(i64),
    /// SQLite's own failure (words only; never a value from the file).
    Sql(String),
    /// The store was forgotten or closed.
    Closed,
}

impl From<rusqlite::Error> for StoreError {
    fn from(e: rusqlite::Error) -> Self {
        StoreError::Sql(e.to_string())
    }
}

pub type StoreResult<T> = Result<T, StoreError>;

/// Where a conversation's changes are read from (ENG-14: the conversation's epoch and a seq).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cursor {
    pub epoch: String,
    pub after_seq: i64,
}

/// Whether older entries are still to fetch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Older {
    /// Not known yet (nothing fetched).
    Unknown,
    /// Older entries exist, before this pos.
    Before(i64),
    /// Everything back to the conversation's start is here.
    Complete,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConversationRow {
    pub id: String,
    pub cursor: Option<Cursor>,
    pub older: Older,
    pub updated_at: String,
    pub entries: i64,
}

/// One message in the outbox.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutboxRow {
    /// The submission id the engine dedupes by (ENG-14).
    pub id: String,
    pub n: i64,
    pub text: String,
    pub created_at: String,
    /// waiting, sending, sent, failed.
    pub state: String,
    pub attempts: i64,
    pub reason: Option<String>,
    pub request_id: Option<String>,
    pub entry_id: Option<String>,
}

pub struct Store {
    conn: Option<Connection>,
    path: PathBuf,
}

fn hex(bytes: &[u8]) -> Zeroizing<String> {
    let mut s = Zeroizing::new(String::with_capacity(bytes.len() * 2));
    for b in bytes {
        s.push(char::from_digit(u32::from(b >> 4), 16).unwrap_or('0'));
        s.push(char::from_digit(u32::from(b & 15), 16).unwrap_or('0'));
    }
    s
}

impl Store {
    /// Opens (or makes) the store at `path` under `key`, upgrading an older schema. The key bytes are zeroized here
    /// whatever happens.
    pub fn open(path: &Path, mut key: Vec<u8>) -> StoreResult<Store> {
        let r = Self::open_keyed(path, &key);
        key.zeroize();
        r
    }

    fn open_keyed(path: &Path, key: &[u8]) -> StoreResult<Store> {
        if key.len() != KEY_BYTES {
            return Err(StoreError::BadKey);
        }
        let conn = Connection::open(path)?;
        {
            // SQLCipher's raw key: no key derivation, the app's random key as it is
            let statement = Zeroizing::new(format!("PRAGMA key = \"x'{}'\";", hex(key).as_str()));
            conn.execute_batch(&statement)?;
        }
        if conn
            .query_row("SELECT count(*) FROM sqlite_master", [], |r| {
                r.get::<_, i64>(0)
            })
            .is_err()
        {
            return Err(StoreError::WrongKey);
        }
        conn.execute_batch("PRAGMA foreign_keys = ON; PRAGMA secure_delete = ON;")?;
        let mut store = Store {
            conn: Some(conn),
            path: path.to_path_buf(),
        };
        store.migrate()?;
        Ok(store)
    }

    fn conn(&self) -> StoreResult<&Connection> {
        self.conn.as_ref().ok_or(StoreError::Closed)
    }

    fn migrate(&mut self) -> StoreResult<()> {
        let conn = self.conn.as_mut().ok_or(StoreError::Closed)?;
        let at: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        if at > VERSION {
            return Err(StoreError::Newer(at));
        }
        for (n, step) in MIGRATIONS.iter().enumerate().skip(at as usize) {
            let tx = conn.transaction()?;
            tx.execute_batch(step)?;
            tx.execute_batch(&format!("PRAGMA user_version = {}", n + 1))?;
            tx.commit()?;
        }
        Ok(())
    }

    /// The schema version of the open file.
    pub fn version(&self) -> StoreResult<i64> {
        Ok(self
            .conn()?
            .query_row("PRAGMA user_version", [], |r| r.get(0))?)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    // ---- meta ----

    pub fn meta(&self, key: &str) -> StoreResult<Option<String>> {
        Ok(self
            .conn()?
            .query_row("SELECT value FROM meta WHERE key = ?1", [key], |r| r.get(0))
            .optional()?)
    }

    pub fn set_meta(&self, key: &str, value: &str) -> StoreResult<()> {
        self.conn()?.execute(
            "INSERT INTO meta (key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = ?2",
            params![key, value],
        )?;
        Ok(())
    }

    // ---- conversations and entries ----

    fn ensure_conversation(&self, conversation: &str) -> StoreResult<()> {
        self.conn()?.execute(
            "INSERT OR IGNORE INTO conversations (id) VALUES (?1)",
            [conversation],
        )?;
        Ok(())
    }

    /// Keeps each entry that is new or newer than the copy held (`ChatEntry::newer_than`); a cut one is never kept.
    /// Returns the entries kept, in one transaction.
    pub fn put_entries(
        &mut self,
        conversation: &str,
        entries: &[ChatEntry],
        at: &str,
    ) -> StoreResult<Vec<ChatEntry>> {
        self.ensure_conversation(conversation)?;
        let conn = self.conn.as_mut().ok_or(StoreError::Closed)?;
        let tx = conn.transaction()?;
        let mut kept = Vec::new();
        for e in entries.iter().filter(|e| !e.cut) {
            let held: Option<(i64, i64)> = tx
                .query_row(
                    "SELECT rev, seq FROM entries WHERE conversation_id = ?1 AND id = ?2",
                    params![conversation, e.id],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?;
            if held.is_some_and(|h| (e.rev, e.seq) <= h) {
                continue;
            }
            tx.execute(
                "INSERT INTO entries (conversation_id, id, pos, seq, rev, deleted, json) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                 ON CONFLICT(conversation_id, id) DO UPDATE SET pos = ?3, seq = ?4, rev = ?5, deleted = ?6, json = ?7",
                params![conversation, e.id, e.pos, e.seq, e.rev, e.deleted, e.to_json().to_string()],
            )?;
            tx.execute(
                "DELETE FROM pending_gets WHERE conversation_id = ?1 AND id = ?2",
                params![conversation, e.id],
            )?;
            kept.push(e.clone());
        }
        tx.execute(
            "UPDATE conversations SET updated_at = ?2 WHERE id = ?1",
            params![conversation, at],
        )?;
        tx.commit()?;
        Ok(kept)
    }

    /// Every entry held of a conversation (each one's newest copy).
    pub fn entries(&self, conversation: &str) -> StoreResult<Vec<ChatEntry>> {
        let conn = self.conn()?;
        let mut st =
            conn.prepare("SELECT json FROM entries WHERE conversation_id = ?1 ORDER BY pos")?;
        let rows = st.query_map([conversation], |r| r.get::<_, String>(0))?;
        let mut out = Vec::new();
        for row in rows {
            let v: Value = serde_json::from_str(&row?).unwrap_or(Value::Null);
            if let Some(e) = ChatEntry::read(&v) {
                out.push(e);
            }
        }
        Ok(out)
    }

    pub fn cursor(&self, conversation: &str) -> StoreResult<Option<Cursor>> {
        Ok(self
            .conn()?
            .query_row(
                "SELECT epoch, after_seq FROM conversations WHERE id = ?1",
                [conversation],
                |r| Ok((r.get::<_, Option<String>>(0)?, r.get::<_, Option<i64>>(1)?)),
            )
            .optional()?
            .and_then(|(e, s)| {
                Some(Cursor {
                    epoch: e?,
                    after_seq: s?,
                })
            }))
    }

    /// Moves the cursor (only forward within an epoch; a new epoch replaces it).
    pub fn set_cursor(&self, conversation: &str, cursor: &Cursor) -> StoreResult<()> {
        self.ensure_conversation(conversation)?;
        self.conn()?.execute(
            "UPDATE conversations SET epoch = ?2, after_seq = ?3
             WHERE id = ?1 AND (epoch IS NULL OR epoch != ?2 OR after_seq IS NULL OR after_seq < ?3)",
            params![conversation, cursor.epoch, cursor.after_seq],
        )?;
        Ok(())
    }

    pub fn older(&self, conversation: &str) -> StoreResult<Older> {
        let row: Option<(String, Option<i64>)> = self
            .conn()?
            .query_row(
                "SELECT older, before_pos FROM conversations WHERE id = ?1",
                [conversation],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        Ok(match row {
            Some((w, Some(p))) if w == "before" => Older::Before(p),
            Some((w, _)) if w == "complete" => Older::Complete,
            _ => Older::Unknown,
        })
    }

    pub fn set_older(&self, conversation: &str, older: &Older) -> StoreResult<()> {
        self.ensure_conversation(conversation)?;
        let (w, p) = match older {
            Older::Unknown => ("unknown", None),
            Older::Before(p) => ("before", Some(*p)),
            Older::Complete => ("complete", None),
        };
        self.conn()?.execute(
            "UPDATE conversations SET older = ?2, before_pos = ?3 WHERE id = ?1",
            params![conversation, w, p],
        )?;
        Ok(())
    }

    /// Drops what was fetched of a conversation and its cursor (`reset_required`, `cursor_ahead`: read again).
    pub fn drop_conversation(&mut self, conversation: &str) -> StoreResult<()> {
        let conn = self.conn.as_mut().ok_or(StoreError::Closed)?;
        let tx = conn.transaction()?;
        tx.execute(
            "DELETE FROM entries WHERE conversation_id = ?1",
            [conversation],
        )?;
        tx.execute(
            "DELETE FROM pending_gets WHERE conversation_id = ?1",
            [conversation],
        )?;
        tx.execute(
            "UPDATE conversations SET epoch = NULL, after_seq = NULL, older = 'unknown', before_pos = NULL,
             read_pos = NULL, read_mark_sent = NULL WHERE id = ?1",
            [conversation],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn conversations(&self) -> StoreResult<Vec<ConversationRow>> {
        let conn = self.conn()?;
        let mut st = conn.prepare(
            "SELECT c.id, c.epoch, c.after_seq, c.updated_at,
                    (SELECT count(*) FROM entries e WHERE e.conversation_id = c.id) FROM conversations c ORDER BY c.updated_at, c.id",
        )?;
        let rows = st.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, Option<String>>(1)?,
                r.get::<_, Option<i64>>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, i64>(4)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (id, epoch, seq, updated_at, entries) = row?;
            let older = self.older(&id)?;
            out.push(ConversationRow {
                cursor: epoch
                    .zip(seq)
                    .map(|(epoch, after_seq)| Cursor { epoch, after_seq }),
                older,
                id,
                updated_at,
                entries,
            });
        }
        Ok(out)
    }

    pub fn add_pending_get(&self, conversation: &str, id: &str) -> StoreResult<()> {
        self.conn()?.execute(
            "INSERT OR IGNORE INTO pending_gets (conversation_id, id) VALUES (?1, ?2)",
            params![conversation, id],
        )?;
        Ok(())
    }

    pub fn pending_gets(&self, conversation: &str) -> StoreResult<Vec<String>> {
        let conn = self.conn()?;
        let mut st =
            conn.prepare("SELECT id FROM pending_gets WHERE conversation_id = ?1 ORDER BY id")?;
        let rows = st.query_map([conversation], |r| r.get(0))?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// The read mark last sent (`chat.entries.mark read_pos`), so it is sent only when it moves.
    pub fn read_mark(&self, conversation: &str) -> StoreResult<Option<i64>> {
        Ok(self
            .conn()?
            .query_row(
                "SELECT read_mark_sent FROM conversations WHERE id = ?1",
                [conversation],
                |r| r.get(0),
            )
            .optional()?
            .flatten())
    }

    pub fn set_read_mark(&self, conversation: &str, pos: i64) -> StoreResult<()> {
        self.ensure_conversation(conversation)?;
        self.conn()?.execute(
            "UPDATE conversations SET read_mark_sent = ?2 WHERE id = ?1",
            params![conversation, pos],
        )?;
        Ok(())
    }

    // ---- the outbox (messages only) ----

    pub fn outbox_add(&self, id: &str, text: &str, created_at: &str) -> StoreResult<OutboxRow> {
        let conn = self.conn()?;
        let n: i64 = conn.query_row("SELECT coalesce(max(n), 0) + 1 FROM outbox", [], |r| {
            r.get(0)
        })?;
        conn.execute(
            "INSERT INTO outbox (id, n, text, created_at, state) VALUES (?1, ?2, ?3, ?4, 'waiting')",
            params![id, n, text, created_at],
        )?;
        self.outbox_get(id)?
            .ok_or_else(|| StoreError::Sql("the outbox row was not written".into()))
    }

    pub fn outbox(&self) -> StoreResult<Vec<OutboxRow>> {
        let conn = self.conn()?;
        let mut st = conn.prepare(
            "SELECT id, n, text, created_at, state, attempts, reason, request_id, entry_id FROM outbox ORDER BY n",
        )?;
        let rows = st.query_map([], |r| {
            Ok(OutboxRow {
                id: r.get(0)?,
                n: r.get(1)?,
                text: r.get(2)?,
                created_at: r.get(3)?,
                state: r.get(4)?,
                attempts: r.get(5)?,
                reason: r.get(6)?,
                request_id: r.get(7)?,
                entry_id: r.get(8)?,
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    pub fn outbox_get(&self, id: &str) -> StoreResult<Option<OutboxRow>> {
        Ok(self.outbox()?.into_iter().find(|r| r.id == id))
    }

    pub fn outbox_update(&self, row: &OutboxRow) -> StoreResult<()> {
        self.conn()?.execute(
            "UPDATE outbox SET state = ?2, attempts = ?3, reason = ?4, request_id = ?5, entry_id = ?6 WHERE id = ?1",
            params![row.id, row.state, row.attempts, row.reason, row.request_id, row.entry_id],
        )?;
        Ok(())
    }

    pub fn outbox_remove(&self, id: &str) -> StoreResult<bool> {
        Ok(self
            .conn()?
            .execute("DELETE FROM outbox WHERE id = ?1", [id])?
            > 0)
    }

    // ---- Forget this Mac ----

    /// Closes the database and deletes its files (the database, its journal and its WAL): nothing of this Mac is
    /// left on the phone. The key was never kept here; the app forgets its wrapped copy (S9).
    pub fn forget(mut self) -> StoreResult<()> {
        if let Some(conn) = self.conn.take() {
            conn.close()
                .map_err(|(_, e)| StoreError::Sql(e.to_string()))?;
        }
        for suffix in ["", "-journal", "-wal", "-shm"] {
            let mut p = self.path.clone().into_os_string();
            p.push(suffix);
            match std::fs::remove_file(PathBuf::from(p)) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => {
                    return Err(StoreError::Sql(format!(
                        "the store's file could not be removed ({e})"
                    )))
                }
            }
        }
        Ok(())
    }
}
