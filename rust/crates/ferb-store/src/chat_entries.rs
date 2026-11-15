//! The conversation as the engine keeps it (stage 2; decisions: ENG-8, ENG-10, STORE-401): `chat_entries` and its
//! counters, client submissions, clients and their marks (migration 0063). Every row is a
//! [`ferb_core::chat_entry::Entry`] at its latest revision; every client (FERB.app, the phones) reads pages, changes
//! after a cursor and its own marks from here.
//!
//! The functions take a `&Connection` so the engine can call them inside the one transaction that also writes its
//! journal row (`Db::transaction` hands the block a `&Connection`); a function that writes more than one row should
//! run inside one. `seq` comes from one per-conversation counter (`chat_streams.last_seq`) bumped inside the caller's
//! transaction, so change order is commit order, never wall-clock. `pos` is set once: the engine's own entries count
//! up from [`POS_BASE`]; imported older history goes below the stream's lowest position.
//!
//! These tables are kept forever (STORE-401: chat stays): no retention rule names them, the clean-up never writes
//! them (`cleanup::PROTECTED`) and nothing here deletes a row: a deleted entry is a tombstone revision.
//!
//! The store does not redact: entries arrive redacted from the engine, and the imported rows were redacted when the
//! app saved them (`chat_history.py:82`).

use std::path::Path;

use ferb_core::chat_entry::Entry;
use rusqlite::{params, params_from_iter, Connection, OpenFlags, OptionalExtension, Row};
use serde::{de::DeserializeOwned, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::{pyval::loads, Db, Error, Result};

/// The first position of a stream's own entries (2^32). Everything imported goes below the stream's `low_pos`, so
/// positions stay unique, are never renumbered and sort chronologically across imports.
pub const POS_BASE: u64 = 1 << 32;

/// The schema version of migration 0063, the first above Python's last (0062): what a store made by this build has
/// beyond the one the Python build made (decisions: ENG-11, ENG-8).
pub const SCHEMA_VERSION: i64 = 63;
/// The tables migration 0063 adds, all kept forever (`cleanup::PROTECTED` names them, in this order).
pub const TABLES: [&str; 6] = [
    "chat_entries",
    "chat_streams",
    "chat_submissions",
    "chat_clients",
    "chat_client_marks",
    "chat_entries_from",
];

/// `imported_from` of the rows migration 0063 carried over from `chat_history`.
pub const FROM_CHAT_HISTORY: &str = "chat_history";
/// `imported_from` of the rows [`import_archive`] added.
pub const FROM_ARCHIVE: &str = "archive";

/// A new stream epoch: 32 lowercase hex characters from the system's random source.
pub fn new_epoch() -> String {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).expect("the system's random source");
    hex::encode(bytes)
}

fn refused<T>(why: impl Into<String>) -> Result<T> {
    Err(Error::Value(why.into()))
}

fn to_i64(value: u64, what: &str) -> Result<i64> {
    i64::try_from(value).or_else(|_| refused(format!("{what} is out of range")))
}

fn to_u64(value: i64, what: &str) -> Result<u64> {
    u64::try_from(value).map_err(|_| Error::Json(format!("a stored {what} is negative")))
}

/// A serde unit variant's snake_case word (`EntryKind::Message` → "message").
fn word<T: Serialize>(value: &T) -> Result<String> {
    match serde_json::to_value(value) {
        Ok(Value::String(word)) => Ok(word),
        _ => Err(Error::Json(
            "a word field did not serialize to a string".to_owned(),
        )),
    }
}

fn from_word<T: DeserializeOwned>(text: String, what: &str) -> Result<T> {
    serde_json::from_value(Value::String(text.clone()))
        .map_err(|_| Error::Json(format!("a stored {what} {text:?} is not one FERB knows")))
}

// ---------------------------------------------------------------------------------------------------------------
// Streams

/// A conversation's counters (`chat_streams`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stream {
    pub conversation_id: String,
    /// Random per stream: a client's cursor from another epoch reads from the start.
    pub epoch: String,
    /// The highest `seq` given (the conversation's change counter).
    pub last_seq: u64,
    /// The highest and lowest `pos` used.
    pub last_pos: u64,
    pub low_pos: u64,
}

/// The conversation's stream, if it has one.
pub fn stream(conn: &Connection, conversation: &str) -> Result<Option<Stream>> {
    let row = conn
        .query_row(
            "SELECT epoch, last_seq, last_pos, low_pos FROM chat_streams WHERE conversation_id = ?",
            [conversation],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, i64>(2)?,
                    r.get::<_, i64>(3)?,
                ))
            },
        )
        .optional()?;
    row.map(|(epoch, last_seq, last_pos, low_pos)| {
        Ok(Stream {
            conversation_id: conversation.to_owned(),
            epoch,
            last_seq: to_u64(last_seq, "last_seq")?,
            last_pos: to_u64(last_pos, "last_pos")?,
            low_pos: to_u64(low_pos, "low_pos")?,
        })
    })
    .transpose()
}

