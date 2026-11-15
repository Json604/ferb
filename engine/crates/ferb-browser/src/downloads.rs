//! A browser download taken into FERB's workspace as facts: a port of `ferb/act/browser/downloads.py` (D35;
//! decisions: HAND-210, HAND-211, HAND-231, HAND-233, HAND-235, HAND-236, HAND-246). It must not judge the file or
//! pick a source.
//!
//! The extension catches a download only in a tab FERB drives for a task and holds it there. This side takes it only
//! while that tab is a live task tab; otherwise it goes back to the browser untouched. It pulls the bytes in parts
//! (`download_part`), one transfer per tab, within the task tree's budget (HAND-210: no size cap per file; 300 files
//! and 32 GB per request), failed only when it stops moving (no bytes for [`STALL_S`]), never for how long it takes;
//! hashes what it received; writes `<workspace>/downloads/<id>-<safe name>` and records facts, never content. The
//! folder's total over `browser.download_total_gb` is a structural fact ([`Downloads::over_cap`]) handed to the daemon,
//! which asks the owner (HAND-211); their Yes deletes the oldest for good ([`Downloads::delete`]). A download that
//! would leave less than `browser.download_min_free_gb` free is refused: by its declared size before any byte moves,
//! and before each part is written.
//!
//! The byte budget is one counter shared with FERB's shell (HAND-231): what a tree's shell commands received through
//! the egress proxy ([`Downloads::shell_received`], kept in `shell_downloads`) counts beside the files its tabs took;
//! [`Downloads::shell_stop`] says when a command receiving bytes must stop. ferb-shell's download watch (it reads
//! every second, HAND-232) reports through its `OnReceived` closure: [`Downloads::shell_hook`] is that closure
//! ([`ShellBudget`] is the same seam as a trait).
//!
//! **Overshoot, as Python behaves:** the budget is checked after each part arrives, so a transfer passes the budget by
//! up to one part (512 KiB from the extension) before it is refused, and that part is never written; transfers in
//! flight in other tabs of the tree count only what has arrived; a shell command is checked by its watch's polling, so
//! it overshoots by what arrived in that interval. Nothing promises a byte-exact cutoff. All sizes are decimal.
//!
//! Seams: [`Peer`] (the extension connection a download came on: [`BridgePeer`] for a real bridge), [`Records`] (the
//! `downloads` and `shell_downloads` tables: [`sqlite::SqliteRecords`] on the Mac), the clock (`now` for records, the
//! monotonic stall clock), the config lookup, the redactor, the bus, the random source for ids, and [`DlHooks`]: the
//! live task tab of a download, a step's task tree, the free space ([`available_bytes`]) and the over-cap check.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use ferb_core::interfaces::{BoxFuture, Clock};
use serde_json::{json, Map, Value};

use crate::bridge::{Bridge, Publish, TaskTab};
use crate::pairing::Random;
use crate::protocol::Event;

/// `ORIGIN`: a downloaded file is outside content (MP2).
pub const ORIGIN: &str = "external:download";
/// `FOLDER`: under FERB's workspace.
pub const FOLDER: &str = "downloads";
/// `NAME_MAX_BYTES`: the safe name; the id prefix adds 16 (APFS allows 255 bytes).
pub const NAME_MAX_BYTES: usize = 150;
/// `STALL_S` (HAND-233): a transfer fails only when no byte arrives for this long.
pub const STALL_S: f64 = 60.0;
/// `TAB_WAIT_S`: a second download in a tab waits this long for the first, then is refused.
pub const TAB_WAIT_S: f64 = 20.0;
/// `SETTLE_S`: a navigation or open waits this long for a download its tab started.
pub const SETTLE_S: f64 = 60.0;
/// `DEFAULTS` (HAND-210, HAND-236): the owner's limits, in decimal GB.
pub const DEFAULTS: [(&str, f64); 4] = [
    ("download_task_max_files", 300.0),
    ("download_task_max_gb", 32.0),
    ("download_total_gb", 20.0),
    ("download_min_free_gb", 10.0),
];
/// `OUTSIDE`: a row whose file isn't in the folder (left as it is, never counted).
pub const OUTSIDE: &str =
    "skipped: not in FERB's downloads folder, so retention leaves it where it is";
const GB: i64 = 1_000_000_000;

/// `TaskTab`'s identity names: `owner` or `ferb`.
pub type TaskTabOf = Arc<dyn Fn(&dyn Peer, i64) -> Option<TaskTab> + Send + Sync>;

/// The extension connection a download came on (`br`): its profile name and its calls.
pub trait Peer: Send + Sync {
    /// `br.name`: `browser/folder`.
    fn name(&self) -> String;
    /// `br.call(op, args, timeout=...)`; `Err` is the error's text (`str(e)`).
    fn call(
        &self,
        op: &str,
        args: Value,
        timeout: f64,
    ) -> BoxFuture<'static, Result<Value, String>>;
    /// The bridge behind it, when there is one (the hub's `task_tab` looks a tab up by it).
    fn bridge(&self) -> Option<Arc<Bridge>> {
        None
    }
}

/// A real extension bridge as a [`Peer`].
pub struct BridgePeer(pub Arc<Bridge>);

impl Peer for BridgePeer {
    fn name(&self) -> String {
        self.0.name()
    }
    fn call(
        &self,
        op: &str,
        args: Value,
        timeout: f64,
    ) -> BoxFuture<'static, Result<Value, String>> {
        let call = self.0.call(op, args, timeout, None);
        Box::pin(async move { call.await.map_err(|e| e.message()) })
    }
    fn bridge(&self) -> Option<Arc<Bridge>> {
        Some(Arc::clone(&self.0))
    }
}

/// A row of the `downloads` table, as `SELECT *` gives it (column order kept).
pub type Row = Map<String, Value>;

/// A Python exception as the records name it: its class (`type(e).__name__`) and its text (`str(e)`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fault {
    pub class: String,
    pub message: String,
}

impl Fault {
    pub fn new(class: &str, message: &str) -> Fault {
        Fault {
            class: class.into(),
            message: message.into(),
        }
    }

    /// `f"{type(e).__name__}: {e}"`.
    pub fn typed(&self) -> String {
        format!("{}: {}", self.class, self.message)
    }
}

impl std::fmt::Display for Fault {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

/// The `downloads` and `shell_downloads` tables (FERB's store). `Err` is the database's error.
pub trait Records: Send + Sync {
    /// `INSERT INTO downloads (<the row's keys>) VALUES (...)`.
    fn insert(&self, row: &Row) -> Result<(), Fault>;
    /// `_finish`'s update, then the row as `SELECT *` reads it.
    #[allow(clippy::too_many_arguments)]
    fn finish(
        &self,
        id: &str,
        state: &str,
        path: Option<&str>,
        name: Option<&str>,
        size: Option<i64>,
        sha256: Option<&str>,
        reason: Option<&str>,
    ) -> Result<Row, Fault>;
    /// `_tree_usage`: (downloads the tree took or is taking, bytes it took).
    fn tree_usage(&self, tree: &str) -> Result<(i64, i64), Fault>;
    /// The tree's `shell_downloads` bytes, `None` without a row.
    fn shell_bytes(&self, tree: &str) -> Result<Option<i64>, Fault>;
    /// `shell_received`'s upsert.
    fn set_shell(&self, tree: &str, bytes: i64, at: &str) -> Result<(), Fault>;
    /// `finalize_stale`'s update: rows changed.
    fn finalize_stale(&self) -> Result<i64, Fault>;
    /// `SELECT id, path, bytes, at, kept FROM downloads WHERE path IS NOT NULL ORDER BY at, rowid`.
    fn with_path(&self) -> Result<Vec<Row>, Fault>;
    /// `UPDATE downloads SET kept = ? WHERE id = ?`.
    fn set_kept(&self, id: &str, kept: &str) -> Result<(), Fault>;
    /// `UPDATE downloads SET path = NULL, kept = ? WHERE id = ?`.
    fn clear_path(&self, id: &str, kept: &str) -> Result<(), Fault>;
    /// `facts_for`'s select: the tasks' rows (in this identity's profiles), `ORDER BY at, rowid`.
    fn of_tasks(&self, tasks: &[String], identity: Option<&str>) -> Result<Vec<Row>, Fault>;
}

/// The owner's limits (`Limits`), in bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    pub task_files: i64,
    pub task_bytes: i64,
    pub total_bytes: i64,
    pub min_free_bytes: i64,
}

/// `OverCap` (HAND-211): the oldest files that would go (download ids, never names), their bytes, the folder's
/// total and the cap.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OverCap {
    pub ids: Vec<String>,
    pub bytes: i64,
    pub total: i64,
    pub cap: i64,
}

