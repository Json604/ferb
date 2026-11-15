//! files.py: literal paths, paged reads, atomic writes, exact edits, copies, moves and the Trash, with normal OS
//! access. Each tool checks again the file it really opened (`ToolContext.path_closed`, F_GETPATH); an OS or decoding
//! error is the tool's plain error (nothing changed), never an outcome the gateway would record as unknown.
//!
//! A write or edit keeps the file as the owner had it apart from its text: its line endings, mode, extended attributes
//! and ACL (the new version starts as an APFS clone of the old one; where cloning isn't possible, mode and extended
//! attributes are copied, the ACL is not: files.py:203-205), and its hard links: a file with more than one name is
//! rewritten in place (not atomic; S1's copy was kept first).

use std::os::fd::RawFd;
use std::path::Path;
use std::sync::Arc;

use ferb_core::types::{
    ActionFacts, ActionSpec, Dict, Identity, RiskLevel, Target, ToolContext, ToolResult,
};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::hooks;
use crate::pypath as pp;
use crate::sys::{self, At, Fd, OsError, St};

pub const READ_DEFAULT: u64 = 256 * 1024;
pub const READ_MAX: u64 = 2 * 1024 * 1024;
pub const LIST_MAX: u64 = 500;
pub const TRASH_DIRS: [&str; 2] = [".Trash", ".Trashes"];

// ---------- errors ----------

/// Why a tool's file work stopped: the forms `_guarded` turns into the tool's plain error (files.py:425-433).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Fail {
    /// `PathRefused`: the message as is.
    Refused(String),
    /// `UnicodeDecodeError` at this byte.
    Decode(usize),
    /// `OSError`.
    Os(OsError),
    /// An exception `_guarded` doesn't catch (Python's `RuntimeError` from a link loop or an unknown home): it escapes
    /// the tool, and the gateway records the call's outcome as Python did for any other exception.
    Raised(String),
}

impl From<OsError> for Fail {
    fn from(e: OsError) -> Fail {
        Fail::Os(e)
    }
}

impl From<pp::SymlinkLoop> for Fail {
    fn from(e: pp::SymlinkLoop) -> Fail {
        Fail::Raised(format!("Symlink loop from {}", crate::py_repr(&e.0)))
    }
}

/// An exception that leaves a tool (Python raised it out of `run`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Raised {
    /// `RuntimeError` and other uncaught exceptions: the text of `str(e)`.
    Error(String),
    /// An `OSError` the tool didn't catch (fs.disk_usage on a missing path).
    Os(OsError),
    /// `ferb.core.errors.Refused` (fs.disk_usage): a refusal, the call failed cleanly.
    Refused(String),
}

pub type Out = Result<ToolResult, Raised>;

// ---------- the seams ----------

/// The Trash macOS names for the volume holding a path (NSFileManager `URLForDirectory:NSTrashDirectory ...
/// appropriateForURL: create:`), files.py `_trash_url`: the one way to the owner's real Trash, so a test gives its own.
pub trait TrashLocator: Send + Sync {
    fn trash_url(&self, near: &str, create: bool) -> Option<String>;
}

/// What the file tools need from the rest of FERB besides the call's context.
#[derive(Clone)]
pub struct FsEnv {
    /// The owner's home (`HOME`), for `~`.
    pub home: String,
    pub trash: Arc<dyn TrashLocator>,
    /// `full_disk_access() is Status.GRANTED` (ferb/device/permissions.py): read only when macOS refused a folder.
    pub full_disk_access: Arc<dyn Fn() -> bool + Send + Sync>,
}

// ---------- paths ----------

/// `Path(path).expanduser()`: Python raises when it can't name the home (`~nobody`).
fn expanduser(path: &str, home: &str) -> Result<String, Fail> {
    let out = pp::expanduser(path, home);
    if pp::tail(&out).first().is_some_and(|p| p.starts_with('~')) && !pp::is_abs(&out) {
        return Err(Fail::Raised("Could not determine home directory.".into()));
    }
    Ok(out)
}

/// `owner_path`: explicit home paths expanded; every other relative path is literal inside the workspace.
pub fn owner_path(path: &str, workspace: &str, home: &str) -> Result<String, Fail> {
    let p = expanduser(path, home)?;
    Ok(if pp::is_abs(&p) {
        p
    } else {
        pp::join(workspace, &p)
    })
}

/// pathlib's `_ignore_error`: what `exists()`/`is_dir()`/`is_file()` read as "no".
fn ignored(e: &OsError) -> bool {
    [libc::ENOENT, libc::ENOTDIR, libc::EBADF, libc::ELOOP].contains(&e.errno)
}

/// `Path.exists()`, `is_dir()`, `is_file()`: links followed; a not-found error is "no", any other is raised.
pub fn pl_stat(p: &str) -> Result<Option<St>, Fail> {
    if p.contains('\0') {
        return Ok(None);
    }
    match sys::stat(p) {
        Ok(st) => Ok(Some(st)),
        Err(e) if ignored(&e) => Ok(None),
        Err(e) => Err(Fail::Os(e)),
    }
}

pub fn pl_exists(p: &str) -> Result<bool, Fail> {
    Ok(pl_stat(p)?.is_some())
}
pub fn pl_is_dir(p: &str) -> Result<bool, Fail> {
    Ok(pl_stat(p)?.is_some_and(|s| s.is_dir()))
}
pub fn pl_is_file(p: &str) -> Result<bool, Fail> {
    Ok(pl_stat(p)?.is_some_and(|s| s.is_reg()))
}

/// `Path.resolve()` with the NUL a path can't hold raised as Python raises it.
fn resolve_path(p: &str) -> Result<String, Fail> {
    if p.contains('\0') {
        return Err(Fail::Raised("embedded null byte".into()));
    }
    Ok(pp::resolve(p)?)
}

/// `real_path`: the file a call really acts on: symlinks followed and `..` applied after them, as the OS does.
pub fn real_path(path: &str, workspace: &str, home: &str) -> Result<String, Fail> {
    let raw = owner_path(path, workspace, home)?;
    if pl_exists(&raw)? {
        resolve_path(&raw)
    } else {
        Ok(pp::join(&resolve_path(&pp::parent(&raw))?, &pp::name(&raw)))
    }
}

/// `link_path`: the item a call names, its last part never followed (a link names the link itself).
pub fn link_path(path: &str, workspace: &str, home: &str) -> Result<String, Fail> {
    let raw = owner_path(path, workspace, home)?;
    Ok(pp::join(&resolve_path(&pp::parent(&raw))?, &pp::name(&raw)))
}

/// `normalize_path`: one form for policy comparisons (textual).
pub fn normalize_path(path: &str, workspace: &str, home: &str) -> Result<String, Fail> {
    Ok(pp::normpath(&owner_path(path, workspace, home)?))
}

/// `resolve(path, ctx, must_exist)`.
pub fn resolve(path: &str, workspace: &str, home: &str, must_exist: bool) -> Result<String, Fail> {
    let p = real_path(path, workspace, home)?;
    if must_exist && !pl_exists(&p)? {
        return Err(Fail::Refused(format!("{path} does not exist")));
    }
    Ok(p)
}

/// `opened_path`: what an open file or folder really is (F_GETPATH).
pub fn opened_path(fd: RawFd) -> Result<String, OsError> {
    sys::opened_path(fd)
}

// ---------- facts ----------

/// `datetime.fromtimestamp(t, timezone.utc).isoformat()` for a float timestamp (round half to even to microseconds,
/// as `_PyTime_DoubleToDenominator`).
pub fn utc_iso(t: f64) -> String {
    let mut intpart = t.trunc();
    let mut frac = (t - intpart) * 1e6;
    let rounded = frac.round();
    frac = if (frac - rounded).abs() == 0.5 {
        2.0 * (frac / 2.0).round()
    } else {
        rounded
    };
    if frac >= 1e6 {
        frac -= 1e6;
        intpart += 1.0;
    } else if frac < 0.0 {
        frac += 1e6;
        intpart -= 1.0;
    }
    let dt = chrono::DateTime::from_timestamp(intpart as i64, (frac as u32) * 1000)
        .unwrap_or_default()
        .fixed_offset();
    ferb_core::when::isoformat(&dt)
}

