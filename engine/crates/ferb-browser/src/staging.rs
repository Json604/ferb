//! FERB's own copies of the files it gives a browser to upload: a port of `ferb/act/browser/uploads.py`'s
//! `Uploads` (rule 17, MP2; decisions: HAND-500, HAND-501). It must not choose a file or change the original.
//!
//! A browser reading a file in a folder macOS protects makes macOS ask the owner in front of their work, so FERB
//! reads the original itself through the handle the driver opened (and checked against the protection lists, rule
//! 10: [`crate::upload::Files::identify`] with ferb-core's `PathCheck`, the seam ferb-fs's tools use), copies it into
//! its workspace (an APFS clone where the volume allows it: `ferb_fs::recovery::copy_into`) and gives the browser the
//! copy under the original's name. The copy must hash to the identity just read, so a file swapped or changed during
//! the copy is refused. A copy of outside content (under an untrusted root) is kept under `outside/`, itself an
//! untrusted root, so it keeps its origin label.
//!
//! FERB's shell can write the workspace, so each folder is held by a handle opened without following a link
//! (O_NOFOLLOW), from `uploads/` down; the copy is made into its folder's handle, never through a link at its name;
//! it must still be a regular file at the path given to the browser; every removal goes through the handle of the
//! folder it is in. Copies belong to the task tree and go when it ends, except one shown in a tab that outlives the
//! tree (Chromium reads it again at submit): it stays until that tab closes or the next start-up.
//!
//! Seams: the untrusted roots (a closure), the random source for a copy's folder (`secrets.token_hex(6)`), and
//! ferb-fs's hook points for the races the Python tests make: `"staging.copy"` right before the copy is made,
//! `"staging.copied"` right after.

use std::collections::{HashMap, HashSet};
use std::os::fd::RawFd;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use ferb_fs::pypath as pp;
use ferb_fs::sys::{self, At, Fd, OsError};

use crate::pairing::Random;

/// `FOLDER`: under FERB's workspace.
pub const FOLDER: &str = "uploads";
/// `OUTSIDE`: copies of outside content, an untrusted fs root (`downloads::untrusted_roots` adds it).
pub const OUTSIDE: &str = "outside";

const OPEN_DIR: i32 = sys::O_RDONLY | sys::O_DIRECTORY | sys::O_NOFOLLOW | sys::O_CLOEXEC;

/// A copy's folder under `uploads/`: `[outside,] tree, random`.
type Leaf = Vec<String>;

/// Why a staging, a cleanup or a sweep stopped, as Python's `str(e)`. `left` is `CleanupLeft.left`: what a cleanup
/// couldn't remove (the rest went); empty for any other error.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StagingError {
    pub message: String,
    /// The class Python raised: `StagingFailed`, `CleanupLeft` or an `OSError` subclass's name.
    pub class: String,
    pub left: Vec<String>,
}

impl std::fmt::Display for StagingError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for StagingError {}

fn failed(message: String) -> StagingError {
    StagingError {
        message,
        class: "StagingFailed".into(),
        left: vec![],
    }
}

fn left_error(left: Vec<String>) -> StagingError {
    StagingError {
        message: format!("FERB couldn't remove {}", left.join("; ")),
        class: "CleanupLeft".into(),
        left,
    }
}

/// Python's class for an `OSError` by its errno.
fn os_class(e: &OsError) -> &'static str {
    match e.errno {
        libc::ENOENT => "FileNotFoundError",
        libc::EEXIST => "FileExistsError",
        libc::ENOTDIR => "NotADirectoryError",
        libc::EISDIR => "IsADirectoryError",
        libc::EACCES | libc::EPERM => "PermissionError",
        _ => "OSError",
    }
}

fn os_error(e: OsError) -> StagingError {
    StagingError {
        message: e.to_string(),
        class: os_class(&e).into(),
        left: vec![],
    }
}

/// Inside: an `OSError`, `_NotOurs` (a link or a file where FERB's folder should be: its full path), or
/// `StagingFailed` (its words).
enum Fail {
    Os(OsError),
    NotOurs(String),
    Failed(String),
}

impl From<OsError> for Fail {
    fn from(e: OsError) -> Fail {
        Fail::Os(e)
    }
}