/// The config lookup (`config.get(section, key, default)`): `Ok(None)` when the key isn't set, `Err` when the config
/// can't be read.
pub type Setting = Arc<dyn Fn(&str, &str) -> Result<Option<Value>, Fault> + Send + Sync>;

/// What the daemon wires in after construction (`task_tab`, `task_root`, `free_bytes`, `on_over_cap`).
#[derive(Clone)]
pub struct DlHooks {
    /// The live task that drives this tab now; `None`: the download goes back to the browser.
    pub task_tab: TaskTabOf,
    /// The task tree a worker step belongs to (budgets).
    pub task_root: Arc<dyn Fn(&str) -> String + Send + Sync>,
    /// Free bytes for the floor; `Err` is an `OSError`'s text (the floor can't be checked: refused).
    pub free_bytes: Arc<dyn Fn() -> Result<i64, String> + Send + Sync>,
    /// The daemon's cap check (HAND-211), after a file is taken; its failure is ignored (the next pass checks).
    pub on_over_cap: Arc<dyn Fn() + Send + Sync>,
}

/// How a transfer ended early (`_Refused`, or any other failure's text).
enum Stop {
    Refused(String, Option<i64>),
    Failed(String),
}

type Key = (String, i64);

#[derive(Default)]
struct State {
    /// tab -> its transfers' job ids still running
    active: HashMap<Key, HashSet<u64>>,
    next_job: u64,
    locks: HashMap<Key, Arc<tokio::sync::Mutex<()>>>,
    /// tree -> bytes received by its transfers still running
    inflight: HashMap<String, i64>,
    /// tree -> transfers still running
    running: HashMap<String, i64>,
    unreported: HashMap<Key, Vec<Map<String, Value>>>,
    /// tab -> the task whose downloads it holds (pruning)
    tab_task: HashMap<Key, String>,
    /// tree -> bytes its shell commands received (cache of the shell_downloads row)
    shell: HashMap<String, i64>,
}

/// `downloads.Downloads`: one per daemon, shared by the owner's and FERB's browser drivers.
pub struct Downloads {
    pub root: PathBuf,
    records: Arc<dyn Records>,
    clock: Arc<dyn Clock>,
    setting: Setting,
    redact: Arc<dyn Fn(&str) -> String + Send + Sync>,
    publish: Publish,
    random: Arc<dyn Random>,
    /// `TAB_WAIT_S` in milliseconds (tests may scale it with their clock).
    tab_wait_ms: std::sync::atomic::AtomicU64,
    hooks: Mutex<DlHooks>,
    state: Mutex<State>,
    done: tokio::sync::Notify,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

// ---------- the pure parts ----------

fn dropped(c: char) -> bool {
    c.is_control() || c == '\u{2028}' || c == '\u{2029}' || ferb_policy::py::is_cf(c)
}

/// `safe_name`: a file name that can only name a file inside the downloads folder: no path separators (`/`, `\`,
/// `:`), no control or invisible format characters (a right-to-left override can disguise an extension), no leading
/// dots, at most [`NAME_MAX_BYTES`] of UTF-8 with the extension kept. Empty becomes "download".
pub fn safe_name(raw: &str) -> String {
    use unicode_normalization::UnicodeNormalization;
    let nfc: String = raw.nfc().collect();
    let kept: String = nfc
        .chars()
        .filter(|c| !dropped(*c))
        .map(|c| {
            if matches!(c, '/' | '\\' | ':') {
                '_'
            } else {
                c
            }
        })
        .collect();
    let squeezed = ferb_py::value::squeeze(&kept);
    let mut name = squeezed
        .trim_start_matches(['.', ' '])
        .trim_end_matches(['.', ' '])
        .to_string();
    if name.len() > NAME_MAX_BYTES {
        let keep = match name.rfind('.') {
            Some(i) if i > 0 && !name[i + 1..].is_empty() && name[i + 1..].len() <= 16 => {
                name[i..].to_string()
            }
            _ => String::new(),
        };
        let budget = NAME_MAX_BYTES - keep.len();
        let mut cut = name[..name.len() - keep.len()].to_string();
        while cut.len() > budget {
            cut.pop();
        }
        name = cut.trim_end_matches(['.', ' ']).to_string() + &keep;
    }
    if name.is_empty() {
        "download".into()
    } else {
        name
    }
}

/// `untrusted_roots`: the configured roots plus FERB's downloads folder and the folder of upload copies made from
/// outside content (`uploads/outside`), which code adds so config can't remove them (MP2).
pub fn untrusted_roots(configured: &[String], workspace: &str) -> Vec<String> {
    let mut out: Vec<String> = configured.to_vec();
    for folder in [
        format!("{}/{FOLDER}", workspace.trim_end_matches('/')),
        format!("{}/uploads/outside", workspace.trim_end_matches('/')),
    ] {
        if !out.contains(&folder) {
            out.push(folder);
        }
    }
    out
}

fn two_places(x: f64) -> String {
    let s = format!("{x:.2}");
    let s = s.trim_end_matches('0');
    s.trim_end_matches('.').to_string()
}

/// `_gb`: decimal GB (MB under 0.1 GB, so a small budget never reads "0 GB").
pub fn gb(n: i64) -> String {
    if 0 < n && n < GB / 10 {
        two_places(n as f64 / 1e6) + " MB"
    } else {
        two_places(n as f64 / 1e9) + " GB"
    }
}

fn low_disk(free: i64, floor: i64, what: &str) -> String {
    format!(
        "only {} free now; FERB keeps at least {} free (browser.download_min_free_gb): {what}; not taken",
        gb(free),
        gb(floor)
    )
}

/// `_not_taken`: a download the browser started itself.
pub fn not_taken(detail: Option<&str>) -> &'static str {
    match detail {
        Some("canceled") => "the browser started this download itself and it ended canceled: a Save dialog may have been shown and dismissed; FERB couldn't intercept it",
        Some("new_tab") => "a new tab this tab opened started a download before FERB could attach to it; the browser handled it (it may have shown its own Save dialog)",
        _ => "the browser started this download itself (a link FERB couldn't intercept, e.g. a script-made blob or data link); FERB couldn't take or stop it, so the browser may have shown its own Save dialog and saved it where it saves downloads",
    }
}

/// `DROPPED`.
fn dropped_reason(n: i64) -> String {
    let what = if n == 1 {
        "this save was dropped".to_string()
    } else {
        format!("{n} saves were dropped")
    };
    format!(
        "the page started more saves at once than FERB holds (20 in a tab, 100 in all): {what}, neither taken nor given to the browser; not taken"
    )
}

/// Python's `float(text)` for a config string: `None` where it raises ValueError.
fn py_float_text(text: &str) -> Option<f64> {
    let t = ferb_py::value::strip(text);
    let (sign, body) = match t.strip_prefix('-') {
        Some(b) => (-1.0, b),
        None => (1.0, t.strip_prefix('+').unwrap_or(t)),
    };
    let lower = body.to_ascii_lowercase();
    if lower == "inf" || lower == "infinity" {
        return Some(sign * f64::INFINITY);
    }
    if lower == "nan" {
        return Some(f64::NAN);
    }
    // underscores only between digits (PEP 515)
    let chars: Vec<char> = body.chars().collect();
    for (i, c) in chars.iter().enumerate() {
        if *c == '_'
            && !(i > 0
                && chars[i - 1].is_ascii_digit()
                && chars.get(i + 1).is_some_and(char::is_ascii_digit))
        {
            return None;
        }
        if !(c.is_ascii_digit() || matches!(c, '_' | '.' | 'e' | 'E' | '+' | '-')) {
            return None;
        }
    }
    let plain: String = body.chars().filter(|c| *c != '_').collect();
    if plain.is_empty() || plain.starts_with(['+', '-']) {
        return None;
    }
    plain.parse::<f64>().ok().map(|v| sign * v)
}

/// `_number`: `max(float(value), 0.0)`, the default when `float()` raises TypeError or ValueError.
fn number(value: Option<&Value>, default: f64) -> f64 {
    let v = match value {
        None => return default,
        Some(Value::Bool(b)) => f64::from(u8::from(*b)),
        Some(Value::Number(n)) => n.as_f64().unwrap_or(default),
        Some(Value::String(s)) => match py_float_text(s) {
            Some(f) => f,
            None => return default,
        },
        Some(_) => return default,
    };
    // Python's max(v, 0.0) keeps a NaN
    if v.is_nan() || v >= 0.0 {
        v
    } else {
        0.0
    }
}

/// `int(x)` of a float: NaN and infinity raise as Python's do; a value past i64 saturates (Python's int grows).
fn to_int(x: f64) -> Result<i64, Fault> {
    if x.is_nan() {
        return Err(Fault::new(
            "ValueError",
            "cannot convert float NaN to integer",
        ));
    }
    if x.is_infinite() {
        return Err(Fault::new(
            "OverflowError",
            "cannot convert float infinity to integer",
        ));
    }
    Ok(x.trunc() as i64)
}

