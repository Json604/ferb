//! S1 recovery (recovery.py): before FERB overwrites or edits a file, a copy of it (an APFS clone, else a byte copy);
//! a durable record of where a trashed item came from and went; restore; retention; and local APFS snapshots before
//! bulk or destructive work.
//! - The original is never lost: a clone failure falls back to a byte copy, and when no copy can be made the caller
//!   must not change the file (`preserve` returns the error).
//! - Restore never overwrites what the owner may have changed since, never reaches outside the recovery root, and
//!   never takes an item out of the Trash that isn't the one FERB put there.
//!
//! The database is the daemon's `recovery` table (migrations 0023, 0024), reached through the caller's connection.

use std::os::fd::RawFd;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::Local;
use ferb_core::interfaces::{Clock, IdSource};
use ferb_core::types::{Dict, ToolResult};
use ferb_core::when::{self, DateTime};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::files::{self, Fail};
use crate::pypath as pp;
use crate::sys::{self, At, OsError, Res};

pub const TRASH_UNVERIFIED: &str = "trash-unverified";
pub const TRASH_COPY: &str = "trash copy";
pub const TRASH_RECORD_MAX_DAYS: i64 = 90;
pub const SNAPSHOT_TIMEOUT_S: f64 = 30.0;
pub const SNAPSHOT_DELETE_TIMEOUT_S: f64 = 120.0;
pub const STARTUP_DISK: &str = "/";
pub const SNAPSHOT_HOW: &str =
    "a recovery point of the whole disk. FERB can't open it (that needs admin rights); you \
                                can, from macOS Recovery or Time Machine";
pub const BEFORE_FERB: &str = "before FERB";
pub const RESTORED: &str = "restored";
pub const RETRY_AFTER_FAILURE_S: i64 = 60;
const INSERT: &str = "INSERT INTO recovery (id, task_id, kind, original, saved_to, reason, at, saved_dev, saved_ino) \
                      VALUES (?,?,?,?,?,?,?,?,?)";

/// Why a record can't be restored, in words the owner reads.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RestoreError {
    /// `NotRestorable`.
    NotRestorable(String),
    /// An OS error that left the restore (Python lets it propagate).
    Os(OsError),
    /// Another exception (a link loop, the database).
    Other(String),
}

impl From<OsError> for RestoreError {
    fn from(e: OsError) -> Self {
        RestoreError::Os(e)
    }
}

impl From<rusqlite::Error> for RestoreError {
    fn from(e: rusqlite::Error) -> Self {
        RestoreError::Other(e.to_string())
    }
}

impl From<Fail> for RestoreError {
    fn from(f: Fail) -> Self {
        match f {
            Fail::Os(e) => RestoreError::Os(e),
            Fail::Refused(m) | Fail::Raised(m) => RestoreError::Other(m),
            Fail::Decode(n) => RestoreError::Other(format!("decode error at {n}")),
        }
    }
}

fn nr(text: String) -> RestoreError {
    RestoreError::NotRestorable(text)
}

/// `short`: a path as the owner writes it, the home folder as ~ (P27).
pub fn short(path: &str, home: &str) -> String {
    let (p, h) = (pp::norm(path), pp::norm(home));
    if pp::is_relative_to(&p, &h) && p != h {
        format!("~/{}", pp::relative_to(&p, &h))
    } else {
        p
    }
}

// ---------- copies ----------

/// `_clone`: clonefile(2), never replacing dst.
fn clone(src: &str, dst: &str) -> Res<()> {
    if crate::hooks::off("clonefile") {
        return Err(OsError::with_text(
            libc::ENOTSUP,
            "clonefile is unavailable",
        ));
    }
    sys::clonefile(src, dst)
}

/// shutil.copystat on macOS: times, mode, then flags (ENOTSUP ignored); no xattrs (macOS has no os.listxattr).
fn copystat(src: &str, dst: &str) -> Res<()> {
    let st = sys::stat(src)?;
    let fd = sys::open(At::Cwd, dst, sys::O_RDONLY | sys::O_CLOEXEC, 0);
    if let Ok(fd) = &fd {
        sys::futimens(fd.raw(), st.atime_ns(), st.mtime_ns())?;
        sys::fchmod(fd.raw(), st.imode())?;
    } else {
        // a file FERB can't open for reading: by path (Python's os.utime / os.chmod)
        let c = std::ffi::CString::new(dst).map_err(|_| OsError::plain("embedded null byte"))?;
        // SAFETY: valid C string.
        if unsafe { libc::chmod(c.as_ptr(), st.imode() as libc::mode_t) } != 0 {
            return Err(OsError::last(Some(dst)));
        }
    }
    match sys::mac::lchflags(dst, st.flags) {
        Ok(()) => Ok(()),
        Err(e) if e.is(libc::ENOTSUP) || e.is(libc::EOPNOTSUPP) => Ok(()),
        Err(e) => Err(e),
    }
}

/// `_byte_copy`: a plain copy into a new file (never replacing one), with the original's mode and times. A partial
/// copy goes.
fn byte_copy(src: &str, dst: &str) -> Res<()> {
    let s = sys::open(At::Cwd, src, sys::O_RDONLY | sys::O_CLOEXEC, 0)?;
    let d = sys::open(
        At::Cwd,
        dst,
        sys::O_WRONLY | sys::O_CREAT | sys::O_EXCL | sys::O_CLOEXEC,
        0o666,
    )?;
    let work = (|| -> Res<()> {
        loop {
            let block = sys::read(s.raw(), 1 << 20)?;
            if block.is_empty() {
                break;
            }
            sys::write_all(d.raw(), &block)?;
        }
        Ok(())
    })();
    drop(d);
    let work = work.and_then(|()| copystat(src, dst));
    if let Err(e) = work {
        unlock(dst);
        let _ = sys::unlink_at(At::Cwd, dst);
        return Err(e);
    }
    Ok(())
}

/// `_unlock`: clear a locked / append-only flag a copy inherited (best effort).
pub fn unlock(path: &str) {
    if let Ok(st) = sys::lstat(path) {
        if st.flags & sys::LOCKED != 0 {
            let _: Result<(), OsError> = sys::lchflags(path, st.flags & !sys::LOCKED);
        }
    }
}

/// `_copy_new`: a clone when the volume allows it, else a byte copy; never replacing dst (EEXIST).
pub fn copy_new(src: &str, dst: &str) -> Res<()> {
    match clone(src, dst) {
        Ok(()) => return Ok(()),
        Err(e) if e.is(libc::EEXIST) => return Err(e),
        Err(_) => {}
    }
    byte_copy(src, dst)
}

fn make_copy(src: &str, dst: &str) -> Res<()> {
    copy_new(src, dst)?;
    unlock(dst);
    Ok(())
}

