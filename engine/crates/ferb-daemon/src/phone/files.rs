//! Files both ways from a paired phone (stage 3 S3b, kb T-1203): browse a folder (`phone.files`), list a folder's files
//! to fetch it whole (`phone.tree`), fetch one file (`phone.read`, a stream) and send files and folders to the Mac
//! (`phone.put_group`, `phone.put_begin`, `phone.put_chunk`, `phone.put_end`). A port of the phone lab's
//! `standin/phone.py` file calls and `standin/protection.py` (ferb-phone-link@57b3f34, read, not run), on FERB's own
//! protection lists.
//!
//! **One judge** (PHN-114, PHN-133): every path is judged by ferb-policy's `Protection::fs_rules`, the check the model's
//! file tools get (the gateway's `path_closed`), through the [`Judge`] seam:
//! - a rule code never lifts (`NEVER_LIFTED`: FERB's own state) is `file_not_allowed`, with or without a proof;
//! - FERB's never-read list (`read:<entry>`), the owner's additions to it (PHN-121) and the places PHN-131 and PHN-133
//!   add for the phone ([`PHONE_KEY_PLACES`], [`KEY_NAMES`]) are **key places**: each access needs its own fresh proof by
//!   the fingerprint-only key where the phone registered one, signed over `{challenge, method, params: {"path": <the
//!   path as sent>}}` with `method` the phone-facing `link.get` / `link.files` / `link.tree` (CONTRACT §11), asked
//!   before anything is opened or said about the place;
//! - a file with more than one name (a hard link) is a key place too (PHN-140), and refused outright when one of its
//!   other names is FERB's own state.
//!
//! **The daemon opens each file itself**: the path as sent is judged by every name it has now, then opened once
//! (`O_NONBLOCK`, so a pipe never blocks), and the file that opened is judged again by the kernel's own name for it
//! (`F_GETPATH`), so `..`, links, a link swapped between the check and the open, letter case, Unicode forms, the Data
//! volume's name (`/System/Volumes/Data`) and `/private/var` for `/var` all end at the same answer. Only ordinary files
//! are sent (never a device, a pipe or a socket); a folder (an app bundle or package too) is listed, never fetched as
//! one. A file is read from the descriptor that was judged, its size, time and identity pinned: a change while it is
//! sent stops it with `file_changed`.
//!
//! **Uploads** (PHN-127, PHN-139): only into the phone's upload folder ([`FileSeams::inbox`], judged as a write), begun
//! behind the unlock window and refused before their first byte when they would leave less than
//! [`FileLimits::upload_free_floor`] free; written as a hidden part file `0600` under a random id, marked as downloaded
//! (`com.apple.quarantine`, so a file that runs when opened asks first: rule 5), and given their name at the end without
//! overwriting anything; one that stops part way is kept [`FileLimits::upload_partial_keep_s`] for the same phone to go
//! on with; a removed phone's are deleted at once. A message's attachments are this phone's finished uploads, by id.
//!
//! **The heavy-fetch notice** (PHN-128, PHN-138): more than [`FileLimits::fetch_notice_files`] files or
//! [`FileLimits::fetch_notice_bytes`] within [`FileLimits::fetch_notice_window_s`] is said once per burst: a bus event
//! FERB.app shows, a journal row, an audit row and an event for the helper to put in the phone's chat. Never a cap.
//!
//! **The audit** (PHN-114): every browse, fetch and upload is one row with its path and bytes, never a file's content.