/// `_fact`: what the worker sees: facts about the file, labeled outside content.
pub fn fact(row: &Map<String, Value>) -> Map<String, Value> {
    let keys = [
        ("id", "download"),
        ("state", "state"),
        ("tab", "tab"),
        ("path", "path"),
        ("name", "name"),
        ("sha256", "sha256"),
        ("mime", "mime"),
        ("source_url", "source_url"),
        ("reason", "reason"),
        ("trashed_to", "trashed_to"),
        ("origin", "origin"),
    ];
    let mut out = Map::new();
    for (src, k) in keys {
        match row.get(src) {
            None | Some(Value::Null) => {}
            Some(Value::String(s)) if s.is_empty() => {}
            Some(v) => {
                out.insert(k.into(), v.clone());
            }
        }
    }
    let state = row.get("state").and_then(Value::as_str).unwrap_or("");
    if let Some(b) = row.get("bytes").filter(|b| !b.is_null()) {
        out.insert(
            if state == "taken" { "bytes" } else { "size" }.into(),
            b.clone(),
        );
    }
    let kept = row.get("kept").and_then(Value::as_str);
    let path_set = row
        .get("path")
        .is_some_and(|p| !p.is_null() && p.as_str() != Some(""));
    if state == "taken" && !path_set {
        let words = match kept {
            Some("gone") => "already gone when retention ran",
            Some("deleted") => "deleted on the owner's Yes (the downloads folder was over its cap)",
            _ => "moved to the Trash (retention)",
        };
        out.insert("kept".into(), words.into());
    } else if let Some(k) = kept.filter(|k| k.starts_with("stuck") || k.starts_with("skipped")) {
        out.insert("kept".into(), k.into());
    }
    out
}

/// `available_bytes` (HAND-236, HAND-246): free bytes on the volume holding `where_`, as Finder counts them: its
/// capacity for important usage (`important`: the Mac's [`crate::macos::important_usage`]); statvfs free space when
/// that can't be read. `Err` is the `OSError`'s text.
#[cfg(unix)]
pub fn available_bytes(where_: &Path, important: Option<Important<'_>>) -> Result<i64, String> {
    if let Some(v) = important.and_then(|f| f(where_)) {
        return Ok(v);
    }
    let c = std::ffi::CString::new(where_.to_string_lossy().as_bytes())
        .map_err(|_| "embedded null byte".to_string())?;
    // SAFETY: a zeroed statvfs is a valid out-parameter; statvfs fills it for the NUL-terminated path.
    let mut s: libc::statvfs = unsafe { std::mem::zeroed() };
    // SAFETY: as above.
    if unsafe { libc::statvfs(c.as_ptr(), &mut s) } != 0 {
        let e = std::io::Error::last_os_error();
        return Err(format!(
            "[Errno {}] {}: {}",
            e.raw_os_error().unwrap_or(0),
            crate::upload::unix::strerror(&e),
            ferb_identity::py::repr(&where_.to_string_lossy())
        ));
    }
    #[allow(clippy::unnecessary_cast)] // the field types differ by platform
    Ok((s.f_bavail as i64) * (s.f_frsize as i64))
}

// ---------- the shell's side (HAND-231, HAND-232) ----------

/// What ferb-shell's download watch calls each second for a command that received bytes through the egress proxy:
/// count them against the task tree's budget, then ask whether the command must stop (the daemon's
/// `_shell_received`).
pub trait ShellBudget: Send + Sync {
    /// `shell_received(task, n)`.
    fn received(&self, task: &str, n: i64) -> Result<(), Fault>;
    /// `shell_stop(task)`: why the command must stop now, as a fact for the worker; `None`: it may go on.
    fn stop(&self, task: &str) -> Result<Option<String>, Fault>;
}

/// The volume's capacity for important usage (the Mac's [`crate::macos::important_usage`]).
pub type Important<'a> = &'a dyn Fn(&Path) -> Option<i64>;
/// `stopped(task, why)`: a command FERB stopped, for the journal.
pub type Stopped = Arc<dyn Fn(&str, &str) + Send + Sync>;
/// The hub's `on_download` hook.
pub type OnDownload = Arc<dyn Fn(&Arc<Bridge>, &Event) + Send + Sync>;

/// ferb-shell's `OnReceived` (`manager::OnReceived`, the same type): bytes a command of a task received, and why
/// it must stop now.
pub type OnReceived = Arc<dyn Fn(&str, i64) -> Option<String> + Send + Sync>;

impl Downloads {
    /// The daemon's `_shell_received` for ferb-shell's download watch (HAND-210, HAND-232): the bytes count against
    /// the task tree's budget; the answer is why the command must stop, journaled through `stopped(task, why)`
    /// (`shell.download_stopped`). A count that fails stops nothing, as ferb-shell treats a broken count.
    pub fn shell_hook(self: &Arc<Self>, stopped: Stopped) -> OnReceived {
        let me = Arc::clone(self);
        Arc::new(move |task: &str, n: i64| {
            me.shell_received(task, n).ok()?;
            let why = me.shell_stop(task).ok()??;
            stopped(task, &why);
            Some(why)
        })
    }
}

impl ShellBudget for Downloads {
    fn received(&self, task: &str, n: i64) -> Result<(), Fault> {
        self.shell_received(task, n)
    }
    fn stop(&self, task: &str) -> Result<Option<String>, Fault> {
        self.shell_stop(task)
    }
}

// ---------- Downloads ----------

#[cfg(unix)]
mod folder {
    use ferb_fs::sys::{self, At, Fd};

    /// `_open_folder`: the downloads folder, made when missing and opened without following a link.
    pub fn open(root: &str) -> Result<Fd, String> {
        mkdir_parents(root).map_err(|e| e.to_string())?;
        let fd = sys::open(
            At::Cwd,
            root,
            sys::O_RDONLY | sys::O_DIRECTORY | sys::O_NOFOLLOW,
            0,
        )
        .map_err(|e| e.to_string())?;
        let st = sys::fstat(fd.raw()).map_err(|e| e.to_string())?;
        if !st.is_dir() {
            return Err(sys::OsError::new(libc::ENOTDIR, Some(root)).to_string());
        }
        Ok(fd)
    }

    /// `Path.mkdir(parents=True, exist_ok=True)`.
    fn mkdir_parents(path: &str) -> sys::Res<()> {
        match sys::mkdir_at(At::Cwd, path, 0o777) {
            Ok(()) => Ok(()),
            Err(e) if e.is(libc::ENOENT) => {
                let parent = ferb_fs::pypath::parent(path);
                if parent == path {
                    return Err(e);
                }
                mkdir_parents(&parent)?;
                match sys::mkdir_at(At::Cwd, path, 0o777) {
                    Err(e) if !ferb_fs::pypath::is_dir(path) => Err(e),
                    _ => Ok(()),
                }
            }
            Err(e) if ferb_fs::pypath::is_dir(path) => {
                let _ = e;
                Ok(())
            }
            Err(e) => Err(e),
        }
    }
}

/// What a running transfer holds, released however it ends (a cancelled task included).
struct Transfer {
    me: Arc<Downloads>,
    tree: String,
    received: i64,
    fid: String,
    peer_name: String,
    tab_id: i64,
    #[cfg(unix)]
    folder: Option<ferb_fs::sys::Fd>,
    #[cfg(unix)]
    fd: Option<ferb_fs::sys::Fd>,
    moved: bool,
    /// Ended (recorded); a drop before this is a cancellation.
    ended: bool,
}

impl Drop for Transfer {
    fn drop(&mut self) {
        if !self.ended {
            // asyncio.CancelledError: recorded as failed, quietly
            let size = (self.received != 0).then_some(self.received);
            self.me.finish_quietly(
                &self.peer_name,
                self.tab_id,
                &self.fid,
                "failed",
                None,
                None,
                size,
                None,
                Some("FERB stopped before the transfer finished; nothing was kept"),
            );
        }
        {
            let mut s = lock(&self.me.state);
            let left = s.inflight.get(&self.tree).copied().unwrap_or(0) - self.received;
            s.inflight.insert(self.tree.clone(), left.max(0));
            let n = s.running.get(&self.tree).copied().unwrap_or(1) - 1;
            s.running.insert(self.tree.clone(), n);
            if n <= 0 {
                // only the last transfer of the tree clears its entries
                s.running.remove(&self.tree);
                s.inflight.remove(&self.tree);
            }
        }
        #[cfg(unix)]
        {
            self.fd.take();
            if let Some(folder) = self.folder.take() {
                if !self.moved && !self.fid.is_empty() {
                    let _ = ferb_fs::sys::unlink_at(
                        ferb_fs::sys::At::Dir(folder.raw()),
                        &format!(".{}.part", self.fid),
                    );
                }
            }
        }
    }
}