/// `_NotOurs`'s `str()`.
fn not_ours_text(path: &str) -> String {
    OsError {
        errno: libc::ENOTDIR,
        strerror: Some("not FERB's folder (a link or a file); nothing under it was touched".into()),
        filename: Some(path.into()),
        filename2: None,
    }
    .to_string()
}

impl Fail {
    fn into_error(self) -> StagingError {
        match self {
            Fail::Os(e) => os_error(e),
            Fail::NotOurs(p) => StagingError {
                message: not_ours_text(&p),
                class: "_NotOurs".into(),
                left: vec![],
            },
            Fail::Failed(m) => failed(m),
        }
    }
}

fn at(fd: Option<&Fd>) -> At {
    fd.map_or(At::Cwd, |f| At::Dir(f.raw()))
}

/// `_open`: a handle on the folder `name` (relative to `dir`, if given), never through a link at its last step.
fn open_dir(name: &str, dir: Option<&Fd>, shown: &str) -> Result<Fd, Fail> {
    sys::open(at(dir), name, OPEN_DIR, 0).map_err(|e| {
        if e.is(libc::ENOTDIR) || e.is(libc::ELOOP) {
            Fail::NotOurs(shown.into())
        } else {
            Fail::Os(e)
        }
    })
}

/// `_made`: `_open`, the folder made owner-only first when missing.
fn made(name: &str, dir: Option<&Fd>, shown: &str) -> Result<Fd, Fail> {
    match sys::mkdir_at(at(dir), name, 0o700) {
        Ok(()) => {}
        Err(e) if e.is(libc::EEXIST) => {}
        Err(e) => return Err(Fail::Os(e)),
    }
    open_dir(name, dir, shown)
}

/// `_existing`: `_open`, or `None` when the folder is missing.
fn existing(name: &str, dir: Option<&Fd>, shown: &str) -> Result<Option<Fd>, Fail> {
    match open_dir(name, dir, shown) {
        Ok(fd) => Ok(Some(fd)),
        Err(Fail::Os(e)) if e.is(libc::ENOENT) => Ok(None),
        Err(e) => Err(e),
    }
}

fn rmdir_at(dir: RawFd, name: &str) -> sys::Res<()> {
    let c = std::ffi::CString::new(name).map_err(|_| OsError::plain("embedded null byte"))?;
    // SAFETY: a valid folder handle and C string.
    if unsafe { libc::unlinkat(dir, c.as_ptr(), libc::AT_REMOVEDIR) } != 0 {
        return Err(OsError::last(Some(name)));
    }
    Ok(())
}

/// `shutil.rmtree(name, dir_fd=dir, onexc=...)` (Python 3.12's `_rmtree_safe_fd`): a folder and all it holds,
/// through handles, never following a link; each failure is handed to `onexc` with the path relative to `dir`
/// (`name/...`) and the rest still goes.
fn rmtree_at(dir: RawFd, name: &str, onexc: &mut dyn FnMut(&str, &OsError)) {
    enum Step {
        Lstat(RawFd, String, Option<String>),
        Close(RawFd),
        Rmdir(RawFd, String, String),
    }
    let mut stack = vec![Step::Lstat(dir, name.to_string(), None)];
    while let Some(step) = stack.pop() {
        match step {
            Step::Close(fd) => {
                // SAFETY: a handle this walk opened, closed once.
                unsafe { libc::close(fd) };
            }
            Step::Rmdir(parent, path, entry) => {
                if let Err(e) = rmdir_at(parent, &entry) {
                    onexc(&path, &e);
                }
            }
            Step::Lstat(parent, path, entry) => {
                let entry_name = entry.unwrap_or_else(|| path.clone());
                let walked = (|| -> Result<(), OsError> {
                    let orig = sys::stat_at(At::Dir(parent), &entry_name, false)?;
                    let top = sys::open(
                        At::Dir(parent),
                        &entry_name,
                        sys::O_RDONLY | sys::O_NONBLOCK,
                        0,
                    )?;
                    let topfd = top.raw();
                    std::mem::forget(top); // closed by its Close step
                    let same = sys::fstat(topfd).map(|s| s.id() == orig.id());
                    let ok = matches!(same, Ok(true));
                    if ok {
                        stack.push(Step::Rmdir(parent, path.clone(), entry_name.clone()));
                    }
                    stack.push(Step::Close(topfd));
                    match same {
                        Err(e) => return Err(e),
                        Ok(false) => {
                            return Err(OsError::plain("Cannot call rmtree on a symbolic link"))
                        }
                        Ok(true) => {}
                    }
                    for e in sys::scandir(topfd)? {
                        let full = pp::join(&path, &e.name);
                        let is_dir = if e.dtype == libc::DT_UNKNOWN {
                            sys::stat_at(At::Dir(topfd), &e.name, false).is_ok_and(|s| s.is_dir())
                        } else {
                            e.dtype == libc::DT_DIR
                        };
                        if is_dir {
                            stack.push(Step::Lstat(topfd, full, Some(e.name)));
                            continue;
                        }
                        if let Err(err) = sys::unlink_at(At::Dir(topfd), &e.name) {
                            onexc(&full, &err);
                        }
                    }
                    Ok(())
                })();
                if let Err(e) = walked {
                    onexc(&path, &e);
                }
            }
        }
    }
}