fn sha256_path(path: &str) -> Res<String> {
    let fd = sys::open(At::Cwd, path, sys::O_RDONLY | sys::O_CLOEXEC, 0)?;
    let mut h = Sha256::new();
    loop {
        let block = sys::read(fd.raw(), 1 << 20)?;
        if block.is_empty() {
            return Ok(hex::encode(h.finalize()));
        }
        h.update(&block);
    }
}

// ---------- copies through handles (upload staging) ----------

/// `_fclone`: fclonefileat(2) of the open file `src` as `name` in the open folder `dir` (CLONE_NOFOLLOW).
pub fn fclone(src: RawFd, dir: RawFd, name: &str) -> Res<()> {
    if crate::hooks::off("fclonefileat") {
        return Err(OsError::with_text(
            libc::ENOTSUP,
            "fclonefileat is unavailable",
        ));
    }
    sys::fclonefileat(src, dir, name, sys::CLONE_NOFOLLOW)
}

/// `_byte_copy_fd`: a plain copy of the open file into a new file `name` in the open folder (O_EXCL | O_NOFOLLOW),
/// with the original's mode and times. A partial copy goes, through the folder.
fn byte_copy_fd(src: RawFd, dir: RawFd, name: &str) -> Res<()> {
    let st = sys::fstat(src)?;
    let out = sys::open(
        At::Dir(dir),
        name,
        sys::O_WRONLY | sys::O_CREAT | sys::O_EXCL | sys::O_NOFOLLOW | sys::O_CLOEXEC,
        0o600,
    )?;
    let work = (|| -> Res<()> {
        let mut offset = 0i64;
        loop {
            let block = sys::pread(src, 1 << 20, offset)?;
            if block.is_empty() {
                break;
            }
            sys::write_all(out.raw(), &block)?;
            offset += block.len() as i64;
        }
        sys::fchmod(out.raw(), st.imode())?;
        sys::futimens(out.raw(), st.atime_ns(), st.mtime_ns())
    })();
    drop(out);
    if let Err(e) = work {
        let _ = sys::unlink_at(At::Dir(dir), name);
        return Err(e);
    }
    Ok(())
}

/// `sha256_fd`: the SHA-256 of an open file from its start (the handle's position is left alone).
pub fn sha256_fd(fd: RawFd) -> Res<String> {
    let mut h = Sha256::new();
    let mut offset = 0i64;
    loop {
        let block = sys::pread(fd, 1 << 20, offset)?;
        if block.is_empty() {
            return Ok(hex::encode(h.finalize()));
        }
        h.update(&block);
        offset += block.len() as i64;
    }
}

/// `copy_into` (upload staging): a new copy of the open file as `name` in the open folder (a clone, else a byte
/// copy) and its SHA-256, read back through the folder. Anything at `name` is EEXIST and nothing is written; an
/// inherited lock flag is cleared (raising when it can't be).
pub fn copy_into(src: RawFd, dir: RawFd, name: &str) -> Res<String> {
    match fclone(src, dir, name) {
        Ok(()) => {}
        Err(e) if e.is(libc::EEXIST) => {
            let mut e = OsError::new(libc::EEXIST, Some(name));
            e.strerror = Some(sys::strerror(libc::EEXIST));
            return Err(e);
        }
        Err(_) => byte_copy_fd(src, dir, name)?,
    }
    let fd = sys::open(
        At::Dir(dir),
        name,
        sys::O_RDONLY | sys::O_NOFOLLOW | sys::O_CLOEXEC,
        0,
    )?;
    let flags = sys::fstat(fd.raw())?.flags;
    if flags & sys::LOCKED != 0 {
        if !cfg!(target_os = "macos") {
            return Err(OsError::plain(
                "the copy's inherited lock flag can't be cleared: fchflags is unavailable",
            ));
        }
        if let Err(code) = sys::fchflags(fd.raw(), flags & !sys::LOCKED) {
            return Err(OsError::plain(&format!(
                "the copy's inherited lock flag can't be cleared: {}",
                sys::strerror(code)
            )));
        }
    }
    sha256_fd(fd.raw())
}

/// `_inside`: a clone's file, only when it resolves inside the recovery root (links followed).
fn inside(root: &str, rel: Option<&str>) -> Option<String> {
    let rel = rel.filter(|r| !r.is_empty())?;
    let path = pp::resolve(&pp::join(root, rel)).ok()?;
    let r = pp::resolve(root).ok()?;
    pp::is_relative_to(&path, &r).then_some(path)
}

/// One copy about to be made: its record id, source, place under the root and time.
#[derive(Clone, Debug)]
pub struct Copy {
    pub eid: String,
    pub src: String,
    pub rel: String,
    pub dst: String,
    pub at: DateTime,
}

/// `Path.mkdir(mode, parents=True, exist_ok=True)`: the parents with the default mode.
fn mkdirs(path: &str, mode: u32) -> Res<()> {
    match sys::mkdir_at(At::Cwd, path, mode) {
        Ok(()) => Ok(()),
        Err(e) if e.is(libc::ENOENT) && pp::parent(path) != path => {
            mkdirs(&pp::parent(path), 0o777)?;
            mkdirs(path, mode)
        }
        Err(e) if e.is(libc::EEXIST) && pp::is_dir(path) => Ok(()),
        Err(e) => Err(e),
    }
}

fn suffix_kept(path: &str) -> String {
    let s = pp::suffix(path);
    let ok = s.len() >= 2 && s.len() <= 17 && s[1..].chars().all(|c| c.is_ascii_alphanumeric());
    if ok {
        s
    } else {
        String::new()
    }
}

/// The local date of an aware time (`at.astimezone().date().isoformat()`).
fn local_day(at: &DateTime) -> String {
    at.with_timezone(&Local)
        .date_naive()
        .format("%Y-%m-%d")
        .to_string()
}

/// `_plan_copy`.
pub fn plan_copy(
    path: &str,
    root: &str,
    clock: &dyn Clock,
    ids: &dyn IdSource,
) -> Result<Option<Copy>, Fail> {
    if !files::pl_is_file(path)? {
        return Ok(None);
    }
    let src = pp::resolve(path)?;
    let eid = ids.new_id();
    let at = clock.now();
    let day = local_day(&at);
    mkdirs(root, 0o700)?;
    match sys::mkdir_at(At::Cwd, &pp::join(root, &day), 0o700) {
        Ok(()) => {}
        Err(e) if e.is(libc::EEXIST) && pp::is_dir(&pp::join(root, &day)) => {}
        Err(e) => return Err(e.into()),
    }
    let rel = format!("{day}/{eid}{}", suffix_kept(&src));
    let dst = pp::join(root, &rel);
    if inside(root, Some(&rel)) != pp::resolve(&dst).ok() {
        return Err(Fail::Raised(format!(
            "recovery id {} is not a plain file name",
            crate::py_repr(&eid)
        )));
    }
    Ok(Some(Copy {
        eid,
        src,
        rel,
        dst,
        at,
    }))
}