impl Downloads {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        root: PathBuf,
        records: Arc<dyn Records>,
        clock: Arc<dyn Clock>,
        setting: Setting,
        redact: Arc<dyn Fn(&str) -> String + Send + Sync>,
        publish: Publish,
        random: Arc<dyn Random>,
    ) -> Arc<Downloads> {
        let disk = root.clone();
        Arc::new(Downloads {
            root,
            records,
            clock,
            setting,
            redact,
            publish,
            random,
            tab_wait_ms: std::sync::atomic::AtomicU64::new((TAB_WAIT_S * 1000.0) as u64),
            hooks: Mutex::new(DlHooks {
                task_tab: Arc::new(|_, _| None),
                task_root: Arc::new(str::to_string),
                free_bytes: Arc::new(move || disk_free(&disk)),
                on_over_cap: Arc::new(|| {}),
            }),
            state: Mutex::default(),
            done: tokio::sync::Notify::new(),
        })
    }

    /// Replaces `TAB_WAIT_S` (a test scaling it to its clock).
    pub fn set_tab_wait(&self, seconds: f64) {
        self.tab_wait_ms.store(
            (seconds * 1000.0) as u64,
            std::sync::atomic::Ordering::Relaxed,
        );
    }

    /// Wires in the daemon's hooks.
    pub fn set_hooks(&self, hooks: DlHooks) {
        *lock(&self.hooks) = hooks;
    }

    pub fn hooks(&self) -> DlHooks {
        lock(&self.hooks).clone()
    }

    /// The hub's `on_download` hook: a download the extension holds is received from that bridge.
    pub fn on_download(self: &Arc<Self>) -> OnDownload {
        let me = Arc::clone(self);
        Arc::new(move |br: &Arc<Bridge>, ev: &Event| {
            me.receive(Arc::new(BridgePeer(Arc::clone(br))), ev.clone());
        })
    }

    fn redact(&self, text: &str) -> String {
        (self.redact)(text)
    }

    fn publish(&self, data: Map<String, Value>) {
        (self.publish)("browser.download", data)
    }

    fn root_text(&self) -> String {
        self.root.to_string_lossy().into_owned()
    }

    /// `limits`: the owner's limits; an old config's per-file or MB keys are not read. `Err` as Python raises (a
    /// config that can't be read, a limit that is infinite or NaN).
    pub fn limits(&self) -> Result<Limits, Fault> {
        let mut b = [0.0f64; 4];
        for (i, (k, v)) in DEFAULTS.iter().enumerate() {
            let value = (self.setting)("browser", k)?;
            b[i] = number(value.as_ref(), *v);
        }
        Ok(Limits {
            task_files: to_int(b[0])?,
            task_bytes: to_int(b[1] * 1e9)?,
            total_bytes: to_int(b[2] * 1e9)?,
            min_free_bytes: to_int(b[3] * 1e9)?,
        })
    }

    /// `_free`: free bytes now, for the floor. Unreadable: refused.
    fn free(&self, what: &str, size: Option<i64>) -> Result<i64, Stop> {
        let free = Arc::clone(&lock(&self.hooks).free_bytes);
        free().map_err(|e| {
            Stop::Refused(
                self.redact(&format!(
                    "couldn't read the free disk space to check browser.download_min_free_gb ({e}): {what}; not taken"
                )),
                size,
            )
        })
    }

    // ---------- the extension's download event ----------

    /// `receive`: a download the extension caught (held) or saw the browser start itself. Runs as its own task: its
    /// parts come back over the same connection the event arrived on. It never fails out of the task.
    pub fn receive(self: &Arc<Self>, peer: Arc<dyn Peer>, ev: Event) {
        let key = (peer.name(), ev.tab_id);
        let job = {
            let mut s = lock(&self.state);
            s.next_job += 1;
            let job = s.next_job;
            s.active.entry(key.clone()).or_default().insert(job);
            job
        };
        let me = Arc::clone(self);
        tokio::spawn(async move {
            struct Done(Arc<Downloads>, Key, u64);
            impl Drop for Done {
                fn drop(&mut self) {
                    let mut s = lock(&self.0.state);
                    if let Some(jobs) = s.active.get_mut(&self.1) {
                        jobs.remove(&self.2);
                        if jobs.is_empty() {
                            s.active.remove(&self.1);
                        }
                    }
                    drop(s);
                    self.0.done.notify_waiters();
                }
            }
            let _done = Done(Arc::clone(&me), key, job);
            me.guarded(peer, ev).await;
        });
    }

    async fn guarded(self: &Arc<Self>, peer: Arc<dyn Peer>, ev: Event) {
        if let Err(e) = self.receive_inner(&peer, &ev).await {
            // never an unretrieved task error; the event says why
            self.publish_error(&peer, &ev, &e);
        }
    }

    async fn receive_inner(
        self: &Arc<Self>,
        peer: &Arc<dyn Peer>,
        ev: &Event,
    ) -> Result<(), Fault> {
        let task_tab = Arc::clone(&lock(&self.hooks).task_tab);
        let uncaptured = ev.uncaptured == Some(true);
        let download_id = ev.download_id.clone().filter(|d| !d.is_empty());
        let Some(owner) = task_tab(peer.as_ref(), ev.tab_id) else {
            // not a live task tab: back to the browser, untouched
            if !uncaptured {
                if let Some(id) = &download_id {
                    send(peer, id, "release").await;
                }
            }
            let mut data = Map::new();
            data.insert("profile".into(), peer.name().into());
            data.insert("tab_id".into(), ev.tab_id.into());
            data.insert("state".into(), "released".into());
            data.insert(
                "reason".into(),
                "not a live task tab: left to the browser".into(),
            );
            self.publish(data);
            return Ok(());
        };
        if uncaptured && ev.detail.as_deref() == Some("dropped") {
            // over the extension's hold caps: neither FERB's nor the browser's
            let n = ev.count.filter(|c| *c != 0).unwrap_or(1);
            self.insert(
                peer,
                &owner,
                ev,
                "refused",
                None,
                Some(&dropped_reason(n)),
                true,
            )?;
            return Ok(());
        }
        if uncaptured {
            self.insert(
                peer,
                &owner,
                ev,
                "not_taken",
                None,
                Some(not_taken(ev.detail.as_deref())),
                true,
            )?;
            return Ok(());
        }
        let Some(id) = download_id else {
            return Ok(());
        };
        // From here the held download is a live task tab's: whatever breaks, it is aborted, never left to the
        // extension's idle hand-back.
        let key = (peer.name(), ev.tab_id);
        let tab_lock = {
            let mut s = lock(&self.state);
            s.tab_task.insert(key.clone(), owner.task.clone());
            Arc::clone(s.locks.entry(key).or_default())
        };
        let wait = std::time::Duration::from_millis(
            self.tab_wait_ms.load(std::sync::atomic::Ordering::Relaxed),
        );
        let Ok(guard) = tokio::time::timeout(wait, tab_lock.lock_owned()).await else {
            send(peer, &id, "abort").await;
            self.insert(
                peer,
                &owner,
                ev,
                "refused",
                ev.size,
                Some("another download from this tab was still being taken (one at a time per tab); not taken"),
                true,
            )?;
            return Ok(());
        };
        if let Some((lim, tree)) = self.admit(peer, &owner, ev, &id).await {
            self.take(peer, &owner, ev, &id, lim, tree).await;
        }
        drop(guard);
        Ok(())
    }

    /// `_not_admitted`: admission broke: abort first, then record the failure, best effort.
    async fn not_admitted(
        &self,
        peer: &Arc<dyn Peer>,
        owner: &TaskTab,
        ev: &Event,
        id: &str,
        e: &Fault,
    ) {
        send(peer, id, "abort").await;
        let reason = self.redact(&format!(
            "FERB couldn't check this download against its limits ({}); not taken",
            e.typed()
        ));
        if self
            .insert(peer, owner, ev, "failed", ev.size, Some(&reason), true)
            .is_err()
        {
            let mut data = Map::new();
            data.insert("profile".into(), peer.name().into());
            data.insert("tab_id".into(), ev.tab_id.into());
            data.insert("state".into(), "failed".into());
            data.insert("reason".into(), reason.into());
            self.publish(data);
        }
    }

    /// `_admit`: the budgets and the free-disk floor, before any byte moves.
    async fn admit(
        &self,
        peer: &Arc<dyn Peer>,
        owner: &TaskTab,
        ev: &Event,
        id: &str,
    ) -> Option<(Limits, String)> {
        let size = ev.size;
        let checked = (|| -> Result<Result<(Limits, String), Stop>, Fault> {
            let lim = self.limits()?;
            let tree = (Arc::clone(&lock(&self.hooks).task_root))(&owner.task);
            let (files, taken) = self.records.tree_usage(&tree)?;
            let used = taken + self.inflight(&tree) + self.shell_bytes(&tree)?;
            if files >= lim.task_files {
                return Ok(Err(Stop::Refused(
                    format!(
                        "this task already took {files} downloads, its limit (browser.download_task_max_files, per \
                         request); not taken"
                    ),
                    size,
                )));
            }
            if let Some(s) = size {
                if used + s > lim.task_bytes {
                    return Ok(Err(Stop::Refused(
                        format!(
                            "would pass this task's {} download budget (browser.download_task_max_gb, per request, \
                             browser and shell together; {used} bytes taken or arriving): {s} bytes; not taken",
                            gb(lim.task_bytes)
                        ),
                        size,
                    )));
                }
            }
            let what = match size {
                Some(s) => format!("this download is {s} bytes"),
                None => "its size isn't declared; nothing was read".to_string(),
            };
            let free = match self.free(&what, size) {
                Ok(f) => f,
                Err(stop) => return Ok(Err(stop)),
            };
            if free - size.unwrap_or(0) < lim.min_free_bytes {
                return Ok(Err(Stop::Refused(
                    low_disk(free, lim.min_free_bytes, &what),
                    size,
                )));
            }
            Ok(Ok((lim, tree)))
        })();
        match checked {
            Ok(Ok(admitted)) => Some(admitted),
            Ok(Err(Stop::Refused(reason, size))) => {
                send(peer, id, "abort").await;
                if let Err(e) = self.insert(peer, owner, ev, "refused", size, Some(&reason), true) {
                    // Python: the failed insert raised out of _admit's handler, into the task's guard
                    self.publish_error(peer, ev, &e);
                }
                None
            }
            Ok(Err(Stop::Failed(e))) => {
                self.not_admitted(peer, owner, ev, id, &Fault::new("OSError", &e))
                    .await;
                None
            }
            Err(e) => {
                self.not_admitted(peer, owner, ev, id, &e).await;
                None
            }
        }
    }

    fn publish_error(&self, peer: &Arc<dyn Peer>, ev: &Event, e: &Fault) {
        let mut data = Map::new();
        data.insert("profile".into(), peer.name().into());
        data.insert("tab_id".into(), ev.tab_id.into());
        data.insert("state".into(), "error".into());
        data.insert("reason".into(), self.redact(&e.typed()).into());
        self.publish(data);
    }

    fn inflight(&self, tree: &str) -> i64 {
        lock(&self.state).inflight.get(tree).copied().unwrap_or(0)
    }

    /// `_take`: the transfer, part by part, into a partial file, renamed to its final name once whole.
    #[cfg(unix)]
    async fn take(
        self: &Arc<Self>,
        peer: &Arc<dyn Peer>,
        owner: &TaskTab,
        ev: &Event,
        id: &str,
        lim: Limits,
        tree: String,
    ) {
        use ferb_fs::sys::{self, At};
        use sha2::Digest;
        let name = safe_name(&self.redact(ev.name.as_deref().unwrap_or("")));
        {
            let mut s = lock(&self.state);
            *s.running.entry(tree.clone()).or_insert(0) += 1;
            s.inflight.entry(tree.clone()).or_insert(0);
        }
        let mut t = Transfer {
            me: Arc::clone(self),
            tree: tree.clone(),
            received: 0,
            fid: String::new(),
            peer_name: peer.name(),
            tab_id: ev.tab_id,
            folder: None,
            fd: None,
            moved: false,
            ended: false,
        };
        let mut moved_at = self.clock.monotonic();
        let mut digest = sha2::Sha256::new();
        let result: Result<(), Stop> = async {
            t.fid = self
                .insert(peer, owner, ev, "in_progress", None, None, false)
                .map_err(|e| Stop::Failed(e.message))?;
            let final_name = format!("{}-{name}", t.fid);
            let temp = format!(".{}.part", t.fid);
            t.folder = Some(folder::open(&self.root_text()).map_err(Stop::Failed)?);
            let dir = t.folder.as_ref().map(|f| f.raw()).unwrap_or(-1);
            t.fd = Some(
                sys::open(
                    At::Dir(dir),
                    &temp,
                    sys::O_WRONLY | sys::O_CREAT | sys::O_EXCL | sys::O_NOFOLLOW,
                    0o600,
                )
                .map_err(|e| Stop::Failed(e.to_string()))?,
            );
            let stalled = |received: i64| {
                Stop::Refused(
                    format!(
                        "stalled: no bytes arrived for {} seconds (stopped after {received} bytes); not taken",
                        STALL_S as i64 // `{STALL_S:g}`
                    ),
                    Some(received),
                )
            };
            loop {
                let left = STALL_S - (self.clock.monotonic() - moved_at);
                if left <= 0.0 {
                    return Err(stalled(t.received));
                }
                let wait = left.max(1.0);
                let asked = self.clock.monotonic();
                let part = match peer
                    .call("download_part", json!({"download_id": id, "offset": t.received}), wait)
                    .await
                {
                    Ok(p) => p,
                    Err(e) => {
                        if self.clock.monotonic() - asked < wait {
                            return Err(Stop::Failed(e)); // it broke (not a wait that ran out)
                        }
                        return Err(stalled(t.received));
                    }
                };
                let in_order = part.as_object().is_some_and(|p| {
                    p.get("offset")
                        .is_some_and(|o| ferb_py::value::py_eq(o, &json!(t.received)))
                });
                if !in_order {
                    return Err(Stop::Failed(
                        "the browser sent download parts out of order".into(),
                    ));
                }
                let text = match part.get("data") {
                    None | Some(Value::Null) => String::new(),
                    Some(v) if !ferb_py::value::truthy(v) => String::new(),
                    Some(v) => ferb_py::value::py_str(v),
                };
                let data = decode(&text).map_err(Stop::Failed)?;
                let n = data.len() as i64;
                if n > 0 {
                    moved_at = self.clock.monotonic(); // progress: the stall clock starts again
                }
                t.received += n;
                *lock(&self.state).inflight.entry(tree.clone()).or_insert(0) += n;
                // what the tree took, what arrives, and what its shell commands received
                if self.used(&tree).map_err(|e| Stop::Failed(e.message))? > lim.task_bytes {
                    return Err(Stop::Refused(
                        format!(
                            "passed this task's {} download budget (browser.download_task_max_gb, per request, browser \
                             and shell together) after {} bytes; not taken",
                            gb(lim.task_bytes),
                            t.received
                        ),
                        Some(t.received),
                    ));
                }
                let written = t.received - n;
                let what = format!("stopped after {written} bytes");
                let free = self.free(&what, Some(written))?;
                if free - n < lim.min_free_bytes {
                    return Err(Stop::Refused(
                        low_disk(free, lim.min_free_bytes, &what),
                        Some(written),
                    ));
                }
                let fd = t.fd.as_ref().map(|f| f.raw()).unwrap_or(-1);
                sys::write_all(fd, &data).map_err(|e| Stop::Failed(e.to_string()))?;
                digest.update(&data);
                if part.get("eof").is_some_and(ferb_py::value::truthy) {
                    break;
                }
            }
            t.fd = None;
            match sys::stat_at(At::Dir(dir), &final_name, false) {
                Ok(_) => {
                    // a random id: never expected; never overwrite
                    return Err(Stop::Failed(
                        sys::OsError::new(libc::EEXIST, Some(&final_name)).to_string(),
                    ));
                }
                Err(e) if e.is(libc::ENOENT) => {}
                Err(e) => return Err(Stop::Failed(e.to_string())),
            }
            sys::rename_at(At::Dir(dir), &temp, At::Dir(dir), &final_name)
                .map_err(|e| Stop::Failed(e.to_string()))?;
            t.moved = true;
            let path = self.root.join(&final_name).to_string_lossy().into_owned();
            let sha = hex(&digest.clone().finalize());
            if let Err(e) = self.finish(
                &peer.name(),
                ev.tab_id,
                &t.fid,
                "taken",
                Some(&path),
                Some(&name),
                Some(t.received),
                Some(&sha),
                None,
            ) {
                // no record: no file (never a file FERB can't account for)
                let _ = sys::unlink_at(At::Dir(dir), &final_name);
                return Err(Stop::Failed(e.message));
            }
            self.evict(); // the folder's cap: the owner is asked when it is over
            Ok(())
        }
        .await;
        match result {
            Ok(()) => {}
            Err(Stop::Refused(reason, size)) => {
                send(peer, id, "abort").await;
                self.finish_quietly(
                    &peer.name(),
                    ev.tab_id,
                    &t.fid,
                    "refused",
                    None,
                    None,
                    size,
                    None,
                    Some(&reason),
                );
            }
            Err(Stop::Failed(e)) => {
                send(peer, id, "abort").await;
                let size = (t.received != 0).then_some(t.received);
                let reason = self.redact(&format!(
                    "the transfer didn't finish ({e}); nothing was kept"
                ));
                self.finish_quietly(
                    &peer.name(),
                    ev.tab_id,
                    &t.fid,
                    "failed",
                    None,
                    None,
                    size,
                    None,
                    Some(&reason),
                );
            }
        }
        t.ended = true;
        drop(t);
    }

    #[cfg(not(unix))]
    async fn take(
        self: &Arc<Self>,
        peer: &Arc<dyn Peer>,
        _owner: &TaskTab,
        ev: &Event,
        id: &str,
        _lim: Limits,
        _tree: String,
    ) {
        send(peer, id, "abort").await;
        self.publish_error(
            peer,
            ev,
            &Fault::new("OSError", "downloads need a Unix file system"),
        );
    }

    // ---------- records ----------

    /// `_used`: bytes counted against the tree's budget: files its tabs took, transfers still arriving, and what its
    /// shell commands received.
    fn used(&self, tree: &str) -> Result<i64, Fault> {
        let (_, taken) = self.records.tree_usage(tree)?;
        Ok(taken + self.inflight(tree) + self.shell_bytes(tree)?)
    }

    fn shell_bytes(&self, tree: &str) -> Result<i64, Fault> {
        if let Some(n) = lock(&self.state).shell.get(tree) {
            return Ok(*n);
        }
        let n = self.records.shell_bytes(tree)?.unwrap_or(0);
        lock(&self.state).shell.insert(tree.to_string(), n);
        Ok(n)
    }

    /// `shell_received`: a shell command of this task received `n` more bytes through FERB's egress proxy: counted
    /// against its tree's budget, kept so a restart doesn't start the tree afresh.
    pub fn shell_received(&self, task: &str, n: i64) -> Result<(), Fault> {
        if n <= 0 {
            return Ok(());
        }
        let tree = (Arc::clone(&lock(&self.hooks).task_root))(task);
        let total = self.shell_bytes(&tree)? + n;
        lock(&self.state).shell.insert(tree.clone(), total);
        let at = ferb_core::when::isoformat(&self.clock.now());
        self.records.set_shell(&tree, total, &at)
    }

    /// Forgets the shell counts held in memory (what a restart forgets; the rows stay).
    pub fn forget_shell_cache(&self) {
        lock(&self.state).shell.clear();
    }

    /// `shell_stop`: why a shell command of this task that is receiving bytes must stop now, as a fact for the
    /// worker; `None`: it may go on.
    pub fn shell_stop(&self, task: &str) -> Result<Option<String>, Fault> {
        let lim = self.limits()?;
        let tree = (Arc::clone(&lock(&self.hooks).task_root))(task);
        let used = self.used(&tree)?;
        if used > lim.task_bytes {
            return Ok(Some(format!(
                "FERB stopped this command: this task has received {} in downloads, past its {} budget \
                 (browser.download_task_max_gb, per request, browser and shell together). Don't start another \
                 download in this task; say what was and wasn't fetched.",
                gb(used),
                gb(lim.task_bytes)
            )));
        }
        let free = Arc::clone(&lock(&self.hooks).free_bytes);
        let free = match free() {
            Ok(f) => f,
            Err(e) => {
                return Ok(Some(self.redact(&format!(
                    "FERB stopped this command: it couldn't read the free disk space to check \
                     browser.download_min_free_gb ({e}) while the command was downloading."
                ))))
            }
        };
        if free < lim.min_free_bytes {
            return Ok(Some(format!(
                "FERB stopped this command: only {} free on the disk now; FERB keeps at least {} free \
                 (browser.download_min_free_gb) and the command was downloading. Tell the owner the disk is nearly \
                 full; don't retry the download.",
                gb(free),
                gb(lim.min_free_bytes)
            )));
        }
        Ok(None)
    }

    /// `forget_task`: task end: its tabs' locks (when idle) and unreported facts go (the records stay).
    pub fn forget_task(&self, task: &str) {
        let mut s = lock(&self.state);
        let keys: Vec<Key> = s
            .tab_task
            .iter()
            .filter(|(_, t)| *t == task)
            .map(|(k, _)| k.clone())
            .collect();
        for key in keys {
            if s.locks.get(&key).is_some_and(|l| l.try_lock().is_ok()) {
                s.locks.remove(&key);
            }
            if !s.active.contains_key(&key) {
                s.unreported.remove(&key);
                s.tab_task.remove(&key);
            }
        }
    }

    /// `finalize_stale`: at start-up a transfer a restart cut off can't still be running.
    pub fn finalize_stale(&self) -> Result<i64, Fault> {
        self.records.finalize_stale()
    }

    /// `_insert`: a record (and, when `final_`, its fact for the worker). The download's id.
    #[allow(clippy::too_many_arguments)]
    fn insert(
        &self,
        peer: &Arc<dyn Peer>,
        owner: &TaskTab,
        ev: &Event,
        state: &str,
        size: Option<i64>,
        reason: Option<&str>,
        final_: bool,
    ) -> Result<String, Fault> {
        let fid = format!("dl-{}", crate::pairing::token_hex(self.random.as_ref(), 6));
        let opt = |v: Option<String>| v.map_or(Value::Null, Value::from);
        let mut row = Map::new();
        row.insert("id".into(), fid.clone().into());
        row.insert("task_id".into(), owner.task.clone().into());
        row.insert("profile".into(), peer.name().into());
        row.insert("identity".into(), owner.identity.clone().into());
        row.insert("tab".into(), owner.tab.clone().into());
        row.insert("state".into(), state.into());
        row.insert("path".into(), Value::Null);
        row.insert(
            "name".into(),
            opt(ev
                .name
                .as_deref()
                .filter(|n| !n.is_empty())
                .map(|n| safe_name(&self.redact(n)))),
        );
        row.insert("bytes".into(), size.map_or(Value::Null, Value::from));
        row.insert("sha256".into(), Value::Null);
        row.insert(
            "mime".into(),
            opt(ev.mime.clone().filter(|m| !m.is_empty())),
        );
        row.insert(
            "source_url".into(),
            opt(Some(self.redact(ev.source_url.as_deref().unwrap_or(""))).filter(|s| !s.is_empty())),
        );
        row.insert("reason".into(), opt(reason.map(String::from)));
        row.insert("origin".into(), ORIGIN.into());
        row.insert(
            "at".into(),
            ferb_core::when::isoformat(&self.clock.now()).into(),
        );
        self.records.insert(&row)?;
        if final_ {
            self.report(&peer.name(), ev.tab_id, &row);
        }
        Ok(fid)
    }

    #[allow(clippy::too_many_arguments)]
    fn finish(
        &self,
        profile: &str,
        tab_id: i64,
        fid: &str,
        state: &str,
        path: Option<&str>,
        name: Option<&str>,
        size: Option<i64>,
        sha256: Option<&str>,
        reason: Option<&str>,
    ) -> Result<(), Fault> {
        let row = self
            .records
            .finish(fid, state, path, name, size, sha256, reason)?;
        self.report(profile, tab_id, &row);
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn finish_quietly(
        &self,
        profile: &str,
        tab_id: i64,
        fid: &str,
        state: &str,
        path: Option<&str>,
        name: Option<&str>,
        size: Option<i64>,
        sha256: Option<&str>,
        reason: Option<&str>,
    ) {
        if fid.is_empty() {
            return; // the in_progress row was never written
        }
        let _ = self.finish(
            profile, tab_id, fid, state, path, name, size, sha256, reason,
        );
    }

    fn report(&self, profile: &str, tab_id: i64, row: &Map<String, Value>) {
        let fact = fact(row);
        lock(&self.state)
            .unreported
            .entry((profile.to_string(), tab_id))
            .or_default()
            .push(fact.clone());
        let mut data = Map::new();
        data.insert("profile".into(), profile.into());
        data.insert(
            "task_id".into(),
            row.get("task_id").cloned().unwrap_or(Value::Null),
        );
        data.extend(fact);
        self.publish(data);
    }

    /// `unreported`: downloads from this tab the worker hasn't seen in a result yet (each final state once per tab).
    pub fn unreported_of(&self, profile: &str, chrome_id: i64) -> Vec<Map<String, Value>> {
        lock(&self.state)
            .unreported
            .remove(&(profile.to_string(), chrome_id))
            .unwrap_or_default()
    }

    /// `busy` (HAND-511): a download this tab started is still being taken.
    pub fn busy_in(&self, profile: &str, chrome_id: i64) -> bool {
        lock(&self.state)
            .active
            .get(&(profile.to_string(), chrome_id))
            .is_some_and(|j| !j.is_empty())
    }

    /// `settle`: wait (at most `timeout` seconds) for the downloads this tab started to finish taking.
    pub async fn settle_in(&self, profile: &str, chrome_id: i64, timeout: f64) {
        let key = (profile.to_string(), chrome_id);
        let jobs: HashSet<u64> = lock(&self.state)
            .active
            .get(&key)
            .cloned()
            .unwrap_or_default();
        if jobs.is_empty() {
            return;
        }
        let wait = async {
            loop {
                let notified = self.done.notified();
                let still = lock(&self.state)
                    .active
                    .get(&key)
                    .is_some_and(|now| now.iter().any(|j| jobs.contains(j)));
                if !still {
                    return;
                }
                notified.await;
            }
        };
        let _ =
            tokio::time::timeout(std::time::Duration::from_secs_f64(timeout.max(0.0)), wait).await;
    }

    /// Waits until no transfer runs in any tab (tests; the daemon's shutdown).
    pub async fn idle(&self) {
        loop {
            let notified = self.done.notified();
            if lock(&self.state).active.is_empty() {
                return;
            }
            notified.await;
        }
    }

    /// `facts_for`: every download of these tasks (in this identity's profiles), oldest first.
    pub fn facts_for(
        &self,
        tasks: &[String],
        identity: Option<&str>,
    ) -> Result<Vec<Map<String, Value>>, Fault> {
        if tasks.is_empty() {
            return Ok(vec![]);
        }
        let mut sorted: Vec<String> = tasks.to_vec();
        sorted.sort();
        sorted.dedup();
        Ok(self
            .records
            .of_tasks(&sorted, identity)?
            .iter()
            .map(fact)
            .collect())
    }

    /// The running transfers' counts per tree (`_inflight`, `_running`) and the tabs remembered (tests).
    pub fn counters(&self) -> Value {
        let s = lock(&self.state);
        let sorted = |m: &HashMap<String, i64>| -> Map<String, Value> {
            let mut v: Vec<_> = m.iter().collect();
            v.sort();
            v.into_iter().map(|(k, n)| (k.clone(), json!(n))).collect()
        };
        let keys = |m: Vec<&Key>| -> Vec<String> {
            let mut v: Vec<String> = m.into_iter().map(|(p, t)| format!("{p}#{t}")).collect();
            v.sort();
            v
        };
        json!({"inflight": sorted(&s.inflight), "running": sorted(&s.running),
               "locks": keys(s.locks.keys().collect()), "unreported": keys(s.unreported.keys().collect()),
               "tab_task": keys(s.tab_task.keys().collect())})
    }

    // ---------- the folder's total: over the cap the owner is asked; their Yes deletes the oldest for good ----------

    /// `_in_folder`: the rows of files kept in the downloads folder, oldest first. A row whose file isn't in the
    /// folder is left as it is, recorded once as skipped, and never counted.
    fn in_folder(&self) -> Result<Vec<Row>, Fault> {
        let root = ferb_fs::pypath::norm(&self.root_text());
        let mut kept = vec![];
        for r in self.records.with_path()? {
            let path = ferb_py::value::py_str(r.get("path").unwrap_or(&Value::Null));
            if ferb_fs::pypath::parent(&path) == root {
                kept.push(r);
            } else if r.get("kept").and_then(Value::as_str) != Some(OUTSIDE) {
                let id = ferb_py::value::py_str(r.get("id").unwrap_or(&Value::Null));
                self.records.set_kept(&id, OUTSIDE)?;
            }
        }
        Ok(kept)
    }

    /// `over_cap` (HAND-211): the folder over `browser.download_total_gb`: the oldest files whose permanent deletion
    /// brings it back under; `None` while it is within. Nothing is changed: the owner decides (the daemon asks).
    pub fn over_cap(&self) -> Result<Option<OverCap>, Fault> {
        let lim = self.limits()?;
        let rows = self.in_folder()?;
        let bytes = |r: &Row| r.get("bytes").and_then(Value::as_i64).unwrap_or(0);
        let total: i64 = rows.iter().map(bytes).sum();
        if total <= lim.total_bytes {
            return Ok(None);
        }
        let (mut doomed, mut left) = (vec![], total);
        for r in &rows {
            if left <= lim.total_bytes {
                break;
            }
            doomed.push(r);
            left -= bytes(r);
        }
        Ok(Some(OverCap {
            ids: doomed
                .iter()
                .map(|r| ferb_py::value::py_str(r.get("id").unwrap_or(&Value::Null)))
                .collect(),
            bytes: doomed.iter().map(|r| bytes(r)).sum(),
            total,
            cap: lim.total_bytes,
        }))
    }

    /// `delete` (HAND-211, HAND-242): the owner's Yes: these downloads' files deleted for good, through the open
    /// downloads folder and only there: a regular file in it, never a link. Each row stays with its path cleared.
    /// (files deleted, their bytes).
    #[cfg(unix)]
    pub fn delete(&self, ids: &[String]) -> Result<(i64, i64), Fault> {
        use ferb_fs::sys::{self, At};
        let mut wanted: Vec<&String> = vec![];
        for i in ids {
            if !wanted.contains(&i) {
                wanted.push(i);
            }
        }
        if wanted.is_empty() {
            return Ok((0, 0));
        }
        let Ok(folder) = folder::open(&self.root_text()) else {
            return Ok((0, 0));
        };
        let (mut count, mut size) = (0, 0);
        for r in self.in_folder()? {
            let id = ferb_py::value::py_str(r.get("id").unwrap_or(&Value::Null));
            if !wanted.contains(&&id) {
                continue;
            }
            let path = ferb_py::value::py_str(r.get("path").unwrap_or(&Value::Null));
            let name = ferb_fs::pypath::name(&path);
            let st = match sys::stat_at(At::Dir(folder.raw()), &name, false) {
                Ok(st) => Some(st),
                Err(e) if e.is(libc::ENOENT) => None,
                Err(e) => return Err(Fault::new("OSError", &e.to_string())),
            };
            let Some(st) = st.filter(|s| s.is_reg()) else {
                self.records.clear_path(&id, "gone")?;
                continue;
            };
            if let Err(e) = sys::unlink_at(At::Dir(folder.raw()), &name) {
                let stuck = self.redact(&format!("stuck: couldn't delete it ({e})"));
                self.records
                    .set_kept(&id, ferb_py::value::prefix(&stuck, 300))?;
                continue;
            }
            self.records.clear_path(&id, "deleted")?;
            count += 1;
            size += st.size;
        }
        Ok((count, size))
    }

    /// `_evict`: after a file is taken the daemon checks the folder's cap (and asks the owner when it is over).
    fn evict(&self) {
        let hook = Arc::clone(&lock(&self.hooks).on_over_cap);
        hook(); // a failure: the next retention pass checks again
    }

    /// `sweep`: at start-up the partial files of transfers that never finished (FERB's own `.dl-*.part`) go.
    pub fn sweep(&self) -> Result<usize, String> {
        let root = &self.root;
        let Ok(meta) = std::fs::symlink_metadata(root) else {
            return Ok(0);
        };
        if !meta.is_dir() {
            return Ok(0);
        }
        let mut gone = 0;
        let entries = std::fs::read_dir(root).map_err(|e| e.to_string())?;
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if name.starts_with(".dl-")
                && name.ends_with(".part")
                && std::fs::symlink_metadata(e.path()).is_ok_and(|m| m.is_file())
            {
                std::fs::remove_file(e.path()).map_err(|e| e.to_string())?;
                gone += 1;
            }
        }
        Ok(gone)
    }
}