use std::collections::{HashMap, VecDeque};
use std::ffi::{CStr, CString};
use std::fs::File;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::fs::{DirBuilderExt, FileExt, MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use ferb_policy::protection::NEVER_LIFTED;
use ferb_policy::{norm, ppath, py};
use ferb_rpc::phone_gate::{self, PhoneSession, Refusal};
use ferb_rpc::server::{BoxFuture, EventStream, HandlerError, HandlerResult};
use ferb_rpc::{wire, Params, Server, Session};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

use super::{lock, proof, refused, Inner, Phones};

/// The places PHN-131 adds to the fresh-fingerprint list (the owner: "yes i agree"): chat apps' stores, Notes and the
/// phone backups, as the lab listed them (`standin/protection.py` `OWNER_ADDED`); then shell history (PHN-133). FERB's
/// own never-read list comes from ferb-policy through the [`Judge`] and is not repeated here.
// decision: PHN-131
// decision: PHN-133
pub const PHONE_KEY_PLACES: &[&str] = &[
    "~/Library/Group Containers/group.net.whatsapp.WhatsApp.shared",
    "~/Library/Containers/net.whatsapp.WhatsApp",
    "~/Library/Group Containers/6N38VWS5BX.ru.keepcoder.Telegram",
    "~/Library/Application Support/Telegram Desktop",
    "~/Library/Application Support/Signal",
    "~/Library/Application Support/Slack",
    "~/Library/Containers/com.tinyspeck.slackmacgap",
    "~/Library/Application Support/discord",
    "~/Library/Group Containers/group.com.apple.notes",
    "~/Library/Containers/com.apple.Notes",
    "~/Library/Application Support/MobileSync",
    "~/.zsh_history",
    "~/.bash_history",
];

/// Key files by name anywhere (PHN-131: `.pem`, `.key`, `id_*`; PHN-133: dotenv files; the lab's `.netrc`, `.p12`,
/// `.pfx`): exact names, name prefixes, name suffixes, compared case- and Unicode-blind.
pub const KEY_NAMES: (&[&str], &[&str], &[&str]) = (
    &[".env", ".netrc"],
    &[".env.", "id_"],
    &[".pem", ".key", ".p12", ".pfx"],
);

/// What a key place's proof is signed as: the phone-facing method (CONTRACT §11).
const SIGNED_AS: &[(&str, &str)] = &[
    ("phone.read", "link.get"),
    ("phone.files", "link.files"),
    ("phone.tree", "link.tree"),
];
/// Three of the link's 16 KiB chunks per stream event (the lab's `READ_PIECE`; how the bridge cuts them is its own).
const READ_PIECE: usize = 48 * 1024;
/// How much of a file is hashed between two looks at whether the phone is still there.
const HASH_SLICE: u64 = 16 << 20;
const DATA_VOLUME: &str = "/System/Volumes/Data";
const VOLFS: &str = "/.vol";
const PART: &str = ".ferb-part";
/// The quarantine agent name (the lab's).
const QUARANTINE_AGENT: &str = "FERB phone link";
/// Deepest folder inside a folder upload (the lab's 32).
const REL_PARTS_MAX: usize = 32;

/// FERB's protection lists as the phone's file area reads them: the very lists the model's file tools are judged by.
pub trait Judge: Send + Sync {
    /// The owner's home the lists were built for (`~` in a path the phone sends): fixed when the lists were loaded,
    /// never this process's `HOME` at the time of a call.
    fn home(&self) -> String;
    /// FERB's own folder (`~/.ferb`): where a hard link's other names are looked for.
    fn ferb_home(&self) -> String;
    /// `Protection::fs_rules(path, access)`: every rule that closes `path` to FERB's own file tools.
    fn rules(&self, path: &str, access: &str) -> Vec<String>;
}

type FreeBytes = dyn Fn(&Path) -> io::Result<u64> + Send + Sync;
pub type TaskFile = dyn Fn(&str, u64) -> Option<PathBuf> + Send + Sync;
pub type MadeFiles = dyn Fn(usize) -> Vec<super::find::Made> + Send + Sync;
type Hook = dyn Fn(&str, &str) + Send + Sync;

/// What the file area takes from the daemon.
pub struct FileSeams {
    pub judge: Arc<dyn Judge>,
    /// Where a phone's uploads land (each folder upload one folder in it). Judged as a write: in a protected place,
    /// nothing is taken.
    pub inbox: PathBuf,
    /// The free bytes of the volume holding a folder (the system's `statvfs`; a test's own).
    pub free_bytes: Arc<FreeBytes>,
    /// The owner's own additions to the key places, beside FERB's never-read list (PHN-121: "the owner can extend the
    /// list on the Mac; a phone cannot read or shorten it").
    pub added_places: Vec<String>,
    /// A finished task's file by (task, index), for `phone.read {artifact}`; `None`: no task's files are offered.
    pub task_file: Option<Arc<TaskFile>>,
    /// The files FERB's tasks made, newest first, at most this many (`phone.recent`, T-1211); `None`: none offered.
    pub made_files: Option<Arc<MadeFiles>>,
    /// A test's look between the steps (`"before_open"`, `"piece"`) with the path: never set in the daemon.
    pub hook: Option<Arc<Hook>>,
}

impl FileSeams {
    /// The system's free space: `statvfs`'s blocks available to the owner times the fragment size.
    pub fn system_free_bytes() -> Arc<FreeBytes> {
        Arc::new(|p: &Path| {
            let c = cstr(p.as_os_str().as_encoded_bytes())?;
            // SAFETY: an all-zero `statvfs` is a valid value of this plain C struct; statvfs() overwrites it.
            let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
            // SAFETY: `c` is a valid NUL-terminated path that outlives the call; `st` is writable.
            if unsafe { libc::statvfs(c.as_ptr(), &mut st) } != 0 {
                return Err(io::Error::last_os_error());
            }
            #[allow(clippy::unnecessary_cast)]
            Ok(st.f_bavail as u64 * st.f_frsize as u64)
        })
    }
}

/// The file area's numbers.
#[derive(Clone, Debug, PartialEq)]
pub struct FileLimits {
    /// An upload that would leave less than this free on the Mac is refused before its first byte (PHN-127, PHN-139:
    /// "refuse when the Mac has under 1 GB free"; the lab's 1 GiB).
    pub upload_free_floor: u64,
    /// An upload that stopped part way is kept this long for the same phone to go on with (PHN-139: one hour).
    pub upload_partial_keep_s: f64,
    /// The heavy-fetch notice: more than this many files ... (PHN-138: 40)
    pub fetch_notice_files: u64,
    /// ... or more than these bytes ... (PHN-138: 1 GB, the lab's 1 GiB)
    pub fetch_notice_bytes: u64,
    /// ... within this window (PHN-138: 10 minutes).
    pub fetch_notice_window_s: f64,
    /// Entries in one folder listing (the lab's `list_max`, CONTRACT §7; no owner decision: asked in T-1203's report).
    pub list_max: usize,
    /// Files in one folder fetched whole (the lab's `tree_max`, CONTRACT §7; no owner decision: asked).
    pub tree_max: usize,
    /// Names one search of file names looks at before it stops and says so (`phone.search`, T-1211): a bound on its
    /// time, not on what it finds (no owner decision: asked with T-1217).
    pub search_names_max: usize,
    /// Names looked at in FERB's own folder for a hard link's other names before the answer is "closed" (the lab's
    /// `never_inode` budget).
    pub own_names_max: usize,
}

impl Default for FileLimits {
    fn default() -> Self {
        FileLimits {
            upload_free_floor: 1 << 30,
            upload_partial_keep_s: 3600.0,
            fetch_notice_files: 40,
            fetch_notice_bytes: 1 << 30,
            fetch_notice_window_s: 600.0,
            list_max: 500,
            tree_max: 2000,
            search_names_max: 100_000,
            own_names_max: 20_000,
        }
    }
}

/// One upload under way, paused or finished.
struct Upload {
    device: String,
    session: String,
    /// The folder it lands in (held open: the part is named and renamed in it, never by a path) and its real name.
    dir: Arc<OwnedFd>,
    dir_real: String,
    part: String,
    name: String,
    size: u64,
    got: u64,
    hasher: Sha256,
    file: Option<File>,
    ident: (u64, u64),
    /// When it was last written to or paused (monotonic).
    at: f64,
    saved: Option<String>,
}

struct Group {
    device: String,
    dir: Arc<OwnedFd>,
    real: String,
}

#[derive(Default)]
struct State {
    uploads: HashMap<String, Upload>,
    groups: HashMap<String, Group>,
    /// Per phone: each fetch (monotonic time, bytes) inside the window, and whether this burst was told.
    fetched: HashMap<String, (VecDeque<(f64, u64)>, bool)>,
    /// The part files a daemon before this one left were swept.
    swept: bool,
}

/// The file area: its seams, numbers and state.
pub(crate) struct Area {
    pub(super) seams: FileSeams,
    pub(super) limits: FileLimits,
    /// The added key places in every form a path to them can take (as written, resolved now, without the Data volume).
    places: Vec<Vec<String>>,
    state: std::sync::Mutex<State>,
}

impl Area {
    pub(crate) fn new(seams: FileSeams, limits: FileLimits) -> Area {
        let home = seams.judge.home();
        let places = PHONE_KEY_PLACES
            .iter()
            .map(|s| (*s).to_owned())
            .chain(seams.added_places.iter().cloned())
            .filter_map(|e| expand(&e, &home))
            .map(|p| forms(&p))
            .collect();
        Area {
            seams,
            limits,
            places,
            state: std::sync::Mutex::new(State::default()),
        }
    }

    fn hook(&self, stage: &str, path: &str) {
        if let Some(h) = &self.seams.hook {
            h(stage, path);
        }
    }

    /// Whether a path, in any of `forms`, is one of the places this area adds (PHN-131, PHN-133, the owner's own) or a
    /// key file by its name.
    fn added_place(&self, forms: &[String]) -> bool {
        let named = forms.iter().any(|f| {
            let name = fold(&ppath::name(f));
            KEY_NAMES.0.contains(&name.as_str())
                || KEY_NAMES.1.iter().any(|p| name.starts_with(p))
                || KEY_NAMES.2.iter().any(|s| name.ends_with(s))
        });
        named
            || self
                .places
                .iter()
                .any(|root| forms.iter().any(|f| root.iter().any(|r| under(f, r))))
    }

    /// The judgment of a path by its names: `Err` for FERB's own data, else whether it is a key place.
    pub(super) fn judged(&self, path: &str, more: &[String]) -> Result<bool, Refusal> {
        let rules = self.seams.judge.rules(path, ferb_policy::protection::READ);
        if rules.iter().any(|r| NEVER_LIFTED.contains(&r.as_str())) {
            return Err(own_data());
        }
        let mut all = forms(path);
        all.extend(more.iter().cloned());
        Ok(!rules.is_empty() || self.added_place(&all))
    }

    /// A key place by its name, for a listing's `locked` and a folder's walk: FERB's own data counts as closed too.
    pub(super) fn closed_or_key(&self, path: &str) -> (bool, bool) {
        match self.judged(path, &[]) {
            Err(_) => (true, false),
            Ok(key) => (false, key),
        }
    }

    /// Whether a file with more than one name is FERB's own state under another of them: a bounded walk of FERB's
    /// folder; when it cannot finish, the answer is yes (closed), as the lab's `never_inode`.
    fn own_data_by_identity(&self, dev: u64, ino: u64) -> bool {
        let root = self.seams.judge.ferb_home();
        let mut seen = 0usize;
        let mut stack = vec![PathBuf::from(&root)];
        while let Some(dir) = stack.pop() {
            let Ok(rd) = std::fs::read_dir(&dir) else {
                continue;
            };
            for e in rd.flatten() {
                seen += 1;
                if seen > self.limits.own_names_max {
                    return true;
                }
                let Ok(m) = std::fs::symlink_metadata(e.path()) else {
                    continue;
                };
                if m.is_dir() {
                    stack.push(e.path());
                } else if m.dev() == dev && m.ino() == ino {
                    let p = e.path().to_string_lossy().into_owned();
                    let rules = self.seams.judge.rules(&p, ferb_policy::protection::READ);
                    if rules.iter().any(|r| NEVER_LIFTED.contains(&r.as_str())) {
                        return true;
                    }
                }
            }
        }
        false
    }
}

// ---- small path facts ----------------------------------------------------------------------------------------------

/// A path as a Mac volume compares it: case and Unicode form ignored (protection.py `_key`).
pub(super) fn fold(s: &str) -> String {
    py::casefold(&norm::nfc(s))
}

fn key(p: &str) -> Vec<String> {
    ppath::parts(p).iter().map(|x| fold(x)).collect()
}

pub(super) fn under(p: &str, root: &str) -> bool {
    let (a, b) = (key(p), key(root));
    a.len() >= b.len() && a[..b.len()] == b[..]
}

fn firmlinked(p: &str) -> Option<String> {
    p.strip_prefix(DATA_VOLUME)
        .filter(|rest| rest.starts_with('/'))
        .map(str::to_owned)
}

/// A path resolved by name now: the whole path, else its folder and its name, else as written.
fn resolved(p: &str) -> String {
    if let Ok(r) = std::fs::canonicalize(p) {
        return r.to_string_lossy().into_owned();
    }
    let parent = ppath::parent(p);
    match std::fs::canonicalize(&parent) {
        Ok(r) if parent != p => ppath::div(&r.to_string_lossy(), &ppath::name(p)),
        _ => p.to_owned(),
    }
}

/// A path as typed (cleaned), as it resolves by name now, and each without the Data volume's prefix.
pub(super) fn forms(p: &str) -> Vec<String> {
    let typed = ppath::clean(&ppath::normpath(p));
    let real = resolved(&typed);
    let mut out = vec![typed.clone(), real.clone()];
    out.extend(firmlinked(&typed));
    out.extend(firmlinked(&real));
    let mut seen = Vec::new();
    for f in out {
        if !seen.contains(&f) {
            seen.push(f);
        }
    }
    seen
}

/// `~`, `~/x` by the lists' home; any other `~name` and anything not absolute afterwards: `None`.
pub(super) fn expand(path: &str, home: &str) -> Option<String> {
    if path.is_empty() || path.contains('\0') {
        return None;
    }
    let p = if path == "~" {
        home.to_owned()
    } else if let Some(rest) = path.strip_prefix("~/") {
        format!("{}/{rest}", home.trim_end_matches('/'))
    } else {
        path.to_owned()
    };
    p.starts_with('/').then_some(p)
}

/// The lab's `human`.
fn human(n: u64) -> String {
    const G: u64 = 1 << 30;
    const M: u64 = 1 << 20;
    if n >= G {
        format!("{:.1} GB", n as f64 / G as f64)
    } else if n >= M {
        format!("{:.1} MB", n as f64 / M as f64)
    } else {
        format!("{} kB", n / 1024)
    }
}

/// The lab's `clean_name`: letters, digits and `.-_ ` only, 80 at most, trimmed, no leading or trailing dots.
fn clean_name(name: &str, fallback: &str) -> String {
    let kept: String = name
        .chars()
        .filter(|c| c.is_alphanumeric() || ".-_ ".contains(*c))
        .take(80)
        .collect();
    let out = kept.trim().trim_matches('.').to_owned();
    if out.is_empty() {
        fallback.to_owned()
    } else {
        out
    }
}

fn cstr(b: &[u8]) -> io::Result<CString> {
    CString::new(b).map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "a NUL in the name"))
}