fn record_copy(
    c: &Copy,
    reason: &str,
    task_id: Option<&str>,
    db: &Connection,
) -> Result<String, Fail> {
    let done = db.execute(
        INSERT,
        params![
            c.eid,
            task_id,
            "clone",
            c.src,
            c.rel,
            reason,
            when::isoformat(&c.at),
            None::<i64>,
            None::<i64>
        ],
    );
    if let Err(e) = done {
        unlock(&c.dst);
        let _ = sys::unlink_at(At::Cwd, &c.dst); // never a copy without its record
        return Err(Fail::Raised(e.to_string()));
    }
    Ok(c.eid.clone())
}

/// `preserve`: copy the file at `path` (the real path the tool writes) to root/<day>/<id><suffix> and record it; the
/// record id, or None when there is nothing to keep. An error means no copy could be made: the caller must then leave
/// the file alone. (Python's `preserve_async` is this run off the loop: the daemon calls it in a blocking task.)
pub fn preserve(
    path: &str,
    reason: &str,
    task_id: Option<&str>,
    db: &Connection,
    clock: &dyn Clock,
    ids: &dyn IdSource,
    root: &str,
) -> Result<Option<String>, Fail> {
    let Some(c) = plan_copy(path, root, clock, ids)? else {
        return Ok(None);
    };
    make_copy(&c.src, &c.dst)?;
    record_copy(&c, reason, task_id, db).map(Some)
}

/// `mark_written_async`: the SHA-256 of what FERB left, on the record.
pub fn mark_written(record_id: &str, path: &str, db: &Connection) -> Result<(), Fail> {
    let digest = sha256_path(&pp::resolve(path)?)?;
    db.execute(
        "UPDATE recovery SET written_sha256 = ? WHERE id = ?",
        params![digest, record_id],
    )
    .map_err(|e| Fail::Raised(e.to_string()))?;
    Ok(())
}

fn same_then_drop(path: &str, copy: &str) -> bool {
    let Ok(real) = pp::resolve(path) else {
        return false;
    };
    match (sha256_path(&real), sha256_path(copy)) {
        (Ok(a), Ok(b)) if a == b => {}
        _ => return false,
    }
    unlock(copy);
    let _ = sys::unlink_at(At::Cwd, copy);
    true
}

/// `discard_if_unchanged`: the call reported an error; its copy and record go only when the file is still
/// byte-identical to the copy.
pub fn discard_if_unchanged(
    record_id: &str,
    path: &str,
    db: &Connection,
    root: &str,
) -> Result<bool, Fail> {
    let saved: Option<Option<String>> = db
        .query_row(
            "SELECT saved_to FROM recovery WHERE id = ? AND kind = 'clone'",
            [record_id],
            |r| r.get(0),
        )
        .optional()
        .map_err(|e| Fail::Raised(e.to_string()))?;
    let copy = saved.and_then(|s| inside(root, s.as_deref()));
    let Some(copy) = copy else { return Ok(false) };
    if !same_then_drop(path, &copy) {
        return Ok(false);
    }
    db.execute("DELETE FROM recovery WHERE id = ?", [record_id])
        .map_err(|e| Fail::Raised(e.to_string()))?;
    Ok(true)
}

/// `_trash_place`: where an item sits in the Trash: absolute, no `..`, its folder resolved, inside a real .Trash /
/// .Trashes folder; else None.
pub fn trash_place(saved_to: Option<&str>) -> Option<String> {
    let s = saved_to.filter(|s| !s.is_empty())?;
    let p = pp::norm(s);
    if !pp::is_abs(&p) || pp::tail(&p).contains(&"..") {
        return None;
    }
    let real = pp::join(&pp::resolve(&pp::parent(&p)).ok()?, &pp::name(&p));
    pp::parents(&real)
        .iter()
        .any(|a| files::TRASH_DIRS.contains(&pp::name(a).as_str()) && pp::is_dir(a))
        .then_some(real)
}

/// `record_trash`: a durable record of a trashed item, with the device and inode fs.trash verified.
#[allow(clippy::too_many_arguments)]
pub fn record_trash(
    original: &str,
    trashed_to: Option<&str>,
    device: Option<i64>,
    inode: Option<u64>,
    reason: &str,
    task_id: Option<&str>,
    db: &Connection,
    clock: &dyn Clock,
    ids: &dyn IdSource,
) -> rusqlite::Result<String> {
    let place = trash_place(trashed_to);
    let eid = ids.new_id();
    let verified = reason != TRASH_UNVERIFIED;
    db.execute(
        INSERT,
        params![
            eid,
            task_id,
            "trash",
            original,
            place.as_deref().or(trashed_to),
            reason,
            when::isoformat(&clock.now()),
            device.filter(|_| verified),
            inode.filter(|_| verified).map(|i| i as i64)
        ],
    )?;
    Ok(eid)
}

/// A `recovery` row.
#[derive(Clone, Debug, PartialEq)]
pub struct Rec {
    pub id: String,
    pub task_id: Option<String>,
    pub kind: String,
    pub original: String,
    pub saved_to: Option<String>,
    pub reason: Option<String>,
    pub at: String,
    pub written_sha256: Option<String>,
    pub saved_dev: Option<i64>,
    pub saved_ino: Option<i64>,
}

const COLS: &str =
    "id, task_id, kind, original, saved_to, reason, at, written_sha256, saved_dev, saved_ino";

fn rec(r: &rusqlite::Row) -> rusqlite::Result<Rec> {
    Ok(Rec {
        id: r.get(0)?,
        task_id: r.get(1)?,
        kind: r.get(2)?,
        original: r.get(3)?,
        saved_to: r.get(4)?,
        reason: r.get(5)?,
        at: r.get(6)?,
        written_sha256: r.get(7)?,
        saved_dev: r.get(8)?,
        saved_ino: r.get(9)?,
    })
}

fn same_trash_item(r: &Rec) -> bool {
    let Some(place) = trash_place(r.saved_to.as_deref()) else {
        return false;
    };
    let Some(dev) = r.saved_dev else { return false };
    match sys::lstat(&place) {
        Ok(st) => (st.dev, st.ino as i64) == (dev, r.saved_ino.unwrap_or(i64::MIN)),
        Err(_) => false,
    }
}

// ---------- reading and restoring ----------