/// `identity(p)`: local filesystem identity facts (links followed), not a judgment that this is the requested file.
pub fn identity(p: &str) -> Result<Dict, OsError> {
    let st = sys::stat(p)?;
    let mut d = Dict::new();
    d.insert("path".into(), json!(p));
    d.insert("source".into(), json!("local"));
    d.insert("size".into(), json!(st.size));
    d.insert("modified_at".into(), json!(utc_iso(st.mtime())));
    d.insert("owner_uid".into(), json!(st.uid));
    d.insert("owner_gid".into(), json!(st.gid));
    d.insert("device".into(), json!(st.dev));
    d.insert("inode".into(), json!(st.ino));
    d.insert("mtime_ns".into(), json!(st.mtime_ns() as i64));
    d.insert("symlink".into(), json!(pp::is_symlink(p)));
    Ok(d)
}

/// `_source`: a file under one of the untrusted roots (downloads) is a download.
pub fn source_of(p: &str, ctx: &ToolContext, home: &str) -> Result<(String, String), Fail> {
    for u in &ctx.untrusted_roots {
        let ur = resolve_path(&expanduser(u, home)?)?;
        if pp::is_relative_to(p, &ur) {
            return Ok(("download".into(), p.to_string()));
        }
    }
    Ok(("file".into(), p.to_string()))
}

// ---------- the results ----------

fn result(content: Dict) -> ToolResult {
    ToolResult {
        content,
        ..Default::default()
    }
}

/// `_err`: a file tool's error; macOS refusing a folder while FERB lacks Full Disk Access says so.
pub fn err(msg: &str, env: &FsEnv) -> ToolResult {
    if msg.contains("Operation not permitted") && !(env.full_disk_access)() {
        let mut d = Dict::new();
        d.insert("code".into(), json!("needs_permission"));
        d.insert("permission".into(), json!("Full Disk Access"));
        d.insert("error".into(), json!(msg));
        d.insert(
            "detail".into(),
            json!(
                "macOS blocks this folder until FERB has Full Disk Access: ask the owner to turn it on in System \
                 Settings → Privacy & Security → Full Disk Access (add FERB), then try again"
            ),
        );
        return ToolResult {
            content: d,
            is_error: true,
            ..Default::default()
        };
    }
    let mut d = Dict::new();
    d.insert("error".into(), json!(msg));
    ToolResult {
        content: d,
        is_error: true,
        ..Default::default()
    }
}

/// `_os_error`: an OS refusal as the tool's plain error, naming what didn't happen.
pub fn os_error(e: &OsError, what: &str, env: &FsEnv) -> ToolResult {
    let where_ = e
        .file()
        .filter(|f| !f.is_empty())
        .map(|f| format!(" ({f})"))
        .unwrap_or_default();
    err(&format!("{}{where_}: {what}", e.strerror_or_str()), env)
}

/// `_guarded`: the file work's outcome as the tool's result.
fn guarded(work: Result<ToolResult, Fail>, undone: &str, env: &FsEnv) -> Out {
    match work {
        Ok(r) => Ok(r),
        Err(Fail::Refused(m)) => Ok(err(&m, env)),
        Err(Fail::Decode(start)) => Ok(err(
            &format!("the file isn't UTF-8 text (byte {start}): {undone}; use a binary-aware tool"),
            env,
        )),
        Err(Fail::Os(e)) => Ok(os_error(&e, undone, env)),
        Err(Fail::Raised(m)) => Err(Raised::Error(m)),
    }
}

/// The protection lists' check on a path and what a call does to it (`ToolContext.path_closed`, by path text).
pub type AccessCheck<'a> = Option<&'a dyn Fn(&str, &str) -> Option<String>>;

/// The protection lists' check on a path, as a tool calls it.
pub type Check<'a> = Option<&'a dyn Fn(&str) -> Option<String>>;

fn path_closed(ctx: &ToolContext, path: &str, access: &str) -> Option<String> {
    ctx.path_closed
        .as_ref()
        .and_then(|c| (c.0)(Path::new(path), access))
}

/// `opened_closed`: the lists' answer for the file a tool really opened; a file FERB can't tell is refused.
pub fn opened_closed(ctx: &ToolContext, fd: RawFd, access: &str) -> Option<String> {
    ctx.path_closed.as_ref()?;
    match opened_path(fd) {
        Err(e) => Some(format!(
            "FERB couldn't tell which file it opened ({}); nothing was read or changed",
            e.strerror_or_str()
        )),
        Ok(where_) => path_closed(ctx, &where_, access),
    }
}

/// `_check_in`: refuses when the lists close `name` inside the opened folder, or when FERB can't tell which folder it
/// opened, before anything is written.
fn check_in(folder: RawFd, name: &str, closed: Check) -> Result<(), Fail> {
    let Some(closed) = closed else { return Ok(()) };
    let where_ = opened_path(folder).map_err(|e| {
        Fail::Refused(format!(
            "FERB couldn't tell which folder it opened ({}); nothing was written",
            e.strerror_or_str()
        ))
    })?;
    match closed(&pp::join(&where_, name)) {
        Some(r) if !r.is_empty() => Err(Fail::Refused(r)),
        _ => Ok(()),
    }
}

// ---------- folders, temp files, writes ----------

/// `make_folders`: `folder` and any missing folders above it, each made inside a folder FERB opened and checked.
/// `exact`: the deepest folder that exists must be where `folder` says, as macOS opened it (restore).
pub fn make_folders(folder: &str, closed: Check, exact: bool) -> Result<(), Fail> {
    let mut missing: Vec<String> = Vec::new();
    let mut top = pp::norm(folder);
    while !pl_exists(&top)? && top != pp::parent(&top) {
        missing.push(pp::name(&top));
        top = pp::parent(&top);
    }
    if missing.is_empty() {
        return Ok(());
    }
    let mut fd = sys::open(
        At::Cwd,
        &top,
        sys::O_RDONLY | sys::O_DIRECTORY | sys::O_CLOEXEC,
        0,
    )?;
    if exact {
        let here = opened_path(fd.raw()).map_err(|e| {
            Fail::Refused(format!(
                "FERB couldn't tell which folder it opened ({})",
                e.strerror_or_str()
            ))
        })?;
        if crate::casefold(&here) != crate::casefold(&top) {
            return Err(Fail::Refused(format!(
                "{top} isn't where it was (it now leads to {here})"
            )));
        }
    }
    for name in missing.iter().rev() {
        check_in(fd.raw(), name, closed)?;
        hooks::at("make_folders.mkdir");
        match sys::mkdir_at(At::Dir(fd.raw()), name, 0o777) {
            Ok(()) => {}
            Err(e) if e.is(libc::EEXIST) => {}
            Err(e) => return Err(e.into()),
        }
        let child = sys::open(
            At::Dir(fd.raw()),
            name,
            sys::O_RDONLY | sys::O_DIRECTORY | sys::O_NOFOLLOW | sys::O_CLOEXEC,
            0,
        )?;
        fd = child;
    }
    Ok(())
}

/// `secrets.token_hex(4)`.
pub fn token_hex4() -> String {
    let mut b = [0u8; 4];
    getrandom::fill(&mut b).expect("the system's random source");
    hex::encode(b)
}

fn temp_name(name: &str) -> String {
    format!(".{name}.{}.ferb-tmp", token_hex4())
}