// ---- refusals ------------------------------------------------------------------------------------------------------

pub(super) fn own_data() -> Refusal {
    refused(
        phone_gate::FILE_NOT_ALLOWED,
        "That is FERB's own data, which a phone never opens, with or without a fingerprint.",
    )
}

fn by_number() -> Refusal {
    refused(
        phone_gate::FILE_NOT_ALLOWED,
        "FERB opens a file by its path, never by its number on the disk (/.vol).",
    )
}

pub(super) fn bad(why: &str) -> Refusal {
    refused(phone_gate::BAD_REQUEST, why)
}

pub(super) fn coded(r: Refusal) -> HandlerError {
    HandlerError::coded(r.code, r.message)
}

fn off() -> Refusal {
    refused(wire::FAILED, "FERB's file area for phones isn't running.")
}

fn io_refusal(e: &io::Error, folder: bool) -> Refusal {
    match e.raw_os_error() {
        Some(libc::ENOENT) => refused(phone_gate::NOT_FOUND, "No such file or folder."),
        Some(libc::ENOTDIR) if folder => {
            refused(phone_gate::NOT_A_FILE, "That is a file, not a folder.")
        }
        Some(libc::ENOTDIR) => refused(phone_gate::NOT_FOUND, "No such file or folder."),
        Some(libc::EACCES) | Some(libc::EPERM) => refused(
            wire::FAILED,
            "macOS does not let FERB open that (permission denied).",
        ),
        _ => refused(wire::FAILED, format!("That could not be opened ({e}).")),
    }
}

// ---- descriptors ---------------------------------------------------------------------------------------------------

/// The kernel's own name for an open file or folder: every link followed, the volume's spelling (`F_GETPATH`).
#[cfg(target_os = "macos")]
fn kernel_path(fd: RawFd) -> io::Result<String> {
    let mut buf = vec![0u8; libc::PATH_MAX as usize];
    // SAFETY: F_GETPATH writes at most PATH_MAX bytes (NUL included) into `buf`, which has that many.
    if unsafe { libc::fcntl(fd, libc::F_GETPATH, buf.as_mut_ptr()) } < 0 {
        return Err(io::Error::last_os_error());
    }
    let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
    String::from_utf8(buf[..end].to_vec())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "a kernel path that isn't UTF-8"))
}

#[cfg(not(target_os = "macos"))]
fn kernel_path(_fd: RawFd) -> io::Result<String> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "F_GETPATH is macOS's",
    ))
}