/// The conversation's stream, made when absent with `epoch` (a new one has no entries: `last_seq` 0, `last_pos`
/// `POS_BASE - 1`, `low_pos` `POS_BASE`). An existing stream keeps its own epoch.
pub fn open_stream(conn: &Connection, conversation: &str, epoch: &str) -> Result<Stream> {
    conn.execute(
        "INSERT INTO chat_streams (conversation_id, epoch, last_seq, last_pos, low_pos) VALUES (?, ?, 0, ?, ?) \
         ON CONFLICT (conversation_id) DO NOTHING",
        params![
            conversation,
            epoch,
            to_i64(POS_BASE - 1, "pos")?,
            to_i64(POS_BASE, "pos")?
        ],
    )?;
    stream(conn, conversation)?.ok_or_else(|| Error::Value("the stream was not made".to_owned()))
}

fn existing_stream(conn: &Connection, conversation: &str) -> Result<Stream> {
    match stream(conn, conversation)? {
        Some(stream) => Ok(stream),
        None => refused(format!("conversation {conversation:?} has no stream")),
    }
}

/// Bump the conversation's change counter by one and return the new value. Call it inside the transaction that
/// writes the change, so the order of `seq` is the order of commits. Refused when the conversation has no stream.
pub fn next_seq(conn: &Connection, conversation: &str) -> Result<u64> {
    let seq: Option<i64> = conn
        .query_row(
            "UPDATE chat_streams SET last_seq = last_seq + 1 WHERE conversation_id = ? RETURNING last_seq",
            [conversation],
            |r| r.get(0),
        )
        .optional()?;
    match seq {
        Some(seq) => to_u64(seq, "last_seq"),
        None => refused(format!("conversation {conversation:?} has no stream")),
    }
}

/// The conversation's change counter (0 without a stream).
pub fn head(conn: &Connection, conversation: &str) -> Result<u64> {
    Ok(stream(conn, conversation)?.map_or(0, |s| s.last_seq))
}

/// Every conversation that has a stream, by id.
pub fn conversations(conn: &Connection) -> Result<Vec<String>> {
    strings(
        conn,
        "SELECT conversation_id FROM chat_streams ORDER BY conversation_id",
        params![],
    )
}