/// `_sibling_temp`: a new temp file next to `name` inside the opened folder, never an existing file or link (0600).
fn sibling_temp(folder: RawFd, name: &str) -> Result<(Fd, String), Fail> {
    for _ in 0..100 {
        let temp = temp_name(name);
        let flags = sys::O_WRONLY | sys::O_CREAT | sys::O_EXCL | sys::O_NOFOLLOW | sys::O_CLOEXEC;
        match sys::open(At::Dir(folder), &temp, flags, 0o600) {
            Ok(fd) => return Ok((fd, temp)),
            Err(e) if e.is(libc::EEXIST) => continue,
            Err(e) => return Err(e.into()),
        }
    }
    Err(OsError::plain(&format!("no free temporary name next to {name}")).into())
}

/// `_clone_temp`: an APFS clone of `name` next to it (never through a link), or None when the volume can't clone.
fn clone_temp(folder: RawFd, name: &str) -> Option<String> {
    if !sys::mac::clonefileat_avail() || hooks::off("clonefileat") {
        return None;
    }
    for _ in 0..100 {
        let temp = temp_name(name);
        match sys::clonefileat(folder, name, folder, &temp, sys::CLONE_NOFOLLOW) {
            Ok(()) => return Some(temp),
            Err(code) if code == libc::EEXIST => continue,
            Err(_) => return None,
        }
    }
    None
}

/// `_copy_attributes`: the old version's mode and extended attributes onto the new one (its ACL can't be copied this
/// way). Best effort for attributes macOS won't let FERB set.
fn copy_attributes(src: RawFd, dst: RawFd) -> Result<(), Fail> {
    sys::fchmod(dst, sys::fstat(src)?.imode())?;
    for (name, value) in sys::xattrs(src) {
        sys::fsetxattr(dst, &name, &value);
    }
    Ok(())
}

/// How a text file was written: replaced atomically, or rewritten in place because it has other hard links.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Written {
    pub in_place: bool,
    pub links: u64,
}

impl Default for Written {
    fn default() -> Self {
        Written {
            in_place: false,
            links: 1,
        }
    }
}

/// `atomic_text`: `text` (UTF-8, line endings as given) to `p`, made next to it and renamed over it inside one opened
/// folder `closed` checks; an existing file keeps its mode, xattrs and ACL (clone) or mode and xattrs (copy); one with
/// other hard links is rewritten in place. A folder at `p` refuses.
pub fn atomic_text(p: &str, text: &str, closed: Check) -> Result<Written, Fail> {
    let name = pp::name(p);
    let folder = sys::open(
        At::Cwd,
        &pp::parent(p),
        sys::O_RDONLY | sys::O_DIRECTORY | sys::O_CLOEXEC,
        0,
    )
    .map_err(|e| {
        Fail::Refused(format!(
            "FERB couldn't open the folder for {name} ({}); nothing was written",
            e.strerror_or_str()
        ))
    })?;
    let data = text.as_bytes();
    let dir = folder.raw();
    check_in(dir, &name, closed)?;
    let old = match sys::stat_at(At::Dir(dir), &name, false) {
        Ok(st) => Some(st),
        Err(e) if e.is(libc::ENOENT) => None,
        Err(e) => return Err(e.into()),
    };
    if old.is_some_and(|o| o.is_dir()) {
        return Err(Fail::Refused(format!(
            "{p} is a folder, not a file; nothing was written"
        )));
    }
    hooks::at("atomic_text.stat");
    if let Some(o) = old.filter(|o| o.is_reg() && o.nlink > 1) {
        let fd = sys::open(
            At::Dir(dir),
            &name,
            sys::O_WRONLY | sys::O_NOFOLLOW | sys::O_CLOEXEC,
            0,
        )?;
        let st = sys::fstat(fd.raw())?;
        if st.id() != o.id() {
            return Err(Fail::Refused(format!(
                "{p} changed while FERB wrote it; nothing was written"
            )));
        }
        sys::ftruncate(fd.raw(), 0)?;
        sys::write_all(fd.raw(), data)?;
        return Ok(Written {
            in_place: true,
            links: o.nlink,
        });
    }
    let regular = old.is_some_and(|o| o.is_reg());
    let mut temp = if regular {
        clone_temp(dir, &name)
    } else {
        None
    };
    let cloned = temp.is_some();
    let work = (|| -> Result<(), Fail> {
        let fd = if cloned {
            let (o, t) = (old.expect("regular"), temp.clone().expect("cloned"));
            // the clone has the old mode; the owner's write bit for the rewrite, the old mode back after
            sys::chmod_at_nofollow(At::Dir(dir), &t, o.imode() | 0o200)?;
            sys::open(
                At::Dir(dir),
                &t,
                sys::O_WRONLY | sys::O_TRUNC | sys::O_NOFOLLOW | sys::O_CLOEXEC,
                0,
            )?
        } else {
            let (fd, t) = sibling_temp(dir, &name)?;
            temp = Some(t);
            fd
        };
        sys::write_all(fd.raw(), data)?;
        if cloned {
            sys::fchmod(fd.raw(), old.expect("regular").imode())?;
        } else if regular {
            let src = sys::open(
                At::Dir(dir),
                &name,
                sys::O_RDONLY | sys::O_NOFOLLOW | sys::O_CLOEXEC,
                0,
            )?;
            copy_attributes(src.raw(), fd.raw())?;
        }
        drop(fd);
        let t = temp.clone().expect("made");
        sys::rename_at(At::Dir(dir), &t, At::Dir(dir), &name)?; // replaces atomically (POSIX rename)
        Ok(())
    })();
    if let Some(t) = &temp {
        match sys::unlink_at(At::Dir(dir), t) {
            Ok(()) => {}
            Err(e) if e.is(libc::ENOENT) => {}
            Err(e) => return Err(e.into()), // Python's `finally` raises over the work's own error
        }
    }
    work?;
    Ok(Written::default())
}

/// `new_file`: create `p` with `data`, never replacing anything (O_EXCL inside one opened folder `closed` checks); a
/// write that fails part way removes the part written.
pub fn new_file(p: &str, data: &[u8], closed: Check) -> Result<(), Fail> {
    let name = pp::name(p);
    let folder = sys::open(
        At::Cwd,
        &pp::parent(p),
        sys::O_RDONLY | sys::O_DIRECTORY | sys::O_CLOEXEC,
        0,
    )
    .map_err(|e| {
        Fail::Refused(format!(
            "FERB couldn't open the folder for {name} ({}); nothing was written",
            e.strerror_or_str()
        ))
    })?;
    check_in(folder.raw(), &name, closed)?;
    let flags = sys::O_WRONLY | sys::O_CREAT | sys::O_EXCL | sys::O_NOFOLLOW | sys::O_CLOEXEC;
    let fd = match sys::open(At::Dir(folder.raw()), &name, flags, 0o644) {
        Ok(fd) => fd,
        Err(e) if e.is(libc::EEXIST) => {
            return Err(Fail::Refused(format!(
                "{p} already exists; FERB never overwrites it, so nothing was written"
            )))
        }
        Err(e) => return Err(e.into()),
    };
    if let Err(e) = sys::write_all(fd.raw(), data) {
        drop(fd);
        sys::unlink_at(At::Dir(folder.raw()), &name)?;
        return Err(e.into());
    }
    Ok(())
}

/// `destination_closed`: the lists' answer for a file about to be made at `path`, checked through the deepest folder
/// above it that exists, as macOS opens it. A folder FERB can't open or tell is refused.
pub fn destination_closed(path: &str, closed: Check) -> Result<Option<String>, Fail> {
    let Some(closed) = closed else {
        return Ok(None);
    };
    let mut tail = vec![pp::name(path)];
    let mut folder = pp::parent(path);
    while !pl_exists(&folder)? && folder != pp::parent(&folder) {
        tail.push(pp::name(&folder));
        folder = pp::parent(&folder);
    }
    let fd = match sys::open(
        At::Cwd,
        &folder,
        sys::O_RDONLY | sys::O_DIRECTORY | sys::O_CLOEXEC,
        0,
    ) {
        Ok(fd) => fd,
        Err(e) => {
            return Ok(Some(format!(
                "FERB couldn't open {folder} ({}); nothing was written",
                e.strerror_or_str()
            )))
        }
    };
    let where_ = match opened_path(fd.raw()) {
        Ok(w) => w,
        Err(e) => {
            return Ok(Some(format!(
                "FERB couldn't tell which folder it opened ({}); nothing was written",
                e.strerror_or_str()
            )))
        }
    };
    Ok(closed(&pp::join_all(
        &where_,
        tail.iter().rev().map(String::as_str),
    )))
}