pub(super) fn stat_at(dir: RawFd, name: &[u8]) -> io::Result<libc::stat> {
    let c = cstr(name)?;
    // SAFETY: an all-zero `stat` is a valid value of this plain C struct; fstatat() overwrites it.
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    // SAFETY: `dir` is an open descriptor, `c` a NUL-terminated name and `st` writable.
    if unsafe { libc::fstatat(dir, c.as_ptr(), &mut st, libc::AT_SYMLINK_NOFOLLOW) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(st)
}

pub(super) fn open_at(
    dir: RawFd,
    name: &[u8],
    flags: libc::c_int,
    mode: libc::mode_t,
) -> io::Result<File> {
    let c = cstr(name)?;
    // SAFETY: `dir` is an open descriptor and `c` a NUL-terminated name; the mode is read only with O_CREAT.
    let fd = unsafe {
        libc::openat(
            dir,
            c.as_ptr(),
            flags | libc::O_CLOEXEC,
            mode as libc::c_uint,
        )
    };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `fd` was just returned by openat() and is owned by nothing else.
    Ok(unsafe { File::from_raw_fd(fd) })
}

/// A folder inside `dir`, made when missing, opened without following a link.
fn folder_at(dir: RawFd, name: &str) -> io::Result<OwnedFd> {
    let c = cstr(name.as_bytes())?;
    // SAFETY: `dir` is an open descriptor and `c` a NUL-terminated name.
    if unsafe { libc::mkdirat(dir, c.as_ptr(), 0o700) } != 0 {
        let e = io::Error::last_os_error();
        if e.raw_os_error() != Some(libc::EEXIST) {
            return Err(e);
        }
    }
    open_at(
        dir,
        name.as_bytes(),
        libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW,
        0,
    )
    .map(OwnedFd::from)
}

fn unlink_at(dir: RawFd, name: &str) {
    if let Ok(c) = cstr(name.as_bytes()) {
        // SAFETY: `dir` is an open descriptor and `c` a NUL-terminated name.
        unsafe {
            libc::unlinkat(dir, c.as_ptr(), 0);
        }
    }
}

/// `from` takes the name `to` in `dir`, never over a file already there (a link fails then).
fn name_at(dir: RawFd, from: &str, to: &str) -> io::Result<()> {
    let (a, b) = (cstr(from.as_bytes())?, cstr(to.as_bytes())?);
    // SAFETY: `dir` is an open descriptor; `a` and `b` are NUL-terminated names.
    if unsafe { libc::linkat(dir, a.as_ptr(), dir, b.as_ptr(), 0) } != 0 {
        return Err(io::Error::last_os_error());
    }
    unlink_at(dir, from);
    Ok(())
}

/// The names in an open folder, read from that very folder (a duplicate of its descriptor).
pub(super) fn names_in(dir: RawFd) -> io::Result<Vec<Vec<u8>>> {
    // SAFETY: duplicating an open descriptor.
    let fd = unsafe { libc::dup(dir) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `fd` is a fresh descriptor owned here; fdopendir takes it over (closedir closes it).
    let d = unsafe { libc::fdopendir(fd) };
    if d.is_null() {
        let e = io::Error::last_os_error();
        // SAFETY: fdopendir failed, so `fd` is still ours to close.
        unsafe { libc::close(fd) };
        return Err(e);
    }
    // SAFETY: `d` is a valid DIR*; the duplicate shares the offset, so start from the top.
    unsafe { libc::rewinddir(d) };
    let mut out = Vec::new();
    loop {
        // SAFETY: `d` is a valid DIR*; readdir returns null at the end, else an entry valid until the next call.
        let ent = unsafe { libc::readdir(d) };
        if ent.is_null() {
            break;
        }
        // SAFETY: `d_name` is a NUL-terminated array inside the entry just returned.
        let name = unsafe { CStr::from_ptr((*ent).d_name.as_ptr()) }
            .to_bytes()
            .to_vec();
        if name != b"." && name != b".." {
            out.push(name);
        }
    }
    // SAFETY: `d` came from fdopendir above and is closed once.
    unsafe { libc::closedir(d) };
    Ok(out)
}

pub(super) fn kind(st: &libc::stat) -> libc::mode_t {
    st.st_mode & libc::S_IFMT
}

/// Marks an upload as downloaded, from the phone link: macOS asks before a file that runs when opened runs.
#[cfg(target_os = "macos")]
fn quarantine(f: &File, epoch: i64) -> io::Result<()> {
    let value = format!("0081;{:08x};{QUARANTINE_AGENT};", epoch.max(0));
    let name = c"com.apple.quarantine";
    // SAFETY: `f` is an open descriptor; `name` is NUL-terminated and `value` is valid for its length.
    let rc = unsafe {
        libc::fsetxattr(
            f.as_raw_fd(),
            name.as_ptr(),
            value.as_ptr().cast(),
            value.len(),
            0,
            0,
        )
    };
    if rc != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn quarantine(_f: &File, _epoch: i64) -> io::Result<()> {
    let _ = QUARANTINE_AGENT;
    Ok(())
}

// ---- the area's state on the phone area ----------------------------------------------------------------------------

pub(super) fn area(inner: &Inner) -> Result<&Area, Refusal> {
    inner.files.get().ok_or_else(off)
}

/// The file transfers the public relay never carries (kb PHN-444): a file to the phone, a folder's file list for a
/// whole-folder fetch, and the start of a file or folder from it. The helper refuses each before the daemon is asked;
/// the daemon refuses them too, on the tier the helper names for the call (S4), so a helper that failed to would not
/// send a file over the shared relay. A listing (`phone.files`) is the helper's to judge by its size.
pub(crate) fn shared_relay_refusal(phone: &PhoneSession, method: &str) -> Option<Refusal> {
    // (and the live preview's frames, PHN-454: never on the public relay)
    const BULK: [&str; 5] = [
        "phone.read",
        "phone.tree",
        "phone.put_begin",
        "phone.put_group",
        "tasks.preview",
    ];
    (BULK.contains(&method) && phone.tier.as_deref() == Some("shared_relay")).then(|| {
        refused(
            phone_gate::FILE_NOT_ALLOWED,
            "files cross only a direct connection or your own relay, not the shared relay",
        )
    })
}

pub(super) fn phone_of(session: &Session) -> PhoneSession {
    session.phone.clone().unwrap_or_default()
}

/// A file or folder opened for a phone and judged as it opened.
pub(super) struct Opened {
    pub(super) file: File,
    pub(super) real: String,
    pub(super) high: bool,
    pub(super) meta: std::fs::Metadata,
}

/// Opens `path` (as the phone sent it) for this phone: the order of the lab's `Places.open`. FERB's own data is refused
/// whatever is offered; a key place asks for its proof before anything is opened or said; then the file is opened once
/// and the file that really opened is judged again.
pub(super) fn open_for_phone(
    inner: &Inner,
    a: &Area,
    phone: &PhoneSession,
    method: &str,
    sent: &str,
    folder: bool,
) -> Result<Opened, Refusal> {
    let home = a.seams.judge.home();
    let typed = expand(sent, &home)
        .ok_or_else(|| bad("A file is named by its whole path (or one starting with ~/)."))?;
    if forms(&typed).iter().any(|f| under(f, VOLFS)) {
        return Err(by_number());
    }
    let mut proven = false;
    if a.judged(&typed, &[])? {
        let name = ppath::name(&ppath::clean(&ppath::normpath(&typed)));
        fresh(inner, phone, method, sent, folder, &name, &mut proven)?;
    }
    a.hook("before_open", &typed);
    // a device, a pipe or a socket is refused before it is opened (an open can have effects); the open's own check
    // below catches one swapped in meanwhile (O_NONBLOCK: a pipe never blocks the daemon)
    if let Ok(m) = std::fs::metadata(&typed) {
        if folder && !m.is_dir() {
            return Err(refused(
                phone_gate::NOT_A_FILE,
                "That is a file, not a folder.",
            ));
        }
        if !folder && !m.is_file() {
            return Err(not_a_file(m.is_dir()));
        }
    }
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK | if folder { libc::O_DIRECTORY } else { 0 })
        .open(&typed)
        .map_err(|e| io_refusal(&e, folder))?;
    let meta = file
        .metadata()
        .map_err(|e| refused(wire::FAILED, format!("That could not be read ({e}).")))?;
    if folder && !meta.is_dir() {
        return Err(refused(
            phone_gate::NOT_A_FILE,
            "That is a file, not a folder.",
        ));
    }
    if !folder && !meta.is_file() {
        return Err(not_a_file(meta.is_dir()));
    }
    let real = kernel_path(file.as_raw_fd()).map_err(|_| {
        refused(
            phone_gate::FILE_NOT_ALLOWED,
            "FERB could not tell which file it opened, so it is not sent.",
        )
    })?;
    if under(&real, VOLFS) {
        return Err(by_number());
    }
    let mut also = vec![real.clone()];
    also.extend(firmlinked(&real));
    let key = a.judged(&real, &also)?;
    // a file with more than one name: where else it lives cannot be known from this one (PHN-140)
    let many = !folder && meta.nlink() > 1;
    if many && a.own_data_by_identity(meta.dev(), meta.ino()) {
        return Err(own_data());
    }
    let high = proven || key || many;
    if high {
        fresh(
            inner,
            phone,
            method,
            sent,
            folder,
            &ppath::name(&real),
            &mut proven,
        )?;
    }
    Ok(Opened {
        file,
        real,
        high,
        meta,
    })
}

/// A key place's own fresh proof for this very call (PHN-121): by the fingerprint-only key where the phone registered
/// one (PHN-129), signed over the phone-facing method and only the path as sent, with a challenge of this session, used
/// up. Asked at most once per call.
// decision: PHN-121
// decision: PHN-129
fn fresh(
    inner: &Inner,
    phone: &PhoneSession,
    method: &str,
    sent: &str,
    folder: bool,
    name: &str,
    proven: &mut bool,
) -> Result<(), Refusal> {
    if *proven {
        return Ok(());
    }
    let name = if name.is_empty() { "This place" } else { name };
    if phone.proof.is_none() {
        let what = if folder { "folder" } else { "file" };
        return Err(refused(
            phone_gate::FRESH_UNLOCK_REQUIRED,
            format!(
                "{name} holds keys, passwords or private data, so it needs your fingerprint (the PIN on a phone \
                 without one) each time, for this very {what}."
            ),
        ));
    }
    let dev = inner.live(&phone.device)?;
    let signed_as = SIGNED_AS
        .iter()
        .find(|(m, _)| *m == method)
        .map_or("link.get", |(_, s)| *s);
    lock(&inner.state).verify_call(inner, &dev, phone, signed_as, &json!({"path": sent}), true)?;
    *proven = true;
    Ok(())
}

fn not_a_file(dir: bool) -> Refusal {
    refused(
        phone_gate::NOT_A_FILE,
        if dir {
            "Only one file at a time can be opened on the phone, never a folder."
        } else {
            "That is not an ordinary file (a device, a pipe or a socket), so it is not sent."
        },
    )
}

pub(super) fn path_param(params: &Params, folder: bool) -> Result<String, Refusal> {
    match params.get("path") {
        Some(Value::String(s)) if !s.is_empty() => Ok(s.clone()),
        None | Some(Value::Null) if folder => Ok("~".into()),
        _ => Err(bad(
            "A file is named by its whole path (or one starting with ~/).",
        )),
    }
}

pub(super) fn audit_refusal(
    inner: &Inner,
    device: &str,
    method: &str,
    class: &str,
    r: &Refusal,
    path: Value,
) {
    inner.audit(
        device,
        method,
        class,
        &super::gate::code_name(r.code),
        json!({"path": path}),
    );
}

// ---- browse ----------------------------------------------------------------------------------------------------------

/// `phone.files {path?}`: one folder's entries (name, dir, link, size, modified, locked), at most `list_max`.
fn files(inner: &Inner, session: &Session, params: &Params) -> HandlerResult {
    let phone = phone_of(session);
    let out = (|| {
        let a = area(inner)?;
        let sent = path_param(params, true)?;
        let o = open_for_phone(inner, a, &phone, "phone.files", &sent, true)?;
        let names = names_in(o.file.as_raw_fd()).map_err(|e| {
            refused(
                wire::FAILED,
                format!("That folder could not be read ({e})."),
            )
        })?;
        let (limit, mut total) = (a.limits.list_max, 0usize);
        let mut entries: Vec<(bool, String, Value)> = Vec::new();
        for raw in names {
            total += 1;
            if entries.len() >= limit {
                continue;
            }
            let Ok(st) = stat_at(o.file.as_raw_fd(), &raw) else {
                continue;
            };
            let name = String::from_utf8_lossy(&raw).into_owned();
            let (is_dir, is_file, is_link) = (
                kind(&st) == libc::S_IFDIR,
                kind(&st) == libc::S_IFREG,
                kind(&st) == libc::S_IFLNK,
            );
            let child = ppath::div(&o.real, &name);
            let (_, key) = a.closed_or_key(&child);
            #[allow(clippy::unnecessary_cast)]
            let locked = key || (is_file && st.st_nlink as u64 > 1);
            #[allow(clippy::unnecessary_cast)]
            let size = if is_file { st.st_size as u64 } else { 0 };
            entries.push((
                is_dir,
                name.clone(),
                json!({"name": name, "locked": locked, "dir": is_dir, "link": is_link, "size": size,
                       "modified": st.st_mtime}),
            ));
        }
        entries.sort_by_key(|x| (!x.0, x.1.to_lowercase()));
        let parent = ppath::parent(&o.real);
        let mut out = Params::new();
        out.insert("path".into(), json!(o.real));
        out.insert(
            "parent".into(),
            if parent == o.real {
                Value::Null
            } else {
                json!(parent)
            },
        );
        out.insert("high_value".into(), json!(o.high));
        out.insert(
            "entries".into(),
            Value::Array(entries.into_iter().map(|e| e.2).collect()),
        );
        out.insert("total".into(), json!(total));
        out.insert("cut".into(), json!(total > limit));
        Ok(out)
    })();
    match &out {
        Ok(o) => inner.audit(
            &phone.device,
            "phone.files",
            "private",
            "ok",
            json!({"path": o["path"], "entries": o["total"]}),
        ),
        Err(r) => audit_refusal(
            inner,
            &phone.device,
            "phone.files",
            "private",
            r,
            params.get("path").cloned().unwrap_or(json!("~")),
        ),
    }
    out.map_err(coded)
}

/// `phone.tree {path?}`: every file under a folder (relative paths and sizes, never contents), at most `tree_max`, for
/// fetching it whole. FERB's own data, and (in an ordinary folder) key places and hard-linked files, are left out and
/// counted: each of those asks by itself when fetched.
fn tree(inner: &Inner, session: &Session, params: &Params) -> HandlerResult {
    let phone = phone_of(session);
    let out = (|| {
        let a = area(inner)?;
        let sent = path_param(params, true)?;
        let o = open_for_phone(inner, a, &phone, "phone.tree", &sent, true)?;
        let mut walk = Walk {
            a,
            high: o.high,
            files: Vec::new(),
            skipped: 0,
            bytes: 0,
            cut: false,
        };
        let fd = OwnedFd::from(o.file);
        walk.folder(&fd, &o.real, "");
        let mut out = Params::new();
        out.insert("path".into(), json!(o.real));
        out.insert("name".into(), json!(ppath::name(&o.real)));
        out.insert("files".into(), Value::Array(walk.files));
        out.insert("bytes".into(), json!(walk.bytes));
        out.insert("skipped".into(), json!(walk.skipped));
        out.insert("cut".into(), json!(walk.cut));
        out.insert("high_value".into(), json!(o.high));
        Ok(out)
    })();
    match &out {
        Ok(o) => inner.audit(
            &phone.device,
            "phone.tree",
            "private",
            "ok",
            json!({"path": o["path"], "files": o["files"].as_array().map_or(0, Vec::len), "bytes": o["bytes"]}),
        ),
        Err(r) => audit_refusal(
            inner,
            &phone.device,
            "phone.tree",
            "private",
            r,
            params.get("path").cloned().unwrap_or(json!("~")),
        ),
    }
    out.map_err(coded)
}

struct Walk<'a> {
    a: &'a Area,
    high: bool,
    files: Vec<Value>,
    skipped: u64,
    bytes: u64,
    cut: bool,
}

impl Walk<'_> {
    /// The lab's `os.walk(followlinks=False)` order: a folder's files (by name), then its folders (by name), each
    /// opened from its parent's descriptor without following a link.
    fn folder(&mut self, dir: &OwnedFd, real: &str, rel: &str) {
        let Ok(mut names) = names_in(dir.as_raw_fd()) else {
            return;
        };
        names.sort();
        let mut dirs = Vec::new();
        for raw in names {
            if self.cut {
                return;
            }
            let Ok(st) = stat_at(dir.as_raw_fd(), &raw) else {
                continue;
            };
            let name = String::from_utf8_lossy(&raw).into_owned();
            let child = ppath::div(real, &name);
            let (closed, key) = self.a.closed_or_key(&child);
            match kind(&st) {
                libc::S_IFDIR => {
                    if closed || (!self.high && key) {
                        self.skipped += 1;
                    } else {
                        dirs.push((raw, name));
                    }
                }
                libc::S_IFREG => {
                    #[allow(clippy::unnecessary_cast)]
                    let many = st.st_nlink as u64 > 1;
                    if closed || (!self.high && (key || many)) {
                        self.skipped += 1;
                        continue;
                    }
                    if self.files.len() >= self.a.limits.tree_max {
                        self.cut = true;
                        return;
                    }
                    #[allow(clippy::unnecessary_cast)]
                    let size = st.st_size as u64;
                    let r = if rel.is_empty() {
                        name
                    } else {
                        format!("{rel}/{name}")
                    };
                    self.files.push(json!({"rel": r, "size": size}));
                    self.bytes += size;
                }
                _ => {} // links, pipes, devices: not files of the folder
            }
        }
        for (raw, name) in dirs {
            if self.cut {
                return;
            }
            let Ok(sub) = open_at(
                dir.as_raw_fd(),
                &raw,
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_NONBLOCK,
                0,
            ) else {
                continue;
            };
            let r = if rel.is_empty() {
                name.clone()
            } else {
                format!("{rel}/{name}")
            };
            self.folder(&OwnedFd::from(sub), &ppath::div(real, &name), &r);
        }
    }
}