fn strings(conn: &Connection, sql: &str, args: impl rusqlite::Params) -> Result<Vec<String>> {
    let mut statement = conn.prepare(sql)?;
    let rows = statement.query_map(args, |r| r.get(0))?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

// ---------------------------------------------------------------------------------------------------------------
// Entries

const COLUMNS: &str = "id, conversation_id, pos, seq, rev, kind, state, open, request_id, task_id, approval_id, ref, \
                       answers, placement, origin, client_id, submission_id, journal_id, created_at, updated_at, day, \
                       body_json, deleted";

/// An `Entry` from a row selected with `COLUMNS`.
fn read(row: &Row<'_>) -> rusqlite::Result<RawEntry> {
    Ok(RawEntry {
        id: row.get(0)?,
        conversation_id: row.get(1)?,
        pos: row.get(2)?,
        seq: row.get(3)?,
        rev: row.get(4)?,
        kind: row.get(5)?,
        state: row.get(6)?,
        open: row.get(7)?,
        request_id: row.get(8)?,
        task_id: row.get(9)?,
        approval_id: row.get(10)?,
        reference: row.get(11)?,
        answers: row.get(12)?,
        placement: row.get(13)?,
        origin: row.get(14)?,
        client_id: row.get(15)?,
        submission_id: row.get(16)?,
        journal_id: row.get(17)?,
        created_at: row.get(18)?,
        updated_at: row.get(19)?,
        day: row.get(20)?,
        body_json: row.get(21)?,
        deleted: row.get(22)?,
    })
}

/// A row as SQLite gives it, before its words and body are read.
struct RawEntry {
    id: String,
    conversation_id: String,
    pos: i64,
    seq: i64,
    rev: i64,
    kind: String,
    state: String,
    open: i64,
    request_id: Option<String>,
    task_id: Option<String>,
    approval_id: Option<String>,
    reference: Option<String>,
    answers: Option<String>,
    placement: Option<String>,
    origin: Option<String>,
    client_id: Option<String>,
    submission_id: Option<String>,
    journal_id: Option<String>,
    created_at: String,
    updated_at: String,
    day: String,
    body_json: String,
    deleted: i64,
}

impl RawEntry {
    fn entry(self) -> Result<Entry> {
        Ok(Entry {
            id: self.id,
            conversation_id: self.conversation_id,
            pos: to_u64(self.pos, "pos")?,
            seq: to_u64(self.seq, "seq")?,
            rev: to_u64(self.rev, "rev")?,
            kind: from_word(self.kind, "kind")?,
            state: from_word(self.state, "state")?,
            open: self.open != 0,
            request_id: self.request_id,
            task_id: self.task_id,
            approval_id: self.approval_id,
            reference: self.reference,
            answers: self.answers,
            placement: self
                .placement
                .map(|p| from_word(p, "placement"))
                .transpose()?,
            origin: self.origin,
            client_id: self.client_id,
            submission_id: self.submission_id,
            journal_id: self.journal_id,
            created_at: self.created_at,
            updated_at: self.updated_at,
            day: self.day,
            body: loads(&self.body_json)?,
            deleted: self.deleted != 0,
        })
    }
}

fn entries(conn: &Connection, sql: &str, args: impl rusqlite::Params) -> Result<Vec<Entry>> {
    let mut statement = conn.prepare(sql)?;
    let rows = statement.query_map(args, read)?;
    rows.map(|row| row?.entry()).collect()
}

/// The entry with this id, any conversation.
pub fn get(conn: &Connection, id: &str) -> Result<Option<Entry>> {
    let raw = conn
        .query_row(
            &format!("SELECT {COLUMNS} FROM chat_entries WHERE id = ?"),
            [id],
            read,
        )
        .optional()?;
    raw.map(RawEntry::entry).transpose()
}

/// Store `entry` exactly as given: inserted, or replacing the stored row with the same id (the row keeps its
/// `imported_from`). The stream's `last_pos`/`low_pos` move to cover `entry.pos`. Refused (`Error::Value`), with
/// nothing written, when the conversation has no stream, `entry.seq` is above the stream's counter (bump it with
/// [`next_seq`] first) or not above the stored revision's, or the stored revision has another `pos` or conversation.
/// A `pos` or `seq` another entry of the conversation holds is refused by SQLite's unique constraints.
pub fn put(conn: &Connection, entry: &Entry) -> Result<()> {
    let stream = existing_stream(conn, &entry.conversation_id)?;
    if entry.seq > stream.last_seq {
        return refused(format!(
            "entry {:?} has seq {} above the conversation's counter {}",
            entry.id, entry.seq, stream.last_seq
        ));
    }
    if let Some(stored) = get(conn, &entry.id)? {
        if stored.conversation_id != entry.conversation_id {
            return refused(format!(
                "entry {:?} belongs to another conversation",
                entry.id
            ));
        }
        if stored.pos != entry.pos {
            return refused(format!(
                "entry {:?} has pos {}; its pos is set once ({})",
                entry.id, entry.pos, stored.pos
            ));
        }
        if entry.seq <= stored.seq {
            return refused(format!(
                "entry {:?} has seq {}, not above its stored seq {}",
                entry.id, entry.seq, stored.seq
            ));
        }
    }
    let body = serde_json::to_string(&entry.body).map_err(|e| Error::Json(e.to_string()))?;
    let placement = entry.placement.as_ref().map(word).transpose()?;
    conn.execute(
        &format!(
            "INSERT INTO chat_entries ({COLUMNS}) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, \
             ?, ?) ON CONFLICT (id) DO UPDATE SET seq = excluded.seq, rev = excluded.rev, kind = excluded.kind, \
             state = excluded.state, open = excluded.open, request_id = excluded.request_id, \
             task_id = excluded.task_id, approval_id = excluded.approval_id, ref = excluded.ref, \
             answers = excluded.answers, placement = excluded.placement, origin = excluded.origin, \
             client_id = excluded.client_id, submission_id = excluded.submission_id, \
             journal_id = excluded.journal_id, created_at = excluded.created_at, updated_at = excluded.updated_at, \
             day = excluded.day, body_json = excluded.body_json, deleted = excluded.deleted"
        ),
        params![
            entry.id,
            entry.conversation_id,
            to_i64(entry.pos, "pos")?,
            to_i64(entry.seq, "seq")?,
            to_i64(entry.rev, "rev")?,
            word(&entry.kind)?,
            word(&entry.state)?,
            i64::from(entry.open),
            entry.request_id,
            entry.task_id,
            entry.approval_id,
            entry.reference,
            entry.answers,
            placement,
            entry.origin,
            entry.client_id,
            entry.submission_id,
            entry.journal_id,
            entry.created_at,
            entry.updated_at,
            entry.day,
            body,
            i64::from(entry.deleted),
        ],
    )?;
    cover(conn, &entry.conversation_id, entry.pos)
}

/// Move the stream's `last_pos`/`low_pos` to cover `pos`.
fn cover(conn: &Connection, conversation: &str, pos: u64) -> Result<()> {
    let pos = to_i64(pos, "pos")?;
    conn.execute(
        "UPDATE chat_streams SET last_pos = max(last_pos, ?1), low_pos = min(low_pos, ?1) WHERE conversation_id = ?2",
        params![pos, conversation],
    )?;
    Ok(())
}

/// A new entry: `pos` = the stream's `last_pos + 1`, `seq` = [`next_seq`], `rev` 1; the given numbers are ignored.
/// The stream is made (with a [`new_epoch`]) when the conversation has none. Refused for an id already stored.
/// Returns the entry as stored.
pub fn create(conn: &Connection, entry: Entry) -> Result<Entry> {
    if get(conn, &entry.id)?.is_some() {
        return refused(format!("entry {:?} already exists", entry.id));
    }
    let stream = match stream(conn, &entry.conversation_id)? {
        Some(stream) => stream,
        None => open_stream(conn, &entry.conversation_id, &new_epoch())?,
    };
    let entry = Entry {
        pos: stream.last_pos + 1,
        seq: next_seq(conn, &entry.conversation_id)?,
        rev: 1,
        ..entry
    };
    put(conn, &entry)?;
    Ok(entry)
}

/// A revision of a stored entry: `entry`'s content with the stored `pos`, `rev + 1`, a new `seq` and `updated_at` =
/// `at`. A revision that changes nothing stored ([`Entry::same_content`]) is not made (ENG-8): the stored entry comes
/// back as it was. Refused for an unknown id (or one of another conversation, by [`put`]).
pub fn revise(conn: &Connection, entry: Entry, at: &str) -> Result<Entry> {
    let Some(stored) = get(conn, &entry.id)? else {
        return refused(format!("entry {:?} is not stored", entry.id));
    };
    let entry = Entry {
        pos: stored.pos,
        ..entry
    };
    if entry.same_content(&stored) {
        return Ok(stored);
    }
    let entry = Entry {
        rev: stored.rev + 1,
        seq: next_seq(conn, &entry.conversation_id)?,
        updated_at: at.to_owned(),
        ..entry
    };
    put(conn, &entry)?;
    Ok(entry)
}

/// Delete an entry the way clients can see: a revision with `deleted` set, the body emptied (`{}`) and `open`
/// cleared (a deleted entry waits on nothing). The row stays. `None` for an unknown id; an entry already deleted
/// comes back unchanged.
pub fn tombstone(conn: &Connection, id: &str, at: &str) -> Result<Option<Entry>> {
    let Some(stored) = get(conn, id)? else {
        return Ok(None);
    };
    if stored.deleted {
        return Ok(Some(stored));
    }
    let gone = Entry {
        deleted: true,
        open: false,
        body: Value::Object(serde_json::Map::new()),
        ..stored
    };
    revise(conn, gone, at).map(Some)
}

/// A page of a conversation (`page`).
#[derive(Debug, Clone, PartialEq)]
pub struct Page {
    /// Oldest first.
    pub entries: Vec<Entry>,
    /// Older entries exist (before the first one here, within the same day filter).
    pub more: bool,
}

/// The newest `limit` entries with `pos < before_pos` (any position when `None`), only of `day` when given,
/// returned in `pos` order. Tombstones are included: clients must see them deleted. Refused for a `limit` of 0.
pub fn page(
    conn: &Connection,
    conversation: &str,
    before_pos: Option<u64>,
    day: Option<&str>,
    limit: usize,
) -> Result<Page> {
    if limit == 0 {
        return refused("limit must be at least 1");
    }
    let before = match before_pos {
        Some(pos) => to_i64(pos, "before_pos")?,
        None => i64::MAX,
    };
    let fetch = i64::try_from(limit).unwrap_or(i64::MAX - 1) + 1;
    let mut found = match day {
        Some(day) => entries(
            conn,
            &format!(
                "SELECT {COLUMNS} FROM chat_entries WHERE conversation_id = ? AND day = ? AND pos < ? \
                 ORDER BY pos DESC LIMIT ?"
            ),
            params![conversation, day, before, fetch],
        )?,
        None => entries(
            conn,
            &format!(
                "SELECT {COLUMNS} FROM chat_entries WHERE conversation_id = ? AND pos < ? ORDER BY pos DESC LIMIT ?"
            ),
            params![conversation, before, fetch],
        )?,
    };
    let more = found.len() > limit;
    found.truncate(limit);
    found.reverse();
    Ok(Page {
        entries: found,
        more,
    })
}

/// What changed in a conversation after a cursor (`changes`).
#[derive(Debug, Clone, PartialEq)]
pub struct Changes {
    /// Ascending `seq`, each entry once at its latest state (a row holds only its latest revision).
    pub entries: Vec<Entry>,
    /// Further changes exist after the last one here.
    pub more: bool,
    /// The conversation's change counter now.
    pub head: u64,
}

/// The entries whose latest revision has `seq > after_seq`, at most `limit`, in `seq` order. Refused for a `limit`
/// of 0.
pub fn changes(
    conn: &Connection,
    conversation: &str,
    after_seq: u64,
    limit: usize,
) -> Result<Changes> {
    if limit == 0 {
        return refused("limit must be at least 1");
    }
    let fetch = i64::try_from(limit).unwrap_or(i64::MAX - 1) + 1;
    let mut found = entries(
        conn,
        &format!(
            "SELECT {COLUMNS} FROM chat_entries WHERE conversation_id = ? AND seq > ? ORDER BY seq LIMIT ?"
        ),
        params![conversation, to_i64(after_seq, "after_seq")?, fetch],
    )?;
    let more = found.len() > limit;
    found.truncate(limit);
    Ok(Changes {
        entries: found,
        more,
        head: head(conn, conversation)?,
    })
}

/// Each day that has an entry in the conversation (tombstones included), newest first.
pub fn days(conn: &Connection, conversation: &str) -> Result<Vec<String>> {
    strings(
        conn,
        "SELECT DISTINCT day FROM chat_entries WHERE conversation_id = ? ORDER BY day DESC",
        [conversation],
    )
}

/// The conversation's open entries (waiting on FERB or the owner), in `pos` order: what the engine's projector
/// takes back after a restart.
pub fn open_entries(conn: &Connection, conversation: &str) -> Result<Vec<Entry>> {
    entries(
        conn,
        &format!(
            "SELECT {COLUMNS} FROM chat_entries WHERE conversation_id = ? AND open = 1 ORDER BY pos"
        ),
        [conversation],
    )
}

/// Every entry of the conversation that belongs to one of `requests`, in `pos` order.
pub fn of_requests(conn: &Connection, conversation: &str, requests: &[&str]) -> Result<Vec<Entry>> {
    let mut out = Vec::new();
    for chunk in requests.chunks(500) {
        let mut args: Vec<&str> = vec![conversation];
        args.extend_from_slice(chunk);
        out.extend(entries(
            conn,
            &format!(
                "SELECT {COLUMNS} FROM chat_entries WHERE conversation_id = ? AND request_id IN ({})",
                crate::pyval::marks(chunk.len())
            ),
            params_from_iter(args),
        )?);
    }
    out.sort_by_key(|e| e.pos);
    Ok(out)
}

/// The conversation's last `n` entries by `pos`, returned in `pos` order.
pub fn recent(conn: &Connection, conversation: &str, n: usize) -> Result<Vec<Entry>> {
    let n = i64::try_from(n).unwrap_or(i64::MAX);
    let mut found = entries(
        conn,
        &format!(
            "SELECT {COLUMNS} FROM chat_entries WHERE conversation_id = ? ORDER BY pos DESC LIMIT ?"
        ),
        params![conversation, n],
    )?;
    found.reverse();
    Ok(found)
}

/// Every `ref` the conversation's entries carry, each once, in the order first used: what was posted already.
pub fn refs(conn: &Connection, conversation: &str) -> Result<Vec<String>> {
    strings(
        conn,
        "SELECT ref FROM chat_entries WHERE conversation_id = ? AND ref IS NOT NULL GROUP BY ref ORDER BY MIN(pos)",
        [conversation],
    )
}

/// The conversation an entry of this request is in.
pub fn conversation_of_request(conn: &Connection, request_id: &str) -> Result<Option<String>> {
    Ok(conn
        .query_row(
            "SELECT conversation_id FROM chat_entries WHERE request_id = ? ORDER BY pos LIMIT 1",
            [request_id],
            |r| r.get(0),
        )
        .optional()?)
}

/// The conversation an entry of this task is in.
pub fn conversation_of_task(conn: &Connection, task_id: &str) -> Result<Option<String>> {
    Ok(conn
        .query_row(
            "SELECT conversation_id FROM chat_entries WHERE task_id = ? ORDER BY pos LIMIT 1",
            [task_id],
            |r| r.get(0),
        )
        .optional()?)
}

/// The journal's highest `seq` when migration 0063 ran: the engine's journal backstop reads only rows after it.
/// `None` when the journal was empty then (or the store was made at 0063 or later): every journal row is newer.
pub fn journal_from(conn: &Connection) -> Result<Option<i64>> {
    Ok(conn
        .query_row(
            "SELECT journal_seq FROM chat_entries_from WHERE singleton = 1",
            [],
            |r| r.get(0),
        )
        .optional()?)
}

// ---------------------------------------------------------------------------------------------------------------
// Cursors

/// What a client's cursor is worth now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CursorCheck {
    /// Read the changes after it; `head` is the counter now.
    Ok { head: u64 },
    /// The cursor is from another epoch (or the conversation has no stream): read from the start. `epoch` is the
    /// current one, if any.
    ResetRequired { epoch: Option<String> },
    /// The cursor is beyond the counter (a client that read a store since replaced): read from the start too.
    Ahead { head: u64 },
}