// ---------- the tools ----------

fn default_list_limit() -> u64 {
    LIST_MAX
}
fn default_read_max() -> u64 {
    READ_DEFAULT
}

/// The arguments every path tool shares (the gateway validates their bounds before a call: pydantic's `Field`).
#[derive(Clone, Debug, Deserialize)]
pub struct ListArgs {
    pub path: String,
    #[serde(default)]
    pub offset: u64,
    #[serde(default = "default_list_limit")]
    pub limit: u64,
}

#[derive(Clone, Debug, Deserialize)]
pub struct ReadArgs {
    pub path: String,
    #[serde(default)]
    pub offset: u64,
    #[serde(default = "default_read_max")]
    pub max_bytes: u64,
}

#[derive(Clone, Debug, Deserialize)]
pub struct WriteArgs {
    pub path: String,
    pub content: String,
    #[serde(default)]
    pub overwrite: bool,
}

#[derive(Clone, Debug, Deserialize)]
pub struct EditArgs {
    pub path: String,
    pub old_text: String,
    pub new_text: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct CopyArgs {
    pub path: String,
    pub destination: String,
    #[serde(default)]
    pub overwrite: bool,
}

#[derive(Clone, Debug, Deserialize)]
pub struct MoveArgs {
    pub path: String,
    pub destination: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct PathArgs {
    pub path: String,
}

/// What every file tool declares about itself (files.py `_Base` and each class's attributes).
#[derive(Clone, Copy, Debug)]
pub struct ToolInfo {
    pub name: &'static str,
    pub description: &'static str,
    pub scopes: &'static [&'static str],
    pub risk: RiskLevel,
    pub reversible: bool,
    pub access: &'static str,
    pub target_args: &'static [&'static str],
    pub identities: &'static [Identity],
}

const BOTH: &[Identity] = &[Identity::Owner, Identity::Ferb];

pub const FS_LIST: ToolInfo = ToolInfo {
    name: "fs.list",
    description: "List every folder candidate through offset pages, including full path, source, size, owner and \
                  modification time.",
    scopes: &["fs:read"],
    risk: RiskLevel::Low,
    reversible: true,
    access: "read",
    target_args: &["path"],
    identities: BOTH,
};
pub const FS_READ: ToolInfo = ToolInfo {
    name: "fs.read",
    description: "Read a text file (UTF-8), optionally a byte range.",
    scopes: &["fs:read"],
    risk: RiskLevel::Low,
    reversible: true,
    access: "read",
    target_args: &["path"],
    identities: BOTH,
};
pub const FS_WRITE: ToolInfo = ToolInfo {
    name: "fs.write",
    description: "Create or overwrite a text file.",
    scopes: &["fs:write"],
    risk: RiskLevel::Medium,
    reversible: false,
    access: "write",
    target_args: &["path"],
    identities: BOTH,
};
pub const FS_EDIT: ToolInfo = ToolInfo {
    name: "fs.edit",
    description: "Replace one exact occurrence of old_text with new_text in a UTF-8 text file. The rest of the file \
                  stays byte for byte, its line endings too (in a file with CRLF line endings, \\n in old_text and \
                  new_text means its CRLF). Include enough surrounding text that old_text matches only the \
                  occurrence you mean.",
    scopes: &["fs:write"],
    risk: RiskLevel::Medium,
    reversible: false,
    access: "write",
    target_args: &["path"],
    identities: BOTH,
};
pub const FS_COPY: ToolInfo = ToolInfo {
    name: "fs.copy",
    description: "Copy one file (any kind: PDF, image, text) to destination, a file path. An existing file there is \
                  replaced only with overwrite=true (FERB keeps a copy of it first).",
    scopes: &["fs:write"],
    risk: RiskLevel::Medium,
    reversible: false,
    access: "write",
    target_args: &["destination"],
    identities: BOTH,
};
pub const FS_MOVE: ToolInfo = ToolInfo {
    name: "fs.move",
    description: "Move or rename a file or folder to destination (its new full path) on the same disk. Never \
                  replaces anything at destination; across disks, copy with fs.copy instead.",
    scopes: &["fs:write"],
    risk: RiskLevel::Low,
    reversible: true,
    access: "remove",
    target_args: &["destination"],
    identities: BOTH,
};
pub const FS_TRASH: ToolInfo = ToolInfo {
    name: "fs.trash",
    description: "Move a file or folder to the Trash (recoverable). A link goes itself; what it points to stays.",
    scopes: &["fs:write"],
    risk: RiskLevel::Low,
    reversible: true,
    access: "remove",
    target_args: &["path"],
    identities: BOTH,
};

/// `_Base.describe_path`: the path as given, `~` expanded (pathlib's spelling).
pub fn describe_path(path: &str, home: &str) -> Result<String, Fail> {
    expanduser(path, home)
}

fn spec(
    kind: &str,
    path: String,
    writes: Vec<String>,
    deletes: Vec<String>,
    content: Option<String>,
) -> ActionSpec {
    let facts = (!writes.is_empty() || !deletes.is_empty()).then(|| ActionFacts {
        writes,
        deletes,
        ..Default::default()
    });
    ActionSpec {
        kind: kind.into(),
        target: Target {
            path: Some(path),
            ..Default::default()
        },
        content,
        facts,
        ..Default::default()
    }
}

/// The gateway's view of each file tool's call: what it does (`describe`), the files it names and what it does to
/// them (`file_paths`, rule 10), its locks, the file S1 copies (`real_path`) and why (`recovery_reason`).
pub mod declared {
    use super::*;

    pub fn list_describe(a: &ListArgs, home: &str) -> Result<ActionSpec, Fail> {
        Ok(spec(
            "read",
            describe_path(&a.path, home)?,
            vec![],
            vec![],
            None,
        ))
    }
    pub fn read_describe(a: &ReadArgs, home: &str) -> Result<ActionSpec, Fail> {
        Ok(spec(
            "read",
            describe_path(&a.path, home)?,
            vec![],
            vec![],
            None,
        ))
    }
    pub fn write_describe(a: &WriteArgs, home: &str) -> Result<ActionSpec, Fail> {
        let p = describe_path(&a.path, home)?;
        Ok(spec(
            "write_file",
            p.clone(),
            vec![p],
            vec![],
            Some(a.content.clone()),
        ))
    }
    pub fn edit_describe(a: &EditArgs, home: &str) -> Result<ActionSpec, Fail> {
        let p = describe_path(&a.path, home)?;
        Ok(spec(
            "write_file",
            p.clone(),
            vec![p],
            vec![],
            Some(a.new_text.clone()),
        ))
    }
    pub fn copy_describe(a: &CopyArgs, home: &str) -> Result<ActionSpec, Fail> {
        let d = expanduser(&a.destination, home)?;
        Ok(spec("write_file", d.clone(), vec![d], vec![], None))
    }
    pub fn move_describe(a: &MoveArgs, home: &str) -> Result<ActionSpec, Fail> {
        let (s, d) = (
            expanduser(&a.path, home)?,
            expanduser(&a.destination, home)?,
        );
        Ok(spec("write_file", d.clone(), vec![s, d], vec![], None))
    }
    pub fn trash_describe(a: &PathArgs, home: &str) -> Result<ActionSpec, Fail> {
        let p = describe_path(&a.path, home)?;
        Ok(spec("trash", p.clone(), vec![], vec![p], None))
    }

