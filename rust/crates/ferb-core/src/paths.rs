//! FERB's runtime locations (ferb/paths.py) and the shipping host's rule for FERB_HOME
//! (ui/mac/FERBAgent/main.c:49-55,108-116).
//!
//! Two rules exist and both are kept. Python's `ferb_home()` takes any FERB_HOME (tests, a second FERB side by
//! side). FERB.app's agent host refuses to start unless FERB_HOME is unset, empty or names the account's own
//! `~/.ferb` (from the account database, never $HOME): [`shipping_home`]. Which one a Rust daemon applies depends
//! on how it was started and is decided by the host port.

use std::ffi::{CStr, OsString};
use std::os::unix::ffi::OsStringExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

/// FERB.app's bundle id (paths.py:27).
pub const APP_ID: &str = "com.ferb.FERB";

/// What `pathlib.Path(text)` keeps of a path's text on POSIX: repeated slashes and `.` parts dropped, no trailing
/// slash, exactly two leading slashes kept, the empty path written `.`. `..` stays.
pub fn py_path(text: &str) -> PathBuf {
    let root = match text.len() - text.trim_start_matches('/').len() {
        0 => "",
        2 => "//",
        _ => "/",
    };
    let parts: Vec<&str> = text
        .split('/')
        .filter(|p| !p.is_empty() && *p != ".")
        .collect();
    let joined = format!("{root}{}", parts.join("/"));
    PathBuf::from(if joined.is_empty() {
        ".".to_string()
    } else {
        joined
    })
}

/// paths.py:8 with the environment passed in: `given` is FERB_HOME (None: unset), `user_home` is `Path.home()`.
/// An empty FERB_HOME is the current folder (`Path("")`), as in Python; the shipping host reads it as unset.
pub fn ferb_home_from(given: Option<&str>, user_home: &Path) -> PathBuf {
    match given {
        Some(given) => py_path(given),
        None => user_home.join(".ferb"),
    }
}

/// `~/.ferb`, or `$FERB_HOME` (paths.py:8). `Path.home()` is $HOME, else the account database's folder.
pub fn ferb_home() -> PathBuf {
    let given = std::env::var_os("FERB_HOME");
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .or_else(account_home)
        .unwrap_or_default();
    ferb_home_from(given.as_deref().and_then(|g| g.to_str()), &home)
}

/// paths.py:13: `<home>/run/ferb.sock`.
pub fn socket_path(home: &Path) -> PathBuf {
    py_path(&home.join("run/ferb.sock").to_string_lossy())
}

/// The account's home folder from the account database (`getpwuid(getuid())`), never $HOME.
pub fn account_home() -> Option<PathBuf> {
    let mut buf = vec![0 as libc::c_char; 16 * 1024];
    let mut pw = std::mem::MaybeUninit::<libc::passwd>::uninit();
    let mut found: *mut libc::passwd = std::ptr::null_mut();
    // SAFETY: every pointer is to a live buffer of the stated size; `found` is null or points at `pw`, whose
    // strings live in `buf`, and both outlive the read below.
    let dir = unsafe {
        let rc = libc::getpwuid_r(
            libc::getuid(),
            pw.as_mut_ptr(),
            buf.as_mut_ptr(),
            buf.len(),
            &mut found,
        );
        if rc != 0 || found.is_null() || (*found).pw_dir.is_null() {
            return None;
        }
        CStr::from_ptr((*found).pw_dir).to_bytes().to_vec()
    };
    (!dir.is_empty()).then(|| PathBuf::from(OsString::from_vec(dir)))
}

/// FERB_HOME named a folder that isn't the account's own `~/.ferb`.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("FERB's background agent runs only with your own FERB home: FERB_HOME isn't your ~/.ferb.")]
pub struct ForeignHome;

/// main.c:50-55 `owners_home`: FERB_HOME unset or empty, or the same folder (device and inode) as `owner`.
pub fn owners_home(given: Option<&str>, owner: &Path) -> bool {
    match given {
        None | Some("") => true,
        Some(given) => match (std::fs::metadata(given), std::fs::metadata(owner)) {
            (Ok(a), Ok(b)) => a.dev() == b.dev() && a.ino() == b.ino(),
            _ => false,
        },
    }
}

/// The shipping host's home: the account's own `~/.ferb`, refusing any other FERB_HOME (and an account the
/// database doesn't know). `given` is FERB_HOME; `account` is [`account_home`].
pub fn shipping_home(given: Option<&str>, account: Option<&Path>) -> Result<PathBuf, ForeignHome> {
    let owner = account.ok_or(ForeignHome)?.join(".ferb");
    if owners_home(given, &owner) {
        Ok(owner)
    } else {
        Err(ForeignHome)
    }
}

/// paths.py:17: the FERB.app whose bundled runtime sits at `prefix` (`<app>/Contents/Resources/python`); None from
/// a checkout or `~/.ferb/venv`. Python's prefix is `sys.prefix`; where the Rust daemon sits in the bundle is the
/// host port's to say, so the prefix is always given here. Symbolic links are followed first; on a case-insensitive
/// volume this also reads `Python` as `python`, which Python's `resolve()` does not.
pub fn app_bundle(prefix: &Path) -> Option<PathBuf> {
    let p = prefix
        .canonicalize()
        .unwrap_or_else(|_| prefix.to_path_buf());
    let resources = p.parent()?;
    let contents = resources.parent()?;
    let app = contents.parent()?;
    let named = |path: &Path, name: &str| path.file_name().is_some_and(|n| n == name);
    (named(&p, "python")
        && named(resources, "Resources")
        && named(contents, "Contents")
        && app.extension().is_some_and(|e| e == "app"))
    .then(|| app.to_path_buf())
}

/// paths.py:30: the installed FERB.app, for a daemon that doesn't run from one: the first candidate whose
/// Info.plist carries FERB's bundle id. An unreadable or malformed plist is skipped.
pub fn installed_app_in(candidates: &[PathBuf]) -> Option<PathBuf> {
    candidates
        .iter()
        .find(|app| bundle_id(app).as_deref() == Some(APP_ID))
        .cloned()
}

/// `/Applications/FERB.app`, then `~/Applications/FERB.app`.
pub fn installed_app() -> Option<PathBuf> {
    let mut candidates = vec![PathBuf::from("/Applications/FERB.app")];
    if let Some(home) = std::env::var_os("HOME")
        .map(PathBuf::from)
        .or_else(account_home)
    {
        candidates.push(home.join("Applications").join("FERB.app"));
    }
    installed_app_in(&candidates)
}

fn bundle_id(app: &Path) -> Option<String> {
    let info = plist::Value::from_file(app.join("Contents").join("Info.plist")).ok()?;
    Some(
        info.as_dictionary()?
            .get("CFBundleIdentifier")?
            .as_string()?
            .to_string(),
    )
}