/// Check a client's cursor (`epoch`, `after_seq`) against the conversation's stream.
pub fn check_cursor(
    conn: &Connection,
    conversation: &str,
    epoch: &str,
    after_seq: u64,
) -> Result<CursorCheck> {
    Ok(match stream(conn, conversation)? {
        None => CursorCheck::ResetRequired { epoch: None },
        Some(s) if s.epoch != epoch => CursorCheck::ResetRequired {
            epoch: Some(s.epoch),
        },
        Some(s) if after_seq > s.last_seq => CursorCheck::Ahead { head: s.last_seq },
        Some(s) => CursorCheck::Ok { head: s.last_seq },
    })
}

// ---------------------------------------------------------------------------------------------------------------
// Submissions

/// An accepted `chat.send`, by its client's own key (`chat_submissions`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Submission {
    pub client_id: String,
    pub submission_id: String,
    pub conversation_id: String,
    pub request_id: String,
    /// The message entry it made, and that entry's `seq` at acceptance.
    pub entry_id: String,
    pub seq: u64,
    /// [`text_sha256`] of the message's redacted text.
    pub text_sha256: String,
    pub accepted_at: String,
}

/// What a submission is, against those already accepted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Accept {
    /// Not seen before.
    New,
    /// The same key with the same words: return the stored acceptance.
    Repeat(Submission),
    /// The same key with other words: the stored acceptance it clashes with.
    Conflict(Submission),
}