    /// `file_paths` for a one-path tool: the path as given and the tool's access.
    pub fn file_paths(path: &str, info: &ToolInfo) -> Vec<(String, &'static str)> {
        vec![(path.to_string(), info.access)]
    }
    pub fn copy_file_paths(a: &CopyArgs) -> Vec<(String, &'static str)> {
        vec![(a.path.clone(), "read"), (a.destination.clone(), "write")]
    }
    pub fn move_file_paths(a: &MoveArgs) -> Vec<(String, &'static str)> {
        vec![(a.path.clone(), "remove"), (a.destination.clone(), "write")]
    }

    /// `locks` for a one-path tool: `file:<the path as given, ~ expanded>`.
    pub fn locks(path: &str, home: &str) -> Result<Vec<String>, Fail> {
        Ok(vec![format!("file:{}", describe_path(path, home)?)])
    }
    pub fn two_locks(path: &str, destination: &str, home: &str) -> Result<Vec<String>, Fail> {
        Ok(vec![
            format!("file:{}", expanduser(path, home)?),
            format!("file:{}", expanduser(destination, home)?),
        ])
    }

    /// `real_path` hooks: what the gateway locks and S1 copies.
    pub fn real(path: &str, workspace: &str, home: &str) -> Result<String, Fail> {
        real_path(path, workspace, home)
    }
    pub fn move_real(a: &MoveArgs, workspace: &str, home: &str) -> Result<String, Fail> {
        link_path(&a.destination, workspace, home)
    }
    pub fn trash_real(a: &PathArgs, workspace: &str, home: &str) -> Result<String, Fail> {
        link_path(&a.path, workspace, home)
    }

    /// `recovery_reason`: why the gateway copies an existing file first (None: this call can't change one).
    pub fn write_reason(a: &WriteArgs) -> Option<&'static str> {
        a.overwrite.then_some("overwrite")
    }
    pub fn edit_reason(_: &EditArgs) -> Option<&'static str> {
        Some("edit")
    }
    pub fn copy_reason(a: &CopyArgs) -> Option<&'static str> {
        a.overwrite.then_some("overwrite")
    }
    pub fn move_reason(_: &MoveArgs) -> Option<&'static str> {
        None
    }
}

/// `write_check`: the lists' check for a write, for the other tools that write files (recordings).
pub fn write_check(ctx: &ToolContext) -> Option<impl Fn(&str) -> Option<String> + '_> {
    ctx.path_closed
        .as_ref()
        .map(|c| move |path: &str| (c.0)(Path::new(path), "write"))
}

/// fs.list.
pub fn fs_list(a: &ListArgs, ctx: &ToolContext, env: &FsEnv) -> Out {
    guarded(list_work(a, ctx, env), "nothing was listed", env)
}

fn list_work(a: &ListArgs, ctx: &ToolContext, env: &FsEnv) -> Result<ToolResult, Fail> {
    let p = resolve(&a.path, &ctx.workspace, &env.home, true)?;
    if !pl_is_dir(&p)? {
        return Ok(err(&format!("{p} is not a folder"), env));
    }
    let folder = sys::open(
        At::Cwd,
        &p,
        sys::O_RDONLY | sys::O_DIRECTORY | sys::O_CLOEXEC,
        0,
    )?;
    if let Some(refusal) =
        opened_closed(ctx, folder.raw(), FS_LIST.access).filter(|r| !r.is_empty())
    {
        return Ok(err(&refusal, env));
    }
    let mut names: Vec<String> = sys::scandir(folder.raw())?
        .into_iter()
        .map(|e| e.name)
        .collect();
    drop(folder);
    names.sort();
    let children: Vec<String> = names.iter().map(|n| pp::join(&p, n)).collect();
    let (offset, limit) = (a.offset as usize, a.limit as usize);
    let (mut entries, mut errors) = (Vec::new(), Vec::new());
    for child in children.iter().skip(offset).take(limit) {
        let kind = match pl_is_dir(child) {
            Ok(d) => Ok(if d { "dir" } else { "file" }),
            Err(Fail::Os(e)) => Err(e),
            Err(other) => return Err(other),
        };
        match kind.and_then(|k| identity(child).map(|id| (k, id))) {
            Ok((k, id)) => {
                let mut d = Dict::new();
                d.insert("name".into(), json!(pp::name(child)));
                d.insert("kind".into(), json!(k));
                d.extend(id);
                entries.push(Value::Object(d));
            }
            Err(e) => errors.push(json!({"path": child, "error": e.to_string()})),
        }
    }
    let next = (offset + limit < children.len()).then_some(offset + limit);
    let mut d = Dict::new();
    d.insert("path".into(), json!(p));
    d.insert("entries".into(), Value::Array(entries));
    d.insert("errors".into(), Value::Array(errors));
    d.insert("total".into(), json!(children.len()));
    d.insert("offset".into(), json!(offset));
    d.insert("next_offset".into(), json!(next));
    d.insert("truncated".into(), json!(next.is_some()));
    d.insert(
        "retrieval".into(),
        json!(next.map(|_| "fs.list with next_offset until null")),
    );
    let mut r = result(d);
    r.read_sources = vec![source_of(&p, ctx, &env.home)?];
    Ok(r)
}

/// fs.read: a page of a UTF-8 file, finishing the scalar a page would cut (at most three more bytes).
pub fn fs_read(a: &ReadArgs, ctx: &ToolContext, env: &FsEnv) -> Out {
    guarded(read_work(a, ctx, env), "nothing was read", env)
}

fn utf8_end_error(data: &[u8]) -> Option<(usize, bool)> {
    match std::str::from_utf8(data) {
        Ok(_) => None,
        Err(e) => Some((e.valid_up_to(), e.error_len().is_none())),
    }
}

fn read_work(a: &ReadArgs, ctx: &ToolContext, env: &FsEnv) -> Result<ToolResult, Fail> {
    let p = resolve(&a.path, &ctx.workspace, &env.home, true)?;
    if !pl_is_file(&p)? {
        return Ok(err(&format!("{p} is not a file"), env));
    }
    let (offset, count) = (a.offset, a.max_bytes as usize);
    let stream = sys::open(At::Cwd, &p, sys::O_RDONLY | sys::O_CLOEXEC, 0)?;
    if let Some(refusal) =
        opened_closed(ctx, stream.raw(), FS_READ.access).filter(|r| !r.is_empty())
    {
        return Ok(err(&refusal, env));
    }
    sys::seek(stream.raw(), offset as i64)?;
    let mut data = sys::read_up_to(stream.raw(), count)?;
    let mut done = false;
    for _ in 0..3 {
        match utf8_end_error(&data) {
            None => {
                done = true;
                break;
            }
            Some((start, at_end)) => {
                if !at_end {
                    return Ok(err(
                        &format!(
                            "{p} is not UTF-8 at byte {}; use a binary-aware tool",
                            offset as usize + start
                        ),
                        env,
                    ));
                }
                let more = sys::read_up_to(stream.raw(), 1)?;
                if more.is_empty() {
                    return Ok(err(
                        &format!("{p} ends with invalid UTF-8; use a binary-aware tool"),
                        env,
                    ));
                }
                data.extend_from_slice(&more);
            }
        }
    }
    if !done && utf8_end_error(&data).is_some() {
        return Ok(err(
            &format!("offset {offset} is not a UTF-8 boundary; resume at the returned next_offset"),
            env,
        ));
    }
    drop(stream);
    let text = String::from_utf8(data.clone()).expect("checked");
    let mut d = identity(&p)?;
    let size = d["size"].as_i64().unwrap_or(0);
    let end = offset as i64 + data.len() as i64;
    let next = (end < size).then_some(end);
    d.insert("text".into(), json!(text));
    d.insert("offset".into(), json!(offset));
    d.insert("next_offset".into(), json!(next));
    d.insert("truncated".into(), json!(next.is_some()));
    d.insert(
        "retrieval".into(),
        json!(next.map(|_| "fs.read with next_offset until null")),
    );
    let mut r = result(d);
    r.read_sources = vec![source_of(&p, ctx, &env.home)?];
    Ok(r)
}