/// `recent`: the records, newest first (at most `limit`), with whether each can still be put back; and how many.
pub fn recent(db: &Connection, root: &str, limit: i64) -> rusqlite::Result<(Vec<Dict>, i64)> {
    let total: i64 = db.query_row("SELECT count(*) FROM recovery", [], |r| r.get(0))?;
    let mut stmt = db.prepare(&format!(
        "SELECT {COLS} FROM recovery ORDER BY at DESC, rowid DESC LIMIT ?"
    ))?;
    let rows: Vec<Rec> = stmt.query_map([limit], rec)?.collect::<Result<_, _>>()?;
    let mut out = Vec::new();
    for r in rows {
        let available = match r.kind.as_str() {
            "clone" => json!(inside(root, r.saved_to.as_deref()).is_some_and(|c| pp::is_file(&c))),
            "trash" => json!(same_trash_item(&r)),
            _ => Value::Null,
        };
        let mut d = Dict::new();
        d.insert("id".into(), json!(r.id));
        d.insert("task_id".into(), json!(r.task_id));
        d.insert("kind".into(), json!(r.kind));
        d.insert("original".into(), json!(r.original));
        d.insert(
            "saved_to".into(),
            if r.kind == "snapshot" {
                json!(r.saved_to)
            } else {
                Value::Null
            },
        );
        d.insert("reason".into(), json!(r.reason));
        d.insert("at".into(), json!(r.at));
        d.insert("available".into(), available);
        d.insert(
            "needs_owner".into(),
            json!(r.reason.as_deref() == Some(TRASH_UNVERIFIED)),
        );
        out.push(d);
    }
    Ok((out, total))
}

/// `find`: a record by its id or the start of it.
pub fn find(db: &Connection, reference: &str) -> Result<Rec, RestoreError> {
    let reference = ferb_py::value::strip(reference);
    if reference.is_empty() {
        return Err(nr(
            "say which record: Settings → Data → Restore lists them".into()
        ));
    }
    let exact = db
        .query_row(
            &format!("SELECT {COLS} FROM recovery WHERE id = ?"),
            [reference],
            rec,
        )
        .optional()?;
    if let Some(r) = exact {
        return Ok(r);
    }
    let mut stmt = db.prepare(&format!(
        "SELECT {COLS} FROM recovery WHERE substr(id, 1, ?) = ? LIMIT 2"
    ))?;
    let rows: Vec<Rec> = stmt
        .query_map(params![reference.chars().count() as i64, reference], rec)?
        .collect::<Result<_, _>>()?;
    match rows.len() {
        0 => Err(nr(format!(
            "no recovery record {reference}; Settings → Data → Restore lists the ones FERB still keeps"
        ))),
        1 => Ok(rows.into_iter().next().expect("one")),
        _ => Err(nr(format!(
            "more than one record starts with {reference}; give more of its id"
        ))),
    }
}

/// The protection lists' answer for a path and an access (gateway).
pub type Closed<'a> = Option<&'a dyn Fn(&str, &str) -> Option<String>>;

/// `restore`: put a record back and return where it went (never over what the owner may have changed since; a
/// trashed item only when it is the very item FERB trashed; a snapshot says how the owner can use it). Hold the
/// file's lock around it; on the daemon's loop run it in a blocking task (`restore_async`).
pub fn restore(
    record_id: &str,
    db: &Connection,
    root: &str,
    closed: Closed,
    home: &str,
) -> Result<String, RestoreError> {
    let r = find(db, record_id)?;
    let back = put_back(&r, root, closed, home)?;
    if r.kind == "trash" {
        db.execute("DELETE FROM recovery WHERE id = ?", [&r.id])?;
    }
    Ok(back)
}

fn put_back(r: &Rec, root: &str, closed: Closed, home: &str) -> Result<String, RestoreError> {
    match r.kind.as_str() {
        "snapshot" => Err(nr(format!(
            "{} is {SNAPSHOT_HOW}",
            r.saved_to.as_deref().unwrap_or("None")
        ))),
        "clone" => {
            let dest = pp::resolve(&r.original).map_err(|e| RestoreError::Other(e.0))?;
            restore_copy(r, &r.original, &dest, root, closed, home)
        }
        "trash" => restore_trashed(r, &r.original, closed, home),
        other => Err(nr(format!("unknown record kind {}", crate::py_repr(other)))),
    }
}

fn uuid8() -> String {
    let mut b = [0u8; 4];
    getrandom::fill(&mut b).expect("the system's random source");
    hex::encode(b)
}

fn restore_copy(
    r: &Rec,
    original: &str,
    dest: &str,
    root: &str,
    closed: Closed,
    home: &str,
) -> Result<String, RestoreError> {
    let copy = inside(root, r.saved_to.as_deref()).filter(|c| pp::is_file(c));
    let Some(copy) = copy else {
        return Err(nr(format!(
            "the copy FERB kept of {} is gone",
            short(original, home)
        )));
    };
    let write = closed.map(|c| move |q: &str| c(q, "write"));
    let check = |target: &str| -> Result<(), RestoreError> {
        let w = write.as_ref().map(|w| w as &dyn Fn(&str) -> Option<String>);
        if let Some(refusal) = files::destination_closed(target, w)?.filter(|r| !r.is_empty()) {
            return Err(nr(format!(
                "{refusal}; the copy stays in FERB's recovery folder"
            )));
        }
        Ok(())
    };
    check(dest)?;
    if !pp::lexists(dest) {
        mkdirs(&pp::parent(dest), 0o777)?;
        copy_new(&copy, dest)?;
        return Ok(dest.to_string());
    }
    if pp::is_file(dest) {
        let now = sha256_path(dest)?;
        if now == sha256_path(&copy)? {
            return Ok(dest.to_string());
        }
        if r.written_sha256
            .as_deref()
            .is_some_and(|w| !w.is_empty() && w == now)
        {
            let tmp = pp::join(
                &pp::parent(dest),
                &format!(".{}.{}.ferb-restore", pp::name(dest), uuid8()),
            );
            copy_new(&copy, &tmp)?;
            let moved = sys::rename_at(At::Cwd, &tmp, At::Cwd, dest);
            let _ = sys::unlink_at(At::Cwd, &tmp);
            moved?;
            return Ok(dest.to_string());
        }
    }
    beside(dest, BEFORE_FERB, home, |target| {
        check(target)?;
        copy_new(&copy, target).map_err(RestoreError::Os)
    })
}

