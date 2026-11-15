//! The file-identity half of `ferb/policy/protection.py` (protection.py:146-298): how a path a call names is spelled
//! for the protection lists, as typed, as the OS reaches it now and as the kernel names the file it reaches (F_GETPATH
//! on its deepest existing folder or regular file, opened for its name only and never through a link). The lists
//! themselves (entries, rules, questions) are `ferb-policy`'s; this is the descriptor work they rest on, so another
//! spelling of a protected file (a link, the Data volume's firmlink, `/.vol/<dev>/<ino>`) is seen as what it is.

use unicode_normalization::UnicodeNormalization;

use crate::pypath as pp;
use crate::sys::{self, At, St};

/// Every firmlinked folder (/Users, /Applications, /private...) is here too.
pub const DATA_VOLUME: &str = "/System/Volumes/Data";
/// `/.vol/<device>/<inode>/...`: any file by its identity.
pub const VOLFS: &str = "/.vol";

/// `_absolute`: a relative path is the current folder's.
pub fn absolute(path: &str) -> String {
    if pp::is_abs(path) {
        pp::norm(path)
    } else {
        pp::join(&pp::abspath("."), path)
    }
}

/// `_typed`: absolute, `.` and `..` folded as text, no link followed.
pub fn typed(path: &str) -> String {
    pp::normpath(&absolute(path))
}

/// `_real`: the path as the OS reaches it (`files.real_path`: links first, then `..`); its typed form when it can't be
/// resolved (a link loop), which keeps the rule rather than dropping it.
pub fn real(path: &str, home: &str) -> String {
    crate::files::real_path(&absolute(path), "/", home).unwrap_or_else(|_| typed(path))
}

/// `_opened_name`: F_GETPATH of a folder or regular file opened for its name only (O_EVTONLY, never through a link),
/// when what opened is what FERB looked at (`seen`).
pub fn opened_name(path: &str, folder: bool, seen: &St) -> Option<String> {
    let flags = sys::O_EVTONLY
        | sys::O_NONBLOCK
        | sys::O_CLOEXEC
        | sys::O_NOFOLLOW
        | if folder { sys::O_DIRECTORY } else { 0 };
    let fd = sys::open(At::Cwd, path, flags, 0).ok()?;
    let st = sys::fstat(fd.raw()).ok()?;
    if st.id() == seen.id() {
        sys::opened_path(fd.raw()).ok()
    } else {
        None
    }
}

/// `_kernel`: every link followed by name first (realpath), then F_GETPATH on the deepest part that exists, then the
/// rest. Only a folder or a regular file is ever opened. None when nothing along it can be named.
pub fn kernel(path: &str) -> Option<String> {
    if path.contains('\0') {
        return None;
    }
    let mut tail: Vec<String> = Vec::new();
    let mut p = pp::norm(&pp::realpath(&absolute(path)));
    loop {
        let st = match sys::lstat(&p) {
            Ok(st) => Some(st),
            Err(e)
                if [libc::ENOENT, libc::ENOTDIR, libc::EACCES, libc::EPERM].contains(&e.errno) =>
            {
                None
            }
            Err(_) => return None,
        };
        let named = match &st {
            Some(st) if st.is_dir() => opened_name(&p, true, st),
            Some(st) if st.is_reg() => opened_name(&p, false, st),
            _ => None,
        };
        if let Some(named) = named {
            let joined = pp::join_all(&named, tail.iter().rev().map(String::as_str));
            return Some(pp::normpath(&joined));
        }
        if st.is_some_and(|s| s.is_dir()) || p == pp::parent(&p) {
            return None;
        }
        tail.push(pp::name(&p));
        p = pp::parent(&p);
    }
}

/// `_firmlinked`: the firmlinked spelling of a path written through the Data volume, from the text alone.
pub fn firmlinked(path: &str) -> Option<String> {
    let t = typed(path);
    if pp::is_relative_to(&t, DATA_VOLUME) && t != DATA_VOLUME {
        Some(pp::join("/", &pp::relative_to(&t, DATA_VOLUME)))
    } else {
        None
    }
}

/// `_forms`: as typed, as resolved now, as the kernel names it and in its firmlinked spelling, each once.
pub fn forms(path: &str, home: &str) -> Vec<String> {
    let t = typed(path);
    let mut out = vec![t.clone(), real(path, home)];
    if let Some(k) = kernel(path) {
        out.push(k);
    }
    if let Some(f) = firmlinked(&t) {
        out.push(f);
    }
    let mut seen = std::collections::HashSet::new();
    out.retain(|p| seen.insert(p.clone()));
    out
}

/// `_unmapped`: a volfs path the kernel can't name (which file it means is unknown).
pub fn unmapped(path: &str) -> bool {
    if !pp::is_relative_to(&typed(path), VOLFS) {
        return false;
    }
    match kernel(path) {
        None => true,
        Some(k) => pp::is_relative_to(&k, VOLFS),
    }
}

/// `_key`: a path as a Mac volume compares it (NFC, casefolded, part by part).
pub fn key(path: &str) -> Vec<String> {
    let p = pp::norm(path);
    let mut parts = Vec::new();
    if pp::is_abs(&p) {
        parts.push(if p.starts_with("//") && !p.starts_with("///") {
            "//".to_string()
        } else {
            "/".to_string()
        });
    }
    for part in pp::tail(&p) {
        parts.push(crate::casefold(&part.nfc().collect::<String>()));
    }
    parts
}

/// `_under`: `path` is `root` or inside it, as the volume compares names.
pub fn under(path: &str, root: &str) -> bool {
    let (p, r) = (key(path), key(root));
    p.len() >= r.len() && p[..r.len()] == r[..]
}