/// `_written_facts`: what a write left, read back; a failure to read it back is said, never an error (it ran).
fn written_facts(p: &str, how: Written) -> Dict {
    let mut out = Dict::new();
    if how.in_place {
        out.insert("in_place".into(), json!(true));
        out.insert("hard_links".into(), json!(how.links));
    }
    let read = identity(p).and_then(|id| sys::stat(p).map(|st| (id, st.size)));
    match read {
        Ok((mut id, size)) => {
            id.insert("bytes".into(), json!(size));
            id.extend(out);
            id
        }
        Err(e) => {
            let mut d = Dict::new();
            d.insert("path".into(), json!(p));
            d.insert("written".into(), json!(true));
            d.insert("readback_error".into(), json!(e.strerror_or_str()));
            d.extend(out);
            d
        }
    }
}

/// fs.write: create or overwrite a text file.
pub fn fs_write(a: &WriteArgs, ctx: &ToolContext, env: &FsEnv) -> Out {
    guarded(write_work(a, ctx, env), "nothing was written", env)
}

fn write_work(a: &WriteArgs, ctx: &ToolContext, env: &FsEnv) -> Result<ToolResult, Fail> {
    let p = resolve(&a.path, &ctx.workspace, &env.home, false)?;
    if pl_is_dir(&p)? {
        return Ok(err(
            &format!("{p} is a folder, not a file; nothing was written"),
            env,
        ));
    }
    if pl_exists(&p)? && !a.overwrite {
        return Ok(err(
            &format!("{p} exists (set overwrite=true to replace it)"),
            env,
        ));
    }
    let check = write_check(ctx);
    let check_ref: Check = check.as_ref().map(|c| c as &dyn Fn(&str) -> Option<String>);
    make_folders(&pp::parent(&p), check_ref, false)?;
    let how = atomic_text(&p, &a.content, check_ref)?;
    Ok(result(written_facts(&p, how)))
}

/// Python's `str.count` and `str.replace(old, new, 1)` (non-overlapping, from the left).
fn count(text: &str, old: &str) -> usize {
    text.matches(old).count()
}

/// fs.edit: one exact occurrence replaced, the rest byte for byte (a CRLF file's `\n` is its CRLF).
pub fn fs_edit(a: &EditArgs, ctx: &ToolContext, env: &FsEnv) -> Out {
    guarded(edit_work(a, ctx, env), "nothing was changed", env)
}

fn edit_work(a: &EditArgs, ctx: &ToolContext, env: &FsEnv) -> Result<ToolResult, Fail> {
    let p = resolve(&a.path, &ctx.workspace, &env.home, true)?;
    if !pl_is_file(&p)? {
        return Ok(err(&format!("{p} is not a file; nothing was changed"), env));
    }
    let stream = sys::open(At::Cwd, &p, sys::O_RDONLY | sys::O_CLOEXEC, 0)?;
    if let Some(refusal) =
        opened_closed(ctx, stream.raw(), FS_EDIT.access).filter(|r| !r.is_empty())
    {
        return Ok(err(&refusal, env));
    }
    let bytes = sys::read_to_end(stream.raw())?;
    drop(stream);
    let text = String::from_utf8(bytes).map_err(|e| Fail::Decode(e.utf8_error().valid_up_to()))?;
    let (mut old, mut new) = (a.old_text.clone(), a.new_text.clone());
    let mut n = count(&text, &old);
    if n == 0 && text.contains("\r\n") && old.contains('\n') && !old.contains('\r') {
        let crlf_old = old.replace('\n', "\r\n");
        let crlf_new = new.replace("\r\n", "\n").replace('\n', "\r\n");
        let c = count(&text, &crlf_old);
        if c != 0 {
            (old, new, n) = (crlf_old, crlf_new, c);
        }
    }
    if n != 1 {
        return Ok(err(
            &format!("old_text occurs {n} times; it must occur exactly once"),
            env,
        ));
    }
    let check = write_check(ctx);
    let check_ref: Check = check.as_ref().map(|c| c as &dyn Fn(&str) -> Option<String>);
    hooks::at("fs_edit.read");
    let how = atomic_text(&p, &text.replacen(&old, &new, 1), check_ref)?;
    let mut d = Dict::new();
    d.insert("path".into(), json!(p));
    d.insert("replaced".into(), json!(1));
    if how.in_place {
        d.insert("in_place".into(), json!(true));
        d.insert("hard_links".into(), json!(how.links));
    }
    let mut r = result(d);
    r.read_sources = vec![source_of(&p, ctx, &env.home)?];
    Ok(r)
}

/// `_copy_into`: the opened file `src` to `p`, made next to it and renamed into place inside one opened folder
/// `closed` checks; never replacing anything unless `overwrite` (RENAME_EXCL otherwise).
fn copy_into(src: RawFd, p: &str, overwrite: bool, closed: Check) -> Result<(), Fail> {
    let name = pp::name(p);
    let folder = sys::open(
        At::Cwd,
        &pp::parent(p),
        sys::O_RDONLY | sys::O_DIRECTORY | sys::O_CLOEXEC,
        0,
    )
    .map_err(|e| {
        Fail::Refused(format!(
            "FERB couldn't open the folder for {name} ({}); nothing was copied",
            e.strerror_or_str()
        ))
    })?;
    let dir = folder.raw();
    check_in(dir, &name, closed)?;
    let (fd, temp) = sibling_temp(dir, &name)?;
    let work = (|| -> Result<(), Fail> {
        loop {
            let chunk = sys::read(src, 1 << 20)?;
            if chunk.is_empty() {
                break;
            }
            sys::write_all(fd.raw(), &chunk)?;
        }
        copy_attributes(src, fd.raw())?;
        Ok(())
    })();
    drop(fd);
    let work = work.and_then(|()| {
        if overwrite {
            sys::rename_at(At::Dir(dir), &temp, At::Dir(dir), &name).map_err(Fail::from)
        } else {
            match move_excl(dir, &temp, dir, &name) {
                Err(e) if e.is(libc::EEXIST) => Err(Fail::Refused(format!(
                    "{p} exists (set overwrite=true to replace it); nothing was copied"
                ))),
                other => other.map_err(Fail::from),
            }
        }
    });
    match sys::unlink_at(At::Dir(dir), &temp) {
        Ok(()) => {}
        Err(e) if e.is(libc::ENOENT) => {}
        Err(e) => return Err(e.into()), // Python's `finally` raises over the work's own error
    }
    work
}

/// fs.copy (GATE-539): one file of any kind; an existing file is replaced only with overwrite.
pub fn fs_copy(a: &CopyArgs, ctx: &ToolContext, env: &FsEnv) -> Out {
    guarded(copy_work(a, ctx, env), "nothing was copied", env)
}

fn copy_work(a: &CopyArgs, ctx: &ToolContext, env: &FsEnv) -> Result<ToolResult, Fail> {
    let src = resolve(&a.path, &ctx.workspace, &env.home, true)?;
    let dest = resolve(&a.destination, &ctx.workspace, &env.home, false)?;
    if !pl_is_file(&src)? {
        return Ok(err(
            &format!("{src} is not a file; nothing was copied"),
            env,
        ));
    }
    if pl_is_dir(&dest)? {
        return Ok(err(
            &format!("{dest} is a folder: give the new file's full path; nothing was copied"),
            env,
        ));
    }
    let fd = sys::open(
        At::Cwd,
        &src,
        sys::O_RDONLY | sys::O_NOFOLLOW | sys::O_CLOEXEC,
        0,
    )?;
    if let Some(refusal) = opened_closed(ctx, fd.raw(), "read").filter(|r| !r.is_empty()) {
        return Ok(err(&refusal, env));
    }
    let check = write_check(ctx);
    let check_ref: Check = check.as_ref().map(|c| c as &dyn Fn(&str) -> Option<String>);
    make_folders(&pp::parent(&dest), check_ref, false)?;
    copy_into(fd.raw(), &dest, a.overwrite, check_ref)?;
    drop(fd);
    let mut d = identity(&dest)?;
    d.insert("copied_from".into(), json!(src));
    let mut r = result(d);
    r.read_sources = vec![source_of(&src, ctx, &env.home)?];
    Ok(r)
}