/// The hex SHA-256 of a message's redacted text (its UTF-8 bytes).
pub fn text_sha256(redacted: &str) -> String {
    hex::encode(Sha256::digest(redacted.as_bytes()))
}

/// Whether `(client, submission_id)` was accepted before, and with the same words (`text_sha256`).
pub fn check_submission(
    conn: &Connection,
    client: &str,
    submission_id: &str,
    text_sha256: &str,
) -> Result<Accept> {
    let stored = conn
        .query_row(
            "SELECT conversation_id, request_id, entry_id, seq, text_sha256, accepted_at FROM chat_submissions \
             WHERE client_id = ? AND submission_id = ?",
            [client, submission_id],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, i64>(3)?,
                    r.get::<_, String>(4)?,
                    r.get::<_, String>(5)?,
                ))
            },
        )
        .optional()?;
    let Some((conversation_id, request_id, entry_id, seq, sum, accepted_at)) = stored else {
        return Ok(Accept::New);
    };
    let submission = Submission {
        client_id: client.to_owned(),
        submission_id: submission_id.to_owned(),
        conversation_id,
        request_id,
        entry_id,
        seq: to_u64(seq, "seq")?,
        text_sha256: sum,
        accepted_at,
    };
    Ok(if submission.text_sha256 == text_sha256 {
        Accept::Repeat(submission)
    } else {
        Accept::Conflict(submission)
    })
}