fn restore_trashed(
    r: &Rec,
    original: &str,
    closed: Closed,
    home: &str,
) -> Result<String, RestoreError> {
    let so = |p: &str| short(p, home);
    let name = pp::name(original);
    if r.reason.as_deref() == Some(TRASH_UNVERIFIED) {
        return Err(nr(format!(
            "what FERB moved to the Trash from {} changed while FERB moved it, so FERB won't put it back itself; it \
             is in the Trash at {}: look at it there, and move it yourself if it's yours",
            so(original),
            so(r.saved_to.as_deref().filter(|s| !s.is_empty()).unwrap_or("?"))
        )));
    }
    let place = trash_place(r.saved_to.as_deref());
    let (Some(place), Some(dev)) = (place, r.saved_dev) else {
        return Err(nr(format!(
            "FERB doesn't know where {} went in the Trash",
            so(original)
        )));
    };
    let saved = (dev, r.saved_ino.unwrap_or(i64::MIN));
    let trash = sys::open(
        At::Cwd,
        &pp::parent(&place),
        sys::O_SEARCH | sys::O_DIRECTORY | sys::O_NOFOLLOW | sys::O_CLOEXEC,
        0,
    )
    .map_err(|_| {
        nr(format!(
            "{} is no longer in the Trash, so it can't be put back",
            so(original)
        ))
    })?;
    let looked = files::opened_path(trash.raw())
        .map(|w| files::in_trash(&w))
        .and_then(|t| {
            sys::stat_at(At::Dir(trash.raw()), &pp::name(&place), false).map(|st| (t, st))
        });
    let (in_a_trash, st) = match looked {
        Ok(x) => x,
        Err(e) if e.is(libc::ENOENT) => {
            return Err(nr(format!(
                "{} is no longer in the Trash, so it can't be put back",
                so(original)
            )))
        }
        Err(e) => {
            return Err(nr(format!(
                "FERB couldn't look in the Trash for {} ({})",
                so(original),
                e.strerror_or_str()
            )))
        }
    };
    if !in_a_trash {
        return Err(nr(format!(
            "{} isn't a Trash folder, so FERB left it alone",
            so(&pp::parent(&place))
        )));
    }
    if (st.dev, st.ino as i64) != saved {
        return Err(nr(format!(
            "what is in the Trash under that name now isn't the {} FERB moved there, so FERB left it alone",
            so(original)
        )));
    }
    let folder = pp::parent(original);
    let write = closed.map(|c| move |p: &str| c(p, "write"));
    let write_ref = write.as_ref().map(|w| w as &dyn Fn(&str) -> Option<String>);
    match files::make_folders(&folder, write_ref, true) {
        Ok(()) => {}
        Err(Fail::Refused(e)) => {
            return Err(nr(format!(
                "{e}, so FERB won't put {name} back through it; it stays in the Trash"
            )))
        }
        Err(Fail::Os(e)) => {
            return Err(nr(format!(
                "FERB couldn't make {} to put {name} back ({e})",
                so(&folder)
            )))
        }
        Err(other) => return Err(other.into()),
    }
    let out = sys::open(
        At::Cwd,
        &folder,
        sys::O_SEARCH | sys::O_DIRECTORY | sys::O_CLOEXEC,
        0,
    )
    .map_err(|e| {
        nr(format!(
            "FERB couldn't open {} to put {name} back ({e})",
            so(&folder)
        ))
    })?;
    let here = files::opened_path(out.raw()).map_err(|e| {
        nr(format!(
            "FERB couldn't tell which folder it opened for {name} ({})",
            e.strerror_or_str()
        ))
    })?;
    if crate::casefold(&here) != crate::casefold(&folder) {
        return Err(nr(format!(
            "{} isn't where it was (it now leads to {}), so FERB won't put {name} back through it; it stays in the \
             Trash",
            so(&folder),
            so(&here)
        )));
    }
    let landed = move_back(
        trash.raw(),
        &pp::name(&place),
        out.raw(),
        &here,
        &name,
        closed,
        home,
    )?;
    let after = sys::stat_at(At::Dir(out.raw()), &landed, false).ok();
    if after.is_none_or(|a| (a.dev, a.ino as i64) != saved) {
        return Err(nr(format!(
            "FERB put {} back at {}, but what is there now isn't it (it changed right after); look at it there",
            so(original),
            so(&pp::join(&here, &landed))
        )));
    }
    Ok(pp::join(&here, &landed))
}

/// `_move_back`: from the open Trash into the open folder under `wanted`, or beside it as "<stem> (restored[ n])
/// <suffix>" when something stands there now; each name checked against the lists first.
fn move_back(
    trash: RawFd,
    name: &str,
    out: RawFd,
    here: &str,
    wanted: &str,
    closed: Closed,
    home: &str,
) -> Result<String, RestoreError> {
    let (stem, suffix) = pp::splitext(wanted);
    for n in 0..1000 {
        let target = if n == 0 {
            wanted.to_string()
        } else {
            let k = if n == 1 {
                String::new()
            } else {
                format!(" {n}")
            };
            format!("{stem} ({RESTORED}{k}){suffix}")
        };
        if let Some(c) = closed {
            if let Some(refusal) = c(&pp::join(here, &target), "write").filter(|r| !r.is_empty()) {
                return Err(nr(refusal));
            }
        }
        match files::move_excl(trash, name, out, &target) {
            Ok(()) => return Ok(target),
            Err(e) if e.is(libc::EEXIST) => continue,
            Err(e) => {
                return Err(nr(format!(
                    "FERB couldn't move {wanted} out of the Trash ({})",
                    e.strerror_or_str()
                )))
            }
        }
    }
    Err(nr(format!(
        "no free name next to {}",
        short(&pp::join(here, wanted), home)
    )))
}

/// `_beside`: the first free "<stem> (<label>[ n])<suffix>" next to dest; `put` creates it exclusively.
fn beside(
    dest: &str,
    label: &str,
    home: &str,
    mut put: impl FnMut(&str) -> Result<(), RestoreError>,
) -> Result<String, RestoreError> {
    for n in 1..1000 {
        let k = if n == 1 {
            String::new()
        } else {
            format!(" {n}")
        };
        let target = pp::with_name(
            dest,
            &format!("{} ({label}{k}){}", pp::stem(dest), pp::suffix(dest)),
        );
        if pp::lexists(&target) {
            continue;
        }
        match put(&target) {
            Ok(()) => return Ok(target),
            Err(RestoreError::Os(e)) if e.is(libc::EEXIST) => continue,
            Err(e) => return Err(e),
        }
    }
    Err(nr(format!("no free name next to {}", short(dest, home))))
}

// ---------- retention ----------

/// `purge_before`: records older than the cutoff go, a clone's file first (one that can't be deleted keeps its
/// record); a trash record stays while its item is in the Trash, up to the backstop; snapshots are left to
/// `purge_snapshots`; unrecorded copies in old day folders and empty day folders go.
pub fn purge_before(
    cutoff_iso: &str,
    db: &Connection,
    root: &str,
    trash_backstop_iso: Option<&str>,
) -> rusqlite::Result<usize> {
    let mut stmt = db.prepare(&format!(
        "SELECT {COLS} FROM recovery WHERE at < ? AND kind != 'snapshot'"
    ))?;
    let rows: Vec<Rec> = stmt
        .query_map([cutoff_iso], rec)?
        .collect::<Result<_, _>>()?;
    let mut gone: Vec<String> = Vec::new();
    for r in rows {
        if r.kind == "trash"
            && same_trash_item(&r)
            && trash_backstop_iso.is_none_or(|b| r.at.as_str() >= b)
        {
            continue;
        }
        let copy = if r.kind == "clone" {
            inside(root, r.saved_to.as_deref())
        } else {
            None
        };
        if let Some(copy) = copy {
            unlock(&copy);
            match sys::unlink_at(At::Cwd, &copy) {
                Ok(()) => {}
                Err(e) if e.is(libc::ENOENT) => {}
                Err(_) => continue,
            }
        }
        gone.push(r.id);
    }
    for chunk in gone.chunks(500) {
        let marks = vec!["?"; chunk.len()].join(",");
        db.execute(
            &format!("DELETE FROM recovery WHERE id IN ({marks})"),
            rusqlite::params_from_iter(chunk.iter()),
        )?;
    }
    sweep(cutoff_iso, db, root);
    Ok(gone.len())
}