/// fs.move (GATE-539): a file or folder moved or renamed on its volume, never replacing anything.
pub fn fs_move(a: &MoveArgs, ctx: &ToolContext, env: &FsEnv) -> Out {
    guarded(move_work(a, ctx, env), "nothing was moved", env)
}

fn move_work(a: &MoveArgs, ctx: &ToolContext, env: &FsEnv) -> Result<ToolResult, Fail> {
    let src = link_path(&a.path, &ctx.workspace, &env.home)?;
    let dest = link_path(&a.destination, &ctx.workspace, &env.home)?;
    if !pp::lexists(&src) {
        return Ok(err(&format!("{} does not exist", a.path), env));
    }
    let check = write_check(ctx);
    let check_ref: Check = check.as_ref().map(|c| c as &dyn Fn(&str) -> Option<String>);
    make_folders(&pp::parent(&dest), check_ref, false)?;
    let src_dir = match sys::open(
        At::Cwd,
        &pp::parent(&src),
        sys::O_SEARCH | sys::O_DIRECTORY | sys::O_CLOEXEC,
        0,
    ) {
        Ok(fd) => fd,
        Err(e) => {
            return Ok(err(
                &format!(
                    "FERB couldn't open the folder of {} ({}); nothing was moved",
                    pp::name(&src),
                    e.strerror_or_str()
                ),
                env,
            ))
        }
    };
    let dst_dir = match sys::open(
        At::Cwd,
        &pp::parent(&dest),
        sys::O_RDONLY | sys::O_DIRECTORY | sys::O_CLOEXEC,
        0,
    ) {
        Ok(fd) => fd,
        Err(e) => {
            return Ok(err(
                &format!(
                    "FERB couldn't open the folder for {} ({}); nothing was moved",
                    pp::name(&dest),
                    e.strerror_or_str()
                ),
                env,
            ))
        }
    };
    let remove = ctx
        .path_closed
        .as_ref()
        .map(|c| move |path: &str| (c.0)(Path::new(path), "remove"));
    let remove_ref: Check = remove
        .as_ref()
        .map(|c| c as &dyn Fn(&str) -> Option<String>);
    check_in(src_dir.raw(), &pp::name(&src), remove_ref)?;
    check_in(dst_dir.raw(), &pp::name(&dest), check_ref)?;
    match move_excl(
        src_dir.raw(),
        &pp::name(&src),
        dst_dir.raw(),
        &pp::name(&dest),
    ) {
        Ok(()) => {}
        Err(e) if e.is(libc::EEXIST) => {
            return Ok(err(&format!("{dest} exists; nothing was moved"), env))
        }
        Err(e) if e.is(libc::EXDEV) => {
            return Ok(err(
                &format!(
                    "{dest} is on another disk: copy it with fs.copy instead; nothing was moved"
                ),
                env,
            ))
        }
        Err(e) => return Err(e.into()),
    }
    let mut d = Dict::new();
    d.insert("path".into(), json!(dest));
    d.insert("moved_from".into(), json!(src));
    Ok(result(d))
}

/// fs.trash: the item itself (a link goes, what it points to stays) to the Trash of its volume.
pub fn fs_trash(a: &PathArgs, ctx: &ToolContext, env: &FsEnv) -> Out {
    guarded(trash_work(a, ctx, env), "nothing was moved", env)
}

fn trash_work(a: &PathArgs, ctx: &ToolContext, env: &FsEnv) -> Result<ToolResult, Fail> {
    let p = link_path(&a.path, &ctx.workspace, &env.home)?;
    if !pp::lexists(&p) {
        return Ok(err(&format!("{} does not exist", a.path), env));
    }
    let folder = match sys::open(
        At::Cwd,
        &pp::parent(&p),
        sys::O_SEARCH | sys::O_DIRECTORY | sys::O_CLOEXEC,
        0,
    ) {
        Ok(fd) => fd,
        Err(e) => {
            return Ok(err(
                &format!(
                    "FERB couldn't open the folder of {} ({}); nothing was moved",
                    pp::name(&p),
                    e.strerror_or_str()
                ),
                env,
            ))
        }
    };
    let closed = ctx
        .path_closed
        .as_ref()
        .map(|c| move |path: &str, access: &str| (c.0)(Path::new(path), access));
    let closed_ref = closed
        .as_ref()
        .map(|c| c as &dyn Fn(&str, &str) -> Option<String>);
    let t = trash_item(
        folder.raw(),
        &pp::name(&p),
        closed_ref,
        FS_TRASH.access,
        env,
    );
    drop(folder);
    let Some(trashed_to) = t.trashed_to.clone() else {
        return Ok(err(t.error.as_deref().unwrap_or("nothing was moved"), env));
    };
    let where_ = t.where_.clone().unwrap_or_default();
    if let Some(e) = &t.error {
        let mut d = Dict::new();
        d.insert("error".into(), json!(e));
        d.insert("path".into(), json!(where_));
        d.insert("trashed_to".into(), json!(trashed_to));
        d.insert("verified".into(), json!(false));
        return Ok(ToolResult {
            content: d,
            is_error: true,
            ..Default::default()
        });
    }
    let mut d = Dict::new();
    d.insert("path".into(), json!(where_));
    d.insert("trashed_to".into(), json!(trashed_to));
    d.insert("device".into(), json!(t.device));
    d.insert("inode".into(), json!(t.inode));
    Ok(result(d))
}

// ---------- the Trash ----------

/// `move_excl`: `src` in one open folder to `dst` in another (renameatx_np RENAME_EXCL); whatever stands at `dst` is
/// never replaced (EEXIST). The point `move_excl.done` follows each move (tests act there as an attacker).
pub fn move_excl(src_dir: RawFd, src: &str, dst_dir: RawFd, dst: &str) -> Result<(), OsError> {
    sys::renameatx_excl(src_dir, src, dst_dir, dst)?;
    hooks::at("move_excl.done");
    Ok(())
}

/// `in_trash`: `folder` is a Trash folder or inside one.
pub fn in_trash(folder: &str) -> bool {
    pp::tail(&pp::norm(folder))
        .iter()
        .any(|p| TRASH_DIRS.contains(p))
}

/// `TrashRefused`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrashRefused(pub String);

fn owner_home() -> Option<String> {
    let mut pwd: libc::passwd = unsafe { std::mem::zeroed() };
    let mut buf = vec![0 as libc::c_char; 16384];
    let mut out: *mut libc::passwd = std::ptr::null_mut();
    // SAFETY: every pointer is valid for the call; `buf` outlives the read of `pw_dir`.
    let rc = unsafe {
        libc::getpwuid_r(
            libc::getuid(),
            &mut pwd,
            buf.as_mut_ptr(),
            buf.len(),
            &mut out,
        )
    };
    if rc != 0 || out.is_null() || pwd.pw_dir.is_null() {
        return None;
    }
    // SAFETY: a NUL-terminated string inside `buf`.
    Some(
        unsafe { std::ffi::CStr::from_ptr(pwd.pw_dir) }
            .to_string_lossy()
            .into_owned(),
    )
}