/// Record an accepted submission. A key already recorded is an error (check it first with [`check_submission`]).
pub fn record_submission(conn: &Connection, submission: &Submission) -> Result<()> {
    conn.execute(
        "INSERT INTO chat_submissions (client_id, submission_id, conversation_id, request_id, entry_id, seq, \
         text_sha256, accepted_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        params![
            submission.client_id,
            submission.submission_id,
            submission.conversation_id,
            submission.request_id,
            submission.entry_id,
            to_i64(submission.seq, "seq")?,
            submission.text_sha256,
            submission.accepted_at
        ],
    )?;
    Ok(())
}

// ---------------------------------------------------------------------------------------------------------------
// Clients and marks

/// Note a client seen at `at` (first seen kept, last seen moved).
pub fn touch_client(conn: &Connection, client_id: &str, at: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO chat_clients (client_id, first_seen, last_seen) VALUES (?1, ?2, ?2) \
         ON CONFLICT (client_id) DO UPDATE SET last_seen = excluded.last_seen",
        params![client_id, at],
    )?;
    Ok(())
}

/// How far a client has received and read a conversation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Marks {
    pub client_id: String,
    pub conversation_id: String,
    /// The highest `seq` the client holds.
    pub delivered_seq: u64,
    /// The highest `pos` the owner has read there.
    pub read_pos: u64,
    /// When a mark last moved.
    pub updated_at: String,
}