fn is_day(name: &str) -> bool {
    let b = name.as_bytes();
    b.len() == 10
        && b.iter().enumerate().all(|(i, c)| {
            if i == 4 || i == 7 {
                *c == b'-'
            } else {
                c.is_ascii_digit()
            }
        })
}

/// `_sweep`: tidying that never fails the pass.
fn sweep(cutoff_iso: &str, db: &Connection, root: &str) {
    let Some(cutoff) = when::parse(cutoff_iso) else {
        return;
    };
    let before = local_day(&cutoff);
    let kept: std::collections::HashSet<String> = match db
        .prepare("SELECT saved_to FROM recovery WHERE kind = 'clone'")
        .and_then(|mut s| {
            s.query_map([], |r| r.get::<_, Option<String>>(0))?
                .collect::<Result<Vec<_>, _>>()
        }) {
        Ok(v) => v.into_iter().flatten().collect(),
        Err(_) => return,
    };
    let Ok(days) = std::fs::read_dir(root) else {
        return;
    };
    for day in days.flatten() {
        let name = day.file_name().to_string_lossy().into_owned();
        let path = pp::join(root, &name);
        if !pp::is_dir(&path) || pp::is_symlink(&path) || !is_day(&name) {
            continue;
        }
        let Ok(items) = std::fs::read_dir(&path) else {
            return;
        };
        if name < before {
            for f in items.flatten() {
                let fname = f.file_name().to_string_lossy().into_owned();
                let fpath = pp::join(&path, &fname);
                if !kept.contains(&format!("{name}/{fname}"))
                    && (pp::is_file(&fpath) || pp::is_symlink(&fpath))
                {
                    unlock(&fpath);
                    match sys::unlink_at(At::Cwd, &fpath) {
                        Ok(()) => {}
                        Err(e) if e.is(libc::ENOENT) => {}
                        Err(_) => return,
                    }
                }
            }
        }
        match std::fs::read_dir(&path) {
            Ok(mut it) => {
                if it.next().is_none() && sys::rmdir(&path).is_err() {
                    return;
                }
            }
            Err(_) => return,
        }
    }
}

/// `_SNAPSHOT_NAME`: com.apple.TimeMachine.YYYY-MM-DD-HHMMSS.local, whole; its date digits.
pub fn snapshot_name(name: &str) -> Option<&str> {
    let date = name
        .strip_prefix("com.apple.TimeMachine.")?
        .strip_suffix(".local")?;
    strict_date(date).then_some(date)
}

/// `\d{4}-\d{2}-\d{2}-\d{6}` (`\d` is any Unicode decimal digit, as Python's `re` reads it).
fn strict_date(d: &str) -> bool {
    let chars: Vec<char> = d.chars().collect();
    let shape = [4usize, 2, 2, 6];
    let mut i = 0;
    for (n, len) in shape.iter().enumerate() {
        for _ in 0..*len {
            match chars.get(i) {
                Some(c) if ferb_py::unicode::is_decimal(*c) => i += 1,
                _ => return false,
            }
        }
        if n < 3 {
            if chars.get(i) != Some(&'-') {
                return false;
            }
            i += 1;
        }
    }
    i == chars.len()
}

/// The deleting half of `purge_snapshots` (`delete_local_snapshot`, or a test's): true when the snapshot is gone.
pub type Delete = Arc<
    dyn Fn(String) -> ferb_core::interfaces::BoxFuture<'static, Result<bool, String>> + Send + Sync,
>;