// ---- fetch -----------------------------------------------------------------------------------------------------------

/// What a file was when it was opened: a change while it is sent stops the transfer.
#[derive(Clone, Copy, PartialEq, Eq)]
struct Pin {
    dev: u64,
    ino: u64,
    size: u64,
    mtime: (i64, i64),
}

impl Pin {
    fn of(m: &std::fs::Metadata) -> Pin {
        Pin {
            dev: m.dev(),
            ino: m.ino(),
            size: m.size(),
            mtime: (m.mtime(), m.mtime_nsec()),
        }
    }
}

enum Stage {
    Meta,
    Sha,
    Pieces,
    Done,
}

/// `phone.read {path, offset?}` or `{artifact: {task_id, index}}`: `{meta}`, `{sha256}`, then `{d: base64}` pieces of
/// the file from `offset` on, read from the one descriptor that was opened and judged.
struct Read {
    inner: Arc<Inner>,
    device: String,
    /// The refusal to give first (nothing was opened), or the file.
    refusal: Option<Refusal>,
    file: Option<Arc<File>>,
    real: String,
    high: bool,
    pin: Option<Pin>,
    offset: u64,
    at: u64,
    stage: Stage,
    sent: u64,
    ended: bool,
}

fn read(inner: Arc<Inner>, session: &Session, params: &Params) -> Read {
    let phone = phone_of(session);
    let mut r = Read {
        inner: inner.clone(),
        device: phone.device.clone(),
        refusal: None,
        file: None,
        real: String::new(),
        high: false,
        pin: None,
        offset: 0,
        at: 0,
        stage: Stage::Meta,
        sent: 0,
        ended: false,
    };
    let sent = params.get("path").cloned().unwrap_or(Value::Null);
    let opened = (|| {
        let a = area(&inner)?;
        let path = if let Some(art) = params.get("artifact").filter(|v| v.is_object()) {
            // a finished task's own file: judged as any path (it may lie anywhere), never trusted for being FERB's
            let task = art.get("task_id").and_then(Value::as_str).unwrap_or("");
            let index = art.get("index").and_then(Value::as_u64).unwrap_or(0);
            let found = a.seams.task_file.as_ref().and_then(|f| f(task, index));
            found
                .ok_or_else(|| refused(phone_gate::NOT_FOUND, "FERB has no such file."))?
                .to_string_lossy()
                .into_owned()
        } else {
            path_param(params, false)?
        };
        open_for_phone(&inner, a, &phone, "phone.read", &path, false)
    })();
    match opened {
        Err(e) => {
            audit_refusal(&inner, &phone.device, "phone.read", "private", &e, sent);
            r.refusal = Some(e);
            r.ended = true;
        }
        Ok(o) => {
            let pin = Pin::of(&o.meta);
            r.offset = params
                .get("offset")
                .and_then(Value::as_u64)
                .unwrap_or(0)
                .min(pin.size);
            r.at = r.offset;
            r.pin = Some(pin);
            r.real = o.real;
            r.high = o.high;
            r.file = Some(Arc::new(o.file));
        }
    }
    r
}

impl Read {
    fn area(&self) -> Option<&Area> {
        self.inner.files.get()
    }

    /// Still this phone's (a removed phone stops at once), and the file still as it was opened.
    fn still(&self) -> Result<(), Refusal> {
        self.inner.live(&self.device)?;
        let (Some(f), Some(pin)) = (&self.file, self.pin) else {
            return Ok(());
        };
        let now = f.metadata().map(|m| Pin::of(&m)).ok();
        if now != Some(pin) {
            return Err(refused(
                phone_gate::FILE_CHANGED,
                "The file changed while it was being sent, so FERB stopped; fetch it again for the new version.",
            ));
        }
        Ok(())
    }

    fn fail(&mut self, r: Refusal) -> Option<HandlerResult> {
        self.ended = true;
        self.inner.audit(
            &self.device,
            "phone.read",
            "private",
            &super::gate::code_name(r.code),
            json!({"path": self.real, "bytes": self.sent}),
        );
        Some(Err(coded(r)))
    }