/// `getattr(exc, 'strerror', exc)`: an error's own words (`None` for one without an errno, as Python prints it).
fn strerror_attr(e: &OsError) -> String {
    match &e.strerror {
        Some(s) => s.clone(),
        None if e.errno == -1 => "None".into(),
        None => e.to_string(),
    }
}

/// `_remove`: one entry of a folder FERB holds open, removed through that handle: a folder with all it holds, anything
/// else unlinked, a link never followed. What can't be removed is added to `left` and the rest still goes. 1 when the
/// entry went.
fn remove(dir: &Fd, name: &str, shown: &str, left: &mut Vec<String>) -> usize {
    match sys::stat_at(At::Dir(dir.raw()), name, false) {
        Err(e) if e.is(libc::ENOENT) => return 0,
        Err(e) => {
            left.push(format!("{shown}: {}", e.strerror_or_str()));
            return 0;
        }
        Ok(st) if !st.is_dir() => {
            return match sys::unlink_at(At::Dir(dir.raw()), name) {
                Ok(()) => 1,
                Err(e) if e.is(libc::ENOENT) => 0,
                Err(e) => {
                    left.push(format!("{shown}: {}", e.strerror_or_str()));
                    0
                }
            };
        }
        Ok(_) => {}
    }
    let parent = pp::parent(shown);
    let mut failed: Vec<String> = vec![];
    rmtree_at(dir.raw(), name, &mut |path, e| {
        if !e.is(libc::ENOENT) {
            failed.push(format!("{}: {}", pp::join(&parent, path), strerror_attr(e)));
        }
    });
    let gone = usize::from(failed.is_empty());
    left.extend(failed);
    gone
}

/// `_names`: `copy`, resolved as the browser will, still names the regular file made in the held folder.
fn names(copy: &str, leaf: &Fd, name: &str) -> bool {
    let Ok(made) = sys::stat_at(At::Dir(leaf.raw()), name, false) else {
        return false;
    };
    made.is_reg() && sys::lstat(copy).is_ok_and(|s| s.id() == made.id())
}

/// `_tree_name`: a plain folder name a task tree can have here.
fn tree_name(tree: &str) -> bool {
    !tree.is_empty() && pp::name(tree) == tree && !tree.starts_with('.') && tree != OUTSIDE
}

/// `uploads.Uploads`.
pub struct Uploads {
    root: String,
    home: String,
    untrusted: Arc<dyn Fn() -> Vec<String> + Send + Sync>,
    random: Arc<dyn Random>,
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    /// tab -> copies it may still show (re-read at submit)
    shown: HashMap<String, HashSet<Leaf>>,
    /// trees that ended while a tab still showed a copy of theirs
    ended: HashSet<String>,
}