/// Move a client's marks forward: each becomes `max(old, new)`; a lower value is ignored, never refused, and
/// `updated_at` changes only when a mark moved (or the marks are new). Returns the marks now.
pub fn mark(
    conn: &Connection,
    client: &str,
    conversation: &str,
    delivered_seq: Option<u64>,
    read_pos: Option<u64>,
    at: &str,
) -> Result<Marks> {
    let delivered = to_i64(delivered_seq.unwrap_or(0), "delivered_seq")?;
    let read = to_i64(read_pos.unwrap_or(0), "read_pos")?;
    conn.execute(
        "INSERT INTO chat_client_marks (client_id, conversation_id, delivered_seq, read_pos, updated_at) \
         VALUES (?, ?, ?, ?, ?) ON CONFLICT (client_id, conversation_id) DO UPDATE SET \
         delivered_seq = max(delivered_seq, excluded.delivered_seq), read_pos = max(read_pos, excluded.read_pos), \
         updated_at = CASE WHEN excluded.delivered_seq > delivered_seq OR excluded.read_pos > read_pos \
         THEN excluded.updated_at ELSE updated_at END",
        params![client, conversation, delivered, read, at],
    )?;
    marks(conn, client, conversation)?
        .ok_or_else(|| Error::Value("the marks were not kept".to_owned()))
}

/// A client's marks in a conversation, if any.
pub fn marks(conn: &Connection, client: &str, conversation: &str) -> Result<Option<Marks>> {
    let row = conn
        .query_row(
            "SELECT delivered_seq, read_pos, updated_at FROM chat_client_marks \
             WHERE client_id = ? AND conversation_id = ?",
            [client, conversation],
            |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, String>(2)?,
                ))
            },
        )
        .optional()?;
    row.map(|(delivered, read, updated_at)| {
        Ok(Marks {
            client_id: client.to_owned(),
            conversation_id: conversation.to_owned(),
            delivered_seq: to_u64(delivered, "delivered_seq")?,
            read_pos: to_u64(read, "read_pos")?,
            updated_at,
        })
    })
    .transpose()
}

// ---------------------------------------------------------------------------------------------------------------
// A forgotten notice

/// `chat_history::clear_notice` for the entries (rule 7: a forgotten notice's words go everywhere): every notice
/// entry with this `ref` says `text` and quotes nothing, and so does every legacy entry holding the app's saved
/// notice with this `ref` (its inner body's `text` and `quotes`, set by SQLite's `json_set` as `chat_history`'s are).
/// Each is a revision (`rev + 1`, a new `seq`, `updated_at` = `at`) so clients see it; an entry already saying so,
/// or deleted, is left alone. Returns the entries revised. Run it inside the transaction of the forget.
pub fn clear_notice(conn: &Connection, reference: &str, text: &str, at: &str) -> Result<usize> {
    let found: Vec<(String, String, String, String)> = {
        let mut statement = conn.prepare(
            "SELECT id, conversation_id, body_json, CASE kind WHEN 'notice' \
             THEN json_set(body_json, '$.text', ?1, '$.quotes', json('[]')) \
             ELSE json_set(body_json, '$.body.text', ?1, '$.body.quotes', json('[]')) END \
             FROM chat_entries WHERE ref = ?2 AND deleted = 0 AND (kind = 'notice' OR (kind = 'legacy' \
             AND json_valid(body_json) AND json_extract(body_json, '$.kind') = 'notice')) ORDER BY conversation_id, pos",
        )?;
        let rows = statement.query_map(params![text, reference], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
        })?;
        rows.collect::<rusqlite::Result<_>>()?
    };
    let mut revised = 0;
    for (id, conversation, old, body) in found {
        if loads(&old)? == loads(&body)? {
            continue;
        }
        let seq = to_i64(next_seq(conn, &conversation)?, "seq")?;
        conn.execute(
            "UPDATE chat_entries SET body_json = ?, rev = rev + 1, seq = ?, updated_at = ? WHERE id = ?",
            params![body, seq, at, id],
        )?;
        revised += 1;
    }
    Ok(revised)
}

// ---------------------------------------------------------------------------------------------------------------
// The pre-Rust archive

/// What [`import_archive`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Imported {
    /// Entries added.
    pub added: usize,
    /// Archive rows whose entry already existed (any conversation): carried over by migration 0063 or imported by
    /// an earlier run.
    pub skipped: usize,
}

/// `path` as a `file:` URI: every byte but the unreserved ones and `/` percent-encoded.
fn file_uri(path: &Path) -> String {
    use std::os::unix::ffi::OsStrExt;
    let mut uri = String::from("file:");
    for &b in path.as_os_str().as_bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'/' | b'-' | b'_' | b'.' | b'~') {
            uri.push(char::from(b));
        } else {
            uri.push_str(&format!("%{b:02X}"));
        }
    }
    uri
}