    async fn step(&mut self) -> Option<HandlerResult> {
        if let Some(r) = self.refusal.take() {
            return Some(Err(coded(r)));
        }
        if self.ended {
            return None;
        }
        let (Some(file), Some(pin)) = (self.file.clone(), self.pin) else {
            return None;
        };
        match self.stage {
            Stage::Meta => {
                self.stage = Stage::Sha;
                let size = pin.size - self.offset;
                if let Some(a) = self.area() {
                    note_fetch(&self.inner, a, &self.device, size);
                }
                let mut m = Map::new();
                m.insert(
                    "meta".into(),
                    json!({"size": size, "total": pin.size, "offset": self.offset,
                           "name": ppath::name(&self.real), "real": self.real, "high_value": self.high}),
                );
                Some(Ok(m))
            }
            Stage::Sha => {
                self.stage = Stage::Pieces;
                let (mut at, to) = (self.offset, pin.size);
                let mut h = Some(Sha256::new());
                // hashing a large file takes a while: off the daemon's threads, a slice at a time, so a phone that
                // leaves stops it within one slice
                let digest: io::Result<String> = loop {
                    if at >= to {
                        break Ok(proof::b64u(&h.take().unwrap_or_default().finalize()));
                    }
                    let (f, mut hh) = (file.clone(), h.take().unwrap_or_default());
                    let slice =
                        tokio::task::spawn_blocking(move || -> io::Result<(Sha256, u64)> {
                            let mut buf = vec![0u8; 1 << 20];
                            let end = to.min(at + HASH_SLICE);
                            let mut here = at;
                            while here < end {
                                let want = ((end - here) as usize).min(buf.len());
                                let n = f.read_at(&mut buf[..want], here)?;
                                if n == 0 {
                                    return Err(io::Error::new(
                                        io::ErrorKind::UnexpectedEof,
                                        "shorter",
                                    ));
                                }
                                hh.update(&buf[..n]);
                                here += n as u64;
                            }
                            Ok((hh, here))
                        })
                        .await;
                    match slice {
                        Ok(Ok((hh, here))) => {
                            h = Some(hh);
                            at = here;
                        }
                        Ok(Err(e)) => break Err(e),
                        Err(_) => break Err(io::Error::other("the hash stopped")),
                    }
                };
                match digest {
                    Ok(d) => {
                        if let Err(r) = self.still() {
                            return self.fail(r);
                        }
                        let mut m = Map::new();
                        m.insert("sha256".into(), json!(d));
                        Some(Ok(m))
                    }
                    _ => self.fail(refused(wire::FAILED, "The file could not be read.")),
                }
            }
            Stage::Pieces => {
                if let Some(a) = self.area() {
                    a.hook("piece", &self.real);
                }
                if let Err(r) = self.still() {
                    return self.fail(r);
                }
                if self.at >= pin.size {
                    self.stage = Stage::Done;
                    self.ended = true;
                    self.inner.audit(
                        &self.device,
                        "phone.read",
                        "private",
                        "ok",
                        json!({"path": self.real, "bytes": self.sent, "offset": self.offset}),
                    );
                    return None;
                }
                let want = ((pin.size - self.at) as usize).min(READ_PIECE);
                let mut buf = vec![0u8; want];
                let mut got = 0usize;
                while got < want {
                    match file.read_at(&mut buf[got..], self.at + got as u64) {
                        Ok(0) => break,
                        Ok(n) => got += n,
                        Err(_) => {
                            return self.fail(refused(wire::FAILED, "The file could not be read."))
                        }
                    }
                }
                if got < want {
                    return self.fail(refused(
                        phone_gate::FILE_CHANGED,
                        "The file got shorter while it was being sent, so FERB stopped; fetch it again.",
                    ));
                }
                self.at += got as u64;
                self.sent += got as u64;
                let mut m = Map::new();
                m.insert("d".into(), json!(STANDARD.encode(&buf)));
                Some(Ok(m))
            }
            Stage::Done => None,
        }
    }
}

impl EventStream for Read {
    fn next(&mut self) -> BoxFuture<'_, Option<HandlerResult>> {
        Box::pin(self.step())
    }
}

impl Drop for Read {
    fn drop(&mut self) {
        // the phone left (paused, the link went) before the end: said in the audit with what was sent
        if !self.ended && self.file.is_some() {
            self.inner.audit(
                &self.device,
                "phone.read",
                "private",
                "stopped",
                json!({"path": self.real, "bytes": self.sent}),
            );
        }
    }
}

/// The heavy-fetch notice (PHN-128): once per burst, a notice, never a cap.
// decision: PHN-128
// decision: PHN-138
fn note_fetch(inner: &Inner, a: &Area, device: &str, size: u64) {
    let now = inner.mono();
    let l = &a.limits;
    let (files, bytes, tell) = {
        let mut st = lock(&a.state);
        let (q, told) = st.fetched.entry(device.to_owned()).or_default();
        while q
            .front()
            .is_some_and(|(t, _)| now - t > l.fetch_notice_window_s)
        {
            q.pop_front();
        }
        if q.is_empty() {
            *told = false; // a quiet window has passed: a new burst is told again
        }
        q.push_back((now, size));
        let (files, bytes) = (q.len() as u64, q.iter().map(|(_, n)| n).sum::<u64>());
        let over = (l.fetch_notice_files > 0 && files > l.fetch_notice_files)
            || (l.fetch_notice_bytes > 0 && bytes > l.fetch_notice_bytes);
        let tell = over && !*told;
        *told = *told || over;
        (files, bytes, tell)
    };
    if !tell {
        return;
    }
    let name = inner
        .live(device)
        .map(|p| p.name)
        .unwrap_or_else(|_| "A phone".into());
    let minutes = ((l.fetch_notice_window_s / 60.0).round() as u64).max(1);
    let message = format!(
        "{name} took {files} files ({}) from your Mac in the last {minutes} minutes.",
        human(bytes)
    );
    let allow = "If that was not you, remove this phone in FERB on the Mac";
    inner.audit(
        device,
        "phone.heavy_fetch",
        "notice",
        "told",
        json!({"files": files, "bytes": bytes}),
    );
    (inner.seams.journal)(
        "phone.heavy_fetch",
        json!({"phone": device, "files": files, "bytes": bytes}),
    );
    let mut ev = Map::new();
    ev.insert("phone".into(), json!(device));
    ev.insert("name".into(), json!(name));
    ev.insert("files".into(), json!(files));
    ev.insert("bytes".into(), json!(bytes));
    ev.insert("message".into(), json!(message));
    ev.insert("allow".into(), json!(allow));
    (inner.seams.publish)("phone.heavy_fetch", ev);
    // for the helper: one notice in that phone's chat (the owner's Mac may be on Do Not Disturb)
    let _ = inner.events.send(
        json!({"event": "heavy_fetch", "id": device, "name": name, "files": files,
        "bytes": bytes, "error": {"code": "heavy_fetch", "message": message, "allow": allow}}),
    );
}

// ---- upload ----------------------------------------------------------------------------------------------------------

/// The inbox, made when missing and opened; refused when it lies in a place FERB's own tools may not write.
fn inbox(a: &Area) -> Result<(Arc<OwnedFd>, String), Refusal> {
    let path = &a.seams.inbox;
    let fail = |e: io::Error| {
        refused(
            wire::FAILED,
            format!("FERB's folder for files from phones could not be opened ({e})."),
        )
    };
    let closed = |p: &str| {
        a.seams
            .judge
            .rules(p, ferb_policy::protection::WRITE)
            .first()
            .map(|rule| {
                refused(
                    phone_gate::FILE_NOT_ALLOWED,
                    format!(
                        "FERB's folder for files from phones is in a protected place ({rule}), so nothing was taken."
                    ),
                )
            })
    };
    // judged before anything is made there, and again by the name the folder really has
    if let Some(r) = closed(&path.to_string_lossy()) {
        return Err(r);
    }
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(path)
        .map_err(fail)?;
    let f = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY)
        .open(path)
        .map_err(fail)?;
    let real = kernel_path(f.as_raw_fd()).map_err(fail)?;
    if let Some(r) = closed(&real) {
        return Err(r);
    }
    Ok((Arc::new(OwnedFd::from(f)), real))
}

/// Part files older than the keep time: those of this daemon (paused and not resumed), and once those a daemon before
/// it left in the inbox.
fn sweep(inner: &Inner, a: &Area, st: &mut State) {
    let (now, keep) = (inner.mono(), a.limits.upload_partial_keep_s);
    st.uploads.retain(|_, up| {
        let stale = up.saved.is_none() && up.file.is_none() && now - up.at > keep;
        if stale {
            unlink_at(up.dir.as_raw_fd(), &up.part);
        }
        !stale
    });
    if !st.swept {
        st.swept = true;
        let wall = inner.seams.clock.now().timestamp();
        let mut stack = vec![a.seams.inbox.clone()];
        while let Some(dir) = stack.pop() {
            let Ok(rd) = std::fs::read_dir(&dir) else {
                continue;
            };
            for e in rd.flatten() {
                let Ok(m) = std::fs::symlink_metadata(e.path()) else {
                    continue;
                };
                if m.is_dir() {
                    stack.push(e.path());
                } else if m.is_file()
                    && e.file_name().to_string_lossy().ends_with(PART)
                    && (wall - m.mtime()) as f64 > keep
                {
                    let _ = std::fs::remove_file(e.path());
                }
            }
        }
    }
}