impl Uploads {
    /// `root`: `<workspace>/uploads`; `home`: the owner's home (`~` in an untrusted root).
    pub fn new(
        root: &str,
        home: &str,
        untrusted: Arc<dyn Fn() -> Vec<String> + Send + Sync>,
        random: Arc<dyn Random>,
    ) -> Uploads {
        Uploads {
            root: root.to_string(),
            home: home.to_string(),
            untrusted,
            random,
            state: Mutex::default(),
        }
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// `_outside`: the source is under an untrusted root.
    fn outside(&self, source: &str) -> bool {
        (self.untrusted)().iter().any(|u| {
            let Ok(ur) = pp::resolve(&pp::expanduser(u, &self.home)) else {
                return false;
            };
            source == ur || pp::parents(source).contains(&ur)
        })
    }

    /// `stage_all`: each (open file, its real path, its SHA-256) staged; when one fails, the copies already made go
    /// at once.
    pub fn stage_all(
        &self,
        tree: &str,
        sources: &[(RawFd, String, String)],
        tab: Option<&str>,
    ) -> Result<Vec<String>, StagingError> {
        let mut made: Vec<String> = vec![];
        for (fd, source, sha256) in sources {
            match self.stage_from(tree, *fd, source, sha256, tab) {
                Ok(copy) => made.push(copy),
                Err(e) => {
                    self.discard(&made);
                    return Err(e);
                }
            }
        }
        Ok(made)
    }

    /// `stage_from`: copy the open file `src_fd` (`source`: its real path) to `<root>[/outside]/<tree>/<random>/<its
    /// name>` and return the copy's path, which must still name the copy, a regular file. Anything already at that name
    /// refuses the copy; a copy that doesn't hash to `sha256` is removed and refused. `tab` (`tab_key`) keeps it past
    /// its tree while that tab is open. Blocking: callers on the event loop run it off the loop.
    pub fn stage_from(
        &self,
        tree: &str,
        src_fd: RawFd,
        source: &str,
        sha256: &str,
        tab: Option<&str>,
    ) -> Result<String, StagingError> {
        if !tree_name(tree) {
            return Err(failed(
                "no task owns this tab, so FERB has nowhere to keep its copy".into(),
            ));
        }
        let src_name = pp::name(source);
        let mut parts: Vec<String> = if self.outside(source) {
            vec![OUTSIDE.into(), tree.into()]
        } else {
            vec![tree.into()]
        };
        mkdir_parents(&pp::parent(&self.root)).map_err(os_error)?;
        let staged = (|| -> Result<(String, String), Fail> {
            let mut shown = self.root.clone();
            let mut held = made(&self.root, None, &shown)?;
            for name in &parts {
                shown = pp::join(&shown, name);
                held = made(name, Some(&held), &shown)?;
            }
            let leaf = crate::pairing::token_hex(self.random.as_ref(), 6);
            sys::mkdir_at(At::Dir(held.raw()), &leaf, 0o700)?;
            let copy = pp::join(&pp::join(&shown, &leaf), &src_name);
            let inner = (|| -> Result<(), Fail> {
                let leaf_fd = open_dir(&leaf, Some(&held), &pp::join(&shown, &leaf))?;
                ferb_fs::hooks::at("staging.copy");
                let digest = match ferb_fs::recovery::copy_into(src_fd, leaf_fd.raw(), &src_name) {
                    Ok(d) => d,
                    Err(e) if e.is(libc::EEXIST) => {
                        return Err(Fail::Failed(format!(
                        "{copy} was already taken (a link or a file put there); nothing was copied"
                    )))
                    }
                    Err(e) => return Err(Fail::Os(e)),
                };
                ferb_fs::hooks::at("staging.copied");
                if digest != sha256 {
                    return Err(Fail::Failed(
                        "the file changed while FERB copied it; inspect it again".into(),
                    ));
                }
                if !names(&copy, &leaf_fd, &src_name) {
                    return Err(Fail::Failed(format!(
                        "{copy} no longer names FERB's copy (one of its folders was replaced)"
                    )));
                }
                Ok(())
            })();
            if let Err(e) = inner {
                // only the leaf just made, through its folder
                rmtree_at(held.raw(), &leaf, &mut |_, _| {});
                return Err(e);
            }
            Ok((copy, leaf))
        })();
        let (copy, leaf) = staged.map_err(|e| match e {
            Fail::NotOurs(p) => failed(format!(
                "{p} is not FERB's folder (a link or a file); nothing was copied"
            )),
            other => other.into_error(),
        })?;
        if let Some(tab) = tab.filter(|t| !t.is_empty()) {
            parts.push(leaf);
            self.lock()
                .shown
                .entry(tab.to_string())
                .or_default()
                .insert(parts);
        }
        Ok(copy)
    }

    /// `closed`: that tab is gone; the copies it showed need not stay. `true` when it showed a copy of a tree that
    /// already ended: the caller runs [`Uploads::collect`] (off the loop).
    pub fn closed(&self, tab: &str) -> bool {
        let mut s = self.lock();
        let leaves = s.shown.remove(tab).unwrap_or_default();
        leaves
            .iter()
            .any(|leaf| leaf.len() >= 2 && s.ended.contains(&leaf[leaf.len() - 2]))
    }

    /// `forget_tree`: the task tree ended: its copies go, except those a tab still open shows. A planted link is
    /// removed, never followed. Entries removed; `Err` (`CleanupLeft`) names what couldn't be, after removing the rest.
    pub fn forget_tree(&self, tree: &str) -> Result<usize, StagingError> {
        if !tree_name(tree) {
            return Ok(0);
        }
        self.lock().ended.insert(tree.to_string());
        let mut left = vec![];
        let gone = self.clean(tree, &mut left);
        if left.is_empty() {
            Ok(gone)
        } else {
            Err(left_error(left))
        }
    }

    /// `collect`: copies of trees that already ended that no open tab shows any more go.
    pub fn collect(&self) -> Result<usize, StagingError> {
        let mut trees: Vec<String> = self.lock().ended.iter().cloned().collect();
        trees.sort();
        let mut left = vec![];
        let gone = trees.iter().map(|t| self.clean(t, &mut left)).sum();
        if left.is_empty() {
            Ok(gone)
        } else {
            Err(left_error(left))
        }
    }

    /// `sweep`: at start-up every copy left here goes, a planted link included (removed, never followed). `Err` when
    /// `uploads/` is not FERB's folder, or names what couldn't be removed, after removing the rest.
    pub fn sweep(&self) -> Result<usize, StagingError> {
        {
            let mut s = self.lock();
            s.shown.clear();
            s.ended.clear();
        }
        let Some(root) = existing(&self.root, None, &self.root).map_err(Fail::into_error)? else {
            return Ok(0);
        };
        let (mut gone, mut left) = (0, vec![]);
        for entry in sys::scandir(root.raw()).map_err(os_error)? {
            let name = entry.name;
            let is_outside = name == OUTSIDE
                && sys::stat_at(At::Dir(root.raw()), &name, false)
                    .map_err(os_error)?
                    .is_dir();
            if is_outside {
                let shown = pp::join(&self.root, OUTSIDE);
                let Some(outside) =
                    existing(OUTSIDE, Some(&root), &shown).map_err(Fail::into_error)?
                else {
                    continue;
                };
                for e in sys::scandir(outside.raw()).map_err(os_error)? {
                    gone += remove(&outside, &e.name, &pp::join(&shown, &e.name), &mut left);
                }
            } else {
                gone += remove(&root, &name, &pp::join(&self.root, &name), &mut left);
            }
        }
        if left.is_empty() {
            Ok(gone)
        } else {
            Err(left_error(left))
        }
    }

    /// `_clean`: an ended tree's copies that no tab shows go, in both folders; the tree is done once no tab shows any.
    fn clean(&self, tree: &str, left: &mut Vec<String>) -> usize {
        let kept: HashSet<Leaf> = self
            .lock()
            .shown
            .values()
            .flatten()
            .filter(|leaf| leaf.len() >= 2 && leaf[leaf.len() - 2] == tree)
            .cloned()
            .collect();
        let (mut gone, mut keeping) = (0, false);
        let walked = (|| -> Result<(), Fail> {
            if let Some(root) = existing(&self.root, None, &self.root)? {
                let (g, k) = self.clean_in(&root, &[], tree, &kept, left);
                gone += g;
                keeping |= k;
                if let Some(outside) =
                    existing(OUTSIDE, Some(&root), &pp::join(&self.root, OUTSIDE))?
                {
                    let (g, k) = self.clean_in(&outside, &[OUTSIDE], tree, &kept, left);
                    gone += g;
                    keeping |= k;
                }
            }
            Ok(())
        })();
        if let Err(e) = walked {
            left.push(e.into_error().message);
        }
        if !keeping {
            self.lock().ended.remove(tree);
        }
        gone
    }

    /// `_clean_in`: `tree`'s folder in the open folder `dir` (`base` names it): copies no tab shows removed through
    /// it, and the folder too once empty. (entries removed, any copy kept).
    fn clean_in(
        &self,
        dir: &Fd,
        base: &[&str],
        tree: &str,
        kept: &HashSet<Leaf>,
        left: &mut Vec<String>,
    ) -> (usize, bool) {
        let mut shown = self.root.clone();
        for b in base {
            shown = pp::join(&shown, b);
        }
        let shown = pp::join(&shown, tree);
        let fd = match sys::stat_at(At::Dir(dir.raw()), tree, false) {
            Err(e) if e.is(libc::ENOENT) => return (0, false),
            Err(e) => {
                left.push(os_error(e).message);
                return (0, false);
            }
            // a planted link or file: removed, never followed
            Ok(st) if !st.is_dir() => return (remove(dir, tree, &shown, left), false),
            Ok(_) => match open_dir(tree, Some(dir), &shown) {
                Ok(fd) => fd,
                Err(Fail::Os(e)) if e.is(libc::ENOENT) => return (0, false),
                Err(e) => {
                    left.push(e.into_error().message);
                    return (0, false);
                }
            },
        };
        let (mut gone, mut keep, before) = (0, 0, left.len());
        match sys::scandir(fd.raw()) {
            Ok(entries) => {
                for e in entries {
                    let mut leaf: Leaf = base.iter().map(|b| b.to_string()).collect();
                    leaf.push(tree.to_string());
                    leaf.push(e.name.clone());
                    if kept.contains(&leaf) {
                        keep += 1;
                    } else {
                        gone += remove(&fd, &e.name, &pp::join(&shown, &e.name), left);
                    }
                }
            }
            Err(e) => left.push(os_error(e).message),
        }
        drop(fd);
        if keep == 0 && left.len() == before {
            // a copy staged meanwhile: its tree end removes it
            let _ = rmdir_at(dir.raw(), tree);
        }
        (gone, keep > 0)
    }

    /// `_discard`: copies just made for an upload that failed go at once, through their folders (best effort).
    fn discard(&self, copies: &[String]) {
        for copy in copies {
            let rel = pp::relative_to(copy, &self.root);
            let mut leaf: Leaf = pp::tail(&rel).iter().map(|s| s.to_string()).collect();
            leaf.pop();
            for leaves in self.lock().shown.values_mut() {
                leaves.remove(&leaf);
            }
            let Some((last, folders)) = leaf.split_last() else {
                continue;
            };
            let walked = (|| -> Result<(), Fail> {
                let mut held = open_dir(&self.root, None, &self.root)?;
                let mut shown = self.root.clone();
                for name in folders {
                    shown = pp::join(&shown, name);
                    held = open_dir(name, Some(&held), &shown)?;
                }
                remove(&held, last, &pp::parent(copy), &mut vec![]);
                Ok(())
            })();
            let _ = walked;
        }
    }
}

/// `Path.mkdir(parents=True, exist_ok=True)` (mode 0o777).
fn mkdir_parents(path: &str) -> sys::Res<()> {
    match sys::mkdir_at(At::Cwd, path, 0o777) {
        Ok(()) => Ok(()),
        Err(e) if e.is(libc::ENOENT) => {
            let parent = pp::parent(path);
            if parent == path {
                return Err(e);
            }
            mkdir_parents(&parent)?;
            match sys::mkdir_at(At::Cwd, path, 0o777) {
                Ok(()) => Ok(()),
                Err(e) if pp::is_dir(path) => {
                    let _ = e;
                    Ok(())
                }
                Err(e) => Err(e),
            }
        }
        Err(e) if pp::is_dir(path) => {
            let _ = e;
            Ok(())
        }
        Err(e) => Err(e),
    }
}

impl crate::ops::Staging for Uploads {
    fn stage_all(
        &self,
        tree: &str,
        sources: &[(i32, String, String)],
        tab: &str,
    ) -> Result<Vec<String>, String> {
        Uploads::stage_all(self, tree, sources, Some(tab)).map_err(|e| e.message)
    }

    fn closed(&self, tab: &str) -> bool {
        Uploads::closed(self, tab)
    }
}