/// Import the pre-Rust archive's saved chat once (ENG-10; owner: once, read-only). Not run by itself: the owner
/// starts it.
///
/// `archive` (a `ferb.db` of the Python build, STORE-421) is opened read-only and immutable (`mode=ro&immutable=1`
/// with `SQLITE_OPEN_READ_ONLY | SQLITE_OPEN_URI`): no migration, no pragma, nothing written beside it. Because an
/// immutable read skips a write-ahead log, an archive with a non-empty `-wal` file is refused (import from a copy
/// checkpointed first). A missing archive is an error; one with no `chat_history` table imports nothing.
///
/// Its `chat_history` rows, in `(at, id)` order, become legacy entries of `conversation` in ONE transaction on `db`,
/// as migration 0063 makes them (id `'h:' || id`, the same body concatenation in SQL so the app's bytes are kept,
/// `imported_from` 'archive'), skipping every row whose entry exists already (any conversation), so a second run
/// adds nothing. The new entries take contiguous positions just below the stream's `low_pos` (the stream is made
/// when absent), in chronological order, and each a new `seq` from the counter, so a client holding a cursor sees
/// them as changes.
pub fn import_archive(db: &Db, archive: &Path, conversation: &str) -> Result<Imported> {
    if !archive.is_file() {
        return Err(Error::Io(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("no archive at {}", archive.display()),
        )));
    }
    let mut wal = archive.as_os_str().to_owned();
    wal.push("-wal");
    if std::fs::metadata(Path::new(&wal)).is_ok_and(|m| m.len() > 0) {
        return refused(format!(
            "the archive has a write-ahead log ({}) that a read-only import would skip; import from a checkpointed \
             copy",
            Path::new(&wal).display()
        ));
    }
    let source = Connection::open_with_flags(
        format!("{}?mode=ro&immutable=1", file_uri(archive)),
        OpenFlags::SQLITE_OPEN_READ_ONLY
            | OpenFlags::SQLITE_OPEN_URI
            | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    let has_table: bool = source.query_row(
        "SELECT EXISTS (SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'chat_history')",
        [],
        |r| r.get(0),
    )?;
    type Row = (
        String,
        String,
        String,
        String,
        Option<String>,
        String,
        String,
    );
    let rows: Vec<Row> = if has_table {
        let mut statement = source.prepare(
            "SELECT id, kind, at, day, ref, body_json, updated_at FROM chat_history ORDER BY at, id",
        )?;
        let rows = statement.query_map([], |r| {
            Ok((
                r.get(0)?,
                r.get(1)?,
                r.get(2)?,
                r.get(3)?,
                r.get(4)?,
                r.get(5)?,
                r.get(6)?,
            ))
        })?;
        rows.collect::<rusqlite::Result<_>>()?
    } else {
        Vec::new()
    };
    drop(source);
    db.transaction(|conn| -> Result<Imported> {
        let mut new = Vec::new();
        let mut skipped = 0;
        for row in rows {
            let id = format!("h:{}", row.0);
            let held: bool = conn.query_row(
                "SELECT EXISTS (SELECT 1 FROM chat_entries WHERE id = ?)",
                [&id],
                |r| r.get(0),
            )?;
            if held {
                skipped += 1;
            } else {
                new.push((id, row));
            }
        }
        if new.is_empty() {
            return Ok(Imported { added: 0, skipped });
        }
        let stream = match stream(conn, conversation)? {
            Some(stream) => stream,
            None => open_stream(conn, conversation, &new_epoch())?,
        };
        let count = new.len() as u64;
        let Some(first) = stream.low_pos.checked_sub(count) else {
            return refused("no position is left below the conversation's history");
        };
        for (i, (id, (_, kind, at, day, reference, body_json, updated_at))) in new.iter().enumerate() {
            let pos = to_i64(first + i as u64, "pos")?;
            let seq = to_i64(next_seq(conn, conversation)?, "seq")?;
            conn.execute(
                "INSERT INTO chat_entries (id, conversation_id, pos, seq, rev, kind, state, open, ref, created_at, \
                 updated_at, day, body_json, deleted, imported_from) VALUES (?, ?, ?, ?, 1, 'legacy', 'done', 0, ?, \
                 ?, ?, ?, '{\"kind\": ' || json_quote(?) || ', \"body\": ' || ? || '}', 0, ?)",
                params![
                    id,
                    conversation,
                    pos,
                    seq,
                    reference,
                    at,
                    updated_at,
                    day,
                    kind,
                    body_json,
                    FROM_ARCHIVE
                ],
            )?;
        }
        cover(conn, conversation, first)?;
        Ok(Imported {
            added: new.len(),
            skipped,
        })
    })
}