/// `phone.put_group {name}`: one folder in the inbox for a folder sent from the phone: `{group, saved}`.
fn put_group(inner: &Inner, session: &Session, params: &Params) -> HandlerResult {
    let phone = phone_of(session);
    let out = (|| {
        let a = area(inner)?;
        let (dir, _) = inbox(a)?;
        let gid = inner.nonce();
        let name = format!(
            "{gid}-{}",
            clean_name(
                params.get("name").and_then(Value::as_str).unwrap_or(""),
                "folder"
            )
        );
        let c = cstr(name.as_bytes()).map_err(|_| bad("That folder's name cannot be used."))?;
        // SAFETY: `dir` is an open descriptor and `c` a NUL-terminated name.
        if unsafe { libc::mkdirat(dir.as_raw_fd(), c.as_ptr(), 0o700) } != 0 {
            return Err(refused(
                wire::FAILED,
                format!(
                    "The folder could not be made ({}).",
                    io::Error::last_os_error()
                ),
            ));
        }
        let sub = open_at(
            dir.as_raw_fd(),
            name.as_bytes(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW,
            0,
        )
        .map_err(|e| {
            refused(
                wire::FAILED,
                format!("The folder could not be opened ({e})."),
            )
        })?;
        let real = kernel_path(sub.as_raw_fd()).map_err(|e| {
            refused(
                wire::FAILED,
                format!("The folder could not be named ({e})."),
            )
        })?;
        lock(&a.state).groups.insert(
            gid.clone(),
            Group {
                device: phone.device.clone(),
                dir: Arc::new(OwnedFd::from(sub)),
                real: real.clone(),
            },
        );
        let mut out = Params::new();
        out.insert("group".into(), json!(gid));
        out.insert("saved".into(), json!(real));
        Ok(out)
    })();
    match &out {
        Ok(o) => inner.audit(
            &phone.device,
            "phone.put_group",
            "plain",
            "ok",
            json!({"path": o["saved"]}),
        ),
        Err(r) => audit_refusal(
            inner,
            &phone.device,
            "phone.put_group",
            "plain",
            r,
            Value::Null,
        ),
    }
    out.map_err(coded)
}

/// `phone.put_begin {name, size, resume?, group?, rel?}`: `{upload_id, offset}`.
// decision: PHN-127
// decision: PHN-139
fn put_begin(inner: &Inner, session: &Session, params: &Params) -> HandlerResult {
    let phone = phone_of(session);
    let out = (|| {
        let a = area(inner)?;
        let size = params
            .get("size")
            .filter(|v| v.is_u64())
            .and_then(Value::as_u64)
            .ok_or_else(|| bad("link.put needs the size"))?;
        let mut st = lock(&a.state);
        sweep(inner, a, &mut st);
        if let Some(id) = params.get("resume").filter(|v| !v.is_null()) {
            let id = id.as_str().unwrap_or("").to_owned();
            return resume(inner, &mut st, &phone, &id, size);
        }
        let (dir, dir_real) = inbox(a)?;
        let free = (a.seams.free_bytes)(Path::new(&dir_real)).map_err(|e| {
            refused(
                wire::FAILED,
                format!("FERB could not tell the Mac's free space ({e})."),
            )
        })?;
        let floor = a.limits.upload_free_floor;
        if free < size.saturating_add(floor) {
            return Err(refused(
                phone_gate::MAC_DISK_FULL,
                format!(
                    "Your Mac has {} free and FERB keeps {} free, so this file ({}) was not taken.",
                    human(free),
                    human(floor),
                    human(size)
                ),
            ));
        }
        let id = inner.nonce();
        let name = clean_name(
            params.get("name").and_then(Value::as_str).unwrap_or(""),
            "file",
        );
        let (dir, dir_real, final_name) = match params.get("group").filter(|v| !v.is_null()) {
            Some(g) => {
                let g = g.as_str().unwrap_or("");
                let group = st
                    .groups
                    .get(g)
                    .filter(|gr| gr.device == phone.device)
                    .ok_or_else(|| {
                        refused(
                            phone_gate::NOT_FOUND,
                            "That folder upload was not started from this phone.",
                        )
                    })?;
                let rel = params
                    .get("rel")
                    .and_then(Value::as_str)
                    .map_or_else(|| name.clone(), str::to_owned);
                let parts: Vec<String> = rel.split('/').map(|x| clean_name(x, "")).collect();
                let raw: Vec<&str> = rel.split('/').collect();
                if parts.is_empty()
                    || parts.len() > REL_PARTS_MAX
                    || parts.iter().any(|x| x.is_empty() || x == "." || x == "..")
                    || raw.iter().any(|x| *x == ".." || *x == ".")
                {
                    return Err(bad("That file's place inside the folder cannot be used."));
                }
                let mut here = group.dir.clone();
                let mut real = group.real.clone();
                for part in &parts[..parts.len() - 1] {
                    let sub = folder_at(here.as_raw_fd(), part).map_err(|_| {
                        refused(
                            phone_gate::FILE_NOT_ALLOWED,
                            "That file's place leads outside the folder being sent.",
                        )
                    })?;
                    real = ppath::div(&real, part);
                    here = Arc::new(sub);
                }
                let last = parts[parts.len() - 1].clone();
                if stat_at(here.as_raw_fd(), last.as_bytes()).is_ok() {
                    return Err(bad(
                        "A file of that name was already sent into this folder.",
                    ));
                }
                (here, real, last)
            }
            None => (dir, dir_real, format!("{id}-{}", name.replace(' ', ""))),
        };
        let part = format!(".{id}{PART}");
        let file = open_at(
            dir.as_raw_fd(),
            part.as_bytes(),
            libc::O_RDWR | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW,
            0o600,
        )
        .map_err(|e| refused(wire::FAILED, format!("The file could not be made ({e}).")))?;
        // marked before its first byte, so even a part that stays behind is a downloaded file
        quarantine(&file, inner.seams.clock.now().timestamp()).map_err(|e| {
            refused(
                wire::FAILED,
                format!("The file could not be marked as downloaded ({e})."),
            )
        })?;
        let m = file
            .metadata()
            .map_err(|e| refused(wire::FAILED, format!("The file could not be made ({e}).")))?;
        st.uploads.insert(
            id.clone(),
            Upload {
                device: phone.device.clone(),
                session: phone.session.clone(),
                dir,
                dir_real,
                part,
                name: final_name,
                size,
                got: 0,
                hasher: Sha256::new(),
                file: Some(file),
                ident: (m.dev(), m.ino()),
                at: inner.mono(),
                saved: None,
            },
        );
        let mut out = Params::new();
        out.insert("upload_id".into(), json!(id));
        out.insert("offset".into(), json!(0));
        Ok(out)
    })();
    match &out {
        Ok(o) => {
            let st = area(inner).map(|a| lock(&a.state));
            let path = st.ok().and_then(|st| {
                o["upload_id"]
                    .as_str()
                    .and_then(|id| st.uploads.get(id))
                    .map(|up| ppath::div(&up.dir_real, &up.name))
            });
            inner.audit(
                &phone.device,
                "phone.put_begin",
                "plain",
                "ok",
                json!({"path": path, "size": params.get("size"), "offset": o["offset"],
                       "resume": params.get("resume").is_some_and(|v| !v.is_null())}),
            );
        }
        Err(r) => audit_refusal(
            inner,
            &phone.device,
            "phone.put_begin",
            "plain",
            r,
            json!(params.get("name").and_then(Value::as_str)),
        ),
    }
    out.map_err(coded)
}

/// Goes on with an upload this phone paused: from the bytes the part file really holds, hashed again.
fn resume(
    inner: &Inner,
    st: &mut State,
    phone: &PhoneSession,
    id: &str,
    size: u64,
) -> Result<Params, Refusal> {
    let gone = || {
        refused(
            phone_gate::NOT_FOUND,
            "That upload cannot be continued; send the file again.",
        )
    };
    let up = st.uploads.get_mut(id).ok_or_else(gone)?;
    if up.device != phone.device || up.saved.is_some() || up.size != size || up.file.is_some() {
        return Err(gone());
    }
    let file = open_at(
        up.dir.as_raw_fd(),
        up.part.as_bytes(),
        libc::O_RDWR | libc::O_NOFOLLOW,
        0,
    )
    .map_err(|_| gone())?;
    let m = file.metadata().map_err(|_| gone())?;
    if (m.dev(), m.ino()) != up.ident || !m.is_file() {
        return Err(gone());
    }
    let got = up.got.min(m.size());
    file.set_len(got).map_err(|_| gone())?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    let mut at = 0u64;
    while at < got {
        let want = ((got - at) as usize).min(buf.len());
        let n = file.read_at(&mut buf[..want], at).map_err(|_| gone())?;
        if n == 0 {
            return Err(gone());
        }
        h.update(&buf[..n]);
        at += n as u64;
    }
    up.hasher = h;
    up.got = got;
    up.file = Some(file);
    up.session = phone.session.clone();
    up.at = inner.mono();
    let mut out = Params::new();
    out.insert("upload_id".into(), json!(id));
    out.insert("offset".into(), json!(got));
    Ok(out)
}

/// `phone.put_chunk {upload_id, d}`: `{got}`. Rides the begin's unlock: a lock meanwhile does not stop it.
fn put_chunk(inner: &Inner, session: &Session, params: &Params) -> HandlerResult {
    let phone = phone_of(session);
    let a = area(inner).map_err(coded)?;
    let id = params
        .get("upload_id")
        .and_then(Value::as_str)
        .unwrap_or("");
    let mut st = lock(&a.state);
    let none = || {
        coded(refused(
            phone_gate::NOT_FOUND,
            "No such upload is under way.",
        ))
    };
    let up = st.uploads.get_mut(id).ok_or_else(none)?;
    if up.device != phone.device || up.file.is_none() {
        return Err(none());
    }
    let data = STANDARD
        .decode(params.get("d").and_then(Value::as_str).unwrap_or(""))
        .map_err(|_| coded(bad("That chunk is not base64.")))?;
    if up.got + data.len() as u64 > up.size {
        return Err(coded(bad(
            "More bytes arrived than the transfer announced.",
        )));
    }
    if let Some(f) = &up.file {
        f.write_all_at(&data, up.got).map_err(|e| {
            coded(refused(
                wire::FAILED,
                format!("The file could not be written ({e})."),
            ))
        })?;
    }
    up.hasher.update(&data);
    up.got += data.len() as u64;
    up.at = inner.mono();
    let mut out = Params::new();
    out.insert("got".into(), json!(up.got));
    Ok(out)
}

/// `phone.put_end {upload_id, abort?, drop?, sha256?}`: finished, `{upload_id, saved, size, sha256}`; stopped part way,
/// `{upload_id, kept, got}` (kept for the same phone to go on with, unless dropped or empty).
fn put_end(inner: &Inner, session: &Session, params: &Params) -> HandlerResult {
    let phone = phone_of(session);
    let a = area(inner).map_err(coded)?;
    let id = params
        .get("upload_id")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned();
    let mut st = lock(&a.state);
    let none = || {
        coded(refused(
            phone_gate::NOT_FOUND,
            "No such upload is under way.",
        ))
    };
    let up = st.uploads.get_mut(&id).ok_or_else(none)?;
    if up.device != phone.device || up.file.is_none() {
        return Err(none());
    }
    up.file = None;
    up.at = inner.mono();
    let abort = params.get("abort") == Some(&json!(true));
    if abort || up.got != up.size {
        let dropped = params.get("drop") == Some(&json!(true)) || up.got == 0;
        let got = up.got;
        let path = ppath::div(&up.dir_real, &up.name);
        if dropped {
            unlink_at(up.dir.as_raw_fd(), &up.part);
            st.uploads.remove(&id);
        }
        drop(st);
        inner.audit(
            &phone.device,
            "phone.put_end",
            "plain",
            if dropped { "dropped" } else { "paused" },
            json!({"path": path, "got": got}),
        );
        let mut out = Params::new();
        out.insert("upload_id".into(), json!(id));
        out.insert("kept".into(), json!(!dropped));
        out.insert("got".into(), json!(got));
        return Ok(out);
    }
    let digest = proof::b64u(&up.hasher.clone().finalize());
    let path = ppath::div(&up.dir_real, &up.name);
    let theirs = params.get("sha256").and_then(Value::as_str);
    let refusal = if theirs.is_some_and(|s| s != digest) {
        Some(bad(
            "The file that arrived is not the file the phone sent (its SHA-256 differs), so it was not kept.",
        ))
    } else {
        match name_at(up.dir.as_raw_fd(), &up.part, &up.name) {
            Ok(()) => None,
            Err(e) if e.raw_os_error() == Some(libc::EEXIST) => Some(bad(
                "A file of that name was already sent into this folder.",
            )),
            Err(e) => Some(refused(
                wire::FAILED,
                format!("The file could not be given its name ({e})."),
            )),
        }
    };
    if let Some(r) = refusal {
        unlink_at(up.dir.as_raw_fd(), &up.part);
        st.uploads.remove(&id);
        drop(st);
        audit_refusal(
            inner,
            &phone.device,
            "phone.put_end",
            "plain",
            &r,
            json!(path),
        );
        return Err(coded(r));
    }
    up.saved = Some(path.clone());
    let size = up.size;
    drop(st);
    inner.audit(
        &phone.device,
        "phone.put_end",
        "plain",
        "ok",
        json!({"path": path, "size": size}),
    );
    let mut out = Params::new();
    out.insert("upload_id".into(), json!(id));
    out.insert("saved".into(), json!(path));
    out.insert("size".into(), json!(size));
    out.insert("sha256".into(), json!(digest));
    Ok(out)
}

// ---- what the rest of the phone area asks ----------------------------------------------------------------------------

/// A message's attachments from this phone: its own finished uploads and the folders it sent, by id, as the paths
/// `chat.send` takes (`input_paths`); anything else is refused, naming nothing of what exists.
// decision: PHN-127
pub(crate) fn attachments(
    inner: &Inner,
    device: &str,
    ids: &Value,
) -> Result<Vec<String>, Refusal> {
    let ids = ids
        .as_array()
        .ok_or_else(|| bad("attachments is a list of this phone's upload ids."))?;
    let a = area(inner)?;
    let st = lock(&a.state);
    ids.iter()
        .map(|v| {
            let id = v.as_str().unwrap_or("");
            if let Some(path) = st
                .uploads
                .get(id)
                .filter(|u| u.device == device)
                .and_then(|u| u.saved.clone())
            {
                return Ok(path);
            }
            match st.groups.get(id).filter(|g| g.device == device) {
                Some(g) => Ok(g.real.clone()),
                None => Err(refused(
                    phone_gate::NOT_FOUND,
                    "An attachment of this message was not uploaded from this phone.",
                )),
            }
        })
        .collect()
}

/// The phone's link connection ended: its uploads under way pause (kept for the same phone to go on with).
pub(crate) fn session_ended(inner: &Inner, phone: &PhoneSession) {
    let Some(a) = inner.files.get() else {
        return;
    };
    let now = inner.mono();
    let mut st = lock(&a.state);
    for up in st.uploads.values_mut() {
        if up.device == phone.device && up.session == phone.session && up.file.is_some() {
            up.file = None;
            up.at = now;
        }
    }
    sweep(inner, a, &mut st);
}

/// A removed phone: its unfinished uploads are deleted now; its folders and finished files stay (the owner's files).
pub(crate) fn forget_device(inner: &Inner, device: &str) {
    let Some(a) = inner.files.get() else {
        return;
    };
    let mut st = lock(&a.state);
    st.uploads.retain(|_, up| {
        if up.device != device {
            return true;
        }
        if up.saved.is_none() {
            up.file = None;
            unlink_at(up.dir.as_raw_fd(), &up.part);
        }
        false
    });
    st.groups.retain(|_, g| g.device != device);
    st.fetched.remove(device);
}

// ---- registration ----------------------------------------------------------------------------------------------------

type Unary = fn(&Inner, &Session, &Params) -> HandlerResult;

impl Phones {
    /// Puts the file area in place and registers its seven methods on `server` (stage 3 S3b), and the recent files and
    /// the name search (T-1211, `find`).
    pub fn register_files(&self, server: &Server, seams: FileSeams, limits: FileLimits) {
        let _ = self.inner.files.set(Area::new(seams, limits));
        super::find::register(self, server);
        let unary: [(&str, Unary); 6] = [
            ("phone.files", files),
            ("phone.tree", tree),
            ("phone.put_group", put_group),
            ("phone.put_begin", put_begin),
            ("phone.put_chunk", put_chunk),
            ("phone.put_end", put_end),
        ];
        for (name, f) in unary {
            let me = self.clone();
            server.register_unary(name, move |s: Session, p: Params| {
                let out = f(&me.inner, &s, &p);
                async move { out }
            });
        }
        let me = self.clone();
        server.register_stream("phone.read", move |s: Session, p: Params| {
            Box::new(read(me.inner.clone(), &s, &p)) as Box<dyn EventStream>
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Files never cross the shared relay, by the daemon's own check too (S4): a file, a folder fetch and an upload's
    /// start are refused on the tier the helper names; a listing and the transfers' other parts are not; an older
    /// helper that names no tier is the helper's to stop.
    // decision: PHN-491
    // decision: PHN-444
    #[test]
    fn files_are_refused_on_the_shared_relay_by_the_daemon_too() {
        let on = |tier: Option<&str>| PhoneSession {
            tier: tier.map(str::to_owned),
            ..PhoneSession::default()
        };
        for m in [
            "phone.read",
            "phone.tree",
            "phone.put_begin",
            "phone.put_group",
        ] {
            let r = shared_relay_refusal(&on(Some("shared_relay")), m).expect(m);
            assert_eq!(r.code, phone_gate::FILE_NOT_ALLOWED, "{m}");
            assert!(
                shared_relay_refusal(&on(Some("direct")), m).is_none(),
                "{m}"
            );
            assert!(
                shared_relay_refusal(&on(Some("own_relay")), m).is_none(),
                "{m}"
            );
            assert!(shared_relay_refusal(&on(None), m).is_none(), "{m}");
        }
        for m in [
            "phone.files",
            "phone.put_chunk",
            "phone.put_end",
            "chat.send",
        ] {
            assert!(
                shared_relay_refusal(&on(Some("shared_relay")), m).is_none(),
                "{m}"
            );
        }
    }

    #[test]
    fn names_are_cleaned_as_the_lab_cleans_them() {
        assert_eq!(clean_name("../../etc/passwd", "file"), "etcpasswd");
        assert_eq!(clean_name("...", "file"), "file");
        assert_eq!(clean_name(" Holiday 2026 ", "folder"), "Holiday 2026");
        assert_eq!(clean_name("a/b\\c:d", "x"), "abcd");
    }

    #[test]
    fn a_path_is_whole_or_starts_with_the_lists_home() {
        assert_eq!(expand("~", "/h").as_deref(), Some("/h"));
        assert_eq!(expand("~/a", "/h/").as_deref(), Some("/h/a"));
        assert_eq!(expand("~root/a", "/h"), None);
        assert_eq!(expand("a/b", "/h"), None);
        assert_eq!(expand("/a\0b", "/h"), None);
    }

    #[test]
    fn case_and_unicode_form_do_not_tell_two_names_apart() {
        assert!(under("/H/SCHLU\u{308}SSEL/k", "/h/Schl\u{fc}ssel"));
        assert!(!under("/h/Schl", "/h/Schl\u{fc}ssel"));
        assert_eq!(
            firmlinked("/System/Volumes/Data/Users/x").as_deref(),
            Some("/Users/x")
        );
        assert_eq!(firmlinked("/System/Volumes/Database"), None);
    }

    #[test]
    fn sizes_read_as_the_lab_says_them() {
        assert_eq!(human(1 << 30), "1.0 GB");
        assert_eq!(human(3 << 20), "3.0 MB");
        assert_eq!(human(2048), "2 kB");
    }
}