/// `purge_snapshots`: FERB's own snapshots older than the cutoff deleted (only a strict name ever is), then their
/// records; one that couldn't be deleted keeps its record for the next pass. `on_gone(name, deleted)` runs right after
/// each record goes.
pub async fn purge_snapshots(
    cutoff_iso: &str,
    db: &Mutex<Connection>,
    delete: Option<Delete>,
    mut on_gone: impl FnMut(&str, bool),
) -> rusqlite::Result<usize> {
    let rows: Vec<(String, Option<String>)> = {
        let conn = db.lock().expect("the store's lock");
        let mut stmt =
            conn.prepare("SELECT id, saved_to FROM recovery WHERE kind = 'snapshot' AND at < ?")?;
        let rows = stmt
            .query_map([cutoff_iso], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<Result<_, _>>()?;
        rows
    };
    let mut done = 0;
    for (id, saved) in rows {
        let name = saved.unwrap_or_default();
        let mut deleted = false;
        if let (Some(delete), Some(_)) = (&delete, snapshot_name(&name)) {
            match delete(name.clone()).await {
                Ok(true) => deleted = true,
                Ok(false) | Err(_) => continue,
            }
        }
        db.lock()
            .expect("the store's lock")
            .execute("DELETE FROM recovery WHERE id = ?", [&id])?;
        done += 1;
        on_gone(&name, deleted);
    }
    Ok(done)
}

// ---------- snapshots before bulk or destructive work ----------

/// `str.splitlines()`: lines split at every boundary Python counts, the boundaries dropped.
fn splitlines(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let (mut start, mut chars) = (0, text.char_indices().peekable());
    while let Some((i, c)) = chars.next() {
        if matches!(
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
        ) {
            out.push(&text[start..i]);
            let mut end = i + c.len_utf8();
            if c == '\r' && chars.peek().is_some_and(|(_, n)| *n == '\n') {
                chars.next();
                end += 1;
            }
            start = end;
        }
    }
    if start < text.len() {
        out.push(&text[start..]);
    }
    out
}

/// `local_snapshot`'s reading of tmutil's answer: the strict name from its date, else its own words.
pub fn snapshot_from_output(returncode: i32, stdout: &str, stderr: &str) -> Option<String> {
    if returncode != 0 {
        return None;
    }
    let text = format!("{stdout}\n{stderr}");
    const SAID: &str = "Created local snapshot with date: ";
    let mut loose = None;
    let mut at = 0;
    while let Some(i) = text[at..].find(SAID) {
        let start = at + i + SAID.len();
        let rest = &text[start..];
        let word_end = rest
            .char_indices()
            .find(|(_, c)| ferb_py::value::is_py_space(*c))
            .map(|(i, _)| i)
            .unwrap_or(rest.len());
        let word = &rest[..word_end];
        if loose.is_none() && !word.is_empty() {
            loose = Some(word.to_string());
        }
        // the strict pattern: the date at this place (the longest digit run that fits the shape)
        let cand: String = rest.chars().take(17).collect();
        if cand.chars().count() == 17 && strict_date(&cand) {
            return Some(format!("com.apple.TimeMachine.{cand}.local"));
        }
        at = start;
    }
    if let Some(l) = loose {
        return Some(l);
    }
    let lines: Vec<&str> = splitlines(&text)
        .into_iter()
        .map(ferb_py::value::strip)
        .filter(|l| !l.is_empty())
        .collect();
    Some(
        lines
            .last()
            .map(|l| l.to_string())
            .unwrap_or_else(|| "a snapshot tmutil reported without a name".into()),
    )
}

#[cfg(target_os = "macos")]
pub mod tmutil {
    //! `/usr/bin/tmutil`, the one macOS tool S1 runs: never on a terminal (stdin closed), in its own session.
    use super::*;

    /// `local_snapshot`: a local APFS snapshot of the startup disk; its name, or None when none was taken.
    pub fn local_snapshot(timeout_s: f64) -> Option<String> {
        use std::io::Read;
        use std::os::unix::process::CommandExt;
        use std::process::{Command, Stdio};
        let mut child = Command::new("/usr/bin/tmutil")
            .arg("localsnapshot")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0)
            .spawn()
            .ok()?;
        let deadline = std::time::Instant::now() + Duration::from_secs_f64(timeout_s);
        loop {
            match child.try_wait() {
                Ok(Some(status)) => {
                    let (mut out, mut err) = (String::new(), String::new());
                    child.stdout.take()?.read_to_string(&mut out).ok()?;
                    child.stderr.take()?.read_to_string(&mut err).ok()?;
                    return snapshot_from_output(status.code().unwrap_or(-1), &out, &err);
                }
                Ok(None) if std::time::Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(20));
                }
                _ => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return None;
                }
            }
        }
    }

    async fn run(args: &[&str], timeout_s: f64) -> Option<(i32, String)> {
        let mut cmd = tokio::process::Command::new("/usr/bin/tmutil");
        cmd.args(args)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .process_group(0);
        let child = cmd.spawn().ok()?;
        let out =
            tokio::time::timeout(Duration::from_secs_f64(timeout_s), child.wait_with_output())
                .await
                .ok()?
                .ok()?;
        Some((
            out.status.code().unwrap_or(0),
            String::from_utf8_lossy(&out.stdout).into_owned(),
        ))
    }

    /// `delete_local_snapshot`: one of FERB's own snapshots (strict name only); true when it is gone.
    pub async fn delete_local_snapshot(name: &str, timeout_s: f64) -> bool {
        let Some(date) = snapshot_name(name) else {
            return false;
        };
        if let Some((0, listed)) =
            run(&["listlocalsnapshots", STARTUP_DISK], SNAPSHOT_TIMEOUT_S).await
        {
            if !listed.split_whitespace().any(|w| w == name) {
                return true;
            }
        }
        matches!(
            run(&["deletelocalsnapshots", date], timeout_s).await,
            Some((0, _))
        )
    }
}

/// What takes a snapshot (`local_snapshot`, run off the loop): its name, None when none was taken, or the class name
/// of the exception it raised (`type(e).__name__`). A `Snapshots` without one has snapshots off (tests).
pub type Take = Arc<dyn Fn() -> Result<Option<String>, String> + Send + Sync>;
/// The redactor applied to the agent's words and an unexpected snapshot name.
pub type Redact = Arc<dyn Fn(&Value) -> Value + Send + Sync>;

struct SnapState {
    running: Option<tokio::sync::watch::Receiver<Option<Dict>>>,
    failed: Option<(DateTime, String)>,
    last: Option<Dict>,
    changed: bool,
}

/// `Snapshots` (HAND-206, HAND-216, HAND-408): a fresh local APFS snapshot before each shell command and permanent
/// delete (`changes`), and when an agent asks; the last one reused only while no such call ran since. Concurrent
/// requests share one attempt; a failure is a result (never a refusal), tried again after RETRY_AFTER_FAILURE_S.
pub struct Snapshots {
    pub take: Option<Take>,
    db: Arc<Mutex<Connection>>,
    clock: Arc<dyn Clock>,
    ids: Arc<dyn IdSource>,
    redact: Redact,
    state: Arc<Mutex<SnapState>>,
}

fn merged(a: &Dict, b: Dict) -> Dict {
    let mut out = a.clone();
    for (k, v) in b {
        out.insert(k, v);
    }
    out
}

fn one(key: &str, value: Value) -> Dict {
    let mut d = Dict::new();
    d.insert(key.into(), value);
    d
}

/// Python's `str(value)[:200]` on the redactor's answer.
fn cut200(v: &Value) -> String {
    let s = match v {
        Value::String(s) => s.clone(),
        other => ferb_py::value::py_str(other),
    };
    s.chars().take(200).collect()
}

impl Snapshots {
    pub fn new(
        take: Option<Take>,
        db: Arc<Mutex<Connection>>,
        clock: Arc<dyn Clock>,
        ids: Arc<dyn IdSource>,
        redact: Redact,
    ) -> Snapshots {
        Snapshots {
            take,
            db,
            clock,
            ids,
            redact,
            state: Arc::new(Mutex::new(SnapState {
                running: None,
                failed: None,
                last: None,
                changed: true,
            })),
        }
    }

    /// `request`: a snapshot as of now: the last one when nothing ran since, else a new one. `changes`: the caller is
    /// a shell command or a permanent delete about to run, so the next request takes a fresh one (also when this
    /// request is cancelled).
    pub async fn request(&self, task_id: Option<&str>, reason: &str, changes: bool) -> Dict {
        struct Mark(Arc<Mutex<SnapState>>, bool);
        impl Drop for Mark {
            fn drop(&mut self) {
                if self.1 {
                    self.0.lock().expect("snapshot state").changed = true;
                }
            }
        }
        let _mark = Mark(self.state.clone(), changes);
        self.inner(task_id, reason).await
    }