/// `_disk_free`: free bytes on the disk that holds the downloads folder (its nearest existing folder).
fn disk_free(root: &Path) -> Result<i64, String> {
    let mut at = root.to_path_buf();
    while !at.exists() {
        match at.parent() {
            Some(p) if p != at => at = p.to_path_buf(),
            _ => break,
        }
    }
    #[cfg(target_os = "macos")]
    let important: Option<Important<'_>> = Some(&crate::macos::important_usage);
    #[cfg(not(target_os = "macos"))]
    let important: Option<Important<'_>> = None;
    #[cfg(unix)]
    return available_bytes(&at, important);
    #[cfg(not(unix))]
    {
        let _ = important;
        Err("free space can't be read here".into())
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// `base64.b64decode(text, validate=True)`.
fn decode(text: &str) -> Result<Vec<u8>, String> {
    if text.is_empty() {
        return Ok(vec![]);
    }
    ferb_identity::py::b64decode(text).ok_or_else(|| {
        if text
            .chars()
            .any(|c| !(c.is_ascii_alphanumeric() || matches!(c, '+' | '/' | '=')))
        {
            "Only base64 data is allowed".to_string()
        } else {
            "Incorrect padding".to_string()
        }
    })
}

/// `_send`: tell the extension what becomes of a held download: abort (failed in the browser: no dialog) or release
/// (back to the browser untouched). Best effort.
async fn send(peer: &Arc<dyn Peer>, download_id: &str, flag: &str) {
    if download_id.is_empty() {
        return;
    }
    let mut args = Map::new();
    args.insert("download_id".into(), download_id.into());
    args.insert(flag.into(), true.into());
    let _ = peer.call("download_part", Value::Object(args), 10.0).await;
}

impl crate::ops::DownloadsView for Downloads {
    fn busy(&self, br: &Arc<Bridge>, chrome_id: i64) -> bool {
        self.busy_in(&br.name(), chrome_id)
    }
    fn settle(&self, br: &Arc<Bridge>, chrome_id: i64, timeout: f64) -> BoxFuture<'_, ()> {
        let name = br.name();
        Box::pin(async move { self.settle_in(&name, chrome_id, timeout).await })
    }
    fn unreported(&self, br: &Arc<Bridge>, chrome_id: i64) -> Vec<Map<String, Value>> {
        self.unreported_of(&br.name(), chrome_id)
    }
    fn facts_for(&self, tasks: &[String], identity: &str) -> Vec<Map<String, Value>> {
        Downloads::facts_for(self, tasks, Some(identity)).unwrap_or_default()
    }
    fn task_bytes(&self) -> i64 {
        self.limits().map_or(32 * GB, |l| l.task_bytes)
    }
    fn total_bytes(&self) -> i64 {
        self.limits().map_or(20 * GB, |l| l.total_bytes)
    }
}

/// The store's `downloads` and `shell_downloads` tables (the Mac's SQLite store).
#[cfg(target_os = "macos")]
pub mod sqlite {
    use std::sync::{Mutex, PoisonError};

    use rusqlite::types::{Value as Sql, ValueRef};
    use rusqlite::{params, params_from_iter, Connection, OptionalExtension};
    use serde_json::{Map, Value};

    use super::{Fault, Records, Row};

    /// One connection to FERB's store, shared under a lock.
    pub struct SqliteRecords {
        conn: Mutex<Connection>,
    }

    impl SqliteRecords {
        pub fn new(conn: Connection) -> SqliteRecords {
            SqliteRecords {
                conn: Mutex::new(conn),
            }
        }

        fn conn(&self) -> std::sync::MutexGuard<'_, Connection> {
            self.conn.lock().unwrap_or_else(PoisonError::into_inner)
        }
    }

    /// sqlite3's errors read as Python's `sqlite3.OperationalError` (their class differs at times: the text is
    /// rusqlite's).
    fn err(e: rusqlite::Error) -> Fault {
        Fault::new("OperationalError", &e.to_string())
    }

    fn to_sql(v: &Value) -> Sql {
        match v {
            Value::Null => Sql::Null,
            Value::Bool(b) => Sql::Integer(i64::from(*b)),
            Value::Number(n) => match n.as_i64() {
                Some(i) => Sql::Integer(i),
                None => Sql::Real(n.as_f64().unwrap_or(0.0)),
            },
            Value::String(s) => Sql::Text(s.clone()),
            other => Sql::Text(other.to_string()),
        }
    }

    fn rows(conn: &Connection, sql: &str, args: Vec<Sql>) -> Result<Vec<Row>, Fault> {
        let mut st = conn.prepare(sql).map_err(err)?;
        let names: Vec<String> = st.column_names().map(|s| s.to_string()).collect();
        let mut out = vec![];
        let mut q = st.query(params_from_iter(args)).map_err(err)?;
        while let Some(r) = q.next().map_err(err)? {
            let mut m = Map::new();
            for (i, n) in names.iter().enumerate() {
                let v = match r.get_ref(i).map_err(err)? {
                    ValueRef::Null => Value::Null,
                    ValueRef::Integer(i) => Value::from(i),
                    ValueRef::Real(f) => Value::from(f),
                    ValueRef::Text(t) => Value::from(String::from_utf8_lossy(t).into_owned()),
                    ValueRef::Blob(b) => Value::from(ferb_identity::py::b64encode(b)),
                };
                m.insert(n.clone(), v);
            }
            out.push(m);
        }
        Ok(out)
    }

    impl Records for SqliteRecords {
        fn insert(&self, row: &Row) -> Result<(), Fault> {
            let cols: Vec<&str> = row.keys().map(String::as_str).collect();
            let marks = vec!["?"; cols.len()].join(", ");
            let sql = format!(
                "INSERT INTO downloads ({}) VALUES ({marks})",
                cols.join(", ")
            );
            self.conn()
                .execute(&sql, params_from_iter(row.values().map(to_sql)))
                .map(|_| ())
                .map_err(err)
        }

        fn finish(
            &self,
            id: &str,
            state: &str,
            path: Option<&str>,
            name: Option<&str>,
            size: Option<i64>,
            sha256: Option<&str>,
            reason: Option<&str>,
        ) -> Result<Row, Fault> {
            let conn = self.conn();
            conn.execute(
                "UPDATE downloads SET state = ?, path = ?, name = COALESCE(?, name), bytes = ?, sha256 = ?, reason = ? \
                 WHERE id = ?",
                params![state, path, name, size, sha256, reason, id],
            )
            .map_err(err)?;
            rows(
                &conn,
                "SELECT * FROM downloads WHERE id = ?",
                vec![Sql::Text(id.into())],
            )?
            .pop()
            .ok_or_else(|| Fault::new("TypeError", "'NoneType' object is not iterable"))
        }

        fn tree_usage(&self, tree: &str) -> Result<(i64, i64), Fault> {
            self.conn()
                .query_row(
                    "SELECT count(*) AS n, COALESCE(sum(CASE WHEN state = 'taken' THEN bytes END), 0) AS b FROM \
                     downloads WHERE (task_id = ? OR (task_id >= ? || '.' AND task_id < ? || '/')) AND state IN \
                     ('taken', 'in_progress')",
                    params![tree, tree, tree],
                    |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)),
                )
                .map_err(err)
        }

        fn shell_bytes(&self, tree: &str) -> Result<Option<i64>, Fault> {
            self.conn()
                .query_row(
                    "SELECT bytes FROM shell_downloads WHERE tree_id = ?",
                    params![tree],
                    |r| r.get::<_, i64>(0),
                )
                .optional()
                .map_err(err)
        }

        fn set_shell(&self, tree: &str, bytes: i64, at: &str) -> Result<(), Fault> {
            self.conn()
                .execute(
                    "INSERT INTO shell_downloads (tree_id, bytes, at) VALUES (?, ?, ?) ON CONFLICT(tree_id) DO UPDATE \
                     SET bytes = excluded.bytes, at = excluded.at",
                    params![tree, bytes, at],
                )
                .map(|_| ())
                .map_err(err)
        }

        fn finalize_stale(&self) -> Result<i64, Fault> {
            self.conn()
                .execute(
                    "UPDATE downloads SET state = 'failed', reason = 'FERB restarted before the transfer finished; \
                     nothing was kept' WHERE state = 'in_progress'",
                    [],
                )
                .map(|n| n as i64)
                .map_err(err)
        }

        fn with_path(&self) -> Result<Vec<Row>, Fault> {
            rows(
                &self.conn(),
                "SELECT id, path, bytes, at, kept FROM downloads WHERE path IS NOT NULL ORDER BY at, rowid",
                vec![],
            )
        }

        fn set_kept(&self, id: &str, kept: &str) -> Result<(), Fault> {
            self.conn()
                .execute(
                    "UPDATE downloads SET kept = ? WHERE id = ?",
                    params![kept, id],
                )
                .map(|_| ())
                .map_err(err)
        }

        fn clear_path(&self, id: &str, kept: &str) -> Result<(), Fault> {
            self.conn()
                .execute(
                    "UPDATE downloads SET path = NULL, kept = ? WHERE id = ?",
                    params![kept, id],
                )
                .map(|_| ())
                .map_err(err)
        }

        fn of_tasks(&self, tasks: &[String], identity: Option<&str>) -> Result<Vec<Row>, Fault> {
            let marks = vec!["?"; tasks.len()].join(", ");
            let mut sql = format!("SELECT * FROM downloads WHERE task_id IN ({marks})");
            let mut args: Vec<Sql> = tasks.iter().map(|t| Sql::Text(t.clone())).collect();
            if let Some(i) = identity {
                sql.push_str(" AND identity = ?");
                args.push(Sql::Text(i.into()));
            }
            sql.push_str(" ORDER BY at, rowid");
            rows(&self.conn(), &sql, args)
        }
    }
}