/// `_expected_trash`: ~/.Trash on the home's volume, else <volume>/.Trashes/<uid>.
fn expected_trash(near: &str) -> Option<String> {
    let dev = sys::stat(near).ok()?.dev;
    let home = owner_home()?;
    if sys::stat(&home).ok()?.dev == dev {
        return Some(pp::join(&home, ".Trash"));
    }
    let mut top = pp::norm(&pp::realpath(near));
    while top != pp::parent(&top) && sys::stat(&pp::parent(&top)).ok()?.dev == dev {
        top = pp::parent(&top);
    }
    // SAFETY: getuid never fails.
    let uid = unsafe { libc::getuid() };
    Some(pp::join_all(&top, [".Trashes", uid.to_string().as_str()]))
}

/// `trash_folder`: the Trash macOS keeps for the volume holding `near`; a missing one is made by macOS only after FERB
/// checked that no link stands at it or above it. The point `trash.folder` comes first (a test's `before_move`).
pub fn trash_folder(near: &str, locator: &dyn TrashLocator) -> Result<String, TrashRefused> {
    hooks::at("trash.folder");
    let found = locator.trash_url(near, false);
    if let Some(f) = &found {
        if pp::is_dir(f) && !pp::is_symlink(f) {
            return Ok(pp::norm(f));
        }
    }
    let Some(expected) = found.map(|f| pp::norm(&f)).or_else(|| expected_trash(near)) else {
        return Err(TrashRefused("macOS named no Trash for it".into()));
    };
    if pp::lexists(&expected) {
        return Err(TrashRefused(format!(
            "a link or a file stands where the Trash should be ({expected})"
        )));
    }
    if pp::realpath(&pp::parent(&expected)) != pp::parent(&expected) {
        return Err(TrashRefused(format!(
            "a link stands above where the Trash should be ({expected})"
        )));
    }
    let made = locator.trash_url(near, true).map(|m| pp::norm(&m));
    if made.as_deref() != Some(expected.as_str()) {
        return Err(TrashRefused(format!(
            "macOS made the Trash at {}, not at {expected}",
            made.as_deref().unwrap_or("None")
        )));
    }
    Ok(expected)
}

/// What a move to the Trash did (files.py `Trashed`). `trashed_to` None: nothing moved (`error` says why). `error`
/// None: what FERB checked is in the Trash at `trashed_to` with that device and inode. Both: an item moved but isn't
/// (or can't be shown to be) what FERB checked; it stays in the Trash for the owner.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Trashed {
    pub where_: Option<String>,
    pub trashed_to: Option<String>,
    pub error: Option<String>,
    pub device: Option<i64>,
    pub inode: Option<u64>,
}

fn trashed(where_: Option<String>, error: String) -> Trashed {
    Trashed {
        where_,
        error: Some(error),
        ..Default::default()
    }
}

/// `trash_item`: `name` in the open `folder` to the Trash of its volume, between two folders FERB holds open, each
/// checked as opened; one move, never replacing anything in the Trash, and nothing ever moved out of it again.
pub fn trash_item(
    folder: RawFd,
    name: &str,
    closed: AccessCheck,
    access: &str,
    env: &FsEnv,
) -> Trashed {
    let looked = opened_path(folder).and_then(|w| {
        sys::stat_at(At::Dir(folder), name, false).map(|st| (pp::join(&w, name), st))
    });
    let (where_, checked) = match looked {
        Ok(x) => x,
        Err(e) => {
            return trashed(
                None,
                format!(
                    "FERB couldn't tell what it would move ({}); nothing was moved",
                    e.strerror_or_str()
                ),
            )
        }
    };
    if let Some(closed) = closed {
        if let Some(refusal) = closed(&where_, access).filter(|r| !r.is_empty()) {
            return trashed(Some(where_), refusal);
        }
    }
    let bin_path = match trash_folder(&pp::parent(&where_), env.trash.as_ref()) {
        Ok(b) => b,
        Err(TrashRefused(why)) => {
            return trashed(
                Some(where_.clone()),
                format!(
                    "couldn't move to Trash: no Trash for {} ({why}); nothing was moved",
                    pp::parent(&where_)
                ),
            )
        }
    };
    let trash = match sys::open(
        At::Cwd,
        &bin_path,
        sys::O_SEARCH | sys::O_DIRECTORY | sys::O_NOFOLLOW | sys::O_CLOEXEC,
        0,
    ) {
        Ok(fd) => fd,
        Err(e) => {
            return trashed(
                Some(where_),
                format!(
                    "FERB couldn't open the Trash at {bin_path} ({}); nothing was moved",
                    e.strerror_or_str()
                ),
            )
        }
    };
    let (bin_where, bin_dev) =
        match opened_path(trash.raw()).and_then(|w| sys::fstat(trash.raw()).map(|s| (w, s.dev))) {
            Ok(x) => x,
            Err(e) => {
                return trashed(
                    Some(where_),
                    format!(
                        "FERB couldn't tell which Trash it opened ({}); nothing was moved",
                        e.strerror_or_str()
                    ),
                )
            }
        };
    if bin_dev != checked.dev {
        return trashed(
            Some(where_.clone()),
            format!(
                "{where_} is on a different disk from its Trash ({bin_where}); FERB moves an item to the Trash only \
                 on its own disk, so nothing was moved"
            ),
        );
    }
    if !in_trash(&bin_where) {
        return trashed(
            Some(where_),
            format!("{bin_where} isn't a Trash folder, so nothing was moved"),
        );
    }
    let landed = match move_into(folder, name, trash.raw(), &bin_where, closed) {
        Ok(l) => l,
        Err(Fail::Refused(m)) => return trashed(Some(where_), m),
        Err(Fail::Os(e)) => {
            return trashed(
                Some(where_.clone()),
                format!(
                    "couldn't move {where_} to the Trash ({}); nothing was moved",
                    e.strerror_or_str()
                ),
            )
        }
        Err(other) => return trashed(Some(where_), format!("{other:?}")),
    };
    let trashed_to = pp::join(&bin_where, &landed);
    let moved = match sys::stat_at(At::Dir(trash.raw()), &landed, false) {
        Ok(st) => st,
        Err(e) => {
            return Trashed {
                where_: Some(where_.clone()),
                trashed_to: Some(trashed_to.clone()),
                error: Some(format!(
                    "FERB moved {where_} to the Trash at {trashed_to} but couldn't confirm it was what it checked \
                     ({e}); it stays there for you to look at"
                )),
                ..Default::default()
            }
        }
    };
    if moved.id() != checked.id() {
        return Trashed {
            where_: Some(where_.clone()),
            trashed_to: Some(trashed_to.clone()),
            error: Some(format!(
                "{where_} changed while FERB moved it: what is in the Trash at {trashed_to} isn't what FERB \
                 checked, so FERB left it there for you to look at and moved nothing out"
            )),
            ..Default::default()
        };
    }
    Trashed {
        where_: Some(where_),
        trashed_to: Some(trashed_to),
        error: None,
        device: Some(moved.dev),
        inode: Some(moved.ino),
    }
}

/// `_move_into`: `name` into the open Trash under a name nothing there has (its own name first, as Finder does), each
/// name checked in the Trash as opened.
fn move_into(
    folder: RawFd,
    name: &str,
    trash: RawFd,
    bin_where: &str,
    closed: AccessCheck,
) -> Result<String, Fail> {
    let (stem, suffix) = pp::splitext(name);
    for attempt in 0..100 {
        let candidate = if attempt == 0 {
            name.to_string()
        } else {
            format!("{stem} {}{suffix}", token_hex4())
        };
        if let Some(closed) = closed {
            if let Some(refusal) =
                closed(&pp::join(bin_where, &candidate), "write").filter(|r| !r.is_empty())
            {
                return Err(Fail::Refused(refusal));
            }
        }
        match move_excl(folder, name, trash, &candidate) {
            Ok(()) => return Ok(candidate),
            Err(e) if e.is(libc::EEXIST) => continue,
            Err(e) => return Err(e.into()),
        }
    }
    Err(OsError::with_text(
        libc::EEXIST,
        &format!("no free name for {name} in the Trash"),
    )
    .into())
}