    async fn inner(&self, task_id: Option<&str>, reason: &str) -> Dict {
        let (mut rx, mine) = {
            let mut st = self.state.lock().expect("snapshot state");
            let busy = st.running.as_ref().is_some_and(|r| r.borrow().is_none());
            if busy {
                (st.running.clone().expect("running"), false)
            } else if !st.changed && st.last.is_some() {
                return merged(st.last.as_ref().expect("last"), one("reused", json!(true)));
            } else if self.take.is_none() {
                return one("error", json!("snapshots are turned off for this FERB"));
            } else if let Some((at, why)) = st.failed.clone().filter(|(at, _)| {
                (self.clock.now() - *at)
                    .num_microseconds()
                    .unwrap_or(i64::MAX)
                    < RETRY_AFTER_FAILURE_S * 1_000_000
            }) {
                let _ = at;
                return one(
                    "error",
                    json!(format!(
                        "no snapshot was taken (the last attempt failed: {why}; FERB tries again within \
                         {RETRY_AFTER_FAILURE_S} s)"
                    )),
                );
            } else {
                let (tx, rx) = tokio::sync::watch::channel(None);
                st.running = Some(rx.clone());
                st.changed = false; // a change reported from now on needs the next one
                drop(st);
                self.spawn_run(task_id.map(str::to_string), reason.to_string(), tx);
                (rx, true)
            }
        };
        let result = loop {
            if let Some(r) = rx.borrow().clone() {
                break r;
            }
            if rx.changed().await.is_err() {
                break one("error", json!("no snapshot was taken (it was cancelled)"));
            }
        };
        if mine || result.contains_key("error") {
            result
        } else {
            merged(&result, one("reused", json!(true)))
        }
    }

    fn spawn_run(
        &self,
        task_id: Option<String>,
        reason: String,
        tx: tokio::sync::watch::Sender<Option<Dict>>,
    ) {
        let take = self.take.clone().expect("on");
        let (db, clock, ids, redact, state) = (
            self.db.clone(),
            self.clock.clone(),
            self.ids.clone(),
            self.redact.clone(),
            self.state.clone(),
        );
        tokio::spawn(async move {
            let fail = |why: String| {
                let mut st = state.lock().expect("snapshot state");
                st.failed = Some((clock.now(), why.clone()));
                st.changed = true;
                one("error", json!(format!("no snapshot was taken ({why})")))
            };
            let taken = tokio::time::timeout(
                Duration::from_secs_f64(SNAPSHOT_TIMEOUT_S + 5.0),
                tokio::task::spawn_blocking(move || take()),
            )
            .await;
            let out = match taken {
                Err(_) => fail("TimeoutError".into()),
                Ok(Err(_)) => fail("RuntimeError".into()),
                Ok(Ok(Err(class))) => fail(class),
                Ok(Ok(Ok(None))) => fail("tmutil refused or is unavailable".into()),
                Ok(Ok(Ok(Some(name)))) if name.is_empty() => {
                    fail("tmutil refused or is unavailable".into())
                }
                Ok(Ok(Ok(Some(name)))) => {
                    state.lock().expect("snapshot state").failed = None;
                    let eid = ids.new_id();
                    let strict = snapshot_name(&name).map(str::to_string);
                    let stored = if strict.is_some() {
                        name.clone()
                    } else {
                        cut200(
                            redact(&json!({"name": name}))
                                .get("name")
                                .unwrap_or(&Value::Null),
                        )
                    };
                    let before = cut200(
                        redact(&json!({"before": reason}))
                            .get("before")
                            .unwrap_or(&Value::Null),
                    );
                    let at = when::isoformat(&clock.now());
                    let inserted = db.lock().expect("the store's lock").execute(
                        INSERT,
                        params![
                            eid,
                            task_id,
                            "snapshot",
                            STARTUP_DISK,
                            stored,
                            before,
                            at,
                            None::<i64>,
                            None::<i64>
                        ],
                    );
                    match inserted {
                        Err(e) => {
                            let kind = match e.sqlite_error_code() {
                                Some(rusqlite::ErrorCode::ConstraintViolation) => "IntegrityError",
                                _ => "OperationalError",
                            };
                            one("error", json!(format!("no snapshot was taken ({kind})")))
                        }
                        Ok(_) => {
                            let mut out = Dict::new();
                            out.insert("snapshot".into(), json!(stored));
                            out.insert("record".into(), json!(eid));
                            if let Some(d) = strict {
                                out.insert(
                                    "snapshot_date".into(),
                                    json!(format!(
                                        "{} {}:{}:{}",
                                        &d[..10],
                                        &d[11..13],
                                        &d[13..15],
                                        &d[15..17]
                                    )),
                                );
                            }
                            state.lock().expect("snapshot state").last =
                                Some(merged(&out, one("taken_at", json!(at))));
                            out
                        }
                    }
                }
            };
            let _ = tx.send(Some(out));
        });
    }

    /// `last_failure` (for `ferb doctor`): when the last attempt failed and why; `off` when snapshots are off.
    pub fn last_failure(&self) -> Option<Dict> {
        if self.take.is_none() {
            return Some(one("off", json!(true)));
        }
        let st = self.state.lock().expect("snapshot state");
        st.failed.as_ref().map(|(at, why)| {
            let mut d = one("at", json!(when::isoformat(at)));
            d.insert("why".into(), json!(why));
            d
        })
    }
}

pub const SNAPSHOT_TOOL: &str = "recovery.snapshot";
pub const SNAPSHOT_DESCRIPTION: &str = "Before bulk or destructive file work, take a local snapshot of the Mac's disk \
    as a recovery point the owner can open. FERB already takes one before every shell command and permanent delete; \
    the last one is reused when no command ran since. Best effort. Not needed for single fs.write / fs.edit / fs.trash \
    calls: FERB keeps those recoverable itself.";

#[derive(Clone, Debug, serde::Deserialize)]
pub struct SnapshotArgs {
    pub before: String,
}

/// `recovery.snapshot`: an agent asks for a recovery point before bulk or destructive work it judged as such.
pub async fn recovery_snapshot(
    a: &SnapshotArgs,
    task_id: &str,
    snapshots: &Snapshots,
) -> ToolResult {
    let out = snapshots.request(Some(task_id), &a.before, false).await;
    if out.contains_key("error") {
        return ToolResult {
            content: merged(
                &out,
                one(
                    "next",
                    json!(
                        "Decide whether the work is still right without a snapshot; if you go on, say in your report \
                         that no snapshot was taken."
                    ),
                ),
            ),
            is_error: true,
            ..Default::default()
        };
    }
    let snapshot = out
        .get("snapshot")
        .map(ferb_py::value::py_str)
        .unwrap_or_default();
    let which = match out
        .get("snapshot_date")
        .filter(|d| ferb_py::value::truthy(d))
    {
        Some(d) => format!(
            "the local snapshot of {} ({snapshot})",
            ferb_py::value::py_str(d)
        ),
        None => snapshot,
    };
    let how = format!(
        "The owner can open it from macOS Recovery or Time Machine (admin rights): {which}."
    );
    let reused = if out.get("reused").is_some_and(ferb_py::value::truthy) {
        "The last snapshot is reused: no command ran since it was taken. "
    } else {
        ""
    };
    ToolResult {
        content: merged(&out, one("note", json!(format!("{reused}{how}")))),
        ..Default::default()
    }
}
